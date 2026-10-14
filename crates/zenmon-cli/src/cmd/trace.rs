//! `zenmon trace`: read a capture store offline.

use zenmon_core::error::ZenmonError;

use super::Ctx;
use crate::cli::TraceCommand;

pub fn run(ctx: &Ctx, command: TraceCommand) -> Result<(), ZenmonError> {
    use zenmon_core::trace::{self, ReadFilter, ReadOptions};
    // Pure file reader — never opens a Zenoh session.
    let now = std::time::SystemTime::now();
    let parse_bound = |o: &Option<String>| -> Result<Option<std::time::SystemTime>, ZenmonError> {
        o.as_deref()
            .map(|s| trace::parse_time_bound(s, now))
            .transpose()
    };
    match command {
        TraceCommand::Verify { path } => {
            let report = zenmon_core::verify::verify(&path)?;
            if ctx.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!(
                    "{} files, {} valid records, {} issues",
                    report.files, report.records, report.issue_count
                );
                for issue in &report.issues {
                    println!(
                        "{}:{}: {}",
                        issue.path.display(),
                        issue.line.unwrap_or(0),
                        issue.message
                    );
                }
                println!("Checks stored data only; cannot prove network delivery or detect missing files without a manifest.");
            }
            if !report.valid {
                return Err(ZenmonError::invalid_input("trace integrity check failed"));
            }
        }
        TraceCommand::Stats {
            dir,
            key,
            since,
            until,
            top,
            max_payload_bytes,
        } => {
            let filter = ReadFilter {
                key,
                since: parse_bound(&since)?,
                until: parse_bound(&until)?,
            };
            let stats = trace::topic_stats(
                &dir,
                &filter,
                top.map(|n| n as usize),
                max_payload_bytes.map(|n| n as usize),
            )?;
            if ctx.json {
                println!("{}", zenmon_core::output::to_collection_json(&stats)?);
            } else if stats.is_empty() {
                println!("No records in store for '{}'", filter.key);
            } else {
                for s in &stats {
                    println!(
                        "{:<40} count={:<8} rate={:.2}Hz last={}",
                        s.key,
                        s.count,
                        s.rate_hz,
                        s.last_ts.as_deref().unwrap_or("-")
                    );
                }
            }
        }
        TraceCommand::Read {
            dir,
            key,
            since,
            until,
            limit,
            last_per_key,
            every,
            max_payload_bytes,
            cursor,
        } => {
            let filter = ReadFilter {
                key,
                since: parse_bound(&since)?,
                until: parse_bound(&until)?,
            };
            let opts = ReadOptions {
                filter,
                limit: if limit == 0 { None } else { Some(limit) },
                last_per_key,
                every,
                cursor,
            };
            let page = trace::read_page(&dir, &opts)?;
            let max = max_payload_bytes.map(|n| n as usize);
            if ctx.json {
                // NDJSON: one record per line, then a summary trailer line.
                for pr in &page.records {
                    let mut v = serde_json::to_value(&pr.record)?;
                    if let (Some(max), Some(obj)) = (max, v.as_object_mut()) {
                        // Re-cap the payload preview from raw bytes.
                        if let Ok(bytes) =
                            zenmon_core::capture::b64_decode_public(&pr.record.payload_base64)
                        {
                            let mp = zenmon_core::types::MessagePayload::from_bytes(bytes);
                            obj.insert("payload".to_string(), mp.to_view_capped(max));
                        }
                    }
                    println!("{}", serde_json::to_string(&v)?);
                }
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "summary": {
                            "returned": page.returned,
                            "matched": page.matched,
                            "truncated": page.truncated,
                            "cursor": page.cursor,
                        }
                    }))?
                );
            } else {
                for pr in &page.records {
                    println!(
                        "{}  {}",
                        pr.record.received_at.as_deref().unwrap_or("-"),
                        pr.record.key_expr
                    );
                }
                eprintln!(
                    "returned {} of {} matched{}",
                    page.returned,
                    page.matched,
                    if page.truncated {
                        " (more — pass --cursor to continue)"
                    } else {
                        ""
                    }
                );
            }
        }
    }
    Ok(())
}
