//! MCP server for `einsd` — Einspeiser Registry + EEG/KWKG Settlement.
//!
//! Exposes plant registration, settlement queries, and EEG rate lookups via
//! the MCP Streamable HTTP transport (spec 2025-11-25).
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `list_plants` | List EEG/KWKG plants (filterable by malo_id, erzeugungsart, status) |
//! | `get_plant` | Get a single plant by TechnischeRessource ID |
//! | `list_expiring` | Plants with Förderung ending within N days |
//! | `list_settlements` | Settlement history for a plant |
//! | `lookup_verguetungssatz` | Look up the applicable EEG/KWKG tariff rate |
//!
//! ## Resources
//!
//! | URI template | Description |
//! |---|---|
//! | `plant://{tr_id}` | EEG/KWKG plant record |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |---|---|
//! | `register-eeg-plant` | Step-by-step: register a new EEG feed-in plant |
//! | `settle-monthly` | Step-by-step: run monthly EEG/KWKG settlement |
//! | `check-foerderung-expiry` | Step-by-step: identify plants nearing Förderungsende |

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
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

#[derive(Clone)]
pub struct EinsdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPlantsParams {
    /// Filter by Marktlokations-ID (11-digit).
    pub malo_id: Option<String>,
    /// Filter by generator type (e.g. SOLAR, WIND_ONSHORE, KWKG).
    pub erzeugungsart: Option<String>,
    /// Filter by status (aktiv, abgemeldet, foerderung_beendet, repowered).
    pub status: Option<String>,
    /// Maximum results to return (default 50, max 200).
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPlantParams {
    /// TechnischeRessource ID of the plant.
    pub tr_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListExpiringParams {
    /// Horizon in days (default 180).
    pub days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSettlementsParams {
    /// TechnischeRessource ID of the plant.
    pub tr_id: String,
    /// Maximum results to return (default 24).
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LookupParams {
    /// Generator type (SOLAR, WIND_ONSHORE, BIOMASSE, WASSERKRAFT, KWKG, …).
    pub erzeugungsart: String,
    /// Plant capacity in kWp.
    pub leistung_kwp: f64,
    /// Commissioning date in ISO-8601 format (YYYY-MM-DD).
    pub inbetriebnahme: String,
}

#[derive(Clone)]
pub struct EinsdMcpHandler {
    state: Arc<EinsdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<EinsdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<EinsdMcpHandler>,
}

#[tool_router]
impl EinsdMcpHandler {
    fn new(state: Arc<EinsdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List EEG/KWKG plants. Filter by malo_id, erzeugungsart (SOLAR/WIND_ONSHORE/KWKG/…), or status (aktiv/abgemeldet/foerderung_beendet/repowered).")]
    async fn list_plants(
        &self,
        Parameters(params): Parameters<ListPlantsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{AnlagenQuery, list_anlagen};
        let q = AnlagenQuery {
            malo_id: params.malo_id,
            erzeugungsart: params.erzeugungsart,
            status: params.status,
            limit: Some(i64::from(params.limit.unwrap_or(50).min(200))),
        };
        match list_anlagen(&self.state.pool, &self.state.tenant, q).await {
            Ok(plants) => ContentBlock::json(serde_json::to_value(plants).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "Get a single EEG/KWKG plant by its TechnischeRessource ID (tr_id). Returns all plant fields including settlement model, Vergütungssatz, Förderendedatum, and KWKG data.")]
    async fn get_plant(
        &self,
        Parameters(params): Parameters<GetPlantParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_anlage;
        match fetch_anlage(&self.state.pool, &params.tr_id, &self.state.tenant).await {
            Ok(Some(plant)) => ContentBlock::json(serde_json::to_value(plant).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("plant {} not found", params.tr_id),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "List plants whose EEG/KWKG Förderung ends within the specified number of days (default 180). Used to trigger early notification to Anlagenbetreiber and plan Post-EEG transitions.")]
    async fn list_expiring(
        &self,
        Parameters(params): Parameters<ListExpiringParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_expiring;
        let days = i64::try_from(params.days.unwrap_or(180)).unwrap_or(180);
        match list_expiring(&self.state.pool, &self.state.tenant, days).await {
            Ok(plants) => ContentBlock::json(serde_json::json!({
                "horizon_days": days,
                "count": plants.len(),
                "plants": plants,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "Get the monthly settlement history for a plant. Returns settlement amount, model, kWh, and CloudEvent ID for each settled month.")]
    async fn list_settlements(
        &self,
        Parameters(params): Parameters<ListSettlementsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_settlement_receipts;
        let limit = i16::try_from(params.limit.unwrap_or(24).min(200)).unwrap_or(24);
        match list_settlement_receipts(&self.state.pool, &self.state.tenant, &params.tr_id, limit).await {
            Ok(receipts) => ContentBlock::json(serde_json::to_value(receipts).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "Look up the applicable EEG or KWKG Vergütungssatz (tariff rate in ct/kWh) for a plant commissioning date and capacity. Returns the fixed tariff rate applicable for the full 20-year Förderdauer.")]
    async fn lookup_verguetungssatz(
        &self,
        Parameters(params): Parameters<LookupParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::lookup_verguetungssatz;
        use rust_decimal::Decimal;
        let kwp = Decimal::try_from(params.leistung_kwp)
            .map_err(|_| McpError::invalid_params("invalid leistung_kwp", None))?;
        match lookup_verguetungssatz(
            &self.state.pool,
            &params.erzeugungsart,
            kwp,
            &params.inbetriebnahme,
        )
        .await
        {
            Ok(rate) => ContentBlock::json(serde_json::json!({
                "erzeugungsart": params.erzeugungsart,
                "leistung_kwp": params.leistung_kwp,
                "inbetriebnahme": params.inbetriebnahme,
                "verguetungssatz_ct_kwh": rate,
                "foerderendedatum_approx": format!("~20 years from {}", params.inbetriebnahme),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl EinsdMcpHandler {
    #[prompt(
        name = "register-eeg-plant",
        description = "Step-by-step: register a new EEG/KWKG feed-in plant"
    )]
    async fn register_eeg_plant_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "I need to register a new EEG feed-in plant."),
            PromptMessage::new_text(Role::Assistant, 
                "1. POST /api/v1/anlagen with:\n                 - tr_id (TechnischeRessource ID from marktd)\n                 - erzeugungsart: SOLAR_AUFDACH | SOLAR_FREIFLÄCHE | WIND_ONSHORE | BIOMASSE | etc.\n                 - installierte_leistung_kw, inbetriebnahme (YYYY-MM-DD), plz, bundesland\n                 - settlement_model: VERGUETUNG | DIREKTVERMARKTUNG | KWKG_ZUSCHLAG | etc.\n\n                 2. einsd auto-calculates:\n                 - foerderendedatum = inbetriebnahme + 20 years (EEG §22)\n                 - Vergütungssatz from the built-in EEG/KWKG rate table\n\n                 3. The 180-day expiry alert fires when foerderendedatum is approaching.\n                 Use `get_eeg_plant` to verify the registration.",
            ),
        ]
    }

    #[prompt(
        name = "settle-monthly",
        description = "Step-by-step: run monthly EEG/KWKG settlement for a plant"
    )]
    async fn settle_monthly_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "Run monthly settlement for an EEG/KWKG plant."),
            PromptMessage::new_text(Role::Assistant, 
                "1. POST /api/v1/anlagen/{tr_id}/settle with billing_month (YYYY-MM).\n                 2. einsd fetches Lastgang from edmd for the month.\n                 3. Calculates Vergütung or Marktprämie based on settlement_model.\n                 4. Emits de.eeg.verguetung.berechnet CloudEvent → accountingd posts credit.\n\n                 DIREKTVERMARKTUNG plants receive Marktprämie = market_value_ref - epex_spot_avg.\n                 KWKG_ZUSCHLAG: Förderdauer tracked in hours; Zuschlag stops when limit reached.",
            ),
        ]
    }

    #[prompt(
        name = "check-foerderung-expiry",
        description = "Step-by-step: identify plants nearing Förderungsende and plan transition"
    )]
    async fn check_foerderung_expiry_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "Which plants are approaching their Förderungsende?"),
            PromptMessage::new_text(Role::Assistant, 
                "GET /api/v1/anlagen?expiring_within_days=180 returns plants with foerderendedatum within 6 months.\n\n                 Transition options after §22 EEG Förderungsende:\n                 - POST_EEG_SPOT: feed-in at spot market price (no subsidy)\n                 - EIGENVERBRAUCH: self-consumption (register with NB)\n                 - DIREKTVERMARKTUNG: direct marketing via Bilanzkreis\n                 - REPOWERING: PUT /api/v1/anlagen/{tr_id}/repowering resets foerderendedatum +20yr\n                 - ZUSAMMENLEGUNG §24: multiple plants → single Bilanzkreis, POST /api/v1/anlagen/{tr_id}/zusammenlegen",
            ),
        ]
    }
}


#[tool_handler]
#[prompt_handler]
impl ServerHandler for EinsdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("einsd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "einsd MCP — Einspeiser Registry + EEG/KWKG Settlement daemon.\n\
             8 settlement models: VERGUETUNG (§21 EEG), MIETERSTROM (§38a), DIREKTVERMARKTUNG (§20 Marktprämie),\n\
             AUSSCHREIBUNG, POST_EEG_SPOT, EIGENVERBRAUCH, KWKG_ZUSCHLAG (§7 KWKG 2023), FLEXIBILITAET (§50 EEG).\n\n\
             Use `list_plants` to survey the plant register.\n\
             Use `list_expiring` to find plants approaching their Förderungsende (§22 MessZV obligation).\n\
             Use `lookup_verguetungssatz` to determine applicable EEG/KWKG tariff rate before registering.\n\
             Use `list_settlements` to audit monthly settlement history.\n\
             Use the `register-eeg-plant` prompt for a guided registration workflow.",
        )
    }

    async fn list_resource_templates(
        &self,
        _request: ListResourceTemplatesRequest,
        _: RequestContext<Self>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![ResourceTemplate {
                uri_template: "plant://{tr_id}".to_owned(),
                name: "EEG/KWKG Plant".to_owned(),
                description: Some("Einspeisanlage master record (all fields)".to_owned()),
                mime_type: Some("application/json".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequest,
        _: RequestContext<Self>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = &request.params.uri;
        if let Some(tr_id) = uri.strip_prefix("plant://") {
            use crate::pg::fetch_anlage;
            match fetch_anlage(&self.state.pool, tr_id, &self.state.tenant).await {
                Ok(Some(p)) => {
                    let json = serde_json::to_string_pretty(&p).unwrap_or_default();
                    Ok(ReadResourceResult {
                        contents: vec![ResourceContents::text(json, uri.clone())],
                        ..Default::default()
                    })
                }
                Ok(None) => Err(McpError::resource_not_found(format!("plant {tr_id} not found"), None)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        } else {
            Err(McpError::resource_not_found(format!("unknown URI: {uri}"), None))
        }
    }

}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<EinsdMcpState>>,
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
        None => return (StatusCode::UNAUTHORIZED, "Authorization: Bearer required").into_response(),
    };
    if state.oidc.verify(&token).is_err() {
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }
    next.run(request).await
}

pub fn router(state: Arc<EinsdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = EinsdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
