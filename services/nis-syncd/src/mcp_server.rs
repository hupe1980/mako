//! MCP (Model Context Protocol) server for `nis-syncd` — Grid Topology Sync.
//!
//! Exposes NIS/GIS grid topology synchronisation operations to LLM tooling.
//!
//! ## Tools
//! | Tool | Description |
//! |---|---|
//! | `sync_grid`     | Trigger a full NIS export sync to marktd (idempotent) |
//! | `dry_run_sync`  | Dry-run: compare NIS data with marktd without writing |

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
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
use tokio_util::sync::CancellationToken;

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct NisSyncdMcpState {
    pub marktd_api_key: String,
    pub nb_mp_id: String,
    pub service_base_url: String,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SyncGridParams {
    /// NIS export batch as JSON array. Each entry: `{ "malo_id": "...", "bilanzierungsgebiet": "...", "netzgebiet": "...", "sparte": "STROM|GAS" }`.
    /// If omitted, nis-syncd triggers a sync using the last cached NIS export.
    pub records: Option<Vec<serde_json::Value>>,
}

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
        description = "Trigger a NIS/GIS grid topology sync: pushes malo_grid records to marktd. Idempotent — safe to call repeatedly. Returns count of synced, drifted, and skipped entries.",
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
        let client = reqwest::Client::new();
        let mut body = serde_json::json!({ "nb_mp_id": self.state.nb_mp_id });
        if let Some(records) = p.records {
            body["records"] = serde_json::Value::Array(records);
        }
        let resp = client
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
        description = "Dry-run grid sync: compares NIS export data against marktd without writing. Returns drift report showing which MaLos have changed bilanzierungsgebiet or netzgebiet.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn dry_run_sync(
        &self,
        Parameters(p): Parameters<SyncGridParams>,
    ) -> Result<CallToolResult, McpError> {
        let client = reqwest::Client::new();
        let mut body = serde_json::json!({ "nb_mp_id": self.state.nb_mp_id });
        if let Some(records) = p.records {
            body["records"] = serde_json::Value::Array(records);
        }
        let resp = client
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
                "processd NB STP (Straight-Through Processing) requires accurate Bilanzierungsgebiet data in marktd.\n                 Without it, processd check 4 falls back to MaLo master data or is skipped, leading to manual escalations.\n\n                 1. First, run a dry-run: use `dry_run_sync` to see which MaLos have changed.\n                 2. Review the drift report — entries with changed `bilanzierungsgebiet` are most impactful.\n                 3. If the drift looks correct, run `sync_grid` to apply changes to marktd.\n                 4. Verify: call processd GET /api/v1/decisions (recent decisions should show more auto-accepts).\n\n                 Target: processd NB STP >= 95% (up from ~80% without grid data).\n                 Schedule sync_grid daily (e.g. via cron) to keep marktd in sync with NIS.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for NisSyncdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder().enable_tools().enable_prompts().build(),
        )
        .with_server_info(Implementation::new("nis-syncd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "nis-syncd MCP — NIS/GIS Grid Topology Sync (NB role).\n             Pushes malo_grid records (bilanzierungsgebiet, netzgebiet, sparte) to marktd.\n             Improves processd NB STP from ~80% to >=95%.\n\n             Use `dry_run_sync` to preview drift before writing.\n             Use `sync_grid` to apply changes (idempotent, safe to repeat)."
        )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<NisSyncdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    match request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(key) if key == state.marktd_api_key => next.run(request).await,
        Some(_) => (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
        None => (StatusCode::UNAUTHORIZED, "Authorization: Bearer required").into_response(),
    }
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
