mod cli;
mod duration;
mod watch;

use clap::Parser;
use cli::{Cli, Command};
use zemon_core::config::{ConnectMode, ZemonConfig};
use zemon_core::error::ZemonError;
use std::time::Duration;

/// Pick the most useful locator to display: prefer tcp/IPv4 non-loopback,
/// then tcp/anything, then the first one. Zenoh peers typically advertise
/// many interfaces, so picking one keeps the table readable.
fn pick_best_locator(locators: &[String]) -> Option<&str> {
    let score = |loc: &str| -> i32 {
        if !loc.starts_with("tcp/") {
            return 0;
        }
        let addr = &loc[4..];
        let is_ipv6 = addr.starts_with('[');
        let is_link_local = addr.starts_with("[fe80") || addr.starts_with("fe80");
        let is_loopback = addr.starts_with("127.") || addr.starts_with("[::1]");
        let is_tailscale =
            addr.starts_with("100.") || addr.starts_with("[fd7a:115c:a1e0");
        if is_link_local || is_loopback {
            return 1;
        }
        let mut s = 10;
        if !is_ipv6 {
            s += 10;
        }
        if !is_tailscale {
            s += 5;
        }
        s
    };
    locators
        .iter()
        .max_by_key(|l| score(l))
        .map(|s| s.as_str())
}

fn print_scout_results(
    results: &[zemon_core::types::PortScoutResult],
    start: u16,
    end: u16,
    per_port_timeout: Duration,
) {
    let hits: Vec<_> = results.iter().filter(|r| !r.nodes.is_empty()).collect();
    if hits.is_empty() {
        println!(
            "No Zenoh nodes found on scouting ports {}-{} ({} per port)",
            start,
            end,
            humantime::format_duration(per_port_timeout)
        );
        return;
    }

    let total: usize = hits.iter().map(|r| r.nodes.len()).sum();

    for (i, result) in hits.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", scouting_port_heading(result.port, result.nodes.len()));
        println!("{}", "─".repeat(78));
        println!("  {:<8} {:<34} {}", "TYPE", "ZID", "LOCATOR");
        for node in &result.nodes {
            let loc = pick_best_locator(&node.locators).unwrap_or("(none)");
            let zid_short = if node.zid.len() > 32 {
                format!("{}…", &node.zid[..31])
            } else {
                node.zid.clone()
            };
            println!("  {:<8} {:<34} {}", node.whatami, zid_short, loc);
        }
    }

    println!();
    println!(
        "Scanned scouting ports {}-{} · {} node{} on {} port{}",
        start,
        end,
        total,
        if total == 1 { "" } else { "s" },
        hits.len(),
        if hits.len() == 1 { "" } else { "s" }
    );
}

fn scouting_port_heading(port: u16, node_count: usize) -> String {
    format!(
        "Scouting port {}  ({} node{})",
        port,
        node_count,
        if node_count == 1 { "" } else { "s" }
    )
}

/// Print the nodes table (header, rows, count footer). `note`, when present,
/// is appended to the footer (e.g. "refreshing every 3s" in watch mode).
fn print_nodes_table(nodes: &[zemon_core::types::NodeInfo], note: Option<&str>) {
    println!("{:<40} {:<10} {}", "ZID", "KIND", "LOCATORS");
    println!("{}", "-".repeat(70));
    for node in nodes {
        println!(
            "{:<40} {:<10} {}",
            node.zid,
            node.kind,
            node.locators.join(", ")
        );
    }
    match note {
        Some(n) => println!("\n{} node(s) — {}", nodes.len(), n),
        None => println!("\n{} node(s)", nodes.len()),
    }
}

fn parse_port_range(s: &str) -> Result<(u16, u16), ZemonError> {
    let (start_s, end_s) = s
        .split_once('-')
        .ok_or_else(|| ZemonError::invalid_input(format!("port range must be START-END, got '{}'", s)))?;
    let start: u16 = start_s
        .trim()
        .parse()
        .map_err(|e| ZemonError::invalid_input(format!("invalid start port '{}': {}", start_s, e)))?;
    let end: u16 = end_s
        .trim()
        .parse()
        .map_err(|e| ZemonError::invalid_input(format!("invalid end port '{}': {}", end_s, e)))?;
    if start > end {
        return Err(ZemonError::invalid_input(format!(
            "start port {} must be <= end port {}",
            start, end
        )));
    }
    Ok((start, end))
}

fn build_config(cli: &Cli) -> ZemonConfig {
    let mut cfg = ZemonConfig::from_env();

    // CLI flags override env
    cfg.endpoint = cli.endpoint.clone();
    cfg.mode = match cli.mode.as_str() {
        "peer" => ConnectMode::Peer,
        _ => ConnectMode::Client,
    };
    if cli.namespace.is_some() {
        cfg.namespace = cli.namespace.clone();
    }
    if cli.config.is_some() {
        cfg.config_file = cli.config.clone();
    }
    if cli.scout_port.is_some() {
        cfg.scout_port = cli.scout_port;
    }
    if cli.connect_timeout.is_some() {
        cfg.connect_timeout = cli.connect_timeout;
    }

    cfg
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let is_tui = matches!(cli.command, Command::Tui { .. });
    let is_json = cli.json;

    // In JSON mode the only permitted stderr output is the single structured
    // error object, so we must not install color_eyre's colored/backtrace hook.
    if !is_json {
        let _ = color_eyre::install();
    }

    // TUI mode: suppress logs to avoid corrupting the terminal display.
    // JSON mode: suppress logs to keep stderr a clean single JSON error.
    // Plain CLI mode: show logs on stderr as normal.
    let default_filter = if is_tui || is_json {
        "off"
    } else {
        "zemon=info,zenoh=warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();

    let config = build_config(&cli);

    if let Err(e) = run(cli, config).await {
        if is_json {
            eprintln!("{}", e.to_json());
        } else {
            eprintln!("Error: {}", e);
        }
        std::process::exit(e.exit_code());
    }
}

async fn run(cli: Cli, config: ZemonConfig) -> Result<(), ZemonError> {
    match cli.command {
        Command::Discover { key_expr } => {
            let session = zemon_core::session::open_session(&config).await?;
            let topics = zemon_core::discover::discover(&session, &key_expr).await?;

            if cli.json {
                println!("{}", zemon_core::output::to_collection_json(&topics)?);
            } else if topics.is_empty() {
                println!("No active keys found for '{}'", key_expr);
            } else {
                for topic in &topics {
                    println!("{}", topic.key_expr);
                }
                println!("\n{} key(s) found", topics.len());
            }
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Sub {
            key_expr,
            pretty,
            timestamp,
            count,
            duration,
        } => {
            let session = zemon_core::session::open_session(&config).await?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let _handle = zemon_core::subscriber::subscribe(&session, &key_expr, tx).await?;

            if !cli.json {
                eprintln!("Subscribing to '{}' ... (Ctrl+C to stop)", key_expr);
            }

            // Duration clock starts now that the subscriber is declared.
            let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
            loop {
                let deadline = budget.deadline();
                tokio::select! {
                    biased;
                    _ = watch::sleep_until_opt(deadline) => break,
                    _ = tokio::signal::ctrl_c() => {
                        if !cli.json {
                            eprintln!("\nStopped.");
                        }
                        break;
                    }
                    item = rx.recv() => match item {
                        Some(msg) => {
                            if cli.json {
                                println!("{}", serde_json::to_string(&msg)?);
                            } else {
                                let ts = if timestamp {
                                    msg.timestamp.as_deref().unwrap_or("--")
                                } else {
                                    ""
                                };
                                let payload_str = if pretty {
                                    match &msg.payload {
                                        zemon_core::types::MessagePayload::Json(v) => {
                                            serde_json::to_string_pretty(v)
                                                .unwrap_or_else(|_| format!("{}", msg.payload))
                                        }
                                        other => format!("{}", other),
                                    }
                                } else {
                                    format!("{}", msg.payload)
                                };

                                let att_str = msg.attachment.as_ref()
                                    .map(|a| format!(" [att: {}]", a))
                                    .unwrap_or_default();

                                if timestamp {
                                    println!("[{}] {} | {}{}", ts, msg.key_expr, payload_str, att_str);
                                } else {
                                    println!("{} | {}{}", msg.key_expr, payload_str, att_str);
                                }
                            }
                            if budget.record() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Query {
            key_expr,
            payload,
            timeout,
        } => {
            let session = zemon_core::session::open_session(&config).await?;
            let results = zemon_core::query::get(
                &session,
                &key_expr,
                payload.as_deref(),
                timeout,
            )
            .await?;

            if cli.json {
                println!("{}", zemon_core::output::to_collection_json(&results)?);
            } else if results.is_empty() {
                println!("No replies for '{}'", key_expr);
            } else {
                for msg in &results {
                    let att_str = msg.attachment.as_ref()
                        .map(|a| format!(" [att: {}]", a))
                        .unwrap_or_default();
                    println!("{} | {}{}", msg.key_expr, msg.payload, att_str);
                }
                println!("\n{} reply(ies)", results.len());
            }
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Nodes {
            watch,
            count,
            duration,
        } => {
            let session = zemon_core::session::open_session(&config).await?;

            if !watch {
                let nodes = zemon_core::registry::query_admin_nodes(&session).await?;
                if cli.json {
                    println!("{}", zemon_core::output::to_collection_json(&nodes)?);
                } else if nodes.is_empty() {
                    println!("No nodes discovered");
                } else {
                    print_nodes_table(&nodes, None);
                }
            } else {
                if !cli.json {
                    eprintln!("Watching for changes... (Ctrl+C to stop)");
                }
                // First interval tick fires immediately, so --count 1 emits one
                // snapshot and exits. Each snapshot is counted.
                let mut interval = tokio::time::interval(Duration::from_secs(3));
                let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
                loop {
                    let deadline = budget.deadline();
                    tokio::select! {
                        biased;
                        _ = watch::sleep_until_opt(deadline) => break,
                        _ = tokio::signal::ctrl_c() => {
                            if !cli.json {
                                eprintln!("\nStopped.");
                            }
                            break;
                        }
                        _ = interval.tick() => {
                            let updated =
                                zemon_core::registry::query_admin_nodes(&session).await?;
                            if cli.json {
                                // NDJSON: one collection envelope per snapshot,
                                // never ANSI.
                                println!(
                                    "{}",
                                    zemon_core::output::to_collection_json(&updated)?
                                );
                            } else {
                                print!("\x1B[2J\x1B[H");
                                print_nodes_table(&updated, Some("refreshing every 3s"));
                            }
                            if budget.record() {
                                break;
                            }
                        }
                    }
                }
            }
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Pub { key_expr, value, att } => {
            let session = zemon_core::session::open_session(&config).await?;
            let mut builder = session.put(&key_expr, value.clone());
            if let Some(ref att_json) = att {
                builder = builder.attachment(att_json.as_bytes());
            }
            builder
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
            if cli.json {
                // Action result on stdout; no duplicate stderr message.
                let attachment_bytes = att.as_ref().map(|a| a.as_bytes().len());
                println!(
                    "{}",
                    zemon_core::output::publish_accepted_json(
                        &key_expr,
                        value.as_bytes().len(),
                        attachment_bytes,
                    )?
                );
            } else if let Some(ref att_json) = att {
                eprintln!("Published to '{}': {} [att: {}]", key_expr, value, att_json);
            } else {
                eprintln!("Published to '{}': {}", key_expr, value);
            }
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Liveliness {
            key_expr,
            watch,
            count,
            duration,
        } => {
            let session = zemon_core::session::open_session(&config).await?;
            let tokens = zemon_core::discover::query_liveliness(&session, &key_expr).await?;

            // The initial token snapshot and the change events are different
            // shapes. In JSON watch mode we keep the stream a pure event NDJSON
            // by skipping the initial collection envelope; humans still see the
            // initial table.
            if cli.json {
                if !watch {
                    println!("{}", zemon_core::output::to_collection_json(&tokens)?);
                }
            } else if tokens.is_empty() {
                println!("No liveliness tokens found for '{}'", key_expr);
            } else {
                println!("{:<50} {:<20} {}", "KEY", "NAME", "SOURCE_ZID");
                println!("{}", "─".repeat(85));
                for token in &tokens {
                    let name = token.node_name().unwrap_or_default();
                    let zid = token.source_zid.as_deref().unwrap_or("-");
                    let status = if token.alive { "●" } else { "○" };
                    println!("{} {:<49} {:<20} {}", status, token.key_expr, name, zid);
                }
                println!("\n{} token(s)", tokens.len());
            }

            if watch {
                if !cli.json {
                    eprintln!("\nWatching liveliness changes... (Ctrl+C to stop)");
                }
                let sub = session
                    .liveliness()
                    .declare_subscriber(&key_expr)
                    .await
                    .map_err(|e| color_eyre::eyre::eyre!(e))?;
                let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
                loop {
                    let deadline = budget.deadline();
                    tokio::select! {
                        biased;
                        _ = watch::sleep_until_opt(deadline) => break,
                        _ = tokio::signal::ctrl_c() => {
                            if !cli.json {
                                eprintln!("Stopped.");
                            }
                            break;
                        }
                        res = sub.recv_async() => match res {
                            Ok(sample) => {
                                let source = sample.source_info()
                                    .map(|s| format!("{}", s.source_id().zid()))
                                    .unwrap_or_else(|| "-".to_string());
                                if cli.json {
                                    let event = serde_json::json!({
                                        "kind": format!("{:?}", sample.kind()),
                                        "key_expr": sample.key_expr().to_string(),
                                        "source_zid": source,
                                    });
                                    println!("{}", serde_json::to_string(&event)?);
                                } else {
                                    println!(
                                        "[{:?}] {} source_zid={}",
                                        sample.kind(),
                                        sample.key_expr(),
                                        source,
                                    );
                                }
                                if budget.record() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }

            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Scout {
            port_range,
            per_port_timeout,
        } => {
            let (start, end) = parse_port_range(&port_range)?;
            let results = zemon_core::scout::scout_port_range(
                &config,
                start,
                end,
                per_port_timeout,
            )
            .await?;

            if cli.json {
                let hits: Vec<_> = results.iter().filter(|r| !r.nodes.is_empty()).collect();
                println!("{}", zemon_core::output::to_collection_json(&hits)?);
            } else {
                print_scout_results(&results, start, end, per_port_timeout);
            }
        }

        Command::Info => {
            let session = zemon_core::session::open_session(&config).await?;
            let detail = zemon_core::info::session_info(&session).await?;

            if cli.json {
                // `info` is a single resource; wrap it as a one-element
                // collection for uniformity: {"count":1,"items":[{...}]}.
                println!(
                    "{}",
                    zemon_core::output::to_collection_json(std::slice::from_ref(&detail))?
                );
            } else {
                println!("Session ZID:  {}", detail.zid);
                println!("Mode:         {}", detail.mode);
                if detail.routers.is_empty() {
                    println!("Routers:      (none)");
                } else {
                    for (i, r) in detail.routers.iter().enumerate() {
                        if i == 0 {
                            println!("Routers:      {}", r);
                        } else {
                            println!("              {}", r);
                        }
                    }
                }
                if detail.peers.is_empty() {
                    println!("Peers:        (none)");
                } else {
                    for (i, p) in detail.peers.iter().enumerate() {
                        if i == 0 {
                            println!("Peers:        {}", p);
                        } else {
                            println!("              {}", p);
                        }
                    }
                }
            }
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Tui { refresh } => {
            zemon_tui::run(config, refresh).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scout_heading_describes_a_scouting_port_without_a_domain() {
        let heading = scouting_port_heading(7446, 2);

        assert_eq!(heading, "Scouting port 7446  (2 nodes)");
        assert!(!heading.to_lowercase().contains("domain"));
    }
}
