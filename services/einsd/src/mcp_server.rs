//! MCP server for `einsd` — Einspeiser Registry + EEG/KWKG Settlement.
//!
//! ## Tools (19)
//!
//! | Tool | Description |
//! |---|---|
//! | `list_plants` | List EEG/KWKG plants (filterable by malo_id, erzeugungsart, status) |
//! | `get_plant` | Get a single plant by TechnischeRessource ID |
//! | `list_expiring` | Plants with Förderung ending within N days |
//! | `list_settlements` | Settlement history for a plant, corrections marked as such |
//! | `lookup_verguetungssatz` | Look up the applicable EEG/KWKG tariff rate (DB) |
//! | `lookup_statutory_rate` | Look up EEG rate from static tables (Solarpaket I 2024) |
//! | `trigger_settle` | Settle one plant for a month — the same path REST and the worker take |
//! | `list_unsettled_plants` | Plants not yet settled for a given month |
//! | `get_epex_monthly_price` | Look up stored EPEX Spot monthly average |
//! | `import_epex_monthly_price` | Store/update EPEX Spot monthly average price |
//! | `import_jahresmarktwert` | Store/update §20 Abs. 2 technology-specific monthly Marktwert (ÜNB) |
//! | `get_compliance_status` | Every §52 Abs. 1 violation einsd derives, priced with the engine's Abs. 2/3/5 rules |
//! | `list_plants_without_mastr` | Plants not registered in MaStR (§52 Abs. 1 Nr. 11 EEG 2023) |
//! | `check_direktvermarktung_compliance` | Plants >100 kW on an Einspeisevergütung model (§52 Abs. 1 Nr. 4) |
//! | `check_sect44b_quota` | Check §44b biogas annual 45%-cap quota status |
//! | `get_settlement_state_history` | § 147 AO / GoBD audit trail of settlement state transitions |
//! | `get_jahresmarktwert` | Look up stored §20 Abs. 2 technology-specific monthly Marktwert |
//! | `get_aw_reduktionen` | What is cutting a plant's anzulegender Wert on a given day (§§53b–54) |
//! | `explain_settlement` | The stored receipt's positions, each with its legal basis |
//!
//! Money and energy cross this surface as exact decimals ([`DecimalArg`]), not
//! `f64`: a rate that reaches a legally binding Gutschrift must not have passed
//! through binary floating point on the way.
//!
//! ## Prompts (6)
//!
//! `register-eeg-plant`, `settle-monthly`, `check-foerderung-expiry`,
//! `ausschreibung-workflow`, `post-eeg-transition`, `anlagenerweiterung`

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
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct EinsdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
    /// Needed to emit `de.eeg.settlement.*` CloudEvents from `trigger_settle`.
    ///
    /// A settlement run through MCP creates the same payment obligation as one
    /// through REST, so it has to notify accountingd and the ERP the same way.
    pub cfg: std::sync::Arc<crate::config::EinsdConfig>,
    pub http_client: std::sync::Arc<reqwest::Client>,
}

// ── Money and quantity over the wire ──────────────────────────────────────────

/// A decimal quantity accepted as a JSON **string** or number.
///
/// EEG amounts are `rust_decimal::Decimal` and never `f64`: a rate on a legally
/// binding Gutschrift must not pass through binary floating point, and 0,1 ct/kWh
/// has no exact `f64`.
///
/// A string is the lossless form and what the schema asks for; a JSON number is
/// also accepted, parsed from its own decimal text rather than through `f64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecimalArg(pub rust_decimal::Decimal);

impl<'de> Deserialize<'de> for DecimalArg {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let v = serde_json::Value::deserialize(d)?;
        let text = match &v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            other => {
                return Err(D::Error::custom(format!(
                    "expected a decimal as a string or number, got {other}"
                )));
            }
        };
        text.trim()
            .parse()
            .map(DecimalArg)
            .map_err(|e| D::Error::custom(format!("invalid decimal `{text}`: {e}")))
    }
}

impl JsonSchema for DecimalArg {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Decimal".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": ["string", "number"],
            "description": "An exact decimal. Prefer a string (\"8.11\") — a JSON number \
                            is accepted but a client that formats it as a float may \
                            already have lost precision.",
        })
    }
}

impl From<DecimalArg> for rust_decimal::Decimal {
    fn from(d: DecimalArg) -> Self {
        d.0
    }
}

impl serde::Serialize for DecimalArg {
    /// Serialised as a string, for the same reason it is parsed from one.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}

impl std::fmt::Display for DecimalArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// ── Parameter types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPlantsParams {
    pub malo_id: Option<String>,
    pub erzeugungsart: Option<String>,
    pub status: Option<String>,
    /// Max results (default 50, max 200).
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPlantParams {
    pub tr_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListExpiringParams {
    /// Horizon in days (default 180).
    pub days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSettlementsParams {
    pub tr_id: String,
    /// Max results (default 24, max 200).
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LookupParams {
    pub erzeugungsart: String,
    pub leistung_kwp: DecimalArg,
    /// ISO-8601 commissioning date YYYY-MM-DD.
    pub inbetriebnahme: String,
    /// UEBERSCHUSS (default) | VOLLEINSPEISUNG (§48 Abs. 2a bonus) | KWK_ZUSCHLAG.
    /// The two solar forms differ by several ct/kWh.
    pub verguetungsform: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TriggerSettleParams {
    pub tr_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    /// Override kWh. When absent, fetched from edmd.
    pub einspeisemenge_kwh: Option<DecimalArg>,
    /// Override the market reference ct/kWh. When absent, the stored monthly
    /// average is used (a §20 Abs. 2 Jahresmarktwert may still supersede it).
    pub epex_avg_ct_kwh: Option<DecimalArg>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListUnsettledParams {
    pub billing_year: i16,
    pub billing_month: i16,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EpexPriceMcpParams {
    pub billing_year: i16,
    pub billing_month: i16,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportEpexPriceParams {
    pub billing_year: i16,
    pub billing_month: i16,
    /// Monthly average EPEX Spot Day-Ahead price in ct/kWh.
    pub avg_ct_kwh: DecimalArg,
    /// Source description (e.g. "netztransparenz.de", "smard.de", "manual").
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LookupStatutoryRateParams {
    /// Technology: SOLAR_AUFDACH | SOLAR_FREIFLAECHE | WIND_ONSHORE | BIOMASSE | KWKG
    pub erzeugungsart: String,
    /// Installed capacity in kWp (or kW_el for KWKG).
    pub leistung_kwp: DecimalArg,
    /// EEG law year: 2017, 2021, 2023, or 2024 (Solarpaket I).
    pub eeg_year: i16,
    /// Inbetriebnahme date (`YYYY-MM-DD`) — **required for solar**. The §49
    /// degression steps on 1 February and 1 August, so a year alone does not
    /// determine a solar rate.
    pub inbetriebnahme: Option<String>,
    /// VOLLEINSPEISUNG or UEBERSCHUSSEINSPEISUNG (solar only; default: UEBERSCHUSS).
    pub messkonzept: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportJahresmarktwertParams {
    pub billing_year: i16,
    pub billing_month: i16,
    /// Technology type matching erzeugungsart column values, e.g. WIND_ONSHORE,
    /// SOLAR_AUFDACH, SOLAR_FREIFLAECHE, BIOMASSE, BIOGAS, WASSERKRAFT, or DEFAULT.
    pub erzeugungsart: String,
    /// §20 Abs. 2 + Anlage 1 EEG 2023 monthly technology-specific Marktwert in ct/kWh.
    /// Published by ÜNB at netztransparenz.de.
    pub avg_ct_kwh: DecimalArg,
    /// Source description (e.g. "netztransparenz.de", "manual").
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct JahresmarktwertLookupParams {
    pub billing_year: i16,
    pub billing_month: i16,
    /// Technology type or DEFAULT.
    pub erzeugungsart: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AwReduktionenParams {
    /// TechnischeRessource ID of the plant.
    pub tr_id: String,
    /// Date to evaluate, ISO 8601 (`YYYY-MM-DD`). Defaults to today.
    pub on: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SettlementStateHistoryParams {
    /// TechnischeRessource ID of the plant.
    pub tr_id: String,
    /// Max results (default 50, max 200).
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExplainSettlementParams {
    /// TechnischeRessource ID of the plant.
    pub tr_id: String,
    /// Billing year (e.g. 2026).
    pub billing_year: i16,
    /// Billing month 1–12.
    pub billing_month: i16,
}

// ── Handler ───────────────────────────────────────────────────────────────────

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
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "List EEG/KWKG plants. Filter by malo_id, erzeugungsart (SOLAR/WIND_ONSHORE/KWKG/etc.), or status (aktiv/abgemeldet/foerderung_beendet/repowered).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_plants(
        &self,
        Parameters(params): Parameters<ListPlantsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{AnlagenQuery, list_anlagen};
        let q = AnlagenQuery {
            malo_id: params.malo_id,
            erzeugungsart: params.erzeugungsart,
            settlement_model: None,
            status: params.status,
            limit: Some(i64::from(params.limit.unwrap_or(50).min(200))),
        };
        match list_anlagen(&self.state.pool, &self.state.tenant, &q).await {
            Ok(p) => ContentBlock::json(serde_json::to_value(p).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get a single EEG/KWKG plant by TechnischeRessource ID (tr_id). Returns all fields including settlement model, Vergütungssatz, Förderendedatum, and KWKG data.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_plant(
        &self,
        Parameters(params): Parameters<GetPlantParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_anlage;
        match fetch_anlage(&self.state.pool, &self.state.tenant, &params.tr_id).await {
            Ok(Some(p)) => ContentBlock::json(serde_json::to_value(p).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("plant {} not found", params.tr_id),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List plants whose EEG/KWKG Foerderung ends within the given days (default 180). Use to trigger early notification and plan Post-EEG transitions.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_expiring(
        &self,
        Parameters(params): Parameters<ListExpiringParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_expiring;
        let days = i32::try_from(params.days.unwrap_or(180)).unwrap_or(180);
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

    #[tool(
        description = "Monthly settlement history for a plant. Returns settlement amount, model, kWh, status, and CloudEvent ID for each settled month.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_settlements(
        &self,
        Parameters(params): Parameters<ListSettlementsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_settlement_receipts;
        let limit = i64::from(params.limit.unwrap_or(24).min(200));
        match list_settlement_receipts(&self.state.pool, &self.state.tenant, &params.tr_id, limit)
            .await
        {
            Ok(r) => ContentBlock::json(serde_json::to_value(r).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Look up the applicable EEG or KWKG Verguetungssatz (tariff rate ct/kWh) for a commissioning date and capacity. The rate is fixed at commissioning for the full 20-year Foerderdauer.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn lookup_verguetungssatz(
        &self,
        Parameters(params): Parameters<LookupParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::lookup_verguetungssatz;
        let kwp = params.leistung_kwp.into();
        match lookup_verguetungssatz(
            &self.state.pool,
            &params.erzeugungsart,
            params.verguetungsform.as_deref().unwrap_or("UEBERSCHUSS"),
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

    // ── Settlement ────────────────────────────────────────────────────────────

    #[tool(
        description = "Trigger monthly EEG/KWKG settlement for one plant. Idempotent. \
        Runs the same path as the REST endpoint and the monthly worker, so the amount does \
        not depend on which one triggered it: Einspeisemenge from edmd, market price from the \
        store, and the §51/§51a figures derived from the ¼h feed-in against the EPEX spot \
        curve, unless supplied. Emits de.eeg.verguetung.berechnet or \
        de.eeg.marktpraemie.berechnet. KWKG hour-limit enforcement is automatic."
    )]
    async fn trigger_settle(
        &self,
        Parameters(params): Parameters<TriggerSettleParams>,
    ) -> Result<CallToolResult, McpError> {
        // The same path REST, batch and the worker take, so an agent settling a
        // plant gets the identical amount — edmd auto-fetch and §51 included.
        let result = crate::settle::settle_by_tr_id(
            &self.state.pool,
            &self.state.cfg,
            &self.state.http_client,
            &params.tr_id,
            params.billing_year,
            params.billing_month,
            crate::settle::SettleRequest {
                einspeisemenge_kwh: params.einspeisemenge_kwh.map(Into::into),
                epex_avg_ct_kwh: params.epex_avg_ct_kwh.map(Into::into),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?
        .ok_or_else(|| {
            McpError::invalid_params(format!("plant {} not found", params.tr_id), None)
        })?;

        ContentBlock::json(serde_json::to_value(&result).unwrap_or_default())
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "List active plants NOT yet settled for the given billing month. \
        Use to preview before POST /api/v1/settle/{year}/{month} batch run.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_unsettled_plants(
        &self,
        Parameters(params): Parameters<ListUnsettledParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_unsettled;
        match list_unsettled(
            &self.state.pool,
            &self.state.tenant,
            params.billing_year,
            params.billing_month,
        )
        .await
        {
            Ok(plants) => ContentBlock::json(serde_json::json!({
                "billing_year": params.billing_year,
                "billing_month": params.billing_month,
                "unsettled_count": plants.len(),
                "plants": plants,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    // ── EPEX price ────────────────────────────────────────────────────────────

    #[tool(
        description = "Look up the stored EPEX Spot Day-Ahead monthly average price (ct/kWh). \
        Required for DIREKTVERMARKTUNG (Gleitende Marktpraemie) and POST_EEG_SPOT settlement.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_epex_monthly_price(
        &self,
        Parameters(params): Parameters<EpexPriceMcpParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_epex_price;
        match fetch_epex_price(&self.state.pool, params.billing_year, params.billing_month).await {
            Ok(Some(price)) => ContentBlock::json(serde_json::json!({
                "billing_year": params.billing_year,
                "billing_month": params.billing_month,
                "avg_ct_kwh": price,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "No EPEX price stored for {:04}-{:02}. \
                 Use import_epex_monthly_price or PUT /api/v1/epex-monthly/{}/{:02}. \
                 Source: netztransparenz.de or smard.de.",
                params.billing_year,
                params.billing_month,
                params.billing_year,
                params.billing_month,
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Store or update the EPEX Spot Day-Ahead monthly average price (ct/kWh). \
        Required before settling DIREKTVERMARKTUNG or POST_EEG_SPOT plants. Idempotent."
    )]
    async fn import_epex_monthly_price(
        &self,
        Parameters(params): Parameters<ImportEpexPriceParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::upsert_epex_price;
        let avg = params.avg_ct_kwh.into();
        let source = params.source.as_deref().unwrap_or("mcp-import");
        match upsert_epex_price(
            &self.state.pool,
            params.billing_year,
            params.billing_month,
            avg,
            source,
        )
        .await
        {
            Ok(()) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "EPEX price {avg:.4} ct/kWh stored for {:04}-{:02} (source: {source}).",
                params.billing_year, params.billing_month,
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Look up the statutory EEG feed-in tariff rate for a plant without DB access.
    ///
    /// Uses the built-in `eeg_billing::rates` static tables (reference starting rates).
    /// For precise quarterly-degressioned rates, use `lookup_verguetungssatz` (DB-backed).
    ///
    /// Returns the rate in ct/kWh for the given technology, installed capacity, EEG year,
    /// and metering concept (Volleinspeisung vs. Überschusseinspeisung).
    #[tool(
        description = "Look up the statutory EEG feed-in tariff (ct/kWh) from the built-in \
            rate tables. Use erzeugungsart: SOLAR_AUFDACH | SOLAR_FREIFLAECHE | WIND_ONSHORE \
            | BIOMASSE | KWKG. messkonzept: VOLLEINSPEISUNG | UEBERSCHUSSEINSPEISUNG (solar only). \
            Solar requires inbetriebnahme (YYYY-MM-DD): the §49 degression steps on 1 Feb \
            and 1 Aug, so a year alone does not determine a solar rate. For the stored \
            per-plant rate use lookup_verguetungssatz.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn lookup_statutory_rate(
        &self,
        Parameters(params): Parameters<LookupStatutoryRateParams>,
    ) -> Result<CallToolResult, McpError> {
        use eeg_billing::rates;

        let kwp = params.leistung_kwp.into();

        let volleinspeisung = params
            .messkonzept
            .as_deref()
            .map(|s| s.to_uppercase() == "VOLLEINSPEISUNG")
            .unwrap_or(false);

        // Parse to the typed enum so the routing shares one canonical vocabulary
        // with `eeg_billing::rates::lookup_rate_for` — no divergent string match.
        let art = eeg_billing::ErzeugungsArt::from_db_str(&params.erzeugungsart.to_uppercase())
            .map_err(|_| {
                McpError::invalid_params(
                    format!(
                        "unknown erzeugungsart: {}. Use e.g. SOLAR_AUFDACH, WIND_ONSHORE, BIOMASSE, KWKG",
                        params.erzeugungsart
                    ),
                    None,
                )
            })?;

        let result = if art.is_solar() {
            // §49 degresses on 1 February and 1 August, so the solar tables are
            // keyed by Inbetriebnahme rather than by law year.
            let ibn = params
                .inbetriebnahme
                .as_deref()
                .ok_or_else(|| {
                    McpError::invalid_params(
                        "inbetriebnahme (YYYY-MM-DD) is required for solar: the §49 degression steps twice a year, so eeg_year alone does not determine the rate",
                        None,
                    )
                })
                .and_then(|s| {
                    time::Date::parse(
                        s,
                        &time::format_description::well_known::Iso8601::DATE,
                    )
                    .map_err(|_| {
                        McpError::invalid_params(
                            format!("invalid inbetriebnahme {s}; expected YYYY-MM-DD"),
                            None,
                        )
                    })
                })?;

            // Solar splits by Messkonzept (Überschuss vs Volleinspeisung + bonus).
            let table = if volleinspeisung {
                rates::solar_pv_volleinspeisung_lookup(ibn).ok_or_else(|| {
                    McpError::invalid_params(
                        format!("no Volleinspeisung rates for Inbetriebnahme {ibn}; use einsd DB lookup_verguetungssatz"),
                        None,
                    )
                })?
            } else {
                rates::solar_pv_ueberschuss_lookup(ibn).ok_or_else(|| {
                    McpError::invalid_params(
                        format!("no Überschusseinspeisung rates for Inbetriebnahme {ibn}; use einsd DB lookup_verguetungssatz"),
                        None,
                    )
                })?
            };
            table.rate_for(kwp)
        } else {
            // Non-solar technologies share the exhaustive enum-keyed router.
            rates::lookup_rate_for(art, kwp, params.eeg_year)
        };

        match result {
            Ok(rate) => {
                let rate_ct = rate.into_decimal() * rust_decimal::Decimal::from(100u32);
                Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                    "Statutory rate for {erzeugungsart} {kwp} kWp (EEG {eeg_year}{ms}): \
                    {rate_ct:.2} ct/kWh ({rate} EUR/kWh).\n\
                    Note: this is the statutory table rate. Use lookup_verguetungssatz for the \
                    rate actually stored against a plant.",
                    erzeugungsart = params.erzeugungsart,
                    eeg_year = params.eeg_year,
                    ms = if volleinspeisung {
                        ", Volleinspeisung"
                    } else {
                        ""
                    },
                ))]))
            }
            Err(e) => Err(McpError::invalid_params(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Check §52 EEG 2023 compliance for a plant: every Abs. 1 violation \
        einsd can derive (§9 Steuerbarkeit, §10b Direktvermarktungspflicht, §21c \
        Wechselmeldung, MaStR), the Pflichtzahlung the settlement engine would charge for \
        them (Abs. 2/3 rates, Abs. 5 cap), and the KWKG hour-limit position.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_compliance_status(
        &self,
        Parameters(p): Parameters<GetPlantParams>,
    ) -> Result<CallToolResult, McpError> {
        let Some(anlage) = crate::pg::fetch_anlage(&self.state.pool, &self.state.tenant, &p.tr_id)
            .await
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?
        else {
            return Err(McpError::invalid_params(
                format!("plant {} not found", p.tr_id),
                None,
            ));
        };

        let today = mako_fristen::heute();
        let foerderung_aktiv = anlage.foerderendedatum >= today;
        let heute_monatserster = today.replace_day(1).unwrap_or(today);

        // The same detector the settlement uses, so the report and the payment
        // agree — including the Abs. 3 reduced rate, the Abs. 5 cap and §9.
        let verstoesse = crate::sect52::derive_pflichtverstoesse(
            &anlage,
            crate::sect52::Sect52Context {
                billing_date: heute_monatserster,
                ausfallverguetung: crate::pg::ausfallverguetung_nutzung(
                    &mut *self
                        .state
                        .pool
                        .acquire()
                        .await
                        .map_err(|e| McpError::internal_error(e.to_string(), None))?,
                    &anlage.tr_id,
                    &self.state.tenant,
                    i16::try_from(heute_monatserster.year()).unwrap_or(0),
                    heute_monatserster.month() as i16,
                )
                .await
                .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?,
            },
        );

        // §52 Abs. 2/3 rates and the Abs. 5 cap, from the engine — one month's
        // exposure, so each violation is priced for a single month.
        let monatlich: rust_decimal::Decimal = verstoesse
            .iter()
            .map(|v| {
                eeg_billing::calculate_pflichtzahlung(&eeg_billing::Pflichtverstoss {
                    monate_des_verstosses: 1,
                    ..v.clone()
                })
            })
            .sum::<rust_decimal::Decimal>()
            .min(anlage.leistung_kwp * rust_decimal::Decimal::from(10));

        let eeg_2023_regime = anlage.eeg_gesetz >= 2023;
        let kwk_max_kwh = anlage
            .kwk_foerderdauer_h
            .map(|h| rust_decimal::Decimal::from(h) * anlage.leistung_kwp);

        ContentBlock::json(serde_json::json!({
            "tr_id": anlage.tr_id,
            "compliance_ok": verstoesse.is_empty(),
            "foerderung_aktiv": foerderung_aktiv,
            "mastr": {
                "registriert": anlage.mastr_registriert,
                "nummer": anlage.mastr_nummer,
                // ISO 8601, not `time::Date`'s derived `[year, ordinal]` array —
                // see the note in obsd's MCP server. A date a consumer cannot
                // read must not look like one it can.
                "datum": anlage.mastr_datum.map(|d| d.to_string()),
            },
            "sect9": {
                "erfuellung": anlage.sect9_erfuellung,
                "pflicht": format!("{:?}", eeg_billing::settlement_state::sect9_pflicht(
                    anlage.leistung_kwp,
                    eeg_billing::ErzeugungsArt::from_db_str(&anlage.erzeugungsart).ok(),
                )),
            },
            "pflichtverstoesse": verstoesse.iter().map(|v| serde_json::json!({
                "typ": format!("{:?}", v.typ),
                "monate": v.monate_des_verstosses,
            })).collect::<Vec<_>>(),
            "pflichtzahlung": {
                "monatlich_eur": monatlich.to_string(),
                "regime": if eeg_2023_regime {
                    "§52 Abs. 2 EEG 2023: 10 €/kW/Monat, auf 2 € reduziert bei Nachholung (Abs. 3),                      gedeckelt auf 10 €/kW (Abs. 5), verrechenbar mit der Vergütung (Abs. 6)"
                } else {
                    "EEG ≤2021 via §100: der Verstoß mindert die Vergütung selbst, es gibt keine                      separate Pflichtzahlung — monatlich_eur ist hier nicht anwendbar"
                },
                "anwendbar": eeg_2023_regime,
            },
            "kwkg_stundenkontingent": kwk_max_kwh.map(|max| {
                let verbraucht = anlage.kwk_strom_kwh_gesamt.unwrap_or_default();
                serde_json::json!({
                    "max_kwh": max.to_string(),
                    "verbraucht_kwh": verbraucht.to_string(),
                    "verbleibend_kwh": (max - verbraucht).max(rust_decimal::Decimal::ZERO).to_string(),
                    "erschoepft": verbraucht >= max,
                    "basis": "§8 KWKG — Vollbenutzungsstunden × installierte Leistung",
                })
            }),
            "recommended_action": if verstoesse.is_empty() {
                "No action required".to_owned()
            } else {
                format!(
                    "Resolve {} open §52 Abs. 1 violation(s); each further calendar month adds to the charge.",
                    verstoesse.len()
                )
            },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "List plants not registered in MaStR (Marktstammdatenregister). §52 Abs. 1 Nr. 11 EEG 2023 charges 10 €/kW/month for them; a plant under the pre-2023 regime instead loses its Vergütung entirely and owes no Pflichtzahlung, so its monthly_penalty_eur is null and it is excluded from the total.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_plants_without_mastr(&self) -> Result<CallToolResult, McpError> {
        use sqlx::Row;
        let rows = sqlx::query(
            r"SELECT tr_id, malo_id, erzeugungsart, leistung_kwp, eeg_gesetz, foerderendedatum
              FROM eeg_anlagen
              WHERE tenant = $1
                AND mastr_registriert = false
                AND status = 'aktiv'
                AND (foerderendedatum IS NULL OR foerderendedatum >= heute())
              ORDER BY leistung_kwp DESC",
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let plants: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let leistung: rust_decimal::Decimal = r
                    .try_get("leistung_kwp")
                    .unwrap_or_default();
                let eeg_gesetz: i16 = r.try_get("eeg_gesetz").unwrap_or(2023);
                // §52 Abs. 2 charges 10 €/kW/month only under the EEG 2023
                // regime. For an older plant §100 keeps the pre-2023 rule, where
                // the breach reduces the Vergütung to zero and no Pflichtzahlung
                // exists — reporting 10 €/kW for those, and summing it into a
                // portfolio total, invented money nobody owes.
                let eeg_2023 = eeg_gesetz >= 2023;
                serde_json::json!({
                    "tr_id": r.try_get::<String, _>("tr_id").unwrap_or_default(),
                    "malo_id": r.try_get::<String, _>("malo_id").unwrap_or_default(),
                    "erzeugungsart": r.try_get::<String, _>("erzeugungsart").unwrap_or_default(),
                    "leistung_kwp": leistung.to_string(),
                    "eeg_gesetz": eeg_gesetz,
                    "monthly_penalty_eur": eeg_2023.then(|| (leistung * rust_decimal::Decimal::from(10)).to_string()),
                    "regime": if eeg_2023 {
                        "§52 Abs. 1 Nr. 11 i.V.m. Abs. 2 EEG 2023: 10 €/kW/Monat"
                    } else {
                        "EEG ≤2021 via §100: Vergütung auf null — keine Pflichtzahlung"
                    },
                })
            })
            .collect();

        // Only the plants that actually owe a Pflichtzahlung are in the total.
        let total_penalty: rust_decimal::Decimal = plants
            .iter()
            .filter_map(|p| p["monthly_penalty_eur"].as_str())
            .filter_map(|s| s.parse::<rust_decimal::Decimal>().ok())
            .sum();

        ContentBlock::json(serde_json::json!({
            "count": plants.len(),
            "total_monthly_penalty_eur": total_penalty.to_string(),
            "plants": plants,
            "regulatory_note": "Register all plants at https://www.marktstammdatenregister.de. POST /api/v1/anlagen/{tr_id}/mastr-registrierung after successful registration.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// List plants where mandatory Direktvermarktung (§3 Nr. 1 + §20 EEG 2023) is required
    /// but the plant is settled under a non-Direktvermarktung scheme.
    ///
    /// Mandatory when: leistung_kwp > 100 AND eeg_gesetz >= 2012 AND status = aktiv.
    /// Settling such plants under VERGUETUNG violates §52 Abs. 1 Nr. 4 EEG 2023.
    #[tool(
        name = "check_direktvermarktung_compliance",
        description = "List active plants above 100 kW settled on an Einspeisevergütung model. §21 Abs. 1 Satz 1 Nr. 1 EEG 2023 grants that claim only up to 100 kW, so a larger plant must market directly (§10b) — the breach is §52 Abs. 1 Nr. 4 at 10 €/kW/month, and the settlement engine now charges it.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn check_direktvermarktung_compliance(&self) -> Result<CallToolResult, McpError> {
        // Exactly the predicate `sect52::derive_pflichtverstoesse` charges on, so
        // the report and the settlement cannot tell an operator different things
        // about the same plant.
        let rows = sqlx::query_as::<_, crate::pg::AnlageRow>(
            r"SELECT * FROM eeg_anlagen
              WHERE tenant = $1
                AND status = 'aktiv'
                AND leistung_kwp > $2
                AND settlement_model = $3
                AND foerderendedatum >= heute()
              ORDER BY leistung_kwp DESC",
        )
        .bind(&self.state.tenant)
        .bind(crate::sect52::DIREKTVERMARKTUNG_PFLICHT_KW)
        .bind(crate::models::VERGUETUNG)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let violations: Vec<serde_json::Value> = rows
            .iter()
            .map(|a| {
                serde_json::json!({
                    "tr_id": a.tr_id,
                    "malo_id": a.malo_id,
                    "erzeugungsart": a.erzeugungsart,
                    "leistung_kwp": a.leistung_kwp.to_string(),
                    "eeg_gesetz": a.eeg_gesetz,
                    "current_settlement_model": a.settlement_model,
                    "foerderendedatum": a.foerderendedatum.to_string(),
                    "monthly_penalty_eur": (a.leistung_kwp * rust_decimal::Decimal::from(10)).to_string(),
                    "required_action": "Switch to DIREKTVERMARKTUNG: PUT /api/v1/anlagen/{tr_id} with settlement_model=DIREKTVERMARKTUNG, direktverm_aw_ct and direktverm_mp_id — or POST /switch-veraeusserungsform, which enforces the §21b/§21c timing.",
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "violations_count": violations.len(),
            "compliant": violations.is_empty(),
            "violations": violations,
            "legal_basis": "§21 Abs. 1 Satz 1 Nr. 1 EEG 2023 caps the Einspeisevergütung claim at 100 kW; the duty to market directly is §10b and the breach is §52 Abs. 1 Nr. 4 i.V.m. Abs. 2 at 10 €/kW/Monat.",
            "note": "Mieterstrom (§21 Abs. 3) and GGV (§42b EnWG) are deliberately out of scope — they carry their own size rules, and the settlement does not charge Nr. 4 on them either.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// §44b Abs. 1 EEG 2023 — the annual Biogas quota for one plant.
    #[tool(
        name = "check_sect44b_quota",
        description = "§44b Abs. 1 EEG 2023: the annual Biogas quota for a plant and how much of \
        it is left. The quota is the share of the calendar year's generation whose \
        Bemessungsleistung equals 45 % of the installed capacity — 0,45 × kW × the §3 Nr. 6 \
        hours of that year, which is 8 784 in a leap year and is shortened for a plant that \
        first generated during it. Applies to BIOGAS above 100 kW outside the §51b Ausschreibung.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn check_sect44b_quota(
        &self,
        Parameters(p): Parameters<GetPlantParams>,
    ) -> Result<CallToolResult, McpError> {
        use rust_decimal::Decimal;

        let Some(anlage) = crate::pg::fetch_anlage(&self.state.pool, &self.state.tenant, &p.tr_id)
            .await
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?
        else {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Plant '{}' not found",
                p.tr_id
            ))]));
        };

        // The same applicability test the settlement uses.
        if anlage.erzeugungsart != "BIOGAS" {
            return ContentBlock::json(serde_json::json!({
                "tr_id": p.tr_id,
                "applicable": false,
                "reason": format!(
                    "§44b Abs. 1 covers Strom aus Biogas; this plant is {}",
                    anlage.erzeugungsart
                ),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None));
        }
        if anlage.is_biogas_sect51b {
            return ContentBlock::json(serde_json::json!({
                "tr_id": p.tr_id,
                "applicable": false,
                "reason": "an Ausschreibungsanlage under §51b is outside the §44b cap",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None));
        }
        if anlage.leistung_kwp <= Decimal::from(100) {
            return ContentBlock::json(serde_json::json!({
                "tr_id": p.tr_id,
                "applicable": false,
                "reason": format!(
                    "§44b Abs. 1 applies above 100 kW; this plant is {} kW",
                    anlage.leistung_kwp
                ),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None));
        }

        let jahr = time::OffsetDateTime::now_utc().year();
        // The quota comes from the engine, in Decimal, against the §3 Nr. 6 hours
        // of *this* year — not a flat 8 760, which would disagree with the
        // settlement in every leap year and in a plant's first year.
        let kontingent = eeg_billing::sect44b_jahreskontingent_kwh(
            anlage.leistung_kwp,
            jahr,
            Some(anlage.inbetriebnahme),
        );
        let ytd = if anlage.biogas_quota_ytd_year == i16::try_from(jahr).ok() {
            anlage.biogas_quota_kwh_ytd
        } else {
            Decimal::ZERO
        };
        let verbleibend = (kontingent - ytd).max(Decimal::ZERO);
        let ausschoepfung = if kontingent > Decimal::ZERO {
            (ytd / kontingent * Decimal::from(100)).min(Decimal::from(100))
        } else {
            Decimal::ZERO
        };

        ContentBlock::json(serde_json::json!({
            "tr_id": p.tr_id,
            "applicable": true,
            "leistung_kwp": anlage.leistung_kwp.to_string(),
            "quota_year": jahr,
            "bemessungsstunden": eeg_billing::bemessungsleistung_stunden(
                jahr,
                Some(anlage.inbetriebnahme),
            )
            .map(|h| h.to_string()),
            "annual_quota_kwh": kontingent.to_string(),
            "ytd_fed_in_kwh": ytd.to_string(),
            "remaining_quota_kwh": verbleibend.to_string(),
            "exhaustion_pct": ausschoepfung.round_dp(1).to_string(),
            "alert": if ausschoepfung >= Decimal::from(90) {
                "CRITICAL: the quota is over 90 % used — further kWh are paid at the Marktwert, or nothing at all on a Marktprämie"
            } else if ausschoepfung >= Decimal::from(75) {
                "WARNING: the quota is over 75 % used"
            } else {
                "OK"
            },
            "legal_basis": "§44b Abs. 1 i.V.m. §3 Nr. 6 EEG 2023",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        name = "import_jahresmarktwert",
        description = "Store or update a §20 Abs. 2 + Anlage 1 EEG 2023 technology-specific monthly \
Marktwert published by the ÜNB (netztransparenz.de). \
For MarketPremium (Direktvermarktung / Ausschreibung) settlements this value takes \
precedence over the generic EPEX monthly average from import_epex_monthly_price. \
erzeugungsart must match plant erzeugungsart values (WIND_ONSHORE, SOLAR_AUFDACH, \
SOLAR_FREIFLAECHE, BIOMASSE, BIOGAS, WASSERKRAFT, etc.) or 'DEFAULT' for the generic fallback."
    )]
    async fn import_jahresmarktwert(
        &self,
        Parameters(params): Parameters<ImportJahresmarktwertParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::upsert_jahresmarktwert;
        let avg = params.avg_ct_kwh.into();
        let source = params.source.as_deref().unwrap_or("manual");
        match upsert_jahresmarktwert(
            &self.state.pool,
            params.billing_year,
            params.billing_month,
            &params.erzeugungsart,
            avg,
            source,
        )
        .await
        {
            Ok(()) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "Jahresmarktwert stored: {}/{}/{} = {:.4} ct/kWh (source: {}). \
                 This value will be used for all {} MarketPremium settlements for \
                 billing_year={} billing_month={}.",
                params.billing_year,
                params.billing_month,
                params.erzeugungsart,
                params.avg_ct_kwh,
                source,
                params.erzeugungsart,
                params.billing_year,
                params.billing_month,
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        name = "get_jahresmarktwert",
        description = "Look up the stored §20 Abs. 2 technology-specific monthly Marktwert for a given \
year, month, and erzeugungsart. Returns NOT_FOUND when no row exists (settlements will \
fall back to EPEX in that case). Use 'DEFAULT' as erzeugungsart to check the generic fallback row.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_jahresmarktwert_tool(
        &self,
        Parameters(params): Parameters<JahresmarktwertLookupParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_jahresmarktwert_single;
        match fetch_jahresmarktwert_single(
            &self.state.pool,
            params.billing_year,
            params.billing_month,
            &params.erzeugungsart,
        )
        .await
        {
            Ok(Some(p)) => ContentBlock::json(serde_json::json!({
                "billing_year": params.billing_year,
                "billing_month": params.billing_month,
                "erzeugungsart": params.erzeugungsart,
                "avg_ct_kwh": p,
                "legal_basis": "§20 Abs. 2 + Anlage 1 EEG 2023 (ÜNB-published technology-specific Marktwert)",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "No Jahresmarktwert stored for {}/{}/{} — settlements use EPEX fallback.",
                params.billing_year, params.billing_month, params.erzeugungsart
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        name = "get_aw_reduktionen",
        description = "Explain why a plant's anzulegender Wert is cut on a given date. Lists every \
active sect. 53b / 53c / 54 EEG 2023 reduction with its statutory amount. Use this FIRST when an \
operator asks why a Gutschrift is smaller than expected: these cuts apply silently and shrink the \
payment without changing the Einspeisemenge or the tariff table. sect. 53b = Regionalnachweis \
(sect. 79a), fixed 0.1 ct/kWh, only where the AW is gesetzlich bestimmt (never a tender award). \
sect. 53c = Stromsteuerbefreiung for grid-transited electricity, capped at the sect. 3 StromStG \
full rate of 2.05 ct/kWh. sect. 54 = solar first-segment auction defects (0.3 / 0.3 / 2.5 ct/kWh, \
or AW to zero). All of them cut the AW BEFORE the settlement formula, and the gleitende \
Marktpraemie is floored at zero, so a cut can reduce the payment to zero but never below it.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_aw_reduktionen(
        &self,
        Parameters(params): Parameters<AwReduktionenParams>,
    ) -> Result<CallToolResult, McpError> {
        let on = match params.on.as_deref() {
            Some(s) => time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE)
                .map_err(|e| {
                McpError::invalid_params(format!("invalid date `{s}`: {e}"), None)
            })?,
            None => mako_fristen::heute(),
        };
        let v =
            crate::pg::aw_reduktionen_am(&self.state.pool, &self.state.tenant, &params.tr_id, on)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        ContentBlock::json(v)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        name = "get_settlement_state_history",
        description = "Fetch the § 147 AO / GoBD audit trail of settlement state transitions for a plant \
(tr_id). Returns all state changes (Active → Reduced → Suspended → PostEeg → Ended) with \
effective dates and transition reasons. Required for BNetzA regulatory audit and §20 EnWG \
compliance reporting.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_settlement_state_history(
        &self,
        Parameters(params): Parameters<SettlementStateHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(50).min(200);
        let rows = sqlx::query(
            "SELECT id, from_state, to_state, effective_from, reason, notes, recorded_at \
              FROM settlement_state_transitions \
             WHERE tr_id = $1 AND tenant = $2 \
             ORDER BY effective_from DESC, recorded_at DESC \
             LIMIT $3",
        )
        .bind(&params.tr_id)
        .bind(&self.state.tenant)
        .bind(i64::from(limit))
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                use sqlx::Row as _;
                serde_json::json!({
                    "id": r.try_get::<String, _>("id").unwrap_or_default(),
                    "from_state": r.try_get::<String, _>("from_state").unwrap_or_default(),
                    "to_state": r.try_get::<String, _>("to_state").unwrap_or_default(),
                    "effective_from": r.try_get::<time::Date, _>("effective_from").map(|d| d.to_string()).unwrap_or_default(),
                    "reason": r.try_get::<String, _>("reason").unwrap_or_default(),
                    "notes": r.try_get::<Option<String>, _>("notes").unwrap_or(None),
                    "recorded_at": r.try_get::<time::OffsetDateTime, _>("recorded_at").map(|t| t.to_string()).unwrap_or_default(),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "tr_id": params.tr_id,
            "total": items.len(),
            "transitions": items,
            "legal_basis": "§ 147 AO / GoBD: audit trail of settlement state transitions (Buchungsbelege, 8-year retention).",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Explain a specific monthly settlement calculation: why was this EUR amount \
             computed, which reductions applied, and what is the full position trace. Returns all \
             SettlePosition entries (description, legal_basis, kWh, rate_ct_kwh, EUR) for the \
             settlement receipt. Essential for operator audits, BNetzA inspections, and dispute \
             resolution.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn explain_settlement(
        &self,
        Parameters(params): Parameters<ExplainSettlementParams>,
    ) -> Result<CallToolResult, McpError> {
        // Fetch the stored positions from settlement_receipts
        let row = sqlx::query(
            "SELECT id, billing_year, billing_month, settlement_model, \
              einspeisemenge_kwh, settlement_eur, status, positions_json, \
              pflichtzahlung_eur, verlaengerungsanspruch_qh, \
              billing_days_fraction, settled_at \
             FROM settlement_receipts \
             WHERE tr_id = $1 AND tenant = $2 \
               AND billing_year = $3 AND billing_month = $4 \
             ORDER BY settled_at DESC LIMIT 1",
        )
        .bind(&params.tr_id)
        .bind(&self.state.tenant)
        .bind(params.billing_year)
        .bind(params.billing_month)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let Some(row) = row else {
            return Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "No settlement found for plant {} in {}/{:02}.",
                params.tr_id, params.billing_year, params.billing_month
            ))]));
        };

        use sqlx::Row as _;
        let positions: serde_json::Value = row
            .try_get::<Option<serde_json::Value>, _>("positions_json")
            .unwrap_or(None)
            .unwrap_or(serde_json::Value::Array(vec![]));

        let result = serde_json::json!({
            "tr_id": params.tr_id,
            "billing_year": params.billing_year,
            "billing_month": params.billing_month,
            "settlement_model": row.try_get::<String, _>("settlement_model").unwrap_or_default(),
            "status": row.try_get::<String, _>("status").unwrap_or_default(),
            "einspeisemenge_kwh": row.try_get::<Option<rust_decimal::Decimal>, _>("einspeisemenge_kwh").unwrap_or(None),
            "settlement_eur": row.try_get::<Option<rust_decimal::Decimal>, _>("settlement_eur").unwrap_or(None),
            "pflichtzahlung_eur": row.try_get::<Option<rust_decimal::Decimal>, _>("pflichtzahlung_eur").unwrap_or(None),
            "verlaengerungsanspruch_qh": row.try_get::<i64, _>("verlaengerungsanspruch_qh").unwrap_or(0),
            "billing_days_fraction": row.try_get::<Option<rust_decimal::Decimal>, _>("billing_days_fraction").unwrap_or(None),
            "settled_at": row.try_get::<Option<time::OffsetDateTime>, _>("settled_at").ok().flatten().map(|t| t.to_string()),
            "positions": positions,
            "interpretation": format!(
                "Settlement for {}/{:02}: {} positions listed above. \
                 Each position shows the legal paragraph, kWh quantity, rate, and EUR amount. \
                 The 'settlement_eur' is the sum of all position EUR values. \
                 A pflichtzahlung_eur > 0 means a separate §52 EEG penalty is owed to the NB.",
                params.billing_year, params.billing_month,
                positions.as_array().map(|a| a.len()).unwrap_or(0)
            )
        });

        ContentBlock::json(result)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }
}

// ── Prompts ───────────────────────────────────────────────────────────────────
//
// These are instructions a model acts on, so a wrong one is worse than none.
// They previously taught an additive Managementprämie of 0,4 ct/kWh (Anlage 1
// defines `MP = AW − MW` and nothing else; §20 EEG 2023 has no Absätze at all),
// a 20-year clock reset under "§22" (that is the Ausschreibung provision), a
// twelve-month advance-notice duty under "§21 Abs. 1" that no such provision
// contains, and MaStR maintenance under "§28a". All of it is gone.

#[prompt_router]
impl EinsdMcpHandler {
    #[prompt(
        name = "register-eeg-plant",
        description = "Register a new EEG/KWKG feed-in plant, with the fields each settlement model needs"
    )]
    async fn register_eeg_plant_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "I need to register a new EEG feed-in plant."),
            PromptMessage::new_text(
                Role::Assistant,
                "## EEG/KWKG plant registration — POST /api/v1/anlagen\n\n\
                 ### Always\n\
                 - tr_id (TechnischeRessource ID from marktd), malo_id, optional melo_id\n\
                 - inbetriebnahme (YYYY-MM-DD), leistung_kwp, eeg_gesetz (year, 0 for KWKG)\n\
                 - erzeugungsart — there is no generic SOLAR: the §48 rate depends on the\n\
                   Bauform, so pick SOLAR_AUFDACH | SOLAR_FREIFLAECHE | SOLAR_AGRIPV |\n\
                   SOLAR_MIETERSTROM | SOLAR_STECKER | WIND_ONSHORE | WIND_OFFSHORE |\n\
                   BIOMASSE | BIOMASSE_HOLZ | BIOGAS | BIOMETHAN | KLAEGAS | GRUBENGAS |\n\
                   DEPONIEGAS | WASSERKRAFT | GEOTHERMIE | GEZEITEN | KWKG\n\
                 - verguetungsform: UEBERSCHUSS (default) | VOLLEINSPEISUNG | KWK_ZUSCHLAG.\n\
                   The two solar forms differ by the §48 Abs. 2a bonus, so the rate lookup\n\
                   cannot answer without it.\n\
                 - sect9_erfuellung: FERNSTEUERBARKEIT | LEISTUNGSBEGRENZUNG_60 | KEINE.\n\
                   §9 is staged — from 100 kW only Fernsteuerbarkeit satisfies it, the\n\
                   25–100 kW band may take the 60 % Leistungsbegrenzung instead, below\n\
                   25 kW the cap alone is enough, and a Steckersolargerät under 2 kW is out\n\
                   of scope. KEINE is a §52 Abs. 1 Nr. 1 breach wherever §9 requires\n\
                   anything, at 10 €/kW/Monat, so say which route the plant took.\n\
                 - settlement_model, one of twelve (see the server instructions).\n\n\
                 ### Per model\n\
                 - DIREKTVERMARKTUNG: direktverm_aw_ct + direktverm_mp_id\n\
                 - AUSSCHREIBUNG: direktverm_aw_ct + ausschreibungs_zuschlag_id, plus\n\
                   zuschlag_erloeschen_datum where a §36e/§37e/§39e deadline applies —\n\
                   from that date the plant has no award left to settle against\n\
                 - AUSFALLVERGUETUNG: register the plant's **ordinary** rate. The engine\n\
                   applies the §53 Abs. 3 −20 %; supplying a pre-reduced rate double-counts it\n\
                 - MIETERSTROM: mieter_zuschlag_ct (§21 Abs. 3)\n\
                 - KWKG_ZUSCHLAG: kwk_foerderdauer_h (>2 MW, e.g. 30000) or\n\
                   kwk_foerderdauer_years (≤2 MW)\n\
                 - FLEXIBILITAET: flex_leistung_kw + flex_praemie_ct_kwh (§50b)\n\n\
                 ### Derived for you\n\
                 - foerderendedatum: §25 Abs. 1 — 20 years extended to 31 December of the\n\
                   twentieth year for a statutory AW, the exact anniversary for a tender\n\
                   award, the §8 Abs. 4 KWKG fifteen-year backstop for an hour-capped\n\
                   KWK plant\n\
                 - the §51 Negativpreisregel version, from the commissioning **date**\n\n\
                 Call lookup_verguetungssatz (erzeugungsart + verguetungsform + kWp + date)\n\
                 for the applicable rate before registering.",
            ),
        ]
    }

    #[prompt(
        name = "settle-monthly",
        description = "Run the monthly EEG/KWKG settlement and read the result"
    )]
    async fn settle_monthly_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I run the monthly EEG/KWKG settlement?"),
            PromptMessage::new_text(
                Role::Assistant,
                "## Monthly EEG/KWKG settlement\n\n\
                 1. Import the market reference: import_jahresmarktwert (§20 Abs. 2,\n\
                    technology-specific, published by the ÜNB) — it takes precedence over\n\
                    the generic import_epex_monthly_price.\n\
                 2. Load the EPEX day-ahead curve (PUT /api/v1/epex-spot) so §51 can be\n\
                    derived. Without it the plant is paid in full for a month §51 may well\n\
                    have excluded, and the run logs that it skipped.\n\
                 3. list_unsettled_plants (year, month) to preview.\n\
                 4. POST /api/v1/settle/{year}/{month} (dry_run first), or trigger_settle\n\
                    for one plant. Both take the same path as the monthly worker, so the\n\
                    amount does not depend on which one you use.\n\
                 5. explain_settlement (tr_id, year, month) to read the positions back,\n\
                    each with the provision that produced it.\n\n\
                 ## Formulas\n\
                 VERGUETUNG          kwh × verguetungssatz_ct / 100\n\
                 AUSFALLVERGUETUNG   kwh × (verguetungssatz_ct × 0,8) / 100  (§53 Abs. 3)\n\
                 MIETERSTROM / GGV   kwh × (verguetungssatz_ct + mieter_zuschlag_ct) / 100\n\
                 DIREKTVERMARKTUNG   max(0, AW − Marktwert) × kwh / 100\n\
                 AUSSCHREIBUNG       the same, on the tender-awarded AW\n\
                 POST_EEG_SPOT       kwh × Marktwert / 100\n\
                 KWKG_ZUSCHLAG       kwh × kwk_ct / 100, capped by the kWh hour limit\n\
                 FLEXIBILITAET       kwh × (verguetungssatz_ct + flex_praemie_ct) / 100\n\
                 FLEXIBILITAET_ZUSCHLAG  kW × rate_eur_per_kw / 12 — a capacity payment\n\
                 EIGENVERBRAUCH / SONSTIGE_DIREKTVERMARKTUNG  EUR 0\n\n\
                 There is **no additive Managementprämie**. Anlage 1 Nr. 3.1.2 defines the\n\
                 Marktprämie as `AW − Marktwert`, floored at zero, with the marketing cost\n\
                 already inside the AW.\n\n\
                 ## What else lands on the receipt\n\
                 §51 reduces the eligible kWh; §§53b–54 cut the AW before the formula;\n\
                 §52 Abs. 1 violations are charged separately as a Pflichtzahlung and are\n\
                 never netted into settlement_eur; §13a EnWG curtailment is added on top.\n\n\
                 CloudEvents: de.eeg.verguetung.berechnet and de.eeg.marktpraemie.berechnet\n\
                 carry the Gutschrift number, the USt and the bank details accountingd needs\n\
                 for the pain.001. Only a `calculated` run emits one.",
            ),
        ]
    }

    #[prompt(
        name = "check-foerderung-expiry",
        description = "Find plants nearing their Förderende and plan the transition"
    )]
    async fn check_foerderung_expiry_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Which plants are approaching their Förderungsende?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## Förderende pipeline\n\n\
                 list_expiring (days=365) for the annual view, (days=180) for the urgent one.\n\
                 A background worker emits de.eeg.anlage.foerderung-auslaufend **once per\n\
                 plant** inside the 180-day window — not once per sweep — and a repowering\n\
                 re-arms it.\n\n\
                 The EEG sets no advance-notice period for this; it is an operational\n\
                 courtesy and a commercial deadline, not a statutory one.\n\n\
                 Two things move the effective end date, and neither changes the stored\n\
                 foerderendedatum:\n\
                 - §51a: quarter-hours §51 reduced to null extend the Vergütungszeitraum.\n\
                   Before the Solarspitzengesetz the claim existed only for\n\
                   ausschreibungspflichtige Anlagen.\n\
                 - §36e/§37e/§39e: a lapsed Zuschlag ends the settlement early.\n\n\
                 ## Options at the end\n\
                 A. POST_EEG_SPOT — paid the market value. No paperwork.\n\
                 B. EIGENVERBRAUCH — self-consumption, no grid payment.\n\
                 C. DIREKTVERMARKTUNG — a new Direktvermarkter and AW. Use\n\
                    POST /switch-veraeusserungsform, which enforces the §21b/§21c timing.\n\
                 D. Vollrepowering — a fresh Inbetriebnahme (§3 Nr. 30) restarts §25.\n\
                    POST /api/v1/anlagen/{tr_id}/repowering. Only for a full replacement of\n\
                    the generator; partial repowering leaves the original date governing.",
            ),
        ]
    }

    #[prompt(
        name = "ausschreibung-workflow",
        description = "Register and settle a BNetzA Ausschreibungsanlage (§22 EEG 2023)"
    )]
    async fn ausschreibung_workflow_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I register and settle a BNetzA Ausschreibungsanlage?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## Ausschreibungsanlage — §22 EEG 2023\n\n\
                 §22 is the *wettbewerbliche Ermittlung der Marktprämie*: the BNetzA\n\
                 determines both who is entitled and the anzulegender Wert. Above the\n\
                 technology threshold a plant must tender — Wind an Land above 750 kW\n\
                 (§22 Abs. 2), Solaranlagen des ersten Segments above 1 MW (§22 Abs. 3).\n\n\
                 1. POST /api/v1/anlagen:\n\
                    settlement_model: AUSSCHREIBUNG\n\
                    direktverm_aw_ct: the awarded AW in ct/kWh\n\
                    ausschreibungs_zuschlag_id: the BNetzA Zuschlag reference\n\
                    zuschlag_erloeschen_datum: the §36e/§37e/§39e deadline, if one applies\n\
                    ist_innovationsausschreibung / ist_buergerenergie where they apply\n\n\
                 2. Settlement is the ordinary gleitende Marktprämie on the awarded AW:\n\
                    max(0, AW − Marktwert) × kwh / 100. No Managementprämie is added.\n\
                    Import the §20 Abs. 2 Marktwert first.\n\n\
                 3. Reductions that apply to tender plants specifically:\n\
                    §54 — the four defects of the Solar-erstes-Segment auction, recorded via\n\
                    POST /aw-reduktionen/sect54-defekt and cutting the AW before the formula.\n\
                    §53b does **not** apply: it reaches only a gesetzlich bestimmter AW.\n\
                    §51b — a Biogas Ausschreibungsanlage settles at zero whenever the market\n\
                    reference is ≤ 2 ct/kWh, and §51/§51a do not apply to it at all.\n\n\
                 4. Compliance: a plant that fails to register in MaStR owes the §52 Abs. 1\n\
                    Nr. 11 Pflichtzahlung of 10 €/kW/Monat — under EEG 2023 the Vergütung\n\
                    keeps flowing alongside it. Confirm with\n\
                    POST /api/v1/anlagen/{tr_id}/mastr-registrierung, which stops the clock.",
            ),
        ]
    }

    #[prompt(
        name = "post-eeg-transition",
        description = "Plan and execute the transition after the Förderdauer ends"
    )]
    async fn post_eeg_transition_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I transition a plant after its 20-year EEG Förderung ends?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## Post-EEG transition\n\n\
                 1. Pipeline: list_expiring (days=365).\n\
                 2. Check the **effective** end first: §51a may have extended it, and the\n\
                    plant keeps being paid through the extension. The stored\n\
                    foerderendedatum is the statutory one and does not move.\n\n\
                 ## Options\n\
                 A. POST_EEG_SPOT — paid the market value.\n\
                    PUT /api/v1/anlagen/{tr_id} settlement_model=POST_EEG_SPOT\n\
                 B. EIGENVERBRAUCH — self-consumption, no grid payment.\n\
                    Notify the NB via UTILMD (GPKE Lieferende).\n\
                 C. DIREKTVERMARKTUNG — a new Direktvermarkter contract.\n\
                    POST /switch-veraeusserungsform enforces §21b (first of a month, one\n\
                    switch per month) and §21c (the NB must be told before the start of the\n\
                    preceding calendar month, so the earliest reachable date is the first of\n\
                    the month after next). An unnotified switch is §52 Abs. 1 Nr. 9.\n\
                 D. Vollrepowering — §3 Nr. 30 i.V.m. §25: a fresh Inbetriebnahme restarts\n\
                    the Förderdauer, and the §51 regime is re-derived from the new date, so a\n\
                    plant repowered after 25.02.2025 falls under the Solarspitzengesetz rules.\n\
                    POST /api/v1/anlagen/{tr_id}/repowering {repowering_datum, leistung_kwp_neu}\n\
                 E. Zusammenlegung — §24, and only where the statute actually fuses the two;\n\
                    the endpoint answers 422 naming the rule that decided. It does not reset\n\
                    the parent's foerderendedatum.\n\n\
                 Keep the Marktstammdatenregister current after any change: the duty is the\n\
                 MaStRV i.V.m. §71 EEG, and the breach is §52 Abs. 1 Nr. 11.",
            ),
        ]
    }

    #[prompt(
        name = "anlagenerweiterung",
        description = "Model a §24 EEG Anlagenerweiterung or Zusammenlegung with capacity blocks"
    )]
    async fn anlagenerweiterung_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I handle an Anlagenerweiterung (§24 EEG) where a plant gets \
                 additional capacity with a newer, lower EEG rate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## §24 EEG 2023 — several plants treated as one\n\n\
                 §24 Abs. 1 Satz 1 deems plants one Anlage for the size-dependent rules —\n\
                 the tariff band, the tender threshold and the §51 size test — when **all**\n\
                 of the following hold, and none of the Sätze 2–5 carve-outs applies:\n\
                 1. the same Grundstück, Gebäude or Betriebsgelände, or unmittelbare\n\
                    räumliche Nähe;\n\
                 2. the same Energieträger (gleichartige erneuerbare Energien);\n\
                 3. a claim that depends on size at all;\n\
                 4. commissioning within twelve consecutive calendar months.\n\n\
                 Ownership is deliberately not a criterion — Satz 1 says *unabhängig von den\n\
                 Eigentumsverhältnissen*. The carve-outs matter: building solar behind\n\
                 different Netzverknüpfungspunkte is not one plant (Satz 4), and biogas from\n\
                 the same Biogaserzeugungsanlage is fused regardless of Satz 1 (Satz 2).\n\n\
                 ### Two ways to model it\n\
                 **A. Zusammenlegung** — one surviving entity.\n\
                   POST /api/v1/anlagen/{child_tr_id}/zusammenlegen {parent_tr_id}\n\
                   The endpoint evaluates §24 Abs. 1 in full and answers 422 naming the rule\n\
                   that decided, rather than merging on request: a merge the statute does not\n\
                   support moves the survivor into a tariff band it never qualified for, for\n\
                   the rest of its Förderdauer, and nothing downstream can tell that apart\n\
                   from a legitimate merge. The parent's foerderendedatum is not reset.\n\n\
                 **B. Capacity blocks** — keep each block's own rate and Förderende.\n\
                   The settlement allocates the month's kWh proportionally by capacity,\n\
                   applies each block's own §51 regime (a block added after 25.02.2025 is\n\
                   governed by the Solarspitzengesetz even when the primary block is not),\n\
                   and drops a block whose Förderende has passed. The §51 **size** test runs\n\
                   on the aggregated capacity — §51 Abs. 2 Satz 2 applies §24 to it — so\n\
                   splitting a plant into blocks cannot buy the exemption.\n\n\
                 Use lookup_statutory_rate for the band the combined capacity lands in.",
            ),
        ]
    }
}

// ── ServerHandler ─────────────────────────────────────────────────────────────

#[tool_handler]
#[prompt_handler]
impl ServerHandler for EinsdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("einsd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "einsd MCP — Einspeiser Registry + EEG/KWKG Settlement daemon (Netzbetreiber side).\n\n\
             ## Settlement models (12, one token each — no aliases)\n\
             VERGUETUNG (§21 Abs. 1) | AUSFALLVERGUETUNG (§21 Abs. 1 Satz 1 Nr. 3, AW −20 %) |\n\
             MIETERSTROM (§21 Abs. 3) | GGV (§42b EnWG) | DIREKTVERMARKTUNG (§20 gleitende\n\
             Marktprämie) | AUSSCHREIBUNG (§22) | SONSTIGE_DIREKTVERMARKTUNG (§21a) |\n\
             POST_EEG_SPOT | EIGENVERBRAUCH | KWKG_ZUSCHLAG (§7 KWKG 2023) |\n\
             FLEXIBILITAET (§50b) | FLEXIBILITAET_ZUSCHLAG (§50a, capacity-based)\n\n\
             ## Tools (19)\n\
             Registry: list_plants, get_plant, list_expiring, list_settlements, list_unsettled_plants\n\
             Rates: lookup_verguetungssatz (DB), lookup_statutory_rate (static tables)\n\
             Market data: get_epex_monthly_price, import_epex_monthly_price,\n\
             get_jahresmarktwert, import_jahresmarktwert\n\
             Settlement: trigger_settle\n\
             Compliance: get_compliance_status, list_plants_without_mastr,\n\
             check_direktvermarktung_compliance, check_sect44b_quota\n\
             Audit: explain_settlement, get_settlement_state_history, get_aw_reduktionen\n\n\
             Money and energy cross this surface as exact decimals — send them as strings\n\
             (\"8.11\"). They end up on a §14 UStG Gutschrift and must not pass through f64.\n\n\
             ## Rules worth knowing before you act\n\
             §51 Negativpreisregel is keyed on the plant's **Inbetriebnahmedatum**, not its\n\
             eeg_gesetz year: the Solarspitzengesetz rewrote it on 25.02.2025, mid-year and\n\
             inside the EEG 2023 range. A plant from 2024 has the staged 4-3-2-1-hour rule\n\
             and a 400 kW exemption; one from mid-2025 loses payment from the first negative\n\
             quarter-hour above 100 kW (2 kW pending the BNetzA Festlegung).\n\
             §51a extends the Vergütungszeitraum for what §51 took — but before the\n\
             Solarspitzengesetz only for ausschreibungspflichtige Anlagen.\n\
             §20/Anlage 1: the Marktprämie is `max(0, AW − Marktwert)`. There is **no**\n\
             additive Managementprämie.\n\
             §9 Steuerbarkeit is staged by capacity; the 25–100 kW band may satisfy it with\n\
             the 60 % Leistungsbegrenzung, and charging those plants §52 Abs. 1 Nr. 1 is wrong.\n\
             §52 Abs. 1 violations are a Pflichtzahlung *alongside* the Vergütung under EEG\n\
             2023 (Abs. 2 10 €/kW, Abs. 3 2 € on remedy, Abs. 5 cap); under the pre-2023\n\
             regime the breach reduces the Vergütung itself and no Pflichtzahlung exists.\n\
             §44b caps a Biogas plant above 100 kW at 45 % Bemessungsleistung — measured\n\
             against the actual hours of the calendar year (8 784 in a leap year), less the\n\
             hours before first generation.\n\
             §13a EnWG curtailment is compensated as a separate position and is not touched\n\
             by §51 — those kWh were never fed in.\n\n\
             Workflow: lookup_verguetungssatz → POST /api/v1/anlagen → import_jahresmarktwert\n\
             → PUT /api/v1/epex-spot (so §51 can be derived) → trigger_settle or\n\
             POST /api/v1/settle/{y}/{m} → explain_settlement.",
        )
    }
}

// ── Auth middleware + router ──────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<EinsdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

/// Build the MCP `Router`. Merge into the main axum app at `/mcp`.
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
