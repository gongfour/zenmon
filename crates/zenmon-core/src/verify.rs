//! Strict, streaming verification. This does not infer network delivery or repair files.
use crate::{capture::CaptureRecord, error::ZenmonError, trace};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct Issue {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub message: String,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub valid: bool,
    pub files: usize,
    pub records: u64,
    pub issue_count: u64,
    /// At most 100 diagnostics; issue_count always includes all failures.
    pub issues: Vec<Issue>,
}
impl Report {
    fn issue(&mut self, path: &Path, line: Option<usize>, message: impl ToString) {
        self.valid = false;
        self.issue_count += 1;
        if self.issues.len() < 100 {
            self.issues.push(Issue {
                path: path.into(),
                line,
                message: message.to_string(),
            });
        }
    }
}

pub fn capture_paths(path: &Path) -> Result<Vec<PathBuf>, ZenmonError> {
    if path.is_dir() {
        Ok(trace::discover_segments(path)?
            .into_iter()
            .map(|s| s.path)
            .collect())
    } else if path.is_file() {
        Ok(vec![path.into()])
    } else {
        Err(ZenmonError::not_found(format!(
            "capture not found: {}",
            path.display()
        )))
    }
}

pub fn validate_record(record: &CaptureRecord) -> Result<(), ZenmonError> {
    let key = zenoh::key_expr::keyexpr::new(&record.key_expr)
        .map_err(|e| ZenmonError::invalid_input(format!("invalid record key: {e}")))?;
    if key.is_wild() {
        return Err(ZenmonError::invalid_input("record key contains a wildcard"));
    }
    record.payload_bytes()?;
    record.attachment_bytes()?;
    if !matches!(record.kind.as_str(), "PUT" | "DELETE") {
        return Err(ZenmonError::invalid_input(
            "record kind must be PUT or DELETE",
        ));
    }
    match &record.received_at {
        Some(t) => {
            humantime::parse_rfc3339(t)
                .map_err(|e| ZenmonError::invalid_input(format!("invalid received_at: {e}")))?;
        }
        None if record.schema_version >= 2 => {
            return Err(ZenmonError::invalid_input("v2 record missing received_at"))
        }
        None => {}
    }
    Ok(())
}

pub fn verify(path: &Path) -> Result<Report, ZenmonError> {
    let paths = capture_paths(path)?;
    let mut report = Report {
        valid: true,
        files: paths.len(),
        records: 0,
        issue_count: 0,
        issues: vec![],
    };
    if paths.is_empty() {
        report.issue(path, None, "no capture segments found");
    }
    for path in paths {
        let lines = match trace::open_capture_lines(&path) {
            Ok(lines) => lines.strict(),
            Err(e) => {
                report.issue(&path, None, e);
                continue;
            }
        };
        for (i, line) in lines.enumerate() {
            let result = line
                .and_then(|line| CaptureRecord::parse_line(&line, i + 1))
                .and_then(|record| validate_record(&record));
            match result {
                Ok(()) => report.records += 1,
                Err(e) => report.issue(&path, Some(i + 1), e),
            }
        }
    }
    if report.records == 0 && report.issue_count == 0 {
        report.issue(path, None, "no records found");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> CaptureRecord {
        CaptureRecord {
            schema_version: 2,
            key_expr: "demo/value".into(),
            payload_base64: "AP8=".into(),
            encoding: "application/octet-stream".into(),
            attachment_base64: Some("/wA=".into()),
            source_timestamp: None,
            received_at: Some("2026-10-14T00:00:00Z".into()),
            received_offset_ms: 0,
            kind: "PUT".into(),
        }
    }
    fn temp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "zenmon-verify-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    #[test]
    fn checks_binary_fields_keys_and_timestamps() {
        let mut r = record();
        validate_record(&r).unwrap();
        r.attachment_base64 = Some("%%%".into());
        assert!(validate_record(&r).is_err());
        r = record();
        r.payload_base64 = "%%%".into();
        assert!(validate_record(&r).is_err());
        r = record();
        r.key_expr = "demo/*".into();
        assert!(validate_record(&r).is_err());
        r = record();
        r.received_at = None;
        assert!(validate_record(&r).is_err());
        r.schema_version = 1;
        validate_record(&r).unwrap();
        r.kind = "UNKNOWN".into();
        assert!(validate_record(&r).is_err());
    }
    #[test]
    fn reports_each_bad_line_and_continues() {
        let dir = temp();
        let path = dir.join("sample.ndjson");
        let good = serde_json::to_string(&record()).unwrap();
        std::fs::write(&path, format!("{good}\nBROKEN\n{good}\n{{")).unwrap();
        let report = verify(&path).unwrap();
        assert!(!report.valid);
        assert_eq!(report.records, 2);
        assert_eq!(report.issue_count, 2);
        assert_eq!(report.issues[0].line, Some(2));
        assert_eq!(report.issues[1].line, Some(4));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unfinished_compressed_frame_is_not_a_successful_verification() {
        let dir = temp();
        let path = dir.join("sample.ndjson.zst");
        let line = format!("{}\n", serde_json::to_string(&record()).unwrap());
        let mut compressed = zstd::stream::encode_all(line.as_bytes(), 0).unwrap();
        std::fs::write(&path, &compressed).unwrap();
        assert!(verify(&path).unwrap().valid);
        compressed.truncate(compressed.len() - 2);
        std::fs::write(&path, compressed).unwrap();
        assert!(!verify(&path).unwrap().valid);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn empty_store_fails_and_issue_memory_is_bounded() {
        let dir = temp();
        assert!(!verify(&dir).unwrap().valid);
        let path = dir.join("bad.ndjson");
        std::fs::write(&path, "broken\n".repeat(250)).unwrap();
        let report = verify(&path).unwrap();
        assert_eq!(report.issue_count, 250);
        assert_eq!(report.issues.len(), 100);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
