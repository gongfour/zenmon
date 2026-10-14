//! A compact time/key index over a private spool, never the caller's live files.
use color_eyre::{eyre::eyre, Result};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, SystemTime},
};
use zenmon_core::{capture::CaptureRecord, trace, verify};

pub(crate) const MAX_RECORDS: usize = 1_000_000;
pub(crate) const MAX_BYTES: usize = 2 * 1024 * 1024 * 1024;
const MAX_LINE: usize = 16 * 1024 * 1024;
const SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct Entry {
    pub time: SystemTime,
    offset: u64,
    len: usize,
    key: usize,
    bytes: u64,
    pub total_bytes: u64,
    key_bytes: u64,
}
pub(crate) struct Recording {
    pub entries: Vec<Entry>,
    pub keys: Vec<String>,
    by_key: Vec<Vec<usize>>,
    spool: File,
}

impl Recording {
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with_limits(path, MAX_RECORDS, MAX_BYTES)
    }
    pub fn load_with_limits(path: &Path, max_records: usize, max_bytes: usize) -> Result<Self> {
        let mut spool = tempfile::tempfile()?;
        let mut entries = Vec::new();
        let mut keys = Vec::new();
        let mut key_ids = HashMap::new();
        let mut offset = 0u64;
        let mut legacy = None;
        for path in verify::capture_paths(path)? {
            for (i, line) in trace::open_capture_lines(&path)?
                .strict()
                .bounded(MAX_LINE)
                .enumerate()
            {
                let line = line.map_err(|e| eyre!("{}:{}: {e}", path.display(), i + 1))?;
                if entries.len() >= max_records || offset + line.len() as u64 > max_bytes as u64 {
                    return Err(eyre!("offline index exceeds {max_records} records or {max_bytes} spool bytes; export a smaller window with trace read"));
                }
                let record = CaptureRecord::parse_line(&line, i + 1)
                    .and_then(|r| {
                        verify::validate_record(&r)?;
                        Ok(r)
                    })
                    .map_err(|e| eyre!("{}:{}: {e}", path.display(), i + 1))?;
                let is_legacy = record.received_at.is_none();
                if legacy.is_some_and(|v| v != is_legacy) {
                    return Err(eyre!("cannot mix relative v1 and absolute v2 timestamps"));
                }
                legacy = Some(is_legacy);
                let time = match &record.received_at {
                    Some(t) => trace::parse_time_bound(t, SystemTime::now())?,
                    None => SystemTime::UNIX_EPOCH
                        .checked_add(Duration::from_millis(record.received_offset_ms))
                        .ok_or_else(|| eyre!("timestamp out of range"))?,
                };
                let next_id = keys.len();
                let key = *key_ids.entry(record.key_expr.clone()).or_insert_with(|| {
                    keys.push(record.key_expr.clone());
                    next_id
                });
                if keys.len() > crate::app::MAX_KEYS {
                    return Err(eyre!(
                        "offline index exceeds 10,000 keys; export a narrower key filter"
                    ));
                }
                let bytes = (record.payload_bytes()?.len()
                    + record.attachment_bytes()?.map_or(0, |v| v.len()))
                    as u64;
                spool.write_all(line.as_bytes())?;
                entries.push(Entry {
                    time,
                    offset,
                    len: line.len(),
                    key,
                    bytes,
                    total_bytes: 0,
                    key_bytes: 0,
                });
                offset += line.len() as u64;
            }
        }
        if entries.is_empty() {
            return Err(eyre!("no records in capture"));
        }
        entries.sort_by_key(|e| e.time);
        let mut by_key = vec![Vec::new(); keys.len()];
        let mut totals = vec![0; keys.len()];
        let mut total = 0;
        for (i, entry) in entries.iter_mut().enumerate() {
            total += entry.bytes;
            totals[entry.key] += entry.bytes;
            entry.total_bytes = total;
            entry.key_bytes = totals[entry.key];
            by_key[entry.key].push(i);
        }
        Ok(Self {
            entries,
            keys,
            by_key,
            spool,
        })
    }
    pub fn read(&mut self, index: usize) -> Result<CaptureRecord> {
        let entry = &self.entries[index];
        self.spool.seek(SeekFrom::Start(entry.offset))?;
        let mut buf = vec![0; entry.len];
        self.spool.read_exact(&mut buf)?;
        Ok(serde_json::from_slice(&buf)?)
    }
    pub fn counts(&self, position: usize) -> Vec<(usize, usize, u64)> {
        self.by_key
            .iter()
            .enumerate()
            .filter_map(|(key, list)| {
                let n = list.partition_point(|&i| i <= position);
                (n > 0).then(|| (key, n, self.entries[list[n - 1]].key_bytes))
            })
            .collect()
    }
    /// Load at most the existing history cap per key, preserving recent history
    /// under a global budget. Always include each key's latest record or fail.
    pub fn snapshot(&self, position: usize) -> Result<Vec<usize>> {
        let mut latest = Vec::new();
        let mut older = Vec::new();
        let mut bytes = 0;
        for list in &self.by_key {
            let n = list.partition_point(|&i| i <= position);
            if n == 0 {
                continue;
            }
            let i = list[n - 1];
            bytes += self.entries[i].len;
            latest.push(i);
            older.extend_from_slice(&list[n.saturating_sub(crate::history::HISTORY_CAP)..n - 1]);
        }
        if bytes > SNAPSHOT_BYTES {
            return Err(eyre!(
                "latest payloads exceed the 64 MiB view budget; export a narrower key filter"
            ));
        }
        older.sort_unstable_by(|a, b| b.cmp(a));
        for i in older {
            if bytes + self.entries[i].len <= SNAPSHOT_BYTES {
                bytes += self.entries[i].len;
                latest.push(i);
            }
        }
        latest.sort_unstable();
        Ok(latest)
    }
    pub fn seek_time(&self, position: usize, forward: bool) -> usize {
        let time = self.entries[position].time;
        let target = if forward {
            time.checked_add(Duration::from_secs(1))
        } else {
            time.checked_sub(Duration::from_secs(1))
        }
        .unwrap_or(time);
        if forward {
            self.entries
                .partition_point(|e| e.time < target)
                .min(self.entries.len() - 1)
        } else {
            self.entries
                .partition_point(|e| e.time <= target)
                .saturating_sub(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line(n: u64, key: &str) -> String {
        serde_json::json!({"schema_version":1,"key_expr":key,"payload_base64":"MQ==","encoding":"application/json","received_offset_ms":n,"kind":"PUT"}).to_string()
    }
    #[test]
    fn indexed_seek_keeps_recent_history_but_counts_the_whole_window() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for i in 0..2000 {
            writeln!(file, "{}", line(i, if i % 2 == 0 { "a" } else { "b" })).unwrap();
        }
        let mut r = Recording::load(file.path()).unwrap();
        let indices = r.snapshot(999).unwrap();
        assert_eq!(indices.len(), 256);
        assert!(indices.iter().all(|i| *i <= 999));
        assert_eq!(r.counts(999), vec![(0, 500, 500), (1, 500, 500)]);
        assert_eq!(r.entries[999].total_bytes, 1000);
        assert_eq!(r.read(indices[0]).unwrap().received_offset_ms, 744);
        // The private spool is a stable snapshot even after the input disappears.
        file.close().unwrap();
        assert_eq!(r.read(1999).unwrap().received_offset_ms, 1999);
    }
    #[test]
    fn sorts_wall_clock_order_stably_and_rejects_mixed_time_bases() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for i in [3, 1, 2, 2] {
            writeln!(file, "{}", line(i, "a")).unwrap();
        }
        let mut r = Recording::load(file.path()).unwrap();
        assert_eq!(r.read(0).unwrap().received_offset_ms, 1);
        assert_eq!(r.read(3).unwrap().received_offset_ms, 3);
        let mut v: serde_json::Value = serde_json::from_str(&line(4, "a")).unwrap();
        v["schema_version"] = 2.into();
        v["received_at"] = "2026-10-14T00:00:00Z".into();
        writeln!(file, "{v}").unwrap();
        assert!(Recording::load(file.path()).is_err());
    }
}
