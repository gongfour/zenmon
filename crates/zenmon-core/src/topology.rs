//! Normalized network topology derived from node admin metadata.
//!
//! Router metadata carries a `sessions` array (`{peer, whatami, links:[{dst}]}`)
//! describing who each node is connected to. We turn that into a normalized,
//! de-duplicated graph so the TUI can render relationships without re-parsing
//! raw metadata. The graph is an admin-based **partial observation**: nodes
//! without metadata contribute no edges, and peers referenced but not in the
//! registry appear as `known: false` (dangling). `partial` flags both cases so
//! the UI never presents "no relationship" as "definitely none".

use crate::types::NodeInfo;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopologyNode {
    pub zid: String,
    pub kind: String,
    /// True if the node was in the registry; false if only referenced as a peer.
    pub known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopologyEdge {
    pub from_zid: String,
    pub to_zid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_dst: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Topology {
    pub nodes: Vec<TopologyNode>,
    pub edges: Vec<TopologyEdge>,
    /// True when relationship data is incomplete (a node lacked session
    /// metadata, or an edge points at a node not in the registry).
    pub partial: bool,
}

/// Build a normalized topology from the node registry.
pub fn build_topology(nodes: &[NodeInfo]) -> Topology {
    let known: BTreeMap<&str, &NodeInfo> = nodes.iter().map(|n| (n.zid.as_str(), n)).collect();

    let mut edges: Vec<TopologyEdge> = Vec::new();
    let mut seen: BTreeSet<(String, String, Option<String>)> = BTreeSet::new();
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut partial = false;

    for node in nodes {
        let sessions = node
            .metadata
            .as_ref()
            .and_then(|m| m.get("sessions"))
            .and_then(|v| v.as_array());

        let Some(sessions) = sessions else {
            // No session metadata → this node's relationships are unknown.
            partial = true;
            continue;
        };

        for s in sessions {
            let Some(peer) = s.get("peer").and_then(|v| v.as_str()) else {
                continue;
            };
            let link_dst = s
                .get("links")
                .and_then(|v| v.as_array())
                .and_then(|l| l.first())
                .and_then(|l| l.get("dst"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let key = (node.zid.clone(), peer.to_string(), link_dst.clone());
            if seen.insert(key) {
                edges.push(TopologyEdge {
                    from_zid: node.zid.clone(),
                    to_zid: peer.to_string(),
                    link_dst,
                });
            }
            referenced.insert(peer.to_string());
            if !known.contains_key(peer) {
                partial = true;
            }
        }
    }

    let mut topo_nodes: Vec<TopologyNode> = nodes
        .iter()
        .map(|n| TopologyNode {
            zid: n.zid.clone(),
            kind: n.kind.clone(),
            known: true,
        })
        .collect();
    for zid in &referenced {
        if !known.contains_key(zid.as_str()) {
            topo_nodes.push(TopologyNode {
                zid: zid.clone(),
                kind: "unknown".to_string(),
                known: false,
            });
        }
    }
    topo_nodes.sort_by(|a, b| a.zid.cmp(&b.zid));

    Topology {
        nodes: topo_nodes,
        edges,
        partial,
    }
}

/// Render the topology as cycle-safe ASCII adjacency lines: each source node
/// followed by its outgoing edges. Because it lists edges (not a recursive
/// tree walk), cycles and multi-router graphs are represented safely.
pub fn to_adjacency_lines(topo: &Topology) -> Vec<String> {
    let mut by_from: BTreeMap<&str, Vec<&TopologyEdge>> = BTreeMap::new();
    for e in &topo.edges {
        by_from.entry(e.from_zid.as_str()).or_default().push(e);
    }

    let kind_of = |zid: &str| -> String {
        topo.nodes
            .iter()
            .find(|n| n.zid == zid)
            .map(|n| {
                if n.known {
                    n.kind.clone()
                } else {
                    format!("{} (unknown)", n.kind)
                }
            })
            .unwrap_or_else(|| "?".to_string())
    };

    let mut lines = Vec::new();
    for (from, edges) in &by_from {
        lines.push(format!("{} [{}]", from, kind_of(from)));
        for (i, e) in edges.iter().enumerate() {
            let branch = if i + 1 == edges.len() { "└─" } else { "├─" };
            let dst = e
                .link_dst
                .as_deref()
                .map(|d| format!("  {}", d))
                .unwrap_or_default();
            lines.push(format!("  {} {} [{}]{}", branch, e.to_zid, kind_of(&e.to_zid), dst));
        }
    }
    if lines.is_empty() {
        lines.push("No relationships observed.".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::NodeSources;
    use serde_json::json;

    fn node(zid: &str, kind: &str, sessions: Option<serde_json::Value>) -> NodeInfo {
        let metadata = sessions.map(|s| json!({ "sessions": s }));
        NodeInfo {
            zid: zid.to_string(),
            kind: kind.to_string(),
            locators: vec![],
            metadata,
            sources: NodeSources::ADMIN,
            admin_last_seen: None,
            scout_last_seen: None,
        }
    }

    #[test]
    fn builds_edges_from_sessions() {
        let nodes = vec![
            node("r1", "router", Some(json!([{"peer":"p1","links":[{"dst":"tcp/1"}]}]))),
            node("p1", "peer", Some(json!([]))),
        ];
        let t = build_topology(&nodes);
        assert_eq!(t.edges.len(), 1);
        assert_eq!(t.edges[0].from_zid, "r1");
        assert_eq!(t.edges[0].to_zid, "p1");
        assert_eq!(t.edges[0].link_dst.as_deref(), Some("tcp/1"));
        assert!(!t.partial, "both nodes have session metadata");
    }

    #[test]
    fn duplicate_sessions_dedupe() {
        let nodes = vec![node(
            "r1",
            "router",
            Some(json!([
                {"peer":"p1","links":[{"dst":"tcp/1"}]},
                {"peer":"p1","links":[{"dst":"tcp/1"}]}
            ])),
        )];
        let t = build_topology(&nodes);
        assert_eq!(t.edges.len(), 1);
    }

    #[test]
    fn dangling_peer_is_marked_and_partial() {
        // r1 references p1, which is not in the registry.
        let nodes = vec![node("r1", "router", Some(json!([{"peer":"p1","links":[]}])))];
        let t = build_topology(&nodes);
        assert!(t.partial);
        let p1 = t.nodes.iter().find(|n| n.zid == "p1").unwrap();
        assert!(!p1.known);
    }

    #[test]
    fn node_without_metadata_makes_partial() {
        let nodes = vec![node("r1", "router", None)];
        let t = build_topology(&nodes);
        assert!(t.partial);
        assert!(t.edges.is_empty());
    }

    #[test]
    fn cycle_is_safe() {
        // r1 <-> r2 mutual edges must not loop.
        let nodes = vec![
            node("r1", "router", Some(json!([{"peer":"r2","links":[]}]))),
            node("r2", "router", Some(json!([{"peer":"r1","links":[]}]))),
        ];
        let t = build_topology(&nodes);
        assert_eq!(t.edges.len(), 2);
        let lines = to_adjacency_lines(&t);
        assert!(lines.iter().any(|l| l.contains("r1")));
        assert!(lines.iter().any(|l| l.contains("r2")));
    }
}
