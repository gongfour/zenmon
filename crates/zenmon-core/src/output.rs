//! Shared JSON output shapes for the CLI.
//!
//! **zenmon application internals — not part of the public API.** This module
//! encodes the `zenmon` CLI's stdout contract; it is `pub` only because
//! `zenmon-cli` is a separate crate. External consumers should format their
//! own output and must not rely on these shapes staying stable.
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

/// Render a query result: successful replies as the usual `{"count":N,"items":[...]}`
/// collection, plus an `"errors":[...]` array carrying any Zenoh reply errors the
/// queryable returned. `count`/`items` count only successful replies (the
/// invariant `count == items.len()` holds); the `errors` field is omitted when
/// there are none, so a purely successful query renders identically to
/// [`to_collection_json_limited`]. This ensures an endpoint that replies with an
/// error is not mistaken for one that never replied.
pub fn to_query_json<T: Serialize, E: Serialize>(
    items: &[T],
    errors: &[E],
    limited: bool,
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct Envelope<'a, T, E> {
        count: usize,
        items: &'a [T],
        #[serde(skip_serializing_if = "is_false")]
        limited: bool,
        #[serde(skip_serializing_if = "<[E]>::is_empty")]
        errors: &'a [E],
    }
    serde_json::to_string(&Envelope {
        count: items.len(),
        items,
        limited,
        errors,
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

/// Render the summary of a fixed-rate `pub --rate` run: the same shape as
/// [`publish_accepted_json`] plus `"published":<count>` (messages actually put)
/// and `"rate_hz":<R>` (the requested frequency). Emitted once, after the loop
/// finishes or is interrupted, so a consumer learns how many puts the local
/// stack accepted. Kept separate from [`publish_accepted_json`] so the
/// single-publish shape stays byte-for-byte identical.
pub fn publish_rate_summary_json(
    key_expr: &str,
    bytes: usize,
    attachment_bytes: Option<usize>,
    published: u64,
    rate_hz: f64,
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct PublishRateSummary<'a> {
        ok: bool,
        status: &'a str,
        key_expr: &'a str,
        bytes: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        attachment_bytes: Option<usize>,
        published: u64,
        rate_hz: f64,
    }
    serde_json::to_string(&PublishRateSummary {
        ok: true,
        status: "accepted",
        key_expr,
        bytes,
        attachment_bytes,
        published,
        rate_hz,
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
    fn query_json_without_errors_matches_plain_collection() {
        // Backward compatible: a query with only successful replies renders the
        // exact same shape as before — no `errors` field.
        let no_errors: &[String] = &[];
        let json = to_query_json(&[1, 2], no_errors, false).unwrap();
        assert_eq!(json, r#"{"count":2,"items":[1,2]}"#);
    }

    #[test]
    fn query_json_surfaces_error_replies() {
        // Reply errors are no longer dropped: they appear in an `errors` array
        // even when there are zero successful replies.
        let items: &[i32] = &[];
        let json = to_query_json(items, &["parse failed"], false).unwrap();
        assert_eq!(
            json,
            r#"{"count":0,"items":[],"errors":["parse failed"]}"#
        );
    }

    #[test]
    fn query_json_errors_do_not_count_toward_count() {
        // `count` remains the number of successful replies (items.len()).
        let json = to_query_json(&[1], &["boom"], false).unwrap();
        assert_eq!(json, r#"{"count":1,"items":[1],"errors":["boom"]}"#);
    }

    #[test]
    fn query_json_limited_and_errors_together() {
        let json = to_query_json(&[1, 2], &["boom"], true).unwrap();
        assert_eq!(
            json,
            r#"{"count":2,"items":[1,2],"limited":true,"errors":["boom"]}"#
        );
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

    #[test]
    fn publish_rate_summary_without_attachment() {
        let json = publish_rate_summary_json("test/hello", 17, None, 30, 10.0).unwrap();
        assert_eq!(
            json,
            r#"{"ok":true,"status":"accepted","key_expr":"test/hello","bytes":17,"published":30,"rate_hz":10.0}"#
        );
    }

    #[test]
    fn publish_rate_summary_with_attachment() {
        let json = publish_rate_summary_json("test/hello", 17, Some(9), 5, 2.5).unwrap();
        assert_eq!(
            json,
            r#"{"ok":true,"status":"accepted","key_expr":"test/hello","bytes":17,"attachment_bytes":9,"published":5,"rate_hz":2.5}"#
        );
    }
}
