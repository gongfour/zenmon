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

#[cfg(test)]
mod tests {
    use super::*;

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
}
