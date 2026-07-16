# CLI Agent Ergonomics — Shared Design (issues #5–#11)

**Date:** 2026-07-16
**Scope:** GitHub issues #5, #6, #7, #8, #9, #10, #11 (excludes #12 MCP PoC)
**Goal:** Make the `zenmon` CLI safely and predictably usable by AI agents as tool calls:
bounded commands, structured JSON output, structured errors, stable exit codes, and
consistent time-duration options.

This is an umbrella design. Each issue is implemented on its own stacked branch and PR
(see *Delivery* below), but they share the cross-cutting contracts defined here. The
detailed per-issue design lives in each issue's GitHub comment (by the repo owner); this
document fixes the shared contracts so the stacked PRs stay coherent.

---

## Motivation

Today every command's logic lives in `zenmon-cli/src/main.rs`, errors are flattened to
`color_eyre::Report` via `eyre!(e)`, JSON output shapes differ per command (some top-level
arrays, some objects, `pub` has no `--json` branch at all), time options mix units
(`query=ms`, `scout=s`, `tui=ms`), and streaming/watch commands only stop on Ctrl+C. None
of this is safe for an agent issuing a single tool call and parsing the result. There are
also no unit tests, and the monolithic `main.rs` is not testable.

## Cross-cutting architectural change (lands with #9)

Extract per-command handling and output formatting out of `main.rs` into testable units so
that later issues can be verified with unit tests (paused-time, golden snapshots, parser
tests, stderr-JSON parsing):

- **`zenmon-core`** gains pure, serializable result/error types and pure logic
  (duration parsing helpers where they belong, key-expr comparison for #11, typed errors).
- **`zenmon-cli`** command handlers become functions that take parsed inputs and a writer /
  return a typed result, so output formatting can be unit-tested without a live Zenoh
  session. Network-touching parts stay behind `zenmon-core` calls that tests can avoid by
  testing the pure formatting/parsing seams.

We refactor only what these issues touch; no unrelated restructuring.

---

## Shared contract 1 — Typed errors (#6; used by #10, #11)

Introduce a typed error in `zenmon-core`:

```rust
pub enum ErrorKind { Connection, Timeout, InvalidInput, NotFound, Internal }

pub struct ZenmonError {
    pub kind: ErrorKind,
    pub message: String,
}
```

- `kind` serializes to a stable snake_case string: `connection | timeout | invalid_input |
  not_found | internal`.
- **JSON mode** (`--json`): on failure, write exactly one line to **stderr**:
  `{"error":{"kind":"connection","message":"..."}}` and exit non-zero. No ANSI, no
  backtrace, no `tracing` log lines mixed in.
- **Non-JSON mode**: keep the existing human-readable stderr UX.
- `message` must not leak internal backtraces or sensitive config values.
- In JSON mode the process must suppress polluting output: set the `tracing` subscriber to
  `off` and disable `color_eyre` ANSI/backtrace so stderr contains only the single JSON
  error object.
- clap parse errors (bad flags) are **out of scope** for the JSON contract in this pass;
  README states "only errors after command dispatch are JSON". (Revisit with a
  `try_parse()` entry point later if needed.)

Exit-code mapping is finalized in #10; #6 only guarantees "non-zero on error".

## Shared contract 2 — Collection envelope (#7)

Finite query results use a common envelope:

```json
{"count": N, "items": [ ... ]}
```

- Applies to: `discover`, `query`, `nodes` (non-watch), `liveliness` (non-watch),
  `scout`, and `info`.
- Invariant: `count == items.len()`. Empty result is exactly `{"count":0,"items":[]}`.
- `info` is a single resource: `{"count":1,"items":[{...}]}`. This is slightly awkward but
  keeps strict uniformity; documented in README.
- `count:0` means "queried successfully, no results" — **exit 0**. It must never be
  conflated with a connection failure (that is a JSON error + non-zero exit, contract 1).
- Action results (`pub`) do **not** use this envelope (see contract 3b).
- This is a breaking change for existing JSON consumers → release note.

## Shared contract 3a — Streaming NDJSON (#5)

Streaming / repeated output (`sub`, `nodes --watch`, `liveliness --watch`) in JSON mode:

- One JSON object per line (NDJSON), one per received message / change event / snapshot.
- **Never** emit ANSI screen-control sequences in JSON mode (today `nodes --watch` clears
  the screen and prints a human table even under `--json` — that must be fixed).
- Flush on normal termination.

## Shared contract 3b — Action result (#8)

`pub` in JSON mode writes exactly one JSON object to **stdout** and nothing duplicated to
stderr:

```json
{"ok":true,"status":"accepted","key_expr":"test/hello","bytes":17}
```

- `status:"accepted"` (not `"published"`): `session.put(...).await` only means the local
  Zenoh stack accepted the publication, not a delivery ACK. Documented.
- `bytes` = `value.as_bytes().len()`. If an attachment is present, also include
  `attachment_bytes`.
- Non-JSON mode keeps the existing stderr message.
- On `put` failure, emit the structured error (contract 1).

## Shared contract 4 — Duration options (#9)

All user-facing time options accept a duration string parsed with `humantime` at the CLI
boundary; internal functions receive `std::time::Duration`, never bare integers with
implicit units.

- `query --timeout 5s`  (was `--timeout <ms>`)
- `scout --per-port-timeout 1s`  (was `<seconds>`)
- `tui --refresh 100ms`  (was `<ms>`)
- `#5 --duration 5s`
- Reject `0`/`0s` for timeouts and watch durations (input error, contract 1
  `invalid_input`). Reject bad suffixes and overflow.
- Defaults preserve today's behavior (5s / 1s / 100ms).
- Bare integers are **no longer silently reinterpreted** → breaking change, release note.
- Scope is limited to user-facing CLI options; fixed internal admin/discover timeouts in
  core are out of scope.

## #5 bounded termination (uses contract 3a + 4)

Add `--count <N>` and `--duration <dur>` to `sub`, `nodes --watch`, `liveliness --watch`:

- `--count`: for `sub` = number of received messages; for `liveliness --watch` = number of
  change events; for `nodes --watch` = number of emitted snapshots.
- `nodes --watch --count 1` emits exactly one initial snapshot then exits.
- `--count` + `--duration` together: exit 0 on whichever condition is met first.
- `0` is rejected as an input error.
- Duration observation starts once the subscriber/watch is ready.
- Normal termination → exit 0, flush.
- Tests (Tokio paused time): count termination, duration termination, both racing,
  zero-message duration termination, every NDJSON line parses, no ANSI in JSON mode.

## #10 exit codes + connect timeout (uses contract 1 + 4)

Split into two concerns:

1. Typed error → stable exit codes, consistent with #6 JSON `kind`:
   - `0` success (including empty results),
   - distinct non-zero codes per kind (e.g. `invalid_input=2`, `connection=3`,
     `timeout=4`, `not_found=5`, `internal=1`). Finalized in the #10 plan; documented.
2. `--connect-timeout <dur>` + connection verification. Prefer Zenoh's
   `connect/timeout_ms` / `connect/exit_on_failure` config over a bare
   `tokio::time::timeout(open_session)` wrapper, because "session object created" ≠
   "expected router connected". `client` mode may require a router; `peer` mode with zero
   routers can be healthy.
   - Also fix `session_info()` mode inference: today it guesses `client` when routers exist
     else `peer`, which misclassifies both a peer connected to a router and a disconnected
     client. Pass/display the actual configured mode before documenting `info` as a
     healthcheck.
- Completion criteria: empty query → exit 0; `client` with no router → connection error
  after the deadline; `peer` with zero routers → healthy by policy; JSON `kind` and exit
  code stay consistent.

## #11 keyexpr command (uses contract 1)

New `keyexpr` (a.k.a. keyexpr-test) command — pure, no session:

- Inputs: two key expressions A and B.
- Output (JSON): booleans `intersects`, `a_includes_b`, `b_includes_a`, plus `equal` and a
  single `relation` field to make direction explicit. Use Zenoh's `keyexpr::intersects()`
  and directional `keyexpr::includes()`.
- `a_includes_b` means "A contains every key of B" — documented with a README example.
- **Excluded:** emitting a representative intersection key/expression (the API only decides
  existence; a witness is non-unique and would balloon scope).
- Invalid / non-canonical key expressions → `invalid_input` error (contract 1).
- Comparison logic + serializable result type in `zenmon-core`; clap/output in CLI.
- Tests: exact match, `*`, `**`, reversed inclusion, partial intersection, disjoint,
  invalid syntax.

---

## Delivery — stacked branches & PRs

Order is dependency-optimized. Each branch is stacked on the previous; each PR's base is
the previous branch (GitHub auto-retargets to `master` as earlier PRs merge). Every PR uses
`Closes #N`.

| Order | Issue | Branch | Base |
|-------|-------|--------|------|
| 1 | #9  | `feat/9-duration-parser`  | `master` |
| 2 | #6  | `feat/6-json-errors`      | `feat/9-duration-parser` |
| 3 | #7  | `feat/7-json-envelope`    | `feat/6-json-errors` |
| 4 | #8  | `feat/8-pub-json`         | `feat/7-json-envelope` |
| 5 | #5  | `feat/5-bounded-sub`      | `feat/8-pub-json` |
| 6 | #10 | `feat/10-exit-codes`      | `feat/5-bounded-sub` |
| 7 | #11 | `feat/11-keyexpr-test`    | `feat/10-exit-codes` |

Per issue: short plan → TDD → `cargo test` + `cargo clippy` green → commit → PR.

## Testing strategy

- Prefer pure unit tests that need no live router: duration parser, key-expr comparison,
  error→JSON/exit mapping, envelope serialization (`count == items.len()`), NDJSON
  line-parse.
- `#5` termination logic tested with Tokio paused time.
- Golden/snapshot tests fix each command's `--json` stdout.
- Live-router integration (needs `zenohd`) is kept out of the automated suite; documented
  as manual testing per CLAUDE.md.

## Out of scope

- #12 MCP crate PoC (separate follow-up; depends on the typed core landed here).
- clap parse-error JSON envelope (documented limitation this pass).
- Intersection-witness generation for #11.
- Refactoring beyond what these issues touch.
