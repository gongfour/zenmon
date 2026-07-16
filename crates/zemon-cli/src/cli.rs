use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "zemon", about = "Zenoh network monitor and debugger")]
pub struct Cli {
    /// Zenoh connection endpoint
    #[arg(short, long, default_value = "tcp/localhost:7447")]
    pub endpoint: String,

    /// Connection mode: peer or client
    #[arg(short, long, default_value = "client")]
    pub mode: String,

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
    #[arg(long, value_parser = crate::duration::parse_duration_arg)]
    pub connect_timeout: Option<Duration>,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
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

        /// With --watch, stop after N snapshots
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        count: Option<u64>,

        /// With --watch, stop after this much time (e.g. 5s)
        #[arg(long, value_parser = crate::duration::parse_duration_arg)]
        duration: Option<Duration>,
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
    /// Scans a multicast port range in parallel; each port maps to
    /// Zenoh domain id = port - 7446.
    Scout {
        /// Multicast port range, START-END inclusive.
        /// Default covers domain ids 0..=100.
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
        #[arg(long, value_parser = crate::duration::parse_count_arg)]
        count: Option<u64>,

        /// With --watch, stop after this much time (e.g. 5s)
        #[arg(long, value_parser = crate::duration::parse_duration_arg)]
        duration: Option<Duration>,
    },

    /// Show current session information
    Info,

    /// Test how two key expressions relate (intersect / include). Pure, no
    /// network. `a_includes_b` means A contains every key of B.
    Keyexpr {
        /// First key expression (A)
        a: String,
        /// Second key expression (B)
        b: String,
    },

    /// Launch interactive TUI dashboard
    Tui {
        /// UI refresh interval (e.g. 100ms, 1s)
        #[arg(long, default_value = "100ms", value_parser = crate::duration::parse_duration_arg)]
        refresh: Duration,
    },
}
