//! MCP (Model Context Protocol) server for `portald` — Customer Portal Gateway.
//!
//! Aggregates customer data from all upstream services into a single LLM-accessible
//! read-model. All tools are read-only (`readOnlyHint = true`).
//!
//! ## Tools (8)
//! | Tool | Description |
//! |------|-------------|
//! | `get_dashboard`       | Aggregated customer snapshot (MaLo, invoices, balance, supply status) |
//! | `get_lastgang`        | Energy consumption time-series (Lastgang) for a MaLo |
//! | `get_invoices`        | Billing history (last N invoices) for a MaLo |
//! | `get_balance`         | Open-items account balance from accountingd |
//! | `get_kontoauszug`     | Full account statement (ledger entries) from accountingd |
//! | `get_vorauszahlung`   | Advance payment schedule (Abschlag) from accountingd |
//! | `get_eeg_status`      | EEG/KWKG feed-in plant status and settlement history |
//! | `get_versorgung`      | Supply status (Beliefert/Unbeliefert/Gesperrt) for a MaLo |

use std::sync::Arc;

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
use tokio_util::sync::CancellationToken;

use crate::handlers::PortalClients;

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PortaldMcpState {
    pub clients: Arc<PortalClients>,
    /// MCP authentication. In typical portald deployments MCP is customer-facing
    /// and read-only — set to `McpAuth::dev()` for open access or configure an
    /// API key for token-gated access.
    pub auth: mako_service::mcp_auth::McpAuth,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MaloParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LastgangParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Start of the time range in ISO 8601 format (e.g. 2025-01-01T00:00:00Z).
    pub from: Option<String>,
    /// End of the time range in ISO 8601 format (e.g. 2025-01-31T23:59:59Z).
    pub to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InvoiceListParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Maximum number of invoices to return (default 10, max 50).
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EegParams {
    /// 11-digit Marktlokations-ID of the feed-in point.
    pub malo_id: String,
}

// ── Handler ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PortaldMcpHandler {
    state: Arc<PortaldMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<PortaldMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<PortaldMcpHandler>,
}

#[tool_router]
impl PortaldMcpHandler {
    fn new(state: Arc<PortaldMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "Aggregated customer snapshot: MaLo metadata, latest invoice, account balance, supply status, and active EEG plants. Use this for a quick customer-service overview.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_dashboard(
        &self,
        Parameters(p): Parameters<MaloParams>,
    ) -> Result<CallToolResult, McpError> {
        let malo_id = &p.malo_id;
        let mut result = serde_json::json!({ "malo_id": malo_id });

        // Supply status from marktd — use direct marktd path, NOT portald's own route
        if let Some(ref client) = self.state.clients.marktd
            && let Ok(Some(v)) = client
                .get_json(&format!("/api/v1/versorgung/{malo_id}"))
                .await
        {
            result["versorgung"] = v;
        }
        // Latest invoice from billingd
        if let Some(ref client) = self.state.clients.billingd
            && let Ok(Some(v)) = client
                .get_json(&format!("/api/v1/billing?malo_id={malo_id}&limit=1"))
                .await
        {
            result["latest_invoice"] = v;
        }
        // Balance from accountingd
        if let Some(ref client) = self.state.clients.accountingd
            && let Ok(Some(v)) = client
                .get_json(&format!("/api/v1/accounts/{malo_id}/balance"))
                .await
        {
            result["balance"] = v;
        }

        ContentBlock::json(result)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Energy consumption time-series (Lastgang) for a MaLo. Returns MSCONS-based 15-min or hourly meter readings. Optional from/to ISO 8601 range.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_lastgang(
        &self,
        Parameters(p): Parameters<LastgangParams>,
    ) -> Result<CallToolResult, McpError> {
        let client = self
            .state
            .clients
            .edmd
            .as_ref()
            .ok_or_else(|| McpError::invalid_params("edmd not configured", None))?;
        let mut path = format!("/api/v1/lastgang/{}", p.malo_id);
        let mut qs = vec![];
        if let Some(ref f) = p.from {
            qs.push(format!("from={f}"));
        }
        if let Some(ref t) = p.to {
            qs.push(format!("to={t}"));
        }
        if !qs.is_empty() {
            path = format!("{path}?{}", qs.join("&"));
        }

        match client.get_json(&path).await {
            Ok(Some(v)) => ContentBlock::json(v)
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "no Lastgang data found for {}",
                p.malo_id
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Billing history for a MaLo: list of invoices with status, amount, and period. Returns newest-first up to `limit` (default 10).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_invoices(
        &self,
        Parameters(p): Parameters<InvoiceListParams>,
    ) -> Result<CallToolResult, McpError> {
        let client = self
            .state
            .clients
            .billingd
            .as_ref()
            .ok_or_else(|| McpError::invalid_params("billingd not configured", None))?;
        let limit = p.limit.unwrap_or(10).min(50);
        let path = format!("/api/v1/billing?malo_id={}&limit={limit}", p.malo_id);
        match client.get_json(&path).await {
            Ok(Some(v)) => ContentBlock::json(v)
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => {
                ContentBlock::json(serde_json::json!({ "invoices": [], "malo_id": p.malo_id }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Open-items account balance for a MaLo from accountingd. Returns balance in EUR cents (positive = amount owed, negative = credit).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_balance(
        &self,
        Parameters(p): Parameters<MaloParams>,
    ) -> Result<CallToolResult, McpError> {
        let client = self
            .state
            .clients
            .accountingd
            .as_ref()
            .ok_or_else(|| McpError::invalid_params("accountingd not configured", None))?;
        let path = format!("/api/v1/accounts/{}/balance", p.malo_id);
        match client.get_json(&path).await {
            Ok(Some(v)) => ContentBlock::json(v)
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("account not found for {}", p.malo_id),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Full account statement (Kontoauszug) for a MaLo — all ledger entries: invoices, payments, credits. Use for billing dispute investigation or §666 BGB account transparency. Complements get_balance which returns only the net total.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_kontoauszug(
        &self,
        Parameters(p): Parameters<MaloParams>,
    ) -> Result<CallToolResult, McpError> {
        let client = self
            .state
            .clients
            .accountingd
            .as_ref()
            .ok_or_else(|| McpError::invalid_params("accountingd not configured", None))?;
        let path = format!("/api/v1/accounts/{}/kontoauszug", p.malo_id);
        match client.get_json(&path).await {
            Ok(Some(v)) => ContentBlock::json(v)
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id, "entries": [],
                "note": "No ledger entries found. Account may not yet have invoices.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Advance payment schedule (Vorauszahlung / Abschlag) for a MaLo. Returns the current monthly Abschlag amount, billing cycle, next due date. §40 Abs. 1 EnWG requires LFs to offer monthly advance payment with 3-week adjustment window.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_vorauszahlung(
        &self,
        Parameters(p): Parameters<MaloParams>,
    ) -> Result<CallToolResult, McpError> {
        let client = self
            .state
            .clients
            .accountingd
            .as_ref()
            .ok_or_else(|| McpError::invalid_params("accountingd not configured", None))?;
        let path = format!("/api/v1/accounts/{}/vorauszahlung", p.malo_id);
        match client.get_json(&path).await {
            Ok(Some(v)) => ContentBlock::json(v)
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id,
                "note": "No Vorauszahlung schedule found. Customer may be billed annually.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "EEG/KWKG feed-in plant list and latest settlement data for a MaLo. Returns Förderungsende, settlement model, installed capacity, and last monthly settlement. Use malo_id (not tr_id).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_eeg_status(
        &self,
        Parameters(p): Parameters<EegParams>,
    ) -> Result<CallToolResult, McpError> {
        let client = self
            .state
            .clients
            .einsd
            .as_ref()
            .ok_or_else(|| McpError::invalid_params("einsd not configured", None))?;
        let path = format!("/api/v1/anlagen?malo_id={}", p.malo_id);
        match client.get_json(&path).await {
            Ok(Some(v)) => ContentBlock::json(v)
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id, "plants": [],
                "note": "No EEG/KWKG plants found for this MaLo.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Supply status (Beliefert/Unbeliefert/Gesperrt) for a MaLo. Indicates whether the customer is currently supplied and the effective date.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_versorgung(
        &self,
        Parameters(p): Parameters<MaloParams>,
    ) -> Result<CallToolResult, McpError> {
        let client = self
            .state
            .clients
            .marktd
            .as_ref()
            .ok_or_else(|| McpError::invalid_params("marktd not configured", None))?;
        let path = format!("/api/v1/versorgung/{}", p.malo_id);
        match client.get_json(&path).await {
            Ok(Some(v)) => ContentBlock::json(v)
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id,
                "status": "unknown",
                "hint": "No VersorgungsStatus record found. The MaLo may not yet be registered in marktd.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl PortaldMcpHandler {
    #[prompt(
        name = "customer-overview",
        description = "Step-by-step: get a complete picture of a customer's energy account"
    )]
    async fn customer_overview_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "I need a complete overview of a customer energy account.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `get_dashboard` with the malo_id for an instant aggregated snapshot.\n                 2. For detailed consumption: use `get_lastgang` with a date range.\n                 3. For billing history: use `get_invoices` (newest first).\n                 4. For account balance: use `get_balance` (positive = amount owed, negative = credit).\n                 5. For supply status: use `get_versorgung` to check Beliefert/Gesperrt.\n                 6. For EEG feed-in plants: use `get_eeg_status` with the tr_id from `get_dashboard`.",
            ),
        ]
    }

    #[prompt(
        name = "billing-dispute",
        description = "Step-by-step: help a customer understand or dispute a billing amount"
    )]
    async fn billing_dispute_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "A customer is disputing their latest invoice. How do I investigate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `get_invoices` to list recent invoices and identify the disputed one.\n\
                 2. Use `get_lastgang` for the billing period to verify the consumption data.\n\
                 3. Compare the invoice arbeitsmenge_kwh against the Lastgang total.\n\
                 4. Use `get_kontoauszug` to check for any unmatched credits or prior payments.\n\
                 5. Use `get_balance` to check if the invoice is already overdue (positive balance).\n\
                 6. If the consumption data is wrong: contact the NB to re-send MSCONS readings.\n\
                 7. If the tariff is wrong: check tarifbd `GET /api/v1/customer/{malo_id}/product`.\n\
                 8. For a REMADV dispute: processd will auto-send REMADV 33002 if invoic-checker fails.",
            ),
        ]
    }

    #[prompt(
        name = "eeg-foerderung-check",
        description = "Diagnose EEG/KWKG Förderungsende: what to do when a plant's 20-year support period is ending"
    )]
    async fn eeg_foerderung_check_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "A customer's EEG plant is approaching the end of its Förderungsdauer. What are the options?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**EEG Förderungsende workflow (einsd + portald)**\n\n\
                 1. Call `get_eeg_status(malo_id)` to get the plant details.\n\
                    Key fields: `foerderendedatum`, `eeg_gesetz`, `kw_peak`, `erzeugungsart`.\n\
                 2. Calculate `days_remaining = foerderendedatum - today`.\n\
                 3. Options by plant size:\n\
                    - **<= 100 kWp (small plants)**: POST_EEG_SPOT is typical. \n\
                      Customer receives hourly EPEX spot price. No paperwork required.\n\
                    - **> 100 kWp**: DIREKTVERMARKTUNG (direct marketing) via a DV-partner is \n\
                      often more valuable than spot price.\n\
                    - **REPOWERING (§22 EEG)**: if plant age < 15 years, modernisation extends \n\
                      Förderberechtigung. Requires MaStR re-registration.\n\
                    - **ZUSAMMENLEGUNG (§24 EEG)**: multiple plants at same Netzverknüpfungspunkt \n\
                      can merge for a combined 20-year period.\n\n\
                 4. Regulatory timeline:\n\
                    - 12 months before: send customer notification (§5 Abs. 4 EEG 2023).\n\
                    - 3 months before: new contract must be signed.\n\
                    - 0 days: Förderung ends. Plant switches to POST_EEG automatically.\n\n\
                 5. §51 EEG Negativpreis-Regel: if EPEX spot price is negative for >4h (EEG 2023),\n\
                    POST_EEG_SPOT plants receive €0 for those hours.\n\
                    Inform customers with batteries or flexible loads to shift consumption.\n\n\
                 6. MaStR compliance (§25 EEG 2023): plant must be registered in\n\
                    https://www.marktstammdatenregister.de — check `mastr_registriert` field.\n\
                    Unregistered plants receive 0% of entitled Vergütung (§25 Abs. 1 EEG 2023).",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for PortaldMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("portald", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "portald MCP — Customer Portal Read-Model Gateway (LF role).\n\
             Aggregates data from edmd, billingd, accountingd, einsd, and marktd.\n\
             All tools are read-only — no writes are performed.\n\n\
             ## Tools (8)\n\
             - `get_dashboard` — instant aggregated customer snapshot\n\
             - `get_lastgang` — energy consumption time-series (MSCONS 15-min/hourly)\n\
             - `get_invoices` — billing history, newest first\n\
             - `get_balance` — open-items net balance (positive = owed, negative = credit)\n\
             - `get_kontoauszug` — full account statement (all ledger entries)\n\
             - `get_vorauszahlung` — advance payment schedule (Abschlag, §40 Abs. 1 EnWG)\n\
             - `get_eeg_status` — EEG/KWKG feed-in plants + latest settlements\n\
             - `get_versorgung` — supply status (Beliefert/Unbeliefert/Gesperrt)\n\n\
             ## Prompts (3)\n\
             - `customer-overview` — complete account overview workflow\n\
             - `billing-dispute` — investigate a disputed invoice\n\
             - `eeg-foerderung-check` — EEG Förderungsende options workflow\n\n\
             **Informatorisches Unbundling (§9 EnWG):**\n\
             portald is an LF-role service. Unbundled NB services (netzbilanzd, sperrd, nis-syncd)\n\
             are NOT accessible here. For full O2C cycle: use `billingd` MCP `order-to-cash` prompt.",
        )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────
// portald MCP accepts requests without token verification when no auth key is
// configured. In production, place this behind an API gateway or OIDC proxy.

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<PortaldMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: Arc<PortaldMcpState>, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)))
        .with_cancellation_token(shutdown);

    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(PortaldMcpHandler::new(state.clone()))
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );

    Router::new()
        .route_service("/mcp", mcp_service)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            mcp_auth_middleware,
        ))
}
