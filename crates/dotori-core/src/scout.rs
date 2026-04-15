use crate::config::DotoriConfig;
use crate::types::{PortScoutResult, ScoutInfo};
use color_eyre::eyre::{eyre, Result};
use std::time::Duration;
use zenoh::config::WhatAmI;

/// Scout the network for Zenoh nodes.
/// This does NOT require a session — it uses multicast scouting directly.
/// Returns after `timeout` duration.
pub async fn scout(config: &DotoriConfig, timeout: Duration) -> Result<Vec<ScoutInfo>> {
    let zenoh_config = config.to_zenoh_config()?;
    run_scout(zenoh_config, timeout).await
}

/// Scout using a specific multicast port. Used for port scanning across
/// different Zenoh multicast addresses.
pub async fn scout_on_port(
    config: &DotoriConfig,
    port: u16,
    timeout: Duration,
) -> Result<Vec<ScoutInfo>> {
    let mut cfg = config.clone();
    cfg.scout_port = Some(port);
    let zenoh_config = cfg.to_zenoh_config()?;
    run_scout(zenoh_config, timeout).await
}

/// Scan a range of multicast ports in parallel, returning per-port results
/// sorted by port. Ports with no hits are still included (empty node list) so
/// the caller can display them.
pub async fn scout_port_range(
    config: &DotoriConfig,
    start: u16,
    end: u16,
    per_port_timeout: Duration,
) -> Result<Vec<PortScoutResult>> {
    if start > end {
        return Err(eyre!(
            "invalid port range: start {} > end {}",
            start,
            end
        ));
    }

    let mut set = tokio::task::JoinSet::new();
    for port in start..=end {
        let config = config.clone();
        set.spawn(async move {
            let nodes = scout_on_port(&config, port, per_port_timeout).await?;
            Ok::<PortScoutResult, color_eyre::Report>(PortScoutResult { port, nodes })
        });
    }

    let mut results = Vec::with_capacity((end - start + 1) as usize);
    while let Some(joined) = set.join_next().await {
        results.push(joined.map_err(|e| eyre!(e))??);
    }
    results.sort_by_key(|r| r.port);
    Ok(results)
}

async fn run_scout(zenoh_config: zenoh::Config, timeout: Duration) -> Result<Vec<ScoutInfo>> {
    let receiver = zenoh::scout(WhatAmI::Router | WhatAmI::Peer | WhatAmI::Client, zenoh_config)
        .await
        .map_err(|e| eyre!(e))?;

    let mut nodes = Vec::new();

    let _ = tokio::time::timeout(timeout, async {
        while let Ok(hello) = receiver.recv_async().await {
            let zid = format!("{}", hello.zid());
            if !nodes.iter().any(|n: &ScoutInfo| n.zid == zid) {
                nodes.push(ScoutInfo {
                    zid,
                    whatami: format!("{}", hello.whatami()),
                    locators: hello.locators().iter().map(|l| format!("{}", l)).collect(),
                });
            }
        }
    })
    .await;

    receiver.stop();
    nodes.sort_by(|a, b| a.zid.cmp(&b.zid));
    Ok(nodes)
}
