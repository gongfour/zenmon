# Issue #11 — `keyexpr` Test Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans.

**Goal:** A pure, no-session `keyexpr` command that reports how two key expressions relate (intersects / includes / equal), for deterministic agent diagnostics.

**Architecture:** Comparison logic + serializable result live in `zenmon_core::keyexpr` using Zenoh's `keyexpr::intersects()` / directional `keyexpr::includes()`. The CLI arm opens no session. Invalid key expressions → `invalid_input` (#6).

## Global Constraints

- Output: `intersects`, `a_includes_b`, `b_includes_a`, `equal`, and a single `relation` (`equal | a_includes_b | b_includes_a | overlaps | disjoint`).
- `a_includes_b` = "A contains every key of B" — documented with a README example.
- No intersection-witness generation (out of scope).
- No session opened.

---

### Task 1: `zenmon_core::keyexpr` compare + types + tests
**Files:** Create `crates/zenmon-core/src/keyexpr.rs`; `pub mod keyexpr;` in lib.
- `Relation` enum (serde snake_case), `KeyExprRelation` struct (Serialize), `compare(a, b) -> Result<KeyExprRelation, ZenmonError>`.
- Tests: exact match (Equal), `a/*` ⊇ `a/b` (AIncludesB), `a/**` ⊇ `a/b/c`, reversed (BIncludesA), `a/*/c` vs `a/b/*` (Overlaps), disjoint (Disjoint), invalid syntax → invalid_input.
- Commit.

### Task 2: `keyexpr` CLI command
**Files:** `crates/zenmon-cli/src/cli.rs`, `crates/zenmon-cli/src/main.rs`.
- `Keyexpr { a: String, b: String }`. Arm: `compare` → JSON (`serde_json::to_string`) or human table. No session.
- Build + `cargo test`.
- Commit.

### Task 3: Deterministic integration test + README
**Files:** Create `crates/zenmon-cli/tests/keyexpr.rs`; `README.md`.
- Test (no router): `keyexpr --json 'a/*' 'a/b'` → `a_includes_b==true`, `relation=="a_includes_b"`, exit 0; invalid → exit 2 with invalid_input JSON error.
- README: short `keyexpr` section with the `a_includes_b` direction example.
- Commit.

## Self-Review

- Covers #11 scope; witness generation excluded. Pure/offline → fully deterministic tests.
- Type consistency: `compare` returns `KeyExprRelation`; CLI serializes it directly.
