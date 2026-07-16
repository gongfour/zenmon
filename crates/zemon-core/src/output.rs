//! Shared JSON output shapes for the CLI.
//!
//! Finite collection queries (`discover`, `query`, `nodes`, `liveliness`,
//! `scout`, `info`) render a common envelope so agents can parse every command
//! the same way and never confuse "queried successfully, zero results" with a
//! connection failure (which is a structured error on stderr instead).

use serde::Serialize;

/// Render a finite collection as a compact `{"count":N,"items":[...]}` JSON
/// document. Invariant: `count == items.len()`. An empty slice renders exactly
/// `{"count":0,"items":[]}`.
pub fn to_collection_json<T: Serialize>(items: &[T]) -> Result<String, serde_json::Error> {
    to_collection_json_limited(items, false)
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Like [`to_collection_json`], but adds `"limited":true` when the output was
/// capped (e.g. by `--limit`), so a consumer knows `count` is the returned
/// count and more items may exist. The flag is omitted when `limited` is false,
/// keeping the plain-collection shape identical to [`to_collection_json`].
pub fn to_collection_json_limited<T: Serialize>(
    items: &[T],
    limited: bool,
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct Envelope<'a, T> {
        count: usize,
        items: &'a [T],
        #[serde(skip_serializing_if = "is_false")]
        limited: bool,
    }
    serde_json::to_string(&Envelope {
        count: items.len(),
        items,
        limited,
    })
}

/// Render the result of a `pub` action as a compact JSON object:
/// `{"ok":true,"status":"accepted","key_expr":"...","bytes":N}` (+
/// `"attachment_bytes":M` when an attachment is present).
///
/// `status` is `"accepted"` (not `"published"`): a successful `put` only means
/// the local Zenoh stack accepted the publication, not that any subscriber
/// received or processed it.
pub fn publish_accepted_json(
    key_expr: &str,
    bytes: usize,
    attachment_bytes: Option<usize>,
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct PublishResult<'a> {
        ok: bool,
        status: &'a str,
        key_expr: &'a str,
        bytes: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        attachment_bytes: Option<usize>,
    }
    serde_json::to_string(&PublishResult {
        ok: true,
        status: "accepted",
        key_expr,
        bytes,
        attachment_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_canonical() {
        let empty: &[u32] = &[];
        assert_eq!(to_collection_json(empty).unwrap(), r#"{"count":0,"items":[]}"#);
    }

    #[test]
    fn count_matches_len() {
        let json = to_collection_json(&[1, 2, 3]).unwrap();
        assert_eq!(json, r#"{"count":3,"items":[1,2,3]}"#);
    }

    #[test]
    fn count_equals_items_len_invariant() {
        let items = vec!["a", "b"];
        let json = to_collection_json(&items).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["count"].as_u64().unwrap() as usize, v["items"].as_array().unwrap().len());
    }

    #[test]
    fn limited_flag_present_when_capped() {
        let json = to_collection_json_limited(&[1, 2], true).unwrap();
        assert_eq!(json, r#"{"count":2,"items":[1,2],"limited":true}"#);
    }

    #[test]
    fn limited_flag_omitted_when_not_capped() {
        let json = to_collection_json_limited(&[1, 2], false).unwrap();
        assert_eq!(json, r#"{"count":2,"items":[1,2]}"#);
    }

    #[test]
    fn single_item_wraps_in_array() {
        // `info` renders one resource as a one-element collection.
        let json = to_collection_json(std::slice::from_ref(&"only")).unwrap();
        assert_eq!(json, r#"{"count":1,"items":["only"]}"#);
    }

    #[test]
    fn publish_without_attachment() {
        let json = publish_accepted_json("test/hello", 17, None).unwrap();
        assert_eq!(
            json,
            r#"{"ok":true,"status":"accepted","key_expr":"test/hello","bytes":17}"#
        );
    }

    #[test]
    fn publish_with_attachment() {
        let json = publish_accepted_json("test/hello", 17, Some(9)).unwrap();
        assert_eq!(
            json,
            r#"{"ok":true,"status":"accepted","key_expr":"test/hello","bytes":17,"attachment_bytes":9}"#
        );
    }
}
