//! A compact time/key index over a private spool, never the caller's live files.
use color_eyre::{eyre::eyre, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, SystemTime},
};
use zenmon_core::{
    capture::CaptureRecord,
    inspection::{self, Progress, Selection},
    ZenmonError,
};

pub(crate) const MAX_RECORDS: usize = 1_000_000;
pub(crate) const MAX_BYTES: usize = 2 * 1024 * 1024 * 1024;
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
    pub fingerprint: String,
    pub entries: Vec<Entry>,
    pub keys: Vec<String>,
    by_key: Vec<Vec<usize>>,
    spool: File,
    legacy: bool,
}

impl Recording {
    #[cfg(test)]
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with_limits(path, MAX_RECORDS, MAX_BYTES)
    }
    #[cfg(test)]
    pub fn load_with_limits(path: &Path, max_records: usize, max_bytes: usize) -> Result<Self> {
        Self::load_selected(path, &Selection::default(), max_records, max_bytes, |_| {
            Ok(())
        })
    }
    pub fn load_selected(
        path: &Path,
        selection: &Selection,
        max_records: usize,
        max_bytes: usize,
        progress: impl FnMut(Progress) -> zenmon_core::Result<()>,
    ) -> Result<Self> {
        let mut spool = tempfile::tempfile()?;
        let mut entries = Vec::new();
        let mut keys = Vec::new();
        let mut key_ids = HashMap::new();
        let mut offset = 0u64;
        let mut legacy = None;
        let mut source_hash = Sha256::new();
        let mut selected_hash = Sha256::new();
        inspection::scan_observed(
            path,
            selection,
            progress,
            |record, line| {
                selected_hash.update((line.len() as u64).to_le_bytes());
                selected_hash.update(line.as_bytes());
                if entries.len() >= max_records || offset + line.len() as u64 > max_bytes as u64 {
                    return Err(ZenmonError::invalid_input(format!("offline index exceeds {max_records} records or {max_bytes} spool bytes; reopen with --key, --since and --until, or use: zenmon trace export <INPUT> --output window.ndjson --key '<KEY>' --since <START> --until <END>")));
                }
                let is_legacy = record.received_at.is_none();
                if legacy.is_some_and(|v| v != is_legacy) {
                    return Err(ZenmonError::invalid_input(
                        "cannot mix relative v1 and absolute v2 timestamps",
                    ));
                }
                legacy = Some(is_legacy);
                let time = inspection::record_time(record)?;
                let next_id = keys.len();
                let key = *key_ids.entry(record.key_expr.clone()).or_insert_with(|| {
                    keys.push(record.key_expr.clone());
                    next_id
                });
                if keys.len() > crate::app::MAX_KEYS {
                    return Err(ZenmonError::invalid_input(
                        "offline index exceeds 10,000 keys; reopen with --key '<KEY>'",
                    ));
                }
                let bytes = (record.payload_bytes()?.len()
                    + record.attachment_bytes()?.map_or(0, |v| v.len()))
                    as u64;
                spool
                    .write_all(line.as_bytes())
                    .map_err(inspection::io_error)?;
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
                Ok(())
            },
            |line| {
                source_hash.update((line.len() as u64).to_le_bytes());
                source_hash.update(line.as_bytes());
                Ok(())
            },
        )?;
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
            fingerprint: format!(
                "{:x}:{:x}",
                source_hash.finalize(),
                selected_hash.finalize()
            ),
            entries,
            keys,
            by_key,
            spool,
            legacy: legacy.unwrap_or(false),
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

impl Recording {
    pub fn key_at(&self, position: usize) -> &str {
        &self.keys[self.entries[position].key]
    }
    pub fn seek_to(&self, value: &str) -> Result<usize> {
        let target = if let Some(offset) = value.strip_prefix('+') {
            let d = zenmon_core::inspection::parse_offset(offset)?;
            self.entries[0]
                .time
                .checked_add(d)
                .ok_or_else(|| eyre!("time out of range"))?
        } else {
            if self.legacy {
                return Err(eyre!("use +offset to jump in a relative v1 recording"));
            }
            zenmon_core::trace::parse_time_bound(value, SystemTime::now())?
        };
        if target < self.entries[0].time || target > self.entries.last().unwrap().time {
            return Err(eyre!("time is outside the loaded window"));
        }
        Ok(self
            .entries
            .partition_point(|e| e.time < target)
            .min(self.entries.len() - 1))
    }
    /// No wraparound. Each operation checks the complete loaded window, not
    /// only the bounded history currently rendered by the dashboard.
    pub fn find(
        &mut self,
        position: usize,
        key: Option<&str>,
        query: &str,
        forward: bool,
        pointer: bool,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Option<usize>> {
        let baseline = if pointer {
            if key.is_none() {
                return Err(eyre!(
                    "select a concrete key before searching for a field change"
                ));
            }
            if !query.starts_with('/')
                && !query.starts_with("attachment:/")
                && !query.starts_with("payload:/")
            {
                return Err(eyre!("field must be a JSON pointer, e.g. /state"));
            }
            let i = (0..=position)
                .rev()
                .find(|&i| key == Some(self.keys[self.entries[i].key].as_str()))
                .ok_or_else(|| eyre!("selected key has no message at this position"))?;
            Some(Some(field_value(&self.read(i)?, query)?.ok_or_else(
                || eyre!("field does not exist in the current message"),
            )?))
        } else {
            if query.is_empty() {
                return Err(eyre!("enter nonempty search text"));
            }
            None
        };
        let indices: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new(position + 1..self.entries.len())
        } else {
            Box::new((0..position).rev())
        };
        for i in indices {
            check()?;
            if key.is_some_and(|key| self.keys[self.entries[i].key] != key) {
                continue;
            }
            let record = self.read(i)?;
            let matched = if let Some(baseline) = &baseline {
                field_value(&record, query)? != *baseline
            } else {
                let payload = record.payload_bytes()?;
                let attachment = record.attachment_bytes()?.unwrap_or_default();
                contains_bytes(&payload, query.as_bytes())
                    || contains_bytes(&attachment, query.as_bytes())
                    || zenmon_core::types::MessagePayload::from_bytes(payload)
                        .to_view()
                        .to_string()
                        .contains(query)
                    || zenmon_core::types::MessagePayload::from_bytes(attachment)
                        .to_view()
                        .to_string()
                        .contains(query)
            };
            if matched {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }
    pub fn export_range(
        &mut self,
        output: &Path,
        start: usize,
        end: usize,
        key: Option<&str>,
        mut check: impl FnMut() -> zenmon_core::Result<()>,
    ) -> Result<u64> {
        if start > end || end >= self.entries.len() {
            return Err(eyre!("invalid marked range"));
        }
        Ok(inspection::atomic_export(output, |file| {
            let mut n = 0;
            for i in start..=end {
                check()?;
                if key.is_some_and(|key| self.keys[self.entries[i].key] != key) {
                    continue;
                }
                let entry = &self.entries[i];
                self.spool
                    .seek(SeekFrom::Start(entry.offset))
                    .map_err(inspection::io_error)?;
                let mut bytes = vec![0; entry.len];
                self.spool
                    .read_exact(&mut bytes)
                    .map_err(inspection::io_error)?;
                file.write_all(&bytes).map_err(inspection::io_error)?;
                file.write_all(b"\n").map_err(inspection::io_error)?;
                n += 1;
            }
            Ok(n)
        })?)
    }
}
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|v| v == needle)
}
fn field_value(record: &CaptureRecord, pointer: &str) -> Result<Option<serde_json::Value>> {
    let (bytes, pointer) = if let Some(pointer) = pointer.strip_prefix("attachment:") {
        (record.attachment_bytes()?.unwrap_or_default(), pointer)
    } else {
        (
            record.payload_bytes()?,
            pointer.strip_prefix("payload:").unwrap_or(pointer),
        )
    };
    let value = zenmon_core::types::MessagePayload::from_bytes(bytes).to_view();
    Ok(value.pointer(pointer).cloned())
}

#[cfg(test)]
mod investigation_tests {
    use super::*;
    use base64::Engine;
    fn fixture() -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for i in 0..300 {
            let key = if i % 2 == 0 {
                "sensor/state"
            } else {
                "sensor/other"
            };
            let payload = base64::engine::general_purpose::STANDARD
                .encode(format!("{{\"value\":{},\"text\":\"sample-{i}\"}}", i / 100).as_bytes());
            let attachment = base64::engine::general_purpose::STANDARD
                .encode(format!("{{\"sequence\":{i}}}").as_bytes());
            let r = CaptureRecord {
                schema_version: 2,
                key_expr: key.into(),
                payload_base64: payload,
                attachment_base64: Some(attachment),
                encoding: "zenoh/bytes".into(),
                kind: "PUT".into(),
                source_timestamp: None,
                received_at: Some(format!("2026-08-06T00:00:{:02}Z", i / 50)),
                received_offset_ms: i * 20,
            };
            writeln!(f, "{}", serde_json::to_string(&r).unwrap()).unwrap();
        }
        f
    }
    #[test]
    fn selection_applies_before_limits_and_preserves_equal_time_order() {
        let f = fixture();
        let selection = Selection::new(
            "sensor/state",
            Some("2026-08-06T00:00:01Z"),
            Some("2026-08-06T00:00:02Z"),
        )
        .unwrap();
        let mut r =
            Recording::load_selected(f.path(), &selection, 25, MAX_BYTES, |_| Ok(())).unwrap();
        assert_eq!(r.entries.len(), 25);
        assert_eq!(r.read(0).unwrap().received_offset_ms, 1000);
        assert_eq!(r.read(24).unwrap().received_offset_ms, 1960);
        assert_eq!(r.seek_to("+0s").unwrap(), 0);
        assert_eq!(r.seek_to("2026-08-06T00:00:01Z").unwrap(), 0);
        assert!(r.seek_to("+1s").is_err());
    }
    #[test]
    fn searches_full_window_payload_attachment_and_changes_both_directions() {
        let f = fixture();
        let mut r = Recording::load(f.path()).unwrap();
        assert_eq!(
            r.find(299, None, "sample-2\"", false, false, || Ok(()))
                .unwrap(),
            Some(2)
        );
        assert_eq!(
            r.find(0, None, "\"sequence\":299", true, false, || Ok(()))
                .unwrap(),
            Some(299)
        );
        assert_eq!(
            r.find(
                0,
                Some("sensor/state"),
                "sample-1\"",
                true,
                false,
                || Ok(())
            )
            .unwrap(),
            None
        );
        assert_eq!(
            r.find(0, Some("sensor/state"), "/value", true, true, || Ok(()))
                .unwrap(),
            Some(100)
        );
        assert_eq!(
            r.find(200, Some("sensor/state"), "/value", false, true, || Ok(()))
                .unwrap(),
            Some(198)
        );
        assert_eq!(
            r.find(
                0,
                Some("sensor/state"),
                "attachment:/sequence",
                true,
                true,
                || Ok(())
            )
            .unwrap(),
            Some(2)
        );
        assert!(r
            .find(0, Some("sensor/state"), "/missing", true, true, || Ok(()))
            .is_err());
        assert!(r
            .find(0, None, "nothing", true, false, || Err(eyre!("cancelled")))
            .is_err());
        assert_eq!(r.seek_to("+2s").unwrap(), 100);
    }
    #[test]
    fn marked_export_keeps_exact_messages_even_with_identical_timestamps() {
        let f = fixture();
        let mut r = Recording::load(f.path()).unwrap();
        let d = tempfile::tempdir().unwrap();
        let output = d.path().join("slice.ndjson");
        assert_eq!(
            r.export_range(&output, 3, 8, Some("sensor/state"), || Ok(()))
                .unwrap(),
            3
        );
        let mut loaded = Recording::load(&output).unwrap();
        assert_eq!(loaded.entries.len(), 3);
        assert_eq!(loaded.read(0).unwrap().received_offset_ms, 80);
        assert_eq!(loaded.read(2).unwrap().received_offset_ms, 160);
        assert!(zenmon_core::verify::verify(&output).unwrap().valid);
        assert!(r.export_range(&output, 0, 10, None, || Ok(())).is_err());
        let cancelled = d.path().join("cancelled.ndjson");
        assert!(r
            .export_range(&cancelled, 0, 299, None, || Err(ZenmonError::new(
                zenmon_core::ErrorKind::Interrupted,
                "cancelled"
            )))
            .is_err());
        assert!(!cancelled.exists());
    }
}
