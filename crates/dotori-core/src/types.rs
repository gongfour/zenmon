use serde::Serialize;
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
    pub timestamp: Option<String>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<MessagePayload>,
}

/// Payload of a message — either parsed JSON or raw bytes info.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MessagePayload {
    Json(serde_json::Value),
    Raw { bytes_len: usize },
}

impl std::fmt::Display for MessagePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessagePayload::Json(v) => write!(f, "{}", v),
            MessagePayload::Raw { bytes_len } => write!(f, "<{} bytes>", bytes_len),
        }
    }
}

impl MessagePayload {
    /// Parse ZBytes into MessagePayload: try JSON first, then string, then raw bytes.
    pub fn from_zbytes(zbytes: &zenoh::bytes::ZBytes) -> Self {
        // Try string first (most reliable for cross-language payloads)
        match zbytes.try_to_string() {
            Ok(s) => {
                // Try parsing the string as JSON
                match serde_json::from_str::<serde_json::Value>(&s) {
                    Ok(json) => MessagePayload::Json(json),
                    Err(_) => MessagePayload::Json(serde_json::Value::String(s.into_owned())),
                }
            }
            Err(_) => {
                // Not valid UTF-8 — try raw bytes as JSON, fallback to raw
                let bytes = zbytes.to_bytes();
                match serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(json) => MessagePayload::Json(json),
                    Err(_) => MessagePayload::Raw {
                        bytes_len: bytes.len(),
                    },
                }
            }
        }
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

/// Detailed session information.
#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    pub zid: String,
    pub mode: String,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

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
