# MessagePack Payload Auto-Decode — Design

**Date:** 2026-07-16
**Status:** Approved (design)
**Scope:** `zenmon-core` payload rendering (`MessagePayload`)

## Motivation

`dotori_rcs` (and its sibling services) serialize almost every Zenoh topic
payload with `nlohmann::json::to_msgpack(json_object)` — see
`dotori_rcs/include/network/msgpack_serializer.hpp:38`. On the wire these are
binary MessagePack maps, **not** JSON and **not** UTF-8 text.

zenmon's current `MessagePayload` tries JSON parse, then UTF-8 string, then
falls back to a base64 object. So dotori msgpack bodies render only as
`{"binary_base64":..,"bytes":N}` (JSON mode) or `<N bytes>` (text/TUI mode) —
unreadable. This blocks using zenmon to inspect dotori message *content*.

dotori does **not** tag payloads with an `application/msgpack` encoding (no such
constant exists; publishers set no explicit encoding — see
`dotori_rcs/include/network/sample_types.hpp:20-28`), so an encoding-hint-based
approach would not reliably trigger. Detection must be content-based.

## Goal & Scope

Add a **conservative, content-based MessagePack decode** step to
`MessagePayload` rendering: when JSON and UTF-8 decoding both fail, attempt a
msgpack decode; on success render as human-readable JSON, otherwise keep the
existing base64 fallback.

### Out of scope (YAGNI)
- No CLI flag — decoding is automatic (content-based fallback).
- No encoding-hint logic — dotori does not tag msgpack encoding.
- No other formats (protobuf, CBOR, …).
- No change to the capture NDJSON format or replay path.

## Decode Pipeline

`MessagePayload` gains a private helper `as_msgpack() -> Option<serde_json::Value>`.
Every render path follows this order:

```
1. as_json()     → if it parses as JSON, use it as-is
2. as_str()      → if valid UTF-8, use the string
                   (a dotori payload is a small fixmap leading with 0x80–0x8f,
                    which are UTF-8 continuation bytes — invalid as a leading
                    byte — so it fails the UTF-8 check and reaches step 3.
                    Larger map16/map32 headers are valid 2-byte UTF-8 leads in
                    principle, but a full msgpack map being valid UTF-8 end-to-end
                    is vanishingly unlikely.)
3. as_msgpack()  → conservative decode; on success render as JSON   ← NEW
4. base64 fallback → {"binary_base64":..,"bytes":N} (JSON) / "<N bytes>" (text)
```

### Conservative acceptance rule

`as_msgpack()` returns `Some(json)` **only if all** hold:

1. `rmpv` decodes the bytes into a `Value` without error.
2. The decode **consumes the entire buffer** (no trailing bytes).
3. The **top-level value is a Map or Array** (bare scalars are rejected).

Rationale: dotori payloads are all msgpack maps, so this accepts 100% of real
dotori messages while rejecting the main false-positive source — short arbitrary
binary that happens to be a valid msgpack scalar (e.g. a single byte `0x05`
decoding to integer `5`). Non-dotori binary on the bus stays as base64.

## Integration Points (centralized)

All decode logic lives in `MessagePayload` (`zenmon-core/src/types.rs`). Existing
callers benefit automatically with **no changes**:

- `to_view()` / `to_view_capped()` — `--json` output and NDJSON streams
  (`types.rs:84`, `types.rs:101`).
- `pretty()` / `Display` — CLI text mode and TUI views
  (`zenmon-tui/src/views/stream.rs:134`, `views/query.rs:73`,
  `views/topics.rs:101,150`, `app.rs:51`).

Concretely, the msgpack step is inserted after the UTF-8 string check in each of
`to_view()`, `to_view_capped()`, `pretty()`, and `Display` (or via a shared
internal helper to keep the four paths consistent).

Attachments use the same `MessagePayload` type, so they gain the same behavior;
dotori attachments are JSON strings and are already caught at step 2.

## rmpv → serde_json::Value Conversion

`rmpv` is chosen over `rmp-serde` because it decodes arbitrary msgpack into a
`Value` that we can inspect for the "full consume + top-level type" rule;
deserializing straight into `serde_json::Value` via `rmp-serde` makes those
checks awkward.

Conversion rules:

| msgpack (`rmpv::Value`) | JSON (`serde_json::Value`) |
|---|---|
| Map | object; **non-string keys stringified** |
| Array | array |
| Integer / Float | number |
| Boolean | bool |
| Nil | null |
| String (utf8) | string |
| Binary (bytes) | **base64 string** (JSON-safe) |

## Capture / Replay — No Impact

`from_zbytes()` stores the **original wire bytes** (`types.rs:42`). Decoding is a
display-only view over those bytes; it never mutates stored bytes. The capture
NDJSON format and replay re-publish path are unchanged, so record → replay
round-trips remain byte-exact.

## Error Handling

- Any `rmpv` decode error → treat as "not msgpack", fall through to base64.
- Trailing bytes or non-container top-level → rejected, fall through to base64.
- No panics: `as_msgpack()` returns `Option`, never unwraps decode results.

## Testing

Unit tests in `zenmon-core/src/types.rs`:

- msgpack map → correct JSON object.
- scalar msgpack (`0x05`) → rejected → base64 fallback.
- msgpack with trailing garbage bytes → rejected.
- msgpack map containing a Binary field → Binary rendered as base64 string.
- Regression: valid UTF-8 text still renders as a string; real JSON still
  renders as JSON (msgpack step not reached).

## Dependencies

Add `rmpv = "1"` to the workspace `Cargo.toml` and to
`crates/zenmon-core/Cargo.toml`.
