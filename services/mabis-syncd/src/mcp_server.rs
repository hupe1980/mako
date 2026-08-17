//! MCP server for `mabis-syncd` — MaBiS Summenzeitreihe submission state (NB/ÜNB role).
//!
//! Read-only, deliberately: a Summenzeitreihe submission is a binding filing
//! with the BIKO, and triggering one stays behind the authenticated REST
//! surface (`POST /api/v1/sync`) where Cedar authorises a *person*. What an
//! agent needs is sight — the submission monitor was reading obsd's KPI report
//! as a proxy for the table this server now exposes.
//!
//! ## Tools (4)
//!
//! | Tool | Description |
//! |---|---|
//! | `get_submission_status`   | Latest runs + open-Korrekturbedarf and failed counts |
//! | `list_failed_submissions` | Failed runs, newest first, with `attempt_count` |
//! | `get_submission_run`      | One run by UUID |
//! | `list_korrekturbedarf`    | Open negative Prüfmitteilungen (§9.8.1 obligations) |
//!
//! ## Prompts (1)
//!
//! | Prompt | Description |
//! |---|---|
//! | `submission-triage` | Step-by-step: triage a failed or objected submission |

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

use crate::pg::{self, SubmissionRunRow};
use crate::server::{rfc3339, rfc3339_opt};

#[derive(Clone)]
pub struct MabisMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StatusParams {
    /// Maximum recent runs to include (default 10).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListFailedParams {
    /// Maximum results (default 20).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRunParams {
    /// UUID of the submission run.
    pub id: String,
}

/// One run, in the wire shape the REST surface serves — RFC 3339 instants,
/// never `time`'s derived component arrays. `version` matters most: §3.8.2
/// identifies a Summenzeitreihe by it, and the Prüfmitteilung endpoint parses
/// it back as RFC 3339.
fn run_json(r: &SubmissionRunRow) -> serde_json::Value {
    serde_json::json!({
        "id": r.id,
        "bilanzierungsgebiet_id": r.bilanzierungsgebiet_id,
        "period_from": r.period_from.to_string(),
        "period_to": r.period_to.to_string(),
        "version": rfc3339(r.version),
        "abrechnungslauf": r.abrechnungslauf,
        "phase": r.phase,
        "datenstatus": r.datenstatus,
        "status": r.status,
        "malo_count": r.malo_count,
        "interval_count": r.interval_count,
        "total_kwh": r.total_kwh,
        "has_substituted": r.has_substituted,
        "triggered_at": rfc3339(r.triggered_at),
        "submitted_at": rfc3339_opt(r.submitted_at),
        "acked_at": rfc3339_opt(r.acked_at),
        "message_ref": r.message_ref,
        "error_msg": r.error_msg,
        "attempt_count": r.attempt_count,
    })
}

/// Shared handler + router state.
#[derive(Clone)]
pub struct MabisMcpHandler {
    state: Arc<MabisMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<MabisMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<MabisMcpHandler>,
}

#[tool_router]
impl MabisMcpHandler {
    fn new(state: Arc<MabisMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "MaBiS submission health snapshot: the most recent Summenzeitreihe runs (with status, Datenstatus and attempt_count), plus failed-run and open-Korrekturbedarf counts. BK6-24-174 Anlage 3 §3.10: Erstaufschlag closes 10 Werktage after month end, Clearing 30.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_submission_status(
        &self,
        Parameters(p): Parameters<StatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(10).clamp(1, 100);
        let runs = pg::list_runs(&self.state.pool, &self.state.tenant, limit)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let failed = runs.iter().filter(|r| r.status == "failed").count();
        let exhausted = runs
            .iter()
            .filter(|r| r.status == "failed" && r.attempt_count >= 3)
            .count();
        let korrekturbedarf = pg::open_korrekturbedarf(&self.state.pool, &self.state.tenant)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
            .len();

        ContentBlock::json(serde_json::json!({
            "runs": runs.iter().map(run_json).collect::<Vec<_>>(),
            "failed_count": failed,
            // Past the scheduler's retry guard — these no longer retry themselves.
            "retry_exhausted_count": exhausted,
            "open_korrekturbedarf": korrekturbedarf,
            "note": if exhausted > 0 || korrekturbedarf > 0 {
                "ACTION REQUIRED: retry-exhausted runs or open Korrekturbedarf need an operator"
            } else if failed > 0 {
                "Failed runs present; the scheduler retries while attempt_count < 3"
            } else {
                "Submission cycle healthy"
            },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "List failed Summenzeitreihe submission runs, newest first. attempt_count >= 3 means the scheduler has stopped retrying (retry-exhausted) and an operator must act. error_msg carries the aggregation or BIKO submission failure.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_failed_submissions(
        &self,
        Parameters(p): Parameters<ListFailedParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(20).clamp(1, 100);
        let runs = pg::list_failed_runs(&self.state.pool, &self.state.tenant, limit)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        ContentBlock::json(serde_json::json!({
            "count": runs.len(),
            "failed_runs": runs.iter().map(run_json).collect::<Vec<_>>(),
            "regulatory_note":
                "BK6-24-174 Anlage 3 §3.10: a version filed within the Erstaufschlag window \
                 becomes Abrechnungsdaten directly; after it, corrections start as Prüfdaten \
                 and need a positive Prüfmitteilung.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Get a single Summenzeitreihe submission run by UUID, including version (RFC 3339), phase, Datenstatus, MSCONS message_ref and error_msg.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_submission_run(
        &self,
        Parameters(p): Parameters<GetRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match pg::get_run(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(r)) => ContentBlock::json(run_json(&r))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("run {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List open Korrekturbedarf: negative Prüfmitteilungen (§9.8.1) with no correcting submission yet. Each must be answered by a corrected Summenzeitreihe under a higher version within the Clearing window (30 Werktage after month end).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_korrekturbedarf(&self) -> Result<CallToolResult, McpError> {
        let rows = pg::open_korrekturbedarf(&self.state.pool, &self.state.tenant)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let items: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(id, gebiet, from, to, version)| {
                serde_json::json!({
                    "pruefmitteilung_id": id.to_string(),
                    "bilanzierungsgebiet_id": gebiet,
                    "period_from": from.to_string(),
                    "period_to": to.to_string(),
                    "version": rfc3339(version),
                })
            })
            .collect();
        ContentBlock::json(serde_json::json!({
            "count": items.len(),
            "korrekturbedarf": items,
            "regulatory_note":
                "BK6-24-174 Anlage 3 §9.8.1: answered by a corrected Summenzeitreihe under a \
                 higher version — an operator triggers it via POST /api/v1/sync with \
                 corrects_run_id.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[prompt_router]
impl MabisMcpHandler {
    #[prompt(
        name = "submission-triage",
        description = "Step-by-step: triage a failed Summenzeitreihe submission or an open Korrekturbedarf"
    )]
    async fn submission_triage_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "A MaBiS submission failed or the BIKO objected. How do I triage it?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**MaBiS Summenzeitreihe triage (BK6-24-174 Anlage 3)**\n\n\
                 **Step 1 — Snapshot**\n\
                 Call `get_submission_status`.\n\
                 - `retry_exhausted_count > 0`: the scheduler has given up (attempt_count >= 3) — go to Step 2.\n\
                 - `open_korrekturbedarf > 0`: the BIKO objected — go to Step 3.\n\
                 - only `failed_count > 0`: the scheduler still retries; watch, don't act.\n\n\
                 **Step 2 — Failed runs**\n\
                 Call `list_failed_submissions` and read `error_msg` per run:\n\
                 - Aggregation errors (edmd data missing): check whether the Lastgänge for the \
                 period exist and are complete before any retry.\n\
                 - BIKO submission errors (makod dispatch): a transport problem — the MSCONS \
                 never went out, so a retry files the same version safely.\n\
                 Deadline context: the Erstaufschlag window closes on the 10. Werktag after \
                 month end. A version filed later starts as Prüfdaten (needs a positive \
                 Prüfmitteilung) — say so in the escalation.\n\n\
                 **Step 3 — Korrekturbedarf**\n\
                 Call `list_korrekturbedarf`. Each entry is an open §9.8.1 obligation: a \
                 corrected Summenzeitreihe under a higher version, within the Clearing window \
                 (30 Werktage after month end). The correction itself is an operator action \
                 (`POST /api/v1/sync` with `corrects_run_id`) — name the run and the deadline \
                 in the worklist entry.\n\n\
                 **Output**: { submission_status, retry_exhausted: N, korrekturbedarf: N, action }",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for MabisMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new(
            "mabis-syncd",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(
            "mabis-syncd MCP — MaBiS Summenzeitreihe submission state (read-only).\n\
             Filing a submission is NOT exposed here: it is a binding filing with the BIKO \
             and stays behind the authenticated REST surface.\n\n\
             ## Tools (4)\n\
             - `get_submission_status(limit)` — recent runs + failed / retry-exhausted / \
             Korrekturbedarf counts\n\
             - `list_failed_submissions(limit)` — failed runs with attempt_count and error_msg\n\
             - `get_submission_run(id)` — one run, full detail\n\
             - `list_korrekturbedarf` — open negative Prüfmitteilungen (§9.8.1)\n\n\
             ## Prompts (1)\n\
             - `submission-triage` — failed-run and Korrekturbedarf triage workflow\n\n\
             ## MaBiS timing (BK6-24-174 Anlage 3 §3.10)\n\
             Erstaufschlag: 10 Werktage after month end — a new version becomes \
             Abrechnungsdaten directly. Clearing: 30 Werktage — a new version is Prüfdaten \
             until a positive Prüfmitteilung. After 30 Werktage: KBKA.",
        )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<MabisMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<MabisMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = MabisMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
