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
    pub last_seen: Option<SystemTime>,
}
