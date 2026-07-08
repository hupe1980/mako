//! MCP server for `processd`.

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
use mako_service::{
    cedar::CedarEnforcer,
    oidc::{Claims, OidcVerifier},
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ProcessdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDecisionsParams {
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetStpRateParams {
    pub days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListQueueParams {
    pub status: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetQueueEntryParams {
    pub id: String,
}

#[derive(Clone)]
pub struct ProcessdMcpHandler {
    state: Arc<ProcessdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<ProcessdMcpHandler>,
}

#[tool_router]
impl ProcessdMcpHandler {
    fn new(state: Arc<ProcessdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List recent Anmeldung STP decisions (NB role). Returns decisions ordered by decided_at descending."
    )]
    async fn list_decisions(
        &self,
        Parameters(params): Parameters<ListDecisionsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::anmeldung::PgAnmeldungRepository;
        let repo = PgAnmeldungRepository::new(self.state.pool.clone());
        let limit = params.limit.unwrap_or(50).min(200);
        match repo.list(&self.state.tenant, limit).await {
            Ok(records) => ContentBlock::json(serde_json::to_value(records).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get the Anmeldung STP rate for the last N days. NB role. Returns a float 0.0–1.0 or null when no decisions exist."
    )]
    async fn get_stp_rate(
        &self,
        Parameters(params): Parameters<GetStpRateParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::anmeldung::PgAnmeldungRepository;
        let repo = PgAnmeldungRepository::new(self.state.pool.clone());
        let days = params.days.unwrap_or(30);
        match repo.stp_rate(&self.state.tenant, days).await {
            Ok(rate) => ContentBlock::json(serde_json::json!({
                "stp_rate": rate, "window_days": days, "target": 0.95,
                "compliant": rate.is_none_or(|r| r >= 0.95),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "List LF approval queue entries needing operator action.")]
    async fn list_queue(
        &self,
        Parameters(params): Parameters<ListQueueParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::approval::{PgApprovalQueue, QueueStatus};
        let queue = PgApprovalQueue::new(self.state.pool.clone());
        let status: Option<QueueStatus> = params
            .status
            .as_deref()
            .map(|s| s.parse().unwrap_or(QueueStatus::Pending))
            .or(Some(QueueStatus::Pending));
        let limit = params.limit.unwrap_or(50);
        match queue.list(&self.state.tenant, status, limit).await {
            Ok(entries) => ContentBlock::json(serde_json::to_value(entries).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "Get a single LF approval queue entry by its UUID.")]
    async fn get_queue_entry(
        &self,
        Parameters(params): Parameters<GetQueueEntryParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::approval::PgApprovalQueue;
        let Ok(id) = params.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        let queue = PgApprovalQueue::new(self.state.pool.clone());
        match queue.find_by_id(id, &self.state.tenant).await {
            Ok(Some(entry)) => ContentBlock::json(serde_json::to_value(entry).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("approval queue entry {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[tool_handler]
impl ServerHandler for ProcessdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("processd", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "processd MCP — Anmeldung STP decisions (NB) and E_0624 approval queue (LF).\n\
                 NB: `list_decisions`, `get_stp_rate`.\n\
                 LF: `list_queue`, `get_queue_entry`.",
            )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<ProcessdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let token = match request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(t) => t.to_owned(),
        None => {
            return (StatusCode::UNAUTHORIZED, "Authorization: Bearer required").into_response();
        }
    };
    let claims = match state.oidc.verify(&token) {
        Ok(c) => Claims(c),
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
    };
    if let Err(e) = state
        .cedar
        .check(&claims.principal(), "use-mcp", &state.tenant)
    {
        return (StatusCode::FORBIDDEN, format!("403: {e}")).into_response();
    }
    next.run(request).await
}

pub fn router(state: Arc<ProcessdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = ProcessdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
