//! Chunked-NDJSON capture store + pure reader (`trace stats` / `trace read`).
//!
//! A long-lived `capture --dir` process appends [`CaptureRecord`] lines into
//! rotating segment files; this module owns the segment filename codec,
//! discovery, the rotating writer, retention, and the pure reader. Nothing here
//! opens a Zenoh session.
//!
//! [`CaptureRecord`]: crate::capture::CaptureRecord

use crate::capture::CaptureRecord;
use crate::error::ZenmonError;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const SEG_PREFIX: &str = "zenmon-trace-";
const SEG_EXT: &str = ".ndjson";
const SEG_EXT_ZSTD: &str = ".ndjson.zst";

/// zstd level for captures. Measured on a dotori robot's full `**` traffic:
/// level 3 gives 3.4x overall (17x without point clouds) at ~400 MB/s on one
/// core — orders of magnitude above a capture's ~1 MB/s. Higher levels bought
/// 10–20% more at a quarter of the speed or less.
const ZSTD_LEVEL: i32 = 3;

/// How often an open zstd segment is flushed to a decodable block boundary.
/// A zstd frame is only fully readable once finished, so between flushes a
/// crash (or a reader on the active segment) sees nothing of the unflushed
/// tail. One second bounds that loss; at capture rates the per-block overhead
/// is negligible.
const ZSTD_FLUSH_INTERVAL: Duration = Duration::from_secs(1);

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// On-disk encoding of capture files. Readers detect it from the file's magic
/// bytes, not its name, so either kind reads the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    /// Plain NDJSON (`.ndjson`).
    #[default]
    None,
    /// zstd-compressed NDJSON (`.ndjson.zst`).
    Zstd,
}

impl Compression {
    fn segment_ext(self) -> &'static str {
        match self {
            Compression::None => SEG_EXT,
            Compression::Zstd => SEG_EXT_ZSTD,
        }
    }
}

impl std::str::FromStr for Compression {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Compression::None),
            "zstd" => Ok(Compression::Zstd),
            other => Err(format!("unknown compression '{}' (expected none or zstd)", other)),
        }
    }
}

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
        &s[0..4],
        &s[4..6],
        &s[6..8],
        &s[9..11],
        &s[11..13],
        &s[13..15]
    );
    humantime::parse_rfc3339(&rfc).ok()
}

/// `zenmon-trace-<stamp>-<seq:05>.ndjson`.
pub fn segment_file_name(first: SystemTime, seq: u32) -> String {
    segment_file_name_with(first, seq, Compression::None)
}

/// [`segment_file_name`] with the extension for `compression`
/// (`.ndjson` or `.ndjson.zst`).
pub fn segment_file_name_with(first: SystemTime, seq: u32, compression: Compression) -> String {
    format!(
        "{}{}-{:05}{}",
        SEG_PREFIX,
        format_segment_stamp(first),
        seq,
        compression.segment_ext()
    )
}

/// Parse a segment filename into `(first_timestamp, seq)`. Non-segment files
/// return `None` (so a directory may hold unrelated files harmlessly).
pub fn parse_segment_file_name(name: &str) -> Option<(SystemTime, u32)> {
    let rest = name.strip_prefix(SEG_PREFIX)?;
    // Both kinds are segments: retention must count and prune compressed
    // ones, and readers must see them, or they would pile up unnoticed.
    let core = rest
        .strip_suffix(SEG_EXT_ZSTD)
        .or_else(|| rest.strip_suffix(SEG_EXT))?;
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
pub fn discover_segments(dir: &Path) -> Result<Vec<Segment>, ZenmonError> {
    let entries = std::fs::read_dir(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            ZenmonError::not_found(format!("trace directory not found: {}", dir.display()))
        }
        _ => ZenmonError::internal(format!("cannot read {}: {}", dir.display(), e)),
    })?;
    let mut segs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| ZenmonError::internal(e.to_string()))?;
        let name = entry.file_name();
        if let Some((first, seq)) = parse_segment_file_name(&name.to_string_lossy()) {
            segs.push(Segment {
                path: entry.path(),
                first,
                seq,
            });
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

/// `Write` adapter counting bytes that reach the file — for a zstd segment,
/// the compressed size that rotation and retention should see.
struct CountingWriter<W> {
    inner: W,
    count: u64,
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

enum FileSink {
    Plain(BufWriter<File>),
    Zstd(zstd::stream::write::Encoder<'static, CountingWriter<BufWriter<File>>>),
}

/// One capture file being written, plain or zstd. Used by [`SegmentWriter`]
/// and by `capture --output`. Call [`CaptureFileWriter::finish`] when done: a
/// zstd frame is only complete once finished (dropping finishes it too, but
/// swallows any error).
pub struct CaptureFileWriter {
    sink: Option<FileSink>,
    /// Uncompressed bytes written (line lengths + newlines).
    raw_bytes: u64,
    last_flush: SystemTime,
}

fn io_err(what: &str, path: Option<&Path>, e: std::io::Error) -> ZenmonError {
    match path {
        Some(p) => ZenmonError::internal(format!("{} {}: {}", what, p.display(), e)),
        None => ZenmonError::internal(format!("{}: {}", what, e)),
    }
}

impl CaptureFileWriter {
    pub fn create(path: &Path, compression: Compression) -> Result<Self, ZenmonError> {
        let file = File::create(path).map_err(|e| io_err("cannot create", Some(path), e))?;
        let file = BufWriter::new(file);
        let sink = match compression {
            Compression::None => FileSink::Plain(file),
            Compression::Zstd => {
                let counting = CountingWriter {
                    inner: file,
                    count: 0,
                };
                FileSink::Zstd(
                    zstd::stream::write::Encoder::new(counting, ZSTD_LEVEL)
                        .map_err(|e| io_err("cannot start zstd in", Some(path), e))?,
                )
            }
        };
        Ok(Self {
            sink: Some(sink),
            raw_bytes: 0,
            last_flush: SystemTime::now(),
        })
    }

    /// Append one line (a newline is added). A zstd file is flushed to a
    /// block boundary at most every [`ZSTD_FLUSH_INTERVAL`] (by `now`).
    pub fn write_line(&mut self, line: &str, now: SystemTime) -> Result<(), ZenmonError> {
        let sink = self.sink.as_mut().expect("write after finish");
        let res = match sink {
            FileSink::Plain(w) => writeln!(w, "{}", line),
            FileSink::Zstd(w) => writeln!(w, "{}", line),
        };
        res.map_err(|e| io_err("write failed", None, e))?;
        self.raw_bytes += line.len() as u64 + 1;
        if let FileSink::Zstd(w) = sink {
            let due = now
                .duration_since(self.last_flush)
                .map(|d| d >= ZSTD_FLUSH_INTERVAL)
                .unwrap_or(true);
            if due {
                w.flush().map_err(|e| io_err("flush failed", None, e))?;
                self.last_flush = now;
            }
        }
        Ok(())
    }

    /// Bytes on disk so far: the raw size for plain files, the compressed
    /// size (up to the last emitted block) for zstd.
    pub fn file_bytes(&self) -> u64 {
        match &self.sink {
            Some(FileSink::Plain(_)) | None => self.raw_bytes,
            Some(FileSink::Zstd(w)) => w.get_ref().count,
        }
    }

    /// Uncompressed bytes written.
    pub fn raw_bytes(&self) -> u64 {
        self.raw_bytes
    }

    /// Make everything written so far readable (a block boundary for zstd).
    pub fn flush(&mut self) -> Result<(), ZenmonError> {
        let res = match self.sink.as_mut() {
            Some(FileSink::Plain(w)) => w.flush(),
            Some(FileSink::Zstd(w)) => w.flush(),
            None => Ok(()),
        };
        res.map_err(|e| io_err("flush failed", None, e))
    }

    /// Complete the file (ends the zstd frame) and flush it. Idempotent.
    pub fn finish(&mut self) -> Result<(), ZenmonError> {
        let res = match self.sink.take() {
            Some(FileSink::Plain(mut w)) => w.flush(),
            Some(FileSink::Zstd(w)) => w.finish().and_then(|mut c| c.inner.flush()),
            None => Ok(()),
        };
        res.map_err(|e| io_err("finish failed", None, e))
    }
}

impl Drop for CaptureFileWriter {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// Appends NDJSON lines into rotating segment files under a directory.
pub struct SegmentWriter {
    dir: PathBuf,
    rotate_size: u64,
    rotate_interval: Duration,
    compression: Compression,
    writer: Option<CaptureFileWriter>,
    seg_first: SystemTime,
    next_seq: u32,
}

impl SegmentWriter {
    pub fn open(
        dir: PathBuf,
        rotate_size: u64,
        rotate_interval: Duration,
    ) -> Result<Self, ZenmonError> {
        Self::open_with(dir, rotate_size, rotate_interval, Compression::None)
    }

    /// [`SegmentWriter::open`] writing `compression` segments. `rotate_size`
    /// is compared against bytes on disk, so for zstd it bounds the
    /// compressed segment size — the same unit retention counts in.
    pub fn open_with(
        dir: PathBuf,
        rotate_size: u64,
        rotate_interval: Duration,
        compression: Compression,
    ) -> Result<Self, ZenmonError> {
        std::fs::create_dir_all(&dir).map_err(|e| {
            ZenmonError::invalid_input(format!("cannot create {}: {}", dir.display(), e))
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
            compression,
            writer: None,
            seg_first: SystemTime::UNIX_EPOCH,
            next_seq,
        })
    }

    fn should_rotate(&self, now: SystemTime) -> bool {
        let Some(w) = self.writer.as_ref() else {
            return true;
        };
        if w.file_bytes() >= self.rotate_size {
            return true;
        }
        now.duration_since(self.seg_first)
            .map(|elapsed| elapsed >= self.rotate_interval)
            .unwrap_or(false)
    }

    fn rotate(&mut self, now: SystemTime) -> Result<(), ZenmonError> {
        if let Some(mut w) = self.writer.take() {
            w.finish()?;
        }
        let name = segment_file_name_with(now, self.next_seq, self.compression);
        self.next_seq += 1;
        let path = self.dir.join(name);
        self.writer = Some(CaptureFileWriter::create(&path, self.compression)?);
        self.seg_first = now;
        Ok(())
    }

    /// Append one line (a newline is added). Rotates first if the current
    /// segment is full or too old.
    pub fn write_line(&mut self, line: &str, now: SystemTime) -> Result<(), ZenmonError> {
        if self.should_rotate(now) {
            self.rotate(now)?;
        }
        let w = self.writer.as_mut().expect("writer present after rotate");
        w.write_line(line, now)
    }

    pub fn flush(&mut self) -> Result<(), ZenmonError> {
        match self.writer.as_mut() {
            Some(w) => w.flush(),
            None => Ok(()),
        }
    }

    /// Finish the active segment (ends its zstd frame). Call when the
    /// capture stops; a later `write_line` starts a new segment.
    pub fn close(&mut self) -> Result<(), ZenmonError> {
        match self.writer.take() {
            Some(mut w) => w.finish(),
            None => Ok(()),
        }
    }
}

/// Lines of a capture file, plain or zstd (detected by magic bytes).
///
/// A zstd file whose data ends mid-frame — the active segment, or one cut off
/// by a crash — ends the iteration at the last complete line instead of
/// erroring: everything up to the last flushed block is intact, and the lost
/// tail is at most [`ZSTD_FLUSH_INTERVAL`] of records. Corrupt data is still
/// an error.
pub struct CaptureLines {
    reader: Box<dyn BufRead>,
    compressed: bool,
    done: bool,
}

/// Open a capture file for line reading. See [`CaptureLines`].
pub fn open_capture_lines(path: &Path) -> Result<CaptureLines, ZenmonError> {
    use std::io::Read as _;
    let mut file = File::open(path).map_err(|e| io_err("cannot open", Some(path), e))?;
    let mut magic = [0u8; 4];
    let n = file
        .read(&mut magic)
        .map_err(|e| io_err("cannot read", Some(path), e))?;
    let compressed = n == 4 && magic == ZSTD_MAGIC;
    let file = std::io::Cursor::new(magic[..n].to_vec()).chain(file);
    let reader: Box<dyn BufRead> = if compressed {
        Box::new(BufReader::new(
            zstd::stream::read::Decoder::new(file)
                .map_err(|e| io_err("cannot start zstd for", Some(path), e))?,
        ))
    } else {
        Box::new(BufReader::new(file))
    };
    Ok(CaptureLines {
        reader,
        compressed,
        done: false,
    })
}

impl CaptureLines {
    /// Whether the file is zstd-compressed.
    pub fn is_compressed(&self) -> bool {
        self.compressed
    }
}

impl Iterator for CaptureLines {
    type Item = Result<String, ZenmonError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut buf = String::new();
        match self.reader.read_line(&mut buf) {
            Ok(0) => {
                self.done = true;
                None
            }
            Ok(_) => {
                if buf.ends_with('\n') {
                    buf.pop();
                    if buf.ends_with('\r') {
                        buf.pop();
                    }
                }
                Some(Ok(buf))
            }
            // An unfinished zstd frame: stop at the last complete line.
            Err(e) if self.compressed && e.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(io_err("read failed", None, e)))
            }
        }
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
) -> Result<u64, ZenmonError> {
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
            std::fs::remove_file(&seg.path).map_err(|e| {
                ZenmonError::internal(format!("cannot remove {}: {}", seg.path.display(), e))
            })?;
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
) -> Result<Vec<PositionedRecord>, ZenmonError> {
    let segment = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lines: Vec<String> = open_capture_lines(path)?.collect::<Result<_, _>>()?;

    let mut out = Vec::with_capacity(lines.len());
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match CaptureRecord::parse_line(line, i + 1) {
            Ok(record) => {
                let received = parse_received(&record);
                out.push(PositionedRecord {
                    segment: segment.clone(),
                    index: i as u64,
                    record,
                    received,
                });
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
pub fn key_matches(filter_key: &str, record_key: &str) -> Result<bool, ZenmonError> {
    use zenoh::key_expr::KeyExpr;
    let filter = KeyExpr::try_from(filter_key).map_err(|e| {
        ZenmonError::invalid_input(format!("invalid key expression '{}': {}", filter_key, e))
    })?;
    // A stored key is always a concrete key; if it fails to parse, treat as no-match.
    match KeyExpr::try_from(record_key) {
        Ok(rk) => Ok(filter.intersects(&rk)),
        Err(_) => Ok(false),
    }
}

/// Parse `--since` / `--until`: a relative duration (interpreted as `now - dur`)
/// or an absolute RFC3339 timestamp.
pub fn parse_time_bound(s: &str, now: SystemTime) -> Result<SystemTime, ZenmonError> {
    let t = s.trim();
    if let Ok(dur) = humantime::parse_duration(t) {
        return now.checked_sub(dur).ok_or_else(|| {
            ZenmonError::invalid_input(format!("time '{}' is before the epoch", s))
        });
    }
    humantime::parse_rfc3339(t).map_err(|e| {
        ZenmonError::invalid_input(format!(
            "invalid time '{}': {} (try 10m or an RFC3339 timestamp)",
            s, e
        ))
    })
}

/// True if a record satisfies the filter's key and time window. A record with
/// no `received` time (v1) is time-unbounded (passes any since/until).
pub fn record_in_window(pr: &PositionedRecord, filter: &ReadFilter) -> Result<bool, ZenmonError> {
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
    let json = serde_json::to_string(&CursorInner {
        segment: segment.to_string(),
        index,
    })
    .unwrap_or_default();
    base64::engine::general_purpose::STANDARD.encode(json)
}

pub fn decode_cursor(s: &str) -> Result<(String, u64), ZenmonError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| ZenmonError::invalid_input(format!("invalid cursor: {}", e)))?;
    let inner: CursorInner = serde_json::from_slice(&bytes)
        .map_err(|e| ZenmonError::invalid_input(format!("invalid cursor: {}", e)))?;
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

pub fn read_page(dir: &Path, opts: &ReadOptions) -> Result<ReadPage, ZenmonError> {
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

    for (i, seg) in segs.iter().enumerate() {
        if !segment_overlaps_window(&segs, i, &opts.filter) {
            continue;
        }
        let loaded = load_segment(&seg.path, is_last_segment(&segs, i))?;
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
    Ok(ReadPage {
        records,
        matched,
        returned,
        cursor,
        truncated,
    })
}

fn read_last_per_key(segs: &[Segment], opts: &ReadOptions) -> Result<ReadPage, ZenmonError> {
    use std::collections::BTreeMap;
    let mut latest: BTreeMap<String, PositionedRecord> = BTreeMap::new();
    for (i, seg) in segs.iter().enumerate() {
        if !segment_overlaps_window(segs, i, &opts.filter) {
            continue;
        }
        for pr in load_segment(&seg.path, is_last_segment(segs, i))? {
            if record_in_window(&pr, &opts.filter)? {
                latest.insert(pr.record.key_expr.clone(), pr); // later segments overwrite → last wins
            }
        }
    }
    finalize_reduced(latest.into_values().collect(), opts.limit)
}

fn read_every_n(segs: &[Segment], opts: &ReadOptions, n: u64) -> Result<ReadPage, ZenmonError> {
    let mut sampled = Vec::new();
    let mut seen: u64 = 0;
    for (i, seg) in segs.iter().enumerate() {
        if !segment_overlaps_window(segs, i, &opts.filter) {
            continue;
        }
        for pr in load_segment(&seg.path, is_last_segment(segs, i))? {
            if record_in_window(&pr, &opts.filter)? {
                if seen.is_multiple_of(n) {
                    sampled.push(pr);
                }
                seen += 1;
            }
        }
    }
    finalize_reduced(sampled, opts.limit)
}

fn finalize_reduced(
    all: Vec<PositionedRecord>,
    limit: Option<u64>,
) -> Result<ReadPage, ZenmonError> {
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

/// Roll up the store per concrete key: count, observed rate, first/last receive
/// time, and the latest value (preview capped to `max_payload_bytes`). Sorted
/// by `count` descending, capped to `top`.
pub fn topic_stats(
    dir: &Path,
    filter: &ReadFilter,
    top: Option<usize>,
    max_payload_bytes: Option<usize>,
) -> Result<Vec<TopicStat>, ZenmonError> {
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
            let e = acc
                .entry(pr.record.key_expr.clone())
                .or_insert_with(|| Acc {
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
                    if secs > 0.0 {
                        a.count as f64 / secs
                    } else {
                        0.0
                    }
                }
                _ => 0.0,
            };
            let payload =
                crate::capture::b64_decode_public(&a.last_payload_b64).unwrap_or_default();
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
        let dir = std::env::temp_dir().join(format!(
            "zenmon-trace-test-{}-{}-{}",
            tag,
            std::process::id(),
            n
        ));
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
        assert!(name.starts_with("zenmon-trace-"));
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
        assert_eq!(parse_segment_file_name("zenmon-trace-bad.ndjson"), None);
    }

    #[test]
    fn parse_segment_stamp_rejects_non_ascii_without_panic() {
        // Build a 16-BYTE non-ASCII string ('é' is 2 bytes) that passes the
        // byte-length check; must return None, not panic on char-boundary slicing.
        let crafted = format!("ABC\u{00e9}DEFTGHIJKL{}", "Z");
        assert_eq!(crafted.len(), 16);
        assert_eq!(parse_segment_stamp(&crafted), None);
        // And a segment-shaped filename with such a stamp must also be ignored, not panic.
        let name = format!("zenmon-trace-{}-00001.ndjson", crafted);
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
        let deleted =
            enforce_retention(&dir, None, Some(Duration::from_secs(1000)), t(3600)).unwrap();
        assert_eq!(deleted, 1);
        let segs = discover_segments(&dir).unwrap();
        assert_eq!(segs[0].first, t(2000));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zstd_segment_roundtrips_and_is_discovered() {
        let dir = tempdir_unique("zstdrt");
        let mut w = SegmentWriter::open_with(
            dir.clone(),
            1 << 30,
            Duration::from_secs(3600),
            Compression::Zstd,
        )
        .unwrap();
        let a = rec_line("a/b", 1000);
        let b = rec_line("a/c", 1001);
        w.write_line(&a, t(1000)).unwrap();
        w.write_line(&b, t(1001)).unwrap();
        w.close().unwrap();
        let segs = discover_segments(&dir).unwrap();
        assert_eq!(segs.len(), 1);
        assert!(segs[0].path.to_string_lossy().ends_with(".ndjson.zst"));
        let recs = load_segment(&segs[0].path, false).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1].record.key_expr, "a/c");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zstd_unfinished_segment_reads_up_to_last_flush() {
        // A crash (or the live active segment) leaves the frame unfinished;
        // everything flushed must still read, without an error.
        let dir = tempdir_unique("zstdopen");
        let mut w = SegmentWriter::open_with(
            dir.clone(),
            1 << 30,
            Duration::from_secs(3600),
            Compression::Zstd,
        )
        .unwrap();
        w.write_line(&rec_line("a/b", 1000), t(1000)).unwrap();
        w.write_line(&rec_line("a/c", 1002), t(1002)).unwrap();
        w.flush().unwrap();
        std::mem::forget(w); // no finish: simulates a crash mid-segment
        let segs = discover_segments(&dir).unwrap();
        let recs = load_segment(&segs[0].path, false).unwrap();
        assert_eq!(recs.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zstd_corrupt_data_is_an_error() {
        let dir = tempdir_unique("zstdbad");
        let path = dir.join(segment_file_name_with(t(1000), 0, Compression::Zstd));
        let mut bytes = ZSTD_MAGIC.to_vec();
        bytes.extend_from_slice(&[0xFF; 64]);
        std::fs::write(&path, bytes).unwrap();
        assert!(load_segment(&path, true).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zstd_rotation_counts_compressed_bytes() {
        let dir = tempdir_unique("zstdrot");
        let mut w = SegmentWriter::open_with(
            dir.clone(),
            4096,
            Duration::from_secs(3600),
            Compression::Zstd,
        )
        .unwrap();
        let line = rec_line("a/b", 1000); // compresses to almost nothing
        for i in 0..200 {
            w.write_line(&line, t(1000 + i)).unwrap(); // flushes every write
        }
        w.close().unwrap();
        // ~40 KB raw would be 10+ plain segments at 4 KB.
        assert_eq!(count_segments(&dir), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retention_counts_and_prunes_mixed_segments() {
        let dir = tempdir_unique("retmixed");
        let zpath = dir.join(segment_file_name_with(t(1000), 0, Compression::Zstd));
        let mut zw = CaptureFileWriter::create(&zpath, Compression::Zstd).unwrap();
        zw.write_line(&"x".repeat(100), t(1000)).unwrap();
        zw.finish().unwrap();
        write_segment(&dir, 2000, 1, &["0123456789"]);
        write_segment(&dir, 3000, 2, &["0123456789"]);
        assert_eq!(count_segments(&dir), 3);
        let deleted = enforce_retention(&dir, Some(25), None, t(4000)).unwrap();
        assert_eq!(deleted, 1);
        assert!(!zpath.exists()); // the compressed one was oldest
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compression_parses_and_names_segments() {
        assert_eq!("zstd".parse::<Compression>().unwrap(), Compression::Zstd);
        assert_eq!("None".parse::<Compression>().unwrap(), Compression::None);
        assert!("gzip".parse::<Compression>().is_err());
        let name = segment_file_name_with(t(1000), 3, Compression::Zstd);
        assert_eq!(parse_segment_file_name(&name), Some((t(1000), 3)));
    }

    #[test]
    fn retention_never_deletes_the_only_segment() {
        let dir = tempdir_unique("retone");
        write_segment(&dir, 1000, 0, &["0123456789"]);
        let deleted =
            enforce_retention(&dir, Some(1), Some(Duration::from_secs(0)), t(9_999_999)).unwrap();
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
        serde_json::to_string(&CaptureRecord::from_message(
            &m,
            Duration::ZERO,
            t(received_secs),
        ))
        .unwrap()
    }

    #[test]
    fn load_segment_positions_and_parses_received_at() {
        let dir = tempdir_unique("load");
        let path = write_segment(
            &dir,
            1000,
            0,
            &[&rec_line("a/b", 1000), &rec_line("c/d", 1001)],
        );
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
        // example already verified in crates/zenmon-core/src/keyexpr.rs.
        assert_eq!(
            key_matches("a//b", "a/b").unwrap_err().kind,
            crate::error::ErrorKind::InvalidInput
        );
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
        let f = ReadFilter {
            key: "a/*".into(),
            since: Some(t(500)),
            until: Some(t(2000)),
        };
        assert!(record_in_window(&pr, &f).unwrap());
        let f2 = ReadFilter {
            key: "a/*".into(),
            since: Some(t(1500)),
            until: None,
        };
        assert!(!record_in_window(&pr, &f2).unwrap()); // before since
        let f3 = ReadFilter {
            key: "z/*".into(),
            since: None,
            until: None,
        };
        assert!(!record_in_window(&pr, &f3).unwrap()); // key mismatch
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_in_window_boundaries_since_inclusive_until_exclusive() {
        let dir = tempdir_unique("winb");
        let path = write_segment(&dir, 1000, 0, &[&rec_line("a/b", 1000)]); // received == t(1000)
        let pr = load_segment(&path, true).unwrap().remove(0);
        // received == since  -> INCLUDED (since is inclusive)
        let f_in = ReadFilter {
            key: "**".into(),
            since: Some(t(1000)),
            until: None,
        };
        assert!(record_in_window(&pr, &f_in).unwrap());
        // received == until  -> EXCLUDED (until is exclusive)
        let f_ex = ReadFilter {
            key: "**".into(),
            since: None,
            until: Some(t(1000)),
        };
        assert!(!record_in_window(&pr, &f_ex).unwrap());
        // received in [since, until) -> INCLUDED
        let f_within = ReadFilter {
            key: "**".into(),
            since: Some(t(1000)),
            until: Some(t(1001)),
        };
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
        assert!(
            pr.received.is_none(),
            "v1 record must have no received time"
        );
        // A time window that would exclude any dated record still passes (unbounded).
        let f = ReadFilter {
            key: "a/*".into(),
            since: Some(t(5000)),
            until: Some(t(6000)),
        };
        assert!(record_in_window(&pr, &f).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn key_matches_unparseable_record_key_is_false() {
        // "a//b" (empty chunk) is an invalid key expression; as a RECORD key it must
        // yield Ok(false), never an error.
        assert!(!key_matches("a/*", "a//b").unwrap());
    }

    fn seed_store(tag: &str) -> PathBuf {
        // 3 segments, 2 records each, keys alternate a/x and b/y, times 1000..1005
        let dir = tempdir_unique(tag);
        write_segment(
            &dir,
            1000,
            0,
            &[&rec_line("a/x", 1000), &rec_line("b/y", 1001)],
        );
        write_segment(
            &dir,
            1002,
            0,
            &[&rec_line("a/x", 1002), &rec_line("b/y", 1003)],
        );
        write_segment(
            &dir,
            1004,
            0,
            &[&rec_line("a/x", 1004), &rec_line("b/y", 1005)],
        );
        dir
    }

    fn plain_opts(key: &str, limit: Option<u64>, cursor: Option<String>) -> ReadOptions {
        ReadOptions {
            filter: ReadFilter {
                key: key.into(),
                since: None,
                until: None,
            },
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
        let c = encode_cursor("zenmon-trace-20260716T000000Z-00000.ndjson", 4);
        let (seg, idx) = decode_cursor(&c).unwrap();
        assert_eq!(
            (seg.as_str(), idx),
            ("zenmon-trace-20260716T000000Z-00000.ndjson", 4)
        );
        assert_eq!(
            decode_cursor("!notbase64!").unwrap_err().kind,
            crate::error::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn topic_stats_rolls_up_per_key() {
        let dir = seed_store("stats1");
        let f = ReadFilter {
            key: "**".into(),
            since: None,
            until: None,
        };
        let stats = topic_stats(&dir, &f, None, Some(64)).unwrap();
        assert_eq!(stats.len(), 2);
        let ax = stats.iter().find(|s| s.key == "a/x").unwrap();
        assert_eq!(ax.count, 3);
        assert!(ax.first_ts.is_some());
        assert!(ax.rate_hz > 0.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn topic_stats_top_n_by_volume_and_key_filter() {
        let dir = seed_store("stats2");
        let f = ReadFilter {
            key: "a/*".into(),
            since: None,
            until: None,
        };
        let stats = topic_stats(&dir, &f, Some(1), None).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].key, "a/x"); // only a/* matched
        std::fs::remove_dir_all(&dir).ok();
    }
}
