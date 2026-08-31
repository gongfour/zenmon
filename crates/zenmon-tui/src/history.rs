//! Per-key message history.
//!
//! The detail pane used to show "history" by filtering one global 500-entry
//! ring by key. On a fleet that is not history at all: one neighbour publishing
//! at 100 Hz evicts a 1 Hz key's entire past within seconds, so the pane that is
//! supposed to explain a quiet key is empty exactly when you need it.
//!
//! Each key gets its own ring instead. Two things follow from that which the
//! global ring could not offer: a key's history no longer depends on its
//! neighbours' rates, and there is a previous message to diff the current one
//! against.
//!
//! Storage is deliberately lossy. The newest message is kept in full by
//! `App::topic_latest`; everything older is kept as a capped structured view,
//! because 128 entries per key across a few thousand keys cannot hold whole
//! payloads. Copying is what the pane needs to read, not what the wire carried.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use zenmon_core::types::ZenohMessage;

/// Entries kept per key.
pub const HISTORY_CAP: usize = 128;

/// Per-entry ceiling on the stored view. Past this, an entry keeps a preview
/// and says so.
pub const PAYLOAD_CAP_BYTES: usize = 64 * 1024;

/// Ceiling across all keys.
///
/// The per-key cap alone does not bound anything useful: 2,000 keys × 128
/// entries is 256,000 entries, and a handful of keys publishing large payloads
/// would take the whole budget while the rest starve.
pub const BUDGET_BYTES: usize = 32 * 1024 * 1024;

/// Evict down to this fraction of [`BUDGET_BYTES`], so eviction is an
/// occasional burst rather than a cost on every message once full.
const EVICT_TO_FRACTION: f64 = 0.9;

/// One past message, as much of it as is worth keeping.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// `MessagePayload::to_view_capped` output — parsed JSON where the payload
    /// was JSON, which is what the diff walks.
    pub view: serde_json::Value,
    /// Wire length of the original payload, whether or not `view` holds all of
    /// it.
    pub raw_len: usize,
    /// `view` is a preview, not the whole payload.
    pub truncated: bool,
    pub encoding: String,
    pub timestamp: Option<String>,
    pub kind: String,
    pub received: Instant,
}

impl HistoryEntry {
    fn from_message(msg: &ZenohMessage) -> Self {
        Self {
            view: msg.payload.to_view_capped(PAYLOAD_CAP_BYTES),
            raw_len: msg.payload_bytes,
            truncated: msg.payload.len() > PAYLOAD_CAP_BYTES,
            encoding: msg.encoding.clone(),
            timestamp: msg.timestamp.clone(),
            kind: msg.kind.clone(),
            received: Instant::now(),
        }
    }

    /// Rough retained size.
    ///
    /// The stored value is a parsed structure, not bytes, so its true footprint
    /// would need a walk. The capped wire length is within a small factor and
    /// costs nothing — good enough to drive a budget, and the budget is a
    /// safety rail rather than an accounting figure.
    fn approx_bytes(&self) -> usize {
        self.raw_len.min(PAYLOAD_CAP_BYTES)
    }
}

/// One key's ring, newest first.
#[derive(Debug, Default, Clone)]
pub struct KeyHistory {
    entries: VecDeque<HistoryEntry>,
    approx_bytes: usize,
}

impl KeyHistory {
    /// Newest first.
    pub fn iter(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The most recent entry.
    pub fn latest(&self) -> Option<&HistoryEntry> {
        self.entries.front()
    }

    /// The entry before the most recent — the diff baseline. `None` until a key
    /// has been seen twice, which is why a first message shows no diff.
    pub fn previous(&self) -> Option<&HistoryEntry> {
        self.entries.get(1)
    }

    fn push(&mut self, entry: HistoryEntry) {
        self.approx_bytes += entry.approx_bytes();
        self.entries.push_front(entry);
        while self.entries.len() > HISTORY_CAP {
            self.pop_oldest();
        }
    }

    /// Drop the oldest entry, returning the bytes reclaimed.
    fn pop_oldest(&mut self) -> Option<usize> {
        let dropped = self.entries.pop_back()?;
        let bytes = dropped.approx_bytes();
        self.approx_bytes = self.approx_bytes.saturating_sub(bytes);
        Some(bytes)
    }
}

/// Every key's history, under a shared budget.
#[derive(Debug, Default, Clone)]
pub struct History {
    per_key: HashMap<String, KeyHistory>,
    total_bytes: usize,
    /// Entries dropped to stay under [`BUDGET_BYTES`]. Surfaced in the UI, so a
    /// short history is visibly a budget decision rather than a gap in what the
    /// network sent.
    pub evicted: usize,
}

impl History {
    pub fn record(&mut self, msg: &ZenohMessage) {
        let entry = HistoryEntry::from_message(msg);
        let key = self.per_key.entry(msg.key_expr.clone()).or_default();
        // Take the delta, not the new entry's size: the push may have evicted
        // an older entry to stay under the per-key cap, and that entry can be
        // larger or smaller than this one.
        let before = key.approx_bytes;
        key.push(entry);
        let after = key.approx_bytes;
        if after >= before {
            self.total_bytes += after - before;
        } else {
            self.total_bytes = self.total_bytes.saturating_sub(before - after);
        }
        self.enforce_budget();
    }

    pub fn get(&self, key: &str) -> Option<&KeyHistory> {
        self.per_key.get(key)
    }

    pub fn remove(&mut self, key: &str) {
        if let Some(h) = self.per_key.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(h.approx_bytes);
        }
    }

    pub fn clear(&mut self) {
        self.per_key.clear();
        self.total_bytes = 0;
        self.evicted = 0;
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Keys currently holding at least one entry.
    pub fn keys_held(&self) -> usize {
        self.per_key.len()
    }

    /// Bring the total under budget by repeatedly dropping the oldest entry
    /// from whichever key currently holds the most.
    ///
    /// Greedy against the biggest holder rather than fair across all keys: a
    /// few keys carrying large payloads are exactly what puts the total over,
    /// and taking one entry from every key would punish the quiet ones for it.
    ///
    /// The scan for the largest is O(keys) per drop, which is affordable
    /// because this runs only when over budget and then clears down to
    /// [`EVICT_TO_FRACTION`] — a burst on the way past the ceiling, not a cost
    /// carried by every message once there.
    fn enforce_budget(&mut self) {
        if self.total_bytes <= BUDGET_BYTES {
            return;
        }
        let target = (BUDGET_BYTES as f64 * EVICT_TO_FRACTION) as usize;
        while self.total_bytes > target {
            let Some(key) = self
                .per_key
                .iter()
                .filter(|(_, h)| !h.is_empty())
                .max_by_key(|(_, h)| h.approx_bytes)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            let Some(dropped) = self.per_key.get_mut(&key).and_then(KeyHistory::pop_oldest) else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(dropped);
            self.evicted += 1;
        }
        self.per_key.retain(|_, h| !h.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenmon_core::types::MessagePayload;

    fn msg(key: &str, payload: serde_json::Value) -> ZenohMessage {
        let p = MessagePayload::from_json(&payload);
        ZenohMessage {
            key_expr: key.into(),
            payload_bytes: p.len(),
            payload: p,
            encoding: "application/json".into(),
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        }
    }

    fn big_msg(key: &str, bytes: usize) -> ZenohMessage {
        let p = MessagePayload::from_bytes(vec![b'x'; bytes]);
        ZenohMessage {
            key_expr: key.into(),
            payload_bytes: p.len(),
            payload: p,
            encoding: "application/octet-stream".into(),
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        }
    }

    #[test]
    fn a_keys_history_is_independent_of_its_neighbours_rate() {
        // The whole reason this module exists: under the old global ring a
        // busy key evicted a quiet one's past.
        let mut h = History::default();
        h.record(&msg("slow/key", serde_json::json!({"n": 1})));
        for i in 0..1000 {
            h.record(&msg("fast/key", serde_json::json!({ "n": i })));
        }
        assert_eq!(h.get("slow/key").unwrap().len(), 1);
    }

    #[test]
    fn newest_first_with_a_previous_for_the_diff() {
        let mut h = History::default();
        h.record(&msg("k", serde_json::json!({"n": 1})));
        h.record(&msg("k", serde_json::json!({"n": 2})));
        let k = h.get("k").unwrap();
        assert_eq!(k.latest().unwrap().view, serde_json::json!({"n": 2}));
        assert_eq!(k.previous().unwrap().view, serde_json::json!({"n": 1}));
    }

    #[test]
    fn a_first_message_has_no_baseline_to_diff_against() {
        let mut h = History::default();
        h.record(&msg("k", serde_json::json!({"n": 1})));
        assert!(h.get("k").unwrap().previous().is_none());
    }

    #[test]
    fn the_per_key_cap_drops_the_oldest() {
        let mut h = History::default();
        for i in 0..(HISTORY_CAP + 10) {
            h.record(&msg("k", serde_json::json!({ "n": i })));
        }
        let k = h.get("k").unwrap();
        assert_eq!(k.len(), HISTORY_CAP);
        assert_eq!(
            k.latest().unwrap().view,
            serde_json::json!({"n": HISTORY_CAP + 9})
        );
        // The oldest survivor is entry #10, not #0.
        assert_eq!(k.iter().last().unwrap().view, serde_json::json!({"n": 10}),);
    }

    #[test]
    fn an_oversized_payload_is_stored_as_a_flagged_preview() {
        let mut h = History::default();
        h.record(&big_msg("k", PAYLOAD_CAP_BYTES * 2));
        let e = h.get("k").unwrap().latest().unwrap().clone();
        assert!(e.truncated);
        // The wire length is reported honestly even though the view is a preview.
        assert_eq!(e.raw_len, PAYLOAD_CAP_BYTES * 2);
        assert_eq!(e.view["truncated"], serde_json::json!(true));
        assert_eq!(
            e.view["original_bytes"],
            serde_json::json!(PAYLOAD_CAP_BYTES * 2)
        );
    }

    #[test]
    fn a_payload_at_the_cap_is_kept_whole() {
        let mut h = History::default();
        h.record(&big_msg("k", PAYLOAD_CAP_BYTES));
        assert!(!h.get("k").unwrap().latest().unwrap().truncated);
    }

    /// A message whose declared wire length is `wire` but whose payload is
    /// tiny.
    ///
    /// The budget is driven by `payload_bytes` — the length the message
    /// actually had on the wire — which is a separate field from the stored
    /// view. Fabricating it exercises the accounting without allocating tens of
    /// megabytes of test payload to reach a 32 MB ceiling.
    fn wire_sized_msg(key: &str, wire: usize) -> ZenohMessage {
        let mut m = msg(key, serde_json::json!({"small": true}));
        m.payload_bytes = wire;
        m
    }

    /// Fill past the budget with `keys` hogs, plus one quiet key.
    fn over_budget() -> History {
        let mut h = History::default();
        let per_key_max = HISTORY_CAP * PAYLOAD_CAP_BYTES;
        let hogs = BUDGET_BYTES / per_key_max + 2;
        for k in 0..hogs {
            for _ in 0..HISTORY_CAP {
                h.record(&wire_sized_msg(&format!("hog/{k}"), PAYLOAD_CAP_BYTES));
            }
        }
        for i in 0..5 {
            h.record(&msg("quiet", serde_json::json!({ "n": i })));
        }
        h
    }

    #[test]
    fn the_budget_evicts_from_the_biggest_holder_first() {
        let h = over_budget();
        assert!(h.total_bytes() <= BUDGET_BYTES, "budget must hold");
        assert!(h.evicted > 0, "something had to give");
        // The quiet key kept everything: a few keys carrying large payloads are
        // what put the total over, and taking one entry from every key would
        // punish the ones that were not the reason.
        assert_eq!(h.get("quiet").unwrap().len(), 5);
    }

    #[test]
    fn eviction_clears_below_the_ceiling_not_just_to_it() {
        // The steady state sits somewhere between the target and the ceiling,
        // so the property is about the moment eviction fires, not about the
        // total at rest: it has to shed real slack, or a history parked at the
        // budget re-runs the O(keys) scan on every single message.
        let mut h = History::default();
        let target = (BUDGET_BYTES as f64 * EVICT_TO_FRACTION) as usize;
        let per_key_max = HISTORY_CAP * PAYLOAD_CAP_BYTES;
        let hogs = BUDGET_BYTES / per_key_max + 2;

        let mut fired = 0;
        let mut prev = 0usize;
        for k in 0..hogs {
            for _ in 0..HISTORY_CAP {
                h.record(&wire_sized_msg(&format!("hog/{k}"), PAYLOAD_CAP_BYTES));
                if h.total_bytes() < prev {
                    assert!(
                        h.total_bytes() <= target,
                        "eviction left {}, above the {} target",
                        h.total_bytes(),
                        target
                    );
                    fired += 1;
                }
                prev = h.total_bytes();
            }
        }
        assert!(fired > 0, "the budget was never reached");
    }

    #[test]
    fn removing_a_key_reclaims_its_bytes() {
        let mut h = History::default();
        h.record(&msg("a", serde_json::json!({"n": 1})));
        h.record(&msg("b", serde_json::json!({"n": 1})));
        let before = h.total_bytes();
        h.remove("a");
        assert!(h.get("a").is_none());
        assert!(h.total_bytes() < before);
        assert_eq!(h.keys_held(), 1);
    }

    #[test]
    fn clear_resets_everything_including_the_evicted_count() {
        let mut h = History::default();
        h.record(&msg("a", serde_json::json!({"n": 1})));
        h.clear();
        assert_eq!(h.total_bytes(), 0);
        assert_eq!(h.keys_held(), 0);
        assert_eq!(h.evicted, 0);
    }
}
