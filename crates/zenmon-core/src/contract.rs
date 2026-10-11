//! Loading and querying a zenmon **contract**: a declarative description of the
//! Zenoh protocol a project speaks (key expressions, messaging patterns,
//! encodings, producers/consumers, and payload schemas).
//!
//! Zenoh is schema-less, so a contract is the missing schema layer. This module
//! loads a contract file and answers "what is this topic?" for observed keys.
//! It *displays* schemas rather than *validating* them, so payload schemas are
//! kept as loose [`serde_json::Value`] rather than parsed into typed schemas.
//!
//! **zenmon application internals — not part of the public API.** The contract
//! file format is owned by the `zenmon` CLI; this module is `pub` only because
//! `zenmon-cli` is a separate crate, and carries no stability promise.

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
    /// Whether payloads are wrapped in a project-specific envelope unless a
    /// topic overrides. zenmon does not know an envelope's shape, so an
    /// enveloped topic's payload schema describes the inner value and is not
    /// checked against what arrives on the wire. Defaults to `false`: a bare
    /// payload is the plain Zenoh case.
    #[serde(default)]
    pub default_enveloped: bool,
}

impl Default for EncodingDefaults {
    fn default() -> Self {
        Self {
            default: default_encoding(),
            default_enveloped: false,
        }
    }
}

fn default_encoding() -> String {
    "application/json".to_string()
}

/// One participant on a topic (contract v0.2).
///
/// A contract's atom is the endpoint, not the topic: two services on the same
/// key can disagree on payload type and QoS, and that disagreement is exactly
/// what is worth reporting. `producers`/`consumers` (v0.1) can only name who is
/// present, so they have nowhere to put per-participant detail.
///
/// `qos` is kept as a loose [`Value`] for the same reason payload schemas are —
/// zenmon displays and compares it, but does not interpret Zenoh's QoS model.
#[derive(Debug, Clone, Deserialize)]
pub struct Endpoint {
    /// Service that owns this endpoint. Should appear in the contract's `services`.
    pub service: String,
    /// One of [`ENDPOINT_ROLES`].
    #[serde(default)]
    pub role: String,
    /// Payload/message type as the service declares it (e.g. `fleet::SafetyState`).
    #[serde(default, rename = "type")]
    pub message_type: Option<String>,
    /// QoS as declared by this endpoint. Shape is project-defined.
    #[serde(default)]
    pub qos: Option<Value>,
    /// `generated` (extracted from source) or `declared` (hand-authored, e.g. a
    /// participant written in another language that no extractor can see).
    #[serde(default)]
    pub origin: Option<String>,
}

/// Roles an endpoint may declare. The producing roles are the ones that put the
/// first bytes on the wire — a call/task *client* produces the request, so it is
/// a producer, matching how v0.1 contracts used `producers` for call/task.
pub const ENDPOINT_ROLES: &[&str] = &[
    "publisher",
    "subscriber",
    "call_server",
    "call_client",
    "task_server",
    "task_client",
];

const PRODUCING_ROLES: &[&str] = &["publisher", "call_client", "task_client"];
const CONSUMING_ROLES: &[&str] = &["subscriber", "call_server", "task_server"];

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
    /// v0.1 participant list. Prefer [`TopicContract::producer_names`], which
    /// derives from `endpoints` when a v0.2 contract supplies them.
    #[serde(default)]
    pub producers: Vec<String>,
    /// v0.1 participant list. See [`TopicContract::consumer_names`].
    #[serde(default)]
    pub consumers: Vec<String>,
    /// v0.2 participants, with per-endpoint type and QoS.
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub phases: Option<Value>,
    #[serde(default)]
    pub request: Option<Value>,
    #[serde(default)]
    pub response: Option<Value>,
}

impl TopicContract {
    /// Services that originate traffic on this topic.
    ///
    /// Derived from `endpoints` when present (v0.2), else the v0.1 `producers`
    /// list. Names are deduplicated and keep declaration order.
    pub fn producer_names(&self) -> Vec<String> {
        self.names_for(PRODUCING_ROLES, &self.producers)
    }

    /// Services that receive traffic on this topic. See [`Self::producer_names`].
    pub fn consumer_names(&self) -> Vec<String> {
        self.names_for(CONSUMING_ROLES, &self.consumers)
    }

    fn names_for(&self, roles: &[&str], legacy: &[String]) -> Vec<String> {
        if self.endpoints.is_empty() {
            return legacy.to_vec();
        }
        let mut out: Vec<String> = Vec::new();
        for e in &self.endpoints {
            if roles.contains(&e.role.as_str()) && !out.contains(&e.service) {
                out.push(e.service.clone());
            }
        }
        out
    }

    /// Endpoints in the producing direction, for checks that need the records
    /// themselves rather than just the names.
    pub fn producing_endpoints(&self) -> impl Iterator<Item = &Endpoint> {
        self.endpoints
            .iter()
            .filter(|e| PRODUCING_ROLES.contains(&e.role.as_str()))
    }
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

/// Render a value with object keys sorted, so two structurally equal QoS blocks
/// compare equal regardless of the order they were written in the YAML.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body = keys
                .iter()
                .map(|k| format!("{}:{}", k, canonical_json(&map[*k])))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(items) => {
            let body = items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{body}]")
        }
        other => other.to_string(),
    }
}

/// Result of a structural lint over a contract.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LintReport {
    pub topics: usize,
    pub types: usize,
    pub services: usize,
    /// Total endpoints across all topics (v0.2). Zero for a v0.1 contract.
    pub endpoints: usize,
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
    /// Named observe-sets for `zenmon scenario --preset NAME`. Keys are
    /// relative to `--prefix`, so one preset serves any fleet namespace. They
    /// live here rather than in zenmon because which topics matter for a
    /// diagnosis is a fact about the project, not about the tool.
    #[serde(default)]
    pub presets: std::collections::BTreeMap<String, Vec<String>>,
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
            self.lint_endpoints(t, &mut warnings);
        }
        for (name, keys) in &self.presets {
            if keys.is_empty() {
                warnings.push(format!("empty preset '{}'", name));
            }
            for k in keys {
                // Checked as the default `**` prefix would expand it.
                let expanded = format!("**/{}", normalize_key(k));
                if keyexpr::new(expanded.as_str()).is_err() {
                    warnings.push(format!("invalid key '{}' in preset '{}'", k, name));
                }
            }
        }
        LintReport {
            topics: self.topics.len(),
            types: self.types.len(),
            services: self.services.len(),
            endpoints: self.topics.iter().map(|t| t.endpoints.len()).sum(),
            warnings,
        }
    }

    /// Endpoint-level checks (v0.2). Only unambiguous disagreements are reported.
    ///
    /// Deliberately *not* checked: QoS differences between a producer and a
    /// consumer, or among consumers. In Zenoh, congestion control and priority
    /// are publisher-side concerns, so a difference there does not by itself
    /// mean anything is wrong — reporting it would be noise. Producers of the
    /// same key disagreeing with each other has no such excuse.
    ///
    /// Also not checked: a topic with no producer or no consumer. Real systems
    /// legitimately have participants a source extractor cannot see (other
    /// languages, external tools); that belongs in a coverage pass once such
    /// participants are declared, not here.
    fn lint_endpoints(&self, t: &TopicContract, warnings: &mut Vec<String>) {
        for e in &t.endpoints {
            if !e.role.is_empty() && !ENDPOINT_ROLES.contains(&e.role.as_str()) {
                warnings.push(format!(
                    "unknown role '{}' on {} ({})",
                    e.role, t.key, e.service
                ));
            }
            if !self.services.is_empty() && !self.services.contains(&e.service) {
                warnings.push(format!("undeclared service '{}' on {}", e.service, t.key));
            }
        }

        // Payload type must agree across every participant on a key: a producer
        // writing an untyped payload while consumers expect a struct is a real
        // defect that a per-topic `payload` schema cannot express.
        let mut types: Vec<(&str, &str)> = Vec::new();
        for e in &t.endpoints {
            if let Some(ty) = e.message_type.as_deref() {
                if !types.iter().any(|(_, seen)| *seen == ty) {
                    types.push((e.service.as_str(), ty));
                }
            }
        }
        if types.len() > 1 {
            let detail = types
                .iter()
                .map(|(svc, ty)| format!("{svc}={ty}"))
                .collect::<Vec<_>>()
                .join(", ");
            warnings.push(format!("type mismatch on {}: {}", t.key, detail));
        }

        let mut qos: Vec<(&str, String)> = Vec::new();
        for e in t.producing_endpoints() {
            if let Some(q) = &e.qos {
                let rendered = canonical_json(q);
                if !qos.iter().any(|(_, seen)| *seen == rendered) {
                    qos.push((e.service.as_str(), rendered));
                }
            }
        }
        if qos.len() > 1 {
            let detail = qos
                .iter()
                .map(|(svc, q)| format!("{svc}={q}"))
                .collect::<Vec<_>>()
                .join(", ");
            warnings.push(format!("producer QoS mismatch on {}: {}", t.key, detail));
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
                    let got = value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string());
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

/// What [`Contract::check_payload`] could say about one message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", content = "violations", rename_all = "snake_case")]
pub enum PayloadCheck {
    /// Checked against the topic's payload schema; empty means it conforms.
    Checked(Vec<String>),
    /// The key is not in the contract, or its entry has no payload schema.
    NoSchema,
    /// The topic is enveloped: the schema describes a wrapped value whose
    /// envelope zenmon does not know, so the wire payload cannot be checked.
    Enveloped,
}

/// Cap on violations reported per message, so one wrong array of a thousand
/// items does not become a thousand lines.
const MAX_VIOLATIONS: usize = 20;

impl Contract {
    /// Check a decoded payload against the payload schema declared for the
    /// key: field presence (`?` marks optional), unknown fields, scalar types
    /// (`f64`, `i32`, `u8`, `str`, `bool`, …, integer ranges included), enums
    /// (`A|B|C`), arrays (`[T]` or a one-item list) and nested objects, with
    /// `$ref`s expanded. A spec zenmon does not recognise is accepted rather
    /// than flagged.
    pub fn check_payload(&self, observed_key: &str, payload: &Value) -> PayloadCheck {
        let Some(topic) = self.lookup(observed_key) else {
            return PayloadCheck::NoSchema;
        };
        let Some(schema) = &topic.payload else {
            return PayloadCheck::NoSchema;
        };
        if self.effective_enveloped(topic) {
            return PayloadCheck::Enveloped;
        }
        let schema = self.resolve_refs(schema);
        let mut out = Vec::new();
        check_value(&schema, payload, "", &mut out);
        out.truncate(MAX_VIOLATIONS);
        PayloadCheck::Checked(out)
    }
}

fn check_value(schema: &Value, value: &Value, path: &str, out: &mut Vec<String>) {
    if out.len() >= MAX_VIOLATIONS {
        return;
    }
    let at = |p: &str| {
        if p.is_empty() {
            "payload".to_string()
        } else {
            format!("'{p}'")
        }
    };
    match schema {
        Value::Object(fields) => {
            if fields.contains_key("$unresolved") {
                return;
            }
            let Some(obj) = value.as_object() else {
                out.push(format!(
                    "{} must be an object, got {}",
                    at(path),
                    kind_of(value)
                ));
                return;
            };
            for key in obj.keys() {
                if !fields.contains_key(key) {
                    out.push(format!("unknown field '{}'", join(path, key)));
                }
            }
            for (key, spec) in fields {
                let optional = spec.as_str().is_some_and(|s| s.trim_end().ends_with('?'));
                match obj.get(key) {
                    None | Some(Value::Null) if optional => {}
                    None => out.push(format!("missing field '{}'", join(path, key))),
                    Some(v) => check_value(spec, v, &join(path, key), out),
                }
            }
        }
        Value::Array(items) => {
            let Some(arr) = value.as_array() else {
                out.push(format!(
                    "{} must be an array, got {}",
                    at(path),
                    kind_of(value)
                ));
                return;
            };
            if let Some(item) = items.first() {
                for (i, v) in arr.iter().enumerate() {
                    check_value(item, v, &format!("{path}[{i}]"), out);
                }
            }
        }
        Value::String(spec) => check_scalar(spec, value, path, out),
        _ => {}
    }
}

fn check_scalar(spec: &str, value: &Value, path: &str, out: &mut Vec<String>) {
    let spec = spec.trim();
    let spec = spec.strip_suffix('?').unwrap_or(spec).trim();
    let shown = |p: &str| {
        if p.is_empty() {
            "payload".to_string()
        } else {
            format!("'{p}'")
        }
    };
    if let Some(opts) = enum_options(spec) {
        let got = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        if !opts.contains(&got.as_str()) {
            out.push(format!(
                "{} must be one of [{}], got '{}'",
                shown(path),
                opts.join(", "),
                got
            ));
        }
        return;
    }
    if let Some(inner) = spec.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let Some(arr) = value.as_array() else {
            out.push(format!(
                "{} must be an array, got {}",
                shown(path),
                kind_of(value)
            ));
            return;
        };
        for (i, v) in arr.iter().enumerate() {
            check_scalar(inner, v, &format!("{path}[{i}]"), out);
        }
        return;
    }
    let ok = match spec {
        "f32" | "f64" | "float" | "double" | "number" => value.is_number(),
        "str" | "string" => value.is_string(),
        "bool" | "boolean" => value.is_boolean(),
        int if integer_bounds(int).is_some() => {
            let (lo, hi) = integer_bounds(int).unwrap();
            match value
                .as_i64()
                .map(i128::from)
                .or(value.as_u64().map(i128::from))
            {
                Some(n) if n >= lo && n <= hi => true,
                Some(n) => {
                    out.push(format!("{} = {n} is out of range for {int}", shown(path)));
                    return;
                }
                None => false,
            }
        }
        // Unknown spec (a project type name, `any`, `json`): not ours to judge.
        _ => true,
    };
    if !ok {
        out.push(format!(
            "{} must be {spec}, got {}",
            shown(path),
            kind_of(value)
        ));
    }
}

fn integer_bounds(spec: &str) -> Option<(i128, i128)> {
    let bits: u32 = spec.get(1..)?.parse().ok()?;
    if ![8, 16, 32, 64].contains(&bits) {
        return None;
    }
    match spec.as_bytes()[0] {
        b'i' => Some((-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)),
        b'u' => Some((0, (1i128 << bits) - 1)),
        _ => None,
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Parse pipe-delimited enum notation from a schema string value
/// (`"waypoint|traversal|action"`, optionally trailing `?`). Returns `None` when
/// the value is not an enum.
fn enum_options(spec: &str) -> Option<Vec<&str>> {
    let s = spec.strip_suffix('?').unwrap_or(spec).trim();
    s.contains('|')
        .then(|| s.split('|').map(str::trim).collect())
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
        assert_eq!(
            c.request_schema("call/safety/estop").unwrap()["active"],
            "bool"
        );
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
        assert!(warns
            .iter()
            .any(|w| w.contains("unknown") && w.contains("foo")));
        assert!(warns
            .iter()
            .any(|w| w.contains("missing") && w.contains("active")));
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
        assert_eq!(
            e.matched_key.as_deref(),
            Some("topic/navigation/robot_pose")
        );
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
        assert_eq!(
            e.matched_key.as_deref(),
            Some("topic/sensor/pcd/{sensor_id}")
        );
        assert_eq!(e.encoding_expected.as_deref(), Some("application/msgpack"));
        assert_eq!(e.encoding_matches, Some(false));
        assert_eq!(e.enveloped, Some(false));
    }

    #[test]
    fn enrich_encoding_matches_ignores_mime_params() {
        let c = Contract::from_yaml_str(ENRICH).unwrap();
        let e = c.enrich(
            "topic/navigation/robot_pose",
            "application/json;charset=utf-8",
        );
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
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({ "declared": false })
        );
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
        assert!(r
            .warnings
            .iter()
            .any(|w| w.contains("duplicate") && w.contains("topic/a")));
    }

    #[test]
    fn lint_warns_on_unknown_pattern() {
        let r = Contract::from_yaml_str(LINT).unwrap().lint();
        assert!(r
            .warnings
            .iter()
            .any(|w| w.contains("pattern") && w.contains("bogus")));
    }

    #[test]
    fn lint_warns_on_not_implemented() {
        let r = Contract::from_yaml_str(LINT).unwrap().lint();
        assert!(r
            .warnings
            .iter()
            .any(|w| w.contains("not-implemented") && w.contains("topic/c")));
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
        assert_eq!(
            out,
            serde_json::json!({ "pose": { "x": "f64", "y": "f64" } })
        );
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

    // ── v0.2 endpoints ───────────────────────────────────────────────────

    const V2: &str = r#"
project: v2
services: [alpha, beta]
topics:
  - key: topic/clean
    pattern: pub-sub
    endpoints:
      - { service: alpha, role: publisher,  type: T, qos: { a: 1, b: 2 }, origin: generated }
      - { service: beta,  role: subscriber, type: T, qos: { a: 9 },       origin: generated }
  - key: topic/mixed_type
    pattern: pub-sub
    endpoints:
      - { service: alpha, role: publisher,  type: nlohmann::json }
      - { service: beta,  role: subscriber, type: T }
  - key: topic/mixed_qos
    pattern: pub-sub
    endpoints:
      - { service: alpha, role: publisher, qos: { congestion_control: Block } }
      - { service: beta,  role: publisher, qos: { congestion_control: Drop } }
  - key: topic/bad
    pattern: pub-sub
    endpoints:
      - { service: alpha,  role: listener }
      - { service: ghost,  role: subscriber }
  - key: call/thing
    pattern: call
    endpoints:
      - { service: alpha, role: call_client }
      - { service: beta,  role: call_server }
"#;

    fn warnings_for(key: &str, c: &Contract) -> Vec<String> {
        c.lint()
            .warnings
            .into_iter()
            .filter(|w| w.contains(key))
            .collect()
    }

    #[test]
    fn v2_derives_producers_and_consumers_from_endpoints() {
        let c = Contract::from_yaml_str(V2).unwrap();
        let t = c.lookup("topic/clean").unwrap();
        assert_eq!(t.producer_names(), vec!["alpha"]);
        assert_eq!(t.consumer_names(), vec!["beta"]);
    }

    /// A call/task client puts the request on the wire first, so it is the
    /// producer — the same reading v0.1 contracts used.
    #[test]
    fn v2_call_client_is_the_producer() {
        let c = Contract::from_yaml_str(V2).unwrap();
        let t = c.lookup("call/thing").unwrap();
        assert_eq!(t.producer_names(), vec!["alpha"]);
        assert_eq!(t.consumer_names(), vec!["beta"]);
    }

    /// v0.1 files have no endpoints; the legacy lists must still be returned.
    #[test]
    fn v1_falls_back_to_legacy_producer_lists() {
        let c = Contract::from_yaml_str(SAMPLE).unwrap();
        let t = &c.topics[0];
        assert!(t.endpoints.is_empty());
        assert_eq!(t.producer_names(), vec!["pose_publisher"]);
    }

    #[test]
    fn v2_lints_type_mismatch() {
        let c = Contract::from_yaml_str(V2).unwrap();
        let w = warnings_for("topic/mixed_type", &c);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].starts_with("type mismatch on topic/mixed_type"),
            "{}",
            w[0]
        );
    }

    #[test]
    fn v2_lints_producer_qos_mismatch() {
        let c = Contract::from_yaml_str(V2).unwrap();
        let w = warnings_for("topic/mixed_qos", &c);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].starts_with("producer QoS mismatch"), "{}", w[0]);
    }

    /// Producer-vs-consumer QoS differences are deliberately not reported:
    /// congestion control and priority are publisher-side in Zenoh.
    #[test]
    fn v2_does_not_lint_consumer_qos_difference() {
        let c = Contract::from_yaml_str(V2).unwrap();
        assert!(warnings_for("topic/clean", &c).is_empty());
    }

    #[test]
    fn v2_lints_unknown_role_and_undeclared_service() {
        let c = Contract::from_yaml_str(V2).unwrap();
        let w = warnings_for("topic/bad", &c);
        assert!(
            w.iter().any(|x| x.contains("unknown role 'listener'")),
            "{w:?}"
        );
        assert!(
            w.iter().any(|x| x.contains("undeclared service 'ghost'")),
            "{w:?}"
        );
    }

    /// QoS blocks that differ only in key order are the same QoS.
    #[test]
    fn v2_qos_comparison_ignores_key_order() {
        let yaml = r#"
topics:
  - key: topic/t
    endpoints:
      - { service: a, role: publisher, qos: { x: 1, y: 2 } }
      - { service: b, role: publisher, qos: { y: 2, x: 1 } }
"#;
        let c = Contract::from_yaml_str(yaml).unwrap();
        assert!(warnings_for("topic/t", &c).is_empty());
    }

    #[test]
    fn lint_counts_endpoints() {
        let c = Contract::from_yaml_str(V2).unwrap();
        assert_eq!(c.lint().endpoints, 10);
        assert_eq!(Contract::from_yaml_str(SAMPLE).unwrap().lint().endpoints, 0);
    }

    #[test]
    fn presets_parse_and_default_to_empty() {
        let yaml = r#"
presets:
  stall: [topic/state, "task/**/response"]
"#;
        let c = Contract::from_yaml_str(yaml).unwrap();
        assert_eq!(c.presets["stall"], ["topic/state", "task/**/response"]);
        assert!(Contract::from_yaml_str(SAMPLE).unwrap().presets.is_empty());
    }

    #[test]
    fn lint_warns_on_empty_or_invalid_preset() {
        let yaml = r#"
presets:
  none: []
  bad: ["topic/a**b"]
  good: ["topic/{robot}/state"]
"#;
        let w = Contract::from_yaml_str(yaml).unwrap().lint().warnings;
        assert!(w.iter().any(|x| x == "empty preset 'none'"), "{w:?}");
        assert!(
            w.iter()
                .any(|x| x.contains("invalid key 'topic/a**b' in preset 'bad'")),
            "{w:?}"
        );
        assert!(!w.iter().any(|x| x.contains("'good'")), "{w:?}");
    }

    const STRICT: &str = r#"
types:
  Pose2D:
    x: f64
    y: f64
topics:
  - key: robot/state
    payload:
      pose: { $ref: Pose2D }
      mode: idle|run|fault
      battery: u8
      tags: "[str]?"
      waypoints:
        - { id: u32, label: str? }
  - key: robot/wrapped
    enveloped: true
    payload: { v: f64 }
"#;

    fn check(payload: Value) -> PayloadCheck {
        Contract::from_yaml_str(STRICT)
            .unwrap()
            .check_payload("robot/state", &payload)
    }

    #[test]
    fn a_conforming_payload_has_no_violations() {
        let ok = json!({
            "pose": {"x": 1.0, "y": 2},
            "mode": "run",
            "battery": 87,
            "waypoints": [{"id": 1}, {"id": 2, "label": "dock"}]
        });
        assert_eq!(check(ok), PayloadCheck::Checked(vec![]));
    }

    #[test]
    fn violations_name_the_field_and_the_problem() {
        let bad = json!({
            "pose": {"x": "1.0"},
            "mode": "flying",
            "battery": 300,
            "tags": [1],
            "waypoints": [{"id": -1}],
            "extra": true
        });
        let PayloadCheck::Checked(v) = check(bad) else {
            panic!()
        };
        for expected in [
            "unknown field 'extra'",
            "'pose.x' must be f64, got string",
            "missing field 'pose.y'",
            "'mode' must be one of [idle, run, fault], got 'flying'",
            "'battery' = 300 is out of range for u8",
            "'tags[0]' must be str, got integer",
            "'waypoints[0].id' = -1 is out of range for u32",
        ] {
            assert!(
                v.iter().any(|x| x == expected),
                "missing {expected:?} in {v:#?}"
            );
        }
    }

    #[test]
    fn enveloped_and_undeclared_topics_are_not_checked() {
        let c = Contract::from_yaml_str(STRICT).unwrap();
        assert_eq!(
            c.check_payload("robot/wrapped", &json!({})),
            PayloadCheck::Enveloped
        );
        assert_eq!(
            c.check_payload("robot/other", &json!({})),
            PayloadCheck::NoSchema
        );
    }

    #[test]
    fn envelopes_are_off_unless_declared() {
        let c = Contract::from_yaml_str("topics: [{key: a}]").unwrap();
        assert!(!c.effective_enveloped(&c.topics[0]));
    }
}
