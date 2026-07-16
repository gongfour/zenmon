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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    /// Key expression to query.
    pub key_expr: String,
    /// Optional payload to include in the GET.
    #[serde(default)]
    pub payload: Option<String>,
    /// Timeout in milliseconds (default 5000).
    #[serde(default = "default_query_timeout_ms")]
    pub timeout_ms: u64,
    /// Return at most N replies.
    #[serde(default)]
    pub limit: Option<u64>,
}

fn default_query_timeout_ms() -> u64 {
    5000
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct LivelinessParams {
    /// Key expression to filter (default "**").
    #[serde(default = "default_key_expr")]
    pub key_expr: String,
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
        let r = handlers::discover_json(&session, &p.key_expr).await;
        self.finish(r).await
    }

    #[tool(description = "Show the effective, allow-listed configuration. No network.")]
    fn config_show(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let json = handlers::config_show_json().map_err(to_mcp_error)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(description = "Show current Zenoh session information.")]
    async fn info(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let session = self.state.session().await.map_err(to_mcp_error)?;
        let r = handlers::info_json(&session, self.state.config.mode).await;
        self.finish(r).await
    }

    #[tool(description = "Send a Zenoh GET query and collect replies.")]
    async fn query(
        &self,
        Parameters(p): Parameters<QueryParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let session = self.state.session().await.map_err(to_mcp_error)?;
        let r = handlers::query_json(
            &session,
            &p.key_expr,
            p.payload.as_deref(),
            std::time::Duration::from_millis(p.timeout_ms),
            p.limit.map(|n| n as usize),
        )
        .await;
        self.finish(r).await
    }

    #[tool(description = "List discovered Zenoh nodes (one snapshot).")]
    async fn nodes(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let session = self.state.session().await.map_err(to_mcp_error)?;
        self.finish(handlers::nodes_json(&session).await).await
    }

    #[tool(description = "Query current liveliness tokens.")]
    async fn liveliness(
        &self,
        Parameters(p): Parameters<LivelinessParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let session = self.state.session().await.map_err(to_mcp_error)?;
        self.finish(handlers::liveliness_json(&session, &p.key_expr).await)
            .await
    }
}

impl ZemonMcpServer {
    /// Shared success/invalidate-on-connection-error path for session-backed
    /// tools: wrap `Ok` JSON as a successful tool result, and on `Err` drop the
    /// cached session first if the failure is connection-kind (see
    /// `ServerState::invalidate_session`) before mapping to an MCP error.
    async fn finish(
        &self,
        r: Result<String, zemon_core::error::ZemonError>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match r {
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
