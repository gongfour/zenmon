use crate::config::DotoriConfig;
use crate::types::ScoutInfo;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use std::time::Duration;
use zenoh::config::WhatAmI;

/// Scout the network for Zenoh nodes.
/// This does NOT require a session — it uses multicast scouting directly.
/// Returns after `timeout` duration.
pub async fn scout(config: &DotoriConfig, timeout: Duration) -> Result<Vec<ScoutInfo>> {
    let zenoh_config = config.to_zenoh_config()?;
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
