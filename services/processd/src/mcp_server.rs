//! MCP server for `processd`.
//!
//! Exposes STP (Standardisierter Technischer Prozess) decisions and LF approval
//! queue reads via the MCP Streamable HTTP transport (spec 2025-11-25).
//! Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `list_decisions`           | List recent NB Anmeldung STP decisions |
//! | `get_stp_rate`             | Approval rate for NB STP decisions over N days |
//! | `list_pending_approvals`   | List LF approval-queue entries needing operator action |
//! | `get_queue_entry`          | Get a single LF approval-queue entry by its UUID |

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
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ProcessdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
    /// makod base URL — required for approve/reject dispatch.
    pub makod_url: String,
    /// makod API key for command dispatch.
    pub makod_api_key: secrecy::SecretString,
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
pub struct QueueActionParams {
    /// UUID of the approval queue entry to approve or reject.
    pub id: String,
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
    #[allow(dead_code)]
    prompt_router: PromptRouter<ProcessdMcpHandler>,
}

#[tool_router]
impl ProcessdMcpHandler {
    fn new(state: Arc<ProcessdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
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

    #[tool(
        description = "List LF approval-queue entries needing operator action (status: Pending/Approved/Rejected/Expired)."
    )]
    async fn list_pending_approvals(
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
    #[tool(
        description = "Approve a pending LF E_0624 Einwilligung queue entry. \
Dispatches `gpke.nb-lieferende.bestaetigen` (PID 55008 Strom) or \
`geli.gas.stornierung.initiieren` (Gas 44022/44023) to makod, then marks the entry Approved. \
⚠ Regulatory: the 45-min APERAK window applies from the original process.initiated event. \
Use `list_pending_approvals` first to check `expires_at` before approving.",
        annotations(
            read_only_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn approve_queue_entry(
        &self,
        Parameters(p): Parameters<QueueActionParams>,
    ) -> Result<CallToolResult, McpError> {
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        // Call processd's own approval REST endpoint via HTTP self-call.
        let client = reqwest::Client::new();
        let url = format!(
            "{}/api/v1/approval-queue/{id}/approve",
            self.state
                .makod_url
                .trim_end_matches('/')
                .replace(":8080", ":8580")
        );
        match client
            .put(&url)
            .bearer_auth(secrecy::ExposeSecret::expose_secret(
                &self.state.makod_api_key,
            ))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() || resp.status() == 204 => {
                ContentBlock::json(serde_json::json!({
                    "id": p.id,
                    "status": "Approved",
                    "note": "einwilligung dispatched to makod; queue entry marked Approved.",
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(resp) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "approve failed: HTTP {}",
                resp.status()
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Reject a pending LF E_0624 Einwilligung queue entry. \
Dispatches `gpke.nb-lieferende.ablehnen` (PID 55009) to makod, then marks the entry Rejected. \
Use this when §20 EnWG affiliate check is the reason (operator override). \
For §20 parity data: use `obsd` `get_kpi_report`.",
        annotations(
            read_only_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn reject_queue_entry(
        &self,
        Parameters(p): Parameters<QueueActionParams>,
    ) -> Result<CallToolResult, McpError> {
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        let client = reqwest::Client::new();
        let url = format!(
            "{}/api/v1/approval-queue/{id}/reject",
            self.state
                .makod_url
                .trim_end_matches('/')
                .replace(":8080", ":8580")
        );
        match client
            .put(&url)
            .bearer_auth(secrecy::ExposeSecret::expose_secret(
                &self.state.makod_api_key,
            ))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() || resp.status() == 204 => {
                ContentBlock::json(serde_json::json!({
                    "id": p.id,
                    "status": "Rejected",
                    "note": "ablehnen dispatched to makod; queue entry marked Rejected.",
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(resp) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "reject failed: HTTP {}",
                resp.status()
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl ProcessdMcpHandler {
    #[prompt(
        name = "triage-nb-rejection",
        description = "Step-by-step: investigate why an NB Anmeldung was rejected"
    )]
    async fn triage_nb_rejection_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "An NB rejected a Lieferbeginn Anmeldung. How do I investigate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `get_anmeldung_decision` with the process_id to see the ERC code.\n\
                 2. ERC codes from netz-checker:\n\
                    - A02: MaLo not found in marktd (malo_grid missing or bilanzierungsgebiet mismatch)\n\
                    - A05: Preisblatt missing for the NB MP-ID + Sparte combination\n\
                    - A06: Lieferbeginn date outside allowed range (too far future / past)\n\
                    - A97: initiator_is_affiliate = true, auto-accept blocked (§20 EnWG parity)\n\
                    - A99: internal processing error (check processd logs)\n\
                 3. Fix A02: PUT /api/v1/malo/{malo_id}/grid in marktd with correct netzebene/bilanzierungsgebiet.\n\
                 4. Fix A05: PUT /api/v1/preisblaetter/{nb_mp_id} in marktd with current tariff.\n\
                 5. Fix A97: submit manual approval via PUT /api/v1/approval-queue/{id}/approve.",
            ),
        ]
    }

    #[prompt(
        name = "trigger-lieferbeginn",
        description = "Step-by-step: initiate a Lieferbeginn Anmeldung (Strom or Gas)"
    )]
    async fn trigger_lieferbeginn_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I trigger a Lieferbeginn Anmeldung for a MaLo?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "For Strom (GPKE UTILMD PID 55001):\n\
                 POST /api/v1/start-supply with malo_id, lieferbeginn_datum (YYYY-MM-DD).\n\
                 LFW24 Vorlauffrist: submission must be before 15:00 on the Meldetermin.\n\n\
                 For Gas (GeLi Gas UTILMD G PID 44001):\n\
                 POST /api/v1/start-supply-gas with malo_id, lieferbeginn_datum, gasqualitaet.\n\n\
                 processd validates:\n\
                 - MaLo exists in marktd with correct netzebene/bilanzierungsgebiet\n\
                 - Active Preisblatt for the NB\n\
                 - Lieferbeginn date within Vorlauffrist window\n\
                 - §20 EnWG: affiliate check (auto-accept blocked if initiator_is_affiliate)\n\n\
                 On success: makod dispatches UTILMD 55001/44001 EDIFACT to NB.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for ProcessdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("processd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "processd MCP — Anmeldung STP decisions (NB) and E_0624 approval queue (LF).\n\
                 NB: `list_decisions`, `get_stp_rate`.\n\
                 LF: `list_pending_approvals`, `get_queue_entry`.",
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
