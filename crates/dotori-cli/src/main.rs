mod cli;

use clap::Parser;
use cli::{Cli, Command};
use color_eyre::Result;
use dotori_core::config::{ConnectMode, DotoriConfig};
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
    results: &[dotori_core::types::PortScoutResult],
    start: u16,
    end: u16,
    per_port_timeout: u64,
) {
    let hits: Vec<_> = results.iter().filter(|r| !r.nodes.is_empty()).collect();
    if hits.is_empty() {
        println!(
            "No Zenoh nodes found in ports {}-{} ({}s per port)",
            start, end, per_port_timeout
        );
        return;
    }

    let total: usize = hits.iter().map(|r| r.nodes.len()).sum();

    for (i, result) in hits.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let domain = result.port.saturating_sub(7446);
        println!(
            "Port {}  (domain {}, {} node{})",
            result.port,
            domain,
            result.nodes.len(),
            if result.nodes.len() == 1 { "" } else { "s" }
        );
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
        "Scanned {}-{} · {} node{} on {} port{}",
        start,
        end,
        total,
        if total == 1 { "" } else { "s" },
        hits.len(),
        if hits.len() == 1 { "" } else { "s" }
    );
}

fn parse_port_range(s: &str) -> Result<(u16, u16)> {
    let (start_s, end_s) = s
        .split_once('-')
        .ok_or_else(|| color_eyre::eyre::eyre!("port range must be START-END, got '{}'", s))?;
    let start: u16 = start_s
        .trim()
        .parse()
        .map_err(|e| color_eyre::eyre::eyre!("invalid start port '{}': {}", start_s, e))?;
    let end: u16 = end_s
        .trim()
        .parse()
        .map_err(|e| color_eyre::eyre::eyre!("invalid end port '{}': {}", end_s, e))?;
    if start > end {
        return Err(color_eyre::eyre::eyre!(
            "start port {} must be <= end port {}",
            start,
            end
        ));
    }
    Ok((start, end))
}

fn build_config(cli: &Cli) -> DotoriConfig {
    let mut cfg = DotoriConfig::from_env();

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

    cfg
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    let is_tui = matches!(cli.command, Command::Tui { .. });

    // TUI mode: suppress all logs to avoid corrupting the terminal display
    // CLI mode: show logs on stderr as normal
    if is_tui {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "off".into()),
            )
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "dotori=info,zenoh=warn".into()),
            )
            .init();
    }

    let config = build_config(&cli);

    match cli.command {
        Command::Discover { key_expr } => {
            let session = dotori_core::session::open_session(&config).await?;
            let topics = dotori_core::discover::discover(&session, &key_expr).await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&topics)?);
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
        } => {
            let session = dotori_core::session::open_session(&config).await?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let _handle = dotori_core::subscriber::subscribe(&session, &key_expr, tx).await?;

            eprintln!("Subscribing to '{}' ... (Ctrl+C to stop)", key_expr);

            loop {
                tokio::select! {
                    Some(msg) = rx.recv() => {
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
                                    dotori_core::types::MessagePayload::Json(v) => {
                                        serde_json::to_string_pretty(v)?
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
                    }
                    _ = tokio::signal::ctrl_c() => {
                        eprintln!("\nStopped.");
                        break;
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
            let session = dotori_core::session::open_session(&config).await?;
            let results = dotori_core::query::get(
                &session,
                &key_expr,
                payload.as_deref(),
                Duration::from_millis(timeout),
            )
            .await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&results)?);
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

        Command::Nodes { watch } => {
            let session = dotori_core::session::open_session(&config).await?;
            let nodes = dotori_core::registry::query_admin_nodes(&session).await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&nodes)?);
            } else if nodes.is_empty() {
                println!("No nodes discovered");
            } else {
                println!("{:<40} {:<10} {}", "ZID", "KIND", "LOCATORS");
                println!("{}", "-".repeat(70));
                for node in &nodes {
                    println!(
                        "{:<40} {:<10} {}",
                        node.zid,
                        node.kind,
                        node.locators.join(", ")
                    );
                }
                println!("\n{} node(s)", nodes.len());
            }

            if watch {
                eprintln!("Watching for changes... (Ctrl+C to stop)");
                let mut interval = tokio::time::interval(Duration::from_secs(3));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let updated = dotori_core::registry::query_admin_nodes(&session).await?;
                            print!("\x1B[2J\x1B[H");
                            println!("{:<40} {:<10} {}", "ZID", "KIND", "LOCATORS");
                            println!("{}", "-".repeat(70));
                            for node in &updated {
                                println!(
                                    "{:<40} {:<10} {}",
                                    node.zid,
                                    node.kind,
                                    node.locators.join(", ")
                                );
                            }
                            println!("\n{} node(s) — refreshing every 3s", updated.len());
                        }
                        _ = tokio::signal::ctrl_c() => {
                            eprintln!("\nStopped.");
                            break;
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
            let session = dotori_core::session::open_session(&config).await?;
            let mut builder = session.put(&key_expr, value.clone());
            if let Some(ref att_json) = att {
                builder = builder.attachment(att_json.as_bytes());
            }
            builder
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
            if let Some(ref att_json) = att {
                eprintln!("Published to '{}': {} [att: {}]", key_expr, value, att_json);
            } else {
                eprintln!("Published to '{}': {}", key_expr, value);
            }
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Liveliness { key_expr, watch } => {
            let session = dotori_core::session::open_session(&config).await?;
            let tokens = dotori_core::discover::query_liveliness(&session, &key_expr).await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&tokens)?);
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
                eprintln!("\nWatching liveliness changes... (Ctrl+C to stop)");
                let sub = session
                    .liveliness()
                    .declare_subscriber(&key_expr)
                    .await
                    .map_err(|e| color_eyre::eyre::eyre!(e))?;
                loop {
                    tokio::select! {
                        Ok(sample) = sub.recv_async() => {
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
                        }
                        _ = tokio::signal::ctrl_c() => {
                            eprintln!("Stopped.");
                            break;
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
            let results = dotori_core::scout::scout_port_range(
                &config,
                start,
                end,
                Duration::from_secs(per_port_timeout),
            )
            .await?;

            if cli.json {
                let hits: Vec<_> = results.iter().filter(|r| !r.nodes.is_empty()).collect();
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else {
                print_scout_results(&results, start, end, per_port_timeout);
            }
        }

        Command::Info => {
            let session = dotori_core::session::open_session(&config).await?;
            let detail = dotori_core::info::session_info(&session).await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&detail)?);
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
            dotori_tui::run(config, refresh).await?;
        }
    }

    Ok(())
}
