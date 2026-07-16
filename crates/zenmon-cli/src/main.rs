mod cli;
mod duration;
mod watch;

use clap::Parser;
use cli::{Cli, Command, ConfigCommand, ContractCommand, QueryableCommand};
use zenmon_core::config::{
    resolve_config, ConfigOverrides, EffectiveConfig, ResolvedConfig, ResolvedValue,
};
use zenmon_core::contract::{Contract, Enrichment};
use zenmon_core::error::ZenmonError;
use std::path::PathBuf;
use std::time::Duration;

/// Resolve a contract path: explicit arg → `--contract` flag → `ZENMON_CONTRACT`.
fn contract_path(explicit: Option<PathBuf>, cli_flag: &Option<PathBuf>) -> Option<PathBuf> {
    explicit
        .or_else(|| cli_flag.clone())
        .or_else(|| std::env::var_os("ZENMON_CONTRACT").map(PathBuf::from))
}

/// Load the contract for enrichment (`sub`/`discover`): `None` when no path is
/// configured, hard error when a configured path fails to load.
fn load_contract_opt(cli_flag: &Option<PathBuf>) -> Result<Option<Contract>, ZenmonError> {
    match contract_path(None, cli_flag) {
        Some(p) => Contract::load(p).map(Some),
        None => Ok(None),
    }
}

/// Load the contract for the `contract` subcommand, where one is required.
fn load_contract_required(
    explicit: Option<PathBuf>,
    cli_flag: &Option<PathBuf>,
) -> Result<Contract, ZenmonError> {
    match contract_path(explicit, cli_flag) {
        Some(p) => Contract::load(p),
        None => Err(ZenmonError::invalid_input(
            "no contract file: pass a path, use --contract, or set ZENMON_CONTRACT",
        )),
    }
}

/// One-line human annotation for an enriched `sub` message.
fn contract_annotation(e: &Enrichment, observed_encoding: &str) -> String {
    if !e.declared {
        return "# ⚠ not declared in contract".to_string();
    }
    let mut s = format!("# {}", e.matched_key.as_deref().unwrap_or_default());
    if let Some(d) = &e.description {
        s.push_str(" — ");
        s.push_str(d);
    }
    match e.encoding_matches {
        Some(true) => {
            s.push_str(&format!(
                "  [encoding: {} ok]",
                e.encoding_expected.as_deref().unwrap_or_default()
            ));
        }
        Some(false) => {
            s.push_str(&format!(
                "  [encoding: expected {}, got {}]",
                e.encoding_expected.as_deref().unwrap_or_default(),
                observed_encoding
            ));
        }
        None => {}
    }
    s
}

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
            warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
            // Load before opening the session so a broken contract fails fast.
            let contract = load_contract_opt(&cli.contract)?;
            let session = zenmon_core::session::open_session(&config).await?;
            let topics = zenmon_core::discover::discover(&session, &key_expr).await?;

            if cli.json {
                match &contract {
                    Some(c) => {
                        // Enriched: annotate each key with its contract context.
                        // Discover has no payload, so encoding is unknown ("").
                        let items: Vec<_> = topics
                            .iter()
                            .map(|t| {
                                serde_json::json!({
                                    "key_expr": t.key_expr,
                                    "contract": c.enrich(&t.key_expr, ""),
                                })
                            })
                            .collect();
                        println!("{}", zenmon_core::output::to_collection_json(&items)?);
                    }
                    None => println!("{}", zenmon_core::output::to_collection_json(&topics)?),
                }
            } else if topics.is_empty() {
                println!("No active keys found for '{}'", key_expr);
            } else {
                for topic in &topics {
                    match &contract {
                        Some(c) => {
                            let e = c.enrich(&topic.key_expr, "");
                            let tag = if e.declared {
                                match e.description.as_deref() {
                                    Some(d) => format!("  # {}", d),
                                    None => "  # declared".to_string(),
                                }
                            } else {
                                "  # ⚠ undeclared".to_string()
                            };
                            println!("{}{}", topic.key_expr, tag);
                        }
                        None => println!("{}", topic.key_expr),
                    }
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
            warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
            let max_payload_bytes = max_payload_bytes.map(|n| n as usize);
            // Load before opening the session so a broken contract fails fast.
            let contract = load_contract_opt(&cli.contract)?;
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
                                // Fast path preserved byte-for-byte when neither
                                // a payload cap nor a contract is in play.
                                if max_payload_bytes.is_none() && contract.is_none() {
                                    println!("{}", serde_json::to_string(&msg)?);
                                } else {
                                    let mut v = serde_json::to_value(&msg)?;
                                    if let Some(max) = max_payload_bytes {
                                        // Replace payload/attachment with capped
                                        // previews so a large message can't blow
                                        // the output budget.
                                        v["payload"] = msg.payload.to_view_capped(max);
                                        if let Some(att) = &msg.attachment {
                                            v["attachment"] = att.to_view_capped(max);
                                        }
                                    }
                                    if let Some(c) = &contract {
                                        v["contract"] = serde_json::to_value(
                                            c.enrich(&msg.key_expr, &msg.encoding),
                                        )?;
                                    }
                                    println!("{}", serde_json::to_string(&v)?);
                                }
                            } else {
                                if let Some(c) = &contract {
                                    println!(
                                        "{}",
                                        contract_annotation(
                                            &c.enrich(&msg.key_expr, &msg.encoding),
                                            &msg.encoding,
                                        )
                                    );
                                }
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
            warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
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

        Command::Pub {
            key_expr,
            value,
            att,
            rate,
            count,
            duration,
        } => {
            warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
            let value = resolve_payload_arg(&value)?;
            let session = zenmon_core::session::open_session(&config).await?;
            let attachment_bytes = att.as_ref().map(|a| a.as_bytes().len());

            // Publish the same value/attachment once. Rebuilt per tick because
            // the put builder is consumed by `.await`.
            let publish_once = || async {
                let mut builder = session.put(&key_expr, value.clone());
                if let Some(ref att_json) = att {
                    builder = builder.attachment(att_json.as_bytes());
                }
                builder.await.map_err(|e| color_eyre::eyre::eyre!(e))
            };

            match rate {
                // Fixed-rate loop: republish on each interval tick until the
                // count/duration budget is spent or Ctrl+C. Clap guarantees a
                // bound is present, so this loop always terminates.
                Some(hz) => {
                    // The first interval tick fires immediately, so --count 1
                    // publishes once and exits. Ticks are phase-locked to the
                    // schedule (no drift from per-put latency).
                    let mut interval =
                        tokio::time::interval(crate::duration::rate_tick_interval(hz));
                    let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
                    let mut published: u64 = 0;
                    if !cli.json {
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
                                if !cli.json {
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
                    if cli.json {
                        // Single summary line on stdout; no duplicate stderr.
                        println!(
                            "{}",
                            zenmon_core::output::publish_rate_summary_json(
                                &key_expr,
                                value.as_bytes().len(),
                                attachment_bytes,
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
                    if cli.json {
                        // Action result on stdout; no duplicate stderr message.
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
                }
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
            warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
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
            warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
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

        Command::Contract { command } => match command {
            ContractCommand::Lint { path } => {
                let contract = load_contract_required(path, &cli.contract)?;
                let report = contract.lint();
                if cli.json {
                    println!("{}", serde_json::to_string(&report)?);
                } else {
                    println!(
                        "{} topics, {} types, {} services",
                        report.topics, report.types, report.services
                    );
                    if report.warnings.is_empty() {
                        println!("no warnings");
                    } else {
                        println!("warnings ({}):", report.warnings.len());
                        for w in &report.warnings {
                            println!("  - {}", w);
                        }
                    }
                }
            }

            ContractCommand::List { path } => {
                let contract = load_contract_required(path, &cli.contract)?;
                if cli.json {
                    let items: Vec<_> = contract
                        .topics
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "key": t.key,
                                "pattern": t.pattern,
                                "encoding": contract.effective_encoding(t),
                            })
                        })
                        .collect();
                    println!("{}", zenmon_core::output::to_collection_json(&items)?);
                } else {
                    for t in &contract.topics {
                        println!(
                            "{:<44} {:<10} {}",
                            t.key,
                            t.pattern,
                            contract.effective_encoding(t)
                        );
                    }
                    println!("\n{} topic(s)", contract.topics.len());
                }
            }

            ContractCommand::Show { key, path } => {
                let contract = load_contract_required(path, &cli.contract)?;
                let topic = contract.lookup(&key).ok_or_else(|| {
                    ZenmonError::not_found(format!("'{}' is not declared in the contract", key))
                })?;
                let resolve = |v: &Option<serde_json::Value>| v.as_ref().map(|s| contract.resolve_refs(s));
                let payload = resolve(&topic.payload);
                let phases = resolve(&topic.phases);
                let request = resolve(&topic.request);
                let response = resolve(&topic.response);

                if cli.json {
                    let mut obj = serde_json::json!({
                        "key": topic.key,
                        "pattern": topic.pattern,
                        "encoding": contract.effective_encoding(topic),
                        "enveloped": contract.effective_enveloped(topic),
                        "producers": topic.producers,
                        "consumers": topic.consumers,
                    });
                    if let Some(s) = &topic.status {
                        obj["status"] = serde_json::json!(s);
                    }
                    if let Some(d) = &topic.description {
                        obj["description"] = serde_json::json!(d);
                    }
                    for (k, v) in [
                        ("payload", &payload),
                        ("phases", &phases),
                        ("request", &request),
                        ("response", &response),
                    ] {
                        if let Some(v) = v {
                            obj[k] = v.clone();
                        }
                    }
                    println!("{}", serde_json::to_string(&obj)?);
                } else {
                    println!("key:         {}", topic.key);
                    println!("pattern:     {}", topic.pattern);
                    println!(
                        "encoding:    {} ({})",
                        contract.effective_encoding(topic),
                        if contract.effective_enveloped(topic) {
                            "enveloped"
                        } else {
                            "bare"
                        }
                    );
                    if let Some(s) = &topic.status {
                        println!("status:      {}", s);
                    }
                    if !topic.producers.is_empty() {
                        println!("producers:   {}", topic.producers.join(", "));
                    }
                    if !topic.consumers.is_empty() {
                        println!("consumers:   {}", topic.consumers.join(", "));
                    }
                    if let Some(d) = &topic.description {
                        println!("description: {}", d);
                    }
                    for (label, v) in [
                        ("payload", &payload),
                        ("phases", &phases),
                        ("request", &request),
                        ("response", &response),
                    ] {
                        if let Some(v) = v {
                            println!("{}:", label);
                            let pretty = serde_json::to_string_pretty(v).unwrap_or_default();
                            for line in pretty.lines() {
                                println!("  {}", line);
                            }
                        }
                    }
                }
            }
        },

        Command::Scenario {
            observe,
            preset,
            prefix,
            pub_,
            task,
            pub_rate,
            pub_for,
            pub_count,
            for_,
            settle,
            track,
            no_timeline,
        } => {
            // A resolved contract lets --task surface & validate its request schema.
            let contract = load_contract_opt(&cli.contract)?;
            run_scenario(
                cli.json, &config, observe, preset, prefix, pub_, task, pub_rate, pub_for,
                pub_count, for_, settle, track, no_timeline, contract,
            )
            .await?;
        }
    }

    Ok(())
}

/// If `namespace` is set and `key` already begins with it (on a segment
/// boundary), return a warning: under `-n` the key is prefixed *again*, so it
/// silently matches nothing — an easy, hard-to-debug mistake.
fn namespace_redundant_prefix(key: &str, namespace: Option<&str>) -> Option<String> {
    let ns = namespace?.trim_matches('/');
    if ns.is_empty() {
        return None;
    }
    let redundant = key == ns
        || key
            .strip_prefix(ns)
            .is_some_and(|rest| rest.starts_with('/'));
    redundant.then(|| {
        format!(
            "# note: key '{key}' already starts with namespace '{ns}' — under -n it is prefixed \
             again and will match nothing (drop the namespace from the key)"
        )
    })
}

/// Emit the redundant-namespace warning for each key, if any.
fn warn_redundant_namespace<'a>(keys: impl IntoIterator<Item = &'a str>, namespace: Option<&str>) {
    for k in keys {
        if let Some(w) = namespace_redundant_prefix(k, namespace) {
            eprintln!("{w}");
        }
    }
}

/// Resolve a publish/request value argument: `@<path>` reads the value from that
/// file, `-` reads it from stdin, and anything else is the literal value. Lets
/// large or dynamically-built payloads (e.g. a VDA5050 mission for `--task`) come
/// from a file instead of an unwieldy inline CLI string.
fn resolve_payload_arg(s: &str) -> Result<String, ZenmonError> {
    if s == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| ZenmonError::invalid_input(format!("reading value from stdin: {e}")))?;
        Ok(buf)
    } else if let Some(path) = s.strip_prefix('@') {
        std::fs::read_to_string(path)
            .map_err(|e| ZenmonError::invalid_input(format!("reading value from '{path}': {e}")))
    } else {
        Ok(s.to_string())
    }
}

/// Extract a string field from a message's attachment, if the attachment parses
/// as a JSON object carrying that key. Used for `correlation_id` / `request_id`.
fn attachment_str(msg: &zenmon_core::types::ZenohMessage, field: &str) -> Option<String> {
    let att = msg.attachment.as_ref()?;
    let v = att.as_json()?;
    v.get(field).and_then(|f| f.as_str()).map(String::from)
}

/// Build an observed [`ScenarioEvent`] from a received message, extracting the
/// causal ids from its attachment and decoding the payload. `trigger` is always
/// false — the synthetic trigger event is built separately.
fn observed_event(
    msg: zenmon_core::types::ZenohMessage,
    t_rel_ms: u64,
) -> zenmon_core::scenario::ScenarioEvent {
    zenmon_core::scenario::ScenarioEvent {
        t_rel_ms,
        correlation_id: attachment_str(&msg, "correlation_id"),
        request_id: attachment_str(&msg, "request_id"),
        encoding: msg.encoding.clone(),
        kind: msg.kind.clone(),
        payload: msg.payload.to_view(),
        key_expr: msg.key_expr,
        trigger: false,
    }
}

/// Build the synthetic trigger event at `t_rel_ms = 0` — the scenario's own
/// actuation/request that caused the episode. `value` is parsed as JSON for the
/// payload view, falling back to a plain string.
fn make_trigger_event(key: &str, value: &str) -> zenmon_core::scenario::ScenarioEvent {
    zenmon_core::scenario::ScenarioEvent {
        t_rel_ms: 0,
        key_expr: key.to_string(),
        correlation_id: None,
        request_id: None,
        encoding: String::new(),
        kind: "PUT".to_string(),
        payload: serde_json::from_str(value)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_string())),
        trigger: true,
    }
}

/// The `scenario` orchestration: resolve observed keys, subscribe, optionally
/// trigger a `--pub`/`--task`, observe a bounded window (early-exiting on the
/// task response), then build and print the episode.
#[allow(clippy::too_many_arguments)]
async fn run_scenario(
    json: bool,
    config: &zenmon_core::config::ZenmonConfig,
    observe: Vec<String>,
    preset: Option<String>,
    prefix: String,
    mut pub_: Option<Vec<String>>,
    mut task: Option<Vec<String>>,
    pub_rate: Option<f64>,
    pub_for: Option<Duration>,
    pub_count: Option<u64>,
    for_: Duration,
    settle: Option<Duration>,
    track: Vec<String>,
    no_timeline: bool,
    contract: Option<Contract>,
) -> Result<(), ZenmonError> {
    use std::time::Instant;
    use zenmon_core::scenario::{
        build_episode, build_tracks, expand_preset, EndedReason, ScenarioEvent, ScenarioMeta,
        TrackSpec, TriggerInfo,
    };

    // Parse --track KEY:FIELD specs up front so a malformed one fails before we
    // open a session.
    let mut specs: Vec<TrackSpec> = Vec::with_capacity(track.len());
    for t in &track {
        let (key, field) = t.split_once(':').ok_or_else(|| {
            ZenmonError::invalid_input(format!("--track must be KEY:FIELD, got '{}'", t))
        })?;
        if key.is_empty() || field.is_empty() {
            return Err(ZenmonError::invalid_input(format!(
                "--track KEY and FIELD must be non-empty, got '{}'",
                t
            )));
        }
        specs.push(TrackSpec {
            key: key.to_string(),
            field: field.to_string(),
        });
    }

    // Resolve @file / stdin payloads for the trigger before anyone reads them.
    if let Some(p) = pub_.as_mut() {
        p[1] = resolve_payload_arg(&p[1])?;
    }
    if let Some(p) = task.as_mut() {
        p[1] = resolve_payload_arg(&p[1])?;
    }

    // If a contract is resolved and we're triggering a --task, surface its
    // request schema (so the caller need not read source to build the request)
    // and lightly validate the provided request. Display-only, never fatal.
    if let (Some(pair), Some(c)) = (&task, &contract) {
        if let Some(schema) = c.request_schema(&pair[0]) {
            eprintln!("# contract request schema for '{}':", pair[0]);
            for line in serde_json::to_string_pretty(&schema)
                .unwrap_or_default()
                .lines()
            {
                eprintln!("#   {}", line);
            }
            if let Ok(provided) = serde_json::from_str::<serde_json::Value>(&pair[1]) {
                for w in zenmon_core::contract::validate_against_schema(&schema, &provided) {
                    eprintln!("# ⚠ request: {}", w);
                }
            }
        }
    }

    // Resolve the observed key set: --observe, then --preset expansion, then the
    // two task-derived topics. Deduplicate while preserving order.
    let mut observed: Vec<String> = Vec::new();
    let push_unique = |set: &mut Vec<String>, k: String| {
        if !set.contains(&k) {
            set.push(k);
        }
    };
    for k in observe {
        push_unique(&mut observed, k);
    }
    if let Some(name) = &preset {
        let expanded = expand_preset(name, &prefix);
        if expanded.is_empty() {
            return Err(ZenmonError::invalid_input(format!(
                "unknown --preset '{}' (known: stall)",
                name
            )));
        }
        for k in expanded {
            push_unique(&mut observed, k);
        }
    }

    // A --task adds its feedback/response topics and defines a response key that
    // ends the scenario early.
    let mut response_key: Option<String> = None;
    let trigger: TriggerInfo = if let Some(pair) = &task {
        let task_prefix = pair[0].trim_end_matches('/').to_string();
        let request_json = pair[1].clone();
        let request_key = format!("{}/request", task_prefix);
        let feedback_key = format!("{}/feedback", task_prefix);
        let resp_key = format!("{}/response", task_prefix);
        push_unique(&mut observed, feedback_key);
        push_unique(&mut observed, resp_key.clone());
        response_key = Some(resp_key);
        TriggerInfo::Task {
            request_key,
            request_bytes: request_json.as_bytes().len(),
        }
    } else if let Some(pair) = &pub_ {
        TriggerInfo::Pub {
            key_expr: pair[0].clone(),
            bytes: pair[1].as_bytes().len(),
        }
    } else {
        TriggerInfo::None
    };

    if observed.is_empty() {
        return Err(ZenmonError::invalid_input(
            "no topics to observe: pass --observe or --preset",
        ));
    }

    {
        let mut keys: Vec<&str> = observed.iter().map(String::as_str).collect();
        if let Some(p) = &pub_ {
            keys.push(p[0].as_str());
        }
        if let Some(p) = &task {
            keys.push(p[0].as_str());
        }
        warn_redundant_namespace(keys, config.namespace.as_deref());
    }

    let session = zenmon_core::session::open_session(config).await?;

    // Subscribe to every observed key into one channel BEFORE triggering, so we
    // never miss the reaction to our own actuation.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut handles = Vec::with_capacity(observed.len());
    for key in &observed {
        handles.push(zenmon_core::subscriber::subscribe(&session, key, tx.clone()).await?);
    }
    // Drop our sender clone so the channel closes when all subscribers stop.
    drop(tx);

    if !json {
        eprintln!(
            "Recording scenario over {} topic(s) for {} ... (Ctrl+C to stop)",
            observed.len(),
            humantime::format_duration(for_)
        );
    }

    // The stopwatch defines t_rel_ms = 0; trigger fires right after so its
    // reaction is measured from the same origin.
    let stopwatch = Instant::now();
    let mut events: Vec<ScenarioEvent> = Vec::new();
    // A sustained --pub runs on a background task, bounded by --pub-for/--pub-count.
    let mut pub_task: Option<tokio::task::JoinHandle<()>> = None;

    // Fire the trigger and record it as the causal origin (t_rel_ms = 0). A
    // publish failure on a one-shot path is fatal.
    let trigger_ev: Option<ScenarioEvent> = match (&task, &pub_) {
        (Some(pair), _) => {
            let request_key = format!("{}/request", pair[0].trim_end_matches('/'));
            session
                .put(&request_key, pair[1].clone())
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
            Some(make_trigger_event(&request_key, &pair[1]))
        }
        (_, Some(pair)) => {
            let key = pair[0].clone();
            let value = pair[1].clone();
            match pub_rate {
                // Sustained: republish at a fixed rate on a background task so
                // the actuation persists through the observe window (defeats a
                // command watchdog), bounded by --pub-for/--pub-count.
                Some(hz) => {
                    let s = session.clone();
                    let bounds = watch::Bounds::new(pub_count, pub_for);
                    let (k, v) = (key.clone(), value.clone());
                    pub_task = Some(tokio::spawn(async move {
                        let mut interval =
                            tokio::time::interval(crate::duration::rate_tick_interval(hz));
                        let mut budget = watch::Budget::start(bounds);
                        loop {
                            interval.tick().await;
                            if s.put(&k, v.clone()).await.is_err() {
                                break;
                            }
                            if budget.record() {
                                break;
                            }
                        }
                    }));
                }
                None => {
                    session
                        .put(&key, value.clone())
                        .await
                        .map_err(|e| color_eyre::eyre::eyre!(e))?;
                }
            }
            Some(make_trigger_event(&key, &value))
        }
        (None, None) => None,
    };
    if let Some(te) = trigger_ev {
        events.push(te);
    }

    // Observe the window. Ends when the task response arrives OR --for elapses;
    // then observe --settle more. Ctrl+C ends early with what we have.
    let window_deadline = stopwatch + for_;
    let mut ended_reason = EndedReason::WindowElapsed;
    let mut settle_until: Option<Instant> = None;

    loop {
        // In the settle phase the effective deadline is the settle end; before
        // that it is the --for window end.
        let deadline = settle_until.unwrap_or(window_deadline);
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline.into()) => break,
            _ = tokio::signal::ctrl_c() => {
                if !json {
                    eprintln!("\nStopped.");
                }
                break;
            }
            item = rx.recv() => match item {
                Some(msg) => {
                    let t_rel_ms = stopwatch.elapsed().as_millis() as u64;
                    let is_response = response_key
                        .as_deref()
                        .is_some_and(|rk| rk == msg.key_expr);
                    events.push(observed_event(msg, t_rel_ms));
                    // First response ends the active window; then run --settle.
                    if is_response && settle_until.is_none() {
                        ended_reason = EndedReason::TaskResponse;
                        match settle {
                            Some(s) => settle_until = Some(Instant::now() + s),
                            None => break,
                        }
                    }
                }
                None => break,
            }
        }
    }

    // If --settle was requested but the window elapsed without a response, still
    // observe the extra settle time before finishing.
    if settle_until.is_none() {
        if let Some(s) = settle {
            let settle_end = Instant::now() + s;
            loop {
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(settle_end.into()) => break,
                    _ = tokio::signal::ctrl_c() => break,
                    item = rx.recv() => match item {
                        Some(msg) => {
                            let t_rel_ms = stopwatch.elapsed().as_millis() as u64;
                            events.push(observed_event(msg, t_rel_ms));
                        }
                        None => break,
                    }
                }
            }
        }
    }

    if let Some(h) = pub_task {
        h.abort();
    }
    for h in handles {
        h.abort();
    }
    session
        .close()
        .await
        .map_err(|e| color_eyre::eyre::eyre!(e))?;

    let meta = ScenarioMeta {
        trigger,
        for_ms: for_.as_millis() as u64,
        settle_ms: settle.map(|s| s.as_millis() as u64).unwrap_or(0),
        observed,
        ended_reason,
    };
    let mut episode = build_episode(&meta, &events);
    if !specs.is_empty() {
        episode["tracks"] = build_tracks(&events, &specs);
    }
    if no_timeline {
        if let Some(obj) = episode.as_object_mut() {
            obj.remove("timeline");
        }
    }

    if json {
        println!("{}", serde_json::to_string(&episode)?);
    } else {
        print_scenario_summary(&episode);
    }
    Ok(())
}

/// Compact human summary of an episode: trigger, per-topic counts, correlation
/// chain count, ended reason.
fn print_scenario_summary(episode: &serde_json::Value) {
    let meta = &episode["meta"];
    let trigger = &meta["trigger"];
    let trigger_str = match trigger["kind"].as_str() {
        Some("task") => format!("task -> {}", trigger["request_key"].as_str().unwrap_or("")),
        Some("pub") => format!("pub -> {}", trigger["key_expr"].as_str().unwrap_or("")),
        _ => "none".to_string(),
    };
    println!("trigger:       {}", trigger_str);
    println!(
        "window:        {}ms (+{}ms settle)",
        meta["for_ms"].as_u64().unwrap_or(0),
        meta["settle_ms"].as_u64().unwrap_or(0)
    );
    println!(
        "messages:      {}",
        meta["message_count"].as_u64().unwrap_or(0)
    );
    println!(
        "ended:         {}",
        meta["ended_reason"].as_str().unwrap_or("")
    );

    if let Some(topics) = episode["topics"].as_object() {
        println!("topics:");
        let mut keys: Vec<&String> = topics.keys().collect();
        keys.sort();
        for k in keys {
            let t = &topics[k];
            println!(
                "  {:<50} {} msg (t {}..{}ms)",
                k,
                t["count"].as_u64().unwrap_or(0),
                t["first_t_rel_ms"].as_u64().unwrap_or(0),
                t["last_t_rel_ms"].as_u64().unwrap_or(0),
            );
        }
    }

    let chains = episode["correlations"]
        .as_object()
        .map(|m| m.len())
        .unwrap_or(0);
    println!("correlations:  {} chain(s)", chains);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::level_filters::LevelFilter;

    fn enr(declared: bool, matches: Option<bool>) -> Enrichment {
        Enrichment {
            declared,
            matched_key: declared.then(|| "topic/x".to_string()),
            description: declared.then(|| "desc".to_string()),
            encoding_expected: declared.then(|| "application/json".to_string()),
            encoding_matches: matches,
            enveloped: declared.then_some(true),
        }
    }

    #[test]
    fn resolve_payload_literal_returned_as_is() {
        assert_eq!(resolve_payload_arg("{\"a\":1}").unwrap(), "{\"a\":1}");
    }

    #[test]
    fn resolve_payload_at_file_reads_contents() {
        let path = std::env::temp_dir().join("zenmon_resolve_payload_test.json");
        std::fs::write(&path, "{\"from\":\"file\"}").unwrap();
        let arg = format!("@{}", path.display());
        assert_eq!(resolve_payload_arg(&arg).unwrap(), "{\"from\":\"file\"}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_payload_missing_file_is_error() {
        assert!(resolve_payload_arg("@/no/such/zenmon/payload/file.json").is_err());
    }

    #[test]
    fn namespace_warns_when_key_already_prefixed() {
        let w = namespace_redundant_prefix("dotori/forky001/topic/x", Some("dotori/forky001"));
        assert!(w.is_some_and(|m| m.contains("already starts with namespace")));
        // Exact-equal to the namespace also warns.
        assert!(namespace_redundant_prefix("dotori/forky001", Some("dotori/forky001")).is_some());
    }

    #[test]
    fn namespace_no_warn_for_relative_or_absent() {
        assert!(namespace_redundant_prefix("topic/x", Some("dotori/forky001")).is_none());
        assert!(namespace_redundant_prefix("dotori/forky001/topic/x", None).is_none());
    }

    #[test]
    fn namespace_no_false_positive_on_partial_segment() {
        // "forky001x" must not be treated as under namespace "forky001".
        assert!(namespace_redundant_prefix("dotori/forky001x/topic", Some("dotori/forky001")).is_none());
    }

    #[test]
    fn annotation_flags_undeclared_topic() {
        let s = contract_annotation(&enr(false, None), "application/json");
        assert_eq!(s, "# ⚠ not declared in contract");
    }

    #[test]
    fn annotation_reports_matching_encoding() {
        let s = contract_annotation(&enr(true, Some(true)), "application/json");
        assert_eq!(s, "# topic/x — desc  [encoding: application/json ok]");
    }

    #[test]
    fn annotation_reports_encoding_mismatch_with_observed() {
        let s = contract_annotation(&enr(true, Some(false)), "application/octet-stream");
        assert_eq!(
            s,
            "# topic/x — desc  [encoding: expected application/json, got application/octet-stream]"
        );
    }

    #[test]
    fn annotation_omits_encoding_when_unknown() {
        // Discover has no payload encoding, so encoding_matches is None.
        let s = contract_annotation(&enr(true, None), "");
        assert_eq!(s, "# topic/x — desc");
    }

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
