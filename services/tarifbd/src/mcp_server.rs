//! MCP server for `tarifbd` — Product & Tariff Catalog (LF role).
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `list_products` | List products for an LF MP-ID |
//! | `get_product` | Get a single product with full Tarifpreisblatt JSONB |
//! | `get_customer_product` | Look up the active product for a MaLo |
//! | `get_epex_price` | Get EPEX day-ahead hourly prices for a date |
//! | `list_expiring_contracts` | Contracts ending within N days (churn prevention) |
//! | `list_angebote` | List B2B quotations (Angebote) — filter by status |
//! | `get_angebot` | Fetch a single Angebot with enriched positions and variants |
//! | `get_angebot_summary` | Summarise an Angebot in plain text for sales staff |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |---|---|
//! | `configure-41a-tariff` | Step-by-step: configure a §41a EPEX dynamic tariff product |
//! | `assign-product` | Step-by-step: assign a tariff product to a MaLo |
//! | `create-b2b-quotation` | Step-by-step: create a formal B2B Angebot for a C&I customer |

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

#[derive(Clone)]
pub struct TarifbdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

// ── Parameter types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListProductsParams {
    /// LF MP-ID (BDEW-Codenummer, 13 digits).
    pub lf_mp_id: String,
    /// Filter by category: STROM|GAS|WAERME|SOLAR|EEG|EINSPEISUNG|WAERMEPUMPE|WALLBOX|HEMS|EMOBILITY|ENERGIEDIENSTLEISTUNG|BUNDLE.
    pub category: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetProductParams {
    pub lf_mp_id: String,
    pub product_code: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CustomerProductParams {
    pub malo_id: String,
    pub lf_mp_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EpexPriceParams {
    /// Date in ISO-8601 format (YYYY-MM-DD).
    pub date: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExpiringContractsParams {
    /// LF MP-ID to filter contracts for.
    pub lf_mp_id: String,
    /// Days until expiry threshold (default 60 — show contracts ending within 2 months).
    pub days_ahead: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListAngeboteParams {
    /// Filter by status: ANGELEGT|VERSANDT|ANGENOMMEN|ABGELEHNT|ABGELAUFEN.  Omit for all open.
    pub status: Option<String>,
    /// Max results (default 20, max 100).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetAngebotParams {
    /// UUID of the Angebot.
    pub id: String,
}

// ── MCP handler ────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TarifbdMcpHandler {
    state: Arc<TarifbdMcpState>,
    #[allow(dead_code)] tool_router: ToolRouter<TarifbdMcpHandler>,
    #[allow(dead_code)] prompt_router: PromptRouter<TarifbdMcpHandler>,
}

#[tool_router]
impl TarifbdMcpHandler {
    fn new(state: Arc<TarifbdMcpState>) -> Self {
        Self { state, tool_router: Self::tool_router(), prompt_router: Self::prompt_router() }
    }

    // ── Product catalog ───────────────────────────────────────────────────────

    #[tool(description = "List products for an LF MP-ID. Filter by category (STROM/GAS/WAERME/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/BUNDLE). Returns product summaries including name, category, and validity.",
        annotations(read_only_hint = true, open_world_hint = false))]
    async fn list_products(
        &self,
        Parameters(p): Parameters<ListProductsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{ProductListQuery, list_products};
        let q = ProductListQuery {
            category: p.category,
            sparte: None,
            kundentyp: None,
            limit: Some(p.limit.unwrap_or(50).min(100)),
        };
        match list_products(&self.state.pool, &p.lf_mp_id, &q).await {
            Ok(products) => ContentBlock::json(serde_json::json!({
                "lf_mp_id": p.lf_mp_id,
                "count": products.len(),
                "products": products,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "Get a single product by LF MP-ID and product code. Returns the full Tarifpreisblatt or Preisblatt JSONB including all Preisstaffeln, ZusatzAttribute, and Energiemix if set.",
        annotations(read_only_hint = true, open_world_hint = false))]
    async fn get_product(
        &self,
        Parameters(p): Parameters<GetProductParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_product;
        match fetch_product(&self.state.pool, &p.lf_mp_id, &p.product_code).await {
            Ok(Some(product)) => ContentBlock::json(serde_json::to_value(product).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("product {}/{} not found", p.lf_mp_id, p.product_code), None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "Look up the currently active product assignment for a MaLo (delivery point). Returns product code, category, and supply period. Use this to verify a MaLo is billed under the correct tariff.",
        annotations(read_only_hint = true, open_world_hint = false))]
    async fn get_customer_product(
        &self,
        Parameters(p): Parameters<CustomerProductParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::get_customer_product;
        match get_customer_product(&self.state.pool, &p.malo_id, &p.lf_mp_id).await {
            Ok(Some(assignment)) => ContentBlock::json(serde_json::to_value(assignment).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id,
                "lf_mp_id": p.lf_mp_id,
                "product": null,
                "hint": "No active product assignment. Use PUT /api/v1/customer/{malo_id}/product to assign one.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "Get EPEX Spot day-ahead hourly prices for a specific date. Returns up to 24 hourly entries in ct/kWh. Used for §41a dynamic tariff billing verification.",
        annotations(read_only_hint = true, open_world_hint = false))]
    async fn get_epex_price(
        &self,
        Parameters(p): Parameters<EpexPriceParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_epex_day;
        let Ok(date) = time::Date::parse(&p.date, &time::format_description::well_known::Iso8601::DEFAULT) else {
            return Err(McpError::invalid_params("date must be YYYY-MM-DD", None));
        };
        match fetch_epex_day(&self.state.pool, date).await {
            Ok(Some(prices)) => ContentBlock::json(serde_json::json!({
                "date": p.date,
                "hours_available": prices.len(),
                "prices_ct_kwh": prices,
                "note": "Prices in ct/kWh. 24 entries expected for a complete day.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => ContentBlock::json(serde_json::json!({
                "date": p.date,
                "hours_available": 0,
                "prices_ct_kwh": [],
                "note": "No EPEX prices imported for this date. Use PUT /api/v1/epex-prices/{date} to import.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List customer supply contracts (Liefervertrage) ending within N days. \
Essential for churn prevention and proactive renewal campaigns. \
Returns malo_id, product_code, assigned_from, assigned_to for each expiring assignment.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_expiring_contracts(
        &self,
        Parameters(p): Parameters<ExpiringContractsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_expiring_assignments;
        let days = p.days_ahead.unwrap_or(60);
        match list_expiring_assignments(&self.state.pool, &p.lf_mp_id, days).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "lf_mp_id": p.lf_mp_id,
                "days_ahead": days,
                "expiring_count": rows.len(),
                "contracts": rows,
                "note": "assigned_to = Liefervertragsende. Null = open-ended supply. Renew via PUT /api/v1/customer/{malo_id}/product.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    // ── Angebot (B2B Quotation, B4) ───────────────────────────────────────────

    #[tool(
        description = "List B2B Angebote (formal quotations for C&I/RLM customers). \
Filter by status: ANGELEGT (draft), VERSANDT (sent), ANGENOMMEN (accepted), ABGELEHNT (declined), ABGELAUFEN (expired). \
Omit status to see all open quotations (ANGELEGT + VERSANDT). \
C&I/RLM customers are 5-50x the revenue of SLP households.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_angebote(
        &self,
        Parameters(p): Parameters<ListAngeboteParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_angebote;
        let limit = p.limit.unwrap_or(20).min(100);
        match list_angebote(&self.state.pool, &self.state.tenant, &self.state.tenant, p.status.as_deref(), limit).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "count": rows.len(),
                "status_filter": p.status,
                "angebote": rows,
                "note": "Use get_angebot to see full position details. Accept via POST /api/v1/angebote/{id}/annehmen.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Fetch a single Angebot (B2B quotation) by UUID. \
Returns the full Angebot with enriched Positionen (per-commodity pricing with NNE, levies, Jahreskosten), \
Varianten (alternative scenarios), and lifecycle state (status, gueltig_bis, accepted_at). \
Essential for reviewing a quotation before sending to a C&I customer.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_angebot(
        &self,
        Parameters(p): Parameters<GetAngebotParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_angebot;
        let id: uuid::Uuid = p.id.parse().map_err(|_| McpError::invalid_params("id must be a valid UUID", None))?;
        match fetch_angebot(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(a)) => ContentBlock::json(serde_json::to_value(a).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(format!("Angebot {id} not found"), None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Summarise an Angebot (B2B quotation) for sales staff review. \
Returns a concise plain-text summary: customer, products, Jahreskosten (netto/brutto), \
Varianten comparison table, validity window, and next-action instructions. \
Use before sending an Angebot to a C&I customer to verify correctness.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_angebot_summary(
        &self,
        Parameters(p): Parameters<GetAngebotParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_angebot;
        let id: uuid::Uuid = p.id.parse().map_err(|_| McpError::invalid_params("id must be a valid UUID", None))?;
        let a = match fetch_angebot(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(a)) => a,
            Ok(None) => return Err(McpError::invalid_params(format!("Angebot {id} not found"), None)),
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };

        let customer = a.interessent_name
            .as_deref()
            .unwrap_or_else(|| a.kunden_id.map(|_| "existing customer").unwrap_or("unknown"));

        let pos_count = a.positionen.as_array().map(|v| v.len()).unwrap_or(0);
        let var_count = a.varianten.as_array().map(|v| v.len()).unwrap_or(0);

        let summary = format!(
            "Angebot {nr} ({status})\n\
             Customer: {customer}\n\
             Products: {pos_count} position(s)\n\
             Jahreskosten netto: {netto} EUR\n\
             Jahreskosten brutto: {brutto} EUR (19% MwSt)\n\
             Variants: {var_count}\n\
             Valid until: {gueltig}\n\
             Lieferbeginn: {lb}\n\
             Laufzeit: {laufzeit} Monate\n\
             ---\n\
             Next actions:\n\
             - Review: GET /api/v1/angebote/{id}\n\
             - Send: POST /api/v1/angebote/{id}/versenden\n\
             - Accept: POST /api/v1/angebote/{id}/annehmen  {{ gewaehlte_variante: 0 }}\n\
             - Decline: POST /api/v1/angebote/{id}/ablehnen",
            nr = a.angebotsnummer,
            status = a.status,
            netto = a.jahreskosten_netto_eur.map(|d| d.to_string()).unwrap_or_else(|| "—".to_owned()),
            brutto = a.jahreskosten_brutto_eur.map(|d| d.to_string()).unwrap_or_else(|| "—".to_owned()),
            gueltig = a.gueltig_bis,
            lb = a.lieferbeginn.map(|d| d.to_string()).unwrap_or_else(|| "TBD".to_owned()),
            laufzeit = a.laufzeit_monate,
            id = a.id,
        );

        ContentBlock::text(summary)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }
}

// ── Prompts ────────────────────────────────────────────────────────────────────

#[prompt_router]
impl TarifbdMcpHandler {
    #[prompt(
        name = "configure-41a-tariff",
        description = "Step-by-step: configure a §41a EPEX dynamic tariff product for iMSys customers"
    )]
    async fn configure_41a_tariff_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I configure a §41a EPEX dynamic tariff for iMSys customers?"),
            PromptMessage::new_text(Role::Assistant,
                "§41a EnWG requires all LFs to offer dynamic tariffs to iMSys customers (mandatory since Jan 2025).\n\n\
                 Steps:\n\n\
                 1. Create the product in tarifbd:\n\
                    PUT /api/v1/products/{lf_mp_id}/STROM-EPEX-01\n\
                    {\n\
                      \"category\": \"STROM\",\n\
                      \"name\": \"Dynamischer Stromtarif §41a\",\n\
                      \"data\": {\n\
                        \"dynamic_epex\": true,\n\
                        \"grundpreis_ct_per_day\": \"5.0\",\n\
                        \"tarifpreispositionen\": [{ \"preistyp\": \"GRUNDPREIS\", ... }]\n\
                      }\n\
                    }\n\n\
                 2. Import EPEX D-1 prices daily (cron at 13:00 CET after EPEX publication):\n\
                    PUT /api/v1/epex-prices/YYYY-MM-DD\n\
                    { \"prices\": [ct_h0, ct_h1, ..., ct_h23] }  -- 24 values\n\n\
                 3. Assign to iMSys-eligible MaLos:\n\
                    PUT /api/v1/customer/{malo_id}/product\n\
                    { \"product_code\": \"STROM-EPEX-01\", \"assigned_from\": \"YYYY-MM-DD\" }\n\n\
                 4. billingd auto-detects dynamic_epex=true:\n\
                    - Fetches 15-min Lastgang from edmd\n\
                    - Joins each interval against the EPEX hourly price for that hour\n\
                    - NNE from marktd (PreisblattNetznutzung) added as pass-through\n\n\
                 5. Verify: use get_epex_price to check prices are imported for upcoming dates.",
            ),
        ]
    }

    #[prompt(
        name = "assign-product",
        description = "Step-by-step: assign a tariff product to a MaLo (delivery point)"
    )]
    async fn assign_product_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I assign a tariff product to a MaLo?"),
            PromptMessage::new_text(Role::Assistant,
                "To assign or change the tariff product for a delivery point:\n\n\
                 PUT /api/v1/customer/{malo_id}/product\n\
                 {\n\
                   \"product_code\": \"STROM-SLP-01\",  -- product from list_products\n\
                   \"assigned_from\": \"2026-01-01\",    -- effective date\n\
                   \"assigned_to\":   null               -- null = open-ended\n\
                 }\n\n\
                 This closes any previous open-ended assignment at assigned_from.\n\
                 billingd uses the product valid at period_from for each billing run.\n\n\
                 Verify with: get_customer_product { malo_id, lf_mp_id }",
            ),
        ]
    }

    #[prompt(
        name = "create-b2b-quotation",
        description = "Step-by-step: create a formal B2B Angebot (quotation) for a C&I or RLM customer"
    )]
    async fn create_b2b_quotation_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I create a formal B2B price quotation for a C&I customer?"),
            PromptMessage::new_text(Role::Assistant,
                "The B2B Angebot workflow (B4) — step by step:\n\n\
                 ## 1. Create the Angebot (draft)\n\
                 POST /api/v1/angebote\n\
                 {\n\
                   \"kunden_id\": \"<UUID from vertragd>\",   -- or use interessent_name for prospects\n\
                   \"gueltig_bis\": \"2026-08-25\",            -- 10 Werktage default if omitted\n\
                   \"lieferbeginn\": \"2027-01-01\",\n\
                   \"laufzeit_monate\": 24,\n\
                   \"positionen\": [{\n\
                     \"product_code\": \"STROM-RLM-2027\",\n\
                     \"sparte\": \"STROM\",\n\
                     \"jahresverbrauch_kwh\": \"1500000\",       -- 1500 MWh/year\n\
                     \"leistung_kw\": \"500\",                   -- for Leistungspreis\n\
                     \"nne_arbeitspreis_ct_per_kwh\": \"1.20\",  -- from marktd /preisblaetter\n\
                     \"nne_grundpreis_eur_per_year\": \"2400\",\n\
                     \"ka_ct_per_kwh\": \"0.11\"\n\
                   }],\n\
                   \"varianten\": [\n\
                     { \"label\": \"12 Monate Festpreis\", \"laufzeit_monate\": 12, \"rabatt_pct\": null },\n\
                     { \"label\": \"24 Monate mit 3% Treuerabatt\", \"laufzeit_monate\": 24, \"rabatt_pct\": \"3.0\" }\n\
                   ]\n\
                 }\n\n\
                 Returns: { id, angebotsnummer, jahreskosten_netto_eur, jahreskosten_brutto_eur }\n\n\
                 ## 2. Review the quotation\n\
                 get_angebot_summary { id }\n\
                 -- verify pricing, NNE pass-through, Varianten comparison\n\n\
                 ## 3. Send to customer\n\
                 POST /api/v1/angebote/{id}/versenden\n\
                 -- transitions ANGELEGT → VERSANDT\n\
                 -- also available: PUT /api/v1/angebote/{id} to update pricing before sending\n\n\
                 ## 4. Customer accepts (digital acceptance)\n\
                 POST /api/v1/angebote/{id}/annehmen\n\
                 { \"gewaehlte_variante\": 1 }  -- index into varianten array (0 = base offer)\n\
                 -- Emits de.angebot.angenommen CloudEvent → ERP/vertragd creates Rahmenvertrag\n\n\
                 ## 5. Contract creation (automated via ERP webhook)\n\
                 -- ERP receives de.angebot.angenommen with positionen + chosen variant\n\
                 -- Creates Rahmenvertrag + N x Versorgungsvertrag in vertragd\n\
                 -- Returns rahmenvertrag_id → tarifbd links the Angebot\n\n\
                 ## Key facts\n\
                 - Angebot expires automatically after gueltig_bis (background worker)\n\
                 - jahreskosten includes NNE + KA if supplied in positionen\n\
                 - Varianten side-by-side comparison: different laufzeit/rabatt/products\n\
                 - §41 EnWG: customer is bound from annehmen; no cooling-off for B2B\n\
                 - NNE data source: marktd GET /api/v1/preisblaetter/{nb_mp_id}",
            ),
        ]
    }
}

// ── ServerHandler ─────────────────────────────────────────────────────────────

#[tool_handler]
#[prompt_handler]
impl ServerHandler for TarifbdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder().enable_tools().enable_prompts().build(),
        )
        .with_server_info(Implementation::new("tarifbd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "tarifbd MCP -- Product & Tariff Catalog (LF role).\n\
             Single source of truth for retail products the LF sells to end customers.\n\
             Categories: STROM/GAS/WAERME/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/BUNDLE.\n\
             Also manages EPEX Spot day-ahead prices for §41a dynamic tariffs (iMSys, mandatory since Jan 2025).\n\
             B2B Angebote (formal quotations) for C&I/RLM customers: lifecycle ANGELEGT→VERSANDT→ANGENOMMEN/ABGELEHNT/ABGELAUFEN.\n\n\
             Key tools:\n\
             - list_products: survey the tariff catalog\n\
             - get_customer_product: check which tariff a MaLo is currently billed under\n\
             - get_epex_price: verify D-1 EPEX prices are imported\n\
             - list_angebote: see open B2B quotations\n\
             - get_angebot_summary: human-readable quotation summary for sales review\n\n\
             Role: LF only. NB network tariffs (PreisblattNetznutzung) are in marktd.",
        )
    }
}

// ── Auth middleware + router ──────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<TarifbdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    match request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(t) if state.oidc.verify(t).is_ok() => next.run(request).await,
        Some(_) => (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
        None => (StatusCode::UNAUTHORIZED, "Authorization: Bearer required").into_response(),
    }
}

pub fn router(state: Arc<TarifbdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = TarifbdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}

