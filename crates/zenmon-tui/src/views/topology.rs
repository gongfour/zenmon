use zenmon_core::types::NodeInfo;
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

const STALE_THRESHOLD: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub enum TopoRow {
    Header(String),
    Node(TopoNode),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TopoNode {
    pub zid: String,
    pub kind: String,
    pub locator: String,
    pub is_child: bool,
    pub alive: bool,
    pub is_self: bool,
    /// True when a full `NodeInfo` backs this row (detail lookup will succeed).
    pub in_registry: bool,
}

fn best_locator(node: &NodeInfo) -> String {
    node.locators.first().cloned().unwrap_or_else(|| "-".to_string())
}

/// Parse a router's admin `metadata.sessions` into (peer_zid, whatami, link_dst).
fn parse_sessions(node: &NodeInfo) -> Vec<(String, String, String)> {
    let Some(meta) = &node.metadata else { return Vec::new() };
    let Some(sessions) = meta.get("sessions").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter_map(|s| {
            let peer = s.get("peer").and_then(|v| v.as_str())?.to_string();
            let whatami = s
                .get("whatami")
                .and_then(|v| v.as_str())
                .unwrap_or("peer")
                .to_string();
            let link = s
                .get("links")
                .and_then(|v| v.as_array())
                .and_then(|l| l.first())
                .and_then(|l| l.get("dst"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some((peer, whatami, link))
        })
        .collect()
}

/// Build the flat topology row list: routers as roots, their non-router
/// sessions as children, remaining non-router nodes under an "unlinked" group.
/// When no routers exist, every node is listed flat under a "no router" header.
pub fn build_topology_rows(
    nodes: &[NodeInfo],
    self_zid: Option<&str>,
    now: SystemTime,
) -> Vec<TopoRow> {
    let is_self = |zid: &str| self_zid == Some(zid);
    let mk = |n: &NodeInfo, is_child: bool| {
        TopoRow::Node(TopoNode {
            zid: n.zid.clone(),
            kind: n.kind.clone(),
            locator: best_locator(n),
            is_child,
            alive: !n.is_scout_stale(now, STALE_THRESHOLD),
            is_self: is_self(&n.zid),
            in_registry: true,
        })
    };

    let mut routers: Vec<&NodeInfo> = nodes.iter().filter(|n| n.kind == "router").collect();
    routers.sort_by(|a, b| a.zid.cmp(&b.zid));

    if routers.is_empty() {
        if nodes.is_empty() {
            return Vec::new();
        }
        let mut rows = vec![TopoRow::Header("── nodes (no router) ──".to_string())];
        let mut sorted: Vec<&NodeInfo> = nodes.iter().collect();
        sorted.sort_by(|a, b| a.zid.cmp(&b.zid));
        rows.extend(sorted.into_iter().map(|n| mk(n, false)));
        return rows;
    }

    let mut rows = Vec::new();
    let mut child_zids: HashSet<String> = HashSet::new();

    for router in &routers {
        rows.push(mk(router, false));
        let mut seen = HashSet::new();
        for (peer_zid, whatami, link) in parse_sessions(router) {
            if !seen.insert(peer_zid.clone()) {
                continue; // dedup within a single router
            }
            // Routers are shown only as their own roots, never as children.
            if nodes.iter().any(|n| n.zid == peer_zid && n.kind == "router") {
                continue;
            }
            child_zids.insert(peer_zid.clone());
            match nodes.iter().find(|n| n.zid == peer_zid) {
                Some(n) => rows.push(mk(n, true)),
                None => rows.push(TopoRow::Node(TopoNode {
                    zid: peer_zid.clone(),
                    kind: whatami,
                    locator: if link.is_empty() { "-".to_string() } else { link },
                    is_child: true,
                    alive: true,
                    is_self: is_self(&peer_zid),
                    in_registry: false,
                })),
            }
        }
    }

    let mut unlinked: Vec<&NodeInfo> = nodes
        .iter()
        .filter(|n| n.kind != "router" && !child_zids.contains(&n.zid))
        .collect();
    unlinked.sort_by(|a, b| a.zid.cmp(&b.zid));
    if !unlinked.is_empty() {
        rows.push(TopoRow::Header("── unlinked (scouted) ──".to_string()));
        rows.extend(unlinked.into_iter().map(|n| mk(n, false)));
    }

    rows
}

pub fn node_row_count(rows: &[TopoRow]) -> usize {
    rows.iter().filter(|r| matches!(r, TopoRow::Node(_))).count()
}

pub fn nth_node_zid(rows: &[TopoRow], n: usize) -> Option<&str> {
    rows.iter()
        .filter_map(|r| match r {
            TopoRow::Node(x) => Some(x.zid.as_str()),
            _ => None,
        })
        .nth(n)
}

/// Map a visual row index (headers included) to a selectable node index.
/// Returns `None` if the visual row is a header or out of range.
pub fn node_index_at_visual(rows: &[TopoRow], visual: usize) -> Option<usize> {
    let mut node_idx = 0;
    for (i, r) in rows.iter().enumerate() {
        match r {
            TopoRow::Node(_) => {
                if i == visual {
                    return Some(node_idx);
                }
                node_idx += 1;
            }
            TopoRow::Header(_) => {
                if i == visual {
                    return None;
                }
            }
        }
    }
    None
}

/// Inverse of `node_index_at_visual`: the visual row index of the n-th node.
pub fn visual_index_of_node(rows: &[TopoRow], node_idx: usize) -> Option<usize> {
    let mut seen = 0;
    for (i, r) in rows.iter().enumerate() {
        if let TopoRow::Node(_) = r {
            if seen == node_idx {
                return Some(i);
            }
            seen += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenmon_core::types::{NodeInfo, NodeSources};
    use std::time::{Duration, SystemTime};

    fn node(zid: &str, kind: &str, sources: NodeSources) -> NodeInfo {
        NodeInfo {
            zid: zid.into(),
            kind: kind.into(),
            locators: vec!["tcp/1.2.3.4:7447".into()],
            metadata: None,
            sources,
            admin_last_seen: None,
            scout_last_seen: None,
        }
    }

    fn router_with_sessions(zid: &str, peers: &[&str]) -> NodeInfo {
        let sessions: Vec<_> = peers
            .iter()
            .map(|p| serde_json::json!({
                "peer": p, "whatami": "peer",
                "links": [{"dst": "tcp/9.9.9.9:41000"}]
            }))
            .collect();
        let mut n = node(zid, "router", NodeSources::ADMIN);
        n.metadata = Some(serde_json::json!({ "sessions": sessions }));
        n
    }

    fn node_zids(rows: &[TopoRow]) -> Vec<&str> {
        rows.iter()
            .filter_map(|r| match r {
                TopoRow::Node(n) => Some(n.zid.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn router_lists_its_sessions_as_children() {
        let nodes = vec![
            router_with_sessions("r1", &["p1", "p2"]),
            node("p1", "peer", NodeSources::ADMIN),
            node("p2", "peer", NodeSources::ADMIN),
        ];
        let rows = build_topology_rows(&nodes, None, SystemTime::now());
        assert_eq!(node_row_count(&rows), 3);
        match (&rows[0], &rows[1], &rows[2]) {
            (TopoRow::Node(r), TopoRow::Node(a), TopoRow::Node(b)) => {
                assert_eq!(r.zid, "r1");
                assert!(!r.is_child);
                assert_eq!(a.zid, "p1");
                assert!(a.is_child);
                assert_eq!(b.zid, "p2");
                assert!(b.is_child);
            }
            _ => panic!("expected 3 node rows"),
        }
    }

    #[test]
    fn peer_under_two_routers_appears_twice() {
        let nodes = vec![
            router_with_sessions("r1", &["p1"]),
            router_with_sessions("r2", &["p1"]),
            node("p1", "peer", NodeSources::ADMIN),
        ];
        let rows = build_topology_rows(&nodes, None, SystemTime::now());
        let p1_count = node_zids(&rows).iter().filter(|z| **z == "p1").count();
        assert_eq!(p1_count, 2);
    }

    #[test]
    fn orphan_non_router_goes_to_unlinked_group() {
        let nodes = vec![
            router_with_sessions("r1", &[]),
            node("p9", "peer", NodeSources::SCOUT),
        ];
        let rows = build_topology_rows(&nodes, None, SystemTime::now());
        let header_pos = rows
            .iter()
            .position(|r| matches!(r, TopoRow::Header(h) if h.contains("unlinked")));
        assert!(header_pos.is_some(), "expected an unlinked header");
        let p9_pos = node_zids(&rows).iter().position(|z| z == &"p9");
        assert!(p9_pos.is_some());
    }

    #[test]
    fn no_router_produces_flat_list_under_header() {
        let nodes = vec![
            node("p1", "peer", NodeSources::ADMIN),
            node("p2", "peer", NodeSources::ADMIN),
        ];
        let rows = build_topology_rows(&nodes, None, SystemTime::now());
        assert!(matches!(&rows[0], TopoRow::Header(h) if h.contains("no router")));
        assert_eq!(node_zids(&rows), vec!["p1", "p2"]);
    }

    #[test]
    fn empty_nodes_produce_no_rows() {
        let rows = build_topology_rows(&[], None, SystemTime::now());
        assert!(rows.is_empty());
    }

    #[test]
    fn self_node_is_marked() {
        let nodes = vec![node("me", "peer", NodeSources::ADMIN)];
        let rows = build_topology_rows(&nodes, Some("me"), SystemTime::now());
        match &rows[1] {
            TopoRow::Node(n) => assert!(n.is_self),
            _ => panic!("expected node row after header"),
        }
    }

    #[test]
    fn scout_only_stale_node_is_not_alive() {
        let now = SystemTime::now();
        let mut n = node("s1", "peer", NodeSources::SCOUT);
        n.scout_last_seen = Some(now - Duration::from_secs(60));
        let rows = build_topology_rows(&[n], None, now);
        match &rows[1] {
            TopoRow::Node(node) => assert!(!node.alive),
            _ => panic!("expected node row after header"),
        }
    }

    #[test]
    fn node_index_at_visual_skips_headers() {
        let rows = vec![
            TopoRow::Header("h".into()),
            TopoRow::Node(TopoNode {
                zid: "a".into(), kind: "peer".into(), locator: "-".into(),
                is_child: false, alive: true, is_self: false, in_registry: true,
            }),
        ];
        assert_eq!(node_index_at_visual(&rows, 0), None); // header
        assert_eq!(node_index_at_visual(&rows, 1), Some(0)); // first node
        assert_eq!(nth_node_zid(&rows, 0), Some("a"));
        assert_eq!(visual_index_of_node(&rows, 0), Some(1));
    }
}
