//! `zenmon pub` and `zenmon queryable serve`: the commands that write.

use zenmon_core::config::ZenmonConfig;
use zenmon_core::error::ZenmonError;

use super::Ctx;
use crate::cli::{PubArgs, QueryableCommand};
use crate::common::{
    internal_err, resolve_payload_arg, resolve_payload_arg_bytes, warn_redundant_namespace,
};
use crate::watch;

/// How long `pub` waits for routing to report a matching subscriber before
/// concluding there is none. A match already known answers at once.
const MATCH_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

pub async fn publish(ctx: &Ctx, config: ZenmonConfig, args: PubArgs) -> Result<(), ZenmonError> {
    let PubArgs {
        key_expr,
        value,
        att,
        rate,
        count,
        duration,
        congestion,
        priority,
        express,
    } = args;
    let qos = zenmon_core::publish::PutQos {
        priority: priority.map(Into::into),
        congestion_control: congestion.map(Into::into),
        express,
    };
    warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
    let value = resolve_payload_arg(&value)?;
    let session = zenmon_core::session::open_session(&config).await?;
    let attachment_bytes = att.as_ref().map(|a| a.len());

    // Checked once, up front: a put nobody hears still succeeds, and that
    // silence is what this tells apart from a working publish.
    let matched = zenmon_core::publish::has_subscribers(&session, &key_expr, MATCH_SETTLE).await?;
    if !matched && !ctx.json {
        eprintln!(
            "note: no subscriber matches '{key_expr}' — this publishes to nobody \
             (check the key, or whether the consumer is running)"
        );
    }

    // Publish the same value/attachment once. Shared with the TUI's
    // publish path so the two cannot drift on attachments or error
    // mapping.
    let publish_once = || async {
        zenmon_core::publish::put_with(
            &session,
            &key_expr,
            // Same wire bytes either way: zenoh encodes a String as
            // its UTF-8, which is what `resolve_payload_arg` validated.
            value.clone().into_bytes(),
            att.as_ref().map(|a| a.as_bytes()),
            qos,
        )
        .await
    };

    match rate {
        // Fixed-rate loop: republish on each interval tick until the
        // count/duration budget is spent or Ctrl+C. Clap guarantees a
        // bound is present, so this loop always terminates.
        Some(hz) => {
            // The first interval tick fires immediately, so --count 1
            // publishes once and exits. Ticks are phase-locked to the
            // schedule (no drift from per-put latency).
            let mut interval = tokio::time::interval(crate::duration::rate_tick_interval(hz));
            let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
            let mut published: u64 = 0;
            if !ctx.json {
                eprintln!(
                    "Publishing to '{}' at {} Hz... (Ctrl+C to stop)",
                    key_expr, hz
                );
            }
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
                    _ = interval.tick() => {
                        publish_once().await?;
                        published += 1;
                        if budget.record() {
                            break;
                        }
                    }
                }
            }
            if ctx.json {
                // Single summary line on stdout; no duplicate stderr.
                println!(
                    "{}",
                    zenmon_core::output::publish_rate_summary_json(
                        &key_expr,
                        value.len(),
                        attachment_bytes,
                        matched,
                        published,
                        hz,
                    )?
                );
            } else {
                eprintln!("published {} message(s) at {} Hz", published, hz);
            }
        }
        // Default: single publish, output unchanged.
        None => {
            publish_once().await?;
            if ctx.json {
                // Action result on stdout; no duplicate stderr message.
                println!(
                    "{}",
                    zenmon_core::output::publish_accepted_json(
                        &key_expr,
                        value.len(),
                        attachment_bytes,
                        matched,
                    )?
                );
            } else if let Some(ref att_json) = att {
                eprintln!("Published to '{}': {} [att: {}]", key_expr, value, att_json);
            } else {
                eprintln!("Published to '{}': {}", key_expr, value);
            }
        }
    }
    session.close().await.map_err(internal_err)?;
    Ok(())
}

pub async fn queryable(
    ctx: &Ctx,
    config: ZenmonConfig,
    command: QueryableCommand,
) -> Result<(), ZenmonError> {
    let QueryableCommand::Serve {
        key_expr,
        reply,
        reply_key,
        encoding,
        count,
        duration,
        include_request,
        max_request_bytes,
    } = command;
    use zenmon_core::types::MessagePayload;

    // Resolve the fixed reply payload (@<path> / - / literal). Bytes,
    // not String, so a binary reply file survives as-is.
    let reply_bytes: Vec<u8> = resolve_payload_arg_bytes(&reply)?;
    let reply_key = zenmon_core::queryable::resolve_reply_key(&key_expr, reply_key.as_deref())?;
    let max_request = max_request_bytes.map(|n| n as usize).unwrap_or(1024);

    let session = zenmon_core::session::open_session(&config).await?;
    let queryable = session
        .declare_queryable(&key_expr)
        .await
        .map_err(internal_err)?;
    if !ctx.json {
        eprintln!(
            "Serving queryable on '{}' (reply key '{}')... (Ctrl+C to stop)",
            key_expr, reply_key
        );
    }

    let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
    let mut seq: u64 = 0;
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
            q = queryable.recv_async() => match q {
                Ok(query) => {
                    seq += 1;
                    let mut builder = query.reply(reply_key.as_str(), reply_bytes.clone());
                    if let Some(enc) = &encoding {
                        builder = builder.encoding(enc.as_str());
                    }
                    // A reply failure is fatal (structured error), per the contract.
                    builder.await.map_err(|e| {
                        ZenmonError::internal(format!("reply failed: {}", e))
                    })?;

                    if ctx.json {
                        let mut ev = serde_json::json!({
                            "event": "replied",
                            "key_expr": reply_key,
                            "request_seq": seq,
                            "reply_bytes": reply_bytes.len(),
                        });
                        if include_request {
                            ev["request_key_expr"] =
                                serde_json::json!(query.key_expr().to_string());
                            if let Some(zb) = query.payload() {
                                ev["request_payload"] = MessagePayload::from_zbytes(zb)
                                    .to_view_capped(max_request);
                            }
                        }
                        println!("{}", serde_json::to_string(&ev)?);
                    } else {
                        println!(
                            "replied #{} to '{}' ({} bytes)",
                            seq,
                            query.key_expr(),
                            reply_bytes.len()
                        );
                    }

                    if budget.record() {
                        stop = true;
                    }
                }
                Err(_) => break,
            }
        }
        if stop {
            break;
        }
    }

    queryable.undeclare().await.map_err(internal_err)?;
    session.close().await.map_err(internal_err)?;
    Ok(())
}
