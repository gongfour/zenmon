use base64::Engine;
use serde::{Serialize, Serializer};
use std::time::SystemTime;

/// Information about a discovered Zenoh key/topic.
#[derive(Debug, Clone, Serialize)]
pub struct TopicInfo {
    pub key_expr: String,
}

/// A received Zenoh message.
#[derive(Debug, Clone, Serialize)]
pub struct ZenohMessage {
    pub key_expr: String,
    pub payload: MessagePayload,
    /// Zenoh encoding string (e.g. "application/json"), for lossless replay.
    pub encoding: String,
    /// Original wire byte length of the payload (not the re-serialized view).
    pub payload_bytes: usize,
    pub timestamp: Option<String>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<MessagePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_bytes: Option<usize>,
}

/// A message payload captured **losslessly** as its original wire bytes.
///
/// Structured/text views (`as_json`, `as_str`) are computed on demand, so the
/// original bytes are always available for accurate size reporting (#14) and
/// round-trip capture/replay (#13). Binary payloads are never discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePayload {
    bytes: Vec<u8>,
}

impl MessagePayload {
    /// Capture the original wire bytes of a ZBytes payload.
    pub fn from_zbytes(zbytes: &zenoh::bytes::ZBytes) -> Self {
        Self {
            bytes: zbytes.to_bytes().into_owned(),
        }
    }

    /// Build from raw bytes (e.g. when loading a captured record).
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// Build from a JSON value (test/helper convenience).
    pub fn from_json(value: &serde_json::Value) -> Self {
        Self {
            bytes: value.to_string().into_bytes(),
        }
    }

    /// Original wire bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Original wire byte length.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The payload as UTF-8 text, if it is valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    /// The payload parsed as JSON, if it parses.
    pub fn as_json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.bytes).ok()
    }

    /// Structured view for JSON output: parsed JSON if it parses, else a string
    /// if valid UTF-8, else a base64 object `{"binary_base64":..,"bytes":N}`.
    pub fn to_view(&self) -> serde_json::Value {
        if let Some(v) = self.as_json() {
            return v;
        }
        if let Some(s) = self.as_str() {
            return serde_json::Value::String(s.to_string());
        }
        serde_json::json!({
            "binary_base64": base64::engine::general_purpose::STANDARD.encode(&self.bytes),
            "bytes": self.bytes.len(),
        })
    }

    /// Structured view capped at `max_bytes`. If the payload fits, this is the
    /// normal [`to_view`](Self::to_view). Otherwise it is a safe preview object
    /// that never splits a UTF-8 code point and reports the sizes:
    /// `{"payload_preview":..,"encoding":..,"truncated":true,"original_bytes":N,"returned_bytes":M}`.
    pub fn to_view_capped(&self, max_bytes: usize) -> serde_json::Value {
        if self.len() <= max_bytes {
            return self.to_view();
        }
        if let Some(s) = self.as_str() {
            // Truncate on a UTF-8 char boundary at or below max_bytes.
            let mut end = max_bytes.min(s.len());
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            serde_json::json!({
                "payload_preview": &s[..end],
                "encoding": "utf8",
                "truncated": true,
                "original_bytes": self.len(),
                "returned_bytes": end,
            })
        } else {
            let slice = &self.bytes[..max_bytes];
            serde_json::json!({
                "payload_preview": base64::engine::general_purpose::STANDARD.encode(slice),
                "encoding": "base64",
                "truncated": true,
                "original_bytes": self.len(),
                "returned_bytes": max_bytes,
            })
        }
    }

    /// Pretty (multi-line) rendering of a JSON payload; plain text or a
    /// `<N bytes>` placeholder otherwise.
    pub fn pretty(&self) -> String {
        if let Some(v) = self.as_json() {
            return serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
        }
        if let Some(s) = self.as_str() {
            return s.to_string();
        }
        format!("<{} bytes>", self.bytes.len())
    }
}

impl std::fmt::Display for MessagePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(v) = self.as_json() {
            write!(f, "{}", v)
        } else if let Some(s) = self.as_str() {
            write!(f, "{}", s)
        } else {
            write!(f, "<{} bytes>", self.bytes.len())
        }
    }
}

impl Serialize for MessagePayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_view().serialize(serializer)
    }
}

/// Information about a discovered Zenoh node/session.
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub zid: String,
    pub kind: String,
    pub locators: Vec<String>,
    pub metadata: Option<serde_json::Value>,
    pub sources: NodeSources,
    #[serde(skip)]
    pub admin_last_seen: Option<SystemTime>,
    #[serde(skip)]
    pub scout_last_seen: Option<SystemTime>,
}

/// Information about a Zenoh node discovered via scouting.
#[derive(Debug, Clone, Serialize)]
pub struct ScoutInfo {
    pub zid: String,
    pub whatami: String,
    pub locators: Vec<String>,
}

/// Scouting results grouped by multicast port (for port scan output).
#[derive(Debug, Clone, Serialize)]
pub struct PortScoutResult {
    pub port: u16,
    pub nodes: Vec<ScoutInfo>,
}

/// Detailed session information.
#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    pub zid: String,
    /// The session's configured connection mode ("client" or "peer"), not a
    /// guess from router presence.
    pub mode: String,
    /// Whether this session currently sees any router or peer. Useful as a
    /// health signal, interpreted per mode (a peer with none can still be fine).
    pub connected: bool,
    pub routers: Vec<String>,
    pub peers: Vec<String>,
}

bitflags::bitflags! {
    /// Which discovery source produced or last confirmed a node entry.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct NodeSources: u8 {
        const ADMIN = 0b01;
        const SCOUT = 0b10;
    }
}

impl serde::Serialize for NodeSources {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.bits())
    }
}

impl NodeInfo {
    /// A node is stale only if it is scout-only and older than `threshold`.
    pub fn is_scout_stale(&self, now: SystemTime, threshold: std::time::Duration) -> bool {
        if self.sources.contains(NodeSources::ADMIN) {
            return false;
        }
        self.scout_last_seen
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d > threshold)
            .unwrap_or(false)
    }
}

impl ScoutInfo {
    /// Convert a scout hello into a `NodeInfo` tagged as scout-derived.
    pub fn to_node_info(&self, now: SystemTime) -> NodeInfo {
        NodeInfo {
            zid: self.zid.clone(),
            kind: self.whatami.clone(),
            locators: self.locators.clone(),
            metadata: None,
            sources: NodeSources::SCOUT,
            admin_last_seen: None,
            scout_last_seen: Some(now),
        }
    }
}

/// A liveliness token discovered on the network.
#[derive(Debug, Clone, Serialize)]
pub struct LivelinessToken {
    pub key_expr: String,
    pub source_zid: Option<String>,
    pub alive: bool,
}

/// Event from a liveliness subscriber.
#[derive(Debug, Clone)]
pub enum LivelinessEvent {
    Join(LivelinessToken),
    Leave(LivelinessToken),
}

impl LivelinessToken {
    /// Extract a human-readable node name from the key expression.
    /// e.g. "hdx/forky001/node/action_executor_ec98a701" -> "action_executor"
    /// Falls back to the last path segment with hash stripped.
    pub fn node_name(&self) -> Option<String> {
        let last = self.key_expr.rsplit('/').next()?;
        // Strip trailing hex hash (pattern: _[0-9a-f]{6,})
        if let Some(pos) = last.rfind('_') {
            let suffix = &last[pos + 1..];
            if suffix.len() >= 6 && suffix.chars().all(|c| c.is_ascii_hexdigit()) {
                let name = &last[..pos];
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
        Some(last.to_string())
    }

    /// Extract the group/robot prefix from the key expression.
    /// e.g. "hdx/forky001/node/action_executor_ec98a701" -> "hdx/forky001"
    pub fn group_prefix(&self) -> Option<String> {
        let parts: Vec<&str> = self.key_expr.split('/').collect();
        if parts.len() >= 3 {
            Some(parts[..parts.len() - 2].join("/"))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn payload_preserves_original_bytes_and_len() {
        let p = MessagePayload::from_bytes(vec![0, 159, 146, 150]); // invalid UTF-8
        assert_eq!(p.len(), 4);
        assert_eq!(p.as_bytes(), &[0, 159, 146, 150]);
        assert!(p.as_str().is_none());
    }

    #[test]
    fn payload_json_view_roundtrips_object() {
        let p = MessagePayload::from_bytes(br#"{"a":1}"#.to_vec());
        assert_eq!(p.as_json().unwrap(), serde_json::json!({"a": 1}));
        // Serializes as the parsed JSON value.
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"a":1}"#
        );
    }

    #[test]
    fn payload_plain_text_view_is_string() {
        let p = MessagePayload::from_bytes(b"hello world".to_vec());
        assert_eq!(p.as_str(), Some("hello world"));
        assert_eq!(serde_json::to_string(&p).unwrap(), r#""hello world""#);
    }

    #[test]
    fn payload_binary_view_is_base64_object() {
        let p = MessagePayload::from_bytes(vec![0, 159, 146, 150]);
        let v = p.to_view();
        assert_eq!(v["bytes"], 4);
        assert!(v["binary_base64"].is_string());
        // base64 of [0,159,146,150]
        assert_eq!(v["binary_base64"], "AJ+Slg==");
    }

    #[test]
    fn capped_view_returns_full_when_within_limit() {
        let p = MessagePayload::from_bytes(b"hello".to_vec());
        assert_eq!(p.to_view_capped(100), serde_json::json!("hello"));
    }

    #[test]
    fn capped_view_truncates_text_on_char_boundary() {
        // "héllo": 'é' is 2 bytes (0xC3 0xA9) at bytes 1..3. Cap at 2 must not
        // split it — preview should be just "h".
        let p = MessagePayload::from_bytes("héllo".as_bytes().to_vec());
        let v = p.to_view_capped(2);
        assert_eq!(v["payload_preview"], "h");
        assert_eq!(v["returned_bytes"], 1);
        assert_eq!(v["truncated"], true);
        assert_eq!(v["encoding"], "utf8");
        assert_eq!(v["original_bytes"], "héllo".len());
    }

    #[test]
    fn capped_view_previews_binary_as_base64() {
        let p = MessagePayload::from_bytes(vec![0u8, 159, 146, 150, 1, 2]);
        let v = p.to_view_capped(4);
        assert_eq!(v["encoding"], "base64");
        assert_eq!(v["returned_bytes"], 4);
        assert_eq!(v["original_bytes"], 6);
        // base64 of first 4 bytes [0,159,146,150]
        assert_eq!(v["payload_preview"], "AJ+Slg==");
    }

    fn node_with(sources: NodeSources, scout_last_seen: Option<SystemTime>) -> NodeInfo {
        NodeInfo {
            zid: "z1".into(),
            kind: "peer".into(),
            locators: vec![],
            metadata: None,
            sources,
            admin_last_seen: None,
            scout_last_seen,
        }
    }

    #[test]
    fn stale_false_when_admin_flag_set() {
        let now = SystemTime::now();
        let old = now - Duration::from_secs(600);
        let n = node_with(NodeSources::ADMIN | NodeSources::SCOUT, Some(old));
        assert!(!n.is_scout_stale(now, Duration::from_secs(30)));
    }

    #[test]
    fn stale_false_when_scout_recent() {
        let now = SystemTime::now();
        let recent = now - Duration::from_secs(5);
        let n = node_with(NodeSources::SCOUT, Some(recent));
        assert!(!n.is_scout_stale(now, Duration::from_secs(30)));
    }

    #[test]
    fn stale_true_when_scout_exceeds_threshold() {
        let now = SystemTime::now();
        let old = now - Duration::from_secs(120);
        let n = node_with(NodeSources::SCOUT, Some(old));
        assert!(n.is_scout_stale(now, Duration::from_secs(30)));
    }

    #[test]
    fn stale_false_when_scout_last_seen_absent() {
        let n = node_with(NodeSources::SCOUT, None);
        assert!(!n.is_scout_stale(SystemTime::now(), Duration::from_secs(30)));
    }
}
