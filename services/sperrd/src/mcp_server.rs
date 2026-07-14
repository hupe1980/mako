//! MCP server for `sperrd` — Sperrung Execution Tracking (NB role).
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `list_sperr_orders` | List Sperrung orders (filterable by status) |
//! | `get_sperr_order` | Get a single Sperrung order by ID |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |---|---|
//! | `execute-sperrung` | Step-by-step: execute a Sperrung order and confirm IFTSTA 21039 |

use axum::{
    Router,
    middleware::{self, Next},
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
    model::*,
    prompt, prompt_handler, prompt_router, schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct SperrdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSperrParams {
    /// Filter by status: pending, executed, failed, cancelled.
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSperrParams {
    pub id: String,
}

#[derive(Clone)]
pub struct SperrdMcpHandler {
    state: Arc<SperrdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<SperrdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<SperrdMcpHandler>,
}

#[tool_router]
impl SperrdMcpHandler {
    fn new(state: Arc<SperrdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "List Sperrung execution orders. Filter by status: pending (awaiting field confirmation), executed, failed, or cancelled. GPKE BK6-22-024 compliance requires IFTSTA 21039 dispatch within execution window.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_sperr_orders(
        &self,
        Parameters(p): Parameters<ListSperrParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_orders_pg;
        let status = p.status.as_deref();
        match list_orders_pg(&self.state.pool, status, None, p.limit.unwrap_or(50)).await {
            Ok(orders) => {
                let pending = orders.iter().filter(|o| o.status == "pending").count();
                Ok(CallToolResult::success(vec![ContentBlock::text(
                    serde_json::json!({
                        "count": orders.len(),
                        "orders": orders,
                        "pending_count": pending,
                    })
                    .to_string(),
                )]))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get a single Sperrung order by UUID. Returns execution timestamps, IFTSTA dispatch status, and associated ORDERS process reference.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_sperr_order(
        &self,
        Parameters(p): Parameters<GetSperrParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_order_pg;
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match fetch_order_pg(&self.state.pool, id).await {
            Ok(Some(order)) => Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_value(order).unwrap_or_default().to_string(),
            )])),
            Ok(None) => Err(McpError::invalid_params(
                format!("order {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl SperrdMcpHandler {
    #[prompt(
        name = "execute-sperrung",
        description = "Step-by-step: execute a Sperrung order and confirm IFTSTA 21039"
    )]
    async fn execute_sperrung_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "The field team has executed a Sperrung. How do I confirm it?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `list_sperr_orders` with status=pending to find the order.\n                 2. Confirm the malo_id and orders_process_id match the field report.\n                 3. PUT /api/v1/sperr-orders/{id}/execute with executed_at timestamp.\n                 4. sperrd auto-dispatches IFTSTA 21039 to the LF via makod.\n                 5. Verify the order moves to status=executed with iftsta_dispatched_at set.\n\n                 GPKE BK6-22-024: The IFTSTA 21039 must be sent within the ORDERS execution window.\n                 A missed dispatch can only be corrected by a new IFTSTA — contact the LF immediately.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for SperrdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder().enable_tools().enable_prompts().build(),
        )
        .with_server_info(Implementation::new("sperrd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "sperrd MCP — Sperrung (disconnection) Execution Tracking (NB role).\n\
             Tracks ORDERS 17115/17117 execution and auto-dispatches IFTSTA 21039 on field confirmation.\n\
             A missing IFTSTA 21039 leaves the Sperrung permanently unresolved in the LF's system (GPKE violation).\n\n\
             Use `list_sperr_orders` to see pending orders requiring field team action.\n\
             Use `get_sperr_order` to check a specific order's execution timeline.\n\
             Confirm execution via PUT /api/v1/sperr-orders/{id}/execute (triggers IFTSTA 21039 auto-dispatch).",
        )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<SperrdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<SperrdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = SperrdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
