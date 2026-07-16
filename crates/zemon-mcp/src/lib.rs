pub mod error;
pub mod handlers;
pub mod server;
pub mod state;

use anyhow::Result;
use rmcp::{transport::stdio, ServiceExt};
use std::sync::Arc;
use zemon_core::config::{EffectiveConfig, ZemonConfig};

/// Run the MCP server over stdio until the client disconnects.
///
/// `effective` is the allow-listed view of the config the caller resolved at
/// startup (CLI flags + env + config-file); `config_show` serializes it
/// verbatim so it always agrees with the session `config` was actually built
/// from, instead of re-resolving from env/config-file alone.
pub async fn serve_stdio(config: ZemonConfig, effective: EffectiveConfig) -> Result<()> {
    let state = Arc::new(state::ServerState::new(config, effective));
    let server = server::ZemonMcpServer::new(state);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
