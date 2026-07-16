use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "zemon", about = "Zenoh network monitor and debugger")]
pub struct Cli {
    /// Zenoh connection endpoint (default: tcp/localhost:7447, resolved via config)
    #[arg(short, long)]
    pub endpoint: Option<String>,

    /// Connection mode: peer or client (default: client, resolved via config)
    #[arg(short, long)]
    pub mode: Option<String>,

    /// Zenoh namespace for key expression isolation
    #[arg(short, long)]
    pub namespace: Option<String>,

    /// Path to Zenoh JSON5 config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Override Zenoh multicast scouting port (default 7446).
    /// Sets scouting/multicast/address to 224.0.0.224:<PORT>.
    #[arg(long, value_name = "PORT")]
    pub scout_port: Option<u16>,

    /// Connect deadline (e.g. 5s). In client mode, fail if the router isn't
    /// reachable within this window. Ignored for peer mode.
    #[arg(long, value_parser = crate::duration::parse_connect_timeout_arg)]
    pub connect_timeout: Option<Duration>,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Validate or inspect the effective configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },

    /// Discover active keys/topics
    Discover {
        /// Key expression to filter (default: "**")
        #[arg(default_value = "**")]
        key_expr: String,
    },

    /// Subscribe to a topic and stream messages
    Sub {
        /// Key expression to subscribe
        key_expr: String,

        /// Pretty-print JSON output
        #[arg(long)]
        pretty: bool,

        /// Show timestamps
        #[arg(long)]
        timestamp: bool,

        /// Stop after N received messages
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        count: Option<u64>,

        /// Stop after this much time (e.g. 5s, 500ms)
        #[arg(long, value_parser = crate::duration::parse_duration_arg)]
        duration: Option<Duration>,

        /// Cap payload/attachment preview to N bytes in --json output
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        max_payload_bytes: Option<u64>,
    },

    /// Send a Zenoh GET query
    Query {
        /// Key expression to query
        key_expr: String,

        /// JSON payload to include in query
        #[arg(long)]
        payload: Option<String>,

        /// Query timeout (e.g. 5s, 500ms)
        #[arg(long, default_value = "5s", value_parser = crate::duration::parse_duration_arg)]
        timeout: Duration,

        /// Return at most N replies (output budget; more may exist)
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        limit: Option<u64>,
    },

    /// List discovered Zenoh nodes
    Nodes {
        /// Watch for changes (live update)
        #[arg(long)]
        watch: bool,

        /// With --watch, stop after N snapshots (or change events with --changes-only)
        #[arg(long, requires = "watch", value_parser = crate::duration::parse_count_arg)]
        count: Option<u64>,

        /// With --watch, stop after this much time (e.g. 5s)
        #[arg(long, requires = "watch", value_parser = crate::duration::parse_duration_arg)]
        duration: Option<Duration>,

        /// With --watch, emit only added/changed/removed diffs (pair with --duration)
        #[arg(long)]
        changes_only: bool,
    },

    /// Publish a message to a key expression
    Pub {
        /// Key expression to publish to
        key_expr: String,

        /// JSON payload to publish
        value: String,

        /// JSON attachment metadata (e.g. '{"request_id":"001","client_id":"zemon"}')
        #[arg(long)]
        att: Option<String>,
    },

    /// Scout the network for Zenoh nodes (no router needed).
    /// Scans multicast scouting ports in parallel to discover nodes on
    /// separately configured discovery networks.
    Scout {
        /// Multicast port range, START-END inclusive.
        /// Default starts at Zenoh's default scouting port and scans 101 ports.
        #[arg(long, value_name = "START-END", default_value = "7446-7546")]
        port_range: String,

        /// Per-port scouting timeout (e.g. 1s, 500ms)
        #[arg(long, default_value = "1s", value_parser = crate::duration::parse_duration_arg)]
        per_port_timeout: Duration,
    },

    /// Query liveliness tokens on the network
    Liveliness {
        /// Key expression to filter (default: "**")
        #[arg(default_value = "**")]
        key_expr: String,

        /// Watch for changes (live subscribe)
        #[arg(long)]
        watch: bool,

        /// With --watch, stop after N change events
        #[arg(long, requires = "watch", value_parser = crate::duration::parse_count_arg)]
        count: Option<u64>,

        /// With --watch, stop after this much time (e.g. 5s)
        #[arg(long, requires = "watch", value_parser = crate::duration::parse_duration_arg)]
        duration: Option<Duration>,

        /// With --watch, suppress the initial snapshot and stream only join/leave events
        #[arg(long)]
        changes_only: bool,
    },

    /// Show current session information
    Info,

    /// Diagnose the connection: config, session, connection and admin checks,
    /// each reported pass/warn/fail, bounded by --timeout.
    Doctor {
        /// Overall diagnostic deadline (e.g. 5s)
        #[arg(long, default_value = "5s", value_parser = crate::duration::parse_duration_arg)]
        timeout: Duration,
    },

    /// Test how two key expressions relate (intersect / include). Pure, no
    /// network. `a_includes_b` means A contains every key of B.
    Keyexpr {
        /// First key expression (A)
        a: String,
        /// Second key expression (B)
        b: String,
    },

    /// Record received messages to a versioned NDJSON trace file
    Capture {
        /// Key expression to subscribe and record
        key_expr: String,

        /// Output NDJSON file
        #[arg(long, short)]
        output: PathBuf,

        /// Stop after N recorded messages
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        count: Option<u64>,

        /// Stop after this much time (e.g. 30s)
        #[arg(long, value_parser = crate::duration::parse_duration_arg)]
        duration: Option<Duration>,
    },

    /// Replay a captured NDJSON trace by re-publishing its messages
    Replay {
        /// Input NDJSON trace file
        input: PathBuf,

        /// Replay original intervals at this speed multiplier (e.g. 2.0)
        #[arg(long, default_value = "1.0", conflicts_with = "rate", value_parser = crate::duration::parse_speed_arg)]
        speed: f64,

        /// Replay at a fixed rate instead of original timing (e.g. 10Hz)
        #[arg(long, conflicts_with = "speed", value_parser = crate::duration::parse_rate_hz_arg)]
        rate: Option<f64>,

        /// Prepend this prefix to every replayed key expression
        #[arg(long)]
        key_prefix: Option<String>,

        /// Print what would be published without actually publishing
        #[arg(long)]
        dry_run: bool,
    },

    /// Test queryable: serve a fixed reply to incoming GET queries
    Queryable {
        #[command(subcommand)]
        command: QueryableCommand,
    },

    /// Launch interactive TUI dashboard
    Tui {
        /// UI refresh interval (e.g. 100ms, 1s)
        #[arg(long, default_value = "100ms", value_parser = crate::duration::parse_duration_arg)]
        refresh: Duration,
    },
}

#[derive(Subcommand, Debug)]
pub enum QueryableCommand {
    /// Serve a fixed reply to incoming GET queries (for testing responder paths).
    Serve {
        /// Key expression to declare the queryable on
        key_expr: String,

        /// Fixed reply payload (string)
        #[arg(long, conflicts_with = "reply_file")]
        reply: Option<String>,

        /// Fixed reply payload read from a file
        #[arg(long, conflicts_with = "reply")]
        reply_file: Option<PathBuf>,

        /// Concrete reply key (required when key_expr is a wildcard)
        #[arg(long)]
        reply_key: Option<String>,

        /// Encoding for the reply (e.g. application/json)
        #[arg(long)]
        encoding: Option<String>,

        /// Stop after N successful replies
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        count: Option<u64>,

        /// Stop after this much time (e.g. 30s)
        #[arg(long, value_parser = crate::duration::parse_duration_arg)]
        duration: Option<Duration>,

        /// Include (capped) request payload/key in JSON events
        #[arg(long)]
        include_request: bool,

        /// Cap included request preview to N bytes (default 1024)
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        max_request_bytes: Option<u64>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Validate the merged configuration without opening a network session
    Validate,

    /// Show configuration after file, environment, and CLI overrides
    Show {
        /// Show the fully resolved effective configuration
        #[arg(long, required = true)]
        effective: bool,
    },
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn global_defaults_remain_unset_until_resolution() {
        let cli = Cli::try_parse_from(["zemon", "config", "validate"]).unwrap();

        assert!(cli.endpoint.is_none());
        assert!(cli.mode.is_none());
    }

    #[test]
    fn effective_flag_is_required_for_config_show() {
        assert!(Cli::try_parse_from(["zemon", "config", "show"]).is_err());
        assert!(Cli::try_parse_from(["zemon", "config", "show", "--effective"]).is_ok());
    }

    #[test]
    fn scout_help_uses_scouting_port_terminology() {
        let mut command = Cli::command();
        let scout = command.find_subcommand_mut("scout").unwrap();
        let help = scout.render_long_help().to_string().to_lowercase();

        assert!(help.contains("scouting port"));
        assert!(!help.contains("domain"));
    }

    /// `--count`/`--duration` bound only the `--watch` loop; without `--watch`
    /// the non-watch branch ignores them, so accepting them would be a silent
    /// no-op that misleads agents. They must be rejected at parse time.
    #[test]
    fn nodes_bounds_require_watch() {
        assert!(Cli::try_parse_from(["zemon", "nodes", "--count", "1"]).is_err());
        assert!(Cli::try_parse_from(["zemon", "nodes", "--duration", "5s"]).is_err());
    }

    #[test]
    fn nodes_bounds_allowed_with_watch() {
        assert!(Cli::try_parse_from(["zemon", "nodes", "--watch", "--count", "1"]).is_ok());
        assert!(Cli::try_parse_from(["zemon", "nodes", "--watch", "--duration", "5s"]).is_ok());
    }

    #[test]
    fn liveliness_bounds_require_watch() {
        assert!(Cli::try_parse_from(["zemon", "liveliness", "--count", "1"]).is_err());
        assert!(Cli::try_parse_from(["zemon", "liveliness", "--duration", "5s"]).is_err());
    }

    #[test]
    fn liveliness_bounds_allowed_with_watch() {
        assert!(Cli::try_parse_from(["zemon", "liveliness", "--watch", "--count", "1"]).is_ok());
        assert!(
            Cli::try_parse_from(["zemon", "liveliness", "--watch", "--duration", "5s"]).is_ok()
        );
    }

    /// Plain (non-watch) invocations remain valid.
    #[test]
    fn plain_nodes_and_liveliness_parse() {
        assert!(Cli::try_parse_from(["zemon", "nodes"]).is_ok());
        assert!(Cli::try_parse_from(["zemon", "liveliness"]).is_ok());
        assert!(Cli::try_parse_from(["zemon", "nodes", "--watch"]).is_ok());
    }
}
