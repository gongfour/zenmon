pub mod error;
pub mod handlers;
pub mod server;
pub mod state;

use anyhow::Result;
use rmcp::{transport::stdio, ServiceExt};
use std::sync::Arc;
use zemon_core::config::ZemonConfig;

/// Run the MCP server over stdio until the client disconnects.
pub async fn serve_stdio(config: ZemonConfig) -> Result<()> {
    let state = Arc::new(state::ServerState::new(config));
    let server = server::ZemonMcpServer::new(state);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
