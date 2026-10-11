//! `zenmon capture` and `zenmon replay`: record traffic to disk, play it back.

use std::time::Duration;

use zenmon_core::config::ZenmonConfig;
use zenmon_core::error::ZenmonError;

use super::Ctx;
use crate::cli::{CaptureArgs, ReplayArgs};
use crate::common::{internal_err, warn_redundant_namespace};
use crate::watch;

pub async fn capture(
    ctx: &Ctx,
    config: ZenmonConfig,
    args: CaptureArgs,
) -> Result<(), ZenmonError> {
    let CaptureArgs {
        key_expr,
        exclude,
        output,
        dir,
        compress,
        rotate_size,
        rotate_interval,
        max_total_size,
        max_age,
        count,
        duration,
    } = args;
    warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
    use zenmon_core::capture::{CaptureExclude, CaptureSink, Recorded, Recorder, Retention};

    // Validate before connecting: a bad exclude must not start a
    // capture that silently drops nothing (or everything).
    let exclude_exprs = exclude;
    let exclude = CaptureExclude::parse(&exclude_exprs)?;
    let session = zenmon_core::session::open_session(&config).await?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let subscription = zenmon_core::subscriber::subscribe(&session, &key_expr, tx).await?;

    let (sink, output_label) = if let Some(dir) = dir {
        let writer = zenmon_core::trace::SegmentWriter::open_with(
            dir.clone(),
            rotate_size,
            rotate_interval,
            compress,
        )?;
        if !ctx.json {
            eprintln!(
                "Recording '{}' to {} (rotating) ... (Ctrl+C to stop)",
                key_expr,
                dir.display()
            );
        }
        let label = dir.display().to_string();
        let sink = CaptureSink::Store {
            writer,
            dir,
            retention: Retention {
                max_total_size,
                max_age,
            },
        };
        (sink, label)
    } else {
        let out = output.expect("clap guarantees output or dir");
        let file = zenmon_core::trace::CaptureFileWriter::create(&out, compress).map_err(|e| {
            ZenmonError::invalid_input(format!("cannot create {}: {}", out.display(), e))
        })?;
        if !ctx.json {
            eprintln!(
                "Capturing '{}' to {} ... (Ctrl+C to stop)",
                key_expr,
                out.display()
            );
        }
        (CaptureSink::File(file), out.display().to_string())
    };
    let mut recorder = Recorder::new(exclude, sink);

    if !ctx.json && recorder.excludes_anything() {
        eprintln!("Excluding: {}", exclude_exprs.join(", "));
    }

    let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
    let mut stop = false;
    loop {
        let deadline = budget.deadline();
        tokio::select! {
            biased;
            _ = watch::sleep_until_opt(deadline) => break,
            _ = tokio::signal::ctrl_c() => {
                if !ctx.json {
                    eprintln!("\nStopped.");
                }
                break;
            }
            item = rx.recv() => match item {
                // Excluded messages don't count toward --count.
                Some(msg) => {
                    let recorded = recorder.record(&msg, std::time::SystemTime::now())?;
                    if matches!(recorded, Recorded::Written { .. }) && budget.record() {
                        stop = true;
                    }
                }
                None => break,
            }
        }
        if stop {
            break;
        }
    }
    // Flush the last records on any exit path (count/duration/Ctrl+C).
    recorder.finish()?;
    let (written, excluded) = (recorder.written(), recorder.excluded());

    if ctx.json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "ok": true,
                "captured": written,
                "excluded": excluded,
                "output": output_label,
            }))?
        );
    } else if !recorder.excludes_anything() {
        eprintln!("Captured {} record(s) to {}", written, output_label);
    } else {
        eprintln!(
            "Captured {} record(s) to {} ({} excluded)",
            written, output_label, excluded
        );
    }
    subscription.stop().await;
    session.close().await.map_err(internal_err)?;
    Ok(())
}

pub async fn replay(ctx: &Ctx, config: ZenmonConfig, args: ReplayArgs) -> Result<(), ZenmonError> {
    let ReplayArgs {
        input,
        speed,
        rate,
        key_prefix,
        dry_run,
    } = args;
    use tokio::time::Instant;
    use zenmon_core::capture::CaptureRecord;

    // Plain or zstd, detected from the file itself.
    let reader = zenmon_core::trace::open_capture_lines(&input).map_err(|e| {
        ZenmonError::invalid_input(format!("cannot open {}: {}", input.display(), e))
    })?;

    let session = if dry_run {
        None
    } else {
        Some(zenmon_core::session::open_session(&config).await?)
    };

    let replay_start = Instant::now();
    let mut published: u64 = 0;
    let mut seq: u64 = 0; // for fixed-rate scheduling

    for (i, line) in reader.enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let rec = CaptureRecord::parse_line(&line, i + 1)?;

        // Schedule this message (skip waiting in dry-run).
        if !dry_run {
            let target = match rate {
                Some(hz) => replay_start + Duration::from_secs_f64(seq as f64 / hz),
                None => {
                    replay_start
                        + Duration::from_secs_f64((rec.received_offset_ms as f64 / 1000.0) / speed)
                }
            };
            watch::sleep_until_opt(Some(target)).await;
        }
        seq += 1;

        let key = match &key_prefix {
            Some(p) => format!("{}/{}", p.trim_end_matches('/'), rec.key_expr),
            None => rec.key_expr.clone(),
        };
        let payload = rec.payload_bytes()?;
        let attachment = rec.attachment_bytes()?;

        if dry_run {
            if ctx.json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "event": "would_publish",
                        "key_expr": key,
                        "bytes": payload.len(),
                        "encoding": rec.encoding,
                    }))?
                );
            } else {
                println!("would publish {} ({} bytes)", key, payload.len());
            }
        } else {
            let s = session.as_ref().expect("session present when not dry-run");
            let mut builder = s.put(&key, payload).encoding(rec.encoding.as_str());
            if let Some(att) = attachment {
                builder = builder.attachment(att);
            }
            builder
                .await
                .map_err(|e| ZenmonError::internal(format!("publish failed: {}", e)))?;
        }
        published += 1;
    }

    if let Some(s) = session {
        s.close().await.map_err(internal_err)?;
    }
    if ctx.json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "ok": true,
                "published": published,
                "dry_run": dry_run,
            }))?
        );
    } else {
        eprintln!(
            "{} {} record(s) from {}",
            if dry_run { "Would replay" } else { "Replayed" },
            published,
            input.display()
        );
    }
    Ok(())
}
