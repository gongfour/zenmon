//! Sparklines over a numeric field's recent values.
//!
//! The master pane already draws a sparkbar, but it plots *bandwidth* — how
//! much a key is sending, never what it is saying. Watching a battery drain or
//! a speed settle means reading the number off the detail pane over and over.
//!
//! Neither zenoh GUI tool plots payload values at all (zenoh-hammer's `egui_plot`
//! dependency only renders images), so this is the one place the terminal ends
//! up ahead of them.
//!
//! No extra storage: the series is extracted from `crate::history` on demand.
//! Entries are capped at 128 per key, so a walk per frame is bounded and the
//! frame gate covers the rest.

use serde_json::Value;

use crate::history::KeyHistory;

/// Block glyphs, low to high.
const GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// A field's recent values with the range they spanned.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    /// Oldest first, so the sparkline reads left to right like time.
    pub values: Vec<f64>,
    pub min: f64,
    pub max: f64,
}

impl Series {
    pub fn last(&self) -> Option<f64> {
        self.values.last().copied()
    }
}

/// Plottable JSON pointers in `v`, in document order, up to `max`.
///
/// Array elements are excluded. Index 2 of one message is not the same quantity
/// as index 2 of the next — a list of detected landmarks reorders, and plotting
/// position 2 over time would draw a line through unrelated values.
///
/// Booleans are excluded for the same reason a bar chart of true/false is not
/// useful: the detail pane already shows the flag, and a two-level sparkline
/// says nothing the value does not.
pub fn numeric_pointers(v: &Value, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    collect(v, String::new(), max, &mut out);
    out
}

fn collect(v: &Value, path: String, max: usize, out: &mut Vec<String>) {
    if out.len() >= max {
        return;
    }
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                collect(child, format!("{path}/{k}"), max, out);
            }
        }
        Value::Number(_) => out.push(path),
        _ => {}
    }
}

/// Pull `pointer` out of every entry, oldest first.
///
/// Entries where the pointer is missing or not a number are skipped rather than
/// filled with zero: a gap in a field's presence is not the value dropping to
/// zero, and drawing it as one invents a cliff that never happened.
pub fn series_for(history: &KeyHistory, pointer: &str) -> Option<Series> {
    let mut values: Vec<f64> = history
        .iter()
        .rev() // stored newest-first; a sparkline reads left to right
        .filter_map(|e| e.view.pointer(pointer).and_then(Value::as_f64))
        .filter(|v| v.is_finite())
        .collect();
    if values.is_empty() {
        return None;
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    values.shrink_to_fit();
    Some(Series { values, min, max })
}

/// Render the tail of a series as block glyphs.
///
/// Normalised over the series' own range, not against zero. A battery moving
/// between 86% and 88% is the interesting part of that signal; anchoring the
/// axis at zero would flatten it to a straight line.
///
/// A flat series draws mid-height rather than empty or full, so "steady" is
/// visibly distinct from "no data" — which the bandwidth sparkbar renders as an
/// empty string.
pub fn spark(series: &Series, width: usize) -> String {
    if series.values.is_empty() || width == 0 {
        return String::new();
    }
    let tail = if series.values.len() > width {
        &series.values[series.values.len() - width..]
    } else {
        &series.values[..]
    };
    let span = series.max - series.min;
    if span <= f64::EPSILON {
        return GLYPHS[GLYPHS.len() / 2].to_string().repeat(tail.len());
    }
    tail.iter()
        .map(|v| {
            let t = (v - series.min) / span;
            let idx = (t * (GLYPHS.len() - 1) as f64).round() as usize;
            GLYPHS[idx.min(GLYPHS.len() - 1)]
        })
        .collect()
}

/// Compact label for a value, trimming the noise off a float.
pub fn format_value(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v:.3}")
    }
}

/// The field name a pointer refers to, for a label.
pub fn pointer_label(pointer: &str) -> &str {
    pointer.rsplit('/').next().unwrap_or(pointer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zenmon_core::types::{MessagePayload, ZenohMessage};

    use crate::history::History;

    fn history_of(key: &str, payloads: &[Value]) -> History {
        let mut h = History::default();
        for p in payloads {
            let payload = MessagePayload::from_json(p);
            h.record(&ZenohMessage {
                key_expr: key.into(),
                payload_bytes: payload.len(),
                payload,
                encoding: "application/json".into(),
                timestamp: None,
                kind: "put".into(),
                attachment: None,
                attachment_bytes: None,
            });
        }
        h
    }

    fn series(key: &str, payloads: &[Value], pointer: &str) -> Option<Series> {
        let h = history_of(key, payloads);
        series_for(h.get(key).unwrap(), pointer)
    }

    #[test]
    fn numbers_are_found_at_any_depth() {
        let v = json!({"speed": 1.5, "pose": {"x": 1, "y": 2}, "mode": "idle"});
        assert_eq!(
            numeric_pointers(&v, 16),
            vec!["/pose/x", "/pose/y", "/speed"]
        );
    }

    #[test]
    fn array_elements_are_not_offered() {
        // Index 2 of one message is not the same quantity as index 2 of the
        // next, so a line through them would be meaningless.
        let v = json!({"landmarks": [1, 2, 3], "count": 3});
        assert_eq!(numeric_pointers(&v, 16), vec!["/count"]);
    }

    #[test]
    fn booleans_and_strings_are_not_offered() {
        let v = json!({"is_valid": true, "id": "abc", "n": 1});
        assert_eq!(numeric_pointers(&v, 16), vec!["/n"]);
    }

    #[test]
    fn the_pointer_list_is_capped() {
        let v = json!({"a": 1, "b": 2, "c": 3, "d": 4});
        assert_eq!(numeric_pointers(&v, 2).len(), 2);
    }

    #[test]
    fn a_series_reads_oldest_to_newest() {
        // History is newest-first; a sparkline has to run the other way or it
        // shows time flowing backwards.
        let s = series(
            "k",
            &[json!({"n": 1}), json!({"n": 2}), json!({"n": 3})],
            "/n",
        )
        .unwrap();
        assert_eq!(s.values, vec![1.0, 2.0, 3.0]);
        assert_eq!(s.last(), Some(3.0));
        assert_eq!((s.min, s.max), (1.0, 3.0));
    }

    #[test]
    fn missing_values_are_skipped_not_zero_filled() {
        // Zero-filling would draw a cliff to the axis that never happened.
        let s = series(
            "k",
            &[json!({"n": 5}), json!({"other": 1}), json!({"n": 7})],
            "/n",
        )
        .unwrap();
        assert_eq!(s.values, vec![5.0, 7.0]);
        assert_eq!(s.min, 5.0);
    }

    #[test]
    fn a_field_that_changes_type_contributes_only_its_numbers() {
        let s = series(
            "k",
            &[json!({"n": 1}), json!({"n": "two"}), json!({"n": 3})],
            "/n",
        )
        .unwrap();
        assert_eq!(s.values, vec![1.0, 3.0]);
    }

    #[test]
    fn a_pointer_that_never_appears_has_no_series() {
        assert!(series("k", &[json!({"n": 1})], "/missing").is_none());
    }

    #[test]
    fn the_sparkline_spans_the_series_own_range() {
        // A battery between 86 and 88 must not flatten because the axis was
        // anchored at zero.
        let s = Series {
            values: vec![86.0, 87.0, 88.0],
            min: 86.0,
            max: 88.0,
        };
        // Midpoint of a 3-value ramp lands on index round(0.5 * 7) = 4.
        assert_eq!(spark(&s, 8), "▁▅█");
    }

    #[test]
    fn a_flat_series_draws_mid_height() {
        // Distinguishable from "no data", which renders as nothing at all.
        let s = Series {
            values: vec![5.0, 5.0, 5.0],
            min: 5.0,
            max: 5.0,
        };
        assert_eq!(spark(&s, 8), "▅▅▅");
    }

    #[test]
    fn negative_values_plot_without_clipping() {
        // `theta` on a real pose message swings through negative radians.
        use std::f64::consts::PI;
        let s = Series {
            values: vec![-PI, 0.0, PI],
            min: -PI,
            max: PI,
        };
        assert_eq!(spark(&s, 8), "▁▅█");
    }

    #[test]
    fn a_long_series_shows_its_tail() {
        let s = Series {
            values: (0..100).map(f64::from).collect(),
            min: 0.0,
            max: 99.0,
        };
        let out = spark(&s, 8);
        assert_eq!(out.chars().count(), 8);
        assert!(out.ends_with('█'), "the newest value is the last glyph");
    }

    #[test]
    fn values_render_without_trailing_float_noise() {
        assert_eq!(format_value(5.0), "5");
        assert_eq!(format_value(-std::f64::consts::PI), "-3.142");
        assert_eq!(format_value(0.5), "0.500");
    }

    #[test]
    fn a_label_is_the_last_segment() {
        assert_eq!(pointer_label("/pose/x"), "x");
        assert_eq!(pointer_label("/speed"), "speed");
    }
}
