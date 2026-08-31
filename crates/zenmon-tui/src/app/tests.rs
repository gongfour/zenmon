//! Behavioural tests for `App`.
//!
//! Kept as one module rather than split along the same lines as the code: each
//! test drives a key or a message all the way through to rendered output, so it
//! touches several of the sibling modules at once.

use super::chrome::domain_port_label;
use super::keys::{apply_detail_scroll, detail_scroll_action, DetailScroll};
use super::mouse::{list_hit, space_tab_hit};
use super::*;
use crate::event::AppEvent;
use crate::tree::RowKind;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::time::Duration;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn uppercase_j_k_map_to_detail_scroll_without_shift() {
    // Uppercase J/K scroll regardless of whether Shift is reported.
    assert_eq!(
        detail_scroll_action(key(KeyCode::Char('J'))),
        Some(DetailScroll::Down)
    );
    assert_eq!(
        detail_scroll_action(key(KeyCode::Char('K'))),
        Some(DetailScroll::Up)
    );
    assert_eq!(
        detail_scroll_action(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT)),
        Some(DetailScroll::Down)
    );
}

#[test]
fn lowercase_j_k_are_not_detail_scroll() {
    assert_eq!(detail_scroll_action(key(KeyCode::Char('j'))), None);
    assert_eq!(detail_scroll_action(key(KeyCode::Char('k'))), None);
}

#[test]
fn rate_window_evicts_beyond_cap_and_detects_idle() {
    let mut w = RateWindow::new(3);
    w.push(10);
    w.push(20);
    w.push(30);
    w.push(40); // evicts the 10
    assert_eq!(w.series(), vec![20, 30, 40]);
    assert_eq!(w.latest(), 40);
    assert!(!w.is_all_zero());
    let mut idle = RateWindow::new(2);
    idle.push(0);
    idle.push(0);
    assert!(idle.is_all_zero());
}

#[test]
fn format_bytes_per_sec_scales_units() {
    assert_eq!(format_bytes_per_sec(500), "500 B/s");
    assert_eq!(format_bytes_per_sec(2048), "2.0 KB/s");
    assert_eq!(format_bytes_per_sec(3_145_728), "3.0 MB/s");
}

#[test]
fn narrow_below_threshold() {
    assert!(is_narrow(89));
    assert!(!is_narrow(90));
    assert!(!is_narrow(200));
}

#[test]
fn empty_reason_reflects_connection_state() {
    let mut app = App::new("test".into());
    app.connection_state = ConnectionState::Connecting;
    assert_eq!(app.topics_empty_reason(), EmptyReason::Connecting);
    app.connection_state = ConnectionState::Disconnected("x".into());
    assert_eq!(app.nodes_empty_reason(), EmptyReason::Disconnected);
}

#[test]
fn empty_reason_distinguishes_filter_from_no_data() {
    let mut app = App::new("test".into());
    app.connection_state = ConnectionState::Connected("zid".into());
    // No data at all → NoDataYet.
    assert_eq!(app.topics_empty_reason(), EmptyReason::NoDataYet);
    // Data exists but the filter hides it → FilteredOut.
    app.key_tree.insert("a/b");
    app.topic_filter = "zzz".into();
    assert_eq!(app.topics_empty_reason(), EmptyReason::FilteredOut);
}

#[test]
fn question_mark_toggles_help_overlay() {
    let mut app = App::new("test".into());
    assert_eq!(app.overlay, Overlay::None);
    app.handle_key(key(KeyCode::Char('?')));
    assert_eq!(app.overlay, Overlay::Help);
    // q closes the overlay (does not quit)
    app.handle_key(key(KeyCode::Char('q')));
    assert_eq!(app.overlay, Overlay::None);
    assert!(!app.should_quit);
}

#[test]
fn help_scrolls_and_esc_closes() {
    let mut app = App::new("test".into());
    app.handle_key(key(KeyCode::Char('?')));
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.help_scroll, 1);
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.help_scroll, 0);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn tab_and_number_keys_switch_space() {
    let mut app = App::new("test".into());
    assert_eq!(app.space, Space::Traffic);
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.space, Space::Network);
    app.handle_key(key(KeyCode::Char('1')));
    assert_eq!(app.space, Space::Traffic);
    app.handle_key(key(KeyCode::Char('2')));
    assert_eq!(app.space, Space::Network);
}

#[test]
fn enter_and_esc_move_pane_focus() {
    let mut app = App::new("test".into());
    assert_eq!(app.pane_focus, PaneFocus::Master);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pane_focus, PaneFocus::Detail);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.pane_focus, PaneFocus::Master);
}

#[test]
fn switching_space_resets_pane_focus_to_master() {
    let mut app = App::new("test".into());
    app.pane_focus = PaneFocus::Detail;
    app.handle_key(key(KeyCode::Char('2')));
    assert_eq!(app.space, Space::Network);
    assert_eq!(app.pane_focus, PaneFocus::Master);
}

#[test]
fn domain_port_label_maps_domain_range() {
    assert_eq!(domain_port_label(7446), "domain 0 (port 7446)");
    assert_eq!(domain_port_label(7450), "domain 4 (port 7450)");
    assert_eq!(domain_port_label(7546), "domain 100 (port 7546)");
}

#[test]
fn domain_port_label_treats_out_of_range_as_custom_port() {
    // Below 7446 must not be misreported as "domain 0".
    assert_eq!(domain_port_label(7000), "port 7000 (custom)");
    assert_eq!(domain_port_label(8000), "port 8000 (custom)");
}

#[test]
fn apply_detail_scroll_saturates_at_zero() {
    assert_eq!(apply_detail_scroll(0, DetailScroll::Up), 0);
    assert_eq!(apply_detail_scroll(0, DetailScroll::Down), 3);
    assert_eq!(apply_detail_scroll(5, DetailScroll::Up), 2);
}

#[test]
fn space_tab_hit_inside_rect_returns_index() {
    let rects = [Some(Rect::new(1, 0, 9, 1)), Some(Rect::new(12, 0, 9, 1))];
    assert_eq!(space_tab_hit(&rects, 2, 0), Some(0));
    assert_eq!(space_tab_hit(&rects, 14, 0), Some(1));
}

#[test]
fn space_tab_hit_outside_returns_none() {
    let rects = [Some(Rect::new(1, 0, 9, 1)), None];
    assert_eq!(space_tab_hit(&rects, 50, 0), None);
    assert_eq!(space_tab_hit(&rects, 2, 5), None);
}

#[test]
fn list_hit_converts_row_to_index() {
    let rect = Rect::new(0, 5, 20, 10);
    assert_eq!(list_hit(rect, 6, 0, 8, 6), Some(0));
    assert_eq!(list_hit(rect, 8, 0, 8, 6), Some(2));
    assert_eq!(list_hit(rect, 5, 0, 8, 6), None);
    assert_eq!(list_hit(rect, 15, 0, 8, 6), None);
    assert_eq!(list_hit(rect, 20, 0, 8, 6), None);
    assert_eq!(list_hit(rect, 14, 0, 8, 6), None);
}

#[test]
fn list_hit_respects_scroll_offset() {
    let rect = Rect::new(0, 5, 20, 10);
    assert_eq!(list_hit(rect, 6, 4, 20, 6), Some(4));
    assert_eq!(list_hit(rect, 9, 4, 20, 6), Some(7));
}

#[test]
fn sub_selected_zero_stays_on_new_message() {
    let mut app = App::new("test".into());
    app.sub_selected = 0;
    let msg = ZenohMessage {
        key_expr: "a".into(),
        payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
        encoding: String::new(),
        payload_bytes: 0,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    };
    app.handle_zenoh_message(msg);
    assert_eq!(app.sub_selected, 0);
}

#[test]
fn sub_selected_nonzero_follows_message_through_shift() {
    let mut app = App::new("test".into());
    let make = |k: &str| ZenohMessage {
        key_expr: k.into(),
        payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
        encoding: String::new(),
        payload_bytes: 0,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    };
    app.handle_zenoh_message(make("a"));
    app.handle_zenoh_message(make("b"));
    app.handle_zenoh_message(make("c"));
    app.pin_stream_at(1);
    app.handle_zenoh_message(make("d"));
    assert!(!app.stream_follow);
    assert_eq!(app.sub_selected, 2);
}

#[test]
fn filtered_sub_messages_match_key_and_payload() {
    let mut app = App::new("test".into());
    app.handle_zenoh_message(ZenohMessage {
        key_expr: "robot/pose".into(),
        payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!({"x": 1})),
        encoding: String::new(),
        payload_bytes: 0,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    });
    app.handle_zenoh_message(ZenohMessage {
        key_expr: "robot/status".into(),
        payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!("idle")),
        encoding: String::new(),
        payload_bytes: 0,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    });

    app.stream_filter = "pose".into();
    assert_eq!(app.filtered_sub_messages().len(), 1);
    assert_eq!(app.filtered_sub_messages()[0].key_expr, "robot/pose");

    app.stream_filter = "idle".into();
    assert_eq!(app.filtered_sub_messages().len(), 1);
    assert_eq!(app.filtered_sub_messages()[0].key_expr, "robot/status");
}

#[test]
fn sub_selected_only_shifts_for_matching_filtered_message() {
    let mut app = App::new("test".into());
    let make = |k: &str| ZenohMessage {
        key_expr: k.into(),
        payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
        encoding: String::new(),
        payload_bytes: 0,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    };
    app.handle_zenoh_message(make("alpha/1"));
    app.handle_zenoh_message(make("beta/1"));
    app.handle_zenoh_message(make("alpha/2"));

    app.stream_filter = "alpha".into();
    app.pin_stream_at(1);

    app.handle_zenoh_message(make("beta/2"));
    assert_eq!(app.sub_selected, 1);

    app.handle_zenoh_message(make("alpha/3"));
    assert_eq!(app.sub_selected, 2);
}

#[test]
fn follow_stream_resets_selection_to_latest() {
    let mut app = App::new("test".into());
    app.stream_follow = false;
    app.sub_selected = 3;
    app.follow_stream();
    assert!(app.stream_follow);
    assert_eq!(app.sub_selected, 0);
}

#[test]
fn pin_stream_disables_follow() {
    let mut app = App::new("test".into());
    app.pin_stream_at(2);
    assert!(!app.stream_follow);
    assert_eq!(app.sub_selected, 0);
}

#[test]
fn clear_network_state_clears_topics_messages_and_nodes() {
    let mut app = App::new("test".into());
    let make = |k: &str| ZenohMessage {
        key_expr: k.into(),
        payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
        encoding: String::new(),
        payload_bytes: 0,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    };
    app.handle_zenoh_message(make("a"));
    app.handle_zenoh_message(make("b"));
    app.total_msg_count = 7;
    app.total_hz = 3.5;
    app.tree_selected = 1;
    app.topic_detail_scroll = 4;
    app.list_scroll_offset = 5;
    app.sub_selected = 1;
    app.admin_nodes.push(zenmon_core::types::NodeInfo {
        zid: "z1".into(),
        kind: "router".into(),
        locators: vec![],
        metadata: None,
        sources: zenmon_core::types::NodeSources::default(),
        admin_last_seen: None,
        scout_last_seen: None,
    });
    app.scout_nodes.push(zenmon_core::types::NodeInfo {
        zid: "z2".into(),
        kind: "peer".into(),
        locators: vec![],
        metadata: None,
        sources: zenmon_core::types::NodeSources::default(),
        admin_last_seen: None,
        scout_last_seen: None,
    });
    app.nodes = zenmon_core::merge::merge_nodes(&app.admin_nodes, &app.scout_nodes);
    app.node_selected = 1;
    app.node_detail_scroll = 2;

    app.clear_network_state();

    assert!(app.key_tree.is_empty());
    assert!(app.topic_latest.is_empty());
    assert!(app.topic_msg_counts.is_empty());
    assert!(app.topic_hz.is_empty());
    assert_eq!(app.total_msg_count, 0);
    assert_eq!(app.total_hz, 0.0);
    assert_eq!(app.tree_selected, 0);
    assert_eq!(app.topic_detail_scroll, 0);
    assert_eq!(app.list_scroll_offset, 0);

    assert!(app.sub_messages.is_empty());
    assert!(app.recent_messages.is_empty());
    assert_eq!(app.sub_selected, 0);

    assert!(app.admin_nodes.is_empty());
    assert!(app.scout_nodes.is_empty());
    assert!(app.nodes.is_empty());
    assert_eq!(app.node_selected, 0);
    assert_eq!(app.node_detail_scroll, 0);
}

#[test]
fn clear_network_state_preserves_query_history_and_filters() {
    let mut app = App::new("test".into());
    app.query_input = "demo/**".into();
    app.query_history.push("demo/**".into());
    app.query_results.push(ZenohMessage {
        key_expr: "demo/x".into(),
        payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(1)),
        encoding: String::new(),
        payload_bytes: 0,
        timestamp: None,
        kind: "get".into(),
        attachment: None,
        attachment_bytes: None,
    });
    app.topic_filter = "abc".into();
    app.stream_filter = "xyz".into();
    app.stream_follow = false;
    app.sub_paused = true;

    app.clear_network_state();

    assert_eq!(app.query_input, "demo/**");
    assert_eq!(app.query_history, vec!["demo/**".to_string()]);
    assert_eq!(app.query_results.len(), 1);
    assert_eq!(app.topic_filter, "abc");
    assert_eq!(app.stream_filter, "xyz");
    assert!(!app.stream_follow);
    assert!(app.sub_paused);
}

/// Seed the key tree and bring the row cache up to date, as a burst of
/// messages would. Single-segment keys give one leaf row each, so tests about
/// cursor movement keep indexing rows directly.
fn seed_topics(app: &mut App, keys: &[&str]) {
    for k in keys {
        app.key_tree.insert(k);
    }
    app.auto_expand();
    app.refresh_tree_rows();
}

#[test]
fn traffic_q_runs_query_on_selected_key() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["demo/a", "demo/b"]);
    // Rows are [demo/, demo/a, demo/b] — the shared branch takes row 0.
    app.tree_selected = 2; // demo/b
    app.handle_key(key(KeyCode::Char('Q')));
    assert_eq!(app.detail_mode, DetailMode::Query);
    assert_eq!(app.pending_query, Some("demo/b".to_string()));
    assert_eq!(app.query_history, vec!["demo/b".to_string()]);
}

#[test]
fn traffic_l_sets_live_mode() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    app.detail_mode = DetailMode::Query;
    app.handle_key(key(KeyCode::Char('L')));
    assert_eq!(app.detail_mode, DetailMode::Live);
}

#[test]
fn traffic_jk_move_and_clamp_selection() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["a", "b", "c"]);
    assert_eq!(app.tree_selected, 0);
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.tree_selected, 1);
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.tree_selected, 2);
    // Clamp at the bottom.
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.tree_selected, 2);
    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.tree_selected, 0);
    // Clamp at the top.
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.tree_selected, 0);
}

#[test]
fn traffic_move_resets_detail_scroll() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["a", "b"]);
    app.topic_detail_scroll = 9;
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.tree_selected, 1);
    assert_eq!(app.topic_detail_scroll, 0);
}

/// Seed enough keys that the automatic expansion does *not* open everything,
/// so expand/collapse can be exercised against a genuinely closed tree.
fn seed_fleet(app: &mut App, vehicles: usize) {
    for i in 0..vehicles {
        app.key_tree.insert(&format!("agv/f{i:03}/pose"));
    }
    app.auto_expand();
    app.refresh_tree_rows();
}

#[test]
fn l_opens_a_branch_and_h_closes_it() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_fleet(&mut app, 30);
    // `agv` is the only root child, so the automatic opening walked into it and
    // stopped where the vehicles fan out.
    assert_eq!(app.tree_rows()[0].path, "agv");

    app.handle_key(key(KeyCode::Char('h')));
    assert_eq!(app.tree_rows().len(), 1, "closing agv leaves only its row");

    app.handle_key(key(KeyCode::Char('l')));
    // 30 children is over the fold threshold, so opening shows a summary row.
    assert_eq!(app.tree_rows().len(), 2);
    assert!(matches!(
        app.tree_rows()[1].kind,
        RowKind::FoldSummary { hidden: 30 }
    ));
}

#[test]
fn l_on_a_fold_summary_lists_the_children() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_fleet(&mut app, 30);
    app.handle_key(key(KeyCode::Char('j'))); // onto the summary row
    assert!(matches!(
        app.selected_row().map(|r| r.kind),
        Some(RowKind::FoldSummary { .. })
    ));
    app.handle_key(key(KeyCode::Char('l')));
    assert_eq!(app.tree_rows().len(), 31, "agv plus its 30 vehicles");
}

#[test]
fn collapsing_a_branch_also_refolds_it() {
    // Otherwise reopening a 200-vehicle branch dumps every child back on screen
    // because it was unfolded once, minutes ago.
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_fleet(&mut app, 30);
    app.handle_key(key(KeyCode::Char('j')));
    app.handle_key(key(KeyCode::Char('l'))); // unfold
    assert_eq!(app.tree_rows().len(), 31);

    app.tree_selected = 0;
    app.handle_key(key(KeyCode::Char('h'))); // collapse agv
    app.handle_key(key(KeyCode::Char('l'))); // and reopen it
    assert_eq!(app.tree_rows().len(), 2, "back to the summary row");
}

#[test]
fn h_on_a_closed_branch_climbs_to_the_parent() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["agv/f1/pose", "agv/f2/pose"]);
    // Rows: agv/, agv/f1/, agv/f1/pose, agv/f2/, agv/f2/pose
    app.tree_selected = 2; // the pose key under f1
    app.handle_key(key(KeyCode::Char('h')));
    assert_eq!(app.selected_row().unwrap().path, "agv/f1");
    app.handle_key(key(KeyCode::Char('h'))); // f1 is open, so this closes it
    app.handle_key(key(KeyCode::Char('h'))); // now it climbs
    assert_eq!(app.selected_row().unwrap().path, "agv");
}

#[test]
fn expand_all_and_collapse_all() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_fleet(&mut app, 30);
    app.handle_key(key(KeyCode::Char('C')));
    assert_eq!(app.tree_rows().len(), 1);
    app.handle_key(key(KeyCode::Char('E')));
    // Every branch open: agv, 30 vehicles, and a pose key under each.
    assert_eq!(app.tree_rows().len(), 61);
}

#[test]
fn a_small_namespace_opens_completely() {
    // Under the fold threshold the tree should look like the flat list it
    // effectively is, not make the user expand to reach anything.
    let mut app = App::new("test".into());
    seed_topics(&mut app, &["a/b/c", "a/b/d"]);
    assert_eq!(
        app.tree_rows()
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "a/b", "a/b/c", "a/b/d"]
    );
}

#[test]
fn a_new_key_never_reopens_what_the_user_closed() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["agv/f1/pose"]);
    app.tree_selected = 0;
    app.handle_key(key(KeyCode::Char('C')));
    assert_eq!(app.tree_rows().len(), 1);

    // A message on a brand-new key arrives while the tree is closed.
    app.handle_zenoh_message(ZenohMessage {
        key_expr: "agv/f2/pose".into(),
        payload: MessagePayload::from_json(&serde_json::json!(null)),
        encoding: String::new(),
        payload_bytes: 0,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    });
    app.refresh_tree_rows();
    assert_eq!(app.tree_rows().len(), 1, "the tree stayed closed");
}

#[test]
fn a_branch_row_is_not_a_queryable_key() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["agv/f1/pose"]);
    app.tree_selected = 0; // the `agv/` branch
    assert_eq!(app.selected_topic_key(), None);
    app.handle_key(key(KeyCode::Char('Q')));
    assert_eq!(app.pending_query, None, "a prefix is not a key to query");
}

#[test]
fn filtering_reaches_inside_a_folded_group() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_fleet(&mut app, 30);
    app.topic_filter = "f017".into();
    app.refresh_tree_rows();
    assert_eq!(
        app.tree_rows()
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>(),
        vec!["agv", "agv/f017", "agv/f017/pose"]
    );
}

/// Record `key` as last seen `secs` ago. `Instant` cannot be constructed, so
/// this walks back from now — saturating, since a machine booted seconds ago
/// has no instant that far in the past.
fn seen_ago(app: &mut App, key: &str, secs: u64) {
    let at = Instant::now()
        .checked_sub(Duration::from_secs(secs))
        .unwrap_or_else(Instant::now);
    app.key_tree.insert(key);
    app.topic_latest.insert(
        key.into(),
        (
            ZenohMessage {
                key_expr: key.into(),
                payload: MessagePayload::from_json(&serde_json::json!(null)),
                encoding: String::new(),
                payload_bytes: 0,
                timestamp: None,
                kind: "put".into(),
                attachment: None,
                attachment_bytes: None,
            },
            at,
        ),
    );
}

#[test]
fn freshness_turns_over_at_the_dim_threshold() {
    let mut app = App::new("test".into());
    seen_ago(&mut app, "a/fresh", 0);
    seen_ago(&mut app, "a/stale", IDLE_DIM_SECS + 1);
    assert_eq!(app.key_freshness("a/fresh"), KeyFreshness::Live);
    assert_eq!(app.key_freshness("a/stale"), KeyFreshness::Idle);
    // In the tree but never carrying a payload.
    app.key_tree.insert("a/unseen");
    assert_eq!(app.key_freshness("a/unseen"), KeyFreshness::NoData);
}

#[test]
fn format_idle_is_coarse_by_design() {
    assert_eq!(format_idle(0), "0m");
    assert_eq!(format_idle(59), "0m");
    assert_eq!(format_idle(305), "5m");
    assert_eq!(format_idle(3600), "1h0m");
    assert_eq!(format_idle(7_845), "2h10m");
}

#[test]
fn keys_under_the_cap_are_never_evicted() {
    // Going quiet is a finding, not a reason to be forgotten.
    let mut app = App::new("test".into());
    seen_ago(&mut app, "a/ancient", 86_400);
    app.evict_excess_keys();
    assert_eq!(app.key_tree.len(), 1);
    assert_eq!(app.keys_aged_out, 0);
}

#[test]
fn exceeding_the_cap_evicts_the_longest_idle_first() {
    let mut app = App::new("test".into());
    // One key per second of age, oldest first, just past the cap.
    for i in 0..=MAX_KEYS {
        seen_ago(&mut app, &format!("k/{i:05}"), (MAX_KEYS - i) as u64);
    }
    assert_eq!(app.key_tree.len(), MAX_KEYS + 1);

    app.evict_excess_keys();

    let target = (MAX_KEYS as f64 * EVICT_TO_FRACTION) as usize;
    assert_eq!(app.key_tree.len(), target, "evicts past the cap, not to it");
    assert_eq!(app.keys_aged_out, MAX_KEYS + 1 - target);
    // The oldest went; the newest stayed.
    assert!(!app.key_tree.contains("k/00000"));
    assert!(app.key_tree.contains(&format!("k/{MAX_KEYS:05}")));
    // And every map is dropped together, so nothing is left keyed by a key
    // the tree no longer knows.
    assert!(!app.topic_latest.contains_key("k/00000"));
    assert_eq!(app.topic_latest.len(), target);
}

#[test]
fn eviction_prunes_expansion_state_for_vanished_branches() {
    // Otherwise the expansion sets grow without bound alongside the keys they
    // outlive — the same leak, one level up.
    let mut app = App::new("test".into());
    for i in 0..=MAX_KEYS {
        seen_ago(&mut app, &format!("k{i:05}/leaf"), (MAX_KEYS - i) as u64);
    }
    app.tree_expanded.insert("k00000".into());
    app.tree_unfolded.insert("k00000".into());

    app.evict_excess_keys();

    assert!(!app.key_tree.contains("k00000/leaf"));
    assert!(!app.tree_expanded.contains("k00000"));
    assert!(!app.tree_unfolded.contains("k00000"));
}

#[test]
fn render_traffic_with_data_wide_and_narrow_shows_key() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    for (w, h) in [(120u16, 30u16), (60u16, 20u16)] {
        let mut app = App::new("tcp/127.0.0.1:7447".into());
        app.space = Space::Traffic;
        app.connection_state = ConnectionState::Connected("zid".into());
        app.key_tree.insert("demo/robot/pose");
        app.auto_expand();
        let msg = ZenohMessage {
            key_expr: "demo/robot/pose".into(),
            payload: MessagePayload::from_json(&serde_json::json!({"x": 1})),
            encoding: "application/json".into(),
            payload_bytes: 8,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        };
        app.topic_latest
            .insert("demo/robot/pose".into(), (msg.clone(), Instant::now()));
        app.sub_messages.push_front(msg);
        app.topic_hz.insert("demo/robot/pose".into(), 12.0);

        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        // Wide → master; narrow default focus is Master → master shows the tree.
        terminal.draw(|f| app.render(f)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        // The master draws one segment per row, not the whole key expression —
        // the hierarchy is what carries the context now.
        for segment in ["demo/", "robot/", "pose"] {
            assert!(
                text.contains(segment),
                "segment {} missing at {}x{}: {}",
                segment,
                w,
                h,
                text
            );
        }
        // A single key is under the fold threshold, so it opens all the way
        // down rather than showing one collapsed row.
        assert!(text.contains("▾"), "expected an open branch at {}x{}", w, h);
    }
}

#[test]
fn render_traffic_narrow_detail_pane_does_not_panic() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = App::new("tcp/127.0.0.1:7447".into());
    app.space = Space::Traffic;
    app.pane_focus = PaneFocus::Detail;
    app.connection_state = ConnectionState::Connected("zid".into());
    app.key_tree.insert("demo/robot/pose");
    app.auto_expand();
    let msg = ZenohMessage {
        key_expr: "demo/robot/pose".into(),
        payload: MessagePayload::from_json(&serde_json::json!({"x": 1})),
        encoding: "application/json".into(),
        payload_bytes: 8,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    };
    app.topic_latest
        .insert("demo/robot/pose".into(), (msg.clone(), Instant::now()));
    app.sub_messages.push_front(msg);

    // Put the cursor on the key itself; rows 0 and 1 are the `demo/` and
    // `robot/` branches, which get a subtree summary rather than a payload.
    app.refresh_tree_rows();
    app.tree_selected = 2;

    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("demo/robot/pose"), "detail key missing");
}

#[test]
fn a_branch_selection_shows_a_subtree_summary_not_a_payload() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = App::new("tcp/127.0.0.1:7447".into());
    app.space = Space::Traffic;
    app.pane_focus = PaneFocus::Detail;
    app.connection_state = ConnectionState::Connected("zid".into());
    app.key_tree.insert("agv/f1/pose");
    app.key_tree.insert("agv/f2/pose");
    app.auto_expand();
    app.topic_hz.insert("agv/f1/pose".into(), 10.0);
    app.topic_hz.insert("agv/f2/pose".into(), 5.0);

    app.refresh_tree_rows();
    app.tree_selected = 0; // the `agv/` branch

    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    // The aggregate is the thing a flat list could never answer.
    assert!(text.contains("15.0 Hz"), "subtree rate missing: {}", text);
    assert!(text.contains("busiest"), "busiest list missing: {}", text);
}

/// Headless render smoke test: drive `render()` through ratatui's in-memory
/// TestBackend at wide and narrow widths (either side of the two-pane
/// breakpoint) to prove the layout builds without panicking and the header
/// tab / space body actually paint. Substitutes for interactive TTY checks.
fn render_at(width: u16, height: u16, space: Space) -> ratatui::buffer::Buffer {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut app = App::new("tcp/127.0.0.1:7447".into());
    app.space = space;
    terminal.draw(|f| app.render(f)).unwrap();
    terminal.backend().buffer().clone()
}

fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    buf.content().iter().map(|c| c.symbol()).collect()
}

/// Render `buf` back into a row-by-row text grid (with borders visible), for
/// eyeballing the real layout in a terminal.
fn buffer_grid(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area();
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            out.push_str(buf[(area.x + x, area.y + y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// Foreground colour of the first non-blank cell on the row containing
/// `needle`. The frame dumps are text-only, so colour — which is the entire
/// diff signal — needs asserting from the buffer itself.
fn row_fg(buf: &ratatui::buffer::Buffer, needle: &str) -> ratatui::style::Color {
    let area = buf.area();
    for y in 0..area.height {
        let row: String = (0..area.width)
            .map(|x| buf[(area.x + x, area.y + y)].symbol())
            .collect();
        if let Some(col) = row.find(needle) {
            return buf[(area.x + col as u16, area.y + y)].fg;
        }
    }
    panic!("no row containing {needle:?}");
}

/// A pair of messages on one key differing in `mode`, `speed` and `error`.
fn seed_state_change(app: &mut App) {
    let state = |mode: &str, speed: f64, err: serde_json::Value| ZenohMessage {
        key_expr: "agv/f001/state".into(),
        payload: MessagePayload::from_json(&serde_json::json!({
            "mode": mode, "speed": speed, "load": 0, "battery": 87, "error": err,
        })),
        encoding: "application/json".into(),
        payload_bytes: 96,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    };
    app.handle_zenoh_message(state("moving", 1.2, serde_json::json!(null)));
    app.handle_zenoh_message(state("stalled", 0.0, serde_json::json!("obstacle")));
    app.auto_expand();
    app.refresh_tree_rows();
    app.tree_selected = app.tree_rows().len() - 1;
}

fn draw(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn the_detail_pane_highlights_only_the_fields_that_moved() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    app.pane_focus = PaneFocus::Detail;
    app.connection_state = ConnectionState::Connected("zid".into());
    seed_state_change(&mut app);

    let buf = draw(&mut app, 88, 22);
    // Three fields moved; two did not. The unchanged ones must recede.
    assert_eq!(row_fg(&buf, "mode:"), ratatui::style::Color::Yellow);
    assert_eq!(row_fg(&buf, "speed:"), ratatui::style::Color::Yellow);
    assert_eq!(row_fg(&buf, "error:"), ratatui::style::Color::Yellow);
    assert_eq!(row_fg(&buf, "battery:"), ratatui::style::Color::Gray);
    assert_eq!(row_fg(&buf, "load:"), ratatui::style::Color::Gray);
}

#[test]
fn d_turns_the_diff_off_and_everything_goes_quiet_again() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    app.pane_focus = PaneFocus::Detail;
    app.connection_state = ConnectionState::Connected("zid".into());
    seed_state_change(&mut app);

    app.handle_key(key(KeyCode::Char('D')));
    assert!(!app.diff_enabled);
    let buf = draw(&mut app, 88, 22);
    assert_eq!(row_fg(&buf, "mode:"), ratatui::style::Color::Gray);
    assert!(
        buffer_text(&buf).contains("D to diff"),
        "the way back is shown"
    );
}

#[test]
fn a_key_seen_once_says_there_is_no_baseline_rather_than_marking_everything() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    app.pane_focus = PaneFocus::Detail;
    app.connection_state = ConnectionState::Connected("zid".into());
    app.handle_zenoh_message(ZenohMessage {
        key_expr: "agv/f001/state".into(),
        payload: MessagePayload::from_json(&serde_json::json!({"mode": "idle"})),
        encoding: "application/json".into(),
        payload_bytes: 16,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    });
    app.auto_expand();
    app.refresh_tree_rows();
    app.tree_selected = app.tree_rows().len() - 1;

    let buf = draw(&mut app, 88, 22);
    assert!(buffer_text(&buf).contains("no previous message yet"));
    assert_eq!(row_fg(&buf, "mode:"), ratatui::style::Color::Gray);
}

#[test]
fn a_quiet_keys_history_survives_a_noisy_neighbour() {
    // The end-to-end version of what `history` exists for: under the old
    // global 500-entry ring this key's past was gone within seconds.
    let mut app = App::new("test".into());
    let msg = |k: &str, n: i64| ZenohMessage {
        key_expr: k.into(),
        payload: MessagePayload::from_json(&serde_json::json!({ "n": n })),
        encoding: "application/json".into(),
        payload_bytes: 12,
        timestamp: None,
        kind: "put".into(),
        attachment: None,
        attachment_bytes: None,
    };
    app.handle_zenoh_message(msg("agv/quiet/pose", 1));
    app.handle_zenoh_message(msg("agv/quiet/pose", 2));
    for i in 0..5_000 {
        app.handle_zenoh_message(msg("agv/loud/pose", i));
    }
    assert_eq!(app.history.get("agv/quiet/pose").unwrap().len(), 2);
    assert!(app
        .history
        .get("agv/quiet/pose")
        .unwrap()
        .previous()
        .is_some());
}

/// A dev harness (not run by default): seed a realistic network and dump the
/// actual rendered frames to stdout for manual inspection.
/// Run with: `cargo test -p zenmon-tui dump_frames -- --ignored --nocapture`
#[test]
#[ignore]
fn dump_frames() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let seed = |space: Space| {
        let mut app = App::new("tcp/127.0.0.1:7447".into());
        app.space = space;
        app.connection_state = ConnectionState::Connected("a3f19c0011223344".into());
        app.self_zid = Some("a3f19c0011223344".into());
        // Topics + rates.
        for (k, hz) in [
            ("demo/robot/pose", 12.0),
            ("demo/robot/battery", 1.0),
            ("demo/sensor/lidar", 30.0),
            ("sys/health", 0.0),
        ] {
            app.key_tree.insert(k);
            app.topic_hz.insert(k.into(), hz);
        }
        let msg = ZenohMessage {
            key_expr: "demo/robot/pose".into(),
            payload: MessagePayload::from_json(
                &serde_json::json!({"x": 1.21, "y": 3.40, "theta": 0.11}),
            ),
            encoding: "application/json".into(),
            payload_bytes: 40,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        };
        app.topic_latest
            .insert("demo/robot/pose".into(), (msg.clone(), Instant::now()));
        app.sub_messages.push_front(msg);
        app.total_hz = 43.0;
        // Nodes + services.
        app.nodes.push(make_node("a3f19c0011223344", "router"));
        app.nodes.push(make_node("b2c4ffee00990011", "peer"));
        app.liveliness_tokens
            .push(make_token("demo/robot/node/action_executor", true));
        app.liveliness_tokens
            .push(make_token("demo/robot/node/topic_recorder", false));
        app.liveliness_tokens
            .push(make_token("sys/node/system_monitor", true));
        app
    };

    for (label, space, w, h) in [
        ("TRAFFIC (wide 110x24)", Space::Traffic, 110u16, 24u16),
        ("NETWORK (wide 110x24)", Space::Network, 110u16, 24u16),
        ("NETWORK (narrow 64x20)", Space::Network, 64u16, 20u16),
    ] {
        let mut app = seed(space);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        println!("\n===== {label} =====");
        print!("{}", buffer_grid(terminal.backend().buffer()));
    }

    // A fleet-sized namespace: the case the tree and the fold exist for, and
    // the one no synthetic four-key seed shows anything about.
    {
        let mut app = App::new("tcp/127.0.0.1:7447".into());
        app.space = Space::Traffic;
        app.connection_state = ConnectionState::Connected("a3f19c0011223344".into());
        for i in 0..200 {
            for (topic, hz) in [("pose", 10.0), ("battery", 1.0), ("state", 1.0)] {
                let key = format!("agv/f{i:03}/{topic}");
                app.key_tree.insert(&key);
                app.topic_hz.insert(key, hz);
            }
        }
        app.key_tree.insert("srv/fleet/health");
        app.auto_expand();
        app.total_hz = 2400.0;

        let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        println!("\n===== TRAFFIC 200 vehicles / 601 keys, collapsed =====");
        print!("{}", buffer_grid(terminal.backend().buffer()));

        // And the same tree with a filter, which is the documented way into a
        // folded group.
        app.topic_filter = "f017".into();
        app.refresh_tree_rows();
        terminal.draw(|f| app.render(f)).unwrap();
        println!("\n===== TRAFFIC 601 keys, filtered to /f017 =====");
        print!("{}", buffer_grid(terminal.backend().buffer()));
    }

    // A repeating status blob where one field moved: the case the diff exists
    // for, and the one a wall of JSON cannot show.
    {
        let mut app = App::new("tcp/127.0.0.1:7447".into());
        app.space = Space::Traffic;
        app.pane_focus = PaneFocus::Detail;
        app.connection_state = ConnectionState::Connected("a3f19c0011223344".into());
        let state = |mode: &str, speed: f64, err: serde_json::Value| ZenohMessage {
            key_expr: "agv/f001/state".into(),
            payload: MessagePayload::from_json(&serde_json::json!({
                "mode": mode,
                "speed": speed,
                "load": 0,
                "battery": 87,
                "error": err,
                "pose": {"x": 12.5, "y": 3.0},
            })),
            encoding: "application/json".into(),
            payload_bytes: 96,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        };
        app.handle_zenoh_message(state("moving", 1.2, serde_json::json!(null)));
        app.handle_zenoh_message(state("stalled", 0.0, serde_json::json!("obstacle")));
        app.auto_expand();
        app.refresh_tree_rows();
        app.tree_selected = app.tree_rows().len() - 1;

        let mut terminal = Terminal::new(TestBackend::new(88, 22)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        println!("\n===== TRAFFIC detail, diff against the previous message =====");
        print!("{}", buffer_grid(terminal.backend().buffer()));
    }

    // A mix of live and long-quiet keys, which is what a stalled publisher
    // actually looks like in this pane.
    {
        let mut app = App::new("tcp/127.0.0.1:7447".into());
        app.space = Space::Traffic;
        app.connection_state = ConnectionState::Connected("a3f19c0011223344".into());
        seen_ago(&mut app, "agv/f001/pose", 0);
        seen_ago(&mut app, "agv/f001/battery", 0);
        seen_ago(&mut app, "agv/f002/pose", 900);
        seen_ago(&mut app, "agv/f002/battery", 7_845);
        app.topic_hz.insert("agv/f001/pose".into(), 10.0);
        app.topic_hz.insert("agv/f001/battery".into(), 1.0);
        app.auto_expand();
        app.keys_aged_out = 137;

        let mut terminal = Terminal::new(TestBackend::new(110, 16)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        println!("\n===== TRAFFIC live vs long-idle keys =====");
        print!("{}", buffer_grid(terminal.backend().buffer()));
    }

    // Doctor overlay over the Network space, with a mixed report.
    {
        use std::time::Duration;
        use zenmon_core::doctor::{Check, DoctorReport};
        let mut app = seed(Space::Network);
        app.doctor_report = Some(DoctorReport::new(vec![
            Check::pass("config", Duration::from_millis(1), None),
            Check::pass("session", Duration::from_millis(8), None),
            Check::pass(
                "connection",
                Duration::from_millis(12),
                Some("1 router(s), 1 peer(s)".into()),
            ),
            Check::warn(
                "liveliness",
                Duration::from_millis(0),
                "no_tokens",
                "no liveliness tokens on 'sys/**'",
                "is the monitored app declaring liveliness?",
            ),
        ]));
        app.overlay = Overlay::Doctor;
        let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        println!("\n===== DOCTOR overlay (110x24) =====");
        print!("{}", buffer_grid(terminal.backend().buffer()));
    }

    // Command palette over Traffic.
    {
        let mut app = seed(Space::Traffic);
        app.overlay = Overlay::Palette;
        let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        println!("\n===== PALETTE overlay (110x24) =====");
        print!("{}", buffer_grid(terminal.backend().buffer()));
    }
}

#[test]
fn render_wide_paints_two_pane_traffic() {
    let buf = render_at(120, 30, Space::Traffic);
    let text = buffer_text(&buf);
    assert!(text.contains("Traffic"), "space tab / master title missing");
    assert!(
        text.contains("Detail"),
        "detail pane missing in wide layout"
    );
}

#[test]
fn render_narrow_single_pane_does_not_panic() {
    // Below TWO_PANE_MIN_WIDTH: single pane, compact 1-line header.
    let buf = render_at(60, 20, Space::Network);
    let text = buffer_text(&buf);
    assert!(text.contains("Network"), "network master title missing");
}

fn make_node(zid: &str, kind: &str) -> NodeInfo {
    NodeInfo {
        zid: zid.into(),
        kind: kind.into(),
        locators: vec!["tcp/127.0.0.1:7447".into()],
        metadata: None,
        sources: zenmon_core::types::NodeSources::ADMIN,
        admin_last_seen: Some(SystemTime::now()),
        scout_last_seen: None,
    }
}

fn make_token(key_expr: &str, alive: bool) -> LivelinessToken {
    LivelinessToken {
        key_expr: key_expr.into(),
        source_zid: None,
        alive,
    }
}

#[test]
fn network_jk_move_and_clamp_selection() {
    let mut app = App::new("test".into());
    app.space = Space::Network;
    app.nodes.push(make_node("z1", "router"));
    app.nodes.push(make_node("z2", "peer"));
    app.liveliness_tokens
        .push(make_token("fleet/r1/node/a", true));
    // 2 sessions + 1 service = 3 selectable rows.
    assert_eq!(app.network_rows().len(), 3);
    assert_eq!(app.network_selected, 0);
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.network_selected, 1);
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.network_selected, 2);
    // Clamp at the bottom.
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.network_selected, 2);
    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.network_selected, 0);
    // Clamp at the top.
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.network_selected, 0);
}

#[test]
fn network_move_resets_detail_scroll() {
    let mut app = App::new("test".into());
    app.space = Space::Network;
    app.nodes.push(make_node("z1", "router"));
    app.nodes.push(make_node("z2", "peer"));
    app.node_detail_scroll = 9;
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.network_selected, 1);
    assert_eq!(app.node_detail_scroll, 0);
}

#[test]
fn network_selection_resolves_session_then_service() {
    let mut app = App::new("test".into());
    app.nodes.push(make_node("z1", "router"));
    app.nodes.push(make_node("z2", "peer"));
    app.liveliness_tokens
        .push(make_token("fleet/r1/node/a", true));
    assert_eq!(app.selected_network_row(), Some(NetworkRow::Session(0)));
    app.network_selected = 1;
    assert_eq!(app.selected_network_row(), Some(NetworkRow::Session(1)));
    app.network_selected = 2;
    assert_eq!(app.selected_network_row(), Some(NetworkRow::Service(0)));
}

#[test]
fn network_s_requests_scout_refresh() {
    let mut app = App::new("test".into());
    app.space = Space::Network;
    assert!(!app.pending_scout_request);
    app.handle_key(key(KeyCode::Char('s')));
    assert!(app.pending_scout_request);
}

#[test]
fn network_s_does_not_request_while_scout_in_progress() {
    let mut app = App::new("test".into());
    app.space = Space::Network;
    app.scout_in_progress = true;
    app.handle_key(key(KeyCode::Char('s')));
    assert!(!app.pending_scout_request);
}

#[test]
fn render_network_with_data_wide_and_narrow_shows_participant() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    for (w, h) in [(120u16, 30u16), (60u16, 20u16)] {
        let mut app = App::new("tcp/127.0.0.1:7447".into());
        app.space = Space::Network;
        app.connection_state = ConnectionState::Connected("zidrouter0000000".into());
        app.nodes.push(make_node("zidrouter0000000", "router"));
        app.liveliness_tokens
            .push(make_token("fleet/r1/node/action_executor_ec98a701", true));

        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        // Master (wide → both panes; narrow default focus is Master) shows the
        // session zid and the service name.
        assert!(
            text.contains("zidrouter"),
            "session zid missing at {}x{}: {}",
            w,
            h,
            text
        );
        assert!(
            text.contains("action_executor"),
            "service name missing at {}x{}: {}",
            w,
            h,
            text
        );
    }
}

#[test]
fn render_help_overlay_paints() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let mut app = App::new("tcp/127.0.0.1:7447".into());
    app.overlay = Overlay::Help;
    terminal.draw(|f| app.render(f)).unwrap();
    // Overlay drew on top without panicking; buffer is non-empty.
    assert!(!buffer_text(terminal.backend().buffer()).trim().is_empty());
}

#[test]
fn d_opens_doctor_overlay_and_requests_run() {
    let mut app = App::new("test".into());
    assert_eq!(app.overlay, Overlay::None);
    assert!(!app.pending_doctor_request);
    app.handle_key(key(KeyCode::Char('d')));
    assert_eq!(app.overlay, Overlay::Doctor);
    assert!(app.pending_doctor_request);
    assert_eq!(app.doctor_scroll, 0);
}

#[test]
fn doctor_overlay_r_reruns_and_esc_closes() {
    let mut app = App::new("test".into());
    app.handle_key(key(KeyCode::Char('d')));
    app.pending_doctor_request = false;
    // r re-runs
    app.handle_key(key(KeyCode::Char('r')));
    assert!(app.pending_doctor_request);
    // j/k scroll
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.doctor_scroll, 1);
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.doctor_scroll, 0);
    // Esc closes
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn doctor_events_toggle_running_and_store_report() {
    use zenmon_core::doctor::Check;
    let mut app = App::new("test".into());
    app.handle_event(AppEvent::DoctorStarted);
    assert!(app.doctor_running);
    assert!(app.doctor_report.is_none());

    let report = DoctorReport::new(vec![Check::pass("config", Duration::from_millis(1), None)]);
    app.handle_event(AppEvent::DoctorReport(report));
    assert!(!app.doctor_running);
    assert!(app.doctor_report.is_some());
}

#[test]
fn render_doctor_overlay_shows_checks_and_hints() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use zenmon_core::doctor::Check;
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let mut app = App::new("tcp/127.0.0.1:7447".into());
    app.doctor_report = Some(DoctorReport::new(vec![
        Check::pass("config", Duration::from_millis(2), None),
        Check::fail(
            "connection",
            Duration::from_millis(5),
            "router_unreachable",
            "no router connected in client mode",
            "start a router (zenohd) and check --endpoint",
        ),
    ]));
    app.overlay = Overlay::Doctor;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("✖"), "fail glyph missing: {}", text);
    assert!(text.contains("connection"), "check name missing: {}", text);
    assert!(
        text.contains("start a router"),
        "hint text missing: {}",
        text
    );
}

#[test]
fn header_dot_reflects_failing_doctor_report() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use zenmon_core::doctor::Check;
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let mut app = App::new("tcp/127.0.0.1:7447".into());
    app.connection_state = ConnectionState::Connected("zid".into());
    app.doctor_report = Some(DoctorReport::new(vec![Check::fail(
        "connection",
        Duration::from_millis(5),
        "router_unreachable",
        "no router connected in client mode",
        "start a router",
    )]));
    // Renders without panic and paints the fail glyph in the header.
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("✖"), "header fail glyph missing: {}", text);
}

#[test]
fn colon_opens_command_palette() {
    let mut app = App::new("test".into());
    assert_eq!(app.overlay, Overlay::None);
    app.handle_key(key(KeyCode::Char(':')));
    assert_eq!(app.overlay, Overlay::Palette);
    assert!(app.palette_input.is_empty());
    assert_eq!(app.palette_selected, 0);
}

#[test]
fn palette_filtering_shrinks_and_matches() {
    let mut app = App::new("test".into());
    app.handle_key(key(KeyCode::Char(':')));
    let all = app.filtered_palette_commands().len();
    assert_eq!(all, palette_commands().len());

    // Type "peer" (case-insensitive) → only the peer-mode command matches.
    for c in "peer".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    let filtered = app.filtered_palette_commands();
    assert!(filtered.len() < all);
    assert_eq!(filtered.len(), 1);
    assert!(matches!(
        palette_commands()[filtered[0]].action,
        PaletteAction::SetMode(ConnectMode::Peer)
    ));
}

#[test]
fn palette_enter_runs_peer_mode_command() {
    let mut app = App::new("test".into());
    app.current_mode = ConnectMode::Client;
    app.handle_key(key(KeyCode::Char(':')));
    for c in "peer".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pending_reconnect_mode, Some(ConnectMode::Peer));
    // Palette closed after running.
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn palette_set_mode_same_mode_does_not_reconnect() {
    let mut app = App::new("test".into());
    app.current_mode = ConnectMode::Client;
    app.run_palette_action(PaletteAction::SetMode(ConnectMode::Client));
    assert_eq!(app.pending_reconnect_mode, None);
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn palette_esc_closes_without_action() {
    let mut app = App::new("test".into());
    app.handle_key(key(KeyCode::Char(':')));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.pending_reconnect_mode, None);
}

#[test]
fn palette_arrow_navigation_clamps() {
    let mut app = App::new("test".into());
    app.handle_key(key(KeyCode::Char(':')));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.palette_selected, 0);
    let count = app.filtered_palette_commands().len();
    for _ in 0..(count + 3) {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(app.palette_selected, count - 1);
}

#[test]
fn scout_port_action_opens_modal_and_reconnects() {
    let mut app = App::new("test".into());
    app.run_palette_action(PaletteAction::OpenScoutPort);
    assert_eq!(app.overlay, Overlay::ScoutPort);
    // Type a port and confirm.
    app.handle_key(key(KeyCode::Char('7')));
    app.handle_key(key(KeyCode::Char('4')));
    app.handle_key(key(KeyCode::Char('5')));
    app.handle_key(key(KeyCode::Char('0')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pending_reconnect_port, Some(7450));
    assert_eq!(app.scout_port_current, Some(7450));
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn scout_port_modal_esc_closes() {
    let mut app = App::new("test".into());
    app.run_palette_action(PaletteAction::OpenScoutPort);
    app.handle_key(key(KeyCode::Char('7')));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.scout_port_input.is_empty());
    assert_eq!(app.pending_reconnect_port, None);
}

#[test]
fn scout_port_modal_s_requests_scan() {
    let mut app = App::new("test".into());
    app.run_palette_action(PaletteAction::OpenScoutPort);
    app.handle_key(key(KeyCode::Char('s')));
    assert!(app.pending_port_scan_request);
}

#[test]
fn scout_port_enter_uses_selected_scan_result() {
    let mut app = App::new("test".into());
    app.run_palette_action(PaletteAction::OpenScoutPort);
    app.port_scan_results = vec![
        PortScoutResult {
            port: 7446,
            nodes: vec![],
        },
        PortScoutResult {
            port: 7448,
            nodes: vec![zenmon_core::types::ScoutInfo {
                zid: "z1".into(),
                whatami: "peer".into(),
                locators: vec![],
            }],
        },
    ];
    app.port_scan_selected = 0; // first non-empty result → port 7448
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pending_reconnect_port, Some(7448));
}

#[test]
fn render_palette_overlay_paints_labels() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let mut app = App::new("tcp/127.0.0.1:7447".into());
    app.overlay = Overlay::Palette;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("Commands"), "palette title missing: {}", text);
    assert!(text.contains("doctor"), "doctor command missing: {}", text);
    assert!(
        text.contains("Network"),
        "network command missing: {}",
        text
    );
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn traffic_click_selects_row_and_drills() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["a", "b", "c"]);
    app.list_rect = Some(Rect::new(0, 2, 40, 10));
    app.list_first_item_row = 3;
    app.list_scroll_offset = 0;
    // Row 5 == first_item_row(3) + 2 → index 2.
    app.handle_click(5, 5);
    assert_eq!(app.tree_selected, 2);
    assert_eq!(app.topic_detail_scroll, 0);
    assert_eq!(app.pane_focus, PaneFocus::Detail);
}

#[test]
fn traffic_click_outside_list_is_ignored() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["a", "b", "c"]);
    app.tree_selected = 1;
    app.list_rect = Some(Rect::new(0, 2, 40, 10));
    app.list_first_item_row = 3;
    // Row 1 is above the list rect (y == 2) → no change.
    app.handle_click(5, 1);
    assert_eq!(app.tree_selected, 1);
}

#[test]
fn network_click_map_marks_headers_and_rows() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = App::new("test".into());
    app.space = Space::Network;
    app.connection_state = ConnectionState::Connected("z1".into());
    app.nodes.push(make_node("z1", "router"));
    app.nodes.push(make_node("z2", "peer"));
    app.liveliness_tokens
        .push(make_token("fleet/r1/node/a", true));

    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    // Display rows: Sessions header, z1, z2, Services header, group header,
    // token. Headers → None; participants → their `network_rows()` index.
    assert_eq!(
        app.network_click_map,
        vec![None, Some(0), Some(1), None, None, Some(2)]
    );

    // Clicking the z2 display row (index 2) selects network_rows() index 1.
    let rect = app.list_rect.unwrap();
    let row = app.list_first_item_row + 2 - app.list_scroll_offset as u16;
    app.handle_click(rect.x + 1, row);
    assert_eq!(app.network_selected, 1);
    assert_eq!(app.pane_focus, PaneFocus::Detail);

    // Clicking a header row (display index 0) leaves the selection alone.
    let hdr_row = app.list_first_item_row - app.list_scroll_offset as u16;
    app.handle_click(rect.x + 1, hdr_row);
    assert_eq!(app.network_selected, 1);
}

#[test]
fn wheel_moves_traffic_selection_and_clamps() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["a", "b", "c"]);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 0, 0));
    assert_eq!(app.tree_selected, 1);
    // Clamp at the bottom.
    app.wheel(1);
    app.wheel(1);
    assert_eq!(app.tree_selected, 2);
    // Scroll back up and clamp at the top.
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 0, 0));
    app.wheel(-1);
    app.wheel(-1);
    assert_eq!(app.tree_selected, 0);
}

#[test]
fn wheel_moves_network_selection_and_clamps() {
    let mut app = App::new("test".into());
    app.space = Space::Network;
    app.nodes.push(make_node("z1", "router"));
    app.nodes.push(make_node("z2", "peer"));
    // 2 selectable rows.
    app.wheel(1);
    assert_eq!(app.network_selected, 1);
    app.wheel(1); // clamp
    assert_eq!(app.network_selected, 1);
    app.wheel(-1);
    assert_eq!(app.network_selected, 0);
}

#[test]
fn wheel_ignored_while_overlay_open() {
    let mut app = App::new("test".into());
    app.space = Space::Traffic;
    seed_topics(&mut app, &["a", "b", "c"]);
    app.overlay = Overlay::Help;
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 0, 0));
    assert_eq!(app.tree_selected, 0);
}
