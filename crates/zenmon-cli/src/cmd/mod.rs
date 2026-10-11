//! One module per subcommand (or family of them). `main.rs` only parses,
//! resolves the config, and dispatches here.

pub mod capture;
pub mod config;
pub mod contract;
pub mod network;
pub mod publish;
pub mod query;
pub mod scenario;
pub mod sub;
pub mod trace;

use std::path::PathBuf;

/// Global flags a command may need besides its own arguments.
pub struct Ctx {
    /// `--json`: machine-readable output, logs off.
    pub json: bool,
    /// `--contract`, before the `ZENMON_CONTRACT` fallback is applied.
    pub contract: Option<PathBuf>,
}
