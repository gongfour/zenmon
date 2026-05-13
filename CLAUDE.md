# CLAUDE.md

## Project Overview

zemon is a Rust CLI + TUI tool for monitoring and debugging Zenoh networks. Single binary `zemon` with headless CLI subcommands and an interactive ratatui TUI dashboard.

## Build & Run

```bash
cargo build --release          # Release binary at ./target/release/zemon
cargo check                    # Quick type check
cargo run -- sub "test/**"     # Run via cargo
```

Requires: Rust 1.75+, zenohd for testing (homebrew: `brew install zenoh`)

## Project Structure

```
crates/
  zemon-core/    # Library: Zenoh session, subscribe, query, registry
  zemon-cli/     # Binary: clap CLI, produces `zemon`
  zemon-tui/     # Library: ratatui views, event loop, app state
```

- `zemon-core` is the shared library — CLI and TUI both depend on it
- `zemon-tui` is a library crate called by `zemon-cli` via `zemon tui` subcommand
- Single binary: `zemon` (defined in zemon-cli/Cargo.toml)

## Key Patterns

- **Zenoh error handling**: Zenoh errors don't implement `Into<color_eyre::Report>`. Use `.map_err(|e| eyre!(e))` pattern.
- **Payload parsing**: Use `MessagePayload::from_zbytes()` which tries `try_to_string()` first, then `from_slice()`. Never use `to_bytes()` + `serde_json::from_slice()` directly — it fails for cross-language string payloads.
- **TUI logs**: TUI mode sets tracing filter to `"off"` to prevent stderr output from corrupting ratatui display.
- **Non-blocking TUI**: Reconnection and queries run in background tokio tasks. Never block the event loop with await on network calls.
- **Topic discovery**: Topics are collected from received messages (not admin space). Admin space doesn't list pub/sub key expressions.

## Testing

No unit tests yet. Manual testing:

```bash
# Terminal 1: Start router
zenohd

# Terminal 2: Start TUI
./target/release/zemon tui

# Terminal 3: Publish test data
./target/release/zemon pub test/hello '{"msg":"world"}' --att '{"source":"debug"}'
```

## Conventions

- Commit messages: `feat(scope):`, `fix(scope):`, `chore:`
- Korean comments are OK in design docs, English in code
- Design spec: `docs/superpowers/specs/`
- Implementation plans: `docs/superpowers/plans/`
