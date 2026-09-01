//! Inbound data: app events, zenoh messages, liveliness, node lists, and the
//! per-second rate accounting the sparklines read.

use super::*;
use crate::event::AppEvent;
use std::time::Instant;
use zenmon_core::merge::merge_nodes;
use zenmon_core::types::{NodeInfo, ZenohMessage};

/// How many 1-second buckets of rate history to keep for sparklines.
pub(crate) const RATE_WINDOW_SECS: usize = 30;

/// A bounded ring buffer of per-second byte counts, used for bandwidth
/// sparklines. Idle seconds are recorded as `0` buckets so a topic that goes
/// quiet shows a real dip rather than a stale value.
#[derive(Debug, Clone)]
pub(crate) struct RateWindow {
    samples: std::collections::VecDeque<u64>,
    cap: usize,
}

impl RateWindow {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            samples: std::collections::VecDeque::new(),
            cap,
        }
    }

    /// Record one completed 1-second bucket, evicting the oldest beyond `cap`.
    pub(crate) fn push(&mut self, bytes: u64) {
        self.samples.push_back(bytes);
        while self.samples.len() > self.cap {
            self.samples.pop_front();
        }
    }

    pub(crate) fn latest(&self) -> u64 {
        self.samples.back().copied().unwrap_or(0)
    }

    pub(crate) fn is_all_zero(&self) -> bool {
        self.samples.iter().all(|&b| b == 0)
    }

    pub(crate) fn series(&self) -> Vec<u64> {
        self.samples.iter().copied().collect()
    }
}

impl App {
    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Mouse(m) => self.handle_mouse(m),
            AppEvent::Zenoh(msg) => self.handle_zenoh_message(msg),
            AppEvent::Tick => self.update_hz(),
            AppEvent::AdminNodes(nodes) => self.handle_admin_nodes(nodes),
            AppEvent::ScoutStarted => {
                self.scout_in_progress = true;
            }
            AppEvent::ScoutNodes(nodes) => self.handle_scout_nodes(nodes),
            AppEvent::PortScanStarted => {
                self.port_scan_in_progress = true;
            }
            AppEvent::PortScanResults(results) => {
                self.port_scan_results = results;
                self.port_scan_selected = 0;
                self.port_scan_in_progress = false;
            }
            AppEvent::Liveliness(event) => self.handle_liveliness(event),
            AppEvent::DoctorStarted => {
                self.doctor_running = true;
            }
            AppEvent::DoctorReport(report) => {
                self.doctor_report = Some(report);
                self.doctor_running = false;
            }
        }
    }

    fn handle_liveliness(&mut self, event: zenmon_core::types::LivelinessEvent) {
        use zenmon_core::types::LivelinessEvent;
        let (token, is_join) = match event {
            LivelinessEvent::Join(t) => (t, true),
            LivelinessEvent::Leave(t) => (t, false),
        };

        // Record event
        let record = LivelinessEventRecord {
            timestamp: Instant::now(),
            is_join,
            key_expr: token.key_expr.clone(),
            node_name: token.node_name().unwrap_or_else(|| token.key_expr.clone()),
        };
        self.liveliness_events.push_front(record);
        if self.liveliness_events.len() > LIVELINESS_EVENT_CAP {
            self.liveliness_events.pop_back();
        }

        // Update token state
        if is_join {
            if let Some(existing) = self
                .liveliness_tokens
                .iter_mut()
                .find(|t| t.key_expr == token.key_expr)
            {
                existing.alive = true;
                existing.source_zid = token.source_zid.or(existing.source_zid.clone());
            } else {
                self.liveliness_tokens.push(token);
            }
        } else if let Some(existing) = self
            .liveliness_tokens
            .iter_mut()
            .find(|t| t.key_expr == token.key_expr)
        {
            existing.alive = false;
        }
    }

    fn handle_admin_nodes(&mut self, nodes: Vec<NodeInfo>) {
        self.admin_nodes = nodes;
        self.nodes = merge_nodes(&self.admin_nodes, &self.scout_nodes);
        self.clamp_node_selection();
    }

    fn handle_scout_nodes(&mut self, nodes: Vec<NodeInfo>) {
        self.scout_nodes = nodes;
        self.last_scout_at = Some(SystemTime::now());
        self.scout_in_progress = false;
        self.nodes = merge_nodes(&self.admin_nodes, &self.scout_nodes);
        self.clamp_node_selection();
    }

    fn clamp_node_selection(&mut self) {
        if self.nodes.is_empty() {
            self.node_selected = 0;
        } else if self.node_selected >= self.nodes.len() {
            self.node_selected = self.nodes.len() - 1;
        }
    }

    pub(crate) fn handle_zenoh_message(&mut self, msg: ZenohMessage) {
        // `insert` reports only genuinely new keys, so the steady state of a
        // running stream does no work here at all — no sort, no cache
        // invalidation, no reconsidering the automatic expansion.
        if self.key_tree.insert(&msg.key_expr) {
            self.auto_expand();
        }

        // Pausing freezes what the detail pane reads — the latest payload and
        // the history behind the diff — and nothing else. The counters below
        // keep running, so the pane can say the key is still publishing at
        // 10 Hz while holding the message you stopped to read.
        if !self.history_paused {
            self.topic_latest
                .insert(msg.key_expr.clone(), (msg.clone(), Instant::now()));
            self.history.record(&msg);
        }

        *self
            .topic_msg_counts
            .entry(msg.key_expr.clone())
            .or_insert(0) += 1;
        self.total_msg_count += 1;

        // Application payload bytes (not protocol overhead), from the lossless
        // wire byte counts captured at receive time.
        let bytes = (msg.payload_bytes + msg.attachment_bytes.unwrap_or(0)) as u64;
        *self
            .topic_byte_counts
            .entry(msg.key_expr.clone())
            .or_insert(0) += bytes;
        self.total_byte_count += bytes;
    }

    /// Drop the longest-idle keys once the tree exceeds [`MAX_KEYS`].
    ///
    /// Before this, `key_tree` and `topic_latest` only ever grew — a reconnect
    /// was the sole way a key left the session. `topic_hz`/`topic_rates` were
    /// already bounded by their own idle sweep in `update_hz`, so this closes
    /// the last two.
    ///
    /// Eviction goes past the cap down to [`EVICT_TO_FRACTION`] of it: the sort
    /// is O(n log n) over every key, and without the slack a namespace sitting
    /// at the cap would pay for it on every single new key.
    pub(crate) fn evict_excess_keys(&mut self) {
        if self.key_tree.len() <= MAX_KEYS {
            return;
        }
        let target = (MAX_KEYS as f64 * EVICT_TO_FRACTION) as usize;
        let drop_n = self.key_tree.len().saturating_sub(target);

        let mut by_age: Vec<(String, Instant)> = self
            .topic_latest
            .iter()
            .map(|(k, (_, at))| (k.clone(), *at))
            .collect();
        by_age.sort_by_key(|(_, at)| *at);

        for (key, _) in by_age.into_iter().take(drop_n) {
            self.key_tree.remove(&key);
            self.topic_latest.remove(&key);
            self.topic_hz.remove(&key);
            self.topic_rates.remove(&key);
            self.topic_msg_counts.remove(&key);
            self.topic_byte_counts.remove(&key);
            self.history.remove(&key);
            self.keys_aged_out += 1;
        }

        // Expansion state is keyed by path, so pruned branches would otherwise
        // leave entries behind that nothing can ever reach again — the same
        // unbounded growth one level up.
        self.tree_expanded.retain(|p| self.key_tree.has_children(p));
        self.tree_unfolded.retain(|p| self.key_tree.has_children(p));
        self.tree_expanded_version = self.tree_expanded_version.wrapping_add(1);
    }

    pub fn update_hz(&mut self) {
        let elapsed = self.last_hz_update.elapsed().as_secs_f64();
        if elapsed < 1.0 {
            return;
        }
        self.evict_excess_keys();

        // Every topic we already track a rate for gets a bucket this interval —
        // even if it received nothing (a 0 bucket) — so sparklines show real
        // dips and idle Hz decays to 0 instead of sticking at its last value.
        let mut keys: std::collections::HashSet<String> =
            self.topic_rates.keys().cloned().collect();
        keys.extend(self.topic_msg_counts.keys().cloned());

        for key in keys {
            let msgs = self.topic_msg_counts.get(&key).copied().unwrap_or(0);
            let bytes = self.topic_byte_counts.get(&key).copied().unwrap_or(0);
            self.topic_hz.insert(key.clone(), msgs as f64 / elapsed);
            self.topic_rates
                .entry(key.clone())
                .or_insert_with(|| RateWindow::new(RATE_WINDOW_SECS))
                .push(bytes);
            // Evict topics idle for the whole window to bound memory.
            if self.topic_rates.get(&key).is_some_and(|w| w.is_all_zero()) {
                self.topic_rates.remove(&key);
                self.topic_hz.remove(&key);
            }
        }

        self.topic_msg_counts.clear();
        self.topic_byte_counts.clear();

        self.total_rate.push(self.total_byte_count);
        self.total_hz = self.total_msg_count as f64 / elapsed;
        self.total_msg_count = 0;
        self.total_byte_count = 0;
        self.last_hz_update = Instant::now();
    }

    /// Latest total application bandwidth in bytes/sec (last completed bucket).
    pub(crate) fn total_bytes_per_sec(&self) -> u64 {
        self.total_rate.latest()
    }

    /// Latest bandwidth + history for a specific topic (empty if idle/evicted).
    pub(crate) fn topic_rate_series(&self, key: &str) -> Vec<u64> {
        self.topic_rates
            .get(key)
            .map(|w| w.series())
            .unwrap_or_default()
    }

    pub(crate) fn topic_bytes_per_sec(&self, key: &str) -> u64 {
        self.topic_rates.get(key).map(|w| w.latest()).unwrap_or(0)
    }
}
