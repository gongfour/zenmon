//! Chunked-NDJSON capture store + pure reader (`trace stats` / `trace read`).
//!
//! A long-lived `capture --dir` process appends [`CaptureRecord`] lines into
//! rotating segment files; this module owns the segment filename codec,
//! discovery, the rotating writer, retention, and the pure reader. Nothing here
//! opens a Zenoh session.
//!
//! [`CaptureRecord`]: crate::capture::CaptureRecord

use crate::capture::CaptureRecord;
use crate::error::ZemonError;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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

/// Appends NDJSON lines into rotating segment files under a directory.
pub struct SegmentWriter {
    dir: PathBuf,
    rotate_size: u64,
    rotate_interval: Duration,
    writer: Option<BufWriter<File>>,
    seg_first: SystemTime,
    seg_bytes: u64,
    next_seq: u32,
}

impl SegmentWriter {
    pub fn open(dir: PathBuf, rotate_size: u64, rotate_interval: Duration) -> Result<Self, ZemonError> {
        std::fs::create_dir_all(&dir).map_err(|e| {
            ZemonError::invalid_input(format!("cannot create {}: {}", dir.display(), e))
        })?;
        // Continue the seq space after any existing segments in the dir.
        let next_seq = discover_segments(&dir)?
            .iter()
            .map(|s| s.seq)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        Ok(Self {
            dir,
            rotate_size,
            rotate_interval,
            writer: None,
            seg_first: SystemTime::UNIX_EPOCH,
            seg_bytes: 0,
            next_seq,
        })
    }

    fn should_rotate(&self, now: SystemTime) -> bool {
        if self.writer.is_none() {
            return true;
        }
        if self.seg_bytes >= self.rotate_size {
            return true;
        }
        now.duration_since(self.seg_first)
            .map(|elapsed| elapsed >= self.rotate_interval)
            .unwrap_or(false)
    }

    fn rotate(&mut self, now: SystemTime) -> Result<(), ZemonError> {
        if let Some(mut w) = self.writer.take() {
            w.flush().map_err(|e| ZemonError::internal(format!("flush failed: {}", e)))?;
        }
        let name = segment_file_name(now, self.next_seq);
        self.next_seq += 1;
        let path = self.dir.join(name);
        let file = File::create(&path)
            .map_err(|e| ZemonError::internal(format!("cannot create {}: {}", path.display(), e)))?;
        self.writer = Some(BufWriter::new(file));
        self.seg_first = now;
        self.seg_bytes = 0;
        Ok(())
    }

    /// Append one line (a newline is added). Rotates first if the current
    /// segment is full or too old.
    pub fn write_line(&mut self, line: &str, now: SystemTime) -> Result<(), ZemonError> {
        if self.should_rotate(now) {
            self.rotate(now)?;
        }
        let w = self.writer.as_mut().expect("writer present after rotate");
        writeln!(w, "{}", line).map_err(|e| ZemonError::internal(format!("write failed: {}", e)))?;
        self.seg_bytes += line.len() as u64 + 1;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), ZemonError> {
        if let Some(w) = self.writer.as_mut() {
            w.flush().map_err(|e| ZemonError::internal(format!("flush failed: {}", e)))?;
        }
        Ok(())
    }
}

/// Prune closed segments to satisfy retention bounds. The newest segment (the
/// active one being written) is never deleted here. Age deletion uses a closed
/// segment's exclusive upper bound (the next segment's first timestamp): the
/// whole segment is older than `now - max_age` only when that bound is.
/// Returns the number of segments deleted.
pub fn enforce_retention(
    dir: &Path,
    max_total_size: Option<u64>,
    max_age: Option<Duration>,
    now: SystemTime,
) -> Result<u64, ZemonError> {
    let segs = discover_segments(dir)?;
    if segs.len() <= 1 {
        return Ok(0);
    }
    let closed = &segs[..segs.len() - 1]; // exclude newest/active

    // Mark for deletion (oldest first), by age then by total-size cap.
    let mut delete: Vec<bool> = vec![false; closed.len()];

    if let Some(age) = max_age {
        if let Some(cutoff) = now.checked_sub(age) {
            for (i, _seg) in closed.iter().enumerate() {
                if let Some(upper) = segment_upper_bound(&segs, i) {
                    if upper < cutoff {
                        delete[i] = true;
                    }
                }
            }
        }
    }

    if let Some(cap) = max_total_size {
        let mut total: u64 = segs.iter().map(|s| file_len(&s.path)).sum();
        // Drop oldest closed segments until within cap (skip already-marked).
        for (i, seg) in closed.iter().enumerate() {
            if total <= cap {
                break;
            }
            if !delete[i] {
                delete[i] = true;
                total = total.saturating_sub(file_len(&seg.path));
            } else {
                total = total.saturating_sub(file_len(&seg.path));
            }
        }
    }

    let mut deleted = 0;
    for (i, seg) in closed.iter().enumerate() {
        if delete[i] {
            std::fs::remove_file(&seg.path)
                .map_err(|e| ZemonError::internal(format!("cannot remove {}: {}", seg.path.display(), e)))?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// A record with its location in the store and parsed receive time.
#[derive(Debug, Clone)]
pub struct PositionedRecord {
    pub segment: String,
    pub index: u64,
    pub record: CaptureRecord,
    pub received: Option<SystemTime>,
}

/// Parse `received_at` (RFC3339) to `SystemTime`. v1 records (None) → None.
fn parse_received(rec: &CaptureRecord) -> Option<SystemTime> {
    rec.received_at
        .as_deref()
        .and_then(|s| humantime::parse_rfc3339(s).ok())
}

/// Load all records of one segment file, tagged with their 0-based index and
/// receive time. When `tolerate_partial_last_line` is set, a final line that
/// fails to parse (a truncated in-flight write in the active segment) is
/// dropped instead of erroring; any earlier bad line is always a hard error.
pub fn load_segment(
    path: &Path,
    tolerate_partial_last_line: bool,
) -> Result<Vec<PositionedRecord>, ZemonError> {
    let segment = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = std::fs::File::open(path)
        .map_err(|e| ZemonError::internal(format!("cannot open {}: {}", path.display(), e)))?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader
        .lines()
        .collect::<std::io::Result<_>>()
        .map_err(|e| ZemonError::internal(format!("read failed: {}", e)))?;

    let mut out = Vec::with_capacity(lines.len());
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match CaptureRecord::parse_line(line, i + 1) {
            Ok(record) => {
                let received = parse_received(&record);
                out.push(PositionedRecord { segment: segment.clone(), index: i as u64, record, received });
            }
            Err(e) => {
                if tolerate_partial_last_line && i == last {
                    break; // truncated final write in the active segment
                }
                return Err(e);
            }
        }
    }
    Ok(out)
}

/// Filters for a reader query.
#[derive(Debug, Clone)]
pub struct ReadFilter {
    pub key: String,
    pub since: Option<SystemTime>,
    pub until: Option<SystemTime>,
}

/// True if `record_key` is matched by `filter_key` (keyexpr intersection).
/// An invalid filter key expression is an `invalid_input` error.
pub fn key_matches(filter_key: &str, record_key: &str) -> Result<bool, ZemonError> {
    use zenoh::key_expr::KeyExpr;
    let filter = KeyExpr::try_from(filter_key)
        .map_err(|e| ZemonError::invalid_input(format!("invalid key expression '{}': {}", filter_key, e)))?;
    // A stored key is always a concrete key; if it fails to parse, treat as no-match.
    match KeyExpr::try_from(record_key) {
        Ok(rk) => Ok(filter.intersects(&rk)),
        Err(_) => Ok(false),
    }
}

/// Parse `--since` / `--until`: a relative duration (interpreted as `now - dur`)
/// or an absolute RFC3339 timestamp.
pub fn parse_time_bound(s: &str, now: SystemTime) -> Result<SystemTime, ZemonError> {
    let t = s.trim();
    if let Ok(dur) = humantime::parse_duration(t) {
        return now
            .checked_sub(dur)
            .ok_or_else(|| ZemonError::invalid_input(format!("time '{}' is before the epoch", s)));
    }
    humantime::parse_rfc3339(t)
        .map_err(|e| ZemonError::invalid_input(format!("invalid time '{}': {} (try 10m or an RFC3339 timestamp)", s, e)))
}

/// True if a record satisfies the filter's key and time window. A record with
/// no `received` time (v1) is time-unbounded (passes any since/until).
pub fn record_in_window(pr: &PositionedRecord, filter: &ReadFilter) -> Result<bool, ZemonError> {
    if !key_matches(&filter.key, &pr.record.key_expr)? {
        return Ok(false);
    }
    if let Some(rx) = pr.received {
        if let Some(since) = filter.since {
            if rx < since {
                return Ok(false);
            }
        }
        if let Some(until) = filter.until {
            if rx >= until {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn count_segments(dir: &Path) -> usize {
        discover_segments(dir).unwrap().len()
    }

    #[test]
    fn rotates_on_size() {
        let dir = tempdir_unique("rotsize");
        // rotate after ~20 bytes; interval huge so only size triggers.
        let mut w = SegmentWriter::open(dir.clone(), 20, Duration::from_secs(3600)).unwrap();
        let line = "0123456789"; // 11 bytes incl newline
        w.write_line(line, t(1000)).unwrap(); // seg A: 11
        w.write_line(line, t(1000)).unwrap(); // seg A: 22 -> next write rotates
        w.write_line(line, t(1000)).unwrap(); // seg B
        w.flush().unwrap();
        assert_eq!(count_segments(&dir), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rotates_on_interval() {
        let dir = tempdir_unique("rotint");
        let mut w = SegmentWriter::open(dir.clone(), 1 << 30, Duration::from_secs(60)).unwrap();
        w.write_line("a", t(1000)).unwrap();
        w.write_line("b", t(1000 + 30)).unwrap(); // within interval -> same seg
        w.write_line("c", t(1000 + 61)).unwrap(); // past interval -> new seg
        w.flush().unwrap();
        assert_eq!(count_segments(&dir), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn same_second_uses_distinct_seq() {
        let dir = tempdir_unique("seq");
        let mut w = SegmentWriter::open(dir.clone(), 1, Duration::from_secs(3600)).unwrap();
        // rotate_size=1 forces a new segment on every write, all at t=1000.
        w.write_line("a", t(1000)).unwrap();
        w.write_line("b", t(1000)).unwrap();
        w.flush().unwrap();
        let segs = discover_segments(&dir).unwrap();
        assert_eq!(segs.len(), 2);
        assert_ne!(segs[0].seq, segs[1].seq); // distinct seq despite same second
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retention_deletes_oldest_over_size_cap() {
        let dir = tempdir_unique("retsize");
        // three ~11-byte segments; cap at 25 bytes -> must drop oldest until <=25.
        write_segment(&dir, 1000, 0, &["0123456789"]);
        write_segment(&dir, 2000, 0, &["0123456789"]);
        write_segment(&dir, 3000, 0, &["0123456789"]); // newest (active) - protected from age, not size
        let deleted = enforce_retention(&dir, Some(25), None, t(4000)).unwrap();
        assert_eq!(deleted, 1);
        let segs = discover_segments(&dir).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].first, t(2000)); // oldest gone
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retention_deletes_closed_segments_older_than_age() {
        let dir = tempdir_unique("retage");
        write_segment(&dir, 1000, 0, &["x"]); // upper bound 2000
        write_segment(&dir, 2000, 0, &["x"]); // upper bound 3000
        write_segment(&dir, 3000, 0, &["x"]); // newest, protected
        // now=3600, max_age=1000s -> cutoff=2600. seg0 upper(2000)<2600 delete; seg1 upper(3000)>=2600 keep.
        let deleted = enforce_retention(&dir, None, Some(Duration::from_secs(1000)), t(3600)).unwrap();
        assert_eq!(deleted, 1);
        let segs = discover_segments(&dir).unwrap();
        assert_eq!(segs[0].first, t(2000));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retention_never_deletes_the_only_segment() {
        let dir = tempdir_unique("retone");
        write_segment(&dir, 1000, 0, &["0123456789"]);
        let deleted = enforce_retention(&dir, Some(1), Some(Duration::from_secs(0)), t(9_999_999)).unwrap();
        assert_eq!(deleted, 0); // newest/active is protected
        std::fs::remove_dir_all(&dir).ok();
    }

    fn rec_line(key: &str, received_secs: u64) -> String {
        let m = crate::types::ZenohMessage {
            key_expr: key.to_string(),
            payload: crate::types::MessagePayload::from_bytes(b"{}".to_vec()),
            encoding: "application/json".to_string(),
            payload_bytes: 2,
            timestamp: None,
            kind: "PUT".to_string(),
            attachment: None,
            attachment_bytes: None,
        };
        serde_json::to_string(&CaptureRecord::from_message(&m, Duration::ZERO, t(received_secs))).unwrap()
    }

    #[test]
    fn load_segment_positions_and_parses_received_at() {
        let dir = tempdir_unique("load");
        let path = write_segment(&dir, 1000, 0, &[&rec_line("a/b", 1000), &rec_line("c/d", 1001)]);
        let recs = load_segment(&path, true).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].index, 0);
        assert_eq!(recs[0].record.key_expr, "a/b");
        assert_eq!(recs[0].received, Some(t(1000)));
        assert_eq!(recs[1].index, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_segment_tolerates_trailing_partial_line_when_allowed() {
        let dir = tempdir_unique("partial");
        let path = dir.join(segment_file_name(t(1000), 0));
        // valid line + a partial (no newline, truncated json) as if mid-write.
        std::fs::write(&path, format!("{}\n{{\"schema_v", rec_line("a/b", 1000))).unwrap();
        let recs = load_segment(&path, true).unwrap();
        assert_eq!(recs.len(), 1); // partial dropped, no error
        // But when NOT tolerated, the corrupt line is an error.
        assert!(load_segment(&path, false).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn key_matches_uses_keyexpr_intersection() {
        assert!(key_matches("a/*", "a/b").unwrap());
        assert!(key_matches("**", "x/y/z").unwrap());
        assert!(!key_matches("a/*", "b/c").unwrap());
        // "a//b" (empty chunk) is not a valid key expression, unlike keyexpr.rs's
        // char-set restrictions this repo's brief assumed; mirror the invalid
        // example already verified in crates/zemon-core/src/keyexpr.rs.
        assert_eq!(key_matches("a//b", "a/b").unwrap_err().kind, crate::error::ErrorKind::InvalidInput);
    }

    #[test]
    fn parse_time_bound_relative_and_absolute() {
        let now = t(10_000);
        assert_eq!(parse_time_bound("1000s", now).unwrap(), t(9_000)); // now - 1000s
        assert_eq!(parse_time_bound("1970-01-01T00:00:05Z", now).unwrap(), t(5));
        assert!(parse_time_bound("garbage", now).is_err());
    }

    #[test]
    fn record_in_window_respects_since_until_and_key() {
        let dir = tempdir_unique("win");
        let path = write_segment(&dir, 1000, 0, &[&rec_line("a/b", 1000)]);
        let pr = load_segment(&path, true).unwrap().remove(0);
        let f = ReadFilter { key: "a/*".into(), since: Some(t(500)), until: Some(t(2000)) };
        assert!(record_in_window(&pr, &f).unwrap());
        let f2 = ReadFilter { key: "a/*".into(), since: Some(t(1500)), until: None };
        assert!(!record_in_window(&pr, &f2).unwrap()); // before since
        let f3 = ReadFilter { key: "z/*".into(), since: None, until: None };
        assert!(!record_in_window(&pr, &f3).unwrap()); // key mismatch
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_in_window_boundaries_since_inclusive_until_exclusive() {
        let dir = tempdir_unique("winb");
        let path = write_segment(&dir, 1000, 0, &[&rec_line("a/b", 1000)]); // received == t(1000)
        let pr = load_segment(&path, true).unwrap().remove(0);
        // received == since  -> INCLUDED (since is inclusive)
        let f_in = ReadFilter { key: "**".into(), since: Some(t(1000)), until: None };
        assert!(record_in_window(&pr, &f_in).unwrap());
        // received == until  -> EXCLUDED (until is exclusive)
        let f_ex = ReadFilter { key: "**".into(), since: None, until: Some(t(1000)) };
        assert!(!record_in_window(&pr, &f_ex).unwrap());
        // received in [since, until) -> INCLUDED
        let f_within = ReadFilter { key: "**".into(), since: Some(t(1000)), until: Some(t(1001)) };
        assert!(record_in_window(&pr, &f_within).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_in_window_v1_none_is_time_unbounded() {
        let dir = tempdir_unique("winv1");
        // A genuine v1 line: schema_version 1, NO received_at field.
        let v1 = r#"{"schema_version":1,"key_expr":"a/b","payload_base64":"","encoding":"","received_offset_ms":0,"kind":"PUT"}"#;
        let path = dir.join(segment_file_name(t(1000), 0));
        std::fs::write(&path, format!("{}\n", v1)).unwrap();
        let pr = load_segment(&path, true).unwrap().remove(0);
        assert!(pr.received.is_none(), "v1 record must have no received time");
        // A time window that would exclude any dated record still passes (unbounded).
        let f = ReadFilter { key: "a/*".into(), since: Some(t(5000)), until: Some(t(6000)) };
        assert!(record_in_window(&pr, &f).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn key_matches_unparseable_record_key_is_false() {
        // "a//b" (empty chunk) is an invalid key expression; as a RECORD key it must
        // yield Ok(false), never an error.
        assert!(!key_matches("a/*", "a//b").unwrap());
    }

    #[test]
    fn parse_time_bound_pre_epoch_underflow_is_error() {
        // `SystemTime::checked_sub` only fails once the result falls outside the
        // platform's representable range (e.g. before 1601-01-01 on Windows,
        // whose FILETIME epoch backs SystemTime there); a plain few-seconds
        // pre-epoch offset is representable on every target and would not
        // exercise this branch. 400 years comfortably underflows everywhere.
        let now = t(5); // 5s after UNIX_EPOCH
        let err = parse_time_bound("400y", now).unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::InvalidInput);
    }
}
