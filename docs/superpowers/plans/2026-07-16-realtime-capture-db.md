# Realtime Capture Store + Trace Reader Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a continuously-running chunked-NDJSON capture *store* (`zemon capture --dir`) with rotation + retention, plus a pure, network-free reader (`zemon trace stats` / `zemon trace read`) so an AI agent can inspect network activity that happened while it was absent.

**Architecture:** A long-lived `capture --dir` process appends `CaptureRecord` lines into rotating segment files under a directory, enforcing size/age retention. The reader lives in a new pure `zemon-core::trace` module (no Zenoh session): it discovers segments by filename, skips whole segments outside the time window, loads one bounded segment at a time, applies filters, and returns either a `{count,items}` rollup (`stats`) or a paginated NDJSON stream with a cursor (`read`). Collector and reader share only files.

**Tech Stack:** Rust 2021, `serde`/`serde_json`, `humantime` (RFC3339 timestamps + durations — already a dep), `base64` (payload + cursor encoding — already a dep), `clap` (CLI), `tokio` (capture loop). Reader logic is synchronous and pure.

## Global Constraints

- **No new dependencies.** Use `humantime::format_rfc3339` / `humantime::parse_rfc3339` for `SystemTime` ↔ RFC3339, and `base64` (STANDARD engine) for the opaque cursor. Both crates are already in `zemon-core`.
- **Windows-safe filenames:** segment names must contain no `:` (illegal on Windows). Use the compact stamp `YYYYMMDDTHHMMSSZ` (no colons/dashes).
- **JSON contracts (reuse existing):** finite output uses `zemon_core::output::to_collection_json` (`{"count":N,"items":[...]}`); streaming output is NDJSON (one object per line, no ANSI); errors are `ZemonError` with stable `kind` + `exit_code()` (`invalid_input=2, not_found=5, internal=1`).
- **Empty ≠ error:** an existing but empty store is success (`{"count":0,"items":[]}` / empty stream, exit 0). Only a missing directory is `not_found`.
- **Reader is pure:** `zemon-core::trace` must not open a Zenoh session. It may use `zenoh::key_expr::KeyExpr` (pure) for key matching.
- **Record schema:** rotating captures write schema v2 (`received_at` populated). `parse_line` accepts v1 and v2; v1 (no `received_at`) records are treated as time-unbounded by the reader.
- **Defaults:** rotate at `64MB` or `1h`; retention `1GB` and `7d`; `trace read --limit` default `100`.
- TDD: write the failing test first. `cargo test` and `cargo clippy` must be green before each commit. One commit per task.
- Commit message convention: `feat(scope):` / `fix(scope):` / `chore:`.

---

## File Structure

- `crates/zemon-core/src/capture.rs` — **modify.** Add `received_at`; bump `SCHEMA_VERSION` to 2; accept `{1,2}` in `parse_line`; extend `from_message`.
- `crates/zemon-core/src/trace.rs` — **create.** All reader + segment-store logic: filename codec, discovery, rotating writer, retention, per-segment load, filters, `read_page`, `topic_stats`, cursor. Pure/synchronous.
- `crates/zemon-core/src/lib.rs` — **modify.** `pub mod trace;`.
- `crates/zemon-cli/src/duration.rs` — **modify.** Add `parse_byte_size_arg`.
- `crates/zemon-cli/src/cli.rs` — **modify.** Extend `Command::Capture` (rotating mode); add `Command::Trace` + `TraceCommand`.
- `crates/zemon-cli/src/main.rs` — **modify.** Update the one `from_message` call site (Task 1); rewrite the `Capture` handler (Task 11); add `Trace` handlers (Task 12).
- `README.md` — **modify.** Document the store + reader (Task 12).

Types produced by `zemon-core::trace` (referenced across tasks):

```rust
pub struct Segment { pub path: PathBuf, pub first: SystemTime, pub seq: u32 }
pub struct PositionedRecord { pub segment: String, pub index: u64, pub record: CaptureRecord, pub received: Option<SystemTime> }
pub struct ReadFilter { pub key: String, pub since: Option<SystemTime>, pub until: Option<SystemTime> }
pub struct ReadPage { pub records: Vec<PositionedRecord>, pub matched: u64, pub returned: u64, pub cursor: Option<String>, pub truncated: bool }
pub struct TopicStat { /* serde: key,count,first_ts,last_ts,rate_hz,last_value_preview,last_value_bytes,encoding */ }
```

---

### Task 1: Record schema v2 — `received_at`

**Files:**
- Modify: `crates/zemon-core/src/capture.rs`
- Modify: `crates/zemon-cli/src/main.rs:816` (the one `from_message` call site)

**Interfaces:**
- Produces: `CaptureRecord.received_at: Option<String>`; `CaptureRecord::from_message(msg: &ZenohMessage, offset: Duration, received_at: SystemTime) -> Self`; `pub const SCHEMA_VERSION: u32 = 2`; `parse_line` accepts versions `{1,2}`.

- [ ] **Step 1: Write the failing tests** (append to `capture.rs` `mod tests`)

```rust
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
    let line = serde_json::to_string(&CaptureRecord::from_message(&m, Duration::ZERO, t)).unwrap();
    assert!(CaptureRecord::parse_line(&line, 1).is_ok());
    let v3 = line.replace("\"schema_version\":2", "\"schema_version\":3");
    assert!(CaptureRecord::parse_line(&v3, 5).unwrap_err().message.contains("schema_version"));
}
```

Also update the existing helper calls in `mod tests`: every `CaptureRecord::from_message(&m, <dur>)` becomes `CaptureRecord::from_message(&m, <dur>, std::time::UNIX_EPOCH)`. Add `use std::time::Duration;` already present; add nothing else.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zemon-core capture`
Expected: FAIL — `from_message` takes 2 args, `received_at` field missing.

- [ ] **Step 3: Implement**

In `capture.rs`:

```rust
use std::time::{Duration, SystemTime};

pub const SCHEMA_VERSION: u32 = 2;
const SUPPORTED_VERSIONS: &[u32] = &[1, 2];
```

Add the field to `CaptureRecord` (after `source_timestamp`):

```rust
    /// Receiver wall-clock time this record was captured, RFC3339 (UTC).
    /// Present from schema v2 on; absent in v1 files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_at: Option<String>,
```

Update `from_message`:

```rust
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
```

Update the version check in `parse_line`:

```rust
        if !SUPPORTED_VERSIONS.contains(&rec.schema_version) {
            return Err(ZemonError::invalid_input(format!(
                "unsupported schema_version {} at line {} (supported: {:?})",
                rec.schema_version, line_no, SUPPORTED_VERSIONS
            )));
        }
```

In `crates/zemon-cli/src/main.rs`, update the call site (currently `CaptureRecord::from_message(&msg, start.elapsed())`):

```rust
let rec = CaptureRecord::from_message(&msg, start.elapsed(), std::time::SystemTime::now());
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-core capture && cargo build -p zemon-cli`
Expected: PASS; CLI builds.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-core/src/capture.rs crates/zemon-cli/src/main.rs
git commit -m "feat(capture): add received_at wall-clock field, schema v2 (v1-compatible parse)"
```

---

### Task 2: Byte-size CLI parser

**Files:**
- Modify: `crates/zemon-cli/src/duration.rs`

**Interfaces:**
- Produces: `pub fn parse_byte_size_arg(s: &str) -> Result<u64, String>` — accepts `64MB`, `1GB`, `512KB`, binary `MiB/GiB/KiB`, and a bare integer (bytes). Rejects `0`.

- [ ] **Step 1: Write the failing test** (append to `duration.rs` `mod tests`)

```rust
#[test]
fn parses_byte_sizes() {
    assert_eq!(parse_byte_size_arg("64MB").unwrap(), 64 * 1000 * 1000);
    assert_eq!(parse_byte_size_arg("1GB").unwrap(), 1_000_000_000);
    assert_eq!(parse_byte_size_arg("512KB").unwrap(), 512_000);
    assert_eq!(parse_byte_size_arg("1MiB").unwrap(), 1024 * 1024);
    assert_eq!(parse_byte_size_arg("2048").unwrap(), 2048); // bare = bytes
}

#[test]
fn rejects_zero_and_garbage_byte_size() {
    assert!(parse_byte_size_arg("0").is_err());
    assert!(parse_byte_size_arg("0MB").is_err());
    assert!(parse_byte_size_arg("big").is_err());
    assert!(parse_byte_size_arg("5PB").is_err()); // unsupported unit
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-cli parses_byte_sizes`
Expected: FAIL — `parse_byte_size_arg` not defined.

- [ ] **Step 3: Implement** (add to `duration.rs`)

```rust
/// Parse a byte-size option (`--rotate-size 64MB`). Accepts decimal
/// (`KB/MB/GB`, ×1000) and binary (`KiB/MiB/GiB`, ×1024) units, or a bare
/// integer (bytes). Rejects zero.
pub fn parse_byte_size_arg(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let units: &[(&str, u64)] = &[
        ("KiB", 1 << 10), ("MiB", 1 << 20), ("GiB", 1 << 30),
        ("KB", 1_000), ("MB", 1_000_000), ("GB", 1_000_000_000),
        ("B", 1),
    ];
    let (num_str, mult) = units
        .iter()
        .find_map(|(suf, m)| t.strip_suffix(suf).map(|n| (n.trim(), *m)))
        .unwrap_or((t, 1));
    let n: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid size '{}': expected e.g. 64MB, 1GB", s))?;
    let bytes = n
        .checked_mul(mult)
        .ok_or_else(|| format!("size '{}' overflows", s))?;
    if bytes == 0 {
        return Err("size must be greater than zero".to_string());
    }
    Ok(bytes)
}
```

Note: order matters — `KiB` must be tested before `B` (both end in `B`); the list above is ordered longest/binary-first so `find_map` matches `KiB` before `B`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-cli byte_size`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-cli/src/duration.rs
git commit -m "feat(cli): byte-size arg parser for capture rotation/retention options"
```

---

### Task 3: Segment filename codec

**Files:**
- Create: `crates/zemon-core/src/trace.rs`
- Modify: `crates/zemon-core/src/lib.rs`

**Interfaces:**
- Produces: `format_segment_stamp(SystemTime) -> String`; `parse_segment_stamp(&str) -> Option<SystemTime>`; `segment_file_name(SystemTime, u32) -> String`; `parse_segment_file_name(&str) -> Option<(SystemTime, u32)>`.

- [ ] **Step 1: Create the module and write failing tests**

Add `pub mod trace;` to `crates/zemon-core/src/lib.rs` (after `pub mod topology;`).

Create `crates/zemon-core/src/trace.rs`:

```rust
//! Chunked-NDJSON capture store + pure reader (`trace stats` / `trace read`).
//!
//! A long-lived `capture --dir` process appends [`CaptureRecord`] lines into
//! rotating segment files; this module owns the segment filename codec,
//! discovery, the rotating writer, retention, and the pure reader. Nothing here
//! opens a Zenoh session.

use crate::capture::CaptureRecord;
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
    if s.len() != 16 || s.as_bytes()[8] != b'T' || s.as_bytes()[15] != b'Z' {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
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
        let mut names = vec![
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
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-core trace::tests`
Expected: FAIL first because `lib.rs` didn't yet include the module / until the code above compiles. After adding both, tests should pass (they exercise the code you just wrote). If any assertion fails, fix the codec.

- [ ] **Step 3: Confirm implementation** — the code in Step 1 is the implementation; no separate impl step.

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-core trace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-core/src/trace.rs crates/zemon-core/src/lib.rs
git commit -m "feat(trace): segment filename codec (windows-safe, chronologically sortable)"
```

---

### Task 4: Segment discovery + time bounds

**Files:**
- Modify: `crates/zemon-core/src/trace.rs`

**Interfaces:**
- Consumes: `parse_segment_file_name`.
- Produces: `pub struct Segment { pub path: PathBuf, pub first: SystemTime, pub seq: u32 }`; `discover_segments(&Path) -> Result<Vec<Segment>, ZemonError>` (sorted, `not_found` if dir missing, empty Vec if dir empty); `segment_upper_bound(&[Segment], usize) -> Option<SystemTime>`.

- [ ] **Step 1: Write failing tests** (append to `trace.rs` tests)

```rust
    use std::io::Write as _;

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
```

Add this helper at the top of `mod tests` (a dependency-free unique temp dir under the OS temp dir — avoids needing the `tempfile` crate):

```rust
    fn tempdir_unique(tag: &str) -> PathBuf {
        // Unique without rand/time crates: use a process-wide atomic counter + pid.
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("zemon-trace-test-{}-{}-{}", tag, std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-core trace::tests::discover`
Expected: FAIL — `discover_segments` / `Segment` not defined.

- [ ] **Step 3: Implement** (add to `trace.rs`, before `mod tests`)

```rust
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-core trace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-core/src/trace.rs
git commit -m "feat(trace): segment discovery + time bounds"
```

---

### Task 5: Rotating segment writer

**Files:**
- Modify: `crates/zemon-core/src/trace.rs`

**Interfaces:**
- Consumes: `segment_file_name`.
- Produces: `pub struct SegmentWriter`; `SegmentWriter::open(dir: PathBuf, rotate_size: u64, rotate_interval: Duration) -> Result<Self, ZemonError>`; `write_line(&mut self, line: &str, now: SystemTime) -> Result<(), ZemonError>`; `flush(&mut self) -> Result<(), ZemonError>`. `now` is injected so rotation is deterministically testable.

- [ ] **Step 1: Write failing tests** (append to `trace.rs` tests; add `use std::time::Duration;` already imported)

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-core trace::tests::rotates`
Expected: FAIL — `SegmentWriter` not defined.

- [ ] **Step 3: Implement** (add to `trace.rs`)

```rust
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Duration;

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
```

Note: move the `use std::time::Duration;` to the module's top `use` block if clippy warns about a duplicate; keep a single import.

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-core trace && cargo clippy -p zemon-core -- -D warnings`
Expected: PASS; no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-core/src/trace.rs
git commit -m "feat(trace): rotating segment writer (size + interval, injected clock)"
```

---

### Task 6: Retention

**Files:**
- Modify: `crates/zemon-core/src/trace.rs`

**Interfaces:**
- Consumes: `discover_segments`, `segment_upper_bound`.
- Produces: `enforce_retention(dir: &Path, max_total_size: Option<u64>, max_age: Option<Duration>, now: SystemTime) -> Result<u64, ZemonError>` — deletes only *closed* (non-newest) segments; returns the number deleted. The newest segment is never age-pruned (it has no successor bound).

- [ ] **Step 1: Write failing tests** (append to `trace.rs` tests)

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-core trace::tests::retention`
Expected: FAIL — `enforce_retention` not defined.

- [ ] **Step 3: Implement** (add to `trace.rs`)

```rust
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-core trace && cargo clippy -p zemon-core -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-core/src/trace.rs
git commit -m "feat(trace): retention (size + age, oldest-first, active segment protected)"
```

---

### Task 7: Per-segment record load

**Files:**
- Modify: `crates/zemon-core/src/trace.rs`

**Interfaces:**
- Consumes: `CaptureRecord::parse_line`.
- Produces: `pub struct PositionedRecord { pub segment: String, pub index: u64, pub record: CaptureRecord, pub received: Option<SystemTime> }`; `load_segment(path: &Path, tolerate_partial_last_line: bool) -> Result<Vec<PositionedRecord>, ZemonError>`. `received` is `received_at` parsed to `SystemTime`, else `None`.

- [ ] **Step 1: Write failing tests** (append to `trace.rs` tests)

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-core trace::tests::load_segment`
Expected: FAIL — `load_segment` / `PositionedRecord` not defined.

- [ ] **Step 3: Implement** (add to `trace.rs`)

```rust
use std::io::{BufRead, BufReader};

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
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-core trace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-core/src/trace.rs
git commit -m "feat(trace): per-segment record load with partial-line tolerance"
```

---

### Task 8: Filters — key match + time-bound parsing

**Files:**
- Modify: `crates/zemon-core/src/trace.rs`

**Interfaces:**
- Produces: `pub struct ReadFilter { pub key: String, pub since: Option<SystemTime>, pub until: Option<SystemTime> }`; `key_matches(filter_key: &str, record_key: &str) -> Result<bool, ZemonError>` (keyexpr intersection; invalid filter → `invalid_input`); `parse_time_bound(s: &str, now: SystemTime) -> Result<SystemTime, ZemonError>` (relative duration = `now - dur`, or absolute RFC3339); `record_in_window(&PositionedRecord, &ReadFilter) -> Result<bool, ZemonError>`.

- [ ] **Step 1: Write failing tests** (append to `trace.rs` tests)

```rust
    #[test]
    fn key_matches_uses_keyexpr_intersection() {
        assert!(key_matches("a/*", "a/b").unwrap());
        assert!(key_matches("**", "x/y/z").unwrap());
        assert!(!key_matches("a/*", "b/c").unwrap());
        assert_eq!(key_matches("a/[bad", "a/b").unwrap_err().kind, crate::error::ErrorKind::InvalidInput);
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-core trace::tests::key_matches`
Expected: FAIL — symbols not defined.

- [ ] **Step 3: Implement** (add to `trace.rs`)

```rust
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
```

Note: confirm the zenoh `KeyExpr::intersects` signature against `crates/zemon-core/src/keyexpr.rs` (that module already uses `KeyExpr`); mirror its import path and method names to stay consistent.

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-core trace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-core/src/trace.rs
git commit -m "feat(trace): reader filters (keyexpr match + since/until parsing)"
```

---

### Task 9: `read_page` — paginated read with cursor + reducers

**Files:**
- Modify: `crates/zemon-core/src/trace.rs`

**Interfaces:**
- Consumes: `discover_segments`, `segment_upper_bound`, `load_segment`, `record_in_window`.
- Produces: `pub struct ReadOptions { pub filter: ReadFilter, pub limit: Option<u64>, pub last_per_key: bool, pub every: Option<u64>, pub cursor: Option<String> }`; `pub struct ReadPage { pub records: Vec<PositionedRecord>, pub matched: u64, pub returned: u64, pub cursor: Option<String>, pub truncated: bool }`; `read_page(dir: &Path, opts: &ReadOptions) -> Result<ReadPage, ZemonError>`; `encode_cursor(&str, u64) -> String`; `decode_cursor(&str) -> Result<(String, u64), ZemonError>`.

Semantics: default plain read is chronological, honors `cursor` (resume) and `limit` (default caller-applied); `matched` counts matches from the cursor onward; `truncated = matched > returned`; `cursor` in the result points just past the last returned record (None when not truncated). `last_per_key` and `every` are whole-window single-shot reducers: they ignore `cursor`, scan the full window, and cap the *result* at `limit` (with `truncated`/`matched` reflecting the reduced set).

- [ ] **Step 1: Write failing tests** (append to `trace.rs` tests)

```rust
    fn seed_store(tag: &str) -> PathBuf {
        // 3 segments, 2 records each, keys alternate a/x and b/y, times 1000..1005
        let dir = tempdir_unique(tag);
        write_segment(&dir, 1000, 0, &[&rec_line("a/x", 1000), &rec_line("b/y", 1001)]);
        write_segment(&dir, 1002, 0, &[&rec_line("a/x", 1002), &rec_line("b/y", 1003)]);
        write_segment(&dir, 1004, 0, &[&rec_line("a/x", 1004), &rec_line("b/y", 1005)]);
        dir
    }

    fn plain_opts(key: &str, limit: Option<u64>, cursor: Option<String>) -> ReadOptions {
        ReadOptions {
            filter: ReadFilter { key: key.into(), since: None, until: None },
            limit,
            last_per_key: false,
            every: None,
            cursor,
        }
    }

    #[test]
    fn read_page_limits_and_reports_matched() {
        let dir = seed_store("rp1");
        let page = read_page(&dir, &plain_opts("a/*", Some(2), None)).unwrap();
        assert_eq!(page.returned, 2);
        assert_eq!(page.matched, 3); // three a/x records match
        assert!(page.truncated);
        assert!(page.cursor.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_page_cursor_resumes_without_overlap() {
        let dir = seed_store("rp2");
        let p1 = read_page(&dir, &plain_opts("a/*", Some(2), None)).unwrap();
        let p2 = read_page(&dir, &plain_opts("a/*", Some(2), p1.cursor.clone())).unwrap();
        assert_eq!(p2.returned, 1); // one a/x record left
        assert!(!p2.truncated);
        assert!(p2.cursor.is_none());
        // No overlap: last of p1 precedes first of p2 chronologically.
        assert_eq!(p1.records[1].received, Some(t(1002)));
        assert_eq!(p2.records[0].received, Some(t(1004)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_page_last_per_key_collapses() {
        let dir = seed_store("rp3");
        let mut opts = plain_opts("**", None, None);
        opts.last_per_key = true;
        let page = read_page(&dir, &opts).unwrap();
        assert_eq!(page.returned, 2); // one per key: a/x, b/y
        // latest a/x is t(1004), latest b/y is t(1005)
        let times: Vec<_> = page.records.iter().map(|r| r.received).collect();
        assert!(times.contains(&Some(t(1004))) && times.contains(&Some(t(1005))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_page_every_n_samples() {
        let dir = seed_store("rp4");
        let mut opts = plain_opts("**", None, None);
        opts.every = Some(3);
        let page = read_page(&dir, &opts).unwrap();
        // 6 matching records, every 3rd -> indices 0 and 3 -> 2 records
        assert_eq!(page.returned, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cursor_roundtrips() {
        let c = encode_cursor("zemon-trace-20260716T000000Z-00000.ndjson", 4);
        let (seg, idx) = decode_cursor(&c).unwrap();
        assert_eq!((seg.as_str(), idx), ("zemon-trace-20260716T000000Z-00000.ndjson", 4));
        assert_eq!(decode_cursor("!notbase64!").unwrap_err().kind, crate::error::ErrorKind::InvalidInput);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-core trace::tests::read_page`
Expected: FAIL — `read_page` and friends not defined.

- [ ] **Step 3: Implement** (add to `trace.rs`)

```rust
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Options for [`read_page`].
#[derive(Debug, Clone)]
pub struct ReadOptions {
    pub filter: ReadFilter,
    pub limit: Option<u64>,
    pub last_per_key: bool,
    pub every: Option<u64>,
    pub cursor: Option<String>,
}

/// One page of a `trace read`.
#[derive(Debug, Clone)]
pub struct ReadPage {
    pub records: Vec<PositionedRecord>,
    pub matched: u64,
    pub returned: u64,
    pub cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Serialize, Deserialize)]
struct CursorInner {
    segment: String,
    index: u64,
}

/// Opaque cursor pointing at the next record to read (segment name + index).
pub fn encode_cursor(segment: &str, index: u64) -> String {
    let json = serde_json::to_string(&CursorInner { segment: segment.to_string(), index }).unwrap_or_default();
    base64::engine::general_purpose::STANDARD.encode(json)
}

pub fn decode_cursor(s: &str) -> Result<(String, u64), ZemonError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| ZemonError::invalid_input(format!("invalid cursor: {}", e)))?;
    let inner: CursorInner = serde_json::from_slice(&bytes)
        .map_err(|e| ZemonError::invalid_input(format!("invalid cursor: {}", e)))?;
    Ok((inner.segment, inner.index))
}

/// True if a segment (by index `i`) can hold any record in `[since, until)`.
/// Skips whole segments outside the window using filename bounds only.
fn segment_overlaps_window(segs: &[Segment], i: usize, filter: &ReadFilter) -> bool {
    let first = segs[i].first;
    let upper = segment_upper_bound(segs, i); // exclusive; None = active/open-ended
    if let Some(until) = filter.until {
        if first >= until {
            return false; // starts at/after the window end
        }
    }
    if let Some(since) = filter.since {
        if let Some(upper) = upper {
            if upper <= since {
                return false; // entirely before the window
            }
        }
    }
    true
}

fn is_last_segment(segs: &[Segment], i: usize) -> bool {
    i + 1 == segs.len()
}

pub fn read_page(dir: &Path, opts: &ReadOptions) -> Result<ReadPage, ZemonError> {
    let segs = discover_segments(dir)?;

    // Reducer paths scan the whole window, single-shot (cursor ignored).
    if opts.last_per_key {
        return read_last_per_key(&segs, opts);
    }
    if let Some(n) = opts.every {
        return read_every_n(&segs, opts, n.max(1));
    }

    // Plain chronological read with optional cursor + limit.
    let cursor = opts.cursor.as_deref().map(decode_cursor).transpose()?;
    let mut records = Vec::new();
    let mut matched: u64 = 0;
    let mut resumed = cursor.is_none();
    let mut next_cursor: Option<(String, u64)> = None;
    let limit = opts.limit;

    for i in 0..segs.len() {
        if !segment_overlaps_window(&segs, i, &opts.filter) {
            continue;
        }
        let loaded = load_segment(&segs[i].path, is_last_segment(&segs, i))?;
        for pr in loaded {
            // Skip forward to the cursor position on the resume segment.
            if !resumed {
                let (cseg, cidx) = cursor.as_ref().unwrap();
                if &pr.segment == cseg && pr.index < *cidx {
                    continue;
                }
                if &pr.segment == cseg && pr.index >= *cidx {
                    resumed = true;
                } else if pr.segment > *cseg {
                    resumed = true; // cursor segment already gone (retention) — resume here
                } else {
                    continue; // still before the cursor segment
                }
            }
            if !record_in_window(&pr, &opts.filter)? {
                continue;
            }
            matched += 1;
            let over_limit = limit.map(|l| records.len() as u64 >= l).unwrap_or(false);
            if over_limit {
                if next_cursor.is_none() {
                    next_cursor = Some((pr.segment.clone(), pr.index));
                }
                // keep counting `matched`, stop collecting
            } else {
                records.push(pr);
            }
        }
    }

    let returned = records.len() as u64;
    let truncated = matched > returned;
    let cursor = next_cursor.map(|(s, i)| encode_cursor(&s, i));
    Ok(ReadPage { records, matched, returned, cursor, truncated })
}

fn read_last_per_key(segs: &[Segment], opts: &ReadOptions) -> Result<ReadPage, ZemonError> {
    use std::collections::BTreeMap;
    let mut latest: BTreeMap<String, PositionedRecord> = BTreeMap::new();
    for i in 0..segs.len() {
        if !segment_overlaps_window(segs, i, &opts.filter) {
            continue;
        }
        for pr in load_segment(&segs[i].path, is_last_segment(segs, i))? {
            if record_in_window(&pr, &opts.filter)? {
                latest.insert(pr.record.key_expr.clone(), pr); // later segments overwrite → last wins
            }
        }
    }
    finalize_reduced(latest.into_values().collect(), opts.limit)
}

fn read_every_n(segs: &[Segment], opts: &ReadOptions, n: u64) -> Result<ReadPage, ZemonError> {
    let mut sampled = Vec::new();
    let mut seen: u64 = 0;
    for i in 0..segs.len() {
        if !segment_overlaps_window(segs, i, &opts.filter) {
            continue;
        }
        for pr in load_segment(&segs[i].path, is_last_segment(segs, i))? {
            if record_in_window(&pr, &opts.filter)? {
                if seen % n == 0 {
                    sampled.push(pr);
                }
                seen += 1;
            }
        }
    }
    finalize_reduced(sampled, opts.limit)
}

fn finalize_reduced(all: Vec<PositionedRecord>, limit: Option<u64>) -> Result<ReadPage, ZemonError> {
    let matched = all.len() as u64;
    let records: Vec<_> = match limit {
        Some(l) => all.into_iter().take(l as usize).collect(),
        None => all,
    };
    let returned = records.len() as u64;
    Ok(ReadPage {
        records,
        matched,
        returned,
        cursor: None, // reducers are single-shot
        truncated: matched > returned,
    })
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-core trace && cargo clippy -p zemon-core -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-core/src/trace.rs
git commit -m "feat(trace): read_page — cursor pagination, limit, last-per-key, every-N"
```

---

### Task 10: `topic_stats` — per-key rollup

**Files:**
- Modify: `crates/zemon-core/src/trace.rs`

**Interfaces:**
- Consumes: `discover_segments`, `load_segment`, `record_in_window`, `segment_overlaps_window`.
- Produces: `pub struct TopicStat` (serde: `key,count,first_ts,last_ts,rate_hz,last_value_preview,last_value_bytes,encoding`); `topic_stats(dir: &Path, filter: &ReadFilter, top: Option<usize>, max_payload_bytes: Option<usize>) -> Result<Vec<TopicStat>, ZemonError>` — sorted by `count` descending, capped to `top`.

- [ ] **Step 1: Write failing tests** (append to `trace.rs` tests)

```rust
    #[test]
    fn topic_stats_rolls_up_per_key() {
        let dir = seed_store("stats1"); // a/x x3 (t1000,1002,1004), b/y x3 (t1001,1003,1005)
        let f = ReadFilter { key: "**".into(), since: None, until: None };
        let stats = topic_stats(&dir, &f, None, Some(64)).unwrap();
        assert_eq!(stats.len(), 2);
        let ax = stats.iter().find(|s| s.key == "a/x").unwrap();
        assert_eq!(ax.count, 3);
        assert_eq!(ax.first_ts.as_deref(), Some("2001-09-09T01:36:40Z")); // rfc3339 of t(1000)
        assert!(ax.rate_hz > 0.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn topic_stats_top_n_by_volume_and_key_filter() {
        let dir = seed_store("stats2");
        let f = ReadFilter { key: "a/*".into(), since: None, until: None };
        let stats = topic_stats(&dir, &f, Some(1), None).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].key, "a/x"); // only a/* matched
        std::fs::remove_dir_all(&dir).ok();
    }
```

(If the exact RFC3339 string in the first test is awkward to predict, assert `ax.first_ts.is_some()` and `ax.count == 3` instead — the point is the rollup, not the literal timestamp.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-core trace::tests::topic_stats`
Expected: FAIL — `topic_stats` / `TopicStat` not defined.

- [ ] **Step 3: Implement** (add to `trace.rs`)

```rust
/// Per-topic rollup for `trace stats`.
#[derive(Debug, Clone, Serialize)]
pub struct TopicStat {
    pub key: String,
    pub count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_ts: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ts: Option<String>,
    pub rate_hz: f64,
    pub last_value_preview: serde_json::Value,
    pub last_value_bytes: usize,
    pub encoding: String,
}

struct Acc {
    count: u64,
    first: Option<SystemTime>,
    last: Option<SystemTime>,
    first_ts: Option<String>,
    last_ts: Option<String>,
    last_payload_b64: String,
    last_encoding: String,
}

pub fn topic_stats(
    dir: &Path,
    filter: &ReadFilter,
    top: Option<usize>,
    max_payload_bytes: Option<usize>,
) -> Result<Vec<TopicStat>, ZemonError> {
    use std::collections::BTreeMap;
    let segs = discover_segments(dir)?;
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();

    for i in 0..segs.len() {
        if !segment_overlaps_window(&segs, i, filter) {
            continue;
        }
        for pr in load_segment(&segs[i].path, is_last_segment(&segs, i))? {
            if !record_in_window(&pr, filter)? {
                continue;
            }
            let e = acc.entry(pr.record.key_expr.clone()).or_insert_with(|| Acc {
                count: 0,
                first: None,
                last: None,
                first_ts: None,
                last_ts: None,
                last_payload_b64: String::new(),
                last_encoding: String::new(),
            });
            e.count += 1;
            if e.first.is_none() {
                e.first = pr.received;
                e.first_ts = pr.record.received_at.clone();
            }
            e.last = pr.received.or(e.last);
            e.last_ts = pr.record.received_at.clone().or(e.last_ts.take());
            e.last_payload_b64 = pr.record.payload_base64.clone();
            e.last_encoding = pr.record.encoding.clone();
        }
    }

    let mut stats: Vec<TopicStat> = acc
        .into_iter()
        .map(|(key, a)| {
            let rate_hz = match (a.first, a.last) {
                (Some(f), Some(l)) if a.count > 1 => {
                    let secs = l.duration_since(f).map(|d| d.as_secs_f64()).unwrap_or(0.0);
                    if secs > 0.0 { a.count as f64 / secs } else { 0.0 }
                }
                _ => 0.0,
            };
            let payload = crate::capture::b64_decode_public(&a.last_payload_b64).unwrap_or_default();
            let mp = crate::types::MessagePayload::from_bytes(payload);
            let last_value_bytes = mp.len();
            let last_value_preview = match max_payload_bytes {
                Some(max) => mp.to_view_capped(max),
                None => mp.to_view(),
            };
            TopicStat {
                key,
                count: a.count,
                first_ts: a.first_ts,
                last_ts: a.last_ts,
                rate_hz,
                last_value_preview,
                last_value_bytes,
                encoding: a.last_encoding,
            }
        })
        .collect();

    stats.sort_by(|a, b| b.count.cmp(&a.count).then(a.key.cmp(&b.key)));
    if let Some(n) = top {
        stats.truncate(n);
    }
    Ok(stats)
}
```

This needs a public base64 decode helper. In `capture.rs`, expose the existing decoder:

```rust
/// Decode a base64 payload string (used by the trace reader). Public wrapper.
pub fn b64_decode_public(s: &str) -> Result<Vec<u8>, ZemonError> {
    b64_decode(s, "payload")
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zemon-core trace && cargo clippy -p zemon-core -- -D warnings`
Expected: PASS. (If the literal-timestamp assertion is brittle, switch it to the `is_some()` form noted in Step 1.)

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-core/src/trace.rs crates/zemon-core/src/capture.rs
git commit -m "feat(trace): topic_stats per-key rollup (count, rate, last value, top-N)"
```

---

### Task 11: CLI — `capture --dir` rotating mode

**Files:**
- Modify: `crates/zemon-cli/src/cli.rs:190-206` (the `Capture` variant)
- Modify: `crates/zemon-cli/src/main.rs:774-854` (the `Capture` handler)

**Interfaces:**
- Consumes: `zemon_core::trace::{SegmentWriter, enforce_retention}`, `parse_byte_size_arg`.
- Produces: `capture` accepts either `--output <file>` (existing single-file) or `--dir <path>` (rotating), mutually exclusive, one required.

- [ ] **Step 1: Write the failing arg-parsing tests** (append to `cli.rs` `mod tests`)

```rust
    #[test]
    fn capture_requires_output_or_dir() {
        assert!(Cli::try_parse_from(["zemon", "capture", "k/**"]).is_err()); // neither
    }

    #[test]
    fn capture_output_and_dir_are_exclusive() {
        assert!(Cli::try_parse_from(["zemon", "capture", "k/**", "-o", "f.ndjson", "--dir", "d"]).is_err());
    }

    #[test]
    fn capture_dir_mode_parses_rotation_and_retention() {
        assert!(Cli::try_parse_from([
            "zemon", "capture", "k/**", "--dir", "d",
            "--rotate-size", "64MB", "--rotate-interval", "1h",
            "--max-total-size", "1GB", "--max-age", "7d"
        ]).is_ok());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-cli capture_`
Expected: FAIL — options don't exist; `--output` still required-by-absence but exclusivity/`--dir` missing.

- [ ] **Step 3: Implement the CLI variant** — replace the `Capture` variant in `cli.rs`:

```rust
    /// Record received messages to a versioned NDJSON trace.
    ///
    /// `--output` writes one file (pairs with `replay`). `--dir` writes a
    /// rotating segment store with retention (pairs with `trace`), suitable for
    /// an always-on recorder.
    Capture {
        /// Key expression to subscribe and record
        key_expr: String,

        /// Single-file output (NDJSON). Mutually exclusive with --dir.
        #[arg(long, short, required_unless_present = "dir", conflicts_with = "dir")]
        output: Option<PathBuf>,

        /// Rotating segment-store directory. Mutually exclusive with --output.
        #[arg(long, required_unless_present = "output")]
        dir: Option<PathBuf>,

        /// Rotate the active segment once it reaches this size (dir mode)
        #[arg(long, default_value = "64MB", value_parser = crate::duration::parse_byte_size_arg)]
        rotate_size: u64,

        /// …or once it is this old (dir mode)
        #[arg(long, default_value = "1h", value_parser = crate::duration::parse_duration_arg)]
        rotate_interval: Duration,

        /// Retention: delete oldest closed segments over this total size (dir mode)
        #[arg(long, default_value = "1GB", value_parser = crate::duration::parse_byte_size_arg)]
        max_total_size: u64,

        /// Retention: delete closed segments older than this (dir mode)
        #[arg(long, default_value = "7d", value_parser = crate::duration::parse_duration_arg)]
        max_age: Duration,

        /// Stop after N recorded messages
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        count: Option<u64>,

        /// Stop after this much time (e.g. 30s)
        #[arg(long, value_parser = crate::duration::parse_duration_arg)]
        duration: Option<Duration>,
    },
```

- [ ] **Step 4: Implement the handler** — replace the `Command::Capture { .. } => { .. }` arm in `main.rs`. Keep the existing single-file path when `output` is `Some`; add the rotating path when `dir` is `Some`:

```rust
        Command::Capture {
            key_expr,
            output,
            dir,
            rotate_size,
            rotate_interval,
            max_total_size,
            max_age,
            count,
            duration,
        } => {
            use zemon_core::capture::CaptureRecord;

            let session = zemon_core::session::open_session(&config).await?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let _handle = zemon_core::subscriber::subscribe(&session, &key_expr, tx).await?;

            let start = std::time::Instant::now();
            let mut budget = watch::Budget::start(watch::Bounds::new(count, duration));
            let mut written: u64 = 0;
            let mut stop = false;

            // Two sinks: single file (with replay) or a rotating segment store.
            enum Sink {
                File(std::io::BufWriter<std::fs::File>),
                Dir { writer: zemon_core::trace::SegmentWriter, dir: std::path::PathBuf, max_total_size: u64, max_age: std::time::Duration },
            }
            let mut sink = if let Some(dir) = dir.clone() {
                let writer = zemon_core::trace::SegmentWriter::open(dir.clone(), rotate_size, rotate_interval)?;
                if !cli.json {
                    eprintln!("Recording '{}' to {} (rotating) ... (Ctrl+C to stop)", key_expr, dir.display());
                }
                Sink::Dir { writer, dir, max_total_size, max_age }
            } else {
                let out = output.clone().expect("clap guarantees output or dir");
                let file = std::fs::File::create(&out).map_err(|e| {
                    ZemonError::invalid_input(format!("cannot create {}: {}", out.display(), e))
                })?;
                if !cli.json {
                    eprintln!("Capturing '{}' to {} ... (Ctrl+C to stop)", key_expr, out.display());
                }
                Sink::File(std::io::BufWriter::new(file))
            };

            loop {
                let deadline = budget.deadline();
                tokio::select! {
                    biased;
                    _ = watch::sleep_until_opt(deadline) => break,
                    _ = tokio::signal::ctrl_c() => { if !cli.json { eprintln!("\nStopped."); } break; }
                    item = rx.recv() => match item {
                        Some(msg) => {
                            let now = std::time::SystemTime::now();
                            let rec = CaptureRecord::from_message(&msg, start.elapsed(), now);
                            let line = serde_json::to_string(&rec)?;
                            match &mut sink {
                                Sink::File(w) => {
                                    use std::io::Write as _;
                                    writeln!(w, "{}", line).map_err(|e| ZemonError::internal(format!("write failed: {}", e)))?;
                                }
                                Sink::Dir { writer, dir, max_total_size, max_age } => {
                                    writer.write_line(&line, now)?;
                                    // Cheap: retention runs on each write; it early-returns when nothing to prune.
                                    zemon_core::trace::enforce_retention(dir, Some(*max_total_size), Some(*max_age), now)?;
                                }
                            }
                            written += 1;
                            if budget.record() { stop = true; }
                        }
                        None => break,
                    }
                }
                if stop { break; }
            }

            // Flush.
            let output_label = match &mut sink {
                Sink::File(w) => {
                    use std::io::Write as _;
                    w.flush().map_err(|e| ZemonError::internal(format!("flush failed: {}", e)))?;
                    output.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
                }
                Sink::Dir { writer, dir, .. } => {
                    writer.flush()?;
                    dir.display().to_string()
                }
            };

            if cli.json {
                println!("{}", serde_json::to_string(&serde_json::json!({
                    "ok": true, "captured": written, "output": output_label,
                }))?);
            } else {
                eprintln!("Captured {} record(s) to {}", written, output_label);
            }
            session.close().await.map_err(|e| color_eyre::eyre::eyre!(e))?;
        }
```

Note: running `enforce_retention` on every write is simple and correct; if profiling shows it is hot, gate it behind a rotation event later (out of scope now).

- [ ] **Step 5: Run tests + manual smoke**

Run: `cargo test -p zemon-cli capture_ && cargo build -p zemon-cli && cargo clippy --all -- -D warnings`
Expected: PASS; builds; no warnings. Manual (needs `zenohd`): `zemon capture "test/**" --dir ./trace --rotate-size 1KB` in one terminal, `zemon pub test/a '{"n":1}'` in another; confirm `./trace/zemon-trace-*.ndjson` appears.

- [ ] **Step 6: Commit**

```bash
git add crates/zemon-cli/src/cli.rs crates/zemon-cli/src/main.rs
git commit -m "feat(capture): rotating --dir store mode with retention"
```

---

### Task 12: CLI — `trace stats` / `trace read` + README

**Files:**
- Modify: `crates/zemon-cli/src/cli.rs` (add `Command::Trace` + `TraceCommand`)
- Modify: `crates/zemon-cli/src/main.rs` (add the `Trace` handler)
- Modify: `README.md`

**Interfaces:**
- Consumes: `zemon_core::trace::{ReadFilter, ReadOptions, read_page, topic_stats, parse_time_bound}`, `zemon_core::output::to_collection_json_limited`.
- Produces: `zemon [--json] trace stats <dir> [...]` and `zemon [--json] trace read <dir> [...]`.

- [ ] **Step 1: Write the failing arg-parsing tests** (append to `cli.rs` `mod tests`)

```rust
    #[test]
    fn trace_stats_and_read_parse() {
        assert!(Cli::try_parse_from(["zemon", "trace", "stats", "d"]).is_ok());
        assert!(Cli::try_parse_from(["zemon", "trace", "stats", "d", "--key", "a/*", "--since", "10m", "--top", "5"]).is_ok());
        assert!(Cli::try_parse_from(["zemon", "trace", "read", "d", "--key", "a/*", "--limit", "50", "--last-per-key"]).is_ok());
        assert!(Cli::try_parse_from(["zemon", "trace", "read", "d", "--every", "10", "--cursor", "abc"]).is_ok());
    }

    #[test]
    fn trace_requires_a_subcommand() {
        assert!(Cli::try_parse_from(["zemon", "trace"]).is_err());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zemon-cli trace_`
Expected: FAIL — `trace` command absent.

- [ ] **Step 3: Add the CLI types** in `cli.rs` — add a `Command::Trace` variant and a `TraceCommand` enum:

```rust
    /// Read a rotating capture store (`capture --dir`). Pure, no network.
    Trace {
        #[command(subcommand)]
        command: TraceCommand,
    },
```

```rust
#[derive(Subcommand, Debug)]
pub enum TraceCommand {
    /// Per-topic rollup: count, rate, last value, time span.
    Stats {
        /// Trace store directory
        dir: PathBuf,
        /// Key expression filter (default "**")
        #[arg(long, default_value = "**")]
        key: String,
        /// Window start: relative (e.g. 10m) or RFC3339
        #[arg(long)]
        since: Option<String>,
        /// Window end: relative or RFC3339
        #[arg(long)]
        until: Option<String>,
        /// Return only the N highest-volume topics
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        top: Option<u64>,
        /// Cap last-value preview to N bytes
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        max_payload_bytes: Option<u64>,
    },
    /// Filtered raw records as NDJSON, bounded and paginated.
    Read {
        /// Trace store directory
        dir: PathBuf,
        /// Key expression filter (default "**")
        #[arg(long, default_value = "**")]
        key: String,
        /// Window start: relative (e.g. 10m) or RFC3339
        #[arg(long)]
        since: Option<String>,
        /// Window end: relative or RFC3339
        #[arg(long)]
        until: Option<String>,
        /// Max records to return (0 = unbounded; use with care)
        #[arg(long, default_value = "100")]
        limit: u64,
        /// Collapse to the latest record per key (whole-window, ignores cursor)
        #[arg(long)]
        last_per_key: bool,
        /// Sample every Nth matching record (whole-window, ignores cursor)
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        every: Option<u64>,
        /// Cap each record's payload/attachment preview to N bytes
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        max_payload_bytes: Option<u64>,
        /// Resume from a previous page's cursor
        #[arg(long)]
        cursor: Option<String>,
    },
}
```

- [ ] **Step 4: Add the handler** in `main.rs` `run()` match (place near `Command::Keyexpr`, before `Command::Capture`):

```rust
        Command::Trace { command } => {
            use zemon_core::trace::{self, ReadFilter, ReadOptions};
            let now = std::time::SystemTime::now();
            let parse_bound = |o: &Option<String>| -> Result<Option<std::time::SystemTime>, ZemonError> {
                o.as_deref().map(|s| trace::parse_time_bound(s, now)).transpose()
            };
            match command {
                TraceCommand::Stats { dir, key, since, until, top, max_payload_bytes } => {
                    let filter = ReadFilter { key, since: parse_bound(&since)?, until: parse_bound(&until)? };
                    let stats = trace::topic_stats(
                        &dir, &filter,
                        top.map(|n| n as usize),
                        max_payload_bytes.map(|n| n as usize),
                    )?;
                    if cli.json {
                        println!("{}", zemon_core::output::to_collection_json(&stats)?);
                    } else if stats.is_empty() {
                        println!("No records in store for '{}'", filter.key);
                    } else {
                        for s in &stats {
                            println!("{:<40} count={:<8} rate={:.2}Hz last={}", s.key, s.count, s.rate_hz, s.last_ts.as_deref().unwrap_or("-"));
                        }
                    }
                }
                TraceCommand::Read { dir, key, since, until, limit, last_per_key, every, max_payload_bytes, cursor } => {
                    let filter = ReadFilter { key, since: parse_bound(&since)?, until: parse_bound(&until)? };
                    let opts = ReadOptions {
                        filter,
                        limit: if limit == 0 { None } else { Some(limit) },
                        last_per_key,
                        every,
                        cursor,
                    };
                    let page = trace::read_page(&dir, &opts)?;
                    let max = max_payload_bytes.map(|n| n as usize);
                    if cli.json {
                        // NDJSON: one record per line, then a summary trailer line.
                        for pr in &page.records {
                            let mut v = serde_json::to_value(&pr.record)?;
                            if let (Some(max), Some(obj)) = (max, v.as_object_mut()) {
                                // Re-cap the payload preview from raw bytes.
                                if let Ok(bytes) = zemon_core::capture::b64_decode_public(&pr.record.payload_base64) {
                                    let mp = zemon_core::types::MessagePayload::from_bytes(bytes);
                                    obj.insert("payload".to_string(), mp.to_view_capped(max));
                                }
                            }
                            println!("{}", serde_json::to_string(&v)?);
                        }
                        println!("{}", serde_json::to_string(&serde_json::json!({
                            "summary": {
                                "returned": page.returned,
                                "matched": page.matched,
                                "truncated": page.truncated,
                                "cursor": page.cursor,
                            }
                        }))?);
                    } else {
                        for pr in &page.records {
                            println!("{}  {}", pr.record.received_at.as_deref().unwrap_or("-"), pr.record.key_expr);
                        }
                        eprintln!("returned {} of {} matched{}", page.returned, page.matched,
                            if page.truncated { " (more — pass --cursor to continue)" } else { "" });
                    }
                }
            }
        }
```

Note: the `Trace` command is pure — it must NOT call `open_session`. It runs entirely on files; place the arm so it returns without touching `config`.

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test --all && cargo clippy --all -- -D warnings`
Expected: PASS across the workspace; no warnings.

- [ ] **Step 6: Manual end-to-end smoke** (needs `zenohd`)

```bash
zenohd &
zemon capture "test/**" --dir ./trace --rotate-size 4KB --rotate-interval 10s &
for i in 1 2 3; do zemon pub test/a "{\"n\":$i}"; done
zemon --json trace stats ./trace                 # {"count":1,"items":[{"key":"test/a","count":3,...}]}
zemon --json trace read ./trace --key "test/**" --limit 2   # 2 NDJSON records + summary trailer with cursor
```

- [ ] **Step 7: Update README** — add to the "CLI Usage" and "Agent-friendly output contracts" sections:

```markdown
# Continuously record a rotating store (run under a supervisor for always-on)
zemon capture "sensor/**" --dir ./trace --rotate-size 64MB --rotate-interval 1h \
  --max-total-size 1GB --max-age 7d

# Read the store WITHOUT a live subscription (pure, offline) — inspect the past
zemon --json trace stats ./trace --since 1h --top 20        # per-topic rollup
zemon --json trace read  ./trace --key "sensor/**" --since 10m --limit 100
zemon --json trace read  ./trace --last-per-key             # latest value per topic
```

Add a short "Time-shifted inspection" note: the collector (`capture --dir`) is a long-lived process supervised by the OS (Windows Task Scheduler / `nssm`); the reader (`trace`) is pure and never opens a Zenoh session, so an agent can read what happened while it was absent. `trace read` is bounded by `--limit` (default 100) and reports `{returned, matched, cursor, truncated}` so results are never silently truncated.

- [ ] **Step 8: Commit**

```bash
git add crates/zemon-cli/src/cli.rs crates/zemon-cli/src/main.rs README.md
git commit -m "feat(trace): trace stats/read CLI subcommands + docs"
```

---

## Self-Review

**Spec coverage:**
- Collector lifecycle (user-supervised, `capture --dir`) → Tasks 5, 11. ✓
- Chunked NDJSON store, segment naming, `[first,next_first)` bounds → Tasks 3, 4. ✓
- Rotation (size 64MB OR interval 1h) → Task 5, 11. ✓
- Retention (total 1GB + age 7d, oldest closed first, active protected) → Tasks 6, 11. ✓
- Record schema v2 `received_at`, `{1,2}` parse → Task 1. ✓
- `trace stats` `{count,items}` rollup with `--key/--since/--until/--top`, payload cap → Tasks 10, 12. ✓
- `trace read` NDJSON, default `--limit 100`, `--last-per-key`, `--every N`, payload cap, cursor, `matched/returned/truncated` trailer → Tasks 9, 12. ✓
- Time-window segment skip → Task 9 (`segment_overlaps_window`), used by 9 & 10. ✓
- Errors: `invalid_input`/`not_found`, empty≠error → Tasks 4, 8, 12. ✓
- Purity / reuse (`output` envelope, `ZemonError`, no session in reader) → Global Constraints, Task 12 note. ✓
- Byte-size parser → Task 2. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code; the one brittle literal-timestamp assertion (Task 10 Step 1) has an explicit fallback. ✓

**Type consistency:** `ReadFilter`/`ReadOptions`/`ReadPage`/`PositionedRecord`/`Segment`/`TopicStat` names match across Tasks 4–12; `from_message(msg, offset, received_at)` used consistently after Task 1; `b64_decode_public` defined in Task 10 and used in Tasks 10 & 12; `segment_overlaps_window`/`is_last_segment` defined in Task 9 and reused in Task 10. ✓

**Out of scope (unchanged):** daemon subcommands, SQLite backend, replay-over-dir, stats `--bucket`, MCP wrapping — none appear as tasks. ✓
