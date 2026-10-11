//! Versioned NDJSON records for `capture` / `replay`.
//!
//! Each captured message is one JSON line. Payloads and attachments are stored
//! base64-encoded so binary round-trips losslessly, alongside the Zenoh
//! encoding, the source timestamp, and a `received_offset_ms` (milliseconds
//! since capture start) that drives replay timing without depending on the
//! source clock. `schema_version` guards forward compatibility.

use crate::error::ZenmonError;
use crate::trace::{enforce_retention, CaptureFileWriter, SegmentWriter};
use crate::types::ZenohMessage;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// Current record schema version.
pub const SCHEMA_VERSION: u32 = 2;
const SUPPORTED_VERSIONS: &[u32] = &[1, 2];

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
    /// Receiver wall-clock time this record was captured, RFC3339 (UTC).
    /// Present from schema v2 on; absent in v1 files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_at: Option<String>,
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

/// Decode a base64 payload string (used by the trace reader).
pub fn b64_decode_public(s: &str) -> Result<Vec<u8>, ZenmonError> {
    b64_decode(s, "payload")
}

impl CaptureRecord {
    /// Build a record from a received message, its offset since capture start,
    /// and the receiver wall-clock time it was captured.
    pub fn from_message(msg: &ZenohMessage, offset: Duration, received_at: SystemTime) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            key_expr: msg.key_expr.clone(),
            payload_base64: b64_encode(msg.payload.as_bytes()),
            encoding: msg.encoding.clone(),
            attachment_base64: msg.attachment.as_ref().map(|a| b64_encode(a.as_bytes())),
            source_timestamp: msg.timestamp.clone(),
            received_offset_ms: offset.as_millis() as u64,
            kind: msg.kind.clone(),
            received_at: Some(humantime::format_rfc3339_seconds(received_at).to_string()),
        }
    }

    /// Parse one NDJSON line, reporting the 1-based line number on failure.
    pub fn parse_line(line: &str, line_no: usize) -> Result<Self, ZenmonError> {
        let rec: CaptureRecord = serde_json::from_str(line).map_err(|e| {
            ZenmonError::invalid_input(format!("corrupt record at line {}: {}", line_no, e))
        })?;
        if !SUPPORTED_VERSIONS.contains(&rec.schema_version) {
            return Err(ZenmonError::invalid_input(format!(
                "unsupported schema_version {} at line {} (supported: {:?})",
                rec.schema_version, line_no, SUPPORTED_VERSIONS
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

/// Key expressions a capture drops before writing — e.g. bulky point clouds
/// out of an always-on recorder. The subscription itself stays as wide as its
/// key expression (Zenoh has no "all except" form); exclusion is applied per
/// received message, so callers count what they drop and report it rather
/// than letting excluded traffic vanish unnoticed.
#[derive(Debug, Clone, Default)]
pub struct CaptureExclude {
    exprs: Vec<zenoh::key_expr::OwnedKeyExpr>,
}

impl CaptureExclude {
    /// Validate every expression up front, so a typo fails the capture at
    /// start instead of silently excluding nothing.
    pub fn parse<S: AsRef<str>>(exprs: &[S]) -> Result<Self, ZenmonError> {
        let exprs = exprs
            .iter()
            .map(|s| {
                let s = s.as_ref().trim();
                zenoh::key_expr::OwnedKeyExpr::autocanonize(s.to_string()).map_err(|e| {
                    ZenmonError::invalid_input(format!(
                        "invalid exclude key expression '{}': {}",
                        s, e
                    ))
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { exprs })
    }

    pub fn is_empty(&self) -> bool {
        self.exprs.is_empty()
    }

    /// True if `key` intersects any exclude expression. A key that is not a
    /// valid key expression is never excluded (it is recorded as-is).
    pub fn excludes(&self, key: &str) -> bool {
        if self.exprs.is_empty() {
            return false;
        }
        match zenoh::key_expr::keyexpr::new(key) {
            Ok(k) => self.exprs.iter().any(|e| e.intersects(k)),
            Err(_) => false,
        }
    }
}

/// Retention limits for a rotating store.
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    pub max_total_size: u64,
    pub max_age: Duration,
}

/// Where a [`Recorder`] writes: one file (pairs with `replay`) or a rotating
/// segment store (pairs with `trace`).
pub enum CaptureSink {
    File(CaptureFileWriter),
    Store {
        writer: SegmentWriter,
        dir: PathBuf,
        retention: Retention,
    },
}

/// How often retention runs even when no segment rotated, so age-based
/// pruning still happens on a store that rotates rarely.
pub const RETENTION_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// What [`Recorder::record`] did with one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// Dropped by the exclude list; counted, not written.
    Excluded,
    /// Written; `bytes` is the uncompressed NDJSON line plus its newline.
    Written { bytes: u64 },
}

/// The per-message capture pipeline shared by `zenmon capture` and
/// zenmon-tray: exclude → record → write → retention, with counters.
///
/// Session handling and the stop condition stay with the caller (Ctrl+C and
/// `--count` for the CLI, a toggle for the tray); everything that decides what
/// lands on disk is here, once.
///
/// Retention runs when a segment rotates — the only time the total size can
/// newly exceed its cap, since the active segment is never pruned — and at
/// most every [`RETENTION_CHECK_INTERVAL`] otherwise, for age. Never per
/// message: each run lists the directory and stats every segment.
pub struct Recorder {
    exclude: CaptureExclude,
    sink: CaptureSink,
    start: Instant,
    retention_checked: Instant,
    segments_seen: u64,
    written: u64,
    excluded: u64,
    bytes: u64,
}

impl Recorder {
    pub fn new(exclude: CaptureExclude, sink: CaptureSink) -> Self {
        Self {
            exclude,
            sink,
            start: Instant::now(),
            retention_checked: Instant::now(),
            segments_seen: 0,
            written: 0,
            excluded: 0,
            bytes: 0,
        }
    }

    /// Handle one received message. A write error leaves the counters as they
    /// were; a retention error is reported after the message was written.
    pub fn record(&mut self, msg: &ZenohMessage, now: SystemTime) -> Result<Recorded, ZenmonError> {
        if self.exclude.excludes(&msg.key_expr) {
            self.excluded += 1;
            return Ok(Recorded::Excluded);
        }
        let record = CaptureRecord::from_message(msg, self.start.elapsed(), now);
        let line = serde_json::to_string(&record)
            .map_err(|e| ZenmonError::internal(format!("cannot serialize record: {e}")))?;
        match &mut self.sink {
            CaptureSink::File(w) => w.write_line(&line, now)?,
            CaptureSink::Store { writer, .. } => writer.write_line(&line, now)?,
        }
        let bytes = line.len() as u64 + 1;
        self.written += 1;
        self.bytes += bytes;
        self.maybe_enforce_retention(now)?;
        Ok(Recorded::Written { bytes })
    }

    fn maybe_enforce_retention(&mut self, now: SystemTime) -> Result<(), ZenmonError> {
        let CaptureSink::Store {
            writer,
            dir,
            retention,
        } = &self.sink
        else {
            return Ok(());
        };
        let rotated = writer.segments_opened() != self.segments_seen;
        if !rotated && self.retention_checked.elapsed() < RETENTION_CHECK_INTERVAL {
            return Ok(());
        }
        self.segments_seen = writer.segments_opened();
        self.retention_checked = Instant::now();
        enforce_retention(
            dir,
            Some(retention.max_total_size),
            Some(retention.max_age),
            now,
        )?;
        Ok(())
    }

    /// Close the sink (ending a zstd frame) and run a final retention pass.
    /// Safe to call once on any exit path.
    pub fn finish(&mut self) -> Result<(), ZenmonError> {
        match &mut self.sink {
            CaptureSink::File(w) => w.finish(),
            CaptureSink::Store {
                writer,
                dir,
                retention,
            } => {
                let closed = writer.close();
                let pruned = enforce_retention(
                    dir,
                    Some(retention.max_total_size),
                    Some(retention.max_age),
                    SystemTime::now(),
                );
                closed.and(pruned.map(|_| ()))
            }
        }
    }

    pub fn excludes_anything(&self) -> bool {
        !self.exclude.is_empty()
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    pub fn excluded(&self) -> u64 {
        self.excluded
    }

    /// Uncompressed NDJSON bytes written; with zstd the disk use is smaller.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessagePayload;

    #[test]
    fn exclude_matches_by_intersection() {
        let ex = CaptureExclude::parse(&["**/topic/sensor/pcd/**", "a/b"]).unwrap();
        assert!(ex.excludes("demo/robot1/topic/sensor/pcd/rear"));
        assert!(ex.excludes("a/b"));
        assert!(!ex.excludes("demo/robot1/topic/sensor/og3d/fused"));
        assert!(!ex.excludes("a/b/c"));
    }

    #[test]
    fn exclude_empty_excludes_nothing() {
        let ex = CaptureExclude::parse::<&str>(&[]).unwrap();
        assert!(ex.is_empty());
        assert!(!ex.excludes("anything"));
    }

    #[test]
    fn exclude_rejects_invalid_expression() {
        assert!(CaptureExclude::parse(&[""]).is_err());
        assert!(CaptureExclude::parse(&["a/#/b"]).is_err());
    }

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
        let rec =
            CaptureRecord::from_message(&m, Duration::from_millis(1500), std::time::UNIX_EPOCH);
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
        let rec = CaptureRecord::from_message(&m, Duration::ZERO, std::time::UNIX_EPOCH);
        let line = serde_json::to_string(&rec).unwrap();
        let parsed = CaptureRecord::parse_line(&line, 1).unwrap();
        assert_eq!(parsed.payload_bytes().unwrap(), vec![0, 159, 146, 150]);
        assert_eq!(parsed.attachment_bytes().unwrap(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn record_carries_schema_version() {
        let m = msg("a/b", b"x".to_vec(), None);
        let rec = CaptureRecord::from_message(&m, Duration::ZERO, std::time::UNIX_EPOCH);
        assert_eq!(rec.schema_version, SCHEMA_VERSION);
        let line = serde_json::to_string(&rec).unwrap();
        assert!(line.contains("\"schema_version\":2"));
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

    #[test]
    fn from_message_populates_received_at_as_rfc3339() {
        let m = msg("a/b", b"{}".to_vec(), None);
        let t = std::time::UNIX_EPOCH + Duration::from_secs(1_752_668_096);
        let rec = CaptureRecord::from_message(&m, Duration::from_millis(10), t);
        assert_eq!(rec.schema_version, 2);
        let got = rec.received_at.as_deref().unwrap();
        assert_eq!(got, "2025-07-16T12:14:56Z"); // humantime rfc3339 of that epoch second
    }

    #[test]
    fn parse_accepts_v1_without_received_at() {
        // A legacy v1 line (no received_at) must still parse.
        let line = r#"{"schema_version":1,"key_expr":"a","payload_base64":"","encoding":"","received_offset_ms":0,"kind":"PUT"}"#;
        let rec = CaptureRecord::parse_line(line, 1).unwrap();
        assert_eq!(rec.schema_version, 1);
        assert!(rec.received_at.is_none());
    }

    #[test]
    fn parse_accepts_v2_and_rejects_v3() {
        let m = msg("a/b", b"x".to_vec(), None);
        let t = std::time::UNIX_EPOCH + Duration::from_secs(1_752_668_096);
        let line =
            serde_json::to_string(&CaptureRecord::from_message(&m, Duration::ZERO, t)).unwrap();
        assert!(CaptureRecord::parse_line(&line, 1).is_ok());
        let v3 = line.replace("\"schema_version\":2", "\"schema_version\":3");
        assert!(CaptureRecord::parse_line(&v3, 5)
            .unwrap_err()
            .message
            .contains("schema_version"));
    }

    fn recorder_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zenmon-recorder-test-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recorder_counts_excluded_without_writing_them() {
        let dir = recorder_dir("exclude");
        let path = dir.join("out.ndjson");
        let file = CaptureFileWriter::create(&path, crate::trace::Compression::None).unwrap();
        let exclude = CaptureExclude::parse(&["**/pcd/**"]).unwrap();
        let mut rec = Recorder::new(exclude, CaptureSink::File(file));

        let now = SystemTime::now();
        assert!(matches!(
            rec.record(&msg("r/pose", b"{}".to_vec(), None), now)
                .unwrap(),
            Recorded::Written { .. }
        ));
        assert_eq!(
            rec.record(&msg("r/pcd/front", b"{}".to_vec(), None), now)
                .unwrap(),
            Recorded::Excluded
        );
        rec.finish().unwrap();

        assert_eq!((rec.written(), rec.excluded()), (1, 1));
        let lines: Vec<_> = crate::trace::open_capture_lines(&path)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(rec.bytes(), lines[0].len() as u64 + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Retention must run while recording, on rotation — not only at finish,
    /// and not per message (the tray used to wait a minute, the CLI ran it on
    /// every write).
    #[test]
    fn recorder_prunes_on_rotation_while_recording() {
        let dir = recorder_dir("rotate");
        // rotate_size 1: every write starts a new segment.
        let writer = SegmentWriter::open(dir.clone(), 1, Duration::from_secs(3600)).unwrap();
        let line_len = 200;
        let sink = CaptureSink::Store {
            writer,
            dir: dir.clone(),
            retention: Retention {
                max_total_size: 3 * line_len,
                max_age: Duration::from_secs(3600),
            },
        };
        let mut rec = Recorder::new(CaptureExclude::default(), sink);

        for _ in 0..12 {
            rec.record(&msg("r/pose", vec![b'x'; 100], None), SystemTime::now())
                .unwrap();
        }
        let segments = crate::trace::discover_segments(&dir).unwrap().len();
        assert!(segments <= 4, "pruned while recording, found {segments}");
        assert_eq!(rec.written(), 12);

        rec.finish().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
