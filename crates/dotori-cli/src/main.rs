mod cli;

use clap::Parser;
use cli::{Cli, Command};
use color_eyre::Result;
use dotori_core::config::{ConnectMode, DotoriConfig};
use std::time::Duration;

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

        Command::Scout { timeout } => {
            let nodes = dotori_core::scout::scout(
                &config,
                Duration::from_secs(timeout),
            )
            .await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&nodes)?);
            } else if nodes.is_empty() {
                println!("No Zenoh nodes found (scouted for {}s)", timeout);
            } else {
                println!("{:<40} {:<10} {}", "ZID", "TYPE", "LOCATORS");
                println!("{}", "-".repeat(70));
                for node in &nodes {
                    println!(
                        "{:<40} {:<10} {}",
                        node.zid,
                        node.whatami,
                        node.locators.join(", ")
                    );
                }
                println!("\n{} node(s) found", nodes.len());
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
