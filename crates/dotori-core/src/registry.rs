use crate::types::{NodeInfo, NodeSources};
use color_eyre::eyre::eyre;
use color_eyre::Result;
use std::collections::HashMap;
use std::time::SystemTime;
use zenoh::Session;

/// Query Zenoh admin space and return the admin-derived node set.
pub async fn query_admin_nodes(session: &Session) -> Result<Vec<NodeInfo>> {
    let now = SystemTime::now();
    let mut by_zid: HashMap<String, NodeInfo> = HashMap::new();

    let replies = session
        .get("@/*/router")
        .timeout(std::time::Duration::from_secs(5))
        .await
        .map_err(|e| eyre!(e))?;

    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            let payload_str = sample
                .payload()
                .try_to_string()
                .unwrap_or_else(|e| e.to_string().into());
            let json: serde_json::Value = match serde_json::from_str(&payload_str) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("admin reply {} not JSON: {}", key, e);
                    continue;
                }
            };

            let router_zid = json
                .get("zid")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| key.split('/').nth(1).unwrap_or("").to_string());

            if !router_zid.is_empty() {
                let router_locators: Vec<String> = json
                    .get("locators")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                by_zid
                    .entry(router_zid.clone())
                    .and_modify(|n| {
                        for loc in &router_locators {
                            if !n.locators.contains(loc) {
                                n.locators.push(loc.clone());
                            }
                        }
                        n.sources |= NodeSources::ADMIN;
                        n.admin_last_seen = Some(now);
                    })
                    .or_insert_with(|| NodeInfo {
                        zid: router_zid.clone(),
                        kind: "router".into(),
                        locators: router_locators.clone(),
                        metadata: Some(json.clone()),
                        sources: NodeSources::ADMIN,
                        admin_last_seen: Some(now),
                        scout_last_seen: None,
                    });
            }

            if let Some(sessions) = json.get("sessions").and_then(|v| v.as_array()) {
                for s in sessions {
                    let peer_zid = match s.get("peer").and_then(|v| v.as_str()) {
                        Some(z) => z.to_string(),
                        None => continue,
                    };
                    let whatami = s
                        .get("whatami")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();

                    by_zid
                        .entry(peer_zid)
                        .and_modify(|n| {
                            n.sources |= NodeSources::ADMIN;
                            n.admin_last_seen = Some(now);
                        })
                        .or_insert_with(|| NodeInfo {
                            zid: s
                                .get("peer")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            kind: whatami,
                            locators: Vec::new(),
                            metadata: None,
                            sources: NodeSources::ADMIN,
                            admin_last_seen: Some(now),
                            scout_last_seen: None,
                        });
                }
            }
        }
    }

    if let Ok(replies) = session
        .get("@/**")
        .timeout(std::time::Duration::from_secs(5))
        .await
    {
        while let Ok(reply) = replies.recv_async().await {
            if let Ok(sample) = reply.result() {
                let key = sample.key_expr().as_str().to_string();
                let parts: Vec<&str> = key.split('/').collect();
                if parts.len() < 3 {
                    continue;
                }
                let zid = parts[1].to_string();
                let kind = parts[2].to_string();

                if let Some(n) = by_zid.get_mut(&zid) {
                    n.sources |= NodeSources::ADMIN;
                    n.admin_last_seen = Some(now);
                    continue;
                }

                let payload_str = sample
                    .payload()
                    .try_to_string()
                    .unwrap_or_else(|e| e.to_string().into());
                let metadata = match serde_json::from_str::<serde_json::Value>(&payload_str) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::debug!("local admin reply {} not JSON: {}", key, e);
                        None
                    }
                };
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

                by_zid.insert(
                    zid.clone(),
                    NodeInfo {
                        zid,
                        kind,
                        locators,
                        metadata,
                        sources: NodeSources::ADMIN,
                        admin_last_seen: Some(now),
                        scout_last_seen: None,
                    },
                );
            }
        }
    }

    let mut out: Vec<NodeInfo> = by_zid.into_values().collect();
    out.sort_by(|a, b| a.zid.cmp(&b.zid));
    Ok(out)
}
