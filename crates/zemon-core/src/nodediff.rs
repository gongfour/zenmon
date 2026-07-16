//! Node snapshot diffing for `nodes --watch --changes-only`.
//!
//! Comparing full `NodeInfo` values directly produces spurious "changed" events
//! because `admin_last_seen` is refreshed every poll and array orders aren't
//! stable. We instead diff a **normalized** snapshot keyed by ZID: kind plus
//! sorted, de-duplicated locators, with timestamps and array order excluded.

use crate::types::NodeInfo;
use serde::Serialize;
use std::collections::BTreeMap;

/// A normalized, comparable view of a node (excludes last_seen / order noise).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeSnapshot {
    pub zid: String,
    pub kind: String,
    pub locators: Vec<String>,
}

impl NodeSnapshot {
    pub fn from_info(node: &NodeInfo) -> Self {
        let mut locators = node.locators.clone();
        locators.sort();
        locators.dedup();
        Self {
            zid: node.zid.clone(),
            kind: node.kind.clone(),
            locators,
        }
    }
}

/// A single node change event, serialized as e.g.
/// `{"event":"added","item":{...}}` or
/// `{"event":"changed","before":{...},"after":{...}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum NodeChange {
    Added { item: NodeSnapshot },
    Removed { item: NodeSnapshot },
    Changed {
        before: NodeSnapshot,
        after: NodeSnapshot,
    },
}

impl NodeChange {
    /// A compact one-line human description (no ANSI).
    pub fn describe(&self) -> String {
        match self {
            NodeChange::Added { item } => {
                format!("+ added   {} {} [{}]", item.zid, item.kind, item.locators.join(", "))
            }
            NodeChange::Removed { item } => {
                format!("- removed {} {}", item.zid, item.kind)
            }
            NodeChange::Changed { after, .. } => {
                format!("~ changed {} {} [{}]", after.zid, after.kind, after.locators.join(", "))
            }
        }
    }
}

/// Diff two normalized snapshots by ZID, returning changes in stable ZID order.
pub fn diff_nodes(prev: &[NodeSnapshot], curr: &[NodeSnapshot]) -> Vec<NodeChange> {
    let pm: BTreeMap<&str, &NodeSnapshot> =
        prev.iter().map(|n| (n.zid.as_str(), n)).collect();
    let cm: BTreeMap<&str, &NodeSnapshot> =
        curr.iter().map(|n| (n.zid.as_str(), n)).collect();

    let mut keys: Vec<&str> = pm.keys().chain(cm.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();

    let mut changes = Vec::new();
    for k in keys {
        match (pm.get(k), cm.get(k)) {
            (None, Some(c)) => changes.push(NodeChange::Added {
                item: (*c).clone(),
            }),
            (Some(p), None) => changes.push(NodeChange::Removed {
                item: (*p).clone(),
            }),
            (Some(p), Some(c)) if p != c => changes.push(NodeChange::Changed {
                before: (*p).clone(),
                after: (*c).clone(),
            }),
            _ => {}
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(zid: &str, kind: &str, locators: &[&str]) -> NodeSnapshot {
        NodeSnapshot {
            zid: zid.to_string(),
            kind: kind.to_string(),
            locators: locators.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn detects_added_and_removed() {
        let prev = vec![snap("a", "router", &["tcp/1"])];
        let curr = vec![snap("b", "peer", &["tcp/2"])];
        let changes = diff_nodes(&prev, &curr);
        assert_eq!(changes.len(), 2);
        // sorted by zid: a removed, then b added
        assert!(matches!(&changes[0], NodeChange::Removed { item } if item.zid == "a"));
        assert!(matches!(&changes[1], NodeChange::Added { item } if item.zid == "b"));
    }

    #[test]
    fn detects_kind_change() {
        let prev = vec![snap("a", "peer", &["tcp/1"])];
        let curr = vec![snap("a", "router", &["tcp/1"])];
        let changes = diff_nodes(&prev, &curr);
        assert_eq!(changes.len(), 1);
        assert!(matches!(&changes[0], NodeChange::Changed { .. }));
    }

    #[test]
    fn locator_reorder_is_not_a_change() {
        // Normalized snapshots sort locators, so a reorder must be a no-op.
        let prev = vec![NodeSnapshot::from_info(&node("a", "router", &["tcp/2", "tcp/1"]))];
        let curr = vec![NodeSnapshot::from_info(&node("a", "router", &["tcp/1", "tcp/2"]))];
        assert!(diff_nodes(&prev, &curr).is_empty());
    }

    #[test]
    fn multiple_changes_in_one_diff() {
        let prev = vec![snap("a", "router", &["tcp/1"]), snap("b", "peer", &["tcp/2"])];
        let curr = vec![snap("b", "peer", &["tcp/2"]), snap("c", "peer", &["tcp/3"])];
        let changes = diff_nodes(&prev, &curr);
        // a removed, c added (b unchanged)
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn remove_then_readd_roundtrips() {
        let base = vec![snap("a", "router", &["tcp/1"])];
        let gone: Vec<NodeSnapshot> = vec![];
        assert_eq!(diff_nodes(&base, &gone).len(), 1); // removed
        assert_eq!(diff_nodes(&gone, &base).len(), 1); // added back
    }

    // Build a NodeInfo for from_info tests.
    fn node(zid: &str, kind: &str, locators: &[&str]) -> NodeInfo {
        NodeInfo {
            zid: zid.to_string(),
            kind: kind.to_string(),
            locators: locators.iter().map(|s| s.to_string()).collect(),
            metadata: None,
            sources: crate::types::NodeSources::ADMIN,
            admin_last_seen: None,
            scout_last_seen: None,
        }
    }
}
