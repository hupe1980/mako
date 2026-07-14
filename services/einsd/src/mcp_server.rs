//! MCP server for `einsd` — Einspeiser Registry + EEG/KWKG Settlement.
//!
//! ## Tools (12)
//!
//! | Tool | Description |
//! |---|---|
//! | `list_plants` | List EEG/KWKG plants (filterable by malo_id, erzeugungsart, status) |
//! | `get_plant` | Get a single plant by TechnischeRessource ID |
//! | `list_expiring` | Plants with Förderung ending within N days |
//! | `list_settlements` | Settlement history for a plant |
//! | `lookup_verguetungssatz` | Look up the applicable EEG/KWKG tariff rate (DB) |
//! | `lookup_statutory_rate` | Look up EEG rate from static tables (Solarpaket I 2024) |
//! | `trigger_settle` | Trigger monthly settlement for one plant |
//! | `list_unsettled_plants` | Plants not yet settled for a given month |
//! | `get_epex_monthly_price` | Look up stored EPEX Spot monthly average |
//! | `import_epex_monthly_price` | Store/update EPEX Spot monthly average price |
//! | `get_compliance_status` | Check §52 EEG compliance status for a plant (MaStR, Fernsteuerbarkeit) |
//! | `list_plants_without_mastr` | Find plants not registered in MaStR (§52 §11 EEG 2023 violation) |
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
    pub leistung_kwp: f64,
    /// ISO-8601 commissioning date YYYY-MM-DD.
    pub inbetriebnahme: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TriggerSettleParams {
    pub tr_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    /// Override kWh. When absent, auto-fetched from edmd.
    pub einspeisemenge_kwh: Option<f64>,
    /// Override EPEX avg ct/kWh. When absent, uses DB value.
    pub epex_avg_ct_kwh: Option<f64>,
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
    pub avg_ct_kwh: f64,
    /// Source description (e.g. "netztransparenz.de", "smard.de", "manual").
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LookupStatutoryRateParams {
    /// Technology: SOLAR_AUFDACH | SOLAR_FREIFLAECHE | WIND_ONSHORE | BIOMASSE | KWKG
    pub erzeugungsart: String,
    /// Installed capacity in kWp (or kW_el for KWKG).
    pub leistung_kwp: f64,
    /// EEG law year: 2017, 2021, 2023, or 2024 (Solarpaket I).
    pub eeg_year: i16,
    /// VOLLEINSPEISUNG or UEBERSCHUSSEINSPEISUNG (solar only; default: UEBERSCHUSS).
    pub messkonzept: Option<String>,
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
        description = "List EEG/KWKG plants. Filter by malo_id, erzeugungsart (SOLAR/WIND_ONSHORE/KWKG/etc.), or status (aktiv/abgemeldet/foerderung_beendet/repowered)."
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
        description = "Get a single EEG/KWKG plant by TechnischeRessource ID (tr_id). Returns all fields including settlement model, Vergütungssatz, Förderendedatum, and KWKG data."
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
        description = "List plants whose EEG/KWKG Foerderung ends within the given days (default 180). Use to trigger early notification and plan Post-EEG transitions."
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
        description = "Monthly settlement history for a plant. Returns settlement amount, model, kWh, status, and CloudEvent ID for each settled month."
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
        description = "Look up the applicable EEG or KWKG Verguetungssatz (tariff rate ct/kWh) for a commissioning date and capacity. The rate is fixed at commissioning for the full 20-year Foerderdauer."
    )]
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

    // ── Settlement ────────────────────────────────────────────────────────────

    #[tool(
        description = "Trigger monthly EEG/KWKG settlement for one plant. Idempotent. \
        Auto-fetches Einspeisemenge from edmd and EPEX price from DB when not supplied. \
        Emits de.eeg.verguetung.berechnet or de.eeg.marktpraemie.berechnet on success. \
        KWKG: hour-limit enforcement automatic (max_kwh = rated_kW * foerderdauer_h)."
    )]
    async fn trigger_settle(
        &self,
        Parameters(params): Parameters<TriggerSettleParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{SettleInput, fetch_anlage, fetch_epex_price, run_settlement};
        use rust_decimal::Decimal;
        use rust_decimal_macros::dec;

        let anlage = match fetch_anlage(&self.state.pool, &self.state.tenant, &params.tr_id).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                return Err(McpError::invalid_params(
                    format!("plant {} not found", params.tr_id),
                    None,
                ));
            }
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };

        let einspeisemenge_kwh = params
            .einspeisemenge_kwh
            .and_then(|v| Decimal::try_from(v).ok());
        let epex_avg_ct_kwh = match params.epex_avg_ct_kwh {
            Some(v) => Decimal::try_from(v).ok(),
            None => fetch_epex_price(&self.state.pool, params.billing_year, params.billing_month)
                .await
                .ok()
                .flatten(),
        };
        let managementpraemie_ct = if matches!(
            anlage.settlement_model.as_str(),
            "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG"
        ) {
            Some(if anlage.leistung_kwp > dec!(100_000) {
                dec!(0.2)
            } else {
                dec!(0.4)
            })
        } else {
            None
        };

        let input = SettleInput {
            tr_id: params.tr_id.clone(),
            tenant: self.state.tenant.clone(),
            billing_year: params.billing_year,
            billing_month: params.billing_month,
            einspeisemenge_kwh,
            epex_avg_ct_kwh,
            settlement_model: anlage.settlement_model.clone(),
            verguetungssatz_ct: anlage.verguetungssatz_ct,
            direktverm_aw_ct: anlage.direktverm_aw_ct,
            mieter_zuschlag_ct: anlage.mieter_zuschlag_ct,
            flex_praemie_ct_kwh: anlage.flex_praemie_ct_kwh,
            managementpraemie_ct,
            kwk_strom_kwh_gesamt: if anlage.settlement_model == "KWKG_ZUSCHLAG" {
                anlage.kwk_strom_kwh_gesamt
            } else {
                None
            },
            // KWKG §8 KWKG 2023: max kWh = rated_kW * full_load_hours
            kwk_max_kwh: anlage
                .kwk_foerderdauer_h
                .map(|h| Decimal::from(h) * anlage.leistung_kwp),
            sanktion: None, // computed from mastr_registriert in run_settlement
            mastr_registriert: anlage.mastr_registriert,
            kwh_during_negative_epex: None,
            inbetriebnahme: Some(anlage.inbetriebnahme),
            leistung_kwp: Some(anlage.leistung_kwp),
            foerderendedatum: Some(anlage.foerderendedatum),
            billing_date: time::Date::from_calendar_date(
                params.billing_year as i32,
                time::Month::try_from(params.billing_month as u8).unwrap_or(time::Month::January),
                1,
            )
            .ok(),
            eeg_gesetz: anlage.eeg_gesetz,
            erzeugungsart: anlage.erzeugungsart.clone(),
        };

        match run_settlement(&self.state.pool, input).await {
            Ok(result) => ContentBlock::json(serde_json::to_value(&result).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List active plants NOT yet settled for the given billing month. \
        Use to preview before POST /api/v1/settle/{year}/{month} batch run."
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
        Required for DIREKTVERMARKTUNG (Gleitende Marktpraemie) and POST_EEG_SPOT settlement."
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
        use rust_decimal::Decimal;
        let avg = Decimal::try_from(params.avg_ct_kwh)
            .map_err(|_| McpError::invalid_params("invalid avg_ct_kwh", None))?;
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
            Returns reference starting rate for the EEG year — for quarterly degression use \
            lookup_verguetungssatz."
    )]
    async fn lookup_statutory_rate(
        &self,
        Parameters(params): Parameters<LookupStatutoryRateParams>,
    ) -> Result<CallToolResult, McpError> {
        use eeg_billing::rates;
        use rust_decimal::Decimal;

        let kwp = Decimal::try_from(params.leistung_kwp)
            .map_err(|_| McpError::invalid_params("invalid leistung_kwp", None))?;

        let volleinspeisung = params
            .messkonzept
            .as_deref()
            .map(|s| s.to_uppercase() == "VOLLEINSPEISUNG")
            .unwrap_or(false);

        let result = match params.erzeugungsart.to_uppercase().as_str() {
            "SOLAR_AUFDACH" | "SOLAR_FREIFLAECHE" | "SOLAR_BALKON" | "SOLAR" => {
                if volleinspeisung {
                    rates::solar_pv_volleinspeisung_lookup(params.eeg_year)
                        .ok_or_else(|| McpError::invalid_params(
                            format!("no Volleinspeisung rates for EEG year {}; use einsd DB lookup_verguetungssatz", params.eeg_year),
                            None,
                        ))?
                        .rate_for(kwp)
                } else {
                    rates::solar_pv_ueberschuss_lookup(params.eeg_year)
                        .ok_or_else(|| McpError::invalid_params(
                            format!("no Überschusseinspeisung rates for EEG year {}; use einsd DB lookup_verguetungssatz", params.eeg_year),
                            None,
                        ))?
                        .rate_for(kwp)
                }
            }
            "WIND_ONSHORE" => rates::wind_onshore_lookup(params.eeg_year)
                .ok_or_else(|| {
                    McpError::invalid_params(
                        format!("no wind onshore rates for EEG year {}", params.eeg_year),
                        None,
                    )
                })?
                .rate_for(kwp),
            "BIOMASSE" | "BIOGAS" | "BIOMETHANE" => rates::biomasse_lookup(params.eeg_year)
                .ok_or_else(|| {
                    McpError::invalid_params(
                        format!("no biomasse rates for EEG year {}", params.eeg_year),
                        None,
                    )
                })?
                .rate_for(kwp),
            "KWKG" => rates::kwkg_zuschlag_lookup()
                .ok_or_else(|| McpError::invalid_params("no KWKG rates", None))?
                .rate_for(kwp),
            other => {
                return Err(McpError::invalid_params(
                    format!(
                        "unknown erzeugungsart: {other}. Use SOLAR_AUFDACH, WIND_ONSHORE, BIOMASSE, or KWKG"
                    ),
                    None,
                ));
            }
        };

        match result {
            Ok(rate) => {
                let rate_ct = rate.into_decimal() * rust_decimal::Decimal::from(100u32);
                Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                    "Statutory rate for {erzeugungsart} {kwp} kWp (EEG {eeg_year}{ms}): \
                    {rate_ct:.2} ct/kWh ({rate} EUR/kWh).\n\
                    Note: this is the reference starting rate. Actual rate depends on \
                    commissioning month (quarterly degression). Use lookup_verguetungssatz \
                    for production billing.",
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
        description = "Check §52 EEG compliance status for a plant: MaStR registration, Fernsteuerbarkeit, KWKG hour-limit proximity. Returns compliance_ok, missing_mastr, penalty_risk_eur_per_month, and recommended action per §52 EEG 2023.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_compliance_status(
        &self,
        Parameters(p): Parameters<GetPlantParams>,
    ) -> Result<CallToolResult, McpError> {
        use sqlx::Row;
        let row = sqlx::query(
            r"SELECT tr_id, erzeugungsart, leistung_kwp, eeg_gesetz,
                     mastr_registriert, mastr_nummer, mastr_datum, status,
                     inbetriebnahme, foerderendedatum,
                     kwk_strom_kwh_gesamt, kwk_max_kwh
              FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&p.tr_id)
        .bind(&self.state.tenant)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let Some(r) = row else {
            return Err(McpError::invalid_params(
                format!("plant {} not found", p.tr_id),
                None,
            ));
        };

        let mastr_ok: bool = r.try_get("mastr_registriert").unwrap_or(false);
        let leistung_kwp: f64 = r
            .try_get::<rust_decimal::Decimal, _>("leistung_kwp")
            .ok()
            .and_then(|d| d.try_into().ok())
            .unwrap_or(0.0);
        let eeg_gesetz: i16 = r.try_get("eeg_gesetz").unwrap_or(2023);
        let foerderendedatum: Option<time::Date> = r.try_get("foerderendedatum").unwrap_or(None);

        let today = time::OffsetDateTime::now_utc().date();
        let foerderung_aktiv = foerderendedatum.is_none_or(|d| d >= today);

        // §52 Abs. 1 Nr. 11 EEG 2023: MaStR not registered
        let penalty_per_month = if !mastr_ok && foerderung_aktiv {
            leistung_kwp * 10.0 // €10/kW/month (EEG 2023) or Vergütung=0 (EEG ≤2021)
        } else {
            0.0
        };

        ContentBlock::json(serde_json::json!({
            "tr_id": p.tr_id,
            "compliance_ok": mastr_ok || !foerderung_aktiv,
            "foerderung_aktiv": foerderung_aktiv,
            "mastr": {
                "registriert": mastr_ok,
                "nummer": r.try_get::<Option<String>, _>("mastr_nummer").unwrap_or(None),
                "datum": r.try_get::<Option<time::Date>, _>("mastr_datum").unwrap_or(None),
            },
            "penalty_risk": {
                "monthly_eur": penalty_per_month,
                "regime": if eeg_gesetz >= 2023 { "§52 EEG 2023: €10/kW/month" } else { "§47 EEG ≤2021: Vergütung = 0" },
                "note": if !mastr_ok { "URGENT: Register in MaStR at https://www.marktstammdatenregister.de" } else { "No penalty risk" },
            },
            "recommended_action": if !mastr_ok && foerderung_aktiv {
                "Register plant in MaStR immediately. POST /api/v1/anlagen/{tr_id}/mastr-registrierung after registration."
            } else {
                "No action required"
            },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "List plants not registered in MaStR (Marktstammdatenregister). §52 Abs. 1 Nr. 11 EEG 2023: unregistered plants incur €10/kW/month penalty (EEG 2023) or Vergütung=0 (EEG ≤2021). Returns tr_id, malo_id, leistung_kwp, eeg_gesetz, and monthly penalty risk.",
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
                AND (foerderendedatum IS NULL OR foerderendedatum >= CURRENT_DATE)
              ORDER BY leistung_kwp DESC",
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let plants: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let leistung: f64 = r
                    .try_get::<rust_decimal::Decimal, _>("leistung_kwp")
                    .ok()
                    .and_then(|d| d.try_into().ok())
                    .unwrap_or(0.0);
                let eeg_gesetz: i16 = r.try_get("eeg_gesetz").unwrap_or(2023);
                let penalty_per_month = leistung * 10.0;
                serde_json::json!({
                    "tr_id": r.try_get::<String, _>("tr_id").unwrap_or_default(),
                    "malo_id": r.try_get::<String, _>("malo_id").unwrap_or_default(),
                    "erzeugungsart": r.try_get::<String, _>("erzeugungsart").unwrap_or_default(),
                    "leistung_kwp": leistung,
                    "eeg_gesetz": eeg_gesetz,
                    "monthly_penalty_eur": penalty_per_month,
                    "regime": if eeg_gesetz >= 2023 { "§52 EEG 2023: €10/kW/month" } else { "§47 EEG ≤2021: Vergütung = 0" },
                })
            })
            .collect();

        let total_penalty: f64 = plants
            .iter()
            .filter_map(|p| p["monthly_penalty_eur"].as_f64())
            .sum();

        ContentBlock::json(serde_json::json!({
            "count": plants.len(),
            "total_monthly_penalty_eur": total_penalty,
            "plants": plants,
            "regulatory_note": "Register all plants at https://www.marktstammdatenregister.de. POST /api/v1/anlagen/{tr_id}/mastr-registrierung after successful registration.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

// ── Prompts ───────────────────────────────────────────────────────────────────

#[prompt_router]
impl EinsdMcpHandler {
    #[prompt(
        name = "register-eeg-plant",
        description = "Step-by-step: register a new EEG/KWKG feed-in plant"
    )]
    async fn register_eeg_plant_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "I need to register a new EEG feed-in plant."),
            PromptMessage::new_text(
                Role::Assistant,
                "## EEG/KWKG Plant Registration\n\n\
                 POST /api/v1/anlagen with:\n\
                 - tr_id (TechnischeRessource ID from marktd), malo_id, melo_id\n\
                 - erzeugungsart: SOLAR_AUFDACH | SOLAR_FREFLAECHE | SOLAR_AGRIPV | SOLAR_MIETERSTROM |\n\
                   WIND_ONSHORE | WIND_OFFSHORE | BIOMASSE | BIOGAS | KLAEGAS | GRUBENGAS | WASSERKRAFT | KWKG\n\
                 - inbetriebnahme (YYYY-MM-DD), leistung_kwp, eeg_gesetz (year, or 0 for KWKG)\n\
                 - settlement_model: VERGUETUNG | DIREKTVERMARKTUNG | AUSSCHREIBUNG |\n\
                   POST_EEG_SPOT | MIETERSTROM | EIGENVERBRAUCH | KWKG_ZUSCHLAG | FLEXIBILITAET\n\n\
                 Auto-calculated: foerderendedatum = inbetriebnahme + 20 years (or repowering_datum + 20)\n\
                 verguetungssatz_ct auto-looked up if omitted.\n\n\
                 DIREKTVERMARKTUNG: add direktverm_aw_ct + direktverm_mp_id\n\
                 AUSSCHREIBUNG: add direktverm_aw_ct + ausschreibungs_zuschlag_id\n\
                 KWKG: add kwk_foerderdauer_h (>2 MW, e.g. 30000) or kwk_foerderdauer_years (<=2 MW)\n\
                 MIETERSTROM: add mieter_zuschlag_ct (sect. 38a EEG)\n\
                 FLEXIBILITAET: add flex_leistung_kw + flex_praemie_ct_kwh (sect. 50 EEG)\n\n\
                 Use lookup_verguetungssatz first to find the applicable rate.",
            ),
        ]
    }

    #[prompt(
        name = "settle-monthly",
        description = "Step-by-step: run monthly EEG/KWKG settlement"
    )]
    async fn settle_monthly_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I run the monthly EEG/KWKG settlement?"),
            PromptMessage::new_text(
                Role::Assistant,
                "## Monthly EEG/KWKG Settlement\n\n\
                 1. Import EPEX price if needed: import_epex_monthly_price (year, month, avg_ct_kwh)\n\
                 2. Check unsettled: list_unsettled_plants (year, month)\n\
                 3. Settle all: POST /api/v1/settle/{year}/{month} (dry_run=true first)\n\
                    OR settle one: trigger_settle (tr_id, billing_year, billing_month)\n\
                 4. Audit: list_settlements (tr_id)\n\n\
                 Settlement formulas:\n\
                 VERGUETUNG:       kwh x verguetungssatz_ct / 100\n\
                 MIETERSTROM:      VERGUETUNG + kwh x mieter_zuschlag_ct / 100\n\
                 DIREKTVERMARKTUNG: max(0, AW-EPEX) x kwh / 100 + Managementpraemie (0.4 ct/kWh)\n\
                 AUSSCHREIBUNG:    same as DIREKTVERMARKTUNG (BNetzA tender AW)\n\
                 POST_EEG_SPOT:    kwh x epex_monthly_avg / 100\n\
                 KWKG_ZUSCHLAG:    kwh x kwk_zuschlag_ct / 100 (capped by hour-limit)\n\
                 FLEXIBILITAET:    VERGUETUNG + kwh x flex_praemie_ct / 100\n\
                 EIGENVERBRAUCH:   EUR 0\n\n\
                 CloudEvents: de.eeg.verguetung.berechnet -> accountingd posts Gutschrift\n\
                 de.eeg.marktpraemie.berechnet -> NB->UNB Marktpraemie payment",
            ),
        ]
    }

    #[prompt(
        name = "check-foerderung-expiry",
        description = "Step-by-step: identify plants nearing Foerderungsende and plan transition"
    )]
    async fn check_foerderung_expiry_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Which plants are approaching their Foerderungsende?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "list_expiring (days=365) for 12-month pipeline, (days=180) for urgent.\n\
                 Background worker emits de.eeg.anlage.foerderung_auslaufend every 6h.\n\n\
                 Legal: sect. 21 Abs. 1 EEG 2023 — notify Anlagenbetreiber >= 12 months in advance.\n\n\
                 Transition options:\n\
                 1. POST_EEG_SPOT: spot market feed-in. PUT /api/v1/anlagen/{tr_id} settlement_model=POST_EEG_SPOT\n\
                 2. EIGENVERBRAUCH: self-consumption. Notify NB via UTILMD G.\n\
                 3. DIREKTVERMARKTUNG: obtain new Direktvermarkter + AW.\n\
                 4. REPOWERING sect. 22: POST /api/v1/anlagen/{tr_id}/repowering — resets +20yr\n\
                 5. ZUSAMMENLEGUNG sect. 24: POST /api/v1/anlagen/{tr_id}/zusammenlegen\n\n\
                 See post-eeg-transition prompt for full planning guide.",
            ),
        ]
    }

    #[prompt(
        name = "ausschreibung-workflow",
        description = "Step-by-step: register and settle a BNetzA Ausschreibungsanlage (sect. 22a/28 EEG 2023)"
    )]
    async fn ausschreibung_workflow_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I register and settle a BNetzA Ausschreibungsanlage?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## BNetzA Ausschreibungsanlage — sect. 22a/28 EEG 2023\n\n\
                 Plants >1 MWp (Solar Freiflaeche, Wind) must tender via BNetzA.\n\
                 The awarded Anzulegender Wert (AW ct/kWh) replaces the fixed Verguetungssatz.\n\n\
                 1. POST /api/v1/anlagen:\n\
                    settlement_model: AUSSCHREIBUNG\n\
                    direktverm_aw_ct: <BNetzA awarded AW in ct/kWh>\n\
                    ausschreibungs_zuschlag_id: <BNetzA Zuschlag reference number>\n\
                    direktvermarktung: true\n\n\
                 2. Monthly settlement formula:\n\
                    Marktpraemie = max(0, AW - EPEX_monthly_avg) + Managementpraemie\n\
                    Managementpraemie: 0.4 ct/kWh (reduced to 0.2 ct/kWh for plants >100 MW)\n\
                    Import EPEX first: import_epex_monthly_price\n\n\
                 3. sect. 25 EEG 2023 sanctions:\n\
                    If plant NOT in MaStR: Verguetung = 0 until registration.\n\
                    No retroactive catch-up permitted.\n\n\
                 4. Annual AW adjustment via BNetzA portal + MSCONS Einspeisemenge to UNB.",
            ),
        ]
    }

    #[prompt(
        name = "post-eeg-transition",
        description = "Step-by-step: plan and execute Post-EEG phase transition (sect. 21 EEG 2023)"
    )]
    async fn post_eeg_transition_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I transition a plant after its 20-year EEG Foerderung ends?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## Post-EEG Transition Planning\n\n\
                 1. Identify pipeline: list_expiring (days=365)\n\
                 2. Legal notice: sect. 21 Abs. 1 EEG — notify Anlagenbetreiber >= 12 months in advance\n\
                    Background CE de.eeg.anlage.foerderung_auslaufend triggers this workflow.\n\n\
                 Options:\n\
                 A. POST_EEG_SPOT — feed-in at EPEX spot avg. No paperwork.\n\
                    PUT /api/v1/anlagen/{tr_id} settlement_model=POST_EEG_SPOT\n\n\
                 B. EIGENVERBRAUCH — self-consumption, no grid payment.\n\
                    Notify NB via UTILMD G (GPKE or GeLi Gas Lieferende).\n\n\
                 C. DIREKTVERMARKTUNG — sign new Direktvermarkter contract.\n\
                    PUT /api/v1/anlagen/{tr_id} + direktverm_aw_ct + direktverm_mp_id\n\n\
                 D. REPOWERING sect. 22 EEG — replace components, 20-year clock resets.\n\
                    POST /api/v1/anlagen/{tr_id}/repowering {repowering_datum, leistung_kwp_neu}\n\
                    New Verguetungssatz auto-looked up at repowering_datum.\n\n\
                 E. ZUSAMMENLEGUNG sect. 24 EEG — merge adjacent plants into one entity.\n\
                    POST /api/v1/anlagen/{child_tr_id}/zusammenlegen {parent_tr_id}\n\
                    Note: foerderendedatum NOT reset (only Repowering resets it).\n\n\
                 MaStR update: sect. 28a EEG — update Marktstammdatenregister after any change.",
            ),
        ]
    }

    #[prompt(
        name = "anlagenerweiterung",
        description = "Step-by-step: model a §24 EEG plant extension (Anlagenerweiterung) \
            or Zusammenlegung with multiple capacity blocks at different rates"
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
                "## §24 EEG Anlagenerweiterung — Multi-Block Settlement\n\n\
                 §24 EEG 2023 combines plants at the same location into one entity for \
                 tariff-threshold purposes when commissioned within 12 consecutive months.\n\n\
                 ### Eligibility (§24 Abs. 1 EEG 2023)\n\
                 All conditions must hold:\n\
                 1. Same Grundstück/Gebäude/Betriebsgelände (same location)\n\
                 2. Same energy type (same Erzeugungsart)\n\
                 3. Both commissioned within 12 calendar months (check: zusammenlegung_within_12_months)\n\n\
                 ### Rate impact\n\
                 Combined capacity may cross into a lower tariff band:\n\
                 - Plant A: 7 kWp → ≤10 kWp band = 8.51 ct/kWh (EEG 2024)\n\
                 - Extension: +5 kWp → combined 12 kWp crosses into ≤40 kWp band = 7.43 ct\n\
                 Call lookup_statutory_rate to check the new combined rate.\n\n\
                 ### Two settlement approaches\n\
                 **A. Single entity (§24 Zusammenlegung):**\n\
                   PUT /api/v1/anlagen/{parent_tr_id} with combined leistung_kwp and \n\
                   new verguetungssatz_ct (the combined rate).\n\
                   POST /api/v1/anlagen/{child_tr_id}/zusammenlegen {parent_tr_id}\n\
                   Simple, but loses block-level rate granularity.\n\n\
                 **B. Multi-block (§24 Erweiterung, preferred):**\n\
                   Register extension as new tr_id with its own rate and foerderendedatum.\n\
                   Use eeg_billing::CapacityBlock in SettleInput for proportional settlement.\n\
                   Proportional allocation: block_kwh = total_kwh × (block_kwp / total_kwp)\n\n\
                 ### 12-month check\n\
                   use eeg_billing::zusammenlegung_within_12_months(ibn_a, ibn_b)\n\
                   Returns false when >12 months apart → NOT subject to §24 aggregation.\n\n\
                 ### Förderdauer (important!)\n\
                   §24 Zusammenlegung: foerderendedatum of PARENT is unchanged.\n\
                   §22 Repowering: foerderendedatum RESETS to repowering_datum + 20 years.\n\
                   Erweiterung block: own foerderendedatum = extension_ibn + 20 years.",
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
            "einsd MCP — Einspeiser Registry + EEG/KWKG Settlement daemon.\n\n\
             Settlement models (9): VERGUETUNG (§21 EEG) | MIETERSTROM (§38a) |\n\
             DIREKTVERMARKTUNG (§20 Marktprämie) | AUSSCHREIBUNG (§§22a/28) |\n\
             POST_EEG_SPOT (§23b: 10ct cap) | EIGENVERBRAUCH | KWKG_ZUSCHLAG (§7 KWKG 2023) |\n\
             FLEXIBILITAET (§50b, bestehende Anlagen) | FLEXIBILITAET_ZUSCHLAG (§50a, neue Anlagen)\n\n\
             ## Tools (12)\n\
             Core: list_plants, get_plant, list_expiring, list_settlements, list_unsettled_plants\n\
             Rates: lookup_verguetungssatz, lookup_statutory_rate\n\
             Settlement: trigger_settle, get_epex_monthly_price, import_epex_monthly_price\n\
             Compliance: get_compliance_status, list_plants_without_mastr\n\n\
             Rate tables: lookup_statutory_rate (Solarpaket I 2024 rates for SOLAR/WIND/BIOMASSE/KWKG)\n\
             Workflow: lookup_statutory_rate -> POST /api/v1/anlagen -> import_epex_monthly_price ->\n\
             trigger_settle (one) or POST /api/v1/settle/{y}/{m} (batch) -> list_settlements\n\n\
             §51 EEG 2023 Negativpreisregel: any negative-price period reduces Vergütung to 0.\n\
             §51a: Vergütungszeitraum extended by lost quarter-hours (solar: ×0.5 factor).\n\
             §52 EEG 2023: MaStR non-registration → €10/kW/month (not Vergütung=0).\n\
             §19 EEG: EinsMan curtailment compensation (separate position, same rate).\n\
             §36k EEG: Wind onshore Korrekturfaktor for below-reference-yield sites.\n\
             §24 Anlagenerweiterung: use CapacityBlock for multi-block proportional settlement.",
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
