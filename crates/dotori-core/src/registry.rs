use crate::types::NodeInfo;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use zenoh::Session;

/// Discover Zenoh nodes by querying the admin space.
pub async fn list_nodes(session: &Session) -> Result<Vec<NodeInfo>> {
    let mut nodes = Vec::new();

    // Query admin space for sessions
    let replies = session.get("@/**").await.map_err(|e| eyre!(e))?;

    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            let payload_str = sample
                .payload()
                .try_to_string()
                .unwrap_or_else(|e| e.to_string().into());

            // Admin keys look like: @/<zid>/router or @/<zid>/peer or @/<zid>/client
            let parts: Vec<&str> = key.split('/').collect();
            if parts.len() >= 3 {
                let zid = parts[1].to_string();
                let kind = parts[2].to_string();

                // Parse metadata from payload (JSON)
                let metadata = serde_json::from_str::<serde_json::Value>(&payload_str).ok();

                let locators = metadata
                    .as_ref()
                    .and_then(|m| m.get("locators"))
                    .and_then(|l| l.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                // Avoid duplicate entries for the same zid
                if !nodes.iter().any(|n: &NodeInfo| n.zid == zid) {
                    nodes.push(NodeInfo {
                        zid,
                        kind,
                        locators,
                        metadata,
                        last_seen: Some(std::time::SystemTime::now()),
                    });
                }
            }
        }
    }

    nodes.sort_by(|a, b| a.zid.cmp(&b.zid));
    Ok(nodes)
}
