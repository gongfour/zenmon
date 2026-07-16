mod cli;
mod duration;
mod watch;

use clap::Parser;
use cli::{Cli, Command, ConfigCommand, QueryableCommand};
use zenmon_core::config::{
    resolve_config, ConfigOverrides, EffectiveConfig, ResolvedConfig, ResolvedValue,
};
use zenmon_core::error::ZenmonError;
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
    results: &[zenmon_core::types::PortScoutResult],
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
fn print_nodes_table(nodes: &[zenmon_core::types::NodeInfo], note: Option<&str>) {
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

fn parse_port_range(s: &str) -> Result<(u16, u16), ZenmonError> {
    let (start_s, end_s) = s
        .split_once('-')
        .ok_or_else(|| ZenmonError::invalid_input(format!("port range must be START-END, got '{}'", s)))?;
    let start: u16 = start_s
        .trim()
        .parse()
        .map_err(|e| ZenmonError::invalid_input(format!("invalid start port '{}': {}", start_s, e)))?;
    let end: u16 = end_s
        .trim()
        .parse()
        .map_err(|e| ZenmonError::invalid_input(format!("invalid end port '{}': {}", end_s, e)))?;
    if start > end {
        return Err(ZenmonError::invalid_input(format!(
            "start port {} must be <= end port {}",
            start, end
        )));
    }
    Ok((start, end))
}

fn build_config(cli: &Cli) -> Result<ResolvedConfig, ZenmonError> {
    // Resolution failures (bad mode, endpoint, scout port, connect timeout, or
    // config file) are all user-input errors, so collapse them to invalid_input
    // (exit 2) to keep the CLI's structured error / exit-code contract.
    resolve_config(ConfigOverrides {
        endpoint: cli.endpoint.clone(),
        mode: cli.mode.clone(),
        namespace: cli.namespace.clone(),
        config_file: cli.config.clone(),
        scout_port: cli.scout_port,
        connect_timeout: cli.connect_timeout,
    })
    .map_err(|e| ZenmonError::invalid_input(e.to_string()))
}

fn print_resolved<T: std::fmt::Display>(label: &str, value: &ResolvedValue<T>) {
    println!(
        "{:<16} {} ({})",
        format!("{}:", label),
        value.value,
        value.source
    );
}

fn print_optional_resolved(label: &str, value: &ResolvedValue<Option<String>>) {
    let rendered = value.value.as_deref().unwrap_or("(none)");
    println!("{:<16} {} ({})", format!("{}:", label), rendered, value.source);
}

fn print_effective_config(effective: &EffectiveConfig, json: bool) -> Result<(), ZenmonError> {
    if json {
        println!("{}", serde_json::to_string_pretty(effective)?);
    } else {
        print_resolved("Endpoint", &effective.endpoint);
        print_resolved("Mode", &effective.mode);
        print_optional_resolved("Namespace", &effective.namespace);
        let config_file = effective
            .config_file
            .value
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(none)".to_string());
        println!(
            "{:<16} {} ({})",
            "Config file:", config_file, effective.config_file.source
        );
        print_resolved("Scout port", &effective.scout_port);
        print_optional_resolved("Connect timeout", &effective.connect_timeout);
    }
    Ok(())
}

/// Default tracing filter for plain CLI mode.
const DEFAULT_LOG_FILTER: &str = "zenmon=info,zenoh=warn";

/// Resolve the tracing filter for the process.
///
/// In JSON or TUI mode logs are forced fully OFF regardless of `RUST_LOG`:
/// - JSON mode: stdout must carry only the structured JSON result and stderr
///   only the single structured error, so no log line may reach either stream.
///   Honoring `RUST_LOG` here leaks Zenoh's full `Config` debug output (which
///   includes authentication fields) and breaks the machine-readable contract.
/// - TUI mode: stray log output corrupts the ratatui display.
///
/// Only plain CLI mode consults `RUST_LOG`, falling back to a sensible default.
fn resolve_log_filter(
    is_tui: bool,
    is_json: bool,
    rust_log: Option<&str>,
) -> tracing_subscriber::EnvFilter {
    if is_tui || is_json {
        return tracing_subscriber::EnvFilter::new("off");
    }
    match rust_log {
        Some(spec) => tracing_subscriber::EnvFilter::try_new(spec)
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
        None => tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER),
    }
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

    tracing_subscriber::fmt()
        .with_env_filter(resolve_log_filter(
            is_tui,
            is_json,
            std::env::var("RUST_LOG").ok().as_deref(),
        ))
        .init();

    let emit_error = |e: ZenmonError| -> ! {
        if is_json {
            eprintln!("{}", e.to_json());
        } else {
            eprintln!("Error: {}", e);
        }
        std::process::exit(e.exit_code());
    };

    let resolved = match build_config(&cli) {
        Ok(resolved) => resolved,
        // A `config` command in JSON mode reports a resolution/validation
        // failure as a structured {"valid": false, ...} document (exit 2)
        // rather than the generic error envelope.
        Err(e) if is_json && matches!(&cli.command, Command::Config { .. }) => {
            let output = serde_json::json!({
                "valid": false,
                "error": { "code": "invalid_config", "message": e.to_string() },
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap_or_default());
            std::process::exit(2);
        }
        Err(e) => emit_error(e),
    };

    if let Err(e) = run(cli, resolved).await {
        emit_error(e);
    }
}

async fn run(cli: Cli, resolved: ResolvedConfig) -> Result<(), ZenmonError> {
    let config = resolved.config;
    match cli.command {
        Command::Config { command } => match command {
            ConfigCommand::Validate => {
                if cli.json {
                    let output = serde_json::json!({
                        "valid": true,
                        "config": resolved.effective,
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    println!("Configuration is valid.");
                }
            }
            ConfigCommand::Show { effective: _ } => {
                print_effective_config(&resolved.effective, cli.json)?;
            }
        },

        Command::Discover { key_expr } => {
            let session = zenmon_core::session::open_session(&config).await?;
            let topics = zenmon_core::discover::discover(&session, &key_expr).await?;

            if cli.json {
                println!("{}", zenmon_core::output::to_collection_json(&topics)?);
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
            max_payload_bytes,
        } => {
            let max_payload_bytes = max_payload_bytes.map(|n| n as usize);
            let session = zenmon_core::session::open_session(&config).await?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let _handle = zenmon_core::subscriber::subscribe(&session, &key_expr, tx).await?;

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
                                match max_payload_bytes {
                                    Some(max) => {
                                        // Replace payload/attachment with capped
                                        // previews so a large message can't blow
                                        // the output budget.
                                        let mut v = serde_json::to_value(&msg)?;
                                        v["payload"] = msg.payload.to_view_capped(max);
                                        if let Some(att) = &msg.attachment {
                                            v["attachment"] = att.to_view_capped(max);
                                        }
                                        println!("{}", serde_json::to_string(&v)?);
                                    }
                                    None => println!("{}", serde_json::to_string(&msg)?),
                                }
                            } else {
                                let ts = if timestamp {
                                    msg.timestamp.as_deref().unwrap_or("--")
                                } else {
                                    ""
                                };
                                let payload_str = if pretty {
                                    msg.payload.pretty()
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
            limit,
        } => {
            let limit = limit.map(|n| n as usize);
            let session = zenmon_core::session::open_session(&config).await?;
            let outcome = zenmon_core::query::get(
                &session,
                &key_expr,
                payload.as_deref(),
                timeout,
                limit,
            )
            .await?;
            let limited = limit.is_some_and(|l| outcome.replies.len() >= l);

            if cli.json {
                println!(
                    "{}",
                    zenmon_core::output::to_query_json(&outcome.replies, &outcome.errors, limited)?
                );
            } else if outcome.replies.is_empty() && outcome.errors.is_empty() {
                println!("No replies for '{}'", key_expr);
            } else {
                for msg in &outcome.replies {
                    let att_str = msg.attachment.as_ref()
                        .map(|a| format!(" [att: {}]", a))
                        .unwrap_or_default();
                    println!("{} | {}{}", msg.key_expr, msg.payload, att_str);
                }
                for err in &outcome.errors {
                    println!("error reply: {}", err.message);
                }
                println!(
                    "\n{} reply(ies), {} error(s)",
                    outcome.replies.len(),
                    outcome.errors.len()
                );
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
            changes_only,
        } => {
            use zenmon_core::nodediff::{diff_nodes, NodeSnapshot};

            let session = zenmon_core::session::open_session(&config).await?;

            if !watch {
                let nodes = zenmon_core::registry::query_admin_nodes(&session).await?;
                if cli.json {
                    println!("{}", zenmon_core::output::to_collection_json(&nodes)?);
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
                // changes-only baseline: None until the first snapshot seeds it,
                // so the initial state is not reported as a burst of "added".
                let mut prev: Option<Vec<NodeSnapshot>> = None;
                let mut done = false;
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
                                zenmon_core::registry::query_admin_nodes(&session).await?;
                            if changes_only {
                                let curr: Vec<NodeSnapshot> =
                                    updated.iter().map(NodeSnapshot::from_info).collect();
                                match &prev {
                                    None => {} // seed baseline below, emit nothing
                                    Some(prev_snap) => {
                                        for change in diff_nodes(prev_snap, &curr) {
                                            if cli.json {
                                                println!("{}", serde_json::to_string(&change)?);
                                            } else {
                                                println!("{}", change.describe());
                                            }
                                            if budget.record() {
                                                done = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                                prev = Some(curr);
                            } else if cli.json {
                                // NDJSON: one collection envelope per snapshot,
                                // never ANSI.
                                println!(
                                    "{}",
                                    zenmon_core::output::to_collection_json(&updated)?
                                );
                                if budget.record() {
                                    done = true;
                                }
                            } else {
                                print!("\x1B[2J\x1B[H");
                                print_nodes_table(&updated, Some("refreshing every 3s"));
                                if budget.record() {
                                    done = true;
                                }
                            }
                            if done {
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
            let session = zenmon_core::session::open_session(&config).await?;
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
                    zenmon_core::output::publish_accepted_json(
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
            changes_only,
        } => {
            let session = zenmon_core::session::open_session(&config).await?;

            // --changes-only suppresses the initial snapshot entirely (its only
            // effect for liveliness), leaving a pure join/leave event stream.
            let suppress_initial = watch && changes_only;
            if !suppress_initial {
                let tokens = zenmon_core::discover::query_liveliness(&session, &key_expr).await?;
                // In JSON watch mode we keep the stream a pure event NDJSON by
                // skipping the initial collection envelope; humans still see the
                // initial table.
                if cli.json {
                    if !watch {
                        println!("{}", zenmon_core::output::to_collection_json(&tokens)?);
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
            let results = zenmon_core::scout::scout_port_range(
                &config,
                start,
                end,
                per_port_timeout,
            )
            .await?;

            if cli.json {
                let hits: Vec<_> = results.iter().filter(|r| !r.nodes.is_empty()).collect();
                println!("{}", zenmon_core::output::to_collection_json(&hits)?);
            } else {
                print_scout_results(&results, start, end, per_port_timeout);
            }
        }

        Command::Info => {
            let session = zenmon_core::session::open_session(&config).await?;
            let detail = zenmon_core::info::session_info(&session, config.mode).await?;

            if cli.json {
                // `info` is a single resource; wrap it as a one-element
                // collection for uniformity: {"count":1,"items":[{...}]}.
                println!(
                    "{}",
                    zenmon_core::output::to_collection_json(std::slice::from_ref(&detail))?
                );
            } else {
                println!("Session ZID:  {}", detail.zid);
                println!("Mode:         {}", detail.mode);
                println!(
                    "Connected:    {}",
                    if detail.connected { "yes" } else { "no" }
                );
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

        Command::Doctor { timeout } => {
            let report = zenmon_core::doctor::run(&config, timeout).await;

            if cli.json {
                // A single result object on stdout; a failing diagnostic is a
                // successful report with status "fail", not an error envelope.
                println!("{}", serde_json::to_string(&report)?);
            } else {
                use zenmon_core::doctor::CheckStatus;
                for c in &report.checks {
                    let mark = match c.status {
                        CheckStatus::Pass => "PASS",
                        CheckStatus::Warn => "WARN",
                        CheckStatus::Fail => "FAIL",
                    };
                    print!("[{}] {:<11} {}ms", mark, c.name, c.latency_ms);
                    if let Some(m) = &c.message {
                        print!("  {}", m);
                    }
                    println!();
                    if let Some(h) = &c.hint {
                        if c.status != CheckStatus::Pass {
                            println!("       hint: {}", h);
                        }
                    }
                }
                println!("\nOverall: {:?}", report.status);
            }

            let code = report.exit_code();
            if code != 0 {
                std::process::exit(code);
            }
        }

        Command::Keyexpr { a, b } => {
            // Pure, offline: no session is opened.
            let rel = zenmon_core::keyexpr::compare(&a, &b)?;
            if cli.json {
                println!("{}", serde_json::to_string(&rel)?);
            } else {
                println!("A:             {}", rel.a);
                println!("B:             {}", rel.b);
                println!("intersects:    {}", rel.intersects);
                println!("A includes B:  {}", rel.a_includes_b);
                println!("B includes A:  {}", rel.b_includes_a);
                println!("equal:         {}", rel.equal);
                println!("relation:      {:?}", rel.relation);
            }
        }

        Command::Capture {
            key_expr,
            output,
            count,
            duration,
        } => {
            use std::io::Write;
            use zenmon_core::capture::CaptureRecord;

            let session = zenmon_core::session::open_session(&config).await?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let _handle = zenmon_core::subscriber::subscribe(&session, &key_expr, tx).await?;

            let file = std::fs::File::create(&output).map_err(|e| {
                ZenmonError::invalid_input(format!("cannot create {}: {}", output.display(), e))
            })?;
            let mut writer = std::io::BufWriter::new(file);
            let start = std::time::Instant::now();
            if !cli.json {
                eprintln!(
                    "Capturing '{}' to {} ... (Ctrl+C to stop)",
                    key_expr,
                    output.display()
                );
            }

            let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
            let mut written: u64 = 0;
            let mut stop = false;
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
                            let rec = CaptureRecord::from_message(&msg, start.elapsed());
                            let line = serde_json::to_string(&rec)?;
                            writeln!(writer, "{}", line).map_err(|e| {
                                ZenmonError::internal(format!("write failed: {}", e))
                            })?;
                            written += 1;
                            if budget.record() {
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
            writer
                .flush()
                .map_err(|e| ZenmonError::internal(format!("flush failed: {}", e)))?;

            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "ok": true,
                        "captured": written,
                        "output": output.display().to_string(),
                    }))?
                );
            } else {
                eprintln!("Captured {} record(s) to {}", written, output.display());
            }
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Replay {
            input,
            speed,
            rate,
            key_prefix,
            dry_run,
        } => {
            use std::io::BufRead;
            use tokio::time::Instant;
            use zenmon_core::capture::CaptureRecord;

            let file = std::fs::File::open(&input).map_err(|e| {
                ZenmonError::invalid_input(format!("cannot open {}: {}", input.display(), e))
            })?;
            let reader = std::io::BufReader::new(file);

            let session = if dry_run {
                None
            } else {
                Some(zenmon_core::session::open_session(&config).await?)
            };

            let replay_start = Instant::now();
            let mut published: u64 = 0;
            let mut seq: u64 = 0; // for fixed-rate scheduling

            for (i, line) in reader.lines().enumerate() {
                let line =
                    line.map_err(|e| ZenmonError::internal(format!("read failed: {}", e)))?;
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
                                + Duration::from_secs_f64(
                                    (rec.received_offset_ms as f64 / 1000.0) / speed,
                                )
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
                    if cli.json {
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
                s.close().await.map_err(|e| color_eyre::eyre::eyre!(e))?;
            }
            if cli.json {
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
        }

        Command::Queryable {
            command:
                QueryableCommand::Serve {
                    key_expr,
                    reply,
                    reply_file,
                    reply_key,
                    encoding,
                    count,
                    duration,
                    include_request,
                    max_request_bytes,
                },
        } => {
            use zenmon_core::types::MessagePayload;

            // Resolve the fixed reply payload.
            let reply_bytes: Vec<u8> = match (reply, reply_file) {
                (Some(s), None) => s.into_bytes(),
                (None, Some(path)) => std::fs::read(&path).map_err(|e| {
                    ZenmonError::invalid_input(format!(
                        "cannot read --reply-file {}: {}",
                        path.display(),
                        e
                    ))
                })?,
                (None, None) => {
                    return Err(ZenmonError::invalid_input(
                        "provide --reply <string> or --reply-file <path>",
                    ))
                }
                (Some(_), Some(_)) => unreachable!("clap conflicts_with"),
            };
            let reply_key = zenmon_core::queryable::resolve_reply_key(&key_expr, reply_key.as_deref())?;
            let max_request = max_request_bytes.map(|n| n as usize).unwrap_or(1024);

            let session = zenmon_core::session::open_session(&config).await?;
            let queryable = session
                .declare_queryable(&key_expr)
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
            if !cli.json {
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
                        if !cli.json {
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

                            if cli.json {
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

            queryable
                .undeclare()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }

        Command::Tui { refresh } => {
            zenmon_tui::run(config, refresh).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::level_filters::LevelFilter;

    #[test]
    fn scout_heading_describes_a_scouting_port_without_a_domain() {
        let heading = scouting_port_heading(7446, 2);

        assert_eq!(heading, "Scouting port 7446  (2 nodes)");
        assert!(!heading.to_lowercase().contains("domain"));
    }

    /// JSON mode must force logs off even when the user exports `RUST_LOG=trace`,
    /// otherwise Zenoh's `Config` debug (auth fields included) leaks and breaks
    /// the single-JSON-line stderr / clean-JSON stdout contract.
    #[test]
    fn json_mode_forces_off_despite_rust_log() {
        let filter = resolve_log_filter(false, true, Some("trace"));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::OFF));
    }

    /// TUI mode must force logs off even with `RUST_LOG` set, to avoid corrupting
    /// the ratatui display.
    #[test]
    fn tui_mode_forces_off_despite_rust_log() {
        let filter = resolve_log_filter(true, false, Some("trace"));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::OFF));
    }

    /// Plain CLI mode honors `RUST_LOG`.
    #[test]
    fn plain_mode_honors_rust_log() {
        let filter = resolve_log_filter(false, false, Some("trace"));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::TRACE));
    }

    /// Plain CLI mode falls back to the default filter when `RUST_LOG` is unset,
    /// and also when it is malformed.
    #[test]
    fn plain_mode_defaults_without_or_with_invalid_rust_log() {
        assert_eq!(
            resolve_log_filter(false, false, None).max_level_hint(),
            Some(LevelFilter::INFO)
        );
        assert_eq!(
            resolve_log_filter(false, false, Some("=not a valid filter=")).max_level_hint(),
            Some(LevelFilter::INFO)
        );
    }
}
