use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "dotori", about = "Zenoh network monitor and debugger")]
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
    },

    /// Send a Zenoh GET query
    Query {
        /// Key expression to query
        key_expr: String,

        /// JSON payload to include in query
        #[arg(long)]
        payload: Option<String>,

        /// Query timeout in milliseconds
        #[arg(long, default_value = "5000")]
        timeout: u64,
    },

    /// List discovered Zenoh nodes
    Nodes {
        /// Watch for changes (live update)
        #[arg(long)]
        watch: bool,
    },

    /// Publish a message to a key expression
    Pub {
        /// Key expression to publish to
        key_expr: String,

        /// JSON payload to publish
        value: String,
    },

    /// Launch interactive TUI dashboard
    Tui {
        /// UI refresh interval in milliseconds
        #[arg(long, default_value = "100")]
        refresh: u64,
    },
}
