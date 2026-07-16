//! Pure structuring for the `scenario` command's **episode JSON**.
//!
//! The network orchestration (subscribe, trigger, stamp `t_rel_ms`) lives in the
//! CLI; this module is deliberately clock-free and network-free so the
//! episode shape is deterministically unit-testable. The CLI stamps each
//! received message into a [`ScenarioEvent`] and calls [`build_episode`] on
//! completion.

use serde_json::{json, Map, Value};

/// One observed message, already stamped with its relative time and with the
/// causal metadata (`correlation_id`, `request_id`) extracted from the
/// attachment. `payload` is the decoded view (`MessagePayload::to_view()`), so
/// msgpack arrives as JSON.
#[derive(Debug, Clone)]
pub struct ScenarioEvent {
    /// Milliseconds since scenario start (first observation / trigger).
    pub t_rel_ms: u64,
    pub key_expr: String,
    /// Causal chain id shared by mission→action→drive→safety, when present.
    pub correlation_id: Option<String>,
    /// Per-request id (e.g. a task request id), when present.
    pub request_id: Option<String>,
    pub encoding: String,
    /// Sample kind ("PUT"/"DELETE").
    pub kind: String,
    /// Decoded payload view.
    pub payload: Value,
    /// True for the synthetic event representing the scenario's own trigger
    /// (the `--pub` actuation or `--task` request) — the causal origin at
    /// `t_rel_ms = 0`. Ordinary observed events are `false`.
    pub trigger: bool,
}

/// What (if anything) the scenario actively triggered before observing.
#[derive(Debug, Clone)]
pub enum TriggerInfo {
    /// No trigger — pure passive observation.
    None,
    /// One-shot `--pub` actuation.
    Pub { key_expr: String, bytes: usize },
    /// A dotori Task request published to `<prefix>/request`.
    Task {
        request_key: String,
        request_bytes: usize,
    },
}

impl TriggerInfo {
    fn to_json(&self) -> Value {
        match self {
            TriggerInfo::None => json!({ "kind": "none" }),
            TriggerInfo::Pub { key_expr, bytes } => json!({
                "kind": "pub",
                "key_expr": key_expr,
                "bytes": bytes,
            }),
            TriggerInfo::Task {
                request_key,
                request_bytes,
            } => json!({
                "kind": "task",
                "request_key": request_key,
                "request_bytes": request_bytes,
            }),
        }
    }
}

/// Why the scenario ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedReason {
    /// A message arrived on the task's `<prefix>/response` topic.
    TaskResponse,
    /// The `--for` window (plus `--settle`) elapsed.
    WindowElapsed,
}

impl EndedReason {
    fn as_str(&self) -> &'static str {
        match self {
            EndedReason::TaskResponse => "task_response",
            EndedReason::WindowElapsed => "window_elapsed",
        }
    }
}

/// The trigger/window context that frames an episode. Paired with the observed
/// events to produce the `meta` block.
#[derive(Debug, Clone)]
pub struct ScenarioMeta {
    pub trigger: TriggerInfo,
    pub for_ms: u64,
    pub settle_ms: u64,
    /// The resolved key expressions that were observed.
    pub observed: Vec<String>,
    pub ended_reason: EndedReason,
}

/// Build the single episode JSON object from the framing [`ScenarioMeta`] and
/// the time-stamped observed events. Pure: no clocks, no network.
///
/// - `topics`: per-key `{count, first_t_rel_ms, last_t_rel_ms}`.
/// - `correlations`: only events carrying a `correlation_id`, grouped by it in
///   time order. Events without one are absent here (they still appear in the
///   timeline with `correlation_id: null`).
/// - `timeline`: every event, ordered by `t_rel_ms`, with the decoded payload.
pub fn build_episode(meta: &ScenarioMeta, events: &[ScenarioEvent]) -> Value {
    // Stable time ordering; ties keep insertion order (a stable sort).
    let mut ordered: Vec<&ScenarioEvent> = events.iter().collect();
    ordered.sort_by_key(|e| e.t_rel_ms);

    // topics: per-key count + first/last relative time.
    let mut topics: Map<String, Value> = Map::new();
    for e in &ordered {
        match topics.get_mut(&e.key_expr) {
            Some(entry) => {
                let count = entry["count"].as_u64().unwrap_or(0) + 1;
                entry["count"] = json!(count);
                // ordered is sorted by t_rel_ms, so the last seen is the latest.
                entry["last_t_rel_ms"] = json!(e.t_rel_ms);
            }
            None => {
                topics.insert(
                    e.key_expr.clone(),
                    json!({
                        "count": 1,
                        "first_t_rel_ms": e.t_rel_ms,
                        "last_t_rel_ms": e.t_rel_ms,
                    }),
                );
            }
        }
    }

    // correlations: group only events that carry a correlation_id, in time order.
    let mut correlations: Map<String, Value> = Map::new();
    for e in &ordered {
        let Some(cid) = &e.correlation_id else {
            continue;
        };
        let entry = correlations
            .entry(cid.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(arr) = entry.as_array_mut() {
            arr.push(json!({
                "t_rel_ms": e.t_rel_ms,
                "key_expr": e.key_expr,
                "request_id": e.request_id,
                "kind": e.kind,
            }));
        }
    }

    // timeline: every event, ordered, with decoded payload.
    let timeline: Vec<Value> = ordered
        .iter()
        .map(|e| {
            let mut obj = json!({
                "t_rel_ms": e.t_rel_ms,
                "key_expr": e.key_expr,
                "correlation_id": e.correlation_id,
                "request_id": e.request_id,
                "encoding": e.encoding,
                "payload": e.payload,
            });
            // Only the trigger event carries the marker, so ordinary events
            // serialize byte-for-byte as before.
            if e.trigger {
                obj["trigger"] = json!(true);
            }
            obj
        })
        .collect();

    json!({
        "meta": {
            "trigger": meta.trigger.to_json(),
            "for_ms": meta.for_ms,
            "settle_ms": meta.settle_ms,
            "observed": meta.observed,
            "message_count": events.len(),
            "ended_reason": meta.ended_reason.as_str(),
        },
        "topics": Value::Object(topics),
        "correlations": Value::Object(correlations),
        "timeline": timeline,
    })
}

/// The mission-stall diagnosis topic set (relative suffixes, prefix applied by
/// [`expand_preset`]).
const STALL_TOPICS: &[&str] = &[
    "topic/safety/safety_state",
    "topic/safety/policy/**",
    "topic/sensor/obstacles",
    "topic/mission/state_snapshot",
    "topic/navigation/robot_pose",
    "topic/forklift/snapshot",
    "topic/actionflow/**",
    "task/**/feedback",
    "task/**/response",
];

/// Expand a named preset into concrete key expressions, each prefixed with
/// `<prefix>/`. Unknown presets yield an empty vec (the CLI treats an empty
/// resolved observe-set as a usage error). The default prefix `**` makes the
/// set prefix-agnostic (`**/topic/safety/safety_state`, …).
pub fn expand_preset(name: &str, prefix: &str) -> Vec<String> {
    let prefix = prefix.trim_end_matches('/');
    match name {
        "stall" => STALL_TOPICS
            .iter()
            .map(|suffix| format!("{}/{}", prefix, suffix))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(
        t: u64,
        key: &str,
        cid: Option<&str>,
        rid: Option<&str>,
        payload: Value,
    ) -> ScenarioEvent {
        ScenarioEvent {
            t_rel_ms: t,
            key_expr: key.to_string(),
            correlation_id: cid.map(String::from),
            request_id: rid.map(String::from),
            encoding: "application/json".to_string(),
            kind: "PUT".to_string(),
            payload,
            trigger: false,
        }
    }

    fn meta() -> ScenarioMeta {
        ScenarioMeta {
            trigger: TriggerInfo::None,
            for_ms: 8000,
            settle_ms: 0,
            observed: vec!["a/**".to_string()],
            ended_reason: EndedReason::WindowElapsed,
        }
    }

    #[test]
    fn topics_summary_counts_and_first_last() {
        let events = vec![
            ev(10, "a/x", None, None, json!({"n": 1})),
            ev(30, "a/x", None, None, json!({"n": 2})),
            ev(20, "b/y", None, None, json!({"n": 3})),
        ];
        let ep = build_episode(&meta(), &events);

        assert_eq!(ep["topics"]["a/x"]["count"], 2);
        assert_eq!(ep["topics"]["a/x"]["first_t_rel_ms"], 10);
        assert_eq!(ep["topics"]["a/x"]["last_t_rel_ms"], 30);
        assert_eq!(ep["topics"]["b/y"]["count"], 1);
        assert_eq!(ep["topics"]["b/y"]["first_t_rel_ms"], 20);
        assert_eq!(ep["topics"]["b/y"]["last_t_rel_ms"], 20);
        assert_eq!(ep["meta"]["message_count"], 3);
    }

    #[test]
    fn correlations_group_shared_id_in_time_order() {
        // Provide out of order; grouping must be by ascending t_rel_ms.
        let events = vec![
            ev(30, "drive/cmd", Some("corr-1"), Some("r9"), json!({})),
            ev(10, "mission/state", Some("corr-1"), Some("r9"), json!({})),
            ev(20, "action/step", Some("corr-1"), None, json!({})),
        ];
        let ep = build_episode(&meta(), &events);

        let chain = ep["correlations"]["corr-1"].as_array().unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0]["t_rel_ms"], 10);
        assert_eq!(chain[0]["key_expr"], "mission/state");
        assert_eq!(chain[0]["request_id"], "r9");
        assert_eq!(chain[1]["t_rel_ms"], 20);
        assert_eq!(chain[1]["key_expr"], "action/step");
        assert_eq!(chain[1]["request_id"], Value::Null);
        assert_eq!(chain[2]["t_rel_ms"], 30);
        assert_eq!(chain[2]["kind"], "PUT");
    }

    #[test]
    fn events_without_correlation_absent_from_correlations_but_in_timeline() {
        let events = vec![
            ev(10, "a/x", None, None, json!({"v": 1})),
            ev(20, "a/y", Some("c"), None, json!({"v": 2})),
        ];
        let ep = build_episode(&meta(), &events);

        // Only the correlated one is grouped.
        let corr = ep["correlations"].as_object().unwrap();
        assert_eq!(corr.len(), 1);
        assert!(corr.contains_key("c"));

        // Both are in the timeline; the uncorrelated one carries null.
        let tl = ep["timeline"].as_array().unwrap();
        assert_eq!(tl.len(), 2);
        assert_eq!(tl[0]["key_expr"], "a/x");
        assert_eq!(tl[0]["correlation_id"], Value::Null);
        assert_eq!(tl[1]["correlation_id"], "c");
    }

    #[test]
    fn timeline_is_ordered_and_carries_decoded_payloads() {
        let events = vec![
            ev(50, "late", None, None, json!({"who": "last"})),
            ev(5, "early", None, None, json!({"who": "first"})),
        ];
        let ep = build_episode(&meta(), &events);

        let tl = ep["timeline"].as_array().unwrap();
        assert_eq!(tl[0]["t_rel_ms"], 5);
        assert_eq!(tl[0]["payload"]["who"], "first");
        assert_eq!(tl[1]["t_rel_ms"], 50);
        assert_eq!(tl[1]["payload"]["who"], "last");
    }

    #[test]
    fn empty_events_yield_empty_sections_and_zero_count() {
        let ep = build_episode(&meta(), &[]);
        assert_eq!(ep["meta"]["message_count"], 0);
        assert_eq!(ep["topics"], json!({}));
        assert_eq!(ep["correlations"], json!({}));
        assert_eq!(ep["timeline"], json!([]));
    }

    #[test]
    fn trigger_event_is_marked_in_timeline() {
        // The synthetic trigger event (the actuation/request that caused the
        // episode) carries `trigger: true` so the causal origin is visible in
        // the timeline, not only in `meta`. Ordinary events omit the marker.
        let mut trig = ev(0, "cmd/go", None, None, json!({ "go": true }));
        trig.trigger = true;
        let normal = ev(10, "a/x", None, None, json!({ "n": 1 }));
        let ep = build_episode(&meta(), &[trig, normal]);

        let tl = ep["timeline"].as_array().unwrap();
        assert_eq!(tl[0]["key_expr"], "cmd/go");
        assert_eq!(tl[0]["trigger"], true);
        // An ordinary event has no `trigger` key at all (output unchanged).
        assert!(tl[1].get("trigger").is_none());
    }

    #[test]
    fn meta_reflects_trigger_and_ended_reason() {
        let m = ScenarioMeta {
            trigger: TriggerInfo::Task {
                request_key: "dotori/forky001/task/mission/mission/request".to_string(),
                request_bytes: 42,
            },
            for_ms: 8000,
            settle_ms: 2000,
            observed: vec!["k1".to_string(), "k2".to_string()],
            ended_reason: EndedReason::TaskResponse,
        };
        let ep = build_episode(&m, &[]);
        assert_eq!(ep["meta"]["trigger"]["kind"], "task");
        assert_eq!(
            ep["meta"]["trigger"]["request_key"],
            "dotori/forky001/task/mission/mission/request"
        );
        assert_eq!(ep["meta"]["trigger"]["request_bytes"], 42);
        assert_eq!(ep["meta"]["for_ms"], 8000);
        assert_eq!(ep["meta"]["settle_ms"], 2000);
        assert_eq!(ep["meta"]["observed"], json!(["k1", "k2"]));
        assert_eq!(ep["meta"]["ended_reason"], "task_response");
    }

    #[test]
    fn pub_trigger_serializes_key_and_bytes() {
        let m = ScenarioMeta {
            trigger: TriggerInfo::Pub {
                key_expr: "cmd/go".to_string(),
                bytes: 7,
            },
            ..meta()
        };
        let ep = build_episode(&m, &[]);
        assert_eq!(ep["meta"]["trigger"]["kind"], "pub");
        assert_eq!(ep["meta"]["trigger"]["key_expr"], "cmd/go");
        assert_eq!(ep["meta"]["trigger"]["bytes"], 7);
    }

    #[test]
    fn expand_preset_stall_applies_prefix() {
        let keys = expand_preset("stall", "dotori/forky001");
        assert_eq!(keys.len(), STALL_TOPICS.len());
        assert!(keys.contains(&"dotori/forky001/topic/safety/safety_state".to_string()));
        assert!(keys.contains(&"dotori/forky001/topic/safety/policy/**".to_string()));
        assert!(keys.contains(&"dotori/forky001/task/**/response".to_string()));
    }

    #[test]
    fn expand_preset_default_prefix_is_prefix_agnostic() {
        let keys = expand_preset("stall", "**");
        assert!(keys.contains(&"**/topic/safety/safety_state".to_string()));
        assert!(keys.contains(&"**/task/**/feedback".to_string()));
    }

    #[test]
    fn expand_preset_trims_trailing_slash_on_prefix() {
        let keys = expand_preset("stall", "dotori/forky001/");
        assert!(keys.contains(&"dotori/forky001/topic/sensor/obstacles".to_string()));
    }

    #[test]
    fn expand_preset_unknown_is_empty() {
        assert!(expand_preset("nope", "**").is_empty());
    }
}
