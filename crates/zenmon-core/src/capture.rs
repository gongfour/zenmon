//! Versioned NDJSON records for `capture` / `replay`.
//!
//! Each captured message is one JSON line. Payloads and attachments are stored
//! base64-encoded so binary round-trips losslessly, alongside the Zenoh
//! encoding, the source timestamp, and a `received_offset_ms` (milliseconds
//! since capture start) that drives replay timing without depending on the
//! source clock. `schema_version` guards forward compatibility.

use crate::error::ZenmonError;
use crate::types::ZenohMessage;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Current record schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// One captured message, serialized as a single NDJSON line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub schema_version: u32,
    pub key_expr: String,
    /// base64 of the original payload wire bytes.
    pub payload_base64: String,
    pub encoding: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_timestamp: Option<String>,
    /// Milliseconds since capture start (monotonic), for replay timing.
    pub received_offset_ms: u64,
    pub kind: String,
}

fn b64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64_decode(s: &str, field: &str) -> Result<Vec<u8>, ZenmonError> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| ZenmonError::invalid_input(format!("invalid base64 in {}: {}", field, e)))
}

impl CaptureRecord {
    /// Build a record from a received message and its offset since capture start.
    pub fn from_message(msg: &ZenohMessage, offset: Duration) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            key_expr: msg.key_expr.clone(),
            payload_base64: b64_encode(msg.payload.as_bytes()),
            encoding: msg.encoding.clone(),
            attachment_base64: msg.attachment.as_ref().map(|a| b64_encode(a.as_bytes())),
            source_timestamp: msg.timestamp.clone(),
            received_offset_ms: offset.as_millis() as u64,
            kind: msg.kind.clone(),
        }
    }

    /// Parse one NDJSON line, reporting the 1-based line number on failure.
    pub fn parse_line(line: &str, line_no: usize) -> Result<Self, ZenmonError> {
        let rec: CaptureRecord = serde_json::from_str(line).map_err(|e| {
            ZenmonError::invalid_input(format!("corrupt record at line {}: {}", line_no, e))
        })?;
        if rec.schema_version != SCHEMA_VERSION {
            return Err(ZenmonError::invalid_input(format!(
                "unsupported schema_version {} at line {} (expected {})",
                rec.schema_version, line_no, SCHEMA_VERSION
            )));
        }
        Ok(rec)
    }

    /// Decode the original payload bytes.
    pub fn payload_bytes(&self) -> Result<Vec<u8>, ZenmonError> {
        b64_decode(&self.payload_base64, "payload")
    }

    /// Decode the original attachment bytes, if any.
    pub fn attachment_bytes(&self) -> Result<Option<Vec<u8>>, ZenmonError> {
        match &self.attachment_base64 {
            Some(s) => Ok(Some(b64_decode(s, "attachment")?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessagePayload;

    fn msg(key: &str, payload: Vec<u8>, attachment: Option<Vec<u8>>) -> ZenohMessage {
        let payload = MessagePayload::from_bytes(payload);
        let attachment = attachment.map(MessagePayload::from_bytes);
        ZenohMessage {
            key_expr: key.to_string(),
            payload_bytes: payload.len(),
            attachment_bytes: attachment.as_ref().map(|a| a.len()),
            payload,
            encoding: "application/json".to_string(),
            timestamp: Some("ts".to_string()),
            kind: "PUT".to_string(),
            attachment,
        }
    }

    #[test]
    fn roundtrips_text_payload() {
        let m = msg("a/b", b"{\"x\":1}".to_vec(), None);
        let rec = CaptureRecord::from_message(&m, Duration::from_millis(1500));
        let line = serde_json::to_string(&rec).unwrap();
        let parsed = CaptureRecord::parse_line(&line, 1).unwrap();
        assert_eq!(parsed, rec);
        assert_eq!(parsed.payload_bytes().unwrap(), b"{\"x\":1}");
        assert_eq!(parsed.received_offset_ms, 1500);
        assert_eq!(parsed.encoding, "application/json");
    }

    #[test]
    fn roundtrips_binary_payload_and_attachment() {
        let m = msg("a/b", vec![0, 159, 146, 150], Some(vec![1, 2, 3]));
        let rec = CaptureRecord::from_message(&m, Duration::ZERO);
        let line = serde_json::to_string(&rec).unwrap();
        let parsed = CaptureRecord::parse_line(&line, 1).unwrap();
        assert_eq!(parsed.payload_bytes().unwrap(), vec![0, 159, 146, 150]);
        assert_eq!(parsed.attachment_bytes().unwrap(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn record_carries_schema_version() {
        let m = msg("a/b", b"x".to_vec(), None);
        let rec = CaptureRecord::from_message(&m, Duration::ZERO);
        assert_eq!(rec.schema_version, SCHEMA_VERSION);
        let line = serde_json::to_string(&rec).unwrap();
        assert!(line.contains("\"schema_version\":1"));
    }

    #[test]
    fn corrupt_line_reports_line_number() {
        let err = CaptureRecord::parse_line("{not json", 7).unwrap_err();
        assert!(err.message.contains("line 7"), "message: {}", err.message);
    }

    #[test]
    fn unknown_schema_version_rejected() {
        let line = r#"{"schema_version":999,"key_expr":"a","payload_base64":"","encoding":"","received_offset_ms":0,"kind":"PUT"}"#;
        let err = CaptureRecord::parse_line(line, 3).unwrap_err();
        assert!(err.message.contains("schema_version"));
    }
}
