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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScoutParams {
    /// Inclusive multicast port range start (default 7446).
    #[serde(default = "default_scout_start")]
    pub start_port: u16,
    /// Inclusive multicast port range end (default 7546).
    #[serde(default = "default_scout_end")]
    pub end_port: u16,
    /// Per-port timeout in milliseconds (default 1000).
    #[serde(default = "default_scout_timeout_ms")]
    pub per_port_timeout_ms: u64,
}

fn default_scout_start() -> u16 {
    7446
}

fn default_scout_end() -> u16 {
    7546
}

fn default_scout_timeout_ms() -> u64 {
    1000
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DoctorParams {
    /// Overall diagnostic deadline in milliseconds (default 5000).
    #[serde(default = "default_doctor_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_doctor_timeout_ms() -> u64 {
    5000
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PubParams {
    /// Key expression to publish to.
    pub key_expr: String,
    /// Value payload (string).
    pub value: String,
    /// Optional attachment metadata (string).
    #[serde(default)]
    pub att: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SubSnapshotParams {
    /// Key expression to subscribe to.
    pub key_expr: String,
    /// Stop after N messages.
    #[serde(default)]
    pub count: Option<u64>,
    /// Stop after this many milliseconds.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Cap each payload preview to N bytes in the JSON view.
    #[serde(default)]
    pub max_payload_bytes: Option<u64>,
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

    #[tool(description = "Scout multicast ports for Zenoh nodes (no router needed).")]
    async fn scout(
        &self,
        Parameters(p): Parameters<ScoutParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let r = handlers::scout_json(
            &self.state.config,
            std::time::Duration::from_millis(p.per_port_timeout_ms),
            (p.start_port, p.end_port),
        )
        .await;
        // scout uses transient sessions, so no shared-session invalidation.
        r.map(|json| CallToolResult::success(vec![ContentBlock::text(json)]))
            .map_err(to_mcp_error)
    }

    #[tool(description = "Diagnose the connection: config, session, connectivity checks.")]
    async fn doctor(
        &self,
        Parameters(p): Parameters<DoctorParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let r = handlers::doctor_json(
            &self.state.config,
            std::time::Duration::from_millis(p.timeout_ms),
        )
        .await;
        // doctor opens its own transient session(s), so no shared-session
        // invalidation.
        r.map(|json| CallToolResult::success(vec![ContentBlock::text(json)]))
            .map_err(to_mcp_error)
    }

    #[tool(
        description = "Subscribe and collect a bounded snapshot of messages (count and/or duration)."
    )]
    async fn sub_snapshot(
        &self,
        Parameters(p): Parameters<SubSnapshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let session = self.state.session().await.map_err(to_mcp_error)?;
        let r = handlers::sub_snapshot_json(
            &session,
            &p.key_expr,
            p.count.map(|n| n as usize),
            p.duration_ms.map(std::time::Duration::from_millis),
            p.max_payload_bytes.map(|n| n as usize),
        )
        .await;
        self.finish(r).await
    }

    // `pub` is a Rust keyword, so the method is named `publish`; the tool is
    // exposed to MCP clients as `pub` via the macro's `name = "..."` argument
    // (mirrors the CLI's `zemon pub` subcommand name).
    #[tool(
        name = "pub",
        description = "Publish a value to a key expression (test injection; mutates the network)."
    )]
    async fn publish(
        &self,
        Parameters(p): Parameters<PubParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let session = self.state.session().await.map_err(to_mcp_error)?;
        let r = handlers::pub_json(&session, &p.key_expr, &p.value, p.att.as_deref()).await;
        self.finish(r).await
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
