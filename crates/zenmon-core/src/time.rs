//! Zenoh HLC timestamps, made readable.
//!
//! A sample's timestamp prints as `<NTP64>/<id>` — a 64-bit fixed-point time
//! and the ID of the node whose clock stamped it. Neither half means anything
//! to a person; this turns them into a wall-clock time and a short source.

use std::str::FromStr;

/// A parsed HLC timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlcView {
    /// UTC wall-clock time, `YYYY-MM-DD HH:MM:SS.ffffff`.
    pub time: String,
    /// The ID of the node that stamped the sample, in hex. Usually the
    /// publisher's session, but a router stamps samples that arrive without
    /// a timestamp, and then it is the router's.
    pub source: String,
}

/// Parse an HLC timestamp string; `None` if it is not one.
pub fn parse_hlc(raw: &str) -> Option<HlcView> {
    let ts = zenoh::time::Timestamp::from_str(raw).ok()?;
    Some(HlcView {
        time: readable_hlc(raw),
        source: ts.get_id().to_string(),
    })
}

/// Render a zenoh HLC timestamp as a readable `YYYY-MM-DD HH:MM:SS.ffffff`.
/// Unparseable input is passed through verbatim, so a malformed timestamp is
/// visible rather than silently blank.
pub fn readable_hlc(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }

    match zenoh::time::Timestamp::from_str(raw) {
        Ok(ts) => {
            let rfc3339 = ts.get_time().to_string_rfc3339_lossy();
            let readable = rfc3339
                .strip_suffix('Z')
                .unwrap_or(&rfc3339)
                .replace('T', " ");
            // `to_string_rfc3339_lossy` fraction width varies by zenoh version
            // (micro- vs nanosecond). Cap to microseconds so the display is
            // deterministic, then drop any trailing zeros.
            trim_fractional_zeros(cap_fractional_digits(readable, 6))
        }
        Err(_) => raw.to_string(),
    }
}

/// Truncate the fractional-seconds part of a `HH:MM:SS.ffffff` string to at most
/// `max` digits. Strings with fewer (or no) fractional digits are unchanged.
/// Assumes no trailing timezone offset (the `Z` suffix is stripped beforehand).
fn cap_fractional_digits(mut ts: String, max: usize) -> String {
    if let Some(dot_idx) = ts.find('.') {
        let frac_end = dot_idx + 1 + max;
        if ts.len() > frac_end {
            ts.truncate(frac_end);
        }
    }
    ts
}

fn trim_fractional_zeros(mut ts: String) -> String {
    if let Some(dot_idx) = ts.find('.') {
        let mut end = ts.len();
        while end > dot_idx + 1 && ts.as_bytes()[end - 1] == b'0' {
            end -= 1;
        }
        if end == dot_idx + 1 {
            end -= 1;
        }
        ts.truncate(end);
    }
    ts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_zenoh_timestamp_as_readable_datetime() {
        let formatted = readable_hlc("7386690599959157260/33");
        // `to_string_rfc3339_lossy()` fraction width is zenoh-version dependent
        // (some emit microseconds, some nanoseconds). We cap it to microseconds
        // ourselves so the display is deterministic regardless of that.
        assert_eq!(formatted, "2024-07-01 15:32:06.860479");
    }

    #[test]
    fn caps_fraction_to_microseconds() {
        // Nanosecond fraction is truncated to 6 digits (version-independent).
        assert_eq!(
            cap_fractional_digits("2024-07-01 15:32:06.860479001".to_string(), 6),
            "2024-07-01 15:32:06.860479"
        );
        // Fewer than 6 fractional digits are left untouched.
        assert_eq!(
            cap_fractional_digits("2024-07-01 15:32:06.86".to_string(), 6),
            "2024-07-01 15:32:06.86"
        );
        // No fractional part: unchanged.
        assert_eq!(
            cap_fractional_digits("2024-07-01 15:32:06".to_string(), 6),
            "2024-07-01 15:32:06"
        );
    }

    #[test]
    fn keeps_raw_timestamp_when_parsing_fails() {
        assert_eq!(readable_hlc("not-a-timestamp"), "not-a-timestamp");
    }

    #[test]
    fn keeps_empty_timestamp_empty() {
        assert_eq!(readable_hlc(""), "");
    }

    #[test]
    fn trims_trailing_fractional_zeros() {
        assert_eq!(
            trim_fractional_zeros("2024-07-01 15:32:06.860479000".to_string()),
            "2024-07-01 15:32:06.860479"
        );
        assert_eq!(
            trim_fractional_zeros("2024-07-01 15:32:06.000000000".to_string()),
            "2024-07-01 15:32:06"
        );
    }

    #[test]
    fn parses_time_and_source() {
        let v = parse_hlc("7386690599959157260/33").unwrap();
        assert_eq!(v.time, "2024-07-01 15:32:06.860479");
        assert_eq!(v.source, "33");
        assert_eq!(parse_hlc("not-a-timestamp"), None);
    }
}
