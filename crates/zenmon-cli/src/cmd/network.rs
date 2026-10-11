//! `discover`, `nodes`, `liveliness`, `scout`, `info`, `doctor`: who is on the network.

use std::time::Duration;

use zenmon_core::config::ZenmonConfig;
use zenmon_core::error::ZenmonError;

use super::Ctx;
use crate::cli::{DiscoverArgs, DoctorArgs, LivelinessArgs, NodesArgs, ScoutArgs};
use crate::common::{internal_err, load_contract_opt, warn_redundant_namespace};
use crate::watch;

pub async fn discover(
    ctx: &Ctx,
    config: ZenmonConfig,
    args: DiscoverArgs,
) -> Result<(), ZenmonError> {
    let DiscoverArgs { key_expr } = args;
    warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
    // Load before opening the session so a broken contract fails fast.
    let contract = load_contract_opt(&ctx.contract)?;
    let session = zenmon_core::session::open_session(&config).await?;
    let topics = zenmon_core::discover::discover(&session, &key_expr).await?;

    if ctx.json {
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
    session.close().await.map_err(internal_err)?;
    Ok(())
}

pub async fn nodes(ctx: &Ctx, config: ZenmonConfig, args: NodesArgs) -> Result<(), ZenmonError> {
    let NodesArgs {
        watch,
        count,
        duration,
        changes_only,
    } = args;
    use zenmon_core::nodediff::{diff_nodes, NodeSnapshot};

    let session = zenmon_core::session::open_session(&config).await?;

    if !watch {
        let nodes = zenmon_core::registry::query_admin_nodes(&session).await?;
        if ctx.json {
            println!("{}", zenmon_core::output::to_collection_json(&nodes)?);
        } else if nodes.is_empty() {
            println!("No nodes discovered");
        } else {
            print_nodes_table(&nodes, None);
        }
    } else {
        if !ctx.json {
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
                    if !ctx.json {
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
                                    if ctx.json {
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
                    } else if ctx.json {
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
    session.close().await.map_err(internal_err)?;
    Ok(())
}

pub async fn liveliness(
    ctx: &Ctx,
    config: ZenmonConfig,
    args: LivelinessArgs,
) -> Result<(), ZenmonError> {
    let LivelinessArgs {
        key_expr,
        watch,
        count,
        duration,
        changes_only,
    } = args;
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
        if ctx.json {
            if !watch {
                println!("{}", zenmon_core::output::to_collection_json(&tokens)?);
            }
        } else if tokens.is_empty() {
            println!("No liveliness tokens found for '{}'", key_expr);
        } else {
            println!("{:<50} {:<20} SOURCE_ZID", "KEY", "NAME");
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
        if !ctx.json {
            eprintln!("\nWatching liveliness changes... (Ctrl+C to stop)");
        }
        let sub = session
            .liveliness()
            .declare_subscriber(&key_expr)
            .await
            .map_err(internal_err)?;
        let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
        loop {
            let deadline = budget.deadline();
            tokio::select! {
                biased;
                _ = watch::sleep_until_opt(deadline) => break,
                _ = tokio::signal::ctrl_c() => {
                    if !ctx.json {
                        eprintln!("Stopped.");
                    }
                    break;
                }
                res = sub.recv_async() => match res {
                    Ok(sample) => {
                        let source = sample.source_info()
                            .map(|s| format!("{}", s.source_id().zid()))
                            .unwrap_or_else(|| "-".to_string());
                        if ctx.json {
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

    session.close().await.map_err(internal_err)?;
    Ok(())
}

pub async fn scout(ctx: &Ctx, config: ZenmonConfig, args: ScoutArgs) -> Result<(), ZenmonError> {
    let ScoutArgs {
        port_range,
        per_port_timeout,
    } = args;
    let (start, end) = parse_port_range(&port_range)?;
    let results =
        zenmon_core::scout::scout_port_range(&config, start, end, per_port_timeout).await?;

    if ctx.json {
        let hits: Vec<_> = results.iter().filter(|r| !r.nodes.is_empty()).collect();
        println!("{}", zenmon_core::output::to_collection_json(&hits)?);
    } else {
        print_scout_results(&results, start, end, per_port_timeout);
    }
    Ok(())
}

pub async fn info(ctx: &Ctx, config: ZenmonConfig) -> Result<(), ZenmonError> {
    let session = zenmon_core::session::open_session(&config).await?;
    let detail = zenmon_core::info::session_info(&session, config.mode).await?;

    if ctx.json {
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
    session.close().await.map_err(internal_err)?;
    Ok(())
}

pub async fn doctor(ctx: &Ctx, config: ZenmonConfig, args: DoctorArgs) -> Result<(), ZenmonError> {
    let DoctorArgs { timeout } = args;
    let report = zenmon_core::doctor::run(&config, timeout).await;

    if ctx.json {
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
    Ok(())
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
        let is_tailscale = addr.starts_with("100.") || addr.starts_with("[fd7a:115c:a1e0");
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
    locators.iter().max_by_key(|l| score(l)).map(|s| s.as_str())
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
        println!("  {:<8} {:<34} LOCATOR", "TYPE", "ZID");
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
    println!("{:<40} {:<10} LOCATORS", "ZID", "KIND");
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
    let (start_s, end_s) = s.split_once('-').ok_or_else(|| {
        ZenmonError::invalid_input(format!("port range must be START-END, got '{}'", s))
    })?;
    let start: u16 = start_s.trim().parse().map_err(|e| {
        ZenmonError::invalid_input(format!("invalid start port '{}': {}", start_s, e))
    })?;
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
