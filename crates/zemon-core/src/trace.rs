//! Chunked-NDJSON capture store + pure reader (`trace stats` / `trace read`).
//!
//! A long-lived `capture --dir` process appends [`CaptureRecord`] lines into
//! rotating segment files; this module owns the segment filename codec,
//! discovery, the rotating writer, retention, and the pure reader. Nothing here
//! opens a Zenoh session.
//!
//! [`CaptureRecord`]: crate::capture::CaptureRecord

use crate::error::ZemonError;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const SEG_PREFIX: &str = "zemon-trace-";
const SEG_EXT: &str = ".ndjson";

/// Compact, colon-free RFC3339-seconds stamp for filenames: `YYYYMMDDTHHMMSSZ`.
pub fn format_segment_stamp(t: SystemTime) -> String {
    humantime::format_rfc3339_seconds(t)
        .to_string() // "2026-07-16T12:34:56Z"
        .chars()
        .filter(|c| *c != '-' && *c != ':')
        .collect() // "20260716T123456Z"
}

/// Inverse of [`format_segment_stamp`]. Returns `None` on malformed input.
pub fn parse_segment_stamp(s: &str) -> Option<SystemTime> {
    if !s.is_ascii() || s.len() != 16 || s.as_bytes()[8] != b'T' || s.as_bytes()[15] != b'Z' {
        return None;
    }
    let rfc = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &s[0..4], &s[4..6], &s[6..8], &s[9..11], &s[11..13], &s[13..15]
    );
    humantime::parse_rfc3339(&rfc).ok()
}

/// `zemon-trace-<stamp>-<seq:05>.ndjson`.
pub fn segment_file_name(first: SystemTime, seq: u32) -> String {
    format!("{}{}-{:05}{}", SEG_PREFIX, format_segment_stamp(first), seq, SEG_EXT)
}

/// Parse a segment filename into `(first_timestamp, seq)`. Non-segment files
/// return `None` (so a directory may hold unrelated files harmlessly).
pub fn parse_segment_file_name(name: &str) -> Option<(SystemTime, u32)> {
    let core = name.strip_prefix(SEG_PREFIX)?.strip_suffix(SEG_EXT)?;
    let (stamp, seq) = core.rsplit_once('-')?;
    Some((parse_segment_stamp(stamp)?, seq.parse().ok()?))
}

/// A discovered segment file with its parsed first-timestamp and sequence.
#[derive(Debug, Clone)]
pub struct Segment {
    pub path: PathBuf,
    pub first: SystemTime,
    pub seq: u32,
}

/// List the store's segments in chronological order. A missing directory is a
/// `not_found` error; an existing directory with no segments is an empty Vec.
/// Non-segment files are ignored.
pub fn discover_segments(dir: &Path) -> Result<Vec<Segment>, ZemonError> {
    let entries = std::fs::read_dir(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            ZemonError::not_found(format!("trace directory not found: {}", dir.display()))
        }
        _ => ZemonError::internal(format!("cannot read {}: {}", dir.display(), e)),
    })?;
    let mut segs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| ZemonError::internal(e.to_string()))?;
        let name = entry.file_name();
        if let Some((first, seq)) = parse_segment_file_name(&name.to_string_lossy()) {
            segs.push(Segment { path: entry.path(), first, seq });
        }
    }
    segs.sort_by(|a, b| a.first.cmp(&b.first).then(a.seq.cmp(&b.seq)));
    Ok(segs)
}

/// The exclusive upper time bound of segment `i` = the next segment's first
/// timestamp, or `None` for the newest (active) segment.
pub fn segment_upper_bound(segs: &[Segment], i: usize) -> Option<SystemTime> {
    segs.get(i + 1).map(|s| s.first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::time::Duration;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn tempdir_unique(tag: &str) -> PathBuf {
        // Unique without rand/time crates: use a process-wide atomic counter + pid.
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("zemon-trace-test-{}-{}-{}", tag, std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_segment(dir: &Path, first_secs: u64, seq: u32, lines: &[&str]) -> PathBuf {
        let name = segment_file_name(t(first_secs), seq);
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{}", l).unwrap();
        }
        path
    }

    #[test]
    fn stamp_has_no_colon_and_roundtrips() {
        let ts = t(1_752_668_096);
        let stamp = format_segment_stamp(ts);
        assert!(!stamp.contains(':'), "windows-illegal colon: {stamp}");
        assert_eq!(stamp.len(), 16);
        assert_eq!(parse_segment_stamp(&stamp), Some(ts));
    }

    #[test]
    fn filename_roundtrips() {
        let ts = t(1_752_668_096);
        let name = segment_file_name(ts, 7);
        assert!(name.starts_with("zemon-trace-"));
        assert!(name.ends_with("-00007.ndjson"));
        assert_eq!(parse_segment_file_name(&name), Some((ts, 7)));
    }

    #[test]
    fn filenames_sort_chronologically() {
        let mut names = [
            segment_file_name(t(2000), 0),
            segment_file_name(t(1000), 9),
            segment_file_name(t(1000), 1),
        ];
        names.sort();
        assert_eq!(parse_segment_file_name(&names[0]).unwrap().1, 1); // 1000/seq1
        assert_eq!(parse_segment_file_name(&names[1]).unwrap().1, 9); // 1000/seq9
        assert_eq!(parse_segment_file_name(&names[2]).unwrap().0, t(2000));
    }

    #[test]
    fn non_segment_files_ignored() {
        assert_eq!(parse_segment_file_name("notes.txt"), None);
        assert_eq!(parse_segment_file_name("zemon-trace-bad.ndjson"), None);
    }

    #[test]
    fn parse_segment_stamp_rejects_non_ascii_without_panic() {
        // Build a 16-BYTE non-ASCII string ('é' is 2 bytes) that passes the
        // byte-length check; must return None, not panic on char-boundary slicing.
        let crafted = format!("ABC\u{00e9}DEFTGHIJKL{}", "Z");
        assert_eq!(crafted.len(), 16);
        assert_eq!(parse_segment_stamp(&crafted), None);
        // And a segment-shaped filename with such a stamp must also be ignored, not panic.
        let name = format!("zemon-trace-{}-00001.ndjson", crafted);
        assert_eq!(parse_segment_file_name(&name), None);
    }

    #[test]
    fn discover_sorts_and_ignores_foreign_files() {
        let dir = tempdir_unique("disc");
        write_segment(&dir, 2000, 0, &[]);
        write_segment(&dir, 1000, 0, &[]);
        std::fs::write(dir.join("README"), b"hi").unwrap();
        let segs = discover_segments(&dir).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].first, t(1000));
        assert_eq!(segs[1].first, t(2000));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discover_missing_dir_is_not_found() {
        let err = discover_segments(Path::new("does/not/exist/xyz")).unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::NotFound);
    }

    #[test]
    fn discover_empty_dir_is_empty_ok() {
        let dir = tempdir_unique("empty");
        assert!(discover_segments(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upper_bound_is_next_first_or_none_for_last() {
        let dir = tempdir_unique("bound");
        write_segment(&dir, 1000, 0, &[]);
        write_segment(&dir, 3000, 0, &[]);
        let segs = discover_segments(&dir).unwrap();
        assert_eq!(segment_upper_bound(&segs, 0), Some(t(3000)));
        assert_eq!(segment_upper_bound(&segs, 1), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
