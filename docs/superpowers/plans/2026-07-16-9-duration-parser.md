# Issue #9 — Duration Option Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify all user-facing time options on `humantime` duration strings (`5s`, `100ms`) parsed at the CLI boundary, replacing the current mixed bare-integer units (`query=ms`, `scout=s`, `tui=ms`).

**Architecture:** Add one pure parser function `parse_duration_arg` in a new `zemon-cli/src/duration.rs`, wire it into clap via `value_parser`, and change the three affected options to hold `std::time::Duration`. Core functions already take `Duration`, so only the CLI boundary and the `tui::run` signature change.

**Tech Stack:** Rust, clap v4 derive, `humantime` crate, `std::time::Duration`.

## Global Constraints

- Rust 2021 edition, workspace deps in root `Cargo.toml`.
- Zenoh error handling: `.map_err(|e| eyre!(e))`.
- Reject zero durations and unit-less integers as input errors.
- Defaults preserve today's behavior: `query`=5s, `scout --per-port-timeout`=1s, `tui --refresh`=100ms.
- English in code, conventional commits (`feat(cli):`).

---

### Task 1: Duration parser with tests

**Files:**
- Create: `crates/zemon-cli/src/duration.rs`
- Modify: `crates/zemon-cli/Cargo.toml` (add `humantime`), root `Cargo.toml` (workspace dep)
- Modify: `crates/zemon-cli/src/main.rs` (add `mod duration;`)

**Interfaces:**
- Produces: `pub fn parse_duration_arg(s: &str) -> Result<std::time::Duration, String>` — parses humantime strings, rejects zero and (implicitly, via humantime) unit-less integers. `Err` is a human message suitable for clap.

- [ ] **Step 1: Add humantime dependency**

Root `Cargo.toml` `[workspace.dependencies]`: add `humantime = "2"`.
`crates/zemon-cli/Cargo.toml` `[dependencies]`: add `humantime.workspace = true`.

- [ ] **Step 2: Write the failing tests**

Create `crates/zemon-cli/src/duration.rs`:

```rust
use std::time::Duration;

/// Parse a user-facing duration option (e.g. "5s", "100ms", "1m500ms").
/// Rejects zero durations and unit-less integers so agents get a clear
/// input error instead of a silently reinterpreted value.
pub fn parse_duration_arg(s: &str) -> Result<Duration, String> {
    let d = humantime::parse_duration(s.trim())
        .map_err(|e| format!("invalid duration '{}': {} (try e.g. 5s, 100ms)", s, e))?;
    if d.is_zero() {
        return Err(format!("duration '{}' must be greater than zero", s));
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(parse_duration_arg("1m500ms").unwrap(), Duration::from_millis(60_500));
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
```

- [ ] **Step 3: Run tests to verify they fail (module not wired)**

Run: `cargo test -p zemon-cli duration::`
Expected: compile error until `mod duration;` added.

- [ ] **Step 4: Wire the module**

In `crates/zemon-cli/src/main.rs` add `mod duration;` near `mod cli;`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zemon-cli duration::`
Expected: 6 passing.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(cli): add humantime duration parser with tests"
```

---

### Task 2: Wire parser into clap and change option types

**Files:**
- Modify: `crates/zemon-cli/src/cli.rs` (Query.timeout, Scout.per_port_timeout, Tui.refresh)
- Modify: `crates/zemon-cli/src/main.rs` (call sites, tui run call)
- Modify: `crates/zemon-tui/src/lib.rs` (`run` takes `Duration`)

**Interfaces:**
- Consumes: `crate::duration::parse_duration_arg`.
- Produces: `Command::Query { timeout: Duration, .. }`, `Command::Scout { per_port_timeout: Duration, .. }`, `Command::Tui { refresh: Duration }`, `zemon_tui::run(config, refresh: Duration)`.

- [ ] **Step 1: Change clap option types in `cli.rs`**

`use std::time::Duration;` at top. Then:

Query:
```rust
    /// Query timeout (e.g. 5s, 500ms)
    #[arg(long, default_value = "5s", value_parser = crate::duration::parse_duration_arg)]
    timeout: Duration,
```
(field stays `timeout`)

Scout:
```rust
    /// Per-port scouting timeout (e.g. 1s, 500ms)
    #[arg(long, default_value = "1s", value_parser = crate::duration::parse_duration_arg)]
    per_port_timeout: Duration,
```

Tui:
```rust
    /// UI refresh interval (e.g. 100ms, 1s)
    #[arg(long, default_value = "100ms", value_parser = crate::duration::parse_duration_arg)]
    refresh: Duration,
```

- [ ] **Step 2: Update call sites in `main.rs`**

- Query arm: replace `Duration::from_millis(timeout)` with `timeout`.
- Scout arm: replace `Duration::from_secs(per_port_timeout)` with `per_port_timeout`.
  Also `print_scout_results` currently takes `per_port_timeout: u64` for the "Xs per port"
  message — change its signature to `per_port_timeout: Duration` and format via
  `humantime::format_duration(per_port_timeout)`.
- Tui arm: `zemon_tui::run(config, refresh).await?` (now a Duration).

- [ ] **Step 3: Change `tui::run` signature**

In `crates/zemon-tui/src/lib.rs`, change `pub async fn run(mut config: ZemonConfig, tick_rate_ms: u64)` to `pub async fn run(mut config: ZemonConfig, refresh: Duration)`. Inside, wherever `tick_rate_ms` built a `Duration` (e.g. `Duration::from_millis(tick_rate_ms)`), use `refresh` directly. Confirm `Duration` is imported (it is).

- [ ] **Step 4: Build and run all tests**

Run: `cargo build && cargo test`
Expected: builds clean, all tests pass.

- [ ] **Step 5: Clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Manual smoke (help snapshot)**

Run: `cargo run -q -- query --help` and confirm `--timeout <TIMEOUT>` help shows `5s` default; `cargo run -q -- scout --help` shows `1s`; `cargo run -q -- tui --help` shows `100ms`.
Run: `cargo run -q -- query 'x/**' --timeout 5000` → expect input error mentioning units.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(cli): unify time options on humantime duration strings

query --timeout, scout --per-port-timeout, tui --refresh now accept
duration strings (5s, 100ms) instead of bare integers with implicit
mixed units. Breaking change for scripts passing bare integers.

Closes #9"
```

---

## Self-Review

- **Spec coverage:** contract 4 (duration options) fully covered: query/scout/tui unified, zero rejected, unit-less rejected, defaults preserved, breaking change noted in commit. `#5 --duration` is out of this issue's scope (lands in #5, reuses `parse_duration_arg`).
- **Placeholder scan:** none.
- **Type consistency:** `parse_duration_arg` signature matches clap `value_parser` requirement (`Fn(&str) -> Result<T, E>` where `E: Display`). `run(config, refresh: Duration)` matches call site.
- **Release note:** breaking change documented in commit body; README update folded into PR body.
