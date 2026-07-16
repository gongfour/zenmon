use std::time::Duration;

/// Parse a user-facing duration option (e.g. "5s", "100ms", "1m500ms").
///
/// Rejects zero durations and unit-less integers so agents get a clear input
/// error instead of a silently reinterpreted value. Used as a clap
/// `value_parser` for every user-facing time option.
pub fn parse_duration_arg(s: &str) -> Result<Duration, String> {
    let d = humantime::parse_duration(s.trim())
        .map_err(|e| format!("invalid duration '{}': {} (try e.g. 5s, 100ms)", s, e))?;
    if d.is_zero() {
        return Err(format!("duration '{}' must be greater than zero", s));
    }
    Ok(d)
}

/// Parse the `--connect-timeout` option.
///
/// Same humantime syntax as [`parse_duration_arg`], but additionally enforces
/// the connect-timeout bounds (>= 1ms, <= ~49 days) shared with the
/// `ZEMON_CONNECT_TIMEOUT` environment variable, so a sub-millisecond value like
/// `1ns` can't silently round to `0ms`.
pub fn parse_connect_timeout_arg(s: &str) -> Result<Duration, String> {
    let d = humantime::parse_duration(s.trim())
        .map_err(|e| format!("invalid duration '{}': {} (try e.g. 5s, 100ms)", s, e))?;
    zemon_core::config::validate_connect_timeout(d)
        .map_err(|msg| format!("invalid --connect-timeout '{}': {}", s, msg))
}

/// Parse a positive count option (`--count N`). Rejects zero and non-numeric
/// input so bounded watch/subscribe commands get a clear error.
pub fn parse_count_arg(s: &str) -> Result<u64, String> {
    let n: u64 = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid count '{}': expected a positive integer", s))?;
    if n == 0 {
        return Err("count must be greater than zero".to_string());
    }
    Ok(n)
}

/// Parse a replay speed multiplier (`--speed 2.0`). Must be > 0.
pub fn parse_speed_arg(s: &str) -> Result<f64, String> {
    let v: f64 = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid speed '{}': expected a number like 2.0", s))?;
    if !(v.is_finite() && v > 0.0) {
        return Err("speed must be a positive number".to_string());
    }
    Ok(v)
}

/// Parse a fixed replay rate (`--rate 10Hz`) in hertz. Must be > 0.
pub fn parse_rate_hz_arg(s: &str) -> Result<f64, String> {
    let trimmed = s.trim();
    let num = trimmed
        .strip_suffix("Hz")
        .or_else(|| trimmed.strip_suffix("hz"))
        .unwrap_or(trimmed);
    let v: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid rate '{}': expected e.g. 10Hz", s))?;
    if !(v.is_finite() && v > 0.0) {
        return Err("rate must be a positive number of Hz".to_string());
    }
    Ok(v)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_speed() {
        assert_eq!(parse_speed_arg("2.0").unwrap(), 2.0);
        assert!(parse_speed_arg("0").is_err());
        assert!(parse_speed_arg("-1").is_err());
    }

    #[test]
    fn parses_rate_hz() {
        assert_eq!(parse_rate_hz_arg("10Hz").unwrap(), 10.0);
        assert_eq!(parse_rate_hz_arg("2.5hz").unwrap(), 2.5);
        assert_eq!(parse_rate_hz_arg("5").unwrap(), 5.0);
        assert!(parse_rate_hz_arg("0Hz").is_err());
    }

    #[test]
    fn parses_positive_count() {
        assert_eq!(parse_count_arg("3").unwrap(), 3);
    }

    #[test]
    fn rejects_zero_count() {
        assert!(parse_count_arg("0").is_err());
    }

    #[test]
    fn rejects_non_numeric_count() {
        assert!(parse_count_arg("abc").is_err());
    }

    #[test]
    fn parses_seconds() {
        assert_eq!(parse_duration_arg("5s").unwrap(), Duration::from_secs(5));
    }

    #[test]
    fn parses_millis() {
        assert_eq!(parse_duration_arg("100ms").unwrap(), Duration::from_millis(100));
    }

    #[test]
    fn parses_compound() {
        assert_eq!(
            parse_duration_arg("1m500ms").unwrap(),
            Duration::from_millis(60_500)
        );
    }

    #[test]
    fn rejects_zero() {
        assert!(parse_duration_arg("0s").is_err());
    }

    #[test]
    fn rejects_unitless_integer() {
        assert!(parse_duration_arg("5000").is_err());
    }

    #[test]
    fn rejects_bad_suffix() {
        assert!(parse_duration_arg("5x").is_err());
    }

    #[test]
    fn connect_timeout_accepts_valid_range() {
        assert_eq!(
            parse_connect_timeout_arg("5s").unwrap(),
            Duration::from_secs(5)
        );
        assert_eq!(
            parse_connect_timeout_arg("1ms").unwrap(),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn connect_timeout_rejects_sub_millisecond() {
        // Would truncate to 0ms via as_millis(); must be rejected, not silenced.
        assert!(parse_connect_timeout_arg("1ns").is_err());
        assert!(parse_connect_timeout_arg("500us").is_err());
    }

    #[test]
    fn connect_timeout_rejects_zero() {
        assert!(parse_connect_timeout_arg("0s").is_err());
    }

    #[test]
    fn connect_timeout_rejects_too_large() {
        // > u32::MAX ms (~49.7 days).
        assert!(parse_connect_timeout_arg("100days").is_err());
    }

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
}
