//! The MCP server: a `#[tool_router]`-decorated struct exposing one
//! `#[tool]`-decorated method per Zenoh operation, dispatched by rmcp's
//! generated `ServerHandler` (`#[tool_handler]`).
//!
//! Note: rmcp 2.2.0 re-exports its own `schemars` (currently 1.2.1) via
//! `rmcp::schemars`. Tool parameter structs MUST derive `JsonSchema` from
//! that re-export (not a separately added `schemars` crate dependency) —
//! otherwise the derived `JsonSchema` impl is a different trait than the one
//! `Parameters<P: JsonSchema>` expects and the crate fails to compile.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};

use crate::error::to_mcp_error;
use crate::handlers;
use crate::state::ServerState;
use std::sync::Arc;

#[derive(Clone)]
pub struct ZemonMcpServer {
    pub(crate) state: Arc<ServerState>,
    tool_router: ToolRouter<ZemonMcpServer>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct KeyexprParams {
    /// First key expression (A).
    pub a: String,
    /// Second key expression (B).
    pub b: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DiscoverParams {
    /// Key expression to filter (default "**").
    #[serde(default = "default_key_expr")]
    pub key_expr: String,
}

fn default_key_expr() -> String {
    "**".to_string()
}

#[tool_router]
impl ZemonMcpServer {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Compare two key expressions (intersect/include). Pure, no network.")]
    fn keyexpr(
        &self,
        Parameters(p): Parameters<KeyexprParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let json = handlers::keyexpr_json(&p.a, &p.b).map_err(to_mcp_error)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(description = "List active keys/topics matching a key expression.")]
    async fn discover(
        &self,
        Parameters(p): Parameters<DiscoverParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let session = self.state.session().await.map_err(to_mcp_error)?;
        match handlers::discover_json(&session, &p.key_expr).await {
            Ok(json) => Ok(CallToolResult::success(vec![ContentBlock::text(json)])),
            Err(e) => {
                if e.is_connection() {
                    self.state.invalidate_session().await;
                }
                Err(to_mcp_error(e))
            }
        }
    }

    #[tool(description = "Show the effective, allow-listed configuration. No network.")]
    fn config_show(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let json = handlers::config_show_json().map_err(to_mcp_error)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(description = "Show current Zenoh session information.")]
    async fn info(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let session = self.state.session().await.map_err(to_mcp_error)?;
        match handlers::info_json(&session, self.state.config.mode).await {
            Ok(json) => Ok(CallToolResult::success(vec![ContentBlock::text(json)])),
            Err(e) => {
                if e.is_connection() {
                    self.state.invalidate_session().await;
                }
                Err(to_mcp_error(e))
            }
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ZemonMcpServer {}
