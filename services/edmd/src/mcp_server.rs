//! MCP (Model Context Protocol) server for `edmd`.
//!
//! Exposes meter data time-series and billing-period summaries.
//! Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Tools (15)
//!
//! | Tool | Description |
//! |---|---|
//! | `get_timeseries`             | Read meter data for a MaLo in a time range |
//! | `get_imbalance`              | Mehr-/Mindermengen imbalance report |
//! | `get_billing_period`         | MeterBillingPeriod summary (arbeitsmenge, spitzenleistung, brennwert) |
//! | `get_device_history`         | RAG: comprehensive device history for LanceDB indexing |
//! | `get_quality_warnings`       | Hampel quality warnings (grade A/B/C/F) |
//! | `list_reading_orders`        | List Ablesesteuerung reading orders for a MaLo |
//! | `list_overdue_reading_orders`| Reading orders past `ausfuehrt_bis` (§ 40b Abs. 1 EnWG compliance) |
//! | `trigger_jahresablesung`     | Launch Jahresablesung campaign for a NB grid area |
//! | `trigger_substitution`       | Generate + store § 60 Abs. 2 MsbG Ersatzwerte for a gap window |
//! | `get_correction_history`     | § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit: list corrections for a MaLo |
//! | `validate_timeseries`        | Run the metering validation rules (V01–V09/V11/V12) on meter reads |
//! | `get_quality_assessments`    | Per-batch quality history (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD)) |
//! | `get_summenzeitreihe`        | Monthly aggregated kWh for MaBiS |
//! | `get_annual_forecast`        | § 60 Abs. 2 MsbG Jahresprognose |
//! | `get_gas_quality`            | PID 13007 Brennwert + Zustandszahl |
//!
//! ## Prompts (5)
//!
//! | Prompt | Description |
//! |---|---|
//! | `analyze-consumption`      | Step-by-step consumption analysis |
//! | `submit-mscons`            | Step-by-step MSCONS ingestion guide |
//! | `quality-assessment`       | Hampel quality assessment guide |
//! | `jahresablesung-workflow`  | § 40b Abs. 1 EnWG Jahresablesung campaign guide |
//! | `reading-order-lifecycle`  | Reading order lifecycle: OFFEN → AUSGEFUEHRT |

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
use sqlx::{PgPool, Row};
use tokio_util::sync::CancellationToken;

use crate::domain::TimeSeriesQuery;
// Brings the `query`/`store_reads` methods on `MeterStoreTimeSeriesRepository`
// into scope for the tools that read the version-resolved series.
use crate::domain::repository::TimeSeriesRepository;

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EdmdMcpState {
    pub pool: PgPool,
    /// meterstore-backed time-series repository. The readings live in meterstore
    /// (the resolved `meter_reads` relation is DataFusion-only, not a Postgres
    /// table), so every time-series read goes through this repo, never raw SQL
    /// against `pool`.
    pub repo: crate::store::MeterStoreTimeSeriesRepository,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
    /// `marktd` base URL — the Jahresablesung campaign enumerates SLP MaLos
    /// from it, so a tool that creates reading orders needs the same access the
    /// HTTP endpoint has.
    pub marktd_url: String,
    /// `marktd` bearer token.
    pub marktd_api_key: secrecy::SecretString,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTimeseriesParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// ISO 8601 start of the query range (inclusive).
    pub from: String,
    /// ISO 8601 end of the query range (inclusive).
    pub to: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetImbalanceParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Year (e.g. 2026).
    pub year: i32,
    /// Month (1–12).
    pub month: u8,
    /// The bilanzierte (profile-allocated) quantity for the month, in kWh.
    ///
    /// Required: edmd holds the measured half of the saldo only. The bilanzierte
    /// Menge comes from the Bilanzkreisabrechnung, not from metering data.
    pub bilanziert_kwh: rust_decimal::Decimal,
    /// `STROM` (default) or `GAS` — Gas balances on the 06:00 Gastag.
    #[serde(default)]
    pub sparte: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetBillingPeriodParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Start of billing period — ISO 8601 date `YYYY-MM-DD` (inclusive).
    pub period_from: String,
    /// End of billing period — ISO 8601 date `YYYY-MM-DD` (inclusive).
    /// Defaults to `period_from` (single-day period) when omitted.
    pub period_to: Option<String>,
}

// ── Device history RAG ─────────────────────────────────────────────────────

/// Parameters for `get_device_history` MCP tool (LanceDB MSB service history RAG).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetDeviceHistoryParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// How many days back to look (default 90).
    pub days_back: Option<i64>,
}

// ── Quality warnings ───────────────────────────────────────────────────────

/// Parameters for `get_quality_warnings` MCP tool (Hampel filter quality scoring).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetQualityWarningsParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// ISO-8601 start datetime (inclusive). Defaults to 30 days ago when omitted.
    pub from: Option<String>,
    /// ISO-8601 end datetime (exclusive). Defaults to now when omitted.
    pub to: Option<String>,
}

// ── Reading orders ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListReadingOrdersParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Filter by status: OFFEN | BEAUFTRAGT | AUSGEFUEHRT | STORNIERT | FEHLGESCHLAGEN.
    pub status: Option<String>,
    /// Filter by Anlass: JAHRESABLESUNG | ZWISCHENABLESUNG | LIEFERBEGINN | SONDERABLESUNG | …
    pub anlass: Option<String>,
    /// Max results (default 50, max 500).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListOverdueReadingOrdersParams {
    /// Filter by NB/MSB MP-ID (optional).
    pub ausfuehrender_msb: Option<String>,
    /// Max results (default 100).
    pub limit: Option<i64>,
}

/// Parameters for `trigger_substitution`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TriggerSubstitutionParams {
    /// 11-digit Marktlokations-ID the gap belongs to.
    pub malo_id: String,
    /// Gap start (UTC, RFC3339).
    pub gap_from: String,
    /// Gap end (UTC, RFC3339).
    pub gap_to: String,
    /// `PriorPeriodAverage` (default) · `LinearInterpolation` · `ZeroFill` ·
    /// `LastValueCarryForward`.
    pub method: Option<String>,
    /// Interval length in seconds (default 900; use 3600 for hourly gas).
    pub interval_secs: Option<u32>,
    /// Reference read window in days (default 7). `PriorPeriodAverage` averages
    /// the matching slot over the 7 days before the gap and ignores anything
    /// past that; a larger value only widens the search for the bracketing
    /// values the other methods use.
    pub prior_days: Option<u32>,
    /// OBIS register the gap belongs to (part of the reading identity).
    pub obis_code: Option<String>,
    /// `STROM` (default) · `GAS` · `WAERME` · `WASSER`.
    pub sparte: Option<String>,
    /// § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit reason (default `NoMeasurementAvailable`).
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TriggerJahresablesungParams {
    /// NB MP-ID (BDEW-Codenummer) for the grid area.
    pub nb_mp_id: String,
    /// Campaign year (default: current year).
    pub campaign_year: Option<i32>,
    /// Dry-run: report what a run would do without creating reading orders.
    pub dry_run: Option<bool>,
    /// Cap on MaLos enumerated in one call (default 2000, max 10000).
    ///
    /// Bounded so the tool cannot exceed an MCP client's request timeout on a
    /// large grid. Re-running is idempotent, so a capped run is resumed by
    /// calling again rather than by starting over.
    pub max_malos: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCorrectionHistoryParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// ISO 8601 start of range (default: 90 days ago).
    pub from: Option<String>,
    /// ISO 8601 end of range (default: now).
    pub to: Option<String>,
    /// Maximum records to return (default: 100, max: 1000).
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateTimeseriesParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// ISO 8601 start of validation window.
    pub from: String,
    /// ISO 8601 end of validation window.
    pub to: String,
    /// Expected interval length in seconds (default: 900 = 15 min).
    pub interval_secs: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetQualityAssessmentsParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// ISO 8601 start (optional, defaults to epoch).
    pub from: Option<String>,
    /// ISO 8601 end (optional, defaults to now).
    pub to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSummenzeitreiheParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// ISO 8601 start (optional).
    pub from: Option<String>,
    /// ISO 8601 end (optional).
    pub to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetAnnualForecastParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// ISO 8601 start of observation window (optional, defaults to epoch).
    pub from: Option<String>,
    /// ISO 8601 end of observation window (optional, defaults to now).
    pub to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetGasQualityParams {
    /// 11-digit Marktlokations-ID (Gas MaLo).
    pub malo_id: String,
}

// ── MCP handler ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EdmdMcpHandler {
    state: Arc<EdmdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<EdmdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<EdmdMcpHandler>,
}

#[tool_router]
impl EdmdMcpHandler {
    fn new(state: Arc<EdmdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    /// Read meter data time-series for a MaLo in a given date-time range.
    ///
    /// Returns an array of `Messwert` records (dtm_from, dtm_to, value,
    /// unit, bo4e_version) ordered by `dtm_from` ascending.  Empty array
    /// means no MSCONS data has been received yet for this MaLo and period.
    #[tool(
        description = "Read meter data (Messwert) for a MaLo between from..to (ISO 8601)",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_timeseries(
        &self,
        Parameters(p): Parameters<GetTimeseriesParams>,
    ) -> Result<CallToolResult, McpError> {
        use time::format_description::well_known::Rfc3339;

        let from = time::OffsetDateTime::parse(&p.from, &Rfc3339).map_err(|_| {
            McpError::invalid_params("from is not a valid ISO 8601 timestamp", None)
        })?;
        let to = time::OffsetDateTime::parse(&p.to, &Rfc3339)
            .map_err(|_| McpError::invalid_params("to is not a valid ISO 8601 timestamp", None))?;

        let reads = self
            .state
            .repo
            .query(&TimeSeriesQuery {
                malo_id: p.malo_id.clone(),
                from,
                to,
                sparte: None,
                tenant: self.state.tenant.clone(),
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let readings: Vec<serde_json::Value> = reads
            .iter()
            .map(|r| {
                serde_json::json!({
                    "dtm_from": r.dtm_from,
                    "dtm_to": r.dtm_to,
                    "quantity_kwh": r.quantity_kwh.to_string(),
                    "quality": r.quality.as_str(),
                    "obis_code": r.obis_code,
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "from": p.from,
            "to": p.to,
            "readings": readings,
            "count": readings.len(),
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Read the Mehr-/Mindermengensaldo for a MaLo and month.
    ///
    /// One implementation, shared with `GET /api/v1/imbalance/…`: the
    /// arithmetic and the sign convention are `metering::compute_imbalance`\'s.
    /// "Mehr = the positive intervals, Minder = the absolute value of the
    /// negative ones" is a series split by sign, not the
    /// Mehr-/Mindermengensaldo, and must never feed the INVOIC 31006 amount.
    #[tool(
        description = "Get the Mehr-/Mindermengensaldo for a MaLo and billing month. \
                       Compares the measured quantity edmd holds against the bilanzierte \
                       (profile-allocated) quantity, which must be supplied as \
                       `bilanziert_kwh` — edmd measures, it does not balance. \
                       Naming is from the network operator\'s side (GPKE Teil 1 Kap. 8.4): \
                       consuming *under* the profile is a MEHRmenge the NB credits; \
                       consuming over it is a MINDERmenge the NB invoices.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_imbalance(
        &self,
        Parameters(p): Parameters<GetImbalanceParams>,
    ) -> Result<CallToolResult, McpError> {
        use time::{Date, Month};

        let month = Month::try_from(p.month)
            .map_err(|_| McpError::invalid_params("month must be 1–12", None))?;
        let from = Date::from_calendar_date(p.year, month, 1)
            .map_err(|_| McpError::invalid_params("invalid year/month combination", None))?;
        let to = {
            let (ny, nm) = if p.month == 12 {
                (p.year + 1, Month::January)
            } else {
                (p.year, Month::try_from(p.month + 1).unwrap())
            };
            Date::from_calendar_date(ny, nm, 1)
                .unwrap()
                .previous_day()
                .unwrap_or(from)
        };
        let sparte = match p
            .sparte
            .as_deref()
            .unwrap_or("STROM")
            .to_uppercase()
            .as_str()
        {
            "GAS" => metering::Sparte::Gas,
            "WAERME" | "WÄRME" => metering::Sparte::Waerme,
            "WASSER" => metering::Sparte::Wasser,
            _ => metering::Sparte::Strom,
        };

        let report = match self
            .state
            .repo
            .imbalance(
                &p.malo_id,
                from,
                to,
                &self.state.tenant,
                sparte,
                p.bilanziert_kwh,
            )
            .await
        {
            Ok(r) => r,
            Err(crate::domain::EdmError::NoData { .. }) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "no_data: no billable meter reads for MaLo \'{}\' in {}-{:02}. \
                     Without a measured quantity the saldo is unknown, not zero.",
                    p.malo_id, p.year, p.month
                ))]));
            }
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };

        ContentBlock::json(serde_json::json!({
            "malo_id":            report.malo_id,
            "year":               p.year,
            "month":              p.month,
            "sparte":             report.sparte.as_str(),
            "gemessen_kwh":       report.gemessen_kwh.to_string(),
            "bilanziert_kwh":     report.bilanziert_kwh.to_string(),
            "mehrmenge_kwh":      report.mehrmenge_kwh.to_string(),
            "mindermenge_kwh":    report.mindermenge_kwh.to_string(),
            "delta_kwh":          report.delta_kwh.to_string(),
            "delta_pct":          report.delta_pct.map(|d| d.to_string()),
            "quality":            report.quality.as_str(),
            "interval_count":     report.interval_count,
            "legal_basis":
                "GPKE (BK6-24-174) Teil 1 Kap. 8.4 (Strom) · GaBi Gas 2.1 \
                 (BK7-24-01-008) Ziff. 3a (Gas)",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Get the aggregated billing-period summary for a MaLo.
    ///
    /// Returns arbeitsmenge (total kWh), spitzenleistung_kw (RLM Strom peak
    /// demand), and Gas conversion factors (brennwert, zustandszahl).
    /// Used by `invoicd` for RLM plausibility and by `netzbilanzd` (N4)
    /// for NNE invoice generation.
    #[tool(
        description = "Get aggregated billing period summary for a MaLo (arbeitsmenge, spitzenleistung, brennwert, zustandszahl). Used by invoicd and netzbilanzd.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_billing_period(
        &self,
        Parameters(p): Parameters<GetBillingPeriodParams>,
    ) -> Result<CallToolResult, McpError> {
        use time::macros::format_description;
        let fmt = format_description!("[year]-[month]-[day]");

        let period_from = time::Date::parse(&p.period_from, &fmt)
            .map_err(|_| McpError::invalid_params("period_from must be YYYY-MM-DD", None))?;
        let period_to = p
            .period_to
            .as_deref()
            .map(|s| time::Date::parse(s, &fmt))
            .transpose()
            .map_err(|_| McpError::invalid_params("period_to must be YYYY-MM-DD", None))?
            .unwrap_or(period_from);

        // A billing period is a Berlin calendar period (Liefermonat), so its UTC
        // window comes from the calendar — naive UTC midnight is 1–2 h off at
        // every month edge and the wrong length across a DST transition.
        let from_ts = metering::calendar::day_start_utc(period_from);
        let to_ts = metering::calendar::day_end_utc(period_to);

        // Query pre-aggregated billing periods first, fall back to raw aggregation.
        let pre = sqlx::query(
            r"SELECT arbeitsmenge_kwh, spitzenleistung_kw, brennwert_kwh_per_m3,
                     zustandszahl, messtyp, sparte, quality, computed_at
              FROM meter_billing_periods
              WHERE malo_id = $1 AND period_from = $2 AND period_to = $3
                AND tenant = $4",
        )
        .bind(&p.malo_id)
        .bind(period_from)
        .bind(period_to)
        .bind(&self.state.tenant)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if let Some(row) = pre {
            use sqlx::Row as _;
            // arbeitsmenge_kwh and spitzenleistung_kw are NUMERIC(18,5) in the schema.
            let arbeitsmenge: Option<rust_decimal::Decimal> =
                row.try_get("arbeitsmenge_kwh").unwrap_or(None);
            let spitzenleistung: Option<rust_decimal::Decimal> =
                row.try_get("spitzenleistung_kw").unwrap_or(None);
            ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id,
                "period_from": period_from.to_string(),
                "period_to": period_to.to_string(),
                "messtyp": row.try_get::<String, _>("messtyp").ok(),
                "sparte": row.try_get::<String, _>("sparte").ok(),
                "arbeitsmenge_kwh": arbeitsmenge.map(|d| d.to_string()),
                "spitzenleistung_kw": spitzenleistung.map(|d| d.to_string()),
                "brennwert_kwh_per_m3": row.try_get::<Option<String>, _>("brennwert_kwh_per_m3").ok().flatten(),
                "zustandszahl": row.try_get::<Option<String>, _>("zustandszahl").ok().flatten(),
                "quality": row.try_get::<String, _>("quality").ok(),
                "computed_at": row.try_get::<time::OffsetDateTime, _>("computed_at").ok()
                    .and_then(|t| {
                        use time::format_description::well_known::Rfc3339;
                        t.format(&Rfc3339).ok()
                    }),
                "source": "pre_aggregated",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
        } else {
            // On-the-fly aggregation from the version-resolved series. `query`
            // does not filter quality, so non-billable values (§60 Abs. 2:
            // `QualityFlag::is_billable`) are dropped here.
            let reads = self
                .state
                .repo
                .query(&TimeSeriesQuery {
                    malo_id: p.malo_id.clone(),
                    from: from_ts,
                    to: to_ts,
                    sparte: None,
                    tenant: self.state.tenant.clone(),
                })
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            let billable: Vec<_> = reads.iter().filter(|r| r.quality.is_billable()).collect();

            if billable.is_empty() {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "no_data: No meter reads for MaLo '{}' in {}/{} — {}.",
                    p.malo_id,
                    p.period_from,
                    p.period_to.as_deref().unwrap_or(&p.period_from),
                    "ensure MSCONS data has been ingested for this period"
                ))]));
            }

            // One canonical Bezug series, so the tool cannot answer with a
            // prosumer's Einspeisung folded into its grid draw, or with a
            // dual-tariff meter's consumption counted twice (`domain::register`).
            let intervals =
                crate::domain::energy_intervals(&reads, crate::domain::EnergyDirection::Bezug);
            let total_kwh: rust_decimal::Decimal = intervals.iter().map(|iv| iv.value).sum();
            // Peak demand is the interval's energy over its own length in hours,
            // so an hourly series reports one instead of none.
            let spitzenleistung_kw: Option<rust_decimal::Decimal> = intervals
                .iter()
                .filter_map(|iv| {
                    let minutes = rust_decimal::Decimal::from(
                        (iv.to - iv.from).whole_minutes().unsigned_abs(),
                    );
                    (minutes > rust_decimal::Decimal::ZERO)
                        .then(|| iv.value * rust_decimal::Decimal::from(60) / minutes)
                })
                .max();
            let count = intervals.len();

            ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id,
                "period_from": period_from.to_string(),
                "period_to": period_to.to_string(),
                "arbeitsmenge_kwh": Some(total_kwh.to_string()),
                "spitzenleistung_kw": spitzenleistung_kw.map(|d| d.to_string()),
                "read_count": count,
                "source": "on_the_fly",
                "note": "Pre-aggregated summary not available — computed from raw reads. Brennwert/Zustandszahl not yet available.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
        }
    }

    // ── Device history RAG ─────────────────────────────────────────────────

    #[tool(
        description = "M9 RAG indexing: Comprehensive device history summary for a MaLo. \
Returns rich natural-language text covering reading orders (Ablesesteuerung), \
meter data quality warnings, iMSys direct-push sessions, and quality scores \
for the past N days (default 90). \
Use this to build the LanceDB RAG index for `agentd` MSB natural-language queries: \
'list all meters at address X that required emergency readings' / \
'show quality issues for meter Y'. \
POST the returned text to agentd POST /api/v1/rag/ingest with source=msb-{malo_id}.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_device_history(
        &self,
        Parameters(p): Parameters<GetDeviceHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        use sqlx::Row as _;

        let days = p.days_back.unwrap_or(90).clamp(1, 365);
        let since = time::OffsetDateTime::now_utc() - time::Duration::days(days);

        // ── Reading orders ────────────────────────────────────────────────
        // The failure note column is `fehlschlag_notiz` (there is no `notiz`
        // column). A failed query surfaces as an error rather than an empty doc.
        let orders = sqlx::query(
            r"SELECT id, anlass, geplant_am, ausfuehrt_bis, status, fehlschlag_notiz
              FROM ablese_auftraege
              WHERE malo_id = $1 AND geplant_am >= $2::timestamptz
                AND tenant = $3
              ORDER BY geplant_am DESC
              LIMIT 50",
        )
        .bind(&p.malo_id)
        .bind(since)
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // ── Quality warnings from direct push ────────────────────────────
        let push_sessions = sqlx::query(
            r"SELECT session_id, source, obis_code, interval_count,
                     period_from, period_to, quality_summary, created_at
              FROM direct_push_sessions
              WHERE malo_id = $1 AND created_at >= $2
                AND tenant = $3
              ORDER BY created_at DESC
              LIMIT 20",
        )
        .bind(&p.malo_id)
        .bind(since)
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // ── Quality scores ────────────────────────────────────────────────
        // There is no per-interval `quality_warnings` column — meterstore owns
        // the readings; the per-batch verdicts live in
        // `quality_assessments`. Surface the non-clean assessments (grade ≠ A) as
        // the "quality warnings" section, sourced from that table on the pool.
        let quality_scores = sqlx::query(
            r"SELECT assessed_at, period_from, period_to, grade, gaps_detected,
                     coverage_pct, source
              FROM quality_assessments
              WHERE malo_id = $1
                AND assessed_at >= $2
                AND tenant = $3
                AND grade <> 'A'
              ORDER BY assessed_at DESC
              LIMIT 20",
        )
        .bind(&p.malo_id)
        .bind(since)
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // ── Format as rich text for RAG indexing ─────────────────────────
        let mut doc = format!(
            "# MSB Device History: MaLo {malo_id}\nPeriod: last {days} days (since {since_date})\n\n",
            malo_id = p.malo_id,
            since_date = since.date(),
        );

        // Reading orders section
        if orders.is_empty() {
            doc.push_str("## Reading Orders\nNo reading orders in this period.\n\n");
        } else {
            doc.push_str("## Reading Orders (Ablesesteuerung)\n\n");
            for r in &orders {
                let anlass: String = r.try_get("anlass").unwrap_or_default();
                let status: String = r.try_get("status").unwrap_or_default();
                let geplant: Option<time::Date> = r.try_get("geplant_am").ok();
                let notiz: Option<String> = r.try_get("fehlschlag_notiz").ok().flatten();
                let geplant_str = geplant.map(|d| d.to_string()).unwrap_or_default();
                doc.push_str(&format!("- **{status}** {anlass} — planned: {geplant_str}",));
                if let Some(n) = notiz.filter(|s| !s.is_empty()) {
                    doc.push_str(&format!(" — note: {n}"));
                }
                doc.push('\n');
            }
            doc.push('\n');
        }

        // Direct push sessions section
        if push_sessions.is_empty() {
            doc.push_str(
                "## iMSys Direct Push Sessions\nNo direct push sessions in this period.\n\n",
            );
        } else {
            doc.push_str("## iMSys Direct Push Sessions\n\n");
            for r in &push_sessions {
                let session_id: String = r.try_get("session_id").unwrap_or_default();
                let source: String = r.try_get("source").unwrap_or_default();
                let obis: Option<String> = r.try_get("obis_code").ok().flatten();
                let count: i32 = r.try_get("interval_count").unwrap_or_default();
                let period_from: Option<time::OffsetDateTime> =
                    r.try_get("period_from").ok().flatten();
                let period_to: Option<time::OffsetDateTime> = r.try_get("period_to").ok().flatten();
                let quality: Option<serde_json::Value> =
                    r.try_get("quality_summary").ok().flatten();

                let period = match (period_from, period_to) {
                    (Some(f), Some(t)) => format!("{} to {}", f.date(), t.date()),
                    (Some(f), None) => f.date().to_string(),
                    _ => "unknown period".to_owned(),
                };

                let warn_flag = quality
                    .as_ref()
                    .and_then(|q| q.get("has_warnings"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let quality_note = if warn_flag {
                    let gaps = quality
                        .as_ref()
                        .and_then(|q| q.get("gaps_detected"))
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let cov = quality
                        .as_ref()
                        .and_then(|q| q.get("coverage_pct"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(100.0);
                    format!(" ⚠ QUALITY WARNING: gaps={gaps}, coverage={cov:.1}%")
                } else {
                    " ✓ clean".to_owned()
                };

                let obis_str = obis.map(|o| format!(" OBIS={o}")).unwrap_or_default();
                doc.push_str(&format!(
                    "- Session {session_id} [{source}{obis_str}] {count} intervals, {period}{quality_note}\n",
                ));
            }
            doc.push('\n');
        }

        // Quality scores section (per-batch assessments, grade ≠ A)
        if quality_scores.is_empty() {
            doc.push_str("## Meter Data Quality Warnings\nNo quality warnings in this period.\n\n");
        } else {
            doc.push_str("## Meter Data Quality Warnings\n\n");
            for r in &quality_scores {
                let assessed: Option<time::OffsetDateTime> = r.try_get("assessed_at").ok();
                let grade: String = r.try_get("grade").unwrap_or_default();
                let gaps: i32 = r.try_get("gaps_detected").unwrap_or(0);
                let coverage: Option<rust_decimal::Decimal> =
                    r.try_get("coverage_pct").ok().flatten();
                let source: String = r.try_get("source").unwrap_or_default();

                let ts = assessed.map(|t| t.date().to_string()).unwrap_or_default();
                let cov_str = coverage
                    .map(|c| format!(", coverage={c}%"))
                    .unwrap_or_default();
                doc.push_str(&format!(
                    "- {ts}: grade={grade} [{source}], gaps={gaps}{cov_str}\n",
                ));
            }
            doc.push('\n');
        }

        doc.push_str(&format!(
            "\n---\nGenerated by edmd get_device_history for RAG indexing.\n\
             Source: `msb-{malo_id}`\n\
             Reading orders: {ro}, push sessions: {ps}, quality warnings: {qw}",
            malo_id = p.malo_id,
            ro = orders.len(),
            ps = push_sessions.len(),
            qw = quality_scores.len(),
        ));

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "days_back": days,
            "document_text": doc,
            "rag_source": format!("msb-{}", p.malo_id),
            "reading_orders_count": orders.len(),
            "push_sessions_count": push_sessions.len(),
            "quality_warnings_count": quality_scores.len(),
            "ingest_hint": "POST this document_text to agentd POST /api/v1/rag/ingest",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// `get_quality_warnings` — query Hampel-filter quality warnings for a MaLo.
    ///
    /// Returns `meter_reads` rows where `quality_warnings->>'has_warnings' = 'true'`
    /// in the given time range, structured by the Hampel-k3-t3 algorithm.
    #[allow(dead_code)]
    #[tool(
        description = "Query Hampel-filter quality warnings for a MaLo in a time range. \
Returns grade (A/B/C/F), outlier/spike timestamps, gaps detected, and coverage %.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_quality_warnings(
        &self,
        Parameters(p): Parameters<GetQualityWarningsParams>,
    ) -> Result<CallToolResult, McpError> {
        use metering::MeterInterval;
        use time::format_description::well_known::Rfc3339;
        let now = time::OffsetDateTime::now_utc();
        let from_dt = p
            .from
            .as_deref()
            .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok())
            .unwrap_or_else(|| now - time::Duration::days(30));
        let to_dt =
            p.to.as_deref()
                .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok())
                .unwrap_or(now);

        // There is no stored per-interval `quality_warnings` column —
        // meterstore owns the readings. Recompute the Hampel/gap verdict on the
        // fly from the version-resolved series — the same scorer the ingest and
        // rescore paths use — so the tool reports a fresh grade for the window.
        let reads = self
            .state
            .repo
            .query(&TimeSeriesQuery {
                malo_id: p.malo_id.clone(),
                from: from_dt,
                to: to_dt,
                sparte: None,
                tenant: self.state.tenant.clone(),
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let intervals: Vec<MeterInterval> = reads
            .iter()
            .map(|r| MeterInterval {
                from: r.dtm_from,
                to: r.dtm_to,
                value: r.quantity_kwh,
                quality: r.quality,
                obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
            })
            .collect();

        // The **same** scorer the REST surface runs, not a second one.
        // Calling `score_intervals` directly with `QualityConfig::default()`
        // diverges three ways at once: electricity thresholds applied to every
        // commodity (a summer gas profile grades as a stuck meter), an assumed
        // 900 s cadence instead of the observed one, and a fixed `hampel_k3_t3`
        // label naming parameters that were not used. An agent and an operator
        // asking the same question would get different answers.
        let sparte = reads.first().map_or(metering::Sparte::Strom, |r| r.sparte);
        let report = crate::server::compute_quality(&intervals, sparte, from_dt, to_dt);

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "window_from": from_dt.to_string(),
            "window_to": to_dt.to_string(),
            "interval_count": report.intervals_accepted,
            "grade": report.grade,
            "has_warnings": report.has_warnings,
            "gaps_detected": report.gaps_detected,
            "zero_run_length": report.zero_run_length,
            "coverage_pct": report.coverage_pct,
            "intervals_consistent": report.intervals_consistent,
            "expected_intervals": report.expected_intervals,
            "outlier_intervals": report.outlier_intervals,
            "spike_intervals": report.spike_intervals,
            "algorithm": report.algorithm,
            "hint": "Use POST /api/v1/quality-score/{malo_id} to persist this rescore to quality_assessments.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// List reading orders (Ablesesteuerung) for a MaLo.
    ///
    /// Returns reading orders filtered by status and/or Anlass.
    /// Use this to check § 40b Abs. 1 EnWG Jahresablesung scheduling,
    /// INSRPT_STOERUNG sonderablesung status, and Lieferbeginn/ende readings.
    #[tool(
        description = "List Ablesesteuerung reading orders for a MaLo. Filter by status (OFFEN/BEAUFTRAGT/AUSGEFUEHRT) and anlass (JAHRESABLESUNG/LIEFERBEGINN/SONDERABLESUNG/…).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_reading_orders(
        &self,
        Parameters(p): Parameters<ListReadingOrdersParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(50).min(500);
        let rows = sqlx::query(
            r"SELECT id, malo_id, melo_id, anlass, auftraggeber_rolle,
                     ausfuehrender_msb, geplant_am, ausfuehrt_bis, status,
                     zaehlerstand_kwh, ausgefuehrt_am, insrpt_process_id, created_at
              FROM ablese_auftraege
              WHERE tenant = $1
                AND malo_id = $2
                AND ($3::text IS NULL OR status = $3)
                AND ($4::text IS NULL OR anlass = $4)
              ORDER BY geplant_am DESC
              LIMIT $5",
        )
        .bind(&self.state.tenant)
        .bind(&p.malo_id)
        .bind(&p.status)
        .bind(&p.anlass)
        .bind(limit)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let orders: Vec<serde_json::Value> = rows.iter().map(|r| {
            use sqlx::Row as _;
            serde_json::json!({
                "id": r.try_get::<uuid::Uuid, _>("id").ok().map(|u| u.to_string()),
                "malo_id": r.try_get::<String, _>("malo_id").ok(),
                "anlass": r.try_get::<String, _>("anlass").ok(),
                "auftraggeber_rolle": r.try_get::<String, _>("auftraggeber_rolle").ok(),
                "geplant_am": r.try_get::<time::Date, _>("geplant_am").ok().map(|d| d.to_string()),
                "ausfuehrt_bis": r.try_get::<Option<time::Date>, _>("ausfuehrt_bis").ok().flatten().map(|d| d.to_string()),
                "status": r.try_get::<String, _>("status").ok(),
                "zaehlerstand_kwh": r.try_get::<Option<f64>, _>("zaehlerstand_kwh").ok().flatten(),
                "ausgefuehrt_am": r.try_get::<Option<time::OffsetDateTime>, _>("ausgefuehrt_am").ok().flatten().map(|t| t.to_string()),
                "insrpt_process_id": r.try_get::<Option<String>, _>("insrpt_process_id").ok().flatten(),
            })
        }).collect();

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "count": orders.len(),
            "orders": orders,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// List overdue reading orders — past their `ausfuehrt_bis` deadline.
    ///
    /// Identifies § 40b Abs. 1 EnWG Jahresablesung compliance failures.
    /// An overdue JAHRESABLESUNG means SLP meter data is missing, which will
    /// cause Mehr-/Mindermengen disputes with the LF.
    #[tool(
        description = "List all reading orders past their ausfuehrt_bis deadline (§ 40b Abs. 1 EnWG compliance). Returns overdue OFFEN/BEAUFTRAGT orders sorted by deadline oldest-first.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_overdue_reading_orders(
        &self,
        Parameters(p): Parameters<ListOverdueReadingOrdersParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(100).min(2000);
        let rows = sqlx::query(
            // FEHLGESCHLAGEN is included: the order is terminal but the reading
            // is still owed, so a failed Jahresablesung past its deadline is
            // exactly the § 40b Abs. 1 EnWG gap this tool exists to surface.
            // Excluding it would let /fail retire an obligation silently.
            r"SELECT id, malo_id, melo_id, anlass, auftraggeber_rolle,
                     ausfuehrender_msb, geplant_am, ausfuehrt_bis, status,
                     fehlschlag_grund, created_at
              FROM ablese_auftraege
              WHERE tenant = $1
                AND status IN ('OFFEN', 'BEAUFTRAGT', 'FEHLGESCHLAGEN')
                AND ausfuehrt_bis < CURRENT_DATE
                AND ($2::text IS NULL OR ausfuehrender_msb = $2)
              ORDER BY ausfuehrt_bis ASC
              LIMIT $3",
        )
        .bind(&self.state.tenant)
        .bind(&p.ausfuehrender_msb)
        .bind(limit)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let orders: Vec<serde_json::Value> = rows.iter().map(|r| {
            use sqlx::Row as _;
            serde_json::json!({
                "id": r.try_get::<uuid::Uuid, _>("id").ok().map(|u| u.to_string()),
                "malo_id": r.try_get::<String, _>("malo_id").ok(),
                "anlass": r.try_get::<String, _>("anlass").ok(),
                "ausfuehrender_msb": r.try_get::<Option<String>, _>("ausfuehrender_msb").ok().flatten(),
                "ausfuehrt_bis": r.try_get::<Option<time::Date>, _>("ausfuehrt_bis").ok().flatten().map(|d| d.to_string()),
                "status": r.try_get::<String, _>("status").ok(),
                "fehlschlag_grund": r.try_get::<Option<String>, _>("fehlschlag_grund").ok().flatten(),
                "days_overdue": r.try_get::<Option<time::Date>, _>("ausfuehrt_bis").ok().flatten().map(|d| {
                    (time::OffsetDateTime::now_utc().date() - d).whole_days()
                }),
            })
        }).collect();

        let by_anlass: std::collections::HashMap<String, usize> =
            orders
                .iter()
                .fold(std::collections::HashMap::new(), |mut acc, o| {
                    if let Some(anlass) = o["anlass"].as_str() {
                        *acc.entry(anlass.to_owned()).or_insert(0) += 1;
                    }
                    acc
                });

        let failed = orders
            .iter()
            .filter(|o| o["status"].as_str() == Some("FEHLGESCHLAGEN"))
            .count();

        ContentBlock::json(serde_json::json!({
            "total_overdue": orders.len(),
            "failed_count": failed,
            "by_anlass": by_anlass,
            "orders": orders,
            "regulatory_note": "JAHRESABLESUNG overdue = § 40b Abs. 1 EnWG violation. SLP Mehr-/Mindermengen will be estimated, not metered. FEHLGESCHLAGEN orders are listed too — the order is closed but the reading is still owed, so it needs re-dispatch or a documented §40a EnWG estimate.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Launch a § 40b Abs. 1 EnWG Jahresablesung campaign via MCP.
    ///
    /// Creates JAHRESABLESUNG reading orders for all SLP MaLos in the NB's
    /// grid area that have not yet been scheduled. Use `dry_run = true` to
    /// preview the count without creating orders.
    #[tool(
        description = "Launch § 40b Abs. 1 EnWG Jahresablesung campaign: creates JAHRESABLESUNG reading orders for all SLP MaLos without a scheduled reading this year. Set dry_run=true to preview.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn trigger_jahresablesung(
        &self,
        Parameters(p): Parameters<TriggerJahresablesungParams>,
    ) -> Result<CallToolResult, McpError> {
        let year = p
            .campaign_year
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().year());
        let dry_run = p.dry_run.unwrap_or(false);
        // Bounded so a large grid cannot exceed the client's request timeout.
        let max_malos = p.max_malos.unwrap_or(2_000).clamp(1, 10_000);

        // Orders already raised for this campaign year. Reported in both modes,
        // because "how much is left" is the question either way.
        let (already_scheduled,): (i64,) = sqlx::query_as(
            r"SELECT COUNT(DISTINCT malo_id)
              FROM ablese_auftraege
              WHERE tenant = $1
                AND anlass = 'JAHRESABLESUNG'
                AND auftraggeber_rolle = 'NB'
                AND extract(year FROM geplant_am) = $2
                AND status IN ('OFFEN','BEAUFTRAGT','AUSGEFUEHRT')",
        )
        .bind(&self.state.tenant)
        .bind(year)
        .fetch_one(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if dry_run {
            return ContentBlock::json(serde_json::json!({
                "dry_run": true,
                "campaign_year": year,
                "nb_mp_id": p.nb_mp_id,
                "already_scheduled": already_scheduled,
                "max_malos": max_malos,
                "note": "no orders were created; re-run with dry_run=false to create them",
            }))
            .map(|b| CallToolResult::success(vec![b]));
        }

        // The same core the HTTP endpoint runs, so both raise identical orders
        // under identical idempotency rules.
        let req = crate::server::JahresablesungCampaignRequest {
            nb_mp_id: p.nb_mp_id.clone(),
            campaign_year: Some(year),
            geplant_am: None,
            ausfuehrt_bis: None,
            ausfuehrender_msb: None,
            max_malos: Some(max_malos),
        };

        match crate::server::run_jahresablesung_campaign(
            &self.state.pool,
            &self.state.tenant,
            &self.state.marktd_url,
            &self.state.marktd_api_key,
            &req,
        )
        .await
        {
            Ok(outcome) => ContentBlock::json(serde_json::json!({
                "campaign_year": outcome.year,
                "nb_mp_id": p.nb_mp_id,
                "malos_enumerated": outcome.total_malos,
                "orders_created": outcome.created,
                "already_scheduled_skipped": outcome.skipped,
                "geplant_am": outcome.geplant_am.to_string(),
                "ausfuehrt_bis": outcome.ausfuehrt_bis.to_string(),
                "capped": outcome.total_malos >= usize::try_from(max_malos).unwrap_or(usize::MAX),
                "legal_basis": "§ 40b Abs. 1 EnWG",
                "note": "re-running is idempotent — already-scheduled MaLos are skipped, \
                         so a capped run is resumed by calling again",
            }))
            .map(|b| CallToolResult::success(vec![b])),
            Err(e) => Err(McpError::internal_error(e.detail(), None)),
        }
    }

    #[tool(
        description = "Generate and store § 60 Abs. 2 MsbG Ersatzwerte (substitute values) for a gap window. Methods: PriorPeriodAverage (default), LinearInterpolation, ZeroFill, LastValueCarryForward. Never overwrites a billable reading; every substitute is logged to substitute_value_log (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit trail). Runs the same core as POST /api/v1/meter-reads/{malo_id}/substitute.",
        annotations(
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn trigger_substitution(
        &self,
        Parameters(p): Parameters<TriggerSubstitutionParams>,
    ) -> Result<CallToolResult, McpError> {
        let req = crate::server::SubstituteRequest {
            gap_from: p.gap_from,
            gap_to: p.gap_to,
            interval_secs: p.interval_secs,
            method: p.method,
            prior_days: p.prior_days,
            operator_id: Some("mcp-agent".to_owned()),
            sparte: p.sparte,
            reason: p.reason,
            obis_code: p.obis_code,
        };

        let resp = crate::server::run_substitute_values(
            &self.state.repo,
            &self.state.tenant,
            &p.malo_id,
            &req,
        )
        .await;

        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(
            |_| serde_json::json!({ "raw": String::from_utf8_lossy(&bytes).into_owned() }),
        );

        if status.is_success() {
            ContentBlock::json(json).map(|b| CallToolResult::success(vec![b]))
        } else {
            Err(McpError::internal_error(json.to_string(), None))
        }
    }
}

#[prompt_router]
impl EdmdMcpHandler {
    #[prompt(
        name = "analyze-consumption",
        description = "Step-by-step: analyze meter readings and consumption for a MaLo"
    )]
    async fn analyze_consumption_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I analyze energy consumption data for a MaLo?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `get_timeseries` with malo_id and time range to fetch MSCONS data.\n\
                 2. Key OBIS codes:\n\
                    - 1-0:1.8.0 (Wirkenergie Bezug gesamt, SLP/RLM consumption)\n\
                    - 1-0:2.8.0 (Wirkenergie Einspeisung, feed-in)\n\
                    - 1-0:1.8.1 / 1-0:1.8.2 (HT / NT for Zweitarif billing)\n\
                 3. Use `get_billing_period` for MeterBillingPeriod (arbeitsmenge_kwh, brennwert/zustandszahl).\n\
                 4. Compare billing period totals against the INVOIC in invoicd.\n\
                 5. Discrepancies > 3% trigger INVOIC dispute (invoic-checker check 2).",
            ),
        ]
    }

    #[prompt(
        name = "submit-mscons",
        description = "Step-by-step: submit MSCONS meter readings into edmd"
    )]
    async fn submit_mscons_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I submit MSCONS meter readings to edmd?"),
            PromptMessage::new_text(
                Role::Assistant,
                "MSCONS readings arrive via makod's EDIFACT pipeline automatically.\n\
                 For manual injection or testing:\n\
                 1. POST /api/v1/deliveries with the MSCONS BO4E Energiemenge payload.\n\
                 2. Required: malo_id, messlokation_id, obis_code, zeitreihe (time series).\n\
                 3. edmd validates the OBIS code and persists the time series.\n\
                 4. Use `get_timeseries` to verify the ingestion was correct.\n\n\
                 For Iceberg archive queries (bulk historical data):\n\
                 GET /api/v1/archive/{malo_id}?from=...&to=... returns Parquet-backed results.",
            ),
        ]
    }

    #[prompt(
        name = "quality-assessment",
        description = "Step-by-step: assess meter read quality for a MaLo using the Hampel filter"
    )]
    async fn quality_assessment_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I assess the quality of meter readings for a MaLo?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "Quality scoring uses the **Hampel filter** (window k=3, threshold t=3.0 robust sigma) — \n\
                 sliding-window median/MAD outlier detection robust to the outliers it detects.\n\n\
                 ## Steps\n\
                 1. Call `get_quality_warnings(malo_id, from, to)` to see existing quality issues.\n\
                 2. Check `grade` field: A (clean) | B (minor) | C (significant) | F (unusable).\n\
                 3. If you suspect historical data was stored without quality scoring (pre-M7), \n\
                    retroactively rescore: `POST /api/v1/quality-score/{malo_id}?from=&to=`.\n\
                 4. Investigate:\n\
                    - `outlier_intervals` — Hampel-flagged timestamps (robust to contamination)\n\
                    - `spike_intervals` — values > 10× window median (decimal-point errors)\n\
                    - `gaps_detected` — discontinuities (missing intervals)\n\
                    - `zero_run_length` — consecutive zero reads (meter fault / firmware bug)\n\
                    - `intervals_consistent` — mixed interval lengths (SLP/RLM mix-up)\n\
                    - `coverage_pct` — < 95% signals incomplete MSCONS delivery\n\n\
                 ## Why Hampel?\n\
                 Global 3-sigma is contaminated by the very outliers it tries to detect.\n\
                 The Hampel filter uses local median + MAD, immune to up to 50% contamination.\n\
                 MAD scale factor 1.4826 ensures equivalence to Gaussian σ for clean data.\n\n\
                 ## Grades\n\
                 | Grade | Billing action |\n\
                 |---|---|\n\
                 | A | Normal billing run |\n\
                 | B | Proceed with note in INVOIC |\n\
                 | C | Manual review before billing |\n\
                 | F | Block billing — data unusable |",
            ),
        ]
    }

    #[prompt(
        name = "jahresablesung-workflow",
        description = "§ 40b Abs. 1 EnWG Jahresablesung: annual SLP meter reading campaign"
    )]
    async fn jahresablesung_workflow_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I run the § 40b Abs. 1 EnWG Jahresablesung campaign?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## § 40b Abs. 1 EnWG — Annual SLP Meter Reading\n\n\
                 The NB must ensure all SLP meters are read at least once per year.\n\
                 Failure → estimated SLP billing → Mehr-/Mindermengendisputes with the LF.\n\n\
                 ### Step 1: Check compliance status\n\
                 `list_overdue_reading_orders` (filter by anlass=JAHRESABLESUNG)\n\
                 → Shows all SLP meters past their reading deadline.\n\n\
                 ### Step 2: Launch the campaign\n\
                 ```http\n\
                 POST /api/v1/reading-orders/campaign\n\
                 { \"nb_mp_id\": \"9910000000002\", \"campaign_year\": 2026 }\n\
                 ```\n\
                 → Creates JAHRESABLESUNG reading orders for all unscheduled SLP MaLos.\n\
                 → Idempotent: re-running skips already-scheduled MaLos.\n\
                 → Default: geplant_am = Dec 31, ausfuehrt_bis = Jan 31 next year.\n\n\
                 ### Step 3: Dispatch to field service\n\
                 `list_reading_orders(malo_id, status=OFFEN)` for individual MaLos.\n\
                 Update status via `PUT /api/v1/reading-orders/{id}/complete` with Zählerstand.\n\n\
                 ### Step 4: Verify completion\n\
                 `list_overdue_reading_orders` after campaign deadline.\n\
                 Count of JAHRESABLESUNG overdue = § 40b Abs. 1 EnWG compliance gap.\n\n\
                 ### Step 5: Billing impact\n\
                 `get_billing_period(malo_id, from=Jan 1, to=Dec 31)` shows the full-year\n\
                 arbeitsmenge_kwh used for SLP Mehr-/Mindermengensaldo.\n\
                 Missing reads → arbeitsmenge_kwh is estimated → dispute risk.",
            ),
        ]
    }

    #[prompt(
        name = "reading-order-lifecycle",
        description = "Ablesesteuerung: reading order lifecycle from creation to billing"
    )]
    async fn reading_order_lifecycle_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How does the reading order lifecycle work in edmd?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## Ablesesteuerung — Reading Order Lifecycle\n\n\
                 Reading orders track physical meter reads for all three market roles.\n\n\
                 ```\n\
                 OFFEN → BEAUFTRAGT → AUSGEFUEHRT   (reading taken, obligation met)\n\
                    └──────────────→ STORNIERT      (no longer owed)\n\
                    └──────────────→ FEHLGESCHLAGEN (Ablesehindernis — still owed)\n\
                 ```\n\n\
                 `STORNIERT` and `FEHLGESCHLAGEN` are both terminal, but only\n\
                 `STORNIERT` discharges the obligation. A `FEHLGESCHLAGEN` order\n\
                 past `ausfuehrt_bis` keeps appearing in\n\
                 `list_overdue_reading_orders` until the reading is re-dispatched\n\
                 or the quantity is estimated under §40a EnWG.\n\n\
                 ```http\n\
                 PUT /api/v1/reading-orders/{id}/fail\n\
                 { \"grund\": \"KEIN_ZUTRITT\", \"notiz\": \"3x angetroffen, niemand vor Ort\" }\n\
                 ```\n\
                 Gründe: KEIN_ZUTRITT · ZAEHLER_UNZUGAENGLICH · ZAEHLER_DEFEKT ·\n\
                 ZAEHLER_NICHT_AUFFINDBAR · KUNDE_VERWEIGERT · ABLESUNG_UNPLAUSIBEL ·\n\
                 SONSTIGES\n\n\
                 ### Triggers (automatic)\n\
                 | Event | Reading order created |\n\
                 |---|---|\n\
                 | INSRPT 23001 Störungsmeldung | `INSRPT_STOERUNG` (WiM Störungsmeldung; die MSB-Antwort ist 3 bzw. 1 Werktag je Messtechnik, WiM Teil 2 Kap. 1.2 Nr. 2) |\n\
                 | INSRPT 23003/23008 Technische Änderung | `SONDERABLESUNG` at handover date |\n\
                 | GPKE 55001 Lieferbeginn | `LIEFERBEGINN` at Lieferbeginndatum |\n\
                 | GPKE 55004/55007 Abmeldung/Beendigung der Zuordnung | `LIEFERENDE` at Lieferendedatum |\n\
                 | NB campaign | `JAHRESABLESUNG` (§ 40b Abs. 1 EnWG) |\n\n\
                 ### Manual creation\n\
                 ```http\n\
                 POST /api/v1/reading-orders\n\
                 {\n\
                   \"malo_id\": \"51238696781\",\n\
                   \"anlass\": \"ZWISCHENABLESUNG\",\n\
                   \"auftraggeber_rolle\": \"LF\",\n\
                   \"geplant_am\": \"2026-08-01\"\n\
                 }\n\
                 ```\n\n\
                 ### Completing a reading\n\
                 ```http\n\
                 PUT /api/v1/reading-orders/{id}/complete\n\
                 { \"zaehlerstand_kwh\": 12345.678, \"mscons_ref\": \"MSG-001\" }\n\
                 ```\n\n\
                 ### Querying\n\
                 `list_reading_orders(malo_id, status=OFFEN)` → pending orders\n\
                 `list_overdue_reading_orders()` → compliance gap report",
            ),
        ]
    }

    // ── New Phase-2 tools ─────────────────────────────────────────────────────

    /// `get_correction_history` — list retroactive corrections for a MaLo (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD)).
    ///
    /// Returns all `meter_read_corrections` rows for the given MaLo in descending
    /// chronological order. Each row shows the original value, corrected value, reason,
    /// and operator — enabling full § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit-trail reconstruction.
    #[tool(
        name = "get_correction_history",
        description = "§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit: list all retroactive corrections applied to a MaLo's \
                       meter reads. Each entry shows original value, corrected value, reason, \
                       source (MSCONS_UPDATE/OPERATOR/AUTO_SUBSTITUTE), and timestamp.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_correction_history(
        &self,
        Parameters(p): Parameters<GetCorrectionHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        use time::format_description::well_known::Rfc3339;
        let pool = &self.state.pool;
        let now = time::OffsetDateTime::now_utc();
        let limit_val = p.limit.unwrap_or(100).min(1000) as i64;
        let malo_id = p.malo_id;

        let from_ts = p
            .from
            .as_deref()
            .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok())
            .unwrap_or_else(|| now - time::Duration::days(90));
        let to_ts =
            p.to.as_deref()
                .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok())
                .unwrap_or(now);

        let rows = sqlx::query(
            r"SELECT correction_id, malo_id, dtm_from, dtm_to,
                     original_kwh, original_quality, corrected_kwh, corrected_quality,
                     reason, source, corrected_by, corrected_at
              FROM meter_read_corrections
              WHERE malo_id = $1
                AND corrected_at >= $2
                AND corrected_at <= $3
                AND tenant = $5
              ORDER BY corrected_at DESC
              LIMIT $4",
        )
        .bind(&malo_id)
        .bind(from_ts)
        .bind(to_ts)
        .bind(limit_val)
        .bind(&self.state.tenant)
        .fetch_all(pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let corrections: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "correction_id": r.try_get::<uuid::Uuid, _>("correction_id").ok(),
                    "malo_id": r.try_get::<String, _>("malo_id").ok(),
                    "dtm_from": r.try_get::<time::OffsetDateTime, _>("dtm_from").ok().map(|t| t.format(&Rfc3339).ok()),
                    "dtm_to": r.try_get::<time::OffsetDateTime, _>("dtm_to").ok().map(|t| t.format(&Rfc3339).ok()),
                    "original_kwh": r.try_get::<String, _>("original_kwh").ok(),
                    "original_quality": r.try_get::<String, _>("original_quality").ok(),
                    "corrected_kwh": r.try_get::<String, _>("corrected_kwh").ok(),
                    "corrected_quality": r.try_get::<String, _>("corrected_quality").ok(),
                    "reason": r.try_get::<String, _>("reason").ok(),
                    "source": r.try_get::<String, _>("source").ok(),
                    "corrected_by": r.try_get::<Option<String>, _>("corrected_by").ok().flatten(),
                    "corrected_at": r.try_get::<time::OffsetDateTime, _>("corrected_at").ok().map(|t| t.format(&Rfc3339).ok()),
                })
            })
            .collect();

        let out = serde_json::json!({
            "malo_id": malo_id,
            "period": { "from": from_ts.format(&Rfc3339).ok(), "to": to_ts.format(&Rfc3339).ok() },
            "correction_count": corrections.len(),
            "corrections": corrections,
            "_note": "Use POST /api/v1/corrections/{malo_id} to submit new corrections (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD))"
        });
        serde_json::to_string_pretty(&out)
            .map(|s| CallToolResult::success(vec![ContentBlock::text(s)]))
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// `validate_timeseries` — run the metering validation engine on a MaLo's time-series.
    ///
    /// Applies the metering validation rules to the stored meter reads: gap
    /// detection, overlap detection, negative energy, statistical outliers, zero
    /// runs, interval-length consistency, DST ambiguity, future timestamps,
    /// non-billable quality flags, unordered input and implausible power.
    ///
    /// There is no V10. It was a "register rollover" rule that compared
    /// consecutive interval energies, which is meaningless for a series of
    /// per-interval quantities rather than cumulative Zählerstände — it could
    /// only fire on a quarter-hour carrying 50 MWh. `metering` removed it and
    /// left the number unused so a stored `V10` finding cannot be reinterpreted.
    #[tool(
        name = "validate_timeseries",
        description = "Run the metering validation engine (rules V01–V09/V11/V12) on stored meter reads \
                       for a MaLo. Identifies gaps (V01), overlaps (V02), negative energy (V03), \
                       statistical outliers (V04), zero-runs (V05), inconsistent intervals (V06), \
                       DST ambiguity (V07), future timestamps (V08), non-billable quality (V09), \
                       unordered input (V11) and implausible power (V12). There is no V10. \
                       Use before billing to detect intervals requiring § 60 Abs. 2 MsbG \
                       substitute values.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn validate_timeseries(
        &self,
        Parameters(p): Parameters<ValidateTimeseriesParams>,
    ) -> Result<CallToolResult, McpError> {
        use metering::{MeterInterval, ValidationConfig, validate_intervals};
        use time::format_description::well_known::Rfc3339;

        let from_ts = time::OffsetDateTime::parse(&p.from, &Rfc3339)
            .map_err(|e| McpError::invalid_params(format!("invalid from: {e}"), None))?;
        let to_ts = time::OffsetDateTime::parse(&p.to, &Rfc3339)
            .map_err(|e| McpError::invalid_params(format!("invalid to: {e}"), None))?;
        let malo_id = p.malo_id;

        // Fetch reads through the repository. `query` does NOT filter quality, so
        // the validation engine sees Faulty/Unknown intervals too — which V09
        // (non-billable quality) needs to fire.
        let reads = self
            .state
            .repo
            .query(&TimeSeriesQuery {
                malo_id: malo_id.clone(),
                from: from_ts,
                to: to_ts,
                sparte: None,
                tenant: self.state.tenant.clone(),
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let intervals: Vec<MeterInterval> = reads
            .iter()
            .map(|r| MeterInterval {
                from: r.dtm_from,
                to: r.dtm_to,
                value: r.quantity_kwh,
                quality: r.quality,
                obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
            })
            .collect();

        // V01 (gap) and V02 (overlap) are statements about a **single** series,
        // and a MaLo delivers several registers at once. Validated flat, every
        // same-slot pair of a bidirectional or dual-tariff meter reported as an
        // overlapping interval at `Error` severity — so this tool told an agent
        // that a perfectly ordinary prosumer delivery was unbillable. The ingest
        // path splits by register for exactly this reason (`domain::validation`);
        // so does this one now.
        let config = ValidationConfig {
            expected_interval_secs: Some(p.interval_secs.unwrap_or(900)),
            now: Some(time::OffsetDateTime::now_utc()),
            ..ValidationConfig::default()
        };
        let groups = crate::domain::register_groups(&reads);
        let mut issues = Vec::new();
        let mut billing_block_count = 0usize;
        let mut has_errors = false;
        for group in &groups {
            let result = validate_intervals(&group.intervals, &config);
            billing_block_count += result.billing_block_count();
            has_errors |= result.has_errors();
            for i in &result.issues {
                issues.push((group.obis_code, i.clone()));
            }
        }
        let is_clean = issues.is_empty();

        let issues_json: Vec<serde_json::Value> = issues
            .iter()
            .map(|(obis, i)| {
                serde_json::json!({
                    "rule": i.rule_id.to_string(),
                    "severity": format!("{:?}", i.severity),
                    "message": i.message,
                    // Which register the finding is about: an index into one
                    // group means nothing without it.
                    "obis_code": obis.map(|c| c.to_string()),
                    "interval_index": i.interval_index,
                    "affected_from": i.affected_from.map(|t| t.format(&Rfc3339).ok()),
                    "affected_value_kwh": i.affected_value,
                    "blocks_billing": i.blocks_billing(),
                })
            })
            .collect();

        let out = serde_json::json!({
            "malo_id": malo_id,
            "period": { "from": &p.from, "to": &p.to },
            "interval_count": intervals.len(),
            "register_count": groups.len(),
            "is_clean": is_clean,
            "has_errors": has_errors,
            "billing_block_count": billing_block_count,
            "issue_count": issues.len(),
            "issues": issues_json,
            "_note": if billing_block_count > 0 {
                format!("{billing_block_count} interval(s) require § 60 Abs. 2 MsbG substitute values before billing")
            } else {
                "All intervals are billing-eligible".to_owned()
            }
        });
        serde_json::to_string_pretty(&out)
            .map(|s| CallToolResult::success(vec![ContentBlock::text(s)]))
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// Get per-batch quality assessment history for a MaLo (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit trail).
    #[tool(
        name = "get_quality_assessments",
        description = "Retrieve quality assessment history for a MaLo — per-batch \
                       grade (A/B/C/F), coverage %, gap count, billing_blocked flag. \
                       Use to investigate recurring quality issues or § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit. \
                       Params: malo_id (required), from / to (ISO 8601, optional).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_quality_assessments(
        &self,
        Parameters(p): Parameters<GetQualityAssessmentsParams>,
    ) -> Result<CallToolResult, McpError> {
        use time::format_description::well_known::Rfc3339;
        let from = p
            .from
            .as_deref()
            .map(|s| time::OffsetDateTime::parse(s, &Rfc3339))
            .transpose()
            .map_err(|e| McpError::invalid_params(format!("invalid from: {e}"), None))?
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
        let to =
            p.to.as_deref()
                .map(|s| time::OffsetDateTime::parse(s, &Rfc3339))
                .transpose()
                .map_err(|e| McpError::invalid_params(format!("invalid to: {e}"), None))?
                .unwrap_or_else(time::OffsetDateTime::now_utc);

        let rows = sqlx::query(
            r"SELECT assessed_at, source, grade, interval_count, expected_count,
                     coverage_pct, gaps_detected, billing_blocked, pid
              FROM quality_assessments
              WHERE malo_id = $1 AND assessed_at BETWEEN $2 AND $3
                AND tenant = $4
              ORDER BY assessed_at DESC LIMIT 100",
        )
        .bind(&p.malo_id)
        .bind(from)
        .bind(to)
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row;
        let assessments: Vec<serde_json::Value> = rows.iter().map(|r| {
            serde_json::json!({
                "assessed_at": r.try_get::<time::OffsetDateTime, _>("assessed_at").ok().map(|t| t.to_string()),
                "source": r.try_get::<String, _>("source").unwrap_or_default(),
                "grade": r.try_get::<String, _>("grade").unwrap_or_default(),
                "interval_count": r.try_get::<i32, _>("interval_count").unwrap_or(0),
                "coverage_pct": r.try_get::<Option<f64>, _>("coverage_pct").ok().flatten(),
                "gaps_detected": r.try_get::<i32, _>("gaps_detected").unwrap_or(0),
                "billing_blocked": r.try_get::<bool, _>("billing_blocked").unwrap_or(false),
                "pid": r.try_get::<Option<i32>, _>("pid").ok().flatten(),
            })
        }).collect();

        serde_json::to_string_pretty(&serde_json::json!({
            "malo_id": p.malo_id,
            "count": assessments.len(),
            "assessments": assessments,
        }))
        .map(|s| CallToolResult::success(vec![ContentBlock::text(s)]))
        .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// Get Summenzeitreihe (monthly aggregated energy) for MaBiS and GPKE (BK6-24-174) Teil 1 Kap. 8.4.
    #[tool(
        name = "get_summenzeitreihe",
        description = "Get monthly aggregated energy (Summenzeitreihe) for a MaLo. \
                       Returns total_kwh per calendar month with coverage percentage. \
                       Used for MaBiS UTILTS submissions (BK6-22-024 Anlage 3) and \
                       Mehr-/Mindermengensaldo (GPKE Teil 1 Kap. 8.4). \
                       Params: malo_id (required), from / to (ISO 8601 UTC).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_summenzeitreihe(
        &self,
        Parameters(p): Parameters<GetSummenzeitreiheParams>,
    ) -> Result<CallToolResult, McpError> {
        use time::format_description::well_known::Rfc3339;
        let from = p
            .from
            .as_deref()
            .map(|s| time::OffsetDateTime::parse(s, &Rfc3339))
            .transpose()
            .map_err(|e| McpError::invalid_params(format!("invalid from: {e}"), None))?
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
        let to =
            p.to.as_deref()
                .map(|s| time::OffsetDateTime::parse(s, &Rfc3339))
                .transpose()
                .map_err(|e| McpError::invalid_params(format!("invalid to: {e}"), None))?
                .unwrap_or_else(time::OffsetDateTime::now_utc);

        // Aggregate per calendar month in Rust over the version-resolved series.
        // `query` does not filter quality (matching the former SQL, which summed
        // every read); `worst_quality` is the highest-severity flag in the month
        // by `metering::QualityFlag` ordering, which is a stricter "worst" than the
        // former `MIN(quality)` (alphabetical, not severity-ordered).
        let reads = self
            .state
            .repo
            .query(&TimeSeriesQuery {
                malo_id: p.malo_id.clone(),
                from,
                to,
                sparte: None,
                tenant: self.state.tenant.clone(),
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // Bucket through the shared `metering::resample`, exactly as the REST
        // `GET /api/v1/summenzeitreihe/{malo_id}` handler does — a MaBiS figure
        // must not depend on which surface asked for it. Hand-rolling a UTC
        // month-start here diverged from resample's DST-aware Berlin months at
        // every month boundary.
        // …including the register projection and the billable-quality filter. The
        // tool used to map every read straight through, so a MaLo whose data the
        // REST surface reported as one Bezug series answered here with its
        // Einspeisung and its Faulty intervals summed in as well.
        let intervals =
            crate::domain::energy_intervals(&reads, crate::domain::EnergyDirection::Bezug);
        let buckets = metering::resample(&intervals, &metering::ResampleConfig::to_monthly());
        let total_kwh: rust_decimal::Decimal = buckets.iter().map(|b| b.total).sum();

        let months: Vec<serde_json::Value> = buckets
            .iter()
            .map(|b| {
                serde_json::json!({
                    "from": b.from,
                    "to": b.to,
                    "total_kwh": b.total.to_string(),
                    "peak_kw": b.peak_kw,
                    "coverage_pct": b.coverage_pct(),
                    "has_missing_data": b.has_missing_data(),
                    "quality": format!("{:?}", b.quality),
                })
            })
            .collect();

        serde_json::to_string_pretty(&serde_json::json!({
            "malo_id": p.malo_id,
            "from": from,
            "to": to,
            "total_kwh": total_kwh.to_string(),
            "month_count": months.len(),
            "months": months,
            "legal_basis": "MaBiS BK6-22-024 Anlage 3 / GPKE (BK6-24-174) Teil 1 Kap. 8.4 Mehr-/Mindermengensaldo",
        }))
        .map(|s| CallToolResult::success(vec![ContentBlock::text(s)]))
        .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// Get § 60 Abs. 2 MsbG annual energy forecast (Jahresprognose) for a MaLo.
    #[tool(
        name = "get_annual_forecast",
        description = "Compute § 60 Abs. 2 MsbG annual energy consumption forecast (Jahresprognose) \
                       for a MaLo from available meter reads. Returns projected_annual_kwh, \
                       observed_kwh, observed_days, seasonal_correction_applied. \
                       Minimum 7 days of data required. \
                       Params: malo_id (required), from / to (ISO 8601 UTC, optional).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_annual_forecast(
        &self,
        Parameters(p): Parameters<GetAnnualForecastParams>,
    ) -> Result<CallToolResult, McpError> {
        use time::format_description::well_known::Rfc3339;
        let from = p
            .from
            .as_deref()
            .map(|s| time::OffsetDateTime::parse(s, &Rfc3339))
            .transpose()
            .map_err(|e| McpError::invalid_params(format!("invalid from: {e}"), None))?
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
        let to =
            p.to.as_deref()
                .map(|s| time::OffsetDateTime::parse(s, &Rfc3339))
                .transpose()
                .map_err(|e| McpError::invalid_params(format!("invalid to: {e}"), None))?
                .unwrap_or_else(time::OffsetDateTime::now_utc);

        // Mirror the REST `get_annual_forecast` exactly: read the window via the
        // repository, then project onto the canonical Bezug registers. Quality is
        // filtered by both — the projection drops non-billable readings, and
        // `project_annual_consumption` filters them again internally — so the two
        // surfaces agree by construction rather than by coincidence.
        let reads = self
            .state
            .repo
            .query(&TimeSeriesQuery {
                malo_id: p.malo_id.clone(),
                from,
                to,
                sparte: None,
                tenant: self.state.tenant.clone(),
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        // The canonical Bezug projection, as the REST forecast uses: a projection
        // over every register would forecast a year of energy nobody draws.
        let intervals =
            crate::domain::energy_intervals(&reads, crate::domain::EnergyDirection::Bezug);

        // Prior-year window enables the seasonal correction branch — without it
        // every forecast is the naive daily × 365 projection. A missing prior-year
        // window is not an error (seasonal correction is optional), so its read
        // degrades to empty rather than failing the forecast.
        let prior_reads = self
            .state
            .repo
            .query(&TimeSeriesQuery {
                malo_id: p.malo_id.clone(),
                from: from - time::Duration::days(365),
                to: to - time::Duration::days(365),
                sparte: None,
                tenant: self.state.tenant.clone(),
            })
            .await
            .unwrap_or_default();
        let prior_intervals =
            crate::domain::energy_intervals(&prior_reads, crate::domain::EnergyDirection::Bezug);
        let prior = (!prior_intervals.is_empty()).then_some(prior_intervals.as_slice());

        match metering::project_annual_consumption(&intervals, prior) {
            Some(forecast) => serde_json::to_string_pretty(&serde_json::json!({
                // The caller's MaLo, not the forecast's: a projection is
                // arithmetic over a series and does not know whose it is.
                "malo_id": p.malo_id,
                "observation_from": forecast.observation_from,
                "observation_to": forecast.observation_to,
                "observed_kwh": forecast.observed.to_string(),
                "observed_days": forecast.observed_days,
                "target_year_days": forecast.target_year_days,
                "projected_annual_kwh": forecast.projected_annual.to_string(),
                "seasonal_correction_applied": forecast.seasonal_correction_applied,
                "seasonal_factor": forecast.seasonal_factor.to_string(),
                // A projection an agent cannot judge the spread of is one it
                // should not bill on; metering 0.17 supplies the interval.
                "confidence_lower_kwh": forecast.confidence_lower.map(|d| d.to_string()),
                "confidence_upper_kwh": forecast.confidence_upper.map(|d| d.to_string()),
                "prediction_interval_note": metering::AnnualForecast::prediction_interval_note(),
                "legal_basis": "§ 60 Abs. 2 MsbG Jahresprognose",
            }))
            .map(|s| CallToolResult::success(vec![ContentBlock::text(s)]))
            .map_err(|e| McpError::internal_error(e.to_string(), None)),
            None => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "Insufficient data for annual forecast of {}: minimum 7 days of readings required.",
                p.malo_id
            ))])),
        }
    }

    /// Get Gas quality data (Brennwert + Zustandszahl) from PID 13007.
    #[tool(
        name = "get_gas_quality",
        description = "Get Gasbeschaffenheitsdaten (Brennwert in kWh/m³ + Zustandszahl) \
                       for a Gas MaLo from PID 13007 deliveries. Required for Gas m³ → kWh_Hs \
                       conversion (§25 Nr. 4 MessEV / DVGW G 685). \
                       Params: malo_id (required), from / to (date YYYY-MM-DD, optional).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_gas_quality_data(
        &self,
        Parameters(p): Parameters<GetGasQualityParams>,
    ) -> Result<CallToolResult, McpError> {
        // `source_pid`, not `pid` — see the note in `server::gas_quality`.
        let rows = sqlx::query(
            r"SELECT period_from, period_to, brennwert_kwh_per_m3, zustandszahl,
                     source_pid, received_at
              FROM gas_quality_data
              WHERE malo_id = $1
                AND tenant = $2
              ORDER BY period_from DESC LIMIT 20",
        )
        .bind(&p.malo_id)
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if rows.is_empty() {
            return Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "No Gasbeschaffenheitsdaten found for {}. Ensure PID 13007 has been delivered.",
                p.malo_id
            ))]));
        }

        use sqlx::Row;
        let records: Vec<serde_json::Value> = rows.iter().map(|r| {
            let dec = |c: &str| r
                .try_get::<Option<rust_decimal::Decimal>, _>(c)
                .ok()
                .flatten()
                .map(|d| d.to_string());
            serde_json::json!({
            "period_from": r.try_get::<time::Date, _>("period_from").ok().map(|d| d.to_string()),
            "period_to": r.try_get::<time::Date, _>("period_to").ok().map(|d| d.to_string()),
            "brennwert_kwh_per_m3": dec("brennwert_kwh_per_m3"),
            "zustandszahl": dec("zustandszahl"),
            "source_pid": r.try_get::<Option<i32>, _>("source_pid").ok().flatten(),
            "received_at": r.try_get::<time::OffsetDateTime, _>("received_at").ok().map(|t| t.to_string()),
            "legal_basis": "§25 Nr. 4 MessEV / DVGW G 685 kWh_Hs = m³ × Brennwert × Zustandszahl",
        })}).collect();

        serde_json::to_string_pretty(&serde_json::json!({
            "malo_id": p.malo_id,
            "count": records.len(),
            "gas_quality": records,
        }))
        .map(|s| CallToolResult::success(vec![ContentBlock::text(s)]))
        .map_err(|e| McpError::internal_error(e.to_string(), None))
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for EdmdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("edmd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "# edmd — Energy Data Management\n\
             \n\
             Stores MSCONS meter data, iMSys direct push (15-min RLM), virtual meters, \
             quality assessments, Gas quality data, and manages reading orders.\n\
             Quality scoring uses the Hampel filter + the V01–V09/V11/V12 validation engine.\n\
             \n\
             ## Tools (15)\n\
             - `get_timeseries` — read meter reads for a MaLo in a time range\n\
             - `get_imbalance` — Mehr-/Mindermengen imbalance report (GPKE (BK6-24-174) Teil 1 Kap. 8.4)\n\
             - `get_billing_period` — MeterBillingPeriod (arbeitsmenge_kwh, brennwert, spitzenleistung)\n\
             - `get_device_history` — M9 RAG: comprehensive device history for LanceDB indexing\n\
             - `get_quality_warnings` — Hampel quality warnings (grade A/B/C/F)\n\
             - `list_reading_orders` — Ablesesteuerung reading orders for a MaLo\n\
             - `list_overdue_reading_orders` — overdue reading orders (§ 40b Abs. 1 EnWG compliance)\n\
             - `trigger_jahresablesung` — launch annual SLP reading campaign\n\
             - `trigger_substitution` — generate + store § 60 Abs. 2 MsbG Ersatzwerte for a gap window\n\
             - `get_correction_history` — § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) bitemporal correction audit trail\n\
             - `validate_timeseries` — V01–V09/V11/V12 validation (gaps, overlaps, outliers, DST, quality)\n\
             - `get_quality_assessments` — per-batch quality history (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD))\n\
             - `get_summenzeitreihe` — monthly aggregated kWh for MaBiS / GPKE Teil 1 Kap. 8.4\n\
             - `get_annual_forecast` — § 60 Abs. 2 MsbG Jahresprognose from available reads\n\
             - `get_gas_quality` — PID 13007 Brennwert + Zustandszahl for Gas kWh_Hs conversion\n\
             \n\
             ## Prompts (5)\n\
             - `analyze-consumption`, `submit-mscons`, `quality-assessment`,\n\
             - `jahresablesung-workflow`, `reading-order-lifecycle`\n\
             \n\
             ## Notes\n\
             - Grade F blocks billing; grade C/F emits de.messwert.reading.quality.warning.\n\
             - Direct push: POST /api/v1/meter-reads/rlm/{malo_id} for 15-min RLM.\n\
             - Virtual meters: GET /api/v1/virtual/{id}/lastgang (§42b EnWG GGV).\n\
             - Substitute values: POST /api/v1/meter-reads/{malo_id}/substitute (§ 60 Abs. 2 MsbG).",
        )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

/// The Cedar action a destructive MCP tool requires, or `None` for a read tool.
///
/// The blanket `use-mcp` gate the auth middleware applies allows any role of the
/// tenant. The two mutating tools call the same cores as their REST endpoints —
/// which require `write-timeseries` / `write-reading-order` (MSB/NB/admin) — so
/// they must be gated with the same action, or an LF-role token escalates to
/// Ersatzwert generation and §40 campaign dispatch through MCP.
fn destructive_tool_action(body: &[u8]) -> Option<&'static str> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    if v.get("method")?.as_str()? != "tools/call" {
        return None;
    }
    match v.get("params")?.get("name")?.as_str()? {
        "trigger_substitution" => Some("write-timeseries"),
        "trigger_jahresablesung" => Some("write-reading-order"),
        _ => None,
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<EdmdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    // A `tools/call` for a destructive tool must clear the same Cedar write
    // action its REST twin enforces, not just `use-mcp`. Buffer the body to read
    // the tool name, authorize, then reconstruct the request for `authenticate`.
    const MAX_MCP_BODY: usize = 1024 * 1024;
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_MCP_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "MCP request body too large").into_response();
        }
    };
    if let Some(action) = destructive_tool_action(&bytes)
        && let Err(resp) = state.auth.authorize(&parts.headers, action)
    {
        return resp;
    }
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    state.auth.authenticate(request, next).await
}
// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: Arc<EdmdMcpState>, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)))
        .with_cancellation_token(shutdown);

    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(EdmdMcpHandler::new(state.clone()))
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

#[cfg(test)]
mod auth_gate_tests {
    use super::destructive_tool_action;

    fn call(name: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": {} }
        }))
        .unwrap()
    }

    #[test]
    fn destructive_tools_map_to_their_rest_write_action() {
        assert_eq!(
            destructive_tool_action(&call("trigger_substitution")),
            Some("write-timeseries")
        );
        assert_eq!(
            destructive_tool_action(&call("trigger_jahresablesung")),
            Some("write-reading-order")
        );
    }

    #[test]
    fn read_tools_and_non_tool_calls_need_no_extra_action() {
        assert_eq!(destructive_tool_action(&call("get_timeseries")), None);
        assert_eq!(
            destructive_tool_action(
                &serde_json::to_vec(&serde_json::json!({"method": "tools/list"})).unwrap()
            ),
            None
        );
        assert_eq!(destructive_tool_action(b"not json"), None);
    }
}
