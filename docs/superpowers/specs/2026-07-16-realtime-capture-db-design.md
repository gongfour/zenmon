# Realtime Capture Store + Trace Reader — Design (`cli-realtime-capture-db`)

**Date:** 2026-07-16
**Scope:** A continuously-running capture *store* (chunked NDJSON with retention) plus a
pure, network-free *reader* (`trace stats` / `trace read`) so an AI agent can inspect what
happened on a Zenoh network **while the agent was absent** — the time-shifted case that a
live, bounded subscription can never cover.
**Goal:** Decouple *collection* (a long-lived process that always records) from *reading*
(what the agent calls). The agent never holds a live subscription; it queries an
already-recorded, bounded, filterable store.

This document fixes the design agreed during brainstorming. Implementation follows in a
separate plan (`writing-plans`).

---

## Motivation

An AI agent operates in discrete, stateless turns and cannot hold a Zenoh subscription open
across turns. To observe "what happened at 3am" or "the event that fires once an hour," the
subscription must already have been running — outside the agent's lifecycle. Live bounded
commands (`sub --count/--duration`) solve *"what is happening now"* but structurally cannot
solve *"what happened while I was away."*

The fix is Architecture B: a **collector** records continuously into a **store**, and the
agent reads the store through a **reader** command. The `capture`/`replay` work (#13)
already gives us a lossless, versioned NDJSON record and a single-file writer; the only
consumer of that file today is `replay` (which re-publishes). This design adds (a) a
long-run *rotating* store with retention, and (b) a read/query surface an agent can actually
use at GB scale.

### Relationship to `zemon-mcp` (#12) — no overlap

`feat/12-zemon-mcp` currently contains only a design doc (no code). Its tool set is
read-only + `pub` and **explicitly excludes `capture`/`replay`**, and its streaming boundary
is "bounded snapshot, no server push" — so an MCP agent also only sees data inside its own
call window and shares the (b) limitation this design removes. The only shared source file
is `zemon-cli/src/cli.rs` (each branch adds a distinct additive `Command` variant — not a
conflict).

Because our reader is **pure and read-only**, it does *not* fall under #12's exclusion
rationale ("file I/O side effects"). We therefore keep the reader pure and reuse the CLI's
serde types, `{count, items}` envelope, and `ZemonError` taxonomy, so `trace_stats` /
`trace_read` can later be wrapped as MCP tools with one contract to maintain. The collector
process and any future MCP session are **separate processes** that share only files, never a
Zenoh session — an intended decoupling.

---

## Key decisions (from brainstorming)

1. **Collector lifecycle:** the collector is a plain long-running foreground command,
   supervised by the OS/user (Windows Task Scheduler / `nssm` / a long-lived terminal), not
   a zemon-managed daemon. Real "always-on" (restart-on-crash, start-on-boot) is the OS's
   job; a pidfile + self-spawn gives false confidence. A `zemon recorder start/stop/status`
   convenience layer is left as future room, not built now.
2. **Store:** chunked NDJSON (segment files), not a single growing file (which cannot be
   front-pruned) and not SQLite (adds a dependency, WAL/VACUUM/concurrency, loss of
   greppability). SQLite may be added later as an optional backend behind the same reader.
3. **Reader shape:** two single-purpose subcommands — `trace stats` (per-topic rollup) and
   `trace read` (filtered raw records) — each returning exactly one output shape, matching
   the existing granular CLI style.
4. **Reads are bounded and honest at scale:** default `--limit`, time-window segment
   skipping, opaque cursor pagination, server-side reducers, payload capping, and an
   explicit match-total in every response (never a silent truncation).
5. **Record model:** add a receiver wall-clock timestamp (`received_at`) to the record and
   bump the schema to v2 (back-compatible parse), so every record is globally
   time-filterable across segments and restarts.

---

## Architecture

```
crates/
  zemon-core/   # + trace.rs (segment discovery, time-skip, filter, rollup, cursor) — PURE
                # + capture.rs gains a rotating segment writer + retention
  zemon-cli/    # + `Command::Trace { Stats, Read }`; `Command::Capture` gains --dir mode
  zemon-tui/    # unchanged
  zemon-mcp/    # (separate branch #12) may later wrap trace_stats/trace_read
```

- **Collector** = `zemon capture --dir <path> <key_expr>` running long-lived. It writes the
  same `CaptureRecord` lines (#13) into rotating segment files and enforces retention.
- **Store** = a directory of `*.ndjson` segments whose filenames encode the first record's
  receiver timestamp, so the reader can order and time-skip segments by name alone.
- **Reader** = `zemon trace stats|read <dir> …`, pure logic in `zemon-core::trace`, no Zenoh
  session. Testable against a fixture directory with no live recorder.

Data flow:

```
recorder process ── writes ─▶ <dir>/*.ndjson  ◀── reads ── zemon trace stats|read ─▶ agent
   (long-lived, network)        (segment store)      (pure, offline)
```

---

## Record model — schema v2 (`zemon-core::capture`)

`CaptureRecord` gains one field:

```rust
/// Receiver wall-clock time this message was recorded, RFC3339 (UTC).
/// Present from schema v2 on; absent in v1 files.
#[serde(skip_serializing_if = "Option::is_none")]
pub received_at: Option<String>,
```

- `SCHEMA_VERSION` becomes `2`; the rotating recorder always writes v2 (`received_at`
  populated). `received_offset_ms` is retained unchanged for replay timing (relative).
- `parse_line` accepts a **supported set** `{1, 2}` instead of a single version, so existing
  v1 single-file captures still `replay`. Unknown versions outside the set are still
  rejected with the line number (contract from #13).
- **Reader time source:** filter/rollup use `received_at`; for a v1 record lacking it, fall
  back to `source_timestamp`; if both are absent the record is time-*unbounded* (always
  included) and this is documented. Over the rotating store every record is v2, so this
  fallback only affects reading legacy single-file traces.

---

## Store layout & the collector (`zemon capture --dir`)

`Command::Capture` gains a rotating mode. `--output <file>` (existing single-file mode) and
`--dir <path>` (new rotating mode) are **mutually exclusive; exactly one is required.**
Rotating-mode options:

| Option | Default | Meaning |
|--------|---------|---------|
| `--dir <path>` | — | Segment directory (rotating mode). May also come from `ZEMON_TRACE_DIR`. |
| `--rotate-size <bytes>` | `64MB` | Start a new segment once the current one reaches this size… |
| `--rotate-interval <dur>` | `1h` | …or this much wall-clock time, whichever comes first. |
| `--max-total-size <bytes>` | `1GB` | Retention: delete oldest *closed* segments while total exceeds this… |
| `--max-age <dur>` | `7d` | …or delete a closed segment once it is entirely older than this. |

Existing `--count` / `--duration` still bound the whole capture (usually omitted for an
always-on recorder). Byte-size options use a dedicated byte-size parser (`64MB`, `1GB`;
binary `MiB`/`GiB` accepted and documented), added alongside the existing duration parser at
the CLI boundary.

**Segment naming:** `zemon-trace-<first_received_at:YYYYMMDDTHHMMSSZ>-<seq>.ndjson`, where
`seq` is a zero-padded per-directory counter guaranteeing order and uniqueness within the
same second. Lexical sort == chronological order.

**Segment time bounds without scanning:** a segment covers
`[own_first_ts, next_segment_first_ts)`; the newest (active) segment's upper bound is "now
/ unbounded". This lets the reader and retention reason about a whole segment from filenames
alone (see below).

**Retention** prunes only *closed* (non-active) segments: delete the oldest while
`total_size > --max-total-size`, and delete any closed segment whose upper bound
(`next_segment_first_ts`) is older than `now - --max-age`. The active segment is never
age-pruned (it has no successor bound).

**Concurrency:** one writer (the recorder, appending to the active segment) and many
readers. Readers treat a trailing partial line in the active segment as end-of-data (stop at
the last complete `\n`); they never assume the active segment is closed.

---

## Reader — `zemon trace stats` (`zemon-core::trace`)

Per-topic rollup for "what topics exist, how much, and when" — the agent's map before it
drills in. Finite → `{count, items}` envelope (contract from #7).

```
zemon --json trace stats <dir> [--key <keyexpr>] [--since <t>] [--until <t>] [--top <N>]
```

- `--key` (default `**`), `--since`/`--until` bound the window; `--top N` returns only the N
  highest-volume topics (rollup itself is bounded when a store has thousands of keys).
- Each item:
  ```json
  {"key":"forklift/1/pose","count":8123,"first_ts":"…","last_ts":"…",
   "rate_hz":12.4,"last_value_preview":"…","last_value_bytes":41,"encoding":"application/json"}
  ```
- `rate_hz = count / (last_ts − first_ts)` over the selected window (0 if a single sample).
- `last_value_preview` is capped like `--max-payload-bytes`; `last_value_bytes` is the true
  size so truncation is visible. Empty window → `{"count":0,"items":[]}`, exit 0.

## Reader — `zemon trace read` (`zemon-core::trace`)

Filtered raw records for drilling into a specific slice. Streaming shape → NDJSON (contract
from #5): one `CaptureRecord`-shaped object per line.

```
zemon --json trace read <dir> [--key <keyexpr>] [--since <t>] [--until <t>]
      [--limit <N>=100] [--last-per-key] [--every <N>] [--max-payload-bytes <N>]
      [--cursor <token>]
```

- **Bounded by default:** `--limit` defaults to `100`; unbounded requires an explicit
  sentinel (e.g. `--limit 0` documented as "no cap — use with care").
- **Server-side reducers** (turn "read a lot, return little"): `--last-per-key` collapses to
  one latest record per topic; `--every N` samples every Nth matching record.
- **Payload cap:** `--max-payload-bytes` truncates each record's payload/attachment preview;
  true byte length stays in the record so truncation is visible.
- **Pagination & honesty:** the response is terminated by a **trailer object** on its own
  line:
  ```json
  {"summary":{"returned":100,"matched":4213,"cursor":"<opaque>","truncated":true}}
  ```
  `cursor` is an **opaque** token (encodes segment + intra-segment offset); pass it back via
  `--cursor` to continue from exactly where the last page stopped, with no re-scan. `matched`
  is the **exact** number of records matching the filters within the scanned window (counting
  is cheap — the reader filters every record anyway and merely stops *serializing* past
  `--limit`); `truncated` is true when `matched > returned`. **The reader never truncates
  silently.** Agents are expected to pass `--since` on large stores so the scanned window
  stays bounded.

**Time inputs (`--since` / `--until`):** accept either a relative duration (`10m`, `2h` →
offset from now) or an absolute RFC3339 timestamp. Parsed at the CLI boundary; bad input →
`invalid_input`.

**Segment skipping:** using segment filename bounds, the reader skips (does not open)
segments entirely outside `[--since, --until]`, so a narrow window over a GB store touches
only the relevant files.

---

## Error handling & exit codes

Reuses the `ZemonError` taxonomy + stable exit codes (#6/#10):

- Bad `--key`, `--since/--until`, negative/garbage sizes, corrupt record (with line number),
  unsupported `schema_version` → `invalid_input` (exit 2).
- Missing directory → `not_found` (exit 5). An **existing but empty** store is *success*:
  `{"count":0,"items":[]}` / empty NDJSON, exit 0 (never conflated with an error).
- In `--json` mode, errors are the single-line stderr `{"error":{"kind":…,"message":…}}`
  (no ANSI, no backtrace), exactly as the rest of the CLI.

---

## Testing

Pure, no live router — the whole reader is testable against fixture directories:

- **Record model:** v1→v2 round-trip; `parse_line` accepts `{1,2}`, rejects others with line
  number; `received_at` present in v2, absent in v1.
- **Segment math:** filename parse/sort; `[first, next_first)` bounds; time-window skip
  (segment fully before/after/overlapping the window); active-segment upper bound.
- **Retention:** oldest-first deletion under size cap and age cap; active segment never
  age-pruned; both bounds racing.
- **Reader stats:** rollup counts, `rate_hz`, `last_value_preview` capping + true
  `last_value_bytes`, `--top N`, empty window → `{count:0}` exit 0, `count == items.len()`.
- **Reader read:** default limit applied; `--last-per-key`; `--every N`; payload cap;
  **cursor round-trip** (page A then `--cursor` resumes with no overlap/gap); trailer
  `matched/returned/truncated` correctness; every NDJSON line + trailer parse; no ANSI.
- **Concurrency:** trailing partial line in the active segment is treated as EOF, not a
  parse error.
- **Rotating writer:** rotation on size and on interval (Tokio paused time for interval);
  segment filenames monotonic.

Live-router capture (the recorder writing real messages) stays a manual test per CLAUDE.md.

---

## Out of scope (v1)

- `zemon recorder start/stop/status` daemon management (future convenience layer).
- SQLite/DuckDB backend (future option behind the same reader interface).
- `replay` over a segment directory (replay stays single-file; the reader owns the dir).
- Time-bucketed histograms in `stats` (`--bucket`) — candidate follow-up.
- Exposing `trace_*` as MCP tools (belongs to #12 once this lands).
- Cross-directory / remote stores; the reader operates on one local directory.

## Delivery

One implementation plan, staged so each stage is independently green (`cargo test` +
`cargo clippy`):

1. Record model v2 (`received_at`, `{1,2}` parse) — pure, unblocks everything.
2. Rotating segment writer + retention on `zemon capture --dir`.
3. `zemon-core::trace` reader logic (discovery, skip, filter, rollup, cursor) — pure.
4. `trace stats` / `trace read` CLI subcommands + `--json` output + README.
