use crate::types::{NodeInfo, NodeSources};
use std::collections::HashMap;

/// Merge admin-derived and scout-derived nodes into a deduped, sorted list.
pub fn merge_nodes(admin: &[NodeInfo], scout: &[NodeInfo]) -> Vec<NodeInfo> {
    let mut by_zid: HashMap<String, NodeInfo> = HashMap::new();

    for n in admin {
        by_zid.insert(n.zid.clone(), n.clone());
    }

    for s in scout {
        by_zid
            .entry(s.zid.clone())
            .and_modify(|existing| {
                existing.sources |= NodeSources::SCOUT;
                if s.scout_last_seen.is_some() {
                    existing.scout_last_seen = s.scout_last_seen;
                }
                for loc in &s.locators {
                    if !existing.locators.contains(loc) {
                        existing.locators.push(loc.clone());
                    }
                }
            })
            .or_insert_with(|| s.clone());
    }

    let mut out: Vec<NodeInfo> = by_zid.into_values().collect();
    out.sort_by(|a, b| a.zid.cmp(&b.zid));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn admin_node(zid: &str, kind: &str, locators: &[&str]) -> NodeInfo {
        NodeInfo {
            zid: zid.into(),
            kind: kind.into(),
            locators: locators.iter().map(|s| s.to_string()).collect(),
            metadata: None,
            sources: NodeSources::ADMIN,
            admin_last_seen: Some(SystemTime::now()),
            scout_last_seen: None,
        }
    }

    fn scout_node(zid: &str, kind: &str, locators: &[&str]) -> NodeInfo {
        NodeInfo {
            zid: zid.into(),
            kind: kind.into(),
            locators: locators.iter().map(|s| s.to_string()).collect(),
            metadata: None,
            sources: NodeSources::SCOUT,
            admin_last_seen: None,
            scout_last_seen: Some(SystemTime::now()),
        }
    }

    #[test]
    fn merge_admin_only_passes_through_sorted() {
        let admin = vec![
            admin_node("z2", "peer", &["tcp/1.1.1.1:7447"]),
            admin_node("z1", "router", &["tcp/2.2.2.2:7447"]),
        ];
        let out = merge_nodes(&admin, &[]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].zid, "z1");
        assert_eq!(out[1].zid, "z2");
        assert_eq!(out[0].sources, NodeSources::ADMIN);
        assert!(out[0].admin_last_seen.is_some());
        assert!(out[1].admin_last_seen.is_some());
    }

    #[test]
    fn merge_scout_only_passes_through_sorted() {
        let scout = vec![
            scout_node("z2", "peer", &["tcp/3.3.3.3:7447"]),
            scout_node("z1", "router", &["tcp/4.4.4.4:7447"]),
        ];
        let out = merge_nodes(&[], &scout);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].zid, "z1");
        assert_eq!(out[0].sources, NodeSources::SCOUT);
        assert!(out[0].scout_last_seen.is_some());
        assert!(out[1].scout_last_seen.is_some());
    }

    #[test]
    fn merge_overlapping_zid_unions_sources_and_locators() {
        let admin = vec![admin_node("z1", "router", &["tcp/a:7447"])];
        let scout = vec![scout_node("z1", "peer", &["tcp/b:7447"])];
        let out = merge_nodes(&admin, &scout);
        assert_eq!(out.len(), 1);
        let n = &out[0];
        assert_eq!(n.zid, "z1");
        assert_eq!(n.kind, "router");
        assert!(n.sources.contains(NodeSources::ADMIN));
        assert!(n.sources.contains(NodeSources::SCOUT));
        assert!(n.admin_last_seen.is_some());
        assert!(n.scout_last_seen.is_some());
        assert!(n.locators.contains(&"tcp/a:7447".to_string()));
        assert!(n.locators.contains(&"tcp/b:7447".to_string()));
    }

    #[test]
    fn merge_disjoint_zids_produces_sorted_union() {
        let admin = vec![admin_node("z3", "router", &[])];
        let scout = vec![
            scout_node("z1", "peer", &[]),
            scout_node("z2", "peer", &[]),
        ];
        let out = merge_nodes(&admin, &scout);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].zid, "z1");
        assert_eq!(out[1].zid, "z2");
        assert_eq!(out[2].zid, "z3");
        assert_eq!(out[0].sources, NodeSources::SCOUT);
        assert_eq!(out[1].sources, NodeSources::SCOUT);
        assert_eq!(out[2].sources, NodeSources::ADMIN);
    }
}
