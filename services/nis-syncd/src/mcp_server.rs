//! MCP (Model Context Protocol) server for `nis-syncd` — Grid Topology Sync.
//!
//! Exposes NIS/GIS grid topology synchronisation operations to LLM tooling.
//!
//! ## Tools
//! | Tool | Description |
//! |---|---|
//! | `sync_grid`             | Trigger a full NIS export sync to marktd (idempotent) |
//! | `dry_run_sync`          | Dry-run: compare NIS data with marktd without writing |
//! | `check_malo_grid`       | Look up a single MaLo's grid record in marktd |
//! | `get_last_sync_report`  | Inspect the most recent sync result without running a new sync |
//!
//! ## Prompts
//! | Prompt | Description |
//! |---|---|
//! | `run-grid-sync`          | Step-by-step: sync NIS data and verify STP improvement |
//! | `check-stp-readiness`    | Diagnose processd NB STP rate; find MaLos with missing grid data |

use std::sync::Arc;

use axum::{
    Router,
    middleware::{self, Next},
};
use mako_markt::marktd_client::MarktdClient;
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
use tokio_util::sync::CancellationToken;

use crate::sync::LastSyncReport;

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct NisSyncdMcpState {
    pub auth: mako_service::mcp_auth::McpAuth,
    pub nb_mp_id: String,
    pub service_base_url: String,
    /// HTTP client for outgoing calls to nis-syncd's own REST API.
    pub http_client: reqwest::Client,
    /// Bearer token for outgoing calls to `marktd` (push MaLo grid records).
    pub marktd_api_key: String,
    /// Direct marktd client for MaLo grid record lookups without an HTTP round-trip.
    pub marktd: Arc<MarktdClient>,
    /// Shared cache of the most recent sync report (updated by the HTTP handler).
    pub last_report: LastSyncReport,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SyncGridParams {
    /// NIS export batch as JSON array.
    ///
    /// Each element: `{ "malo_id": "...", "bilanzierungsgebiet": "...", "netzgebiet": "...", "sparte": "STROM|GAS" }`.
    /// `bilanzierungsgebiet` and `netzgebiet` may be `null`.
    /// If omitted, behaviour depends on the NIS adapter configuration.
    pub entries: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckMaloGridParams {
    /// 11-digit Marktlokations-ID to look up.
    pub malo_id: String,
}

/// Placeholder parameter struct for tools that take no arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EmptyParams {}

// ── Handler ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct NisSyncdMcpHandler {
    state: Arc<NisSyncdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<NisSyncdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<NisSyncdMcpHandler>,
}

#[tool_router]
impl NisSyncdMcpHandler {
    fn new(state: Arc<NisSyncdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "Trigger a NIS/GIS grid topology sync: pushes malo_grid records (bilanzierungsgebiet, netzgebiet, sparte) to marktd. Idempotent — safe to call repeatedly. Uses bounded concurrency. Returns SyncReport with counts of updated, skipped, drift_count, and errors.",
        annotations(
            idempotent_hint = true,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn sync_grid(
        &self,
        Parameters(p): Parameters<SyncGridParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut body = serde_json::json!({});
        if let Some(entries) = p.entries {
            body["entries"] = serde_json::Value::Array(entries);
        }
        let resp = self
            .state
            .http_client
            .post(format!("{}/api/v1/grid/sync", self.state.service_base_url))
            .bearer_auth(&self.state.marktd_api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let status = resp.status();
        let json: serde_json::Value = resp
            .json()
            .await
            .unwrap_or(serde_json::json!({ "status": status.as_u16() }));
        ContentBlock::json(json)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Dry-run grid sync: compares NIS export data against marktd without writing. Returns drift report showing which MaLos have changed bilanzierungsgebiet, netzgebiet, or sparte. Useful to preview changes before committing.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn dry_run_sync(
        &self,
        Parameters(p): Parameters<SyncGridParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut body = serde_json::json!({});
        if let Some(entries) = p.entries {
            body["entries"] = serde_json::Value::Array(entries);
        }
        let resp = self
            .state
            .http_client
            .post(format!(
                "{}/api/v1/grid/sync?dry_run=true",
                self.state.service_base_url
            ))
            .bearer_auth(&self.state.marktd_api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let json: serde_json::Value = resp
            .json()
            .await
            .unwrap_or(serde_json::json!({ "error": "failed to parse response" }));
        ContentBlock::json(json)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Look up a specific MaLo's grid record in marktd. Returns bilanzierungsgebiet (Bilanzierungsgebiet-EIC), netzgebiet, sparte, nb_mp_id, source, and last sync timestamp. Returns found=false if the MaLo has never been synced.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn check_malo_grid(
        &self,
        Parameters(p): Parameters<CheckMaloGridParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .state
            .marktd
            .get_malo_grid(&p.malo_id)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let json = match result {
            Some(rec) => serde_json::json!({
                "found": true,
                "malo_id": p.malo_id,
                "nb_mp_id": rec.nb_mp_id,
                "bilanzierungsgebiet": rec.bilanzierungsgebiet,
                "netzgebiet": rec.netzgebiet,
                "sparte": rec.sparte.to_string(),
                "source": rec.source,
                "note": if rec.bilanzierungsgebiet.is_none() {
                    "bilanzierungsgebiet is null — processd check 4 will use UTILMD value, which may differ from NIS."
                } else {
                    "Grid record present and complete."
                },
            }),
            None => serde_json::json!({
                "found": false,
                "malo_id": p.malo_id,
                "note": "No grid record in marktd. processd check 4 will escalate for this MaLo. Run sync_grid to import from NIS.",
            }),
        };

        ContentBlock::json(json)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Returns the most recent sync report (updated, skipped, drift_detected, drift_count, errors) without triggering a new sync. Returns a note if no sync has run since startup. Use this to assess grid data health before investigating STP drops.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_last_sync_report(
        &self,
        Parameters(_): Parameters<EmptyParams>,
    ) -> Result<CallToolResult, McpError> {
        let guard = self.state.last_report.read().await;
        let json = match guard.as_ref() {
            Some(r) => serde_json::to_value(r).unwrap_or_default(),
            None => serde_json::json!({
                "note": "No sync has run since startup. Call sync_grid or dry_run_sync to populate.",
            }),
        };
        ContentBlock::json(json)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[prompt_router]
impl NisSyncdMcpHandler {
    #[prompt(
        name = "run-grid-sync",
        description = "Step-by-step: sync NIS/GIS data to marktd and verify processd STP improvement"
    )]
    async fn run_grid_sync_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I sync NIS/GIS grid topology data to improve processd NB STP rate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "processd NB STP (Straight-Through Processing) requires accurate Bilanzierungsgebiet \
                 data in marktd. Without it, processd check 4 falls back to the MaLo master data or \
                 escalates to the operator.\n\n\
                 1. **Dry-run first**: call `dry_run_sync` with your NIS export to preview changes.\n\
                    - `drift_count > 0`: NIS has changed since last sync — review the diff.\n\
                    - `drift_count = 0`: marktd is up-to-date, no write needed.\n\
                 2. **Commit**: call `sync_grid` to apply changes (idempotent, safe to repeat).\n\
                 3. **Verify individual MaLos**: use `check_malo_grid` for recently rejected Anmeldungen.\n\
                    - `found: false` = MaLo not in marktd. Add it to the NIS export and sync.\n\
                    - `bilanzierungsgebiet: null` = record present but incomplete. Enrich NIS data.\n\
                 4. **Check last report**: `get_last_sync_report` shows update/skip/error counts.\n\n\
                 Target STP: >= 95 %% (requires grid records for all active MaLos).\n\
                 Recommended schedule: nightly cron job posting the full NIS export.",
            ),
        ]
    }

    #[prompt(
        name = "check-stp-readiness",
        description = "Diagnose processd NB STP rate: identify MaLos missing bilanzierungsgebiet or not yet synced from NIS"
    )]
    async fn check_stp_readiness_prompt(&self) -> Vec<PromptMessage> {
        let nb = &self.state.nb_mp_id;
        vec![
            PromptMessage::new_text(
                Role::User,
                "My processd NB STP rate dropped. Which MaLos are causing escalations due to missing grid data?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                format!(
                    "STP drops caused by missing malo_grid records follow a clear pattern. \
                     Here is the full diagnostic workflow for NB {nb}:\n\n\
                     **Step 1 — Check last sync health**\n\
                     Call `get_last_sync_report`.\n\
                     - `errors > 0`: some MaLos failed to sync — check network/auth to marktd.\n\
                     - `drift_count > 0`: NIS diverged after the last sync. Call `sync_grid`.\n\
                     - No report (first run): call `sync_grid` immediately.\n\n\
                     **Step 2 — Check specific MaLos from rejected Anmeldungen**\n\
                     For each rejected Anmeldung in processd (ERC A02/A99):\n\
                     1. Call `check_malo_grid` with the malo_id.\n\
                     2. `found: false` → run `sync_grid` with that MaLo's NIS data.\n\
                     3. `found: true, bilanzierungsgebiet: null` → enrich NIS export and re-sync.\n\n\
                     **Step 3 — Full NIS sync (safest fix)**\n\
                     1. Export ALL MaLos from NIS in JSON format.\n\
                     2. Call `dry_run_sync` to preview. Review `drift_count`.\n\
                     3. Call `sync_grid` to commit. STP should recover within minutes.\n\n\
                     **Regulatory note**: per §20 EnWG, NB must not disadvantage LF Anmeldungen \
                     due to stale internal data. Missing grid records = §20 EnWG risk.\n\n\
                     Configured NB: {nb}. Concurrency: 20 parallel marktd writes."
                ),
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for NisSyncdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("nis-syncd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "nis-syncd MCP — NIS/GIS Grid Topology Sync (NB role).\n\
             Pushes malo_grid records (bilanzierungsgebiet, netzgebiet, sparte) to marktd.\n\
             Improves processd NB STP from ~80% to >=95%.\n\n\
             ## Tools (4)\n\
             - sync_grid: Push NIS export to marktd (idempotent, concurrent)\n\
             - dry_run_sync: Preview drift without writing\n\
             - check_malo_grid: Look up a specific MaLo's grid record\n\
             - get_last_sync_report: Read the latest sync result without running a new sync\n\n\
             ## Prompts (2)\n\
             - run-grid-sync: Step-by-step sync + verification workflow\n\
             - check-stp-readiness: Diagnose STP drops, find missing grid records",
        )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<NisSyncdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: Arc<NisSyncdMcpState>, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)))
        .with_cancellation_token(shutdown);

    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(NisSyncdMcpHandler::new(state.clone()))
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );

    Router::new()
        .route_service("/mcp", mcp_service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
