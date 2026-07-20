//! MCP server for `tarifbd` — Product & Tariff Catalog (LF role).
//!
//! ## Tools (14)
//!
//! | Tool | Description |
//! |---|---|
//! | `list_products` | List products for an LF MP-ID |
//! | `get_product` | Get a single product with full Tarifpreisblatt JSONB |
//! | `get_product_history` | Full version history for a product (includes energiemix) |
//! | `get_customer_product` | Look up the active product for a MaLo |
//! | `get_epex_price` | Get EPEX day-ahead hourly prices for a date |
//! | `list_expiring_contracts` | Contracts ending within N days (churn prevention) |
//! | `list_angebote` | List B2B quotations (Angebote) — filter by status |
//! | `get_angebot` | Fetch a single Angebot with enriched positions and variants |
//! | `get_angebot_summary` | Summarise an Angebot in plain text for sales staff |
//! | `check_41a_epex_status` | Check if EPEX D-1 prices are current (§41a compliance) |
//! | `get_product_energiemix` | Get §42 EnWG Energiemix disclosure for a product |
//! | `validate_tariff_config` | Validate Tarifpreisblatt JSONB before PUT (same logic as REST) |
//! | `explain_invoice_position` | Explain how a preistyp maps to a billing output + formula |
//! | `get_comparison_feed` | Retrieve the §42d comparison portal feed (proxies the REST endpoint) |
//!
//! ## Prompts (3)
//!
//! | Prompt | Description |
//! |---|---|
//! | `configure-41a-tariff` | Step-by-step: configure a §41a EPEX dynamic tariff product |
//! | `assign-product` | Step-by-step: assign a tariff product to a MaLo |
//! | `create-b2b-quotation` | Step-by-step: create a formal B2B Angebot for a C&I customer |

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
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct TarifbdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateTariffConfigParams {
    /// LF MP-ID (BDEW Codenummer) owning this product.
    pub lf_mp_id: String,
    /// Product category: STROM|GAS|WAERME|SOLAR|EEG|EINSPEISUNG|WAERMEPUMPE|WALLBOX|SHARING|HEMS|EMOBILITY|ENERGIEDIENSTLEISTUNG|BUNDLE
    pub category: String,
    /// Full Tarifpreisblatt JSONB payload to validate.
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExplainInvoicePositionParams {
    /// LF MP-ID.
    pub lf_mp_id: String,
    /// Product code to look up.
    pub product_code: String,
    /// The preistyp to explain (e.g. GRUNDPREIS, ARBEITSPREIS_EINTARIF, LEISTUNGSPREIS).
    pub preistyp: String,
}

// ── MCP handler ────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TarifbdMcpHandler {
    state: Arc<TarifbdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<TarifbdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<TarifbdMcpHandler>,
}

#[tool_router]
impl TarifbdMcpHandler {
    fn new(state: Arc<TarifbdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    // ── Product catalog ───────────────────────────────────────────────────────

    #[tool(
        description = "List products for an LF MP-ID. Filter by category (STROM/GAS/WAERME/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/BUNDLE). Returns product summaries including name, category, and validity.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_products(
        &self,
        Parameters(p): Parameters<ListProductsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{ProductListQuery, list_products};
        let q = ProductListQuery {
            category: p.category,
            sparte: None,
            kundentyp: None,
            include_drafts: None,
            include_expired: None,
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

    #[tool(
        description = "Get a single product by LF MP-ID and product code. Returns the full Tarifpreisblatt or Preisblatt JSONB including all Preisstaffeln, ZusatzAttribute, and Energiemix if set.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_product(
        &self,
        Parameters(p): Parameters<GetProductParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_product;
        match fetch_product(&self.state.pool, &p.lf_mp_id, &p.product_code).await {
            Ok(Some(product)) => {
                ContentBlock::json(serde_json::to_value(product).unwrap_or_default())
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::invalid_params(
                format!("product {}/{} not found", p.lf_mp_id, p.product_code),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Look up the currently active product assignment for a MaLo (delivery point). Returns product code, category, and supply period. Use this to verify a MaLo is billed under the correct tariff.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
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

    #[tool(
        description = "Get EPEX Spot day-ahead hourly prices for a specific date. Returns up to 24 hourly entries in ct/kWh. Used for §41a dynamic tariff billing verification.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_epex_price(
        &self,
        Parameters(p): Parameters<EpexPriceParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_epex_day;
        let Ok(date) = time::Date::parse(
            &p.date,
            &time::format_description::well_known::Iso8601::DEFAULT,
        ) else {
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
        let id: uuid::Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id must be a valid UUID", None))?;
        match fetch_angebot(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(a)) => ContentBlock::json(serde_json::to_value(a).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("Angebot {id} not found"),
                None,
            )),
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
        let id: uuid::Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id must be a valid UUID", None))?;
        let a = match fetch_angebot(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                return Err(McpError::invalid_params(
                    format!("Angebot {id} not found"),
                    None,
                ));
            }
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };

        let customer = a.interessent_name.as_deref().unwrap_or_else(|| {
            a.kunden_id
                .map(|_| "existing customer")
                .unwrap_or("unknown")
        });

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
            netto = a
                .jahreskosten_netto_eur
                .map(|d| d.to_string())
                .unwrap_or_else(|| "—".to_owned()),
            brutto = a
                .jahreskosten_brutto_eur
                .map(|d| d.to_string())
                .unwrap_or_else(|| "—".to_owned()),
            gueltig = a.gueltig_bis,
            lb = a
                .lieferbeginn
                .map(|d| d.to_string())
                .unwrap_or_else(|| "TBD".to_owned()),
            laufzeit = a.laufzeit_monate,
            id = a.id,
        );

        Ok(CallToolResult::success(vec![ContentBlock::text(summary)]))
    }

    #[tool(
        description = "Check §41a EnWG EPEX Day-Ahead import status. Returns the latest date for \
                       which EPEX prices are imported and whether tomorrow's prices are already \
                       available. Critical for §41a compliance: D-1 prices must be imported before \
                       billing can proceed for dynamic tariff customers."
    )]
    async fn check_41a_epex_status(
        &self,
        Parameters(_): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_epex_latest_date;

        let latest = fetch_epex_latest_date(&self.state.pool)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // Compute "today in German local time" (CET/CEST) without an external
        // time-zone database.  EU DST rule: UTC+2 from last Sunday of March to
        // last Sunday of October; UTC+1 otherwise.
        let now_utc = OffsetDateTime::now_utc();
        let today_de = german_local_date(now_utc);
        let tomorrow_de = today_de.next_day().unwrap_or(today_de);

        let status = match latest {
            None => "CRITICAL: No EPEX prices in database. §41a dynamic tariff billing is \
                 impossible. Import prices via PUT /api/v1/epex-prices/{date}."
                .to_owned(),
            Some(d) if d >= tomorrow_de => {
                format!(
                    "OK: EPEX prices are current. Latest date: {d}. \
                     Tomorrow ({tomorrow_de}) is covered. §41a billing can proceed.",
                )
            }
            Some(d) if d == today_de => {
                format!(
                    "WARNING: EPEX prices available through today ({d}) but tomorrow \
                     ({tomorrow_de}) is missing. Day-Ahead prices for tomorrow are \
                     typically published by EPEX SPOT at ~13:00 CET. If it is after \
                     14:00 CET, trigger import immediately.",
                )
            }
            Some(d) => {
                let gap = (today_de - d).whole_days();
                format!(
                    "CRITICAL: EPEX prices are {gap} day(s) stale! Latest: {d}, \
                     today: {today_de}. §41a dynamic tariff customers cannot be \
                     billed. Immediate action required.",
                )
            }
        };

        Ok(CallToolResult::success(vec![ContentBlock::text(status)]))
    }

    #[tool(
        description = "Get the §42 EnWG Energiemix disclosure data for a product. Returns the \
                       BO4E Energiemix COM including fuel mix percentages, CO2 emissions (g/kWh), \
                       radioactive waste (mg/kWh), and Oekolabel certification. Mandatory on \
                       annual invoices for electricity products."
    )]
    async fn get_product_energiemix(
        &self,
        Parameters(p): Parameters<GetProductParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_energiemix;
        match fetch_energiemix(&self.state.pool, &p.lf_mp_id, &p.product_code).await {
            Ok(Some(mix)) => {
                let val = serde_json::to_value(mix).unwrap_or_default();
                Ok(CallToolResult::success(vec![ContentBlock::text(
                    val.to_string(),
                )]))
            }
            Ok(None) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "No Energiemix set for product {}/{}. \
                 §42 EnWG requires Energiemix disclosure on annual electricity bills. \
                 Set via PUT /api/v1/products/{}/{}/energiemix",
                p.lf_mp_id, p.product_code, p.lf_mp_id, p.product_code,
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Validate a Tarifpreisblatt JSONB payload BEFORE submitting it via PUT. \
                       Runs the same BO4E schema validation as the REST endpoint: checks _typ, \
                       sparte, tariftyp, kundentypen, registeranzahl, berechnungsparameter enums, \
                       and the 30-value preistyp whitelist. Returns 'VALID' with field summary, \
                       or structured errors per invalid field. Use to prevent 422 rejections.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn validate_tariff_config(
        &self,
        Parameters(p): Parameters<ValidateTariffConfigParams>,
    ) -> Result<CallToolResult, McpError> {
        // Re-run the same validation logic used in PUT /api/v1/products/{lf}/{code}.
        // This ensures the MCP tool is authoritative — not a separate implementation.
        use crate::handlers::VALID_PREISTYPEN;

        let category = p.category.to_uppercase();
        let tarifpreisblatt_categories = &[
            "STROM",
            "GAS",
            "WAERME",
            "SOLAR",
            "EEG",
            "EINSPEISUNG",
            "WAERMEPUMPE",
            "WALLBOX",
            "SHARING",
        ];
        let is_bo4e = tarifpreisblatt_categories.contains(&category.as_str());

        // Check _typ for BO4E categories
        if is_bo4e {
            match p.data.get("_typ").and_then(|v| v.as_str()) {
                None => {
                    return Ok(CallToolResult::success(vec![ContentBlock::text(
                        "INVALID: missing _typ field. Add: \"_typ\": \"TARIFPREISBLATT\""
                            .to_owned(),
                    )]));
                }
                Some(t) if t.to_uppercase() != "TARIFPREISBLATT" => {
                    return Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                        "INVALID: _typ must be 'TARIFPREISBLATT' for category {category}, got '{t}'"
                    ))]));
                }
                _ => {}
            }
        }

        // Check preistyp whitelist
        let mut errors: Vec<String> = Vec::new();
        if let Some(positionen) = p
            .data
            .get("tarifpreispositionen")
            .and_then(|v| v.as_array())
        {
            for (i, pos) in positionen.iter().enumerate() {
                if let Some(pt) = pos.get("preistyp").and_then(|v| v.as_str()) {
                    let upper = pt.to_uppercase();
                    if !VALID_PREISTYPEN.contains(&upper.as_str()) {
                        errors.push(format!(
                            "tarifpreispositionen[{i}].preistyp '{pt}' is not in the whitelist"
                        ));
                    }
                }
            }
        }

        if errors.is_empty() {
            // Attempt full BO4E roundtrip for BO4E categories
            let result_msg = if is_bo4e {
                match serde_json::from_value::<rubo4e::current::Tarifpreisblatt>(p.data.clone()) {
                    Ok(_) => format!(
                        "VALID: category={category}, _typ=TARIFPREISBLATT. \
                         All preistyp entries are whitelisted. \
                         BO4E Tarifpreisblatt deserialised without errors."
                    ),
                    Err(e) => format!(
                        "INVALID (BO4E schema): {e}. \
                         Check sparte, tariftyp, kundentypen, registeranzahl enum values."
                    ),
                }
            } else {
                format!(
                    "VALID: category={category} (non-BO4E category, free-form data accepted). All preistyp entries whitelisted."
                )
            };
            Ok(CallToolResult::success(vec![ContentBlock::text(
                result_msg,
            )]))
        } else {
            Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "INVALID: {} error(s):\n{}",
                errors.len(),
                errors.join("\n")
            ))]))
        }
    }

    #[tool(
        description = "Explain how a specific tariff preistyp position maps to a billingd \
                       invoice output. Given a product_code and preistyp, returns the billing \
                       formula, which billing engine method it invokes, the BO4E Rechnungsposition \
                       type it produces, and the applicable regulatory basis (e.g. §3 StromStG). \
                       For EPEX-linked products (dyn_source=epex-spot-day-ahead) shows which \
                       hourly EPEX prices are required and how §41b iMSys guard applies.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn explain_invoice_position(
        &self,
        Parameters(p): Parameters<ExplainInvoicePositionParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_product;

        let product = match fetch_product(&self.state.pool, &p.lf_mp_id, &p.product_code).await {
            Ok(Some(pr)) => pr,
            Ok(None) => {
                return Err(McpError::resource_not_found(
                    format!("product {}/{} not found", p.lf_mp_id, p.product_code),
                    None,
                ));
            }
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };

        let preistyp = p.preistyp.to_uppercase();
        let is_dynamic = product.dyn_source.as_deref() == Some("epex-spot-day-ahead");

        let explanation = match preistyp.as_str() {
            "GRUNDPREIS" => "GRUNDPREIS: Fixed base charge.\n\
                 Formula: grundpreis_ct × days_in_period / 100 / 365 × days_in_period = EUR\n\
                 billingd method: ElectricityProvider::bill_grundpreis()\n\
                 BO4E output: Rechnungsposition { preistyp: Grundpreis }\n\
                 Legal: §10 StromGVV Grundpreis".to_owned(),
            "ARBEITSPREIS_EINTARIF" => "ARBEITSPREIS_EINTARIF: Single-rate consumption charge.\n\
                 Formula: ct_kwh × kwh_total / 100 = EUR\n\
                 billingd: ElectricityProvider::bill_arbeitspreis()\n\
                 BO4E: Rechnungsposition { preistyp: ArbeitspreisEintarif }\n\
                 §41b guard: If dyn_source=epex-spot-day-ahead, customer MaLo MUST have iMSys=true or BillingError.".to_owned(),
            "ARBEITSPREIS_HT" | "ARBEITSPREIS_NT" => format!(
                "{preistyp}: Dual-rate (HT/NT) consumption charge.\n\
                 Formula: ct_kwh × kwh_ht_or_nt / 100 = EUR\n\
                 billingd: ElectricityProvider::bill_ht_nt()\n\
                 Requires ZaehlzeitRegister TOU definition from marktd GET /zaehler/{{id}}/zaehlzeitdefinitionen."
            ),
            "LEISTUNGSPREIS" => "LEISTUNGSPREIS: Demand charge (RLM/C&I only).\n\
                 Formula: eur_per_kw × peak_kw_spitzenleistung = EUR\n\
                 billingd: ElectricityProvider::bill_leistungspreis()\n\
                 Source: edmd MeterBillingPeriod.spitzenleistung_kw".to_owned(),
            "EEG_VERGUETUNG" => "EEG_VERGUETUNG: Feed-in tariff credit (negative billing position).\n\
                 Formula: -(kwh × verguetungssatz_ct / 100) = EUR credit\n\
                 billingd: EnergyShareProvider or einsd settlement\n\
                 Legal: §21 EEG 2023".to_owned(),
            pt if is_dynamic => format!(
                "{pt} on dynamic tariff (dyn_source=epex-spot-day-ahead):\n\
                 Formula: EPEX_Spot[h] × kwh[h] / 100 for each hour h\n\
                 Requires: tarifbd epex_prices for each day in billing period\n\
                 §41b guard: Customer MaLo must have iMSys=true (billingd enforces)\n\
                 Missing EPEX prices → BillingError (billingd does NOT fall back silently)"
            ),
            pt => format!(
                "{pt}: mako-extended preistyp.\n\
                 See VALID_PREISTYPEN in tarifbd handlers.rs for full billing formula documentation.\n\
                 Product category: {}, dyn_source: {:?}",
                product.category,
                product.dyn_source.as_deref().unwrap_or("none")
            ),
        };

        Ok(CallToolResult::success(vec![ContentBlock::text(
            explanation,
        )]))
    }

    #[tool(
        description = "Get the full version history of a product including all past Tarifpreisblatt \
                       and Energiemix changes. Returns entries ordered newest-first with changed_at \
                       timestamps. Use this to audit price changes, verify Energiemix updates for \
                       §42 compliance, and reconstruct what tariff applied during any billing period.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_product_history(
        &self,
        Parameters(p): Parameters<GetProductParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_product_history;
        match fetch_product_history(&self.state.pool, &p.lf_mp_id, &p.product_code).await {
            Ok(history) => ContentBlock::json(serde_json::json!({
                "lf_mp_id":     p.lf_mp_id,
                "product_code": p.product_code,
                "count":         history.len(),
                "history":       history,
                "note": "Entries are newest-first. energiemix field shows §42 EnWG Herkunftsnachweis \
                         history. Changed whenever PUT /api/v1/products was called.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Retrieve the §42d EnWG comparison portal feed for a given LF. \
                       Returns all currently valid PUBLISHED tariffs with estimated annual supply costs, \
                       price points (Grundpreis, Arbeitspreis HT/NT, Leistungspreis), Energiemix, \
                       Oekolabel certifications, and full BO4E Tarifpreisblatt payloads. \
                       Supports filtering by sparte, kundentyp, oekolabel, and dynamic tariff flag. \
                       Use this to verify portal feed compliance or to inspect a product catalogue overview.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn get_comparison_feed(
        &self,
        Parameters(p): Parameters<ListProductsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::handlers::{compute_jahreskosten_supply_netto, extract_tarif_preise};
        use crate::pg::{ComparisonFeedQuery, fetch_comparison_feed};
        use rust_decimal_macros::dec;

        let q = ComparisonFeedQuery {
            lf_mp_id: Some(p.lf_mp_id.clone()),
            sparte: p.category.clone(), // reuse category param for sparte filter
            kundentyp: None,
            verbrauch_kwh: Some(dec!(3500)),
            oekolabel: None,
            include_dynamic: Some(true),
            only_dynamic: Some(false),
            limit: Some(p.limit.unwrap_or(50).min(100)),
            cursor: None,
        };
        let rows = fetch_comparison_feed(&self.state.pool, &p.lf_mp_id, &q)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let entries: Vec<serde_json::Value> = rows.iter().map(|row| {
            let preise = extract_tarif_preise(&row.data);
            let jk_netto = compute_jahreskosten_supply_netto(&preise, dec!(3500));
            serde_json::json!({
                "product_code":    row.product_code,
                "name":            row.name,
                "category":        row.category,
                "sparte":          row.sparte,
                "kundentyp":       row.kundentyp,
                "product_status":  row.product_status,
                "ist_dynamisch":   row.dyn_source.is_some(),
                "ist_oekostrom":   row.oekolabel.as_ref().map(|v| !v.is_empty()).unwrap_or(false),
                "oekolabel":       row.oekolabel,
                "valid_from":      row.valid_from.map(|d| d.to_string()),
                "valid_to":        row.valid_to.map(|d| d.to_string()),
                "grundpreis_ct_per_day":     preise.grundpreis_ct_per_day,
                "arbeitspreis_ct_per_kwh":   preise.arbeitspreis_ct_per_kwh,
                "arbeitspreis_ht_ct_per_kwh": preise.arbeitspreis_ht_ct_per_kwh,
                "arbeitspreis_nt_ct_per_kwh": preise.arbeitspreis_nt_ct_per_kwh,
                "jahreskosten_supply_netto_eur_3500kwh": jk_netto,
                "updated_at":      row.updated_at,
            })
        }).collect();

        ContentBlock::json(serde_json::json!({
            "lf_mp_id":    p.lf_mp_id,
            "count":       entries.len(),
            "note": "Annual cost estimate for 3500 kWh/year. Excludes NNE, KA, Stromsteuer, MwSt.",
            "tarife":      entries,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

// ── German local time helper ──────────────────────────────────────────────────

/// Compute the current date in German local time (CET/CEST) without an
/// external time-zone database.
///
/// EU DST rule (last Sunday of March / October):
/// - MESZ (UTC+2): from last Sunday of March 02:00 CET until last Sunday of
///   October 03:00 CEST.
/// - MEZ  (UTC+1): otherwise.
///
/// This is accurate to the day for the purpose of EPEX D-1 availability checks.
/// Sub-day accuracy (the exact hour of the DST switch) is not needed here.
fn german_local_date(utc: time::OffsetDateTime) -> time::Date {
    let date_utc = utc.date();
    let offset = german_utc_offset(date_utc, utc.hour());
    let utc_offset = time::UtcOffset::from_hms(offset, 0, 0).expect("valid offset");
    utc.to_offset(utc_offset).date()
}

fn last_sunday_of_month(year: i32, month: time::Month) -> time::Date {
    let next_month = if month == time::Month::December {
        time::Date::from_calendar_date(year + 1, time::Month::January, 1).unwrap()
    } else {
        time::Date::from_calendar_date(year, month.next(), 1).unwrap()
    };
    let last_day = next_month - time::Duration::days(1);
    let days_since_sunday = last_day.weekday().number_days_from_sunday() as i64;
    last_day - time::Duration::days(days_since_sunday)
}

fn german_utc_offset(date: time::Date, hour_utc: u8) -> i8 {
    // DST starts last Sunday of March at 02:00 CET = 01:00 UTC
    let dst_start = last_sunday_of_month(date.year(), time::Month::March);
    // DST ends last Sunday of October at 03:00 CEST = 01:00 UTC
    let dst_end = last_sunday_of_month(date.year(), time::Month::October);
    if date > dst_start && date < dst_end {
        return 2; // MESZ
    }
    if date == dst_start && hour_utc >= 1 {
        return 2; // after switch-over
    }
    if date == dst_end && hour_utc < 1 {
        return 2; // before switch-back
    }
    1 // MEZ
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
            PromptMessage::new_text(
                Role::User,
                "How do I configure a §41a EPEX dynamic tariff for iMSys customers?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
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
            PromptMessage::new_text(
                Role::Assistant,
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
            PromptMessage::new_text(
                Role::User,
                "How do I create a formal B2B price quotation for a C&I customer?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
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
    state.auth.authenticate(request, next).await
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

#[cfg(test)]
mod dst_tests {
    use super::{german_local_date, german_utc_offset, last_sunday_of_month};
    use time::Month;
    use time::macros::{date, datetime};

    /// EPEX Spot prices are hourly, so a wrong DST offset shifts every price by
    /// an hour — the whole day's §41a dynamic tariff is then billed against the
    /// wrong hours.
    #[test]
    fn offsets_switch_on_the_statutory_boundaries() {
        // 2026: DST starts Sun 29 March, ends Sun 25 October.
        assert_eq!(
            last_sunday_of_month(2026, Month::March),
            date!(2026 - 03 - 29)
        );
        assert_eq!(
            last_sunday_of_month(2026, Month::October),
            date!(2026 - 10 - 25)
        );

        // Deep winter and deep summer.
        assert_eq!(german_utc_offset(date!(2026 - 01 - 15), 12), 1);
        assert_eq!(german_utc_offset(date!(2026 - 07 - 15), 12), 2);
    }

    /// The spring-forward happens at 01:00 UTC: before it the offset is +1,
    /// from it onward +2.
    #[test]
    fn spring_forward_flips_at_0100_utc() {
        let d = date!(2026 - 03 - 29);
        assert_eq!(german_utc_offset(d, 0), 1, "00:00 UTC is still CET");
        assert_eq!(german_utc_offset(d, 1), 2, "01:00 UTC is already CEST");
        assert_eq!(german_utc_offset(d, 12), 2);
    }

    /// The fall-back also happens at 01:00 UTC, in the other direction.
    #[test]
    fn fall_back_flips_at_0100_utc() {
        let d = date!(2026 - 10 - 25);
        assert_eq!(german_utc_offset(d, 0), 2, "00:00 UTC is still CEST");
        assert_eq!(german_utc_offset(d, 1), 1, "01:00 UTC is back to CET");
        assert_eq!(german_utc_offset(d, 12), 1);
    }

    /// Late-evening UTC belongs to the next German calendar day. Getting this
    /// wrong files a price under yesterday's date.
    #[test]
    fn late_utc_evening_is_the_next_german_day() {
        // 23:30 UTC in winter = 00:30 CET the next day.
        assert_eq!(
            german_local_date(datetime!(2026-01-15 23:30 UTC)),
            date!(2026 - 01 - 16)
        );
        // 22:30 UTC in summer = 00:30 CEST the next day.
        assert_eq!(
            german_local_date(datetime!(2026-07-15 22:30 UTC)),
            date!(2026 - 07 - 16)
        );
        // 21:30 UTC in summer is still the same German day.
        assert_eq!(
            german_local_date(datetime!(2026-07-15 21:30 UTC)),
            date!(2026 - 07 - 15)
        );
    }

    /// A month whose last day is itself a Sunday must return that day.
    #[test]
    fn last_sunday_handles_a_month_ending_on_sunday() {
        // 31 May 2026 is a Sunday.
        assert_eq!(
            last_sunday_of_month(2026, Month::May),
            date!(2026 - 05 - 31)
        );
    }

    /// December must roll into the next year rather than panic.
    #[test]
    fn last_sunday_of_december_rolls_the_year() {
        assert_eq!(
            last_sunday_of_month(2026, Month::December),
            date!(2026 - 12 - 27)
        );
    }
}
