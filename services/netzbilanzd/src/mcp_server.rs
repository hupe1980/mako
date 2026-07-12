//! MCP server for `netzbilanzd` — NNE/KA/MMM Billing Daemon (NB role).
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `list_billing_records` | List NNE/MMM invoice records (drafts + dispatched) |
//! | `list_disputed` | List invoices with `Disputed` outcome (REMADV 33002 received) |
//! | `get_billing_record` | Get a single invoice record with full Rechnung BO4E |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |---|---|
//! | `trigger-nne-billing` | Step-by-step: run an NNE billing run for a MaLo |
//! | `investigate-dispute` | Step-by-step: investigate a disputed REMADV 33002 |

use std::sync::Arc;
use axum::{Router, http::StatusCode, middleware::{self, Next}, response::IntoResponse};
use mako_service::{cedar::CedarEnforcer, oidc::OidcVerifier};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::{prompt::PromptRouter, tool::ToolRouter}, wrapper::Parameters},
    model::*,
    prompt, prompt_handler, prompt_router, schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
pub struct NetzbilanzMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListBillingParams {
    /// Filter by MaLo-ID.
    pub malo_id: Option<String>,
    /// Filter by LF MP-ID.
    pub lf_mp_id: Option<String>,
    /// Filter by outcome (Sent/Paid/PartialPaid/Disputed/ValidationFailed/Error).
    pub outcome: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRecordParams {
    pub id: String,
}

#[derive(Clone)]
pub struct NetzbilanzMcpHandler {
    state: Arc<NetzbilanzMcpState>,
    #[allow(dead_code)] tool_router: ToolRouter<NetzbilanzMcpHandler>,
    #[allow(dead_code)] prompt_router: PromptRouter<NetzbilanzMcpHandler>,
}

#[tool_router]
impl NetzbilanzMcpHandler {
    fn new(state: Arc<NetzbilanzMcpState>) -> Self {
        Self { state, tool_router: Self::tool_router(), prompt_router: Self::prompt_router() }
    }

    #[tool(description = "List NNE/KA/MMM invoice drafts (INVOIC 31001/31002/31005). Filter by malo_id, lf_mp_id, or status (draft/dispatched/paid/disputed). Returns summary without full Rechnung. Use after POST /api/v1/billing/run.",
        annotations(read_only_hint = true, open_world_hint = false))]
    async fn list_nne_drafts(
        &self,
        Parameters(p): Parameters<ListBillingParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_billing_records;
        match list_billing_records(
            &self.state.pool, &self.state.tenant,
            p.malo_id.as_deref(), p.lf_mp_id.as_deref(),
            p.outcome.as_deref(), p.limit.unwrap_or(50),
        ).await {
            Ok(rows) => ContentBlock::json(serde_json::to_value(rows).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b])).map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "List all NNE/KA/MMM invoices with Disputed outcome (REMADV 33002 received). Shows records requiring COMDIS 29001 escalation or re-billing.",
        annotations(read_only_hint = true, open_world_hint = false))]
    async fn list_disputed(&self, Parameters(_): Parameters<serde_json::Value>) -> Result<CallToolResult, McpError> {
        use crate::pg::list_billing_records;
        match list_billing_records(
            &self.state.pool, &self.state.tenant,
            None, None, Some("Disputed"), 100,
        ).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "disputed_count": rows.len(),
                "records": rows,
                "hint": "Use COMDIS 29001 (mako-gpke) for formal dispute escalation after REMADV 33002.",
            })).map(|b| CallToolResult::success(vec![b])).map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "Get a single NNE invoice draft by UUID, including the full BO4E Rechnung JSON payload (Grundpreis, Arbeitspreis, Leistungspreis, KA, invoic-checker findings).",
        annotations(read_only_hint = true, open_world_hint = false))]
    async fn get_nne_draft(&self, Parameters(p): Parameters<GetRecordParams>) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_billing_record;
        let Ok(id) = p.id.parse::<Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match fetch_billing_record(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(row)) => ContentBlock::json(serde_json::to_value(row).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b])).map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(format!("record {id} not found"), None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl NetzbilanzMcpHandler {
    #[prompt(
        name = "trigger-nne-billing",
        description = "Step-by-step: run NNE billing for a MaLo"
    )]
    async fn trigger_nne_billing_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "Trigger an NNE billing run for a MaLo."),
            PromptMessage::new_text(Role::Assistant, 
                "1. POST /api/v1/billing/run with malo_id, nb_mp_id, lf_mp_id, period_from, period_to.\n                 2. The draft is validated by invoic-checker before dispatch.\n                 3. GET /api/v1/billing/drafts to review the draft Rechnung BO4E.\n                 4. PUT /api/v1/billing/drafts/{id}/dispatch → sends INVOIC 31001 to makod.\n                 5. Alternatively: PUT /api/v1/billing/drafts/{id}/reject to cancel.\n\n                 Use `list_nne_drafts` to monitor draft status.\n                 PID 31002 (MMM-Rechnung) and 31005 (KA) follow the same flow.",
            ),
        ]
    }

    #[prompt(
        name = "investigate-dispute",
        description = "Step-by-step: investigate a REMADV 33002 dispute"
    )]
    async fn investigate_dispute_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "A REMADV 33002 dispute was received. What should I do?"),
            PromptMessage::new_text(Role::Assistant, 
                "1. Use `list_nne_drafts` with status=rejected to find the disputed invoice.\n                 2. The dispute reason is in the REMADV ERC code (e.g. A02=Datenfehler, A05=Preisfehler).\n                 3. Check `get_nne_draft` for the full Rechnung BO4E and invoic-checker findings.\n                 4. Fix the underlying data: update tariff in marktd, correct Messreihe in edmd.\n                 5. Re-run billing: POST /api/v1/billing/run with corrected parameters.\n                 6. Confirm with the LF after re-dispatch.",
            ),
        ]
    }
}


#[tool_handler]
#[prompt_handler]
impl ServerHandler for NetzbilanzMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder().enable_tools().enable_prompts().build()
        )
        .with_server_info(Implementation::new("netzbilanzd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "netzbilanzd MCP — NNE/KA/MMM Billing Daemon (NB role).\n\
             Generates INVOIC 31001/31002/31005/31009 and tracks payment outcomes.\n\
             Pre-dispatch self-validation via invoic-checker (same 6-check pipeline as invoicd).\n\n\
             Use `list_nne_drafts` to audit NNE/KA/MMM invoice status (distinct from LF customer invoices in billingd).\n\
             Use `list_disputed` to find REMADV 33002 rejections requiring COMDIS escalation.\n\
             Trigger billing runs via POST /api/v1/billing/run from your ERP.",
        )
    }

}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<NetzbilanzMcpState>>,
    request: axum::extract::Request, next: Next,
) -> axum::response::Response {
    match request.headers().get("Authorization")
        .and_then(|v| v.to_str().ok()).and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(t) if state.oidc.verify(t).is_ok() => next.run(request).await,
        Some(_) => (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
        None => (StatusCode::UNAUTHORIZED, "Authorization: Bearer required").into_response(),
    }
}

pub fn router(state: Arc<NetzbilanzMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = NetzbilanzMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new().route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
