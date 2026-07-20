---
name: run-zenmon
description: Build and run the zenmon CLI to inspect, diagnose, capture, and analyze Zenoh networks. Use for node discovery, liveliness checks, bounded topic capture, publish/subscribe/query workflows, and JSON snapshots.
---

# Run zenmon

`zenmon` is a Rust CLI and TUI for monitoring and debugging Zenoh networks.
For automation, prefer bounded CLI commands and the included driver. Paths in
this document are relative to the repository root.

The TUI is interactive and should not be used for headless automation.

## Prerequisites

- Rust 1.75 or newer
- Bash (Git Bash on Windows) for `driver.sh`
- Optional: `zenohd` for the routed smoke-test tier
- Optional: `jq` for snapshots and capture-rate summaries

## Build

```bash
cargo build --release
# Windows: target/release/zenmon.exe
# Unix:    target/release/zenmon
```

## Driver

`.claude/skills/run-zenmon/driver.sh` builds the binary when needed and exposes
three bounded workflows:

```bash
# Isolated self-test; does not use the default 7446/7447 ports
bash .claude/skills/run-zenmon/driver.sh smoke

# One JSON object containing info, nodes, and liveliness responses
bash .claude/skills/run-zenmon/driver.sh snapshot -e tcp/127.0.0.1:7447

# Five-second NDJSON capture plus a per-key rate table
bash .claude/skills/run-zenmon/driver.sh capture 'demo/**' 5s \
  -e tcp/127.0.0.1:7447
```

Arguments following `snapshot`, or following the key and duration for
`capture`, are passed as zenmon global flags. Common flags include
`-e/--endpoint`, `-m/--mode`, `-n/--namespace`, `--scout-port`, and
`-c/--config`.

## Raw CLI

```bash
D=target/release/zenmon.exe  # Unix: target/release/zenmon
```

Finite commands:

```bash
"$D" info
"$D" --json info | jq .
"$D" --json nodes | jq '.items'
"$D" --json liveliness | jq '.items'
"$D" query '@/*/router'
"$D" pub demo/hello '{"msg":"world"}' --att '{"source":"agent"}'
"$D" scout --port-range 7446-7446 --per-port-timeout 2s
"$D" doctor --timeout 5s
```

Bound streaming commands with their native limits:

```bash
"$D" --json sub 'demo/**' --duration 5s
"$D" --json sub 'demo/**' --count 10
"$D" --json nodes --watch --duration 5s --changes-only
"$D" --json liveliness '**' --watch --duration 5s --changes-only
```

Finite collection output uses an envelope shaped like
`{"count":N,"items":[...]}`. Streaming JSON output is NDJSON, one event per
line. JSON mode disables tracing automatically, so it is safe to pipe into
`jq`.

## Capture and replay

Use the native capture command when the trace may be replayed later:

```bash
"$D" capture 'demo/**' --output capture.jsonl --duration 5s
"$D" replay capture.jsonl --dry-run
"$D" replay capture.jsonl --speed 2
"$D" replay capture.jsonl --rate 10
```

Each capture line contains `schema_version`, `key_expr`, base64-encoded payload
and attachment data, encoding, source timestamp, receive offset, and sample
kind. Keep capture files local unless they are deliberately sanitized.

## Network workflow

1. Find active multicast domains:

   ```bash
   "$D" --json scout | jq '.items[] | {port, nodes}'
   ```

2. Connect to a discovered router and inspect the session:

   ```bash
   "$D" -e tcp/127.0.0.1:7447 --json info | jq '.items[0]'
   "$D" -e tcp/127.0.0.1:7447 --json nodes | jq '.items'
   ```

3. Inspect liveliness and traffic without assuming a project-specific key
   layout:

   ```bash
   "$D" -e tcp/127.0.0.1:7447 --json liveliness '**' | jq '.items'
   "$D" -e tcp/127.0.0.1:7447 --json sub '**' --duration 5s \
     | jq -r '.key_expr' | sort | uniq -c | sort -rn
   ```

4. Use peer mode for router-less networks:

   ```bash
   "$D" -m peer --scout-port 7446 --json sub '**' --duration 5s
   ```

## Notes

- `discover` cannot guarantee discovery of publish-only topics. Use a bounded
  `sub '**'` capture to observe actual traffic.
- Client mode needs a reachable router; use `-m peer` for a router-less mesh.
- A source timestamp may be absent in peer-only traffic. Use capture receive
  offsets when replay timing matters.
- The multicast scouting port and router TCP port are independent settings.
- `-n <namespace>` prefixes keys at the Zenoh session boundary. Avoid passing a
  key that already contains the same prefix.
- Use loopback listeners and non-default multicast ports for tests so they do
  not join an operational network. The driver smoke test does this.

## Troubleshooting

- Connection failure in client mode: verify `-e`, run `doctor`, or use peer
  mode if the network has no router.
- Empty capture: verify the endpoint/domain with `scout`, then subscribe to a
  broader key expression.
- `jq` parse errors: confirm the command used `--json` and did not combine
  human-readable output with the JSON stream.
- On Windows, run the driver through Git Bash.
