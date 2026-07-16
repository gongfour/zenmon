# Issue #7 — JSON Collection Envelope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans.

**Goal:** Unify finite-collection `--json` output on `{"count":N,"items":[...]}` so agents parse every query command identically and can't confuse "0 results" with a connection error.

**Architecture:** One helper `zemon_core::output::to_collection_json` renders a compact envelope; each finite command's JSON branch calls it. `info` (single resource) wraps as a one-element collection.

## Global Constraints

- Invariant `count == items.len()`; empty renders exactly `{"count":0,"items":[]}`.
- Applies to `discover`, `query`, `nodes` (non-watch), `liveliness` (non-watch), `scout`, `info`.
- Streaming/watch output and `pub` are out of scope (contracts 3a/3b).
- Breaking change for JSON consumers (was top-level array / bare object) → release note.

---

### Task 1: `to_collection_json` helper + tests
- Create `crates/zemon-core/src/output.rs`, `pub mod output;` in lib.
- `pub fn to_collection_json<T: Serialize>(items: &[T]) -> Result<String, serde_json::Error>`.
- Tests: empty canonical, count matches, invariant `count==len`, single-item wraps in array.
- Commit.

### Task 2: Wire into finite commands
- `discover`, `query`, `nodes`, `liveliness`, `scout` JSON branches → `to_collection_json(&items)`.
- `info` → `to_collection_json(std::slice::from_ref(&detail))` → `{"count":1,"items":[{...}]}`.
- Build + `cargo test`. Commit.

## Self-Review

- Covers contract 2 (collection envelope) for all six commands. `scout` keeps its hit-filter then wraps.
- Live-router golden output documented as manual (needs `zenohd`); envelope shape verified deterministically at unit level.
- Type consistency: helper signature matches all call sites (`&[T]`).
