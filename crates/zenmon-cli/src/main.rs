mod cli;
mod cmd;
mod common;
mod duration;
mod remote;
mod update;
mod watch;

use clap::Parser;
use cli::{Cli, Command};
use cmd::Ctx;
use common::{internal_err, load_contract_opt};
use zenmon_core::config::{resolve_config, ConfigOverrides, ResolvedConfig};
use zenmon_core::error::ZenmonError;

fn build_config(cli: &Cli) -> Result<ResolvedConfig, ZenmonError> {
    // Resolution failures (bad mode, endpoint, scout port, connect timeout, or
    // config file) are all user-input errors, so collapse them to invalid_input
    // (exit 2) to keep the CLI's structured error / exit-code contract.
    resolve_config(ConfigOverrides {
        endpoint: cli.endpoint.clone(),
        mode: cli.mode.clone(),
        namespace: cli.namespace.clone(),
        config_file: cli.config.clone(),
        scout_port: cli.scout_port,
        connect_timeout: cli.connect_timeout,
    })
    .map_err(|e| ZenmonError::invalid_input(e.to_string()))
}

/// Default tracing filter for plain CLI mode.
const DEFAULT_LOG_FILTER: &str = "zenmon=info,zenoh=warn";

/// Resolve the tracing filter for the process.
///
/// In JSON or TUI mode logs are forced fully OFF regardless of `RUST_LOG`:
/// - JSON mode: stdout must carry only the structured JSON result and stderr
///   only the single structured error, so no log line may reach either stream.
///   Honoring `RUST_LOG` here leaks Zenoh's full `Config` debug output (which
///   includes authentication fields) and breaks the machine-readable contract.
/// - TUI mode: stray log output corrupts the ratatui display.
///
/// Only plain CLI mode consults `RUST_LOG`, falling back to a sensible default.
fn resolve_log_filter(
    is_tui: bool,
    is_json: bool,
    rust_log: Option<&str>,
) -> tracing_subscriber::EnvFilter {
    if is_tui || is_json {
        return tracing_subscriber::EnvFilter::new("off");
    }
    match rust_log {
        Some(spec) => tracing_subscriber::EnvFilter::try_new(spec)
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
        None => tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER),
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    #[cfg(feature = "tui")]
    let is_tui = matches!(cli.command, Command::Tui(_));
    #[cfg(not(feature = "tui"))]
    let is_tui = false;
    let is_json = cli.json;

    // In JSON mode the only permitted stderr output is the single structured
    // error object, so we must not install color_eyre's colored/backtrace hook.
    if !is_json {
        let _ = color_eyre::install();
    }

    // stderr: stdout carries results, and a log line there is a fake record
    // to whatever reads it (`sub | head` used to get two INFO lines).
    tracing_subscriber::fmt()
        .with_env_filter(resolve_log_filter(
            is_tui,
            is_json,
            std::env::var("RUST_LOG").ok().as_deref(),
        ))
        .with_writer(std::io::stderr)
        .init();

    let emit_error = |e: ZenmonError| -> ! {
        if is_json {
            eprintln!("{}", e.to_json());
        } else {
            eprintln!("Error: {}", e);
        }
        std::process::exit(e.exit_code());
    };

    let resolved = match build_config(&cli) {
        Ok(resolved) => resolved,
        // A `config` command in JSON mode reports a resolution/validation
        // failure as a structured {"valid": false, ...} document (exit 2)
        // rather than the generic error envelope.
        Err(e) if is_json && matches!(&cli.command, Command::Config { .. }) => {
            let output = serde_json::json!({
                "valid": false,
                "error": { "code": "invalid_config", "message": e.to_string() },
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).unwrap_or_default()
            );
            std::process::exit(2);
        }
        Err(e) => emit_error(e),
    };

    if let Err(e) = run(cli, resolved).await {
        emit_error(e);
    }
}

/// Dispatch to the command's module. Everything a command needs beyond its
/// own arguments is the resolved Zenoh config and the global flags in [`Ctx`].
async fn run(cli: Cli, resolved: ResolvedConfig) -> Result<(), ZenmonError> {
    let config = resolved.config;
    let ctx = Ctx {
        json: cli.json,
        contract: cli.contract,
    };
    match cli.command {
        Command::Config { command } => cmd::config::run(&ctx, &resolved.effective, command)?,

        // Purely local: edits a per-user file and never opens a session. The
        // Zenoh config resolved above is irrelevant here.
        Command::Remote { command } => remote::run(command, ctx.json)?,

        // Talks to a release remote, never to a Zenoh network.
        Command::Update { command } => update::run(command, ctx.json).await?,

        Command::Discover(args) => cmd::network::discover(&ctx, config, args).await?,

        Command::Sub(args) => cmd::sub::run(&ctx, config, args).await?,

        Command::Query(args) => cmd::query::run(&ctx, config, args).await?,

        Command::Nodes(args) => cmd::network::nodes(&ctx, config, args).await?,

        Command::Pub(args) => cmd::publish::publish(&ctx, config, args).await?,

        Command::Liveliness(args) => cmd::network::liveliness(&ctx, config, args).await?,

        Command::Scout(args) => cmd::network::scout(&ctx, config, args).await?,

        Command::Info => cmd::network::info(&ctx, config).await?,

        Command::Doctor(args) => cmd::network::doctor(&ctx, config, args).await?,

        Command::Keyexpr(cli::KeyexprArgs { a, b }) => {
            // Pure, offline: no session is opened.
            let rel = zenmon_core::keyexpr::compare(&a, &b)?;
            if ctx.json {
                println!("{}", serde_json::to_string(&rel)?);
            } else {
                println!("A:             {}", rel.a);
                println!("B:             {}", rel.b);
                println!("intersects:    {}", rel.intersects);
                println!("A includes B:  {}", rel.a_includes_b);
                println!("B includes A:  {}", rel.b_includes_a);
                println!("equal:         {}", rel.equal);
                println!("relation:      {:?}", rel.relation);
            }
        }

        Command::Trace { command } => cmd::trace::run(&ctx, command)?,

        Command::Capture(args) => cmd::capture::capture(&ctx, config, args).await?,

        Command::Replay(args) => cmd::capture::replay(&ctx, config, args).await?,

        Command::Queryable { command } => cmd::publish::queryable(&ctx, config, command).await?,

        #[cfg(feature = "tui")]
        Command::Tui(cli::TuiArgs {
            refresh,
            allow_publish,
        }) => {
            // `--contract` / ZENMON_CONTRACT is global, so the TUI picks it up
            // the same way `sub` and `discover` do; without one the detail pane
            // simply shows no contract badge.
            let contract = load_contract_opt(&ctx.contract)?;
            zenmon_tui::run(config, refresh, contract, allow_publish)
                .await
                .map_err(internal_err)?;
        }

        Command::Contract { command } => cmd::contract::run(&ctx, command)?,

        Command::Scenario(args) => cmd::scenario::run(&ctx, config, args).await?,
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::level_filters::LevelFilter;

    /// JSON mode must force logs off even when the user exports `RUST_LOG=trace`,
    /// otherwise Zenoh's `Config` debug (auth fields included) leaks and breaks
    /// the single-JSON-line stderr / clean-JSON stdout contract.
    #[test]
    fn json_mode_forces_off_despite_rust_log() {
        let filter = resolve_log_filter(false, true, Some("trace"));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::OFF));
    }

    /// TUI mode must force logs off even with `RUST_LOG` set, to avoid corrupting
    /// the ratatui display.
    #[test]
    fn tui_mode_forces_off_despite_rust_log() {
        let filter = resolve_log_filter(true, false, Some("trace"));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::OFF));
    }

    /// Plain CLI mode honors `RUST_LOG`.
    #[test]
    fn plain_mode_honors_rust_log() {
        let filter = resolve_log_filter(false, false, Some("trace"));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::TRACE));
    }

    /// Plain CLI mode falls back to the default filter when `RUST_LOG` is unset,
    /// and also when it is malformed.
    #[test]
    fn plain_mode_defaults_without_or_with_invalid_rust_log() {
        assert_eq!(
            resolve_log_filter(false, false, None).max_level_hint(),
            Some(LevelFilter::INFO)
        );
        assert_eq!(
            resolve_log_filter(false, false, Some("=not a valid filter=")).max_level_hint(),
            Some(LevelFilter::INFO)
        );
    }
}
