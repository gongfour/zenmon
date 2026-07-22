use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;

/// Reply consolidation strategy for GET queries. Zenoh's default (auto) keeps
/// only one reply per key — when multiple queryables share a key expression,
/// only the fastest reply is observable. `none` delivers every reply.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsolidationArg {
    Auto,
    None,
    Monotonic,
    Latest,
}

impl From<ConsolidationArg> for zenoh::query::ConsolidationMode {
    fn from(arg: ConsolidationArg) -> Self {
        match arg {
            ConsolidationArg::Auto => Self::Auto,
            ConsolidationArg::None => Self::None,
            ConsolidationArg::Monotonic => Self::Monotonic,
            ConsolidationArg::Latest => Self::Latest,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "zenmon", about = "Zenoh network monitor and debugger")]
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

    /// Path to a zenmon contract file (YAML). Enables contract-aware enrichment
    /// of `sub`/`discover`. Falls back to the ZENMON_CONTRACT env var.
    #[arg(long, value_name = "PATH")]
    pub contract: Option<PathBuf>,

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

        /// Reply consolidation. Default `auto` keeps one reply per key, so
        /// when several queryables serve the same key only the fastest reply
        /// is visible; use `none` to receive every reply.
        #[arg(long, value_enum, default_value_t = ConsolidationArg::Auto)]
        consolidation: ConsolidationArg,
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

        /// JSON attachment metadata (e.g. '{"request_id":"001","client_id":"zenmon"}')
        #[arg(long)]
        att: Option<String>,

        /// Republish the same value at a fixed rate (e.g. 10Hz). Requires
        /// --count or --duration to bound the loop.
        #[arg(long, requires = "rate_bound", value_parser = crate::duration::parse_rate_hz_arg)]
        rate: Option<f64>,

        /// With --rate, stop after N published messages
        #[arg(long, group = "rate_bound", requires = "rate", value_parser = crate::duration::parse_count_arg)]
        count: Option<u64>,

        /// With --rate, stop after this much time (e.g. 5s)
        #[arg(long, group = "rate_bound", requires = "rate", value_parser = crate::duration::parse_duration_arg)]
        duration: Option<Duration>,
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

    /// Inspect a zenmon contract file (offline; no network).
    Contract {
        #[command(subcommand)]
        command: ContractCommand,
    },

    /// Record a correlated multi-topic diagnostic session and emit one episode
    /// JSON an AI can read to reason about cause & effect. Optionally triggers a
    /// `--pub` actuation or a `--task` request first, then observes a bounded
    /// window. Data-only: it correlates, it does not diagnose.
    #[command(group(clap::ArgGroup::new("topics").required(true).multiple(true).args(["observe", "preset"])))]
    Scenario {
        /// Topic to record (repeatable). At least one of --observe/--preset.
        #[arg(long)]
        observe: Vec<String>,

        /// Expand a built-in diagnosis topic set (currently: stall)
        #[arg(long)]
        preset: Option<String>,

        /// Prefix applied to --preset expansion (default: "**", prefix-agnostic)
        #[arg(long, default_value = "**")]
        prefix: String,

        /// One-shot actuation trigger: publish VALUE to KEY once, after observing
        /// starts. Mutually exclusive with --task.
        #[arg(long = "pub", value_names = ["KEY", "VALUE"], num_args = 2, conflicts_with = "task")]
        pub_: Option<Vec<String>>,

        /// Task trigger: publish REQUEST_JSON to <PREFIX>/request and also
        /// observe <PREFIX>/feedback and <PREFIX>/response. Mutually exclusive
        /// with --pub.
        #[arg(long, value_names = ["PREFIX", "REQUEST_JSON"], num_args = 2, conflicts_with = "pub_")]
        task: Option<Vec<String>>,

        /// Sustain the --pub actuation: republish at this rate (Hz) instead of
        /// once. Requires --pub and one of --pub-for/--pub-count.
        #[arg(long = "pub-rate", requires = "pub_", requires = "pub_bound", value_parser = crate::duration::parse_rate_hz_arg)]
        pub_rate: Option<f64>,

        /// With --pub-rate, stop republishing after this long (e.g. 10s)
        #[arg(long = "pub-for", group = "pub_bound", requires = "pub_rate", value_parser = crate::duration::parse_duration_arg)]
        pub_for: Option<Duration>,

        /// With --pub-rate, stop republishing after N messages
        #[arg(long = "pub-count", group = "pub_bound", requires = "pub_rate", value_parser = crate::duration::parse_count_arg)]
        pub_count: Option<u64>,

        /// Capture window (e.g. 8s). Required.
        #[arg(long = "for", value_parser = crate::duration::parse_duration_arg)]
        for_: Duration,

        /// Extra observe time after the trigger/window ends (e.g. 2s)
        #[arg(long, value_parser = crate::duration::parse_duration_arg)]
        settle: Option<Duration>,

        /// Extract a payload field over time into the episode's `tracks` section
        /// (repeatable). Format `KEY:FIELD`, FIELD a dot-path — e.g.
        /// `myfleet/topic/safety/safety_state:kind`.
        #[arg(long)]
        track: Vec<String>,

        /// Omit the per-event `timeline` from the episode (keep meta/topics/
        /// correlations/tracks). Much smaller output for long/high-rate sessions.
        #[arg(long)]
        no_timeline: bool,

        /// Dry run: print the resolved observe set, trigger, tracks, and window,
        /// then exit without opening a session or triggering anything.
        #[arg(long)]
        explain: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ContractCommand {
    /// Parse the contract and report counts plus structural warnings.
    Lint {
        /// Contract path (defaults to --contract / ZENMON_CONTRACT)
        path: Option<PathBuf>,
    },

    /// List every declared topic: key, pattern, encoding.
    List {
        /// Contract path (defaults to --contract / ZENMON_CONTRACT)
        path: Option<PathBuf>,
    },

    /// Show the full contract entry for a topic key (with $ref expanded).
    Show {
        /// Observed or declared key expression to look up
        key: String,

        /// Contract path (defaults to --contract / ZENMON_CONTRACT)
        path: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum QueryableCommand {
    /// Serve a fixed reply to incoming GET queries (for testing responder paths).
    Serve {
        /// Key expression to declare the queryable on
        key_expr: String,

        /// Fixed reply payload: a literal string, `@<path>` to read it from a
        /// file, or `-` to read it from stdin
        #[arg(long)]
        reply: String,

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
        let cli = Cli::try_parse_from(["zenmon", "config", "validate"]).unwrap();

        assert!(cli.endpoint.is_none());
        assert!(cli.mode.is_none());
    }

    #[test]
    fn effective_flag_is_required_for_config_show() {
        assert!(Cli::try_parse_from(["zenmon", "config", "show"]).is_err());
        assert!(Cli::try_parse_from(["zenmon", "config", "show", "--effective"]).is_ok());
    }

    #[test]
    fn queryable_serve_requires_reply() {
        assert!(Cli::try_parse_from(["zenmon", "queryable", "serve", "k/e"]).is_err());
        assert!(
            Cli::try_parse_from(["zenmon", "queryable", "serve", "k/e", "--reply", "ok"]).is_ok()
        );
    }

    #[test]
    fn queryable_serve_reply_file_flag_is_gone() {
        // Replaced by the unified `--reply @<path>` / `-` payload syntax.
        assert!(Cli::try_parse_from([
            "zenmon",
            "queryable",
            "serve",
            "k/e",
            "--reply-file",
            "x.json"
        ])
        .is_err());
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
        assert!(Cli::try_parse_from(["zenmon", "nodes", "--count", "1"]).is_err());
        assert!(Cli::try_parse_from(["zenmon", "nodes", "--duration", "5s"]).is_err());
    }

    #[test]
    fn nodes_bounds_allowed_with_watch() {
        assert!(Cli::try_parse_from(["zenmon", "nodes", "--watch", "--count", "1"]).is_ok());
        assert!(Cli::try_parse_from(["zenmon", "nodes", "--watch", "--duration", "5s"]).is_ok());
    }

    #[test]
    fn liveliness_bounds_require_watch() {
        assert!(Cli::try_parse_from(["zenmon", "liveliness", "--count", "1"]).is_err());
        assert!(Cli::try_parse_from(["zenmon", "liveliness", "--duration", "5s"]).is_err());
    }

    #[test]
    fn liveliness_bounds_allowed_with_watch() {
        assert!(Cli::try_parse_from(["zenmon", "liveliness", "--watch", "--count", "1"]).is_ok());
        assert!(
            Cli::try_parse_from(["zenmon", "liveliness", "--watch", "--duration", "5s"]).is_ok()
        );
    }

    /// Plain (non-watch) invocations remain valid.
    #[test]
    fn plain_nodes_and_liveliness_parse() {
        assert!(Cli::try_parse_from(["zenmon", "nodes"]).is_ok());
        assert!(Cli::try_parse_from(["zenmon", "liveliness"]).is_ok());
        assert!(Cli::try_parse_from(["zenmon", "nodes", "--watch"]).is_ok());
    }

    /// A single `pub` (no --rate) publishes once and needs no stop condition.
    #[test]
    fn plain_pub_parses() {
        assert!(Cli::try_parse_from(["zenmon", "pub", "test/k", "{}"]).is_ok());
        assert!(
            Cli::try_parse_from(["zenmon", "pub", "test/k", "{}", "--att", "{\"s\":1}"]).is_ok()
        );
    }

    /// `--rate` republishes forever without a stop condition; requiring
    /// `--count`/`--duration` mirrors the `nodes --watch` gating so an agent
    /// can't accidentally launch an unbounded publisher.
    #[test]
    fn pub_rate_requires_count_or_duration() {
        assert!(Cli::try_parse_from(["zenmon", "pub", "test/k", "{}", "--rate", "10"]).is_err());
    }

    #[test]
    fn pub_rate_allowed_with_count_or_duration() {
        assert!(
            Cli::try_parse_from(["zenmon", "pub", "test/k", "{}", "--rate", "10", "--count", "5"])
                .is_ok()
        );
        assert!(Cli::try_parse_from([
            "zenmon", "pub", "test/k", "{}", "--rate", "10Hz", "--duration", "5s"
        ])
        .is_ok());
    }

    /// `--count`/`--duration` bound only the `--rate` loop; without `--rate` a
    /// single publish ignores them, so accepting them would be a silent no-op.
    #[test]
    fn pub_bounds_require_rate() {
        assert!(Cli::try_parse_from(["zenmon", "pub", "test/k", "{}", "--count", "5"]).is_err());
        assert!(
            Cli::try_parse_from(["zenmon", "pub", "test/k", "{}", "--duration", "5s"]).is_err()
        );
    }

    /// Multiple queryables on a shared key (e.g. dotori `call/*` services) are
    /// invisible under default consolidation — only the fastest reply survives.
    /// `--consolidation none` must parse so every reply can be observed.
    #[test]
    fn query_consolidation_values_parse() {
        for (arg, expected) in [
            ("auto", ConsolidationArg::Auto),
            ("none", ConsolidationArg::None),
            ("monotonic", ConsolidationArg::Monotonic),
            ("latest", ConsolidationArg::Latest),
        ] {
            let cli =
                Cli::try_parse_from(["zenmon", "query", "k/e", "--consolidation", arg]).unwrap();
            let Command::Query { consolidation, .. } = cli.command else {
                panic!("expected query command");
            };
            assert_eq!(consolidation, expected, "--consolidation {arg}");
        }
    }

    /// Without the flag, behavior must stay what it was before the flag
    /// existed: Zenoh's default (auto).
    #[test]
    fn query_consolidation_defaults_to_auto() {
        let cli = Cli::try_parse_from(["zenmon", "query", "k/e"]).unwrap();
        let Command::Query { consolidation, .. } = cli.command else {
            panic!("expected query command");
        };
        assert_eq!(consolidation, ConsolidationArg::Auto);
    }

    #[test]
    fn query_consolidation_rejects_unknown_value() {
        assert!(Cli::try_parse_from(["zenmon", "query", "k/e", "--consolidation", "all"]).is_err());
    }

    /// `--for` is required: scenario has no default window, so an unbounded
    /// invocation would never terminate (unsafe for an agent).
    #[test]
    fn scenario_requires_for() {
        assert!(
            Cli::try_parse_from(["zenmon", "scenario", "--observe", "a/**"]).is_err(),
            "missing --for must be rejected"
        );
        assert!(
            Cli::try_parse_from(["zenmon", "scenario", "--observe", "a/**", "--for", "8s"]).is_ok()
        );
    }

    /// At least one of --observe/--preset must be given, else there is nothing
    /// to record.
    #[test]
    fn scenario_requires_observe_or_preset() {
        assert!(
            Cli::try_parse_from(["zenmon", "scenario", "--for", "8s"]).is_err(),
            "no topic source must be rejected"
        );
        assert!(
            Cli::try_parse_from(["zenmon", "scenario", "--preset", "stall", "--for", "8s"]).is_ok()
        );
    }

    /// --pub and --task are mutually exclusive triggers.
    #[test]
    fn scenario_pub_and_task_conflict() {
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "8s", "--pub", "k", "v", "--task",
            "p", "{}",
        ])
        .is_err());
    }

    /// Each trigger accepts exactly its two positional values.
    #[test]
    fn scenario_pub_and_task_take_two_values() {
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "8s", "--pub", "cmd/go",
            "{\"go\":true}",
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "zenmon",
            "scenario",
            "--preset",
            "stall",
            "--prefix",
            "myfleet",
            "--for",
            "8s",
            "--settle",
            "2s",
            "--task",
            "myfleet/task/mission/mission",
            "{\"mission_id\":\"m1\"}",
        ])
        .is_ok());
        // --pub with only one value is rejected (num_args = 2).
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "8s", "--pub", "onlykey",
        ])
        .is_err());
    }

    /// `--pub-rate` sustains the `--pub` actuation; it is meaningless without a
    /// `--pub` trigger and must be rejected then.
    #[test]
    fn scenario_no_timeline_flag_parses() {
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "8s", "--no-timeline",
        ])
        .is_ok());
    }

    #[test]
    fn scenario_explain_flag_parses() {
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--preset", "stall", "--for", "8s", "--explain",
        ])
        .is_ok());
    }

    #[test]
    fn scenario_pub_rate_requires_pub() {
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "8s", "--pub-rate", "10",
            "--pub-for", "5s",
        ])
        .is_err());
    }

    /// `--pub-rate` republishes forever without a bound; require --pub-for or
    /// --pub-count so an agent can't launch an unbounded publisher.
    #[test]
    fn scenario_pub_rate_requires_bound() {
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "8s", "--pub", "k", "v",
            "--pub-rate", "10",
        ])
        .is_err());
    }

    #[test]
    fn scenario_pub_rate_allowed_with_pub_and_bound() {
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "11s", "--pub", "cmd/go",
            "{\"go\":true}", "--pub-rate", "10", "--pub-for", "10s",
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "8s", "--pub", "k", "v",
            "--pub-rate", "10", "--pub-count", "50",
        ])
        .is_ok());
    }

    /// A plain `--pub` (no --pub-rate) still publishes once; the bounds are
    /// meaningless alone and must be rejected.
    #[test]
    fn scenario_pub_one_shot_still_ok_and_bounds_require_rate() {
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "8s", "--pub", "k", "v",
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "zenmon", "scenario", "--observe", "a/**", "--for", "8s", "--pub", "k", "v",
            "--pub-for", "5s",
        ])
        .is_err());
    }
}
