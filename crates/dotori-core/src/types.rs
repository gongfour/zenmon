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

/// Information about a discovered Zenoh node/session.
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub zid: String,
    pub kind: String,
    pub locators: Vec<String>,
    pub metadata: Option<serde_json::Value>,
    pub last_seen: Option<SystemTime>,
}
