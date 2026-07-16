# Foundation — Lossless Payload Fidelity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans.

**Goal:** Preserve the original wire bytes + encoding of every received message so downstream features can report accurate sizes (#14), round-trip capture/replay (#13), and measure real bandwidth (#26). Prerequisite for those issues; closes none itself.

**Architecture:** `MessagePayload` becomes a lossless byte holder with lazy structured/text views; `ZenohMessage` gains `encoding`, `payload_bytes`, `attachment_bytes`. Receive paths (subscriber/query) capture bytes+encoding; TUI/CLI render via new view methods.

## Global Constraints

- No original bytes discarded (binary included).
- JSON view: parsed JSON if parseable, else UTF-8 string, else `{"binary_base64":..,"bytes":N}`.
- Keep TUI + CLI compiling and all tests green.

---

### Task 1: Redefine `MessagePayload` + extend `ZenohMessage`
- `MessagePayload { bytes: Vec<u8> }` with `from_zbytes/from_bytes/from_json`, `as_bytes/len/is_empty/as_str/as_json/to_view/pretty`, `Display`, custom `Serialize` → `to_view()`.
- `ZenohMessage` + `encoding: String`, `payload_bytes: usize`, `attachment_bytes: Option<usize>`.
- Add `base64` dep.
- Unit tests: byte/len preservation, JSON view, text view, binary base64.

### Task 2: Update receive paths + consumers
- `subscriber.rs`, `query.rs`: capture `sample.encoding()`, populate new fields.
- CLI `sub` arm: `payload.pretty()`.
- TUI `app.rs`/views: `payload_to_string` → `pretty()`; view matches → `pretty()`; drop unused imports; fix test constructors (`from_json` + new fields).
- Build + `cargo test` green; warning-free.

## Self-Review

- Lossless model satisfies owner's prerequisite for #13/#14/#26.
- Additive JSON fields; `sub --json` payload view unchanged for JSON/text, improved (base64) for binary.
