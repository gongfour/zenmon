//! The hierarchical index over observed key expressions.
//!
//! A flat, sorted key list stops being usable somewhere around fifty keys, and a
//! namespace like `agv/{id}/pose` reaches that with twenty vehicles. This splits
//! each key on `/` and keeps the segments as a tree, so a whole fleet collapses
//! to one row you expand.
//!
//! Purely structural: no ratatui, no zenoh, no clock. Recency lives in the app's
//! `topic_latest`, so there is one source of truth for when a key was last seen
//! and this module stays testable without a terminal.

use std::collections::{BTreeMap, HashSet};

/// One node in the key hierarchy.
///
/// `path` is stored rather than rebuilt on demand because it is the identity
/// used by the expansion sets and by selection, and the flatten pass needs it
/// for every visible row on every frame.
#[derive(Debug, Clone)]
struct TreeNode {
    /// This node's own segment (`"f1"`). Empty for the root.
    segment: String,
    /// Full key prefix down to and including this node (`"agv/f1"`).
    path: String,
    children: BTreeMap<String, TreeNode>,
    /// A message has arrived on exactly this path, so it is a real key and not
    /// just an intermediate segment.
    is_key: bool,
    /// Keys at or below this node, counting itself when `is_key`. Maintained on
    /// insert/remove so a collapsed row can show what it is hiding without a
    /// walk.
    key_count: usize,
}

impl TreeNode {
    fn new(segment: &str, path: &str) -> Self {
        Self {
            segment: segment.to_string(),
            path: path.to_string(),
            children: BTreeMap::new(),
            is_key: false,
            key_count: 0,
        }
    }
}

/// What a flattened row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// An intermediate segment. `expanded` is what the row should draw, which
    /// under an active filter can be true even when the user never expanded it.
    Branch { expanded: bool },
    /// A real key: a message has arrived on exactly this path.
    Leaf,
    /// Stands in for `hidden` sibling rows that were folded away. Carries its
    /// parent's `path`, so unfolding is "add this path to `unfolded`".
    FoldSummary { hidden: usize },
}

/// One visible row produced by [`KeyTree::flatten`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub path: String,
    pub segment: String,
    pub depth: u16,
    pub kind: RowKind,
    /// Keys at or below this row.
    pub key_count: usize,
}

/// How to flatten the tree into rows.
pub struct FlattenOpts<'a> {
    /// Branches the user has opened.
    pub expanded: &'a HashSet<String>,
    /// Branches whose over-threshold children the user has asked to see in full.
    pub unfolded: &'a HashSet<String>,
    /// Case-insensitive substring filter. Empty means no filtering.
    pub filter: &'a str,
    /// Above this many children, an expanded branch shows one summary row
    /// instead of listing them.
    pub fold_threshold: usize,
}

/// The tree of every key expression seen this session.
#[derive(Debug, Clone)]
pub struct KeyTree {
    root: TreeNode,
    version: u64,
}

impl Default for KeyTree {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyTree {
    pub fn new() -> Self {
        Self {
            root: TreeNode::new("", ""),
            version: 0,
        }
    }

    /// Bumped whenever the structure changes, never on a repeat of a key already
    /// present. The render cache keys off this, so a steady stream of messages
    /// on known keys costs no re-flattening.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Total keys in the tree.
    pub fn len(&self) -> usize {
        self.root.key_count
    }

    pub fn is_empty(&self) -> bool {
        self.root.key_count == 0
    }

    /// Record a key. Returns whether this changed the tree — false when the key
    /// was already present, which is the common case once a stream is running.
    pub fn insert(&mut self, key: &str) -> bool {
        let segments: Vec<&str> = key.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return false;
        }

        // Walk down first to find out whether this is new, so `key_count` is
        // only incremented on a genuine insert.
        if self.contains(key) {
            return false;
        }

        let mut node = &mut self.root;
        node.key_count += 1;
        let mut path = String::new();
        for segment in segments {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(segment);
            node = node
                .children
                .entry(segment.to_string())
                .or_insert_with(|| TreeNode::new(segment, &path));
            node.key_count += 1;
        }
        node.is_key = true;
        self.version = self.version.wrapping_add(1);
        true
    }

    /// Whether `key` has been recorded as a key (not merely as a prefix of one).
    pub fn contains(&self, key: &str) -> bool {
        self.find(key).is_some_and(|n| n.is_key)
    }

    /// Forget a key, pruning any ancestors it was the last key under. Returns
    /// whether anything was removed.
    pub fn remove(&mut self, key: &str) -> bool {
        if !self.contains(key) {
            return false;
        }
        let segments: Vec<String> = key
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        Self::remove_inner(&mut self.root, &segments);
        self.version = self.version.wrapping_add(1);
        true
    }

    fn remove_inner(node: &mut TreeNode, segments: &[String]) {
        node.key_count -= 1;
        let Some((head, rest)) = segments.split_first() else {
            node.is_key = false;
            return;
        };
        let Some(child) = node.children.get_mut(head) else {
            return;
        };
        Self::remove_inner(child, rest);
        // An interior node that is neither a key nor on the way to one is no
        // longer worth a row.
        if child.key_count == 0 {
            node.children.remove(head);
        }
    }

    fn find(&self, key: &str) -> Option<&TreeNode> {
        let mut node = &self.root;
        for segment in key.split('/').filter(|s| !s.is_empty()) {
            node = node.children.get(segment)?;
        }
        Some(node)
    }

    /// Keys at or below `path`, or 0 if the path is unknown.
    pub fn key_count_at(&self, path: &str) -> usize {
        self.find(path).map_or(0, |n| n.key_count)
    }

    /// Whether `path` names a branch that has children (as opposed to a leaf).
    pub fn has_children(&self, path: &str) -> bool {
        self.find(path).is_some_and(|n| !n.children.is_empty())
    }

    /// Every path that has children, in no particular order.
    ///
    /// Used to open a small tree completely, so a handful of keys looks like the
    /// flat list it effectively is rather than making the user expand to reach
    /// anything.
    pub fn branch_paths(&self) -> Vec<String> {
        let mut out = Vec::new();
        Self::collect_branches(&self.root, &mut out);
        out
    }

    fn collect_branches(node: &TreeNode, out: &mut Vec<String>) {
        for child in node.children.values() {
            if !child.children.is_empty() {
                out.push(child.path.clone());
                Self::collect_branches(child, out);
            }
        }
    }

    /// The run of branches from the root down while each has exactly one child.
    ///
    /// A namespace under a single common prefix (`agv/...` and nothing else)
    /// would otherwise open on one useless row; this opens through the shared
    /// stem to the first point where there is actually a choice to make.
    pub fn single_child_chain(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut node = &self.root;
        while node.children.len() == 1 {
            let child = node.children.values().next().expect("len checked");
            if child.children.is_empty() {
                break;
            }
            out.push(child.path.clone());
            node = child;
        }
        out
    }

    /// The visible rows, in display order.
    ///
    /// Two rules make the filter and the fold compose:
    ///
    /// - A branch is kept when its own path matches **or** any descendant's
    ///   does. Since a child's path contains its parent's as a prefix, a branch
    ///   that matches keeps its whole subtree without a second test.
    /// - Filtering ignores `fold_threshold` entirely. Typing a filter is how you
    ///   look inside a folded group, so folding the results back up would close
    ///   the only door into them.
    ///
    /// While a filter is active, branches on the way to a match draw as expanded
    /// without touching `expanded` — the user's own expansion state is left
    /// alone so clearing the filter restores exactly the tree they had.
    pub fn flatten(&self, opts: &FlattenOpts) -> Vec<TreeRow> {
        let needle = opts.filter.to_lowercase();
        let mut rows = Vec::new();
        for child in self.root.children.values() {
            Self::walk(child, opts, &needle, 0, false, &mut rows);
        }
        rows
    }

    /// Append `node`'s rows. Returns whether it kept anything.
    ///
    /// The row is pushed before the children are known, then truncated away if
    /// nothing under it matched — one pass instead of a matches-anywhere probe
    /// per node.
    fn walk(
        node: &TreeNode,
        opts: &FlattenOpts,
        needle: &str,
        depth: u16,
        ancestor_matched: bool,
        rows: &mut Vec<TreeRow>,
    ) -> bool {
        let filtering = !needle.is_empty();
        let self_matched =
            ancestor_matched || !filtering || node.path.to_lowercase().contains(needle);

        let is_branch = !node.children.is_empty();
        // Under a filter every branch on a kept path is drawn open; otherwise it
        // is whatever the user left it as.
        let expanded = if filtering {
            true
        } else {
            opts.expanded.contains(&node.path)
        };

        let mark = rows.len();
        rows.push(TreeRow {
            path: node.path.clone(),
            segment: node.segment.clone(),
            depth,
            kind: if is_branch {
                RowKind::Branch { expanded }
            } else {
                RowKind::Leaf
            },
            key_count: node.key_count,
        });

        let mut kept_child = false;
        if is_branch && expanded {
            let folded = !filtering
                && node.children.len() > opts.fold_threshold
                && !opts.unfolded.contains(&node.path);
            if folded {
                rows.push(TreeRow {
                    path: node.path.clone(),
                    segment: String::new(),
                    depth: depth + 1,
                    kind: RowKind::FoldSummary {
                        hidden: node.children.len(),
                    },
                    key_count: node.key_count,
                });
                kept_child = true;
            } else {
                for child in node.children.values() {
                    kept_child |= Self::walk(child, opts, needle, depth + 1, self_matched, rows);
                }
            }
        }

        if self_matched || kept_child {
            true
        } else {
            rows.truncate(mark);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(keys: &[&str]) -> KeyTree {
        let mut t = KeyTree::new();
        for k in keys {
            t.insert(k);
        }
        t
    }

    fn opts<'a>(
        expanded: &'a HashSet<String>,
        unfolded: &'a HashSet<String>,
        filter: &'a str,
    ) -> FlattenOpts<'a> {
        FlattenOpts {
            expanded,
            unfolded,
            filter,
            fold_threshold: 12,
        }
    }

    fn set(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    fn paths(rows: &[TreeRow]) -> Vec<&str> {
        rows.iter().map(|r| r.path.as_str()).collect()
    }

    #[test]
    fn insert_builds_branches_and_marks_only_the_leaf_as_a_key() {
        let t = tree(&["a/b/c"]);
        assert!(t.contains("a/b/c"));
        // Intermediate segments exist as structure but are not keys themselves.
        assert!(!t.contains("a"));
        assert!(!t.contains("a/b"));
        assert!(t.has_children("a"));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn reinserting_a_known_key_does_not_bump_the_version() {
        // The whole point of the version: a stream on known keys must not force
        // the flatten cache to rebuild on every message.
        let mut t = tree(&["a/b"]);
        let v = t.version();
        assert!(!t.insert("a/b"));
        assert_eq!(t.version(), v);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn key_count_accumulates_up_the_path() {
        let t = tree(&["agv/f1/pose", "agv/f1/battery", "agv/f2/pose"]);
        assert_eq!(t.key_count_at("agv"), 3);
        assert_eq!(t.key_count_at("agv/f1"), 2);
        assert_eq!(t.key_count_at("agv/f2"), 1);
        assert_eq!(t.key_count_at("nope"), 0);
    }

    #[test]
    fn a_node_can_be_both_a_key_and_a_branch() {
        // `a/b` receives messages *and* has `a/b/c` under it: it counts itself.
        let t = tree(&["a/b", "a/b/c"]);
        assert!(t.contains("a/b"));
        assert!(t.has_children("a/b"));
        assert_eq!(t.key_count_at("a/b"), 2);
    }

    #[test]
    fn remove_prunes_ancestors_that_held_nothing_else() {
        let mut t = tree(&["a/b/c", "a/x"]);
        assert!(t.remove("a/b/c"));
        assert_eq!(t.len(), 1);
        // `a/b` existed only to reach `a/b/c`, so it goes too; `a` still holds `a/x`.
        assert!(!t.has_children("a/b"));
        assert_eq!(t.key_count_at("a"), 1);
        assert!(!t.remove("a/b/c"));
    }

    #[test]
    fn removing_a_key_that_is_also_a_branch_keeps_the_branch() {
        let mut t = tree(&["a/b", "a/b/c"]);
        assert!(t.remove("a/b"));
        assert!(!t.contains("a/b"));
        // `a/b/c` still needs `a/b` as a path segment.
        assert!(t.has_children("a/b"));
        assert_eq!(t.key_count_at("a/b"), 1);
    }

    #[test]
    fn collapsed_branches_hide_their_descendants() {
        let t = tree(&["a/b/c", "a/b/d"]);
        let (e, u) = (set(&[]), set(&[]));
        assert_eq!(paths(&t.flatten(&opts(&e, &u, ""))), vec!["a"]);

        let e = set(&["a"]);
        assert_eq!(paths(&t.flatten(&opts(&e, &u, ""))), vec!["a", "a/b"]);

        let e = set(&["a", "a/b"]);
        assert_eq!(
            paths(&t.flatten(&opts(&e, &u, ""))),
            vec!["a", "a/b", "a/b/c", "a/b/d"]
        );
    }

    #[test]
    fn rows_carry_depth_and_kind() {
        let t = tree(&["a/b"]);
        let (e, u) = (set(&["a"]), set(&[]));
        let rows = t.flatten(&opts(&e, &u, ""));
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[0].kind, RowKind::Branch { expanded: true });
        assert_eq!(rows[0].segment, "a");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].kind, RowKind::Leaf);
        assert_eq!(rows[1].segment, "b");
    }

    #[test]
    fn a_matching_branch_keeps_its_whole_subtree() {
        let t = tree(&["agv/f1/pose", "agv/f1/battery", "srv/health"]);
        let (e, u) = (set(&[]), set(&[]));
        // "agv" matches, so everything under it shows without matching itself.
        assert_eq!(
            paths(&t.flatten(&opts(&e, &u, "agv"))),
            vec!["agv", "agv/f1", "agv/f1/battery", "agv/f1/pose"]
        );
    }

    #[test]
    fn a_deep_match_pulls_its_ancestors_along() {
        let t = tree(&["agv/f1/pose", "agv/f2/battery", "srv/health"]);
        let (e, u) = (set(&[]), set(&[]));
        // Only `battery` matches, but its path to the root has to be walkable.
        assert_eq!(
            paths(&t.flatten(&opts(&e, &u, "battery"))),
            vec!["agv", "agv/f2", "agv/f2/battery"]
        );
    }

    #[test]
    fn filtering_expands_without_touching_the_users_expansion_state() {
        let t = tree(&["agv/f1/pose"]);
        let (e, u) = (set(&[]), set(&[]));
        // Nothing is expanded, yet the match is reachable...
        let rows = t.flatten(&opts(&e, &u, "pose"));
        assert_eq!(paths(&rows), vec!["agv", "agv/f1", "agv/f1/pose"]);
        assert_eq!(rows[0].kind, RowKind::Branch { expanded: true });
        // ...and clearing the filter returns the collapsed tree unchanged.
        assert_eq!(paths(&t.flatten(&opts(&e, &u, ""))), vec!["agv"]);
    }

    #[test]
    fn a_filter_matching_nothing_yields_no_rows() {
        let t = tree(&["agv/f1/pose"]);
        let (e, u) = (set(&[]), set(&[]));
        assert!(t.flatten(&opts(&e, &u, "zzz")).is_empty());
    }

    #[test]
    fn filtering_is_case_insensitive() {
        let t = tree(&["AGV/F1/Pose"]);
        let (e, u) = (set(&[]), set(&[]));
        assert_eq!(t.flatten(&opts(&e, &u, "pose")).len(), 3);
    }

    #[test]
    fn an_expanded_branch_over_the_threshold_folds_to_one_row() {
        let mut t = KeyTree::new();
        for i in 0..200 {
            t.insert(&format!("agv/f{i:03}/pose"));
        }
        let (e, u) = (set(&["agv"]), set(&[]));
        let rows = t.flatten(&opts(&e, &u, ""));
        assert_eq!(rows.len(), 2, "expected the branch plus one summary");
        assert_eq!(rows[0].kind, RowKind::Branch { expanded: true });
        assert_eq!(rows[1].kind, RowKind::FoldSummary { hidden: 200 });
        // The summary carries its parent's path so unfolding is a set insert.
        assert_eq!(rows[1].path, "agv");
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn unfolding_lists_the_children_after_all() {
        let mut t = KeyTree::new();
        for i in 0..20 {
            t.insert(&format!("agv/f{i:02}/pose"));
        }
        let (e, u) = (set(&["agv"]), set(&["agv"]));
        let rows = t.flatten(&opts(&e, &u, ""));
        // The branch plus its 20 children (each still collapsed).
        assert_eq!(rows.len(), 21);
        assert!(rows
            .iter()
            .all(|r| !matches!(r.kind, RowKind::FoldSummary { .. })));
    }

    #[test]
    fn a_branch_at_the_threshold_is_not_folded() {
        let mut t = KeyTree::new();
        for i in 0..12 {
            t.insert(&format!("agv/f{i:02}"));
        }
        let (e, u) = (set(&["agv"]), set(&[]));
        let rows = t.flatten(&opts(&e, &u, ""));
        assert_eq!(rows.len(), 13, "12 is the threshold, not over it");
    }

    #[test]
    fn filtering_overrides_folding() {
        // Filtering is the documented way to find one id inside a folded group,
        // so folding the filtered results back up would defeat it.
        let mut t = KeyTree::new();
        for i in 0..200 {
            t.insert(&format!("agv/f{i:03}/pose"));
        }
        let (e, u) = (set(&[]), set(&[]));
        let rows = t.flatten(&opts(&e, &u, "f017"));
        assert_eq!(paths(&rows), vec!["agv", "agv/f017", "agv/f017/pose"]);
    }

    #[test]
    fn siblings_come_out_sorted() {
        let t = tree(&["b/x", "a/x", "c/x"]);
        let (e, u) = (set(&[]), set(&[]));
        assert_eq!(paths(&t.flatten(&opts(&e, &u, ""))), vec!["a", "b", "c"]);
    }

    #[test]
    fn leading_and_repeated_slashes_do_not_create_empty_segments() {
        let mut t = KeyTree::new();
        assert!(t.insert("/a//b/"));
        assert!(t.contains("a/b"));
        assert_eq!(t.len(), 1);
        let (e, u) = (set(&["a"]), set(&[]));
        assert_eq!(paths(&t.flatten(&opts(&e, &u, ""))), vec!["a", "a/b"]);
    }

    #[test]
    fn branch_paths_lists_every_node_with_children() {
        let t = tree(&["agv/f1/pose", "agv/f2/pose", "srv/health"]);
        let mut got = t.branch_paths();
        got.sort();
        assert_eq!(got, vec!["agv", "agv/f1", "agv/f2", "srv"]);
    }

    #[test]
    fn single_child_chain_stops_at_the_first_real_choice() {
        let t = tree(&["agv/f1/pose", "agv/f2/pose"]);
        // `agv` is the only root child, but under it there are two vehicles, so
        // opening past `agv` would be a guess.
        assert_eq!(t.single_child_chain(), vec!["agv"]);
    }

    #[test]
    fn single_child_chain_walks_a_long_stem() {
        let t = tree(&["a/b/c/d"]);
        assert_eq!(t.single_child_chain(), vec!["a", "a/b", "a/b/c"]);
    }

    #[test]
    fn single_child_chain_is_empty_when_the_root_branches() {
        let t = tree(&["a/x", "b/x"]);
        assert!(t.single_child_chain().is_empty());
    }

    #[test]
    fn an_empty_key_is_ignored() {
        let mut t = KeyTree::new();
        assert!(!t.insert(""));
        assert!(!t.insert("///"));
        assert!(t.is_empty());
        assert_eq!(t.version(), 0);
    }
}
