//! Structural diff between consecutive payloads on a key.
//!
//! A twenty-field status blob at 10 Hz is unreadable as a wall of JSON: the
//! screen changes constantly and nothing tells you *what* changed. Marking the
//! leaves that moved since the previous message turns that wall back into a
//! signal — which is the one feature MQTT Explorer has that neither zenoh GUI
//! tool does.
//!
//! Rendering happens here rather than over `MessagePayload::pretty()` output.
//! Diffing produces facts about JSON *paths*, and mapping those back onto the
//! lines of an already-serialised string is guesswork; emitting the lines and
//! their tags together makes the correspondence exact by construction.

use std::collections::BTreeMap;

use serde_json::Value;

/// What happened to one leaf between the previous message and this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTag {
    /// Present in both, different value.
    Changed,
    /// Not in the previous message.
    Added,
    /// In the previous message, gone from this one.
    Removed,
    Same,
}

/// One line of pretty-printed JSON, with the verdict for the leaf it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedLine {
    pub indent: u16,
    pub text: String,
    pub tag: DiffTag,
}

/// Render `cur` as indented lines, tagging each against `prev`.
///
/// `prev` of `None` (a key seen only once) tags everything [`DiffTag::Same`]:
/// with no baseline, calling every field "added" would be noise on the very
/// first message of every key.
pub fn render(cur: &Value, prev: Option<&Value>) -> Vec<TaggedLine> {
    let mut out = Vec::new();
    walk(cur, prev, 0, None, &mut out);
    out
}

/// Emit `cur`, comparing against `prev` at the same position.
///
/// `label` is the object key or array index this value sits under, rendered
/// ahead of the value on the same line.
fn walk(
    cur: &Value,
    prev: Option<&Value>,
    indent: u16,
    label: Option<&str>,
    out: &mut Vec<TaggedLine>,
) {
    let prefix = label.map(|l| format!("{l}: ")).unwrap_or_default();

    match cur {
        Value::Object(map) => {
            let prev_map = prev.and_then(Value::as_object);
            out.push(TaggedLine {
                indent,
                text: format!("{prefix}{{"),
                // A container's own line is never "changed": the change is
                // always attributable to some leaf inside it, and colouring
                // both would double-report it.
                tag: DiffTag::Same,
            });
            for (k, v) in map {
                walk(v, prev_map.and_then(|m| m.get(k)), indent + 1, Some(k), out);
            }
            // Keys that were there last time and are not now have no line of
            // their own to mark, so they get one.
            if let Some(pm) = prev_map {
                for (k, v) in pm {
                    if !map.contains_key(k) {
                        out.push(TaggedLine {
                            indent: indent + 1,
                            text: format!("{k}: {}", scalar_text(v)),
                            tag: DiffTag::Removed,
                        });
                    }
                }
            }
            out.push(TaggedLine {
                indent,
                text: "}".into(),
                tag: DiffTag::Same,
            });
        }
        Value::Array(items) => {
            let prev_items = prev.and_then(Value::as_array);
            out.push(TaggedLine {
                indent,
                text: format!("{prefix}["),
                tag: DiffTag::Same,
            });
            for (i, v) in items.iter().enumerate() {
                walk(v, prev_items.and_then(|a| a.get(i)), indent + 1, None, out);
            }
            if let Some(pa) = prev_items {
                for v in pa.iter().skip(items.len()) {
                    out.push(TaggedLine {
                        indent: indent + 1,
                        text: scalar_text(v),
                        tag: DiffTag::Removed,
                    });
                }
            }
            out.push(TaggedLine {
                indent,
                text: "]".into(),
                tag: DiffTag::Same,
            });
        }
        scalar => {
            let tag = match prev {
                None => {
                    // No baseline at all means a first message; a missing value
                    // *within* a baseline means the field is new.
                    DiffTag::Same
                }
                Some(p) if p == scalar => DiffTag::Same,
                Some(_) => DiffTag::Changed,
            };
            out.push(TaggedLine {
                indent,
                text: format!("{prefix}{}", scalar_text(scalar)),
                tag,
            });
        }
    }
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Null => "null".into(),
        Value::Object(_) | Value::Array(_) => v.to_string(),
        other => other.to_string(),
    }
}

/// Leaf paths that changed, added or were removed, as JSON-pointer-ish strings.
///
/// Separate from [`render`] because the master pane wants the *count* of moving
/// fields without building any lines.
pub fn changed_paths(cur: &Value, prev: &Value) -> BTreeMap<String, DiffTag> {
    let mut out = BTreeMap::new();
    collect(cur, Some(prev), String::new(), &mut out);
    out
}

fn collect(cur: &Value, prev: Option<&Value>, path: String, out: &mut BTreeMap<String, DiffTag>) {
    match cur {
        Value::Object(map) => {
            let prev_map = prev.and_then(Value::as_object);
            for (k, v) in map {
                collect(
                    v,
                    prev_map.and_then(|m| m.get(k)),
                    format!("{path}/{k}"),
                    out,
                );
            }
            if let Some(pm) = prev_map {
                for k in pm.keys() {
                    if !map.contains_key(k) {
                        out.insert(format!("{path}/{k}"), DiffTag::Removed);
                    }
                }
            }
        }
        Value::Array(items) => {
            let prev_items = prev.and_then(Value::as_array);
            for (i, v) in items.iter().enumerate() {
                collect(
                    v,
                    prev_items.and_then(|a| a.get(i)),
                    format!("{path}/{i}"),
                    out,
                );
            }
            if let Some(pa) = prev_items {
                for i in items.len()..pa.len() {
                    out.insert(format!("{path}/{i}"), DiffTag::Removed);
                }
            }
        }
        scalar => match prev {
            None => {
                out.insert(path, DiffTag::Added);
            }
            Some(p) if p != scalar => {
                out.insert(path, DiffTag::Changed);
            }
            Some(_) => {}
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tags(cur: &Value, prev: Option<&Value>) -> Vec<(String, DiffTag)> {
        render(cur, prev)
            .into_iter()
            .map(|l| (l.text, l.tag))
            .collect()
    }

    fn tag_of(cur: &Value, prev: Option<&Value>, needle: &str) -> DiffTag {
        tags(cur, prev)
            .into_iter()
            .find(|(t, _)| t.starts_with(needle))
            .unwrap_or_else(|| panic!("no line starting with {needle}"))
            .1
    }

    #[test]
    fn only_the_field_that_moved_is_marked() {
        // The point of the whole module: one field of twenty changes and the
        // other nineteen must not compete for attention.
        let prev = json!({"mode": "idle", "speed": 0, "load": 0});
        let cur = json!({"mode": "idle", "speed": 3, "load": 0});
        assert_eq!(tag_of(&cur, Some(&prev), "speed"), DiffTag::Changed);
        assert_eq!(tag_of(&cur, Some(&prev), "mode"), DiffTag::Same);
        assert_eq!(tag_of(&cur, Some(&prev), "load"), DiffTag::Same);
    }

    #[test]
    fn a_first_message_is_not_a_wall_of_additions() {
        let cur = json!({"a": 1, "b": 2});
        for (_, tag) in tags(&cur, None) {
            assert_eq!(tag, DiffTag::Same);
        }
    }

    #[test]
    fn a_new_field_reads_as_changed_against_an_existing_baseline() {
        let prev = json!({"a": 1});
        let cur = json!({"a": 1, "b": 2});
        // `b` had no previous value, so its line is marked; `a` is untouched.
        assert_eq!(tag_of(&cur, Some(&prev), "a"), DiffTag::Same);
        assert_eq!(changed_paths(&cur, &prev).get("/b"), Some(&DiffTag::Added));
    }

    #[test]
    fn a_dropped_field_gets_a_line_of_its_own() {
        let prev = json!({"a": 1, "error": "stalled"});
        let cur = json!({"a": 1});
        let lines = tags(&cur, Some(&prev));
        let removed: Vec<_> = lines
            .iter()
            .filter(|(_, t)| *t == DiffTag::Removed)
            .collect();
        assert_eq!(removed.len(), 1, "{lines:?}");
        assert!(removed[0].0.starts_with("error"), "{:?}", removed[0]);
    }

    #[test]
    fn containers_are_never_themselves_marked() {
        // Otherwise a single deep change lights up every enclosing brace.
        let prev = json!({"pose": {"x": 1, "y": 2}});
        let cur = json!({"pose": {"x": 9, "y": 2}});
        let lines = render(&cur, Some(&prev));
        for l in &lines {
            if l.text.ends_with('{') || l.text == "}" {
                assert_eq!(l.tag, DiffTag::Same, "{l:?}");
            }
        }
        assert_eq!(tag_of(&cur, Some(&prev), "x"), DiffTag::Changed);
    }

    #[test]
    fn nesting_shows_up_as_indent() {
        let cur = json!({"pose": {"x": 1}});
        let lines = render(&cur, None);
        let x = lines.iter().find(|l| l.text.starts_with("x")).unwrap();
        assert_eq!(x.indent, 2);
    }

    #[test]
    fn array_elements_compare_by_position() {
        let prev = json!({"xs": [1, 2, 3]});
        let cur = json!({"xs": [1, 9, 3]});
        let paths = changed_paths(&cur, &prev);
        assert_eq!(paths.get("/xs/1"), Some(&DiffTag::Changed));
        assert!(!paths.contains_key("/xs/0"));
    }

    #[test]
    fn a_shortened_array_reports_the_missing_tail() {
        let prev = json!({"xs": [1, 2, 3]});
        let cur = json!({"xs": [1, 2]});
        assert_eq!(
            changed_paths(&cur, &prev).get("/xs/2"),
            Some(&DiffTag::Removed)
        );
    }

    #[test]
    fn a_type_change_counts_as_a_change() {
        let prev = json!({"v": 1});
        let cur = json!({"v": "1"});
        assert_eq!(tag_of(&cur, Some(&prev), "v"), DiffTag::Changed);
    }

    #[test]
    fn identical_documents_report_nothing() {
        let v = json!({"a": {"b": [1, 2]}, "c": null});
        assert!(changed_paths(&v, &v.clone()).is_empty());
        for (_, tag) in tags(&v, Some(&v.clone())) {
            assert_eq!(tag, DiffTag::Same);
        }
    }

    #[test]
    fn a_bare_scalar_payload_still_diffs() {
        // Not every payload is an object — plain numbers and strings are common.
        assert_eq!(tag_of(&json!(2), Some(&json!(1)), "2"), DiffTag::Changed);
        assert_eq!(tag_of(&json!(1), Some(&json!(1)), "1"), DiffTag::Same);
    }

    #[test]
    fn null_is_a_value_not_an_absence() {
        // `{"error": null}` -> `{"error": "stalled"}` is the transition being
        // hunted; treating null as missing would hide it.
        let prev = json!({"error": null});
        let cur = json!({"error": "stalled"});
        assert_eq!(tag_of(&cur, Some(&prev), "error"), DiffTag::Changed);
        assert_eq!(
            changed_paths(&cur, &prev).get("/error"),
            Some(&DiffTag::Changed)
        );
    }
}
