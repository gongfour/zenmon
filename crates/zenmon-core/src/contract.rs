//! Loading and querying a zenmon **contract**: a declarative description of the
//! Zenoh protocol a project speaks (key expressions, messaging patterns,
//! encodings, producers/consumers, and payload schemas).
//!
//! Zenoh is schema-less, so a contract is the missing schema layer. This module
//! loads a contract file and answers "what is this topic?" for observed keys.
//! It *displays* schemas rather than *validating* them, so payload schemas are
//! kept as loose [`serde_json::Value`] rather than parsed into typed schemas.

use crate::error::ZenmonError;
use serde::Deserialize;
use serde_json::Value;
use zenoh::key_expr::keyexpr;

/// Normalize a contract key into a valid Zenoh key expression by replacing each
/// `{placeholder}` segment with a single-segment `*` wildcard.
fn normalize_key(key: &str) -> String {
    key.split('/')
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                "*"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Global encoding defaults for a contract.
#[derive(Debug, Clone, Deserialize)]
pub struct EncodingDefaults {
    /// MIME type used when a topic does not set its own `encoding`.
    #[serde(default = "default_encoding")]
    pub default: String,
    /// Whether JSON payloads are MessageEnvelope-wrapped unless a topic overrides.
    #[serde(default = "default_true")]
    pub default_enveloped: bool,
}

impl Default for EncodingDefaults {
    fn default() -> Self {
        Self {
            default: default_encoding(),
            default_enveloped: true,
        }
    }
}

fn default_encoding() -> String {
    "application/json".to_string()
}

fn default_true() -> bool {
    true
}

/// One topic's contract entry. Known metadata is typed; the payload schema body
/// is kept raw for display.
#[derive(Debug, Clone, Deserialize)]
pub struct TopicContract {
    pub key: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub enveloped: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub producers: Vec<String>,
    #[serde(default)]
    pub consumers: Vec<String>,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub phases: Option<Value>,
    #[serde(default)]
    pub request: Option<Value>,
    #[serde(default)]
    pub response: Option<Value>,
}

/// Contract context for a single observed message. Fields are omitted from JSON
/// when not applicable so the enrichment object stays compact.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Enrichment {
    /// Whether the observed key matched any declared topic.
    pub declared: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding_expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding_matches: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enveloped: Option<bool>,
}

/// Compare an observed Zenoh encoding against an expected MIME type by prefix,
/// ignoring parameters like `;charset=utf-8`. Returns `None` when the observed
/// encoding is empty/unknown.
fn encoding_matches(expected: &str, observed: &str) -> Option<bool> {
    let observed = observed.trim();
    if observed.is_empty() {
        return None;
    }
    let base = |s: &str| s.split(';').next().unwrap_or(s).trim().to_string();
    Some(base(expected) == base(observed))
}

/// Result of a structural lint over a contract.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LintReport {
    pub topics: usize,
    pub types: usize,
    pub services: usize,
    pub warnings: Vec<String>,
}

/// A loaded contract.
#[derive(Debug, Clone, Deserialize)]
pub struct Contract {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub encoding: EncodingDefaults,
    #[serde(default)]
    pub types: serde_json::Map<String, Value>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub topics: Vec<TopicContract>,
}

/// Messaging patterns a topic may declare.
const VALID_PATTERNS: &[&str] = &["pub-sub", "call", "task", "liveliness"];

impl Contract {
    /// Parse a contract from a YAML string.
    pub fn from_yaml_str(s: &str) -> Result<Contract, ZenmonError> {
        serde_yaml::from_str(s)
            .map_err(|e| ZenmonError::invalid_input(format!("invalid contract: {e}")))
    }

    /// Load and parse a contract from a file path.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Contract, ZenmonError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| {
            ZenmonError::invalid_input(format!("cannot read contract '{}': {e}", path.display()))
        })?;
        Self::from_yaml_str(&text)
    }

    /// Build the contract context for an observed message.
    pub fn enrich(&self, observed_key: &str, observed_encoding: &str) -> Enrichment {
        match self.lookup(observed_key) {
            None => Enrichment {
                declared: false,
                matched_key: None,
                description: None,
                encoding_expected: None,
                encoding_matches: None,
                enveloped: None,
            },
            Some(t) => {
                let expected = self.effective_encoding(t);
                Enrichment {
                    declared: true,
                    matched_key: Some(t.key.clone()),
                    description: t.description.clone(),
                    encoding_expected: Some(expected.to_string()),
                    encoding_matches: encoding_matches(expected, observed_encoding),
                    enveloped: Some(self.effective_enveloped(t)),
                }
            }
        }
    }

    /// The encoding a topic actually uses: its own `encoding`, or the contract default.
    pub fn effective_encoding<'a>(&'a self, t: &'a TopicContract) -> &'a str {
        t.encoding.as_deref().unwrap_or(&self.encoding.default)
    }

    /// Whether a topic's payload is envelope-wrapped: its own `enveloped`, or the default.
    pub fn effective_enveloped(&self, t: &TopicContract) -> bool {
        t.enveloped.unwrap_or(self.encoding.default_enveloped)
    }

    /// Find the contract entry that best matches a concrete observed key.
    ///
    /// Contract keys may contain doc placeholders (`{sensor_id}`) and Zenoh
    /// wildcards (`*`, `**`); placeholders are normalized to `*` before matching.
    /// When several entries match, the most specific one wins (fewest wildcard
    /// segments, then longest key, then declaration order).
    pub fn lookup(&self, observed_key: &str) -> Option<&TopicContract> {
        let observed = keyexpr::new(observed_key).ok()?;
        let mut best: Option<(&TopicContract, usize, usize)> = None; // (topic, wildcards, key_len)
        for t in &self.topics {
            let normalized = normalize_key(&t.key);
            let Ok(pattern) = keyexpr::new(normalized.as_str()) else {
                continue;
            };
            if !pattern.includes(observed) {
                continue;
            }
            let wildcards = normalized
                .split('/')
                .filter(|s| *s == "*" || *s == "**")
                .count();
            let key_len = t.key.len();
            let better = match best {
                None => true,
                // Fewer wildcard segments is more specific; tie broken by longer key.
                Some((_, bw, bl)) => wildcards < bw || (wildcards == bw && key_len > bl),
            };
            if better {
                best = Some((t, wildcards, key_len));
            }
        }
        best.map(|(t, _, _)| t)
    }

    /// Structurally lint the contract: counts plus non-fatal warnings for
    /// declared-not-implemented topics, duplicate keys, unknown patterns, and
    /// unresolved `$ref`s in payload schemas.
    pub fn lint(&self) -> LintReport {
        let mut warnings = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for t in &self.topics {
            if !seen.insert(t.key.as_str()) {
                warnings.push(format!("duplicate key: {}", t.key));
            }
            if !t.pattern.is_empty() && !VALID_PATTERNS.contains(&t.pattern.as_str()) {
                warnings.push(format!("unknown pattern '{}' on {}", t.pattern, t.key));
            }
            if t.status.as_deref() == Some("declared-not-implemented") {
                warnings.push(format!("declared-not-implemented: {}", t.key));
            }
            for schema in [&t.payload, &t.phases, &t.request, &t.response]
                .into_iter()
                .flatten()
            {
                for name in self.unresolved_refs(schema) {
                    warnings.push(format!("unresolved $ref '{}' in {}", name, t.key));
                }
            }
        }
        LintReport {
            topics: self.topics.len(),
            types: self.types.len(),
            services: self.services.len(),
            warnings,
        }
    }

    /// Collect names of `$ref`s in a schema value that are not defined in `types`.
    fn unresolved_refs(&self, value: &Value) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_unresolved(value, &mut out);
        out
    }

    fn collect_unresolved(&self, value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let (1, Some(Value::String(name))) = (map.len(), map.get("$ref")) {
                    if !self.types.contains_key(name) {
                        out.push(name.clone());
                    }
                    return;
                }
                for v in map.values() {
                    self.collect_unresolved(v, out);
                }
            }
            Value::Array(items) => {
                for v in items {
                    self.collect_unresolved(v, out);
                }
            }
            _ => {}
        }
    }

    /// Recursively expand `{ $ref: TypeName }` objects against the contract's
    /// `types`. An unknown ref is left as `{"$ref": "Name", "$unresolved": true}`
    /// so display can flag it rather than silently drop it. Guards against
    /// self-referential types with a bounded depth.
    pub fn resolve_refs(&self, value: &Value) -> Value {
        self.resolve_refs_depth(value, 0)
    }

    /// The request-payload schema for an observed key, with `$ref`s expanded.
    /// A `task` topic keeps it under `phases.request`; a `call` topic under
    /// `request`. Returns `None` when the key is not declared or has no request
    /// schema. Used to help a caller build a `--task`/`call` request.
    pub fn request_schema(&self, observed_key: &str) -> Option<Value> {
        let topic = self.lookup(observed_key)?;
        let raw = topic
            .phases
            .as_ref()
            .and_then(|p| p.get("request"))
            .or(topic.request.as_ref())?;
        Some(self.resolve_refs(raw))
    }

    fn resolve_refs_depth(&self, value: &Value, depth: usize) -> Value {
        const MAX_DEPTH: usize = 32;
        if depth > MAX_DEPTH {
            return value.clone();
        }
        match value {
            Value::Object(map) => {
                if let (1, Some(Value::String(name))) = (map.len(), map.get("$ref")) {
                    return match self.types.get(name) {
                        Some(def) => self.resolve_refs_depth(def, depth + 1),
                        None => serde_json::json!({ "$ref": name, "$unresolved": true }),
                    };
                }
                Value::Object(
                    map.iter()
                        .map(|(k, v)| (k.clone(), self.resolve_refs_depth(v, depth + 1)))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|v| self.resolve_refs_depth(v, depth + 1))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// Light, display-only validation of a provided request against a schema object:
/// checks only top-level keys. Flags keys present in `provided` but not in
/// `schema` ("unknown field") and required keys in `schema` absent from
/// `provided` ("missing field"). A schema field is treated as optional when its
/// value is a string ending in `?` (the contract's compact notation, e.g.
/// `str?`, `[i32]?`). Non-object inputs yield no warnings.
pub fn validate_against_schema(schema: &Value, provided: &Value) -> Vec<String> {
    let (Some(schema), Some(provided)) = (schema.as_object(), provided.as_object()) else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for key in provided.keys() {
        if !schema.contains_key(key) {
            warnings.push(format!("unknown field '{}' (not in contract request)", key));
        }
    }
    for (key, spec) in schema {
        let spec_str = spec.as_str();
        let optional = spec_str.is_some_and(|s| s.trim_end().ends_with('?'));
        match provided.get(key) {
            None => {
                if !optional {
                    warnings.push(format!("missing field '{}'", key));
                }
            }
            Some(value) => {
                // Enum notation `A|B|C`: the provided value must be one of them.
                if let Some(opts) = spec_str.and_then(enum_options) {
                    let got = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                    if !opts.contains(&got.as_str()) {
                        warnings.push(format!(
                            "field '{}' must be one of [{}], got '{}'",
                            key,
                            opts.join(", "),
                            got
                        ));
                    }
                }
            }
        }
    }
    warnings
}

/// Parse pipe-delimited enum notation from a schema string value
/// (`"waypoint|traversal|action"`, optionally trailing `?`). Returns `None` when
/// the value is not an enum.
fn enum_options(spec: &str) -> Option<Vec<&str>> {
    let s = spec.strip_suffix('?').unwrap_or(spec).trim();
    s.contains('|').then(|| s.split('|').map(str::trim).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TASK: &str = r#"
types:
  Pose2D:
    x: f64
    y: f64
topics:
  - key: task/nav/trajectory
    pattern: task
    phases:
      request:
        points:
          - pose: { $ref: Pose2D }
        trajectory_id: str?
      feedback:
        progress: f64
  - key: call/safety/estop
    pattern: call
    request:
      active: bool
"#;

    #[test]
    fn request_schema_extracts_task_phases_request_with_refs_resolved() {
        let c = Contract::from_yaml_str(TASK).unwrap();
        let s = c.request_schema("task/nav/trajectory").unwrap();
        assert_eq!(s["points"][0]["pose"], json!({ "x": "f64", "y": "f64" }));
        assert_eq!(s["trajectory_id"], "str?");
    }

    #[test]
    fn request_schema_extracts_call_request() {
        let c = Contract::from_yaml_str(TASK).unwrap();
        assert_eq!(c.request_schema("call/safety/estop").unwrap()["active"], "bool");
    }

    #[test]
    fn request_schema_none_for_unknown_key() {
        let c = Contract::from_yaml_str(TASK).unwrap();
        assert!(c.request_schema("task/does/not/exist").is_none());
    }

    #[test]
    fn validate_flags_unknown_and_missing_required_but_not_optional() {
        let schema = json!({ "active": "bool", "mode": "str?" });
        let warns = validate_against_schema(&schema, &json!({ "foo": 1 }));
        assert!(warns.iter().any(|w| w.contains("unknown") && w.contains("foo")));
        assert!(warns.iter().any(|w| w.contains("missing") && w.contains("active")));
        // `mode` is optional (str?) so its absence is not flagged.
        assert!(!warns.iter().any(|w| w.contains("mode")));
    }

    #[test]
    fn validate_clean_request_has_no_warnings() {
        let schema = json!({ "active": "bool" });
        assert!(validate_against_schema(&schema, &json!({ "active": true })).is_empty());
    }

    #[test]
    fn validate_flags_value_not_in_enum() {
        let schema = json!({ "type": "waypoint|traversal|action" });
        let w = validate_against_schema(&schema, &json!({ "type": "node" }));
        assert!(
            w.iter()
                .any(|m| m.contains("type") && m.contains("one of") && m.contains("node")),
            "got {w:?}"
        );
    }

    #[test]
    fn validate_accepts_valid_enum_value() {
        let schema = json!({ "type": "waypoint|traversal|action" });
        assert!(validate_against_schema(&schema, &json!({ "type": "traversal" })).is_empty());
    }

    #[test]
    fn validate_optional_enum_absent_is_ok_but_invalid_when_present() {
        let schema = json!({ "dir": "forward|back?" });
        assert!(validate_against_schema(&schema, &json!({})).is_empty());
        assert!(!validate_against_schema(&schema, &json!({ "dir": "sideways" })).is_empty());
    }

    const SAMPLE: &str = r#"
version: "0.1"
project: demo
encoding:
  default: application/json
  default_enveloped: true
types:
  Pose2D:
    x: f64
    y: f64
topics:
  - key: topic/navigation/robot_pose
    pattern: pub-sub
    encoding: application/json
    producers: [pose_publisher]
    consumers: [orchestrator]
    description: Robot 2D pose
    payload:
      x: f64
      y: f64
      theta: f64
"#;

    const ENC: &str = r#"
encoding:
  default: application/json
  default_enveloped: true
topics:
  - key: a/json
    pattern: pub-sub
  - key: a/msgpack
    pattern: pub-sub
    encoding: application/msgpack
    enveloped: false
"#;

    #[test]
    fn effective_encoding_falls_back_to_default_then_override() {
        let c = Contract::from_yaml_str(ENC).unwrap();
        let json = &c.topics[0];
        let mp = &c.topics[1];
        assert_eq!(c.effective_encoding(json), "application/json");
        assert!(c.effective_enveloped(json));
        assert_eq!(c.effective_encoding(mp), "application/msgpack");
        assert!(!c.effective_enveloped(mp));
    }

    const MATCH: &str = r#"
topics:
  - key: topic/navigation/robot_pose
    pattern: pub-sub
  - key: topic/sensor/pcd/{sensor_id}
    pattern: pub-sub
  - key: topic/safety/policy/{policy_name}
    pattern: pub-sub
  - key: topic/behavior/**
    pattern: pub-sub
  - key: topic/behavior/snapshot
    pattern: pub-sub
"#;

    const REFS: &str = r#"
types:
  Pose2D:
    x: f64
    y: f64
  Waypoint:
    pose: { $ref: Pose2D }
    velocity: f64
topics: []
"#;

    const LINT: &str = r#"
types:
  Pose2D: { x: f64 }
services: [a, b]
topics:
  - key: topic/a
    pattern: pub-sub
  - key: topic/a
    pattern: pub-sub
  - key: topic/b
    pattern: bogus
  - key: topic/c
    pattern: pub-sub
    status: declared-not-implemented
  - key: topic/d
    pattern: pub-sub
    payload:
      pose: { $ref: Missing }
"#;

    const ENRICH: &str = r#"
encoding:
  default: application/json
  default_enveloped: true
topics:
  - key: topic/navigation/robot_pose
    pattern: pub-sub
    description: Robot 2D pose
  - key: topic/sensor/pcd/{sensor_id}
    pattern: pub-sub
    encoding: application/msgpack
    enveloped: false
    description: Per-sensor point cloud
"#;

    #[test]
    fn enrich_declared_topic_with_matching_encoding() {
        let c = Contract::from_yaml_str(ENRICH).unwrap();
        let e = c.enrich("topic/navigation/robot_pose", "application/json");
        assert!(e.declared);
        assert_eq!(e.matched_key.as_deref(), Some("topic/navigation/robot_pose"));
        assert_eq!(e.description.as_deref(), Some("Robot 2D pose"));
        assert_eq!(e.encoding_expected.as_deref(), Some("application/json"));
        assert_eq!(e.encoding_matches, Some(true));
        assert_eq!(e.enveloped, Some(true));
    }

    #[test]
    fn enrich_flags_encoding_mismatch() {
        let c = Contract::from_yaml_str(ENRICH).unwrap();
        // Point-cloud topic expects msgpack; observed says json.
        let e = c.enrich("topic/sensor/pcd/front", "application/json");
        assert!(e.declared);
        assert_eq!(e.matched_key.as_deref(), Some("topic/sensor/pcd/{sensor_id}"));
        assert_eq!(e.encoding_expected.as_deref(), Some("application/msgpack"));
        assert_eq!(e.encoding_matches, Some(false));
        assert_eq!(e.enveloped, Some(false));
    }

    #[test]
    fn enrich_encoding_matches_ignores_mime_params() {
        let c = Contract::from_yaml_str(ENRICH).unwrap();
        let e = c.enrich("topic/navigation/robot_pose", "application/json;charset=utf-8");
        assert_eq!(e.encoding_matches, Some(true));
    }

    #[test]
    fn enrich_undeclared_topic() {
        let c = Contract::from_yaml_str(ENRICH).unwrap();
        let e = c.enrich("topic/foo/unknown", "application/json");
        assert!(!e.declared);
        assert!(e.matched_key.is_none());
        assert!(e.encoding_expected.is_none());
    }

    #[test]
    fn enrich_undeclared_serializes_to_declared_false_only() {
        let c = Contract::from_yaml_str(ENRICH).unwrap();
        let e = c.enrich("topic/foo/unknown", "application/json");
        assert_eq!(serde_json::to_value(&e).unwrap(), serde_json::json!({ "declared": false }));
    }

    #[test]
    fn lint_reports_counts() {
        let c = Contract::from_yaml_str(LINT).unwrap();
        let r = c.lint();
        assert_eq!(r.topics, 5);
        assert_eq!(r.types, 1);
        assert_eq!(r.services, 2);
    }

    #[test]
    fn lint_warns_on_duplicate_key() {
        let r = Contract::from_yaml_str(LINT).unwrap().lint();
        assert!(r.warnings.iter().any(|w| w.contains("duplicate") && w.contains("topic/a")));
    }

    #[test]
    fn lint_warns_on_unknown_pattern() {
        let r = Contract::from_yaml_str(LINT).unwrap().lint();
        assert!(r.warnings.iter().any(|w| w.contains("pattern") && w.contains("bogus")));
    }

    #[test]
    fn lint_warns_on_not_implemented() {
        let r = Contract::from_yaml_str(LINT).unwrap().lint();
        assert!(r.warnings.iter().any(|w| w.contains("not-implemented") && w.contains("topic/c")));
    }

    #[test]
    fn lint_warns_on_unresolved_ref() {
        let r = Contract::from_yaml_str(LINT).unwrap().lint();
        assert!(r.warnings.iter().any(|w| w.contains("Missing")));
    }

    #[test]
    fn resolve_refs_expands_named_type() {
        let c = Contract::from_yaml_str(REFS).unwrap();
        let input = serde_json::json!({ "pose": { "$ref": "Pose2D" } });
        let out = c.resolve_refs(&input);
        assert_eq!(out, serde_json::json!({ "pose": { "x": "f64", "y": "f64" } }));
    }

    #[test]
    fn resolve_refs_expands_nested_refs() {
        let c = Contract::from_yaml_str(REFS).unwrap();
        let input = serde_json::json!({ "$ref": "Waypoint" });
        let out = c.resolve_refs(&input);
        assert_eq!(
            out,
            serde_json::json!({ "pose": { "x": "f64", "y": "f64" }, "velocity": "f64" })
        );
    }

    #[test]
    fn resolve_refs_marks_unknown_ref() {
        let c = Contract::from_yaml_str(REFS).unwrap();
        let input = serde_json::json!({ "$ref": "Nope" });
        let out = c.resolve_refs(&input);
        assert_eq!(out["$unresolved"], true);
        assert_eq!(out["$ref"], "Nope");
    }

    #[test]
    fn lookup_matches_literal_key() {
        let c = Contract::from_yaml_str(MATCH).unwrap();
        let t = c.lookup("topic/navigation/robot_pose").unwrap();
        assert_eq!(t.key, "topic/navigation/robot_pose");
    }

    #[test]
    fn lookup_matches_placeholder_segment() {
        let c = Contract::from_yaml_str(MATCH).unwrap();
        let t = c.lookup("topic/sensor/pcd/front").unwrap();
        assert_eq!(t.key, "topic/sensor/pcd/{sensor_id}");
    }

    #[test]
    fn lookup_returns_none_for_undeclared() {
        let c = Contract::from_yaml_str(MATCH).unwrap();
        assert!(c.lookup("topic/foo/unknown").is_none());
    }

    #[test]
    fn lookup_prefers_most_specific_over_double_star() {
        // Both `topic/behavior/**` and the literal `topic/behavior/snapshot`
        // match; the literal (fewer wildcards) must win.
        let c = Contract::from_yaml_str(MATCH).unwrap();
        let t = c.lookup("topic/behavior/snapshot").unwrap();
        assert_eq!(t.key, "topic/behavior/snapshot");
    }

    #[test]
    fn lookup_double_star_matches_deep() {
        let c = Contract::from_yaml_str(MATCH).unwrap();
        let t = c.lookup("topic/behavior/library/list").unwrap();
        assert_eq!(t.key, "topic/behavior/**");
    }

    #[test]
    fn parses_topics_and_metadata() {
        let c = Contract::from_yaml_str(SAMPLE).unwrap();
        assert_eq!(c.project, "demo");
        assert_eq!(c.topics.len(), 1);
        let t = &c.topics[0];
        assert_eq!(t.key, "topic/navigation/robot_pose");
        assert_eq!(t.pattern, "pub-sub");
        assert_eq!(t.description.as_deref(), Some("Robot 2D pose"));
        assert_eq!(t.producers, vec!["pose_publisher"]);
    }
}
