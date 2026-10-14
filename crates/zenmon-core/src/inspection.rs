//! Strict offline selection. Time windows are start-inclusive, end-exclusive.
use crate::{capture::CaptureRecord, trace, verify, Result, ZenmonError};
use std::{
    io::Write,
    path::Path,
    time::{Duration, SystemTime},
};

pub const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
enum Bound {
    Wall(SystemTime),
    Offset(SystemTime),
}
impl Bound {
    fn parse(value: &str, now: SystemTime) -> Result<Self> {
        if let Some(value) = value.strip_prefix('+') {
            let duration = parse_offset(value)?;
            return SystemTime::UNIX_EPOCH
                .checked_add(duration)
                .map(Self::Offset)
                .ok_or_else(|| ZenmonError::invalid_input("offset out of range"));
        }
        trace::parse_time_bound(value, now).map(Self::Wall)
    }
    fn value(&self) -> SystemTime {
        match self {
            Self::Wall(t) => *t,
            Self::Offset(t) => *t,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Selection {
    key: String,
    since: Option<Bound>,
    until: Option<Bound>,
}
impl Selection {
    pub fn new(key: &str, since: Option<&str>, until: Option<&str>) -> Result<Self> {
        trace::key_matches(key, "validation")?;
        let now = SystemTime::now();
        let since = since.map(|s| Bound::parse(s, now)).transpose()?;
        let until = until.map(|s| Bound::parse(s, now)).transpose()?;
        if let (Some(a), Some(b)) = (&since, &until) {
            if std::mem::discriminant(a) != std::mem::discriminant(b) || a.value() >= b.value() {
                return Err(ZenmonError::invalid_input(
                    "time bounds must use the same clock and since must precede until",
                ));
            }
        }
        Ok(Self {
            key: key.into(),
            since,
            until,
        })
    }
    pub fn matches(&self, record: &CaptureRecord) -> Result<bool> {
        if !trace::key_matches(&self.key, &record.key_expr)? {
            return Ok(false);
        }
        for bound in [&self.since, &self.until].into_iter().flatten() {
            if matches!(bound, Bound::Offset(_)) != record.received_at.is_none() {
                return Err(ZenmonError::invalid_input("v1 time bounds require +offset (e.g. +10s); v2 bounds require RFC3339 or an age (e.g. 10m)"));
            }
        }
        let t = record_time(record)?;
        Ok(!self.since.as_ref().is_some_and(|b| t < b.value())
            && !self.until.as_ref().is_some_and(|b| t >= b.value()))
    }
}
impl Default for Selection {
    fn default() -> Self {
        Self {
            key: "**".into(),
            since: None,
            until: None,
        }
    }
}
pub fn record_time(record: &CaptureRecord) -> Result<SystemTime> {
    match &record.received_at {
        Some(t) => trace::parse_time_bound(t, SystemTime::now()),
        None => SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(record.received_offset_ms))
            .ok_or_else(|| ZenmonError::invalid_input("timestamp out of range")),
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub files: usize,
    pub total_files: usize,
    pub scanned: u64,
    pub matched: u64,
    pub decoded_bytes: u64,
}
/// Callback runs before every read, including records rejected by the filter.
/// Returning an error cancels the scan and drops any temporary output.
pub fn scan(
    path: &Path,
    selection: &Selection,
    mut progress: impl FnMut(Progress) -> Result<()>,
    mut visit: impl FnMut(&CaptureRecord, &str) -> Result<()>,
) -> Result<Progress> {
    scan_observed(path, selection, &mut progress, &mut visit, |_| Ok(()))
}

/// Observe every validated source line, including records excluded by selection.
/// Used to fingerprint an offline investigation without a second pass over input.
pub fn scan_observed(
    path: &Path,
    selection: &Selection,
    mut progress: impl FnMut(Progress) -> Result<()>,
    mut visit: impl FnMut(&CaptureRecord, &str) -> Result<()>,
    mut observe: impl FnMut(&str) -> Result<()>,
) -> Result<Progress> {
    let paths = verify::capture_paths(path)?;
    let mut p = Progress {
        total_files: paths.len(),
        ..Progress::default()
    };
    for path in paths {
        let mut lines = trace::open_capture_lines(&path)?
            .strict()
            .bounded(MAX_RECORD_BYTES);
        let mut line_number = 0;
        loop {
            progress(p)?;
            let Some(line) = lines.next() else { break };
            line_number += 1;
            let line = line.map_err(|e| {
                ZenmonError::invalid_input(format!("{}:{line_number}: {e}", path.display()))
            })?;
            let record = CaptureRecord::parse_line(&line, line_number)
                .and_then(|r| {
                    verify::validate_record(&r)?;
                    Ok(r)
                })
                .map_err(|e| {
                    ZenmonError::invalid_input(format!("{}:{line_number}: {e}", path.display()))
                })?;
            observe(&line)?;
            p.scanned += 1;
            p.decoded_bytes += line.len() as u64;
            if selection.matches(&record)? {
                visit(&record, &line)?;
                p.matched += 1;
            }
        }
        p.files += 1;
    }
    progress(p)?;
    Ok(p)
}

/// Write in the destination directory, then publish atomically without replacement.
/// No summary trailer; stored records and binary fields are left intact.
pub fn atomic_export(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> Result<u64>,
) -> Result<u64> {
    if path.exists() {
        return Err(ZenmonError::invalid_input(
            "output already exists; choose a new path",
        ));
    }
    if path.extension().is_some_and(|e| e == "zst") {
        return Err(ZenmonError::invalid_input(
            "export writes plain NDJSON; use a .ndjson output path",
        ));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    let count = write(temp.as_file_mut())?;
    if count == 0 {
        return Err(ZenmonError::invalid_input(
            "no matching records; no output created",
        ));
    }
    temp.as_file().sync_all().map_err(io_error)?;
    temp.persist_noclobber(path)
        .map_err(|e| io_error(e.error))?;
    Ok(count)
}
pub fn io_error(e: std::io::Error) -> ZenmonError {
    ZenmonError::internal(e.to_string())
}
pub fn export(
    path: &Path,
    output: &Path,
    selection: &Selection,
    progress: impl FnMut(Progress) -> Result<()>,
) -> Result<u64> {
    atomic_export(output, |file| {
        let result = scan(path, selection, progress, |_, line| {
            writeln!(file, "{line}").map_err(io_error)
        })?;
        Ok(result.matched)
    })
}

pub fn parse_offset(value: &str) -> Result<Duration> {
    humantime::parse_duration(value).map_err(|e| ZenmonError::invalid_input(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(n: u64) -> CaptureRecord {
        CaptureRecord {
            schema_version: 2,
            key_expr: "sensor/state".into(),
            payload_base64: "AP8=".into(),
            attachment_base64: Some("/wA=".into()),
            encoding: "zenoh/bytes".into(),
            kind: "PUT".into(),
            received_at: Some(format!("2026-08-06T00:00:{n:02}Z")),
            received_offset_ms: n * 1000,
            source_timestamp: Some("preserved".into()),
        }
    }
    fn fixture(dir: &Path, records: &[CaptureRecord]) -> std::path::PathBuf {
        let path = dir.join("source.ndjson");
        let text: String = records
            .iter()
            .map(|r| format!("{}\n", serde_json::to_string(r).unwrap()))
            .collect();
        std::fs::write(&path, text).unwrap();
        path
    }
    #[test]
    fn windows_are_half_open_and_reject_ambiguous_clocks() {
        let filter = Selection::new(
            "sensor/**",
            Some("2026-08-06T00:00:01Z"),
            Some("2026-08-06T00:00:03Z"),
        )
        .unwrap();
        assert!(!filter.matches(&record(0)).unwrap());
        assert!(filter.matches(&record(1)).unwrap());
        assert!(filter.matches(&record(2)).unwrap());
        assert!(!filter.matches(&record(3)).unwrap());
        let mut legacy = record(2);
        legacy.schema_version = 1;
        legacy.received_at = None;
        assert!(filter.matches(&legacy).is_err());
        let offset = Selection::new("**", Some("+1s"), Some("+3s")).unwrap();
        assert!(offset.matches(&legacy).unwrap());
        assert!(offset.matches(&record(2)).is_err());
        assert!(Selection::new("**", Some("+3s"), Some("+1s")).is_err());
        assert!(Selection::new("**", Some("+1s"), Some("2026-08-06T00:00:03Z")).is_err());
        assert!(Selection::new("bad//key", None, None).is_err());
    }
    #[test]
    fn export_is_lossless_standalone_and_no_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let input = fixture(dir.path(), &[record(0), record(1), record(2)]);
        let output = dir.path().join("export.ndjson");
        let filter = Selection::new("sensor/**", Some("2026-08-06T00:00:01Z"), None).unwrap();
        assert_eq!(export(&input, &output, &filter, |_| Ok(())).unwrap(), 2);
        let before = std::fs::read(&output).unwrap();
        let report = verify::verify(&output).unwrap();
        assert!(report.valid);
        assert_eq!(report.records, 2);
        let r = CaptureRecord::parse_line(
            std::str::from_utf8(&before)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
            1,
        )
        .unwrap();
        assert_eq!(r.payload_bytes().unwrap(), [0, 255]);
        assert_eq!(r.attachment_bytes().unwrap().unwrap(), [255, 0]);
        assert_eq!(r.source_timestamp.as_deref(), Some("preserved"));
        assert_eq!(r.received_offset_ms, 1000);
        assert!(export(&input, &output, &filter, |_| Ok(())).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), before);
        assert!(export(&input, &input, &filter, |_| Ok(())).is_err());
    }
    #[test]
    fn errors_cancellation_and_zero_matches_publish_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let input = fixture(dir.path(), &[record(0), record(1)]);
        let output = dir.path().join("export.ndjson");
        let mut calls = 0;
        let e = export(&input, &output, &Selection::default(), |_| {
            calls += 1;
            if calls > 1 {
                Err(ZenmonError::new(crate::ErrorKind::Interrupted, "cancel"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(e.kind, crate::ErrorKind::Interrupted);
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let empty = Selection::new("other/**", None, None).unwrap();
        assert!(export(&input, &output, &empty, |_| Ok(())).is_err());
        assert!(!output.exists());
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&input)
            .unwrap();
        writeln!(file, "BROKEN").unwrap();
        assert!(export(&input, &output, &Selection::default(), |_| Ok(())).is_err());
        assert!(!output.exists());
    }
    #[test]
    fn compressed_input_progress_and_output_race() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("source.ndjson.zst");
        let line = format!("{}\n", serde_json::to_string(&record(0)).unwrap());
        std::fs::write(
            &input,
            zstd::stream::encode_all(line.as_bytes(), 0).unwrap(),
        )
        .unwrap();
        let output = dir.path().join("export.ndjson");
        let mut last = Progress::default();
        export(&input, &output, &Selection::default(), |p| {
            last = p;
            Ok(())
        })
        .unwrap();
        assert_eq!(last.scanned, 1);
        assert_eq!(last.matched, 1);
        assert_eq!(last.files, 1);
        let raced = dir.path().join("raced.ndjson");
        assert!(atomic_export(&raced, |file| {
            file.write_all(b"new").map_err(io_error)?;
            std::fs::write(&raced, "existing").unwrap();
            Ok(1)
        })
        .is_err());
        assert_eq!(std::fs::read_to_string(raced).unwrap(), "existing");
    }
}
