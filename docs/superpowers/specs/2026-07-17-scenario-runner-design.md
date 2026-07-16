# `zenmon scenario` — correlated multi-topic diagnostic session recorder

## Purpose

Let an AI diagnose robot behavior (e.g. "why did the mission stall?") in ONE
command: optionally trigger an action/task, capture a correlated set of topics
over a bounded window, and emit a single structured **episode JSON** that an AI
reads to reason about cause & effect. It is **data-only** — it does NOT judge;
it produces clean correlated data, and the AI/user diagnoses.

This replaces a painful manual workflow (separately backgrounding `sub`, firing
`pub`, then hand-correlating NDJSON).

## Command surface

```
zenmon scenario \
  [--observe <KEY_EXPR>]...        # topics to record (repeatable)
  [--preset stall]                 # expands to a built-in diagnosis topic set
  [--prefix <BASE>]                # prefix for --preset expansion (default: "**")
  [--pub <KEY> <VALUE>]            # optional one-shot actuation trigger
  [--task <PREFIX> <REQUEST_JSON>] # optional Task trigger (dotori 3-topic pattern)
  --for <DURATION>                 # capture window, required (e.g. 8s)
  [--settle <DURATION>]            # extra observe time after trigger/window ends
```

- At least one of `--observe` / `--preset` is required.
- `--pub` and `--task` are mutually exclusive (clap `conflicts_with`).
- Global `--json` selects the episode JSON; otherwise a compact human summary.
- `--for` / `--settle` reuse `crate::duration::parse_duration_arg`.

### `--task <PREFIX> <REQUEST_JSON>` semantics

dotori Tasks follow a 3-topic pattern: publish the request to `<PREFIX>/request`
and the server streams `<PREFIX>/feedback` and finally `<PREFIX>/response`. So
`--task`:

1. Automatically ADDS `<PREFIX>/feedback` and `<PREFIX>/response` to the
   observed set.
2. Starts observing FIRST, then publishes `REQUEST_JSON` to `<PREFIX>/request`.
3. Ends the scenario when a message arrives on `<PREFIX>/response` OR `--for`
   elapses (whichever first), then observes `--settle` more.

Completion detection matches the response topic **by exact key-expression
equality** (`<PREFIX>/response`), since dotori publishes the response on that
concrete key. `ended_reason` is `task_response` when the response arrived first,
else `window_elapsed`.

### `--preset stall` expansion

Expands to the mission-stall diagnosis set, each prefixed with `<--prefix>/`
(default prefix `**`):

- `topic/safety/safety_state`
- `topic/safety/policy/**`
- `topic/sensor/obstacles`
- `topic/mission/state_snapshot`
- `topic/navigation/robot_pose`
- `topic/forklift/snapshot`
- `topic/actionflow/**`
- `task/**/feedback`
- `task/**/response`

With the default prefix `**` they become `**/topic/safety/safety_state` etc.
(prefix-agnostic).

## Output — episode JSON

One JSON object in `--json` mode:

```json
{
  "meta": {
    "trigger": {"kind":"task","request_key":"…/request","request_bytes":N}
             | {"kind":"pub","key_expr":"…","bytes":N}
             | {"kind":"none"},
    "for_ms": 8000,
    "settle_ms": 2000,
    "observed": ["…resolved key exprs…"],
    "message_count": M,
    "ended_reason": "task_response" | "window_elapsed"
  },
  "topics": { "<key_expr>": {"count": N, "first_t_rel_ms": T0, "last_t_rel_ms": T1} },
  "correlations": {
    "<correlation_id>": [ {"t_rel_ms": T, "key_expr": "…", "request_id": "…|null", "kind":"PUT"} ]
  },
  "timeline": [
    {"t_rel_ms": T, "key_expr": "…", "correlation_id": "…|null", "request_id": "…|null",
     "encoding": "…", "payload": <decoded JSON>}
  ]
}
```

- `t_rel_ms` = ms since scenario start (first observation / trigger).
- `correlations` groups only events carrying a `correlation_id` attachment (the
  causal chains: mission→action→drive→safety share it). Events without one
  appear only in `timeline` with `correlation_id: null`.
- `payload` uses the decoded view (`MessagePayload::to_view()`), so msgpack
  shows as JSON.
- Human mode prints a compact summary: trigger, per-topic counts, correlation
  chain count, ended_reason.

## Architecture (testability)

Split the PURE structuring from the network IO:

- `zenmon-core/src/scenario.rs` (pure, no clocks / no network):
  - `ScenarioEvent { t_rel_ms, key_expr, correlation_id, request_id, encoding,
    kind, payload }`.
  - `ScenarioMeta` (trigger, for/settle windows, observed keys, ended_reason).
  - `TriggerInfo` enum: `None` / `Pub` / `Task`.
  - `pub fn build_episode(meta, events) -> serde_json::Value` — computes
    `topics`, `correlations`, and time-ordered `timeline`; `message_count` from
    the event count.
  - `pub fn expand_preset(name, prefix) -> Vec<String>`.
- `zenmon-cli/src/main.rs` `Command::Scenario` handler does the orchestration:
  resolve observed keys (observe + preset + task-derived), open a session,
  subscribe to each key (reuse `zenmon_core::subscriber`), optionally trigger,
  stamp each received message with `t_rel_ms` from a `std::time::Instant`
  stopwatch, extract `correlation_id`/`request_id` from the attachment JSON,
  build `ScenarioEvent`s, and on completion call `build_episode` and print it.
  Bounded by `--for`/`--settle` (always terminates) and respects Ctrl+C.

## TDD

Pure unit tests in `scenario.rs`: `build_episode` (topic counts + first/last,
correlation grouping in time order, null-correlation events only in timeline,
timeline ordering + decoded payloads, empty → empty sections) and
`expand_preset`. CLI parse tests in `cli.rs` mirror the existing style
(`--pub`/`--task` mutually exclusive, `--for` required, at least one of
`--observe`/`--preset`). No flaky live-capture tests.
