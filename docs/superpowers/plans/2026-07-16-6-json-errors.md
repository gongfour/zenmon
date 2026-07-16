# Issue #6 — Typed Errors + JSON Error Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox syntax.

**Goal:** In `--json` mode, failures emit exactly one JSON object `{"error":{"kind":...,"message":...}}` on stderr with a non-zero exit and no ANSI/backtrace/tracing noise. A typed `ZenmonError`/`ErrorKind` lives at the core/CLI boundary.

**Architecture:** Add `zenmon-core::error` with `ErrorKind` (serde snake_case) and `ZenmonError`. `open_session` returns `ZenmonError` (Connection). `main` splits into a thin entrypoint (parses CLI, configures logging/color, renders errors) and an inner `run() -> Result<(), ZenmonError>` holding the command match. Untyped `color_eyre::Report` and `serde_json::Error` convert to `Internal`; user-input parse errors map to `InvalidInput`.

**Tech Stack:** Rust, serde, serde_json, color-eyre.

## Global Constraints

- `kind` serializes snake_case: `connection | timeout | invalid_input | not_found | internal`.
- JSON error is one line, no ESC (`\x1b`), on stderr; exit non-zero (code `1` for now; #10 finalizes per-kind codes).
- JSON mode suppresses tracing (`off`) and does not install color_eyre's colored hook.
- clap parse errors are out of scope (documented).

---

### Task 1: `ZenmonError` / `ErrorKind` in core with tests

**Files:**
- Create: `crates/zenmon-core/src/error.rs`
- Modify: `crates/zenmon-core/src/lib.rs` (add `pub mod error;`)

**Interfaces:**
- Produces: `zenmon_core::error::{ErrorKind, ZenmonError}`; constructors `connection/timeout/invalid_input/not_found/internal`; `ZenmonError::to_json(&self) -> String`; `From<color_eyre::Report>` and `From<serde_json::Error>` (both → `Internal`); `Display` = message; `impl std::error::Error`.

- [ ] **Step 1: Write error.rs with inline tests** (full code in implementation).
- [ ] **Step 2: Wire `pub mod error;` in lib.rs.**
- [ ] **Step 3: `cargo test -p zenmon-core error::` → all pass.**
- [ ] **Step 4: Commit.**

Tests: kind serde strings; `to_json` exact string + no `\x1b` + no `\n`; `From<Report>`→Internal; `From<serde_json::Error>`→Internal.

### Task 2: `open_session` + `parse_port_range` return typed errors

**Files:**
- Modify: `crates/zenmon-core/src/session.rs`
- Modify: `crates/zenmon-cli/src/main.rs` (`parse_port_range` → `ZenmonError::invalid_input`)

**Interfaces:**
- Produces: `open_session(&ZenmonConfig) -> Result<Session, ZenmonError>` (Connection on open failure, InvalidInput on config error); `parse_port_range(&str) -> Result<(u16,u16), ZenmonError>`.

- [ ] Change signatures, map errors. tui caller uses `format!("{}", e)` (Display) — unaffected. Build.
- [ ] Commit.

### Task 3: Split `main` into entrypoint + `run()`, render errors

**Files:**
- Modify: `crates/zenmon-cli/src/main.rs`

- [ ] `main` (no `Result` return): parse CLI, `color_eyre::install()` only when `!is_json`, tracing filter `off` when `is_tui || is_json`, call `run`, on `Err` render JSON (`e.to_json()`) or human (`Error: {e}`) to stderr + `process::exit(1)`.
- [ ] `run(cli: Cli, config: ZenmonConfig) -> Result<(), ZenmonError>` wraps the existing command match unchanged (the `?` conversions handle typing).
- [ ] Build + `cargo test`.
- [ ] Commit.

### Task 4: Deterministic integration test (no router)

**Files:**
- Create: `crates/zenmon-cli/tests/json_errors.rs`

- [ ] Test: `zenmon --json scout --port-range not-a-range` → exit != 0, stderr is single-line JSON, `error.kind == "invalid_input"`, stderr contains no `\x1b`. Uses `env!("CARGO_BIN_EXE_zenmon")`.
- [ ] `cargo test -p zenmon-cli --test json_errors` → pass.
- [ ] Commit.

## Self-Review

- Covers contract 1: typed error, JSON envelope on stderr, non-zero exit, no ANSI, tracing suppressed, backtrace not shown (we render plain message), message has no internal backtrace. clap errors documented out of scope.
- Exit-code-per-kind deferred to #10 (comment split). #6 exits `1` for all errors — non-zero as required.
- Type consistency: `to_json` used by main; constructors used by session/parse_port_range.
