//! MCP server for `productd` — Product & Tariff Catalog (LF role).
//!
//! ## Tools (14)
//!
//! | Tool | Description |
//! |---|---|
//! | `list_products` | List products for an LF MP-ID |
//! | `get_product` | Get a single product with full Tarifpreisblatt JSONB |
//! | `get_product_history` | Full version history for a product (includes energiemix) |
//! | `resolve_product` | The version of a product code in force on a given day |
//! | `get_epex_price` | Get EPEX day-ahead 15-min MTU prices for a date |
//! | `list_angebote` | List B2B quotations (Angebote) — filter by status |
//! | `get_angebot` | Fetch a single Angebot with enriched positions and variants |
//! | `get_angebot_summary` | Summarise an Angebot in plain text for sales staff |
//! | `check_41a_epex_status` | Check if EPEX D-1 prices are current (§41a compliance) |
//! | `get_product_energiemix` | Get §42 EnWG Energiemix disclosure for a product |
//! | `validate_tariff_config` | Validate Tarifpreisblatt JSONB before PUT (same logic as REST) |
//! | `explain_invoice_position` | Explain how a preistyp maps to a billing output + formula |
//! | `get_comparison_feed` | Retrieve the § 41c comparison portal feed (proxies the REST endpoint) |
//!
//! ## Prompts (3)
//!
//! | Prompt | Description |
//! |---|---|
//! | `configure-41a-tariff` | Step-by-step: configure a §41a EPEX dynamic tariff product |
//! | `assign-product` | Where a MaLo→product assignment is made (vertragd), and productd's part in it |
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
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ProductdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
}

// ── Parameter types ───────────────────────────────────────────────────────────

/// A tool that takes no arguments.
///
/// **Not `serde_json::Value`.** Its JSON Schema is the empty schema — no
/// `type` — and the MCP specification requires a tool's `inputSchema` to have
/// root type `object`. `rmcp` asserts that while building the router, so a
/// single argument-less tool declared as `Parameters<serde_json::Value>`
/// panics the whole service at startup. Nothing else catches it: the router is
/// built in `main`, and no test starts the binary.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct NoParams {}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListProductsParams {
    /// LF MP-ID (BDEW-Codenummer, 13 digits).
    pub lf_mp_id: String,
    /// Filter by category: STROM|GAS|WAERME|WASSER|SOLAR|EEG|EINSPEISUNG|WAERMEPUMPE|WALLBOX|HEMS|EMOBILITY|ENERGIEDIENSTLEISTUNG|BUNDLE|SHARING.
    pub category: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetProductParams {
    pub lf_mp_id: String,
    pub product_code: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveProductParams {
    pub lf_mp_id: String,
    pub product_code: String,
    /// ISO date; defaults to today.
    pub as_of: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
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
    /// Product category: STROM|GAS|WAERME|WASSER|SOLAR|EEG|EINSPEISUNG|WAERMEPUMPE|WALLBOX|HEMS|EMOBILITY|ENERGIEDIENSTLEISTUNG|BUNDLE|SHARING
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
pub struct ProductdMcpHandler {
    state: Arc<ProductdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<ProductdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<ProductdMcpHandler>,
}

#[tool_router]
impl ProductdMcpHandler {
    fn new(state: Arc<ProductdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    // ── Product catalog ───────────────────────────────────────────────────────

    #[tool(
        description = "List products for an LF MP-ID. Filter by category (STROM/GAS/WAERME/WASSER/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/BUNDLE/SHARING). Returns product summaries including name, category, and validity.",
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
        match list_products(&self.state.pool, &p.lf_mp_id, &self.state.tenant, &q).await {
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
        match fetch_product(
            &self.state.pool,
            &p.lf_mp_id,
            &self.state.tenant,
            &p.product_code,
            None,
        )
        .await
        {
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
        description = "Resolve a product version: the definition of `product_code` that is in \
                       force on `as_of` (default today). Which product a MaLo is billed on is a \
                       contract fact and lives in vertragd — ask `vertragd/get_malo_produkt` for \
                       that, then this for the prices.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn resolve_product(
        &self,
        Parameters(p): Parameters<ResolveProductParams>,
    ) -> Result<CallToolResult, McpError> {
        let as_of = match p.as_of.as_deref() {
            Some(s) => Some(
                time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT)
                    .map_err(|_| McpError::invalid_params("as_of must be YYYY-MM-DD", None))?,
            ),
            None => None,
        };
        let row = crate::pg::fetch_product(
            &self.state.pool,
            &p.lf_mp_id,
            &self.state.tenant,
            &p.product_code,
            as_of,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        ContentBlock::json(serde_json::json!({
            "lf_mp_id": p.lf_mp_id,
            "product_code": p.product_code,
            "as_of": p.as_of,
            "product": row,
            "hinweis": if row.is_none() {
                "Keine Version dieses Produkts ist an diesem Tag gültig —                  valid_from/valid_to prüfen."
            } else {
                "Beide Grenzen werden angewandt: ein zurückgezogenes Produkt bepreist                  die Vergangenheit weiter, aber keinen neuen Zeitraum."
            },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Get EPEX Spot day-ahead prices for a specific date as 15-minute market time units (ct/kWh, each with its UTC mtu_start). 96 entries expected for a complete day (92/100 on DST days). Used for §41a dynamic tariff billing verification.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_epex_price(
        &self,
        Parameters(p): Parameters<EpexPriceParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_epex_day;
        use time::format_description::well_known::Rfc3339;
        let Ok(date) = time::Date::parse(
            &p.date,
            &time::format_description::well_known::Iso8601::DEFAULT,
        ) else {
            return Err(McpError::invalid_params("date must be YYYY-MM-DD", None));
        };
        match fetch_epex_day(&self.state.pool, date).await {
            Ok(Some(points)) => {
                let prices: Vec<serde_json::Value> = points
                    .iter()
                    .map(|pt| {
                        serde_json::json!({
                            "mtu_start": pt.mtu_start.format(&Rfc3339).unwrap_or_default(),
                            "price_ct_kwh": pt.avg_ct_kwh,
                        })
                    })
                    .collect();
                ContentBlock::json(serde_json::json!({
                    "date": p.date,
                    "mtu_minutes": 15,
                    "mtus_available": prices.len(),
                    "prices_ct_kwh": prices,
                    "note": "Prices in ct/kWh per 15-min MTU. 96 entries expected for a complete day.",
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => ContentBlock::json(serde_json::json!({
                "date": p.date,
                "mtus_available": 0,
                "prices_ct_kwh": [],
                "note": "No EPEX prices imported for this date. Use PUT /api/v1/epex-prices/{date} to import.",
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
             Jahreskosten brutto: {brutto} EUR (inkl. USt)\n\
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
                       billing can proceed for dynamic tariff customers.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn check_41a_epex_status(
        &self,
        Parameters(_): Parameters<NoParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_epex_latest_date;

        let latest = fetch_epex_latest_date(&self.state.pool)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // The delivery day an EPEX series is keyed on is a Europe/Berlin
        // calendar date, so it comes from the one calendar the platform counts
        // business dates in rather than off the UTC clock.
        let today_de = mako_fristen::heute();
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
                       annual invoices for electricity products.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_product_energiemix(
        &self,
        Parameters(p): Parameters<GetProductParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_energiemix;
        match fetch_energiemix(
            &self.state.pool,
            &p.lf_mp_id,
            &self.state.tenant,
            &p.product_code,
        )
        .await
        {
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
        // Single-source of truth: run the EXACT validation the PUT endpoint
        // runs (`crate::handlers::normalize_tarifpreisblatt`), so config that
        // this tool blesses cannot be rejected by the write path and vice
        // versa. Previously this was a separate re-implementation that
        // diverged on `_version`, `preisstaffeln`, and the sparte↔category
        // cross-check.
        let category = p.category.to_uppercase();
        match crate::handlers::normalize_tarifpreisblatt(&category, p.data.clone()) {
            Ok(_) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "VALID: category={category}. The payload passes the same validation as \
                 PUT /api/v1/products/{{lf}}/{{code}} (preistyp whitelist, _typ/_version, \
                 scalar preisstaffeln, sparte↔category, BO4E Tarifpreisblatt roundtrip)."
            ))])),
            Err((_status, body)) => {
                let msg = body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("validation failed")
                    .to_owned();
                Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                    "INVALID: {msg}"
                ))]))
            }
        }
    }

    #[tool(
        description = "Explain how a specific tariff preistyp position maps to a billingd \
                       invoice output. Given a product_code and preistyp, returns the billing \
                       formula, which billing engine method it invokes, the BO4E Rechnungsposition \
                       type it produces, and the applicable regulatory basis (e.g. §3 StromStG). \
                       For EPEX-linked products (dyn_source=epex-spot-day-ahead) shows which \
                       15-min EPEX prices are required and how §41a iMSys guard applies.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn explain_invoice_position(
        &self,
        Parameters(p): Parameters<ExplainInvoicePositionParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_product;

        let product = match fetch_product(
            &self.state.pool,
            &p.lf_mp_id,
            &self.state.tenant,
            &p.product_code,
            None,
        )
        .await
        {
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
            "GRUNDPREIS" => "GRUNDPREIS: Fixed base charge, priced per day.\n\
                 Formula: grundpreis_ct_per_day / 100 × prorate_days = EUR\n\
                 (the rate is per DAY, so the period length enters exactly once)\n\
                 energy-billing: ElectricityProvider::bill() → grundpreis_position()\n\
                 BO4E output: Rechnungsposition { preistyp: Grundpreis }\n\
                 Legal basis emitted on the position: §41 EnWG (supply-contract content).\n\
                 Note: §10 StromGVV is Vertragsstrafe, not the Grundpreis — in \
                 Grundversorgung the price term is §5 StromGVV.".to_owned(),
            "ARBEITSPREIS_EINTARIF" => "ARBEITSPREIS_EINTARIF: Single-rate consumption charge.\n\
                 Formula: arbeitspreis_ct_per_kwh / 100 × billable_kwh = EUR\n\
                 Graduated products (block_tiers) go through billing::RateSchedule instead.\n\
                 energy-billing: ElectricityProvider::bill()\n\
                 BO4E: Rechnungsposition { preistyp: ArbeitspreisEintarif }\n\
                 §41a guard: If dyn_source=epex-spot-day-ahead, customer MaLo MUST have iMSys=true or BillingError.".to_owned(),
            "ARBEITSPREIS_HT" | "ARBEITSPREIS_NT" => format!(
                "{preistyp}: Dual-rate (HT/NT) consumption charge.\n\
                 Formula: arbeitspreis_{{ht,nt}}_ct_per_kwh / 100 × kwh_in_that_band = EUR\n\
                 energy-billing: ElectricityProvider::bill() → billing::TimeOfUsePricing\n\
                 Selected only when the METER reports both registers AND the PRODUCT \
                 prices both bands; a half-priced Zweitarif is refused by validate_warnings.\n\
                 Requires ZaehlzeitRegister TOU definition from marktd GET /zaehler/{{id}}/zaehlzeitdefinitionen."
            ),
            "LEISTUNGSPREIS" => "LEISTUNGSPREIS: Demand charge (RLM/C&I only).\n\
                 Formula: leistungspreis_strom_ct_per_kw_month / 100 × billed_months × spitzenleistung_kw = EUR\n\
                 (the rate is per kW and MONTH, so it prorates to the billed period)\n\
                 energy-billing: ElectricityProvider::bill()\n\
                 Source: edmd MeterBillingPeriod.spitzenleistung_kw\n\
                 Legal basis emitted on the position: §41 EnWG.".to_owned(),
            "EEG_VERGUETUNG" => "EEG_VERGUETUNG: Feed-in tariff credit (negative billing position).\n\
                 Formula: -(kwh × verguetungssatz_ct / 100) = EUR credit\n\
                 einsd settlement, or energy-billing's Einspeisung provider\n\
                 Legal: §21 EEG 2023".to_owned(),
            pt if is_dynamic => format!(
                "{pt} on dynamic tariff (dyn_source=epex-spot-day-ahead):\n\
                 Formula: EPEX_Spot[q] × kwh[q] / 100 for each 15-min MTU q\n\
                 Requires: productd epex_prices for each day in billing period\n\
                 §41a guard: Customer MaLo must have iMSys=true (billingd enforces)\n\
                 Missing EPEX prices → BillingError (billingd does NOT fall back silently)"
            ),
            pt => format!(
                "{pt}: mako-extended preistyp.\n\
                 See VALID_PREISTYPEN in productd handlers.rs for full billing formula documentation.\n\
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
        match fetch_product_history(&self.state.pool, &p.lf_mp_id, &self.state.tenant, &p.product_code).await {
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
        description = "Retrieve the § 41c EnWG comparison portal feed for a given LF. \
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
        use rust_decimal::dec;

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
            let preise = extract_tarif_preise(&row.data, dec!(3500));
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

// ── Prompts ────────────────────────────────────────────────────────────────────

#[prompt_router]
impl ProductdMcpHandler {
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
                 1. Create the product in productd:\n\
                    PUT /api/v1/products/{lf_mp_id}/STROM-EPEX-01\n\
                    {\n\
                      \"category\": \"STROM\",\n\
                      \"name\": \"Dynamischer Stromtarif §41a\",\n\
                      \"data\": {\n\
                        \"dynamic_epex\": true,\n\
                        \"grundpreis_ct_per_day\": \"5.0\",\n\
                        \"tarifpreise\": [{ \"preistyp\": \"GRUNDPREIS\", ... }]\n\
                      }\n\
                    }\n\n\
                 2. Import EPEX D-1 prices daily (cron at 13:00 CET after EPEX publication):\n\
                    PUT /api/v1/epex-prices/YYYY-MM-DD\n\
                    { \"prices\": [ct_h0, ct_h1, ..., ct_h23] }  -- 24 values\n\n\
                 3. Put iMSys-eligible MaLos on it via vertragd:\n\
                    POST vertragd /api/v1/vertraege/{id}/tarifwechsel\n\
                    { \"komp_id\": \"…\", \"new_product_code\": \"STROM-EPEX-01\", \"wirksamkeit\": \"YYYY-MM-DD\",\n\
                      \"initiator\": \"LIEFERANT\", \"preise\": [{ \"bezeichnung\": \"Arbeitspreis\", … }] }\n\n\
                 4. billingd auto-detects dynamic_epex=true:\n\
                    - Fetches 15-min Lastgang from edmd\n\
                    - Joins each 15-min interval against the EPEX price for that MTU\n\
                    - NNE from marktd (PreisblattNetznutzung) added as pass-through\n\n\
                 5. Verify: use get_epex_price to check prices are imported for upcoming dates.",
            ),
        ]
    }

    #[prompt(
        name = "assign-product",
        description = "Step-by-step: put a delivery point on a tariff product"
    )]
    async fn assign_product_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I assign a tariff product to a MaLo?"),
            PromptMessage::new_text(
                Role::Assistant,
                "Not here. Which product a MaLo is on is a contract fact — agreeing it is a \
                 Tarifwechsel under § 41 Abs. 5 EnWG — so it lives in vertragd:\n\n\
                 POST vertragd /api/v1/vertraege/{vertrag_id}/tarifwechsel\n\
                 { \"komp_id\": \"…\", \"new_product_code\": \"STROM-SLP-01\",\n\
                   \"wirksamkeit\": \"2026-01-01\", \"initiator\": \"KUNDE\" }\n\n\
                 `initiator` says who is changing the tariff. LIEFERANT owes the § 41 Abs. 5 \
                 notice and must carry `preise` (the Umfang of the change); KUNDE is an agreed \
                 switch and carries no Sonderkündigungsrecht.\n\n\
                 vertragd enforces the Preisgarantie and the § 41 Abs. 5 notice period, and \
                 writes a valid-time slice. A future wirksamkeit is simply a slice that \
                 starts then.\n\n\
                 productd's part is the catalogue: `list_products` to pick a code, \
                 `resolve_product` to see what it costs on a given day.\n\
                 Verify the assignment with vertragd `get_malo_produkt { malo_id }`.",
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
                 -- Emits de.tarif.angebot.angenommen CloudEvent → ERP/vertragd creates Rahmenvertrag\n\n\
                 ## 5. Contract creation (automated via ERP webhook)\n\
                 -- ERP receives de.tarif.angebot.angenommen with positionen + chosen variant\n\
                 -- Creates Rahmenvertrag + N x Versorgungsvertrag in vertragd\n\
                 -- Returns rahmenvertrag_id → productd links the Angebot\n\n\
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
impl ServerHandler for ProductdMcpHandler {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder().enable_tools().enable_prompts().build(),
        )
        .with_server_info(Implementation::new("productd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "productd MCP -- Product & Tariff Catalog (LF role).\n\
             Single source of truth for retail products the LF sells to end customers.\n\
             Categories: STROM/GAS/WAERME/WASSER/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/BUNDLE/SHARING.\n\
             Also manages EPEX Spot day-ahead prices for §41a dynamic tariffs (iMSys, mandatory since Jan 2025).\n\
             B2B Angebote (formal quotations) for C&I/RLM customers: lifecycle ANGELEGT→VERSANDT→ANGENOMMEN/ABGELEHNT/ABGELAUFEN.\n\n\
             Key tools:\n\
             - list_products: survey the tariff catalog\n\
             - resolve_product: what a product code costs on a given day\n\
             - get_epex_price: verify D-1 EPEX prices are imported\n\
             - list_angebote: see open B2B quotations\n\
             - get_angebot_summary: human-readable quotation summary for sales review\n\n\
             Role: LF only. NB network tariffs (PreisblattNetznutzung) are in marktd.",
        )
    }
}

// ── Auth middleware + router ──────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<ProductdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<ProductdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = ProductdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
