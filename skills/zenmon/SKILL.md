---
name: zenmon
description: >
  Use when debugging or observing a Zenoh network — messages not arriving,
  pub/sub not connecting, "who is on the network?", inspecting live topics, or
  diagnosing Zenoh connectivity. Read-only diagnostics driven through the zenmon
  CLI. Does NOT publish or mutate the network without explicit user confirmation.
---

# Debugging a Zenoh network with zenmon

`zenmon` is a standalone CLI for monitoring and debugging Zenoh networks. Use it
to diagnose a Zenoh problem in the current project **read-only**. You are not
importing a library — you run the `zenmon` binary against the network the
project's Zenoh app is on.

## 1. Preflight

Before diagnosing:

1. Confirm the binary exists: run `zenmon --help`. If it is missing, tell the
   user this project is debugged with zenmon and how to install it
   (`cargo install --path crates/zenmon-cli` from the zenmon repo, or a brew
   install), then stop.
2. Determine how to connect, in this order — use the first that applies:
   - An endpoint/config already given by the user, or `ZENMON_CONTRACT` /
     `-c <config>` / `-e <endpoint>` set in the environment.
   - A `*.zenmon.yaml` contract file in the repo → pass it with `--contract`.
   - An endpoint or namespace mentioned in the project's CLAUDE.md/README →
     pass with `-e` / `-n`.
   - Otherwise assume the default `tcp/localhost:7447` and **tell the user you
     assumed it.**

Add `--json` to any command when you need to parse the output structurally.

## 2. Symptom → command

Pick the row matching the symptom and run the sequence, stopping when the
problem is explained:

| Symptom | Sequence |
|---|---|
| Can't connect / router unreachable | `zenmon doctor` → if it fails, `zenmon scout` (finds nodes with no router) |
| Who is on the network? | `zenmon nodes`, then `zenmon liveliness` for liveliness tokens |
| What topics are flowing? | `zenmon discover`, then `zenmon sub "<key-expr>"` on keys of interest |
| Messages aren't arriving | `zenmon discover` (is anyone publishing?) → `zenmon sub "<ke>"` (do you actually receive?) → `zenmon keyexpr "<pub-ke>" "<sub-ke>"` (do the two key expressions intersect?) |
| Why did it reach this state? (cause & effect) | `zenmon scenario ...` to collect an episode JSON, then reason over that JSON |

## 3. Long-running commands

`sub`, `scenario`, and `queryable` run until stopped. NEVER attach and forget:

- Bound them with a timeout (e.g. `--timeout`/`--connect-timeout` where the
  subcommand supports it), or run in the background and stop after a fixed
  window.
- `tui` is interactive — you cannot drive it. Do NOT launch `tui`. If a
  dashboard would help, recommend the human run `zenmon tui` themselves.

## 4. Gotchas

- Trust zenmon's own rendering of payloads (it decodes strings, JSON, and
  conservatively MessagePack). Do not re-parse raw bytes yourself.
- Topics are discovered from **received messages**, not the admin space — a
  topic that is currently silent only appears once you `sub` to it.
- **Read-only boundary:** `pub`, `replay`, and `queryable` inject data into a
  live network. You may *propose* them, but run them only after the user
  explicitly confirms.
