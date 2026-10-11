//! `zenmon sub`: stream messages on a key expression.

use zenmon_core::config::ZenmonConfig;
use zenmon_core::contract::Contract;
use zenmon_core::error::ZenmonError;
use zenmon_core::subscriber::MissReport;

use super::Ctx;
use crate::cli::SubArgs;
use crate::common::{
    contract_annotation, internal_err, load_contract_opt, warn_redundant_namespace, write_out,
};
use crate::watch;

pub async fn run(ctx: &Ctx, config: ZenmonConfig, args: SubArgs) -> Result<(), ZenmonError> {
    let SubArgs {
        key_expr,
        pretty,
        timestamp,
        count,
        duration,
        max_payload_bytes,
        detect_misses,
    } = args;
    warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
    let max_payload_bytes = max_payload_bytes.map(|n| n as usize);
    // Load before opening the session so a broken contract fails fast.
    let contract = load_contract_opt(&ctx.contract)?;
    let session = zenmon_core::session::open_session(&config).await?;
    // Lossy, not unbounded or bounded: a reader that stops (a paused
    // pager) must neither grow this process without limit nor push
    // back through Zenoh onto the publishers being watched.
    let (sink, mut rx, dropped) = zenmon_core::subscriber::MessageSink::lossy(SUB_QUEUE);
    // Misses are rare and tiny; without --detect-misses the channel simply
    // never yields.
    let (miss_tx, mut miss_rx) = tokio::sync::mpsc::unbounded_channel();
    let subscription = if detect_misses {
        zenmon_core::subscriber::subscribe_detecting_misses(&session, &key_expr, sink, miss_tx)
            .await?
    } else {
        drop(miss_tx);
        zenmon_core::subscriber::subscribe(&session, &key_expr, sink).await?
    };
    let mut missed: MissTally = MissTally::default();

    if !ctx.json {
        eprintln!("Subscribing to '{}' ... (Ctrl+C to stop)", key_expr);
    }

    // Buffered, flushed whenever the queue is drained: one syscall per
    // burst instead of per message, and no added latency when idle.
    let mut out = std::io::BufWriter::new(std::io::stdout());
    let mut rendered = String::new();
    // Duration clock starts now that the subscriber is declared.
    let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
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
            Some(miss) = miss_rx.recv() => {
                missed.add(&miss);
                rendered.clear();
                render_miss(&mut rendered, &miss, ctx.json)?;
                if !write_out(&mut out, &rendered, rx.is_empty())? {
                    break;
                }
            }
            item = rx.recv() => match item {
                Some(msg) => {
                    rendered.clear();
                    render_sub_message(
                        &mut rendered,
                        &msg,
                        ctx.json,
                        pretty,
                        timestamp,
                        max_payload_bytes,
                        contract.as_ref(),
                    )?;
                    if !write_out(&mut out, &rendered, rx.is_empty())? {
                        break; // reader went away (`| head`)
                    }
                    if budget.record() {
                        break;
                    }
                }
                None => break,
            }
        }
    }
    let _ = std::io::Write::flush(&mut out);
    subscription.stop().await;
    session.close().await.map_err(internal_err)?;
    if detect_misses {
        eprintln!("{}", missed.summary());
    }
    let dropped = dropped.get();
    if dropped > 0 {
        eprintln!(
            "warning: dropped {dropped} message(s) — output was read slower than \
             they arrived (queue limit {SUB_QUEUE})"
        );
    }
    Ok(())
}

/// Running totals for `--detect-misses`, for the summary on exit.
#[derive(Default)]
struct MissTally {
    samples: u64,
    sources: std::collections::HashSet<(String, u32)>,
}

impl MissTally {
    fn add(&mut self, miss: &MissReport) {
        self.samples += u64::from(miss.missed);
        self.sources
            .insert((miss.source_zid.clone(), miss.source_eid));
    }

    fn summary(&self) -> String {
        if self.samples == 0 {
            "no samples missed from sequence-numbered publishers".to_string()
        } else {
            format!(
                "missed {} sample(s) from {} publisher(s)",
                self.samples,
                self.sources.len()
            )
        }
    }
}

/// A miss, as a stream line: `{"miss":{...}}` in --json (told apart from
/// messages by its single `miss` key), a marked line otherwise.
fn render_miss(buf: &mut String, miss: &MissReport, json: bool) -> Result<(), ZenmonError> {
    if json {
        buf.push_str(&serde_json::to_string(
            &serde_json::json!({ "miss": miss }),
        )?);
        buf.push('\n');
    } else {
        buf.push_str(&format!(
            "!! missed {} sample(s) from {}:{}\n",
            miss.missed, miss.source_zid, miss.source_eid
        ));
    }
    Ok(())
}

/// Messages `sub` queues for a slow reader before it starts dropping.
const SUB_QUEUE: usize = 10_000;

/// Render one `sub` message into `buf` (newline-terminated lines).
fn render_sub_message(
    buf: &mut String,
    msg: &zenmon_core::types::ZenohMessage,
    json: bool,
    pretty: bool,
    timestamp: bool,
    max_payload_bytes: Option<usize>,
    contract: Option<&Contract>,
) -> Result<(), ZenmonError> {
    use std::fmt::Write;
    if json {
        // Fast path preserved byte-for-byte when neither a payload cap nor a
        // contract is in play.
        if max_payload_bytes.is_none() && contract.is_none() {
            buf.push_str(&serde_json::to_string(msg)?);
        } else {
            let mut v = serde_json::to_value(msg)?;
            if let Some(max) = max_payload_bytes {
                // Replace payload/attachment with capped previews so a large
                // message can't blow the output budget.
                v["payload"] = msg.payload.to_view_capped(max);
                if let Some(att) = &msg.attachment {
                    v["attachment"] = att.to_view_capped(max);
                }
            }
            if let Some(c) = contract {
                v["contract"] = serde_json::to_value(c.enrich(&msg.key_expr, &msg.encoding))?;
            }
            buf.push_str(&serde_json::to_string(&v)?);
        }
        buf.push('\n');
        return Ok(());
    }
    if let Some(c) = contract {
        let _ = writeln!(
            buf,
            "{}",
            contract_annotation(&c.enrich(&msg.key_expr, &msg.encoding), &msg.encoding)
        );
    }
    let payload_str = if pretty {
        msg.payload.pretty()
    } else {
        format!("{}", msg.payload)
    };
    let att_str = msg
        .attachment
        .as_ref()
        .map(|a| format!(" [att: {}]", a))
        .unwrap_or_default();
    if timestamp {
        let ts = msg.timestamp.as_deref().unwrap_or("--");
        let _ = writeln!(
            buf,
            "[{}] {} | {}{}",
            ts, msg.key_expr, payload_str, att_str
        );
    } else {
        let _ = writeln!(buf, "{} | {}{}", msg.key_expr, payload_str, att_str);
    }
    Ok(())
}
