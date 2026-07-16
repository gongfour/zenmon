# Issue #5 — Bounded sub/watch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans.

**Goal:** Give `sub`, `nodes --watch`, and `liveliness --watch` bounded termination via `--count N` / `--duration <dur>` so agents can call them as one-shot tool calls, and make every watch loop emit clean NDJSON (no ANSI) in JSON mode.

**Architecture:** A generic, paused-time-testable `watch::run_bounded(bounds, next, emit)` in `zenmon-cli` owns the count/deadline/Ctrl+C termination; each command supplies a `next` future (its item source, returning `None` on Ctrl+C or stream close) and an `emit` closure (its formatting). Deadline uses `tokio::time::Instant`.

## Global Constraints

- `--count`: `sub` = received messages, `liveliness --watch` = change events, `nodes --watch` = emitted snapshots. `nodes --watch --count 1` = one immediate snapshot then exit.
- `--count` + `--duration` → exit 0 on whichever fires first. `0` rejected as input error.
- Duration observation starts when the watch is ready.
- JSON watch output is NDJSON (one object per line), never ANSI. Human watch may keep ANSI screen-clear.
- Normal termination → exit 0.

---

### Task 1: `watch::run_bounded` + `Bounds` with paused-time tests
**Files:** Create `crates/zenmon-cli/src/watch.rs`; `mod watch;` in main.
- `Bounds { max_count: Option<u64>, duration: Option<Duration> }`.
- `async fn run_bounded<T, F: FnMut()->Fut, Fut: Future<Output=Option<T>>, E: FnMut(T)>(bounds, next, emit) -> u64` — returns count emitted; stops on max_count, deadline (tokio Instant), or `next()==None`.
- `#[tokio::test(start_paused=true)]`: count termination; duration termination (zero items); both set → count wins; both set → duration wins; stream-closed stops; ordering preserved.
- Commit.

### Task 2: `parse_count_arg`
**Files:** `crates/zenmon-cli/src/duration.rs`.
- `pub fn parse_count_arg(s: &str) -> Result<u64, String>` — parse u64, reject 0.
- Tests: parses positive, rejects 0, rejects non-numeric.
- Commit.

### Task 3: Add `--count`/`--duration` to clap
**Files:** `crates/zenmon-cli/src/cli.rs`.
- Add `count: Option<u64>` (value_parser `parse_count_arg`) and `duration: Option<Duration>` (value_parser `parse_duration_arg`) to `Sub`, `Nodes`, `Liveliness`.
- Build. Commit.

### Task 4: Rewrite `sub` arm on `run_bounded`
- `next` = select { `rx.recv()` | ctrl_c → None }. `emit` = existing human/JSON formatting (JSON = compact single line). Suppress "Subscribing..." banner in JSON mode.
- Build + test. Commit.

### Task 5: Rewrite `nodes --watch` arm
- Non-watch path unchanged (single snapshot). Watch path: `run_bounded` with `next` = select { `interval.tick()` → `query_admin_nodes().ok()` | ctrl_c → None }; `emit` = JSON envelope line (NDJSON) or human ANSI table. First `interval.tick()` is immediate → `--count 1` emits one snapshot. Skip the pre-loop snapshot in watch mode.
- Build + test. Commit.

### Task 6: Rewrite `liveliness --watch` arm
- Initial token list printed in human mode always; in JSON mode only when not watching (keeps JSON watch a pure event NDJSON stream). Watch: `run_bounded` with `next` = select { `sub.recv_async().ok()` | ctrl_c → None }; `emit` = event NDJSON line or human line.
- Build + test. Commit.

### Task 7: Docs
- README/help mention `--count`/`--duration`. Commit.

## Self-Review

- Covers contract 3a (NDJSON, no ANSI in JSON) + #5 termination semantics. Count/duration validation via value_parsers.
- `run_bounded` is deterministic and paused-time-tested; live-router loop behavior is manual.
- Type consistency: `next` returns `Option<T>`, `emit` takes `T`; `Bounds` fields `max_count`/`duration` used across all three arms.
