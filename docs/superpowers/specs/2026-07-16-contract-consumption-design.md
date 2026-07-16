# Contract Consumption — Design (Milestone 1)

Date: 2026-07-16
Status: Approved design, ready for implementation planning
Branch: `feat/contract-consumption`

## Background

Zenoh is a schema-less data plane: it moves bytes on key expressions and
enforces nothing about what those bytes mean. zenmon today observes traffic and
decodes payloads by guessing (JSON → text → MsgPack → base64, in
`MessagePayload`), but it cannot tell the operator *what* a topic is, *what*
fields it should carry, *who* should produce/consume it, or whether a payload's
encoding matches expectations.

A **contract** supplies that missing schema layer. `contracts/dotori_rcs.contract.yaml`
is the first hand-authored contract (60 topics, 9 reusable types, 16 services):
per-topic key expression, messaging pattern, encoding, producers/consumers, and
a payload field schema written in a compact human-readable notation.

This milestone makes zenmon **consume** a contract to enrich what it shows —
without yet validating payloads against the schema.

## Goals

- Load and structurally validate a contract file.
- Resolve a contract path from `--contract <path>` or `ZENMON_CONTRACT`.
- Add a `zenmon contract` subcommand to inspect the contract (`lint`, `list`, `show`).
- Enrich `sub` and `discover` output with contract context: topic type/description,
  expected-vs-observed encoding, `enveloped` flag, and an "undeclared topic" warning.
- Keep all enrichment **additive and opt-in**: with no contract, behavior is unchanged.

## Non-goals (deferred to later milestones)

- Strict payload validation (interpreting the compact notation, type/field checks).
- Coverage/audit (declared-but-unseen topics, direction mismatches).
- TUI integration.
- Zenoh config-file / config-stack integration for the contract path.
- A parser for the compact schema notation (`f64`, `str?`, `[T]`, `$ref`).

## Architecture

### Data model — `zenmon-core/src/contract.rs` (new)

Because enrichment *displays* schemas rather than *validating* them, payload
schemas are kept as loosely-typed `serde_json::Value` (nested maps) rather than
parsed into a typed schema. Known metadata is typed; the schema body is raw.

```rust
pub struct Contract {
    pub version: String,
    pub project: String,
    pub encoding: EncodingDefaults,       // { default: String, default_enveloped: bool }
    pub types: serde_json::Map<String, serde_json::Value>,  // for $ref resolution
    pub topics: Vec<TopicContract>,
}

pub struct TopicContract {
    pub key: String,
    pub pattern: String,                  // pub-sub | call | task | liveliness
    pub encoding: Option<String>,         // None → encoding.default
    pub enveloped: Option<bool>,          // None → encoding.default_enveloped
    pub status: Option<String>,           // e.g. "declared-not-implemented"
    pub description: Option<String>,
    pub producers: Vec<String>,
    pub consumers: Vec<String>,
    pub payload: Option<serde_json::Value>,   // pub-sub display schema
    pub phases: Option<serde_json::Value>,    // task: request/feedback/response
    pub request: Option<serde_json::Value>,   // call
    pub response: Option<serde_json::Value>,  // call
    // Unknown extra fields are tolerated (serde default / flatten catch-all).
}
```

Serde deserialization is lenient: unknown top-level and per-topic keys are
ignored so the contract format can evolve without breaking the loader.

Public API on `Contract`:

- `Contract::from_yaml_str(&str) -> Result<Contract, ContractError>`
- `Contract::load(path) -> Result<Contract, ContractError>`
- `fn effective_encoding(&self, t: &TopicContract) -> &str`
- `fn effective_enveloped(&self, t: &TopicContract) -> bool`
- `fn lookup(&self, observed_key: &str) -> Option<&TopicContract>` — best (most
  specific) match; see Key matching.
- `fn resolve_ref(&self, value) -> Value` — expand `{ $ref: TypeName }` against `types`.
- `fn lint(&self) -> LintReport` — structural warnings.

`ContractError` uses the project's `eyre!` pattern; loader errors carry the file
path and a human message. YAML parsing uses `serde_yaml` (add dependency).

### Key matching

Observed keys are concrete (`topic/sensor/pcd/front`); contract keys may contain
doc placeholders (`topic/sensor/pcd/{sensor_id}`) and Zenoh wildcards (`*`, `**`).

Matching algorithm:

1. Normalize a contract key to a valid Zenoh key expression by replacing each
   `{segment}` placeholder with `*` (single-segment wildcard).
2. Reuse the existing `keyexpr.rs` intersect/include logic to test whether the
   normalized contract key includes the observed key.
3. If several topics match, pick the **most specific**: fewest wildcard/placeholder
   segments, then longest literal prefix. Ties broken by declaration order.

This is pure and unit-testable with no network.

### Contract path resolution — CLI

A new global flag on `Cli`:

```
--contract <PATH>     # explicit contract file
```

Resolution order (first present wins): `--contract` flag → `ZENMON_CONTRACT`
env var → none (enrichment disabled). Mirrors the spirit of the existing config
resolution but is intentionally minimal (no config-file layer this milestone).

Resolution lives in the CLI layer; `zenmon-core` only loads a given path.

### `zenmon contract` subcommand

```
zenmon contract lint [PATH]         # parse + structural checks, counts, warnings
zenmon contract list [PATH]         # one line per topic: key  pattern  encoding
zenmon contract show <KEY> [PATH]   # full entry for KEY, with $ref expanded
```

- `PATH` optional; falls back to resolved `--contract`/`ZENMON_CONTRACT`.
- All support global `--json`.
- `lint` warnings: `declared-not-implemented` topics, unresolved `$ref`,
  duplicate keys, unknown `pattern`, `enveloped/encoding` inconsistencies.
- `lint` exit code: 0 = clean, non-zero reserved for structural errors (parse
  failure). Warnings alone do not fail (report-only), matching `doctor`'s tone.
- `show <KEY>` matches by the same key-matching rule, so `show topic/sensor/pcd/front`
  resolves the `{sensor_id}` entry.

### Enrichment — `sub`

When a contract is resolved, each received message gains contract context.

Human output — a header annotation before the existing payload rendering:

```
[topic/navigation/robot_pose]  RobotPose — 로봇 2D pose
  encoding: application/json (matches)
  { ...payload as today... }

[topic/foo/unknown]  ⚠ not declared in contract
  { ...payload as today... }
```

- The type label is derived from the payload schema's leading comment / topic
  description; if absent, just the description.
- Encoding line compares `ZenohMessage.encoding` against the topic's effective
  encoding by MIME prefix (`application/json` vs `application/json;charset=…`),
  reporting `(matches)` or `(expected X, got Y)`.

JSON output (`--json`) — an additive `contract` object per event; all existing
fields are unchanged:

```json
{
  "key_expr": "topic/navigation/robot_pose",
  "payload": { ... },
  "encoding": "application/json",
  "contract": {
    "matched_key": "topic/navigation/robot_pose",
    "description": "Robot 2D pose ...",
    "encoding_expected": "application/json",
    "encoding_matches": true,
    "enveloped": true,
    "declared": true
  }
}
```

For an undeclared topic: `"contract": { "declared": false }`.

Enrichment is skipped entirely when no contract is resolved — the `contract`
key is simply absent, preserving current output byte-for-byte.

### Enrichment — `discover`

Discovered keys are annotated `declared`/`undeclared` with the description when
declared. Declared-but-unseen reporting is out of scope (coverage milestone).

## Data flow

```
--contract / ZENMON_CONTRACT ──► Contract::load ──► Contract (in memory)
                                                        │
sub/discover received key ──► Contract::lookup ──► Option<&TopicContract>
                                                        │
                                       enrichment view (human / JSON)
```

`zenmon-core` owns the model, matching, and lint. The CLI owns path resolution,
the `contract` subcommand, and wiring enrichment into `sub`/`discover`.

## Error handling

- Missing/unreadable contract file with an explicit `--contract`: hard error
  (non-zero exit) via `eyre!`, naming the path.
- Contract resolved via env but unreadable: same hard error (explicit intent).
- Malformed YAML / structural parse error: `contract lint` reports it as an
  error with location; `sub`/`discover` fail fast rather than silently running
  unenriched, so a broken contract is never mistaken for "no matches".
- A key that matches no topic is **not** an error — it is the `declared: false`
  path.

## Testing strategy

Fixture: a trimmed, self-contained copy at
`crates/zenmon-core/tests/fixtures/sample.contract.yaml` (a handful of topics
covering each pattern, a `{param}` topic, `$ref` usage, and an encoding
override) — mirrors the real `contracts/dotori_rcs.contract.yaml` shape but is
committed with the PR, since the real contract stays untracked.

- **core unit tests**: parse the fixture; `$ref` resolution; effective
  encoding/enveloped defaults vs overrides; key matching for literal, `{param}`,
  `*`, and `**` cases; most-specific tie-breaking; lint warning detection.
- **CLI integration tests** (`crates/zenmon-cli/tests/`): `contract lint`
  counts/warnings, `contract list`, `contract show <key>` incl. a `{param}`
  topic; `--json` shapes; path resolution via flag and env.
- **enrichment tests**: `sub`/`discover` JSON output includes a correct
  `contract` object for declared and undeclared keys; output with no contract is
  unchanged.

## Rollout / PR

- Delivered on branch `feat/contract-consumption` as a standalone PR.
- The contract YAML (`contracts/`) is **not** committed in this PR (kept in the
  working tree per the author's instruction); tests reference it by path, so the
  PR must either vendor a small fixture copy under `tests/fixtures/` or gate the
  fixture-dependent tests. Decision: **vendor a trimmed fixture** under
  `crates/zenmon-core/tests/fixtures/sample.contract.yaml` so tests are
  self-contained and the PR does not depend on an untracked file.
