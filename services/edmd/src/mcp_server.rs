//! MCP (Model Context Protocol) server for `edmd`.
//!
//! Exposes meter data time-series and billing-period summaries.
//! Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `get_timeseries`       | Read meter data for a MaLo in a time range |
//! | `get_imbalance`        | Read the Mehr-/Mindermengen imbalance report |
//! | `get_billing_period`   | Aggregated billing period summary (arbeitsmenge, spitzenleistung, brennwert) |
//! | `get_device_history`   | M9 RAG: Comprehensive device history for LanceDB indexing |
//! | `get_quality_warnings` | M7: Hampel-filter quality warnings (grade A/B/C/F, outliers, spikes, gaps) |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |---|---|
//! | `analyze-consumption` | Step-by-step consumption analysis |
//! | `submit-mscons` | Step-by-step MSCONS ingestion guide |
//! | `quality-assessment` | M7 Hampel quality assessment guide |

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
use mako_service::{
    cedar::CedarEnforcer,
    oidc::{Claims, OidcVerifier},
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

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EdmdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
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
    /// Year (e.g. 2025).
    pub year: i32,
    /// Month (1–12).
    pub month: u8,
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

// ── M9: Device history RAG ─────────────────────────────────────────────────

/// Parameters for `get_device_history` MCP tool (M9 — LanceDB MSB service history RAG).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetDeviceHistoryParams {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// How many days back to look (default 90).
    pub days_back: Option<i64>,
}

// ── M7: Quality warnings ───────────────────────────────────────────────────

/// Parameters for `get_quality_warnings` MCP tool (M7 — Hampel filter quality scoring).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetQualityWarningsParams {
    /// 33-character Marktlokations-ID.
    pub malo_id: String,
    /// ISO-8601 start datetime (inclusive). Defaults to 30 days ago when omitted.
    pub from: Option<String>,
    /// ISO-8601 end datetime (exclusive). Defaults to now when omitted.
    pub to: Option<String>,
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

        let rows =
            sqlx::query_as::<_, (time::OffsetDateTime, time::OffsetDateTime, String, String)>(
                r#"
            SELECT dtm_from, dtm_to, messwert, bo4e_version
            FROM meter_readings
            WHERE tenant = $1
              AND malo_id = $2
              AND dtm_from >= $3
              AND dtm_to <= $4
            ORDER BY dtm_from
            LIMIT 5000
            "#,
            )
            .bind(&self.state.tenant)
            .bind(&p.malo_id)
            .bind(from)
            .bind(to)
            .fetch_all(&self.state.pool)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let readings: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(dtm_from, dtm_to, messwert, bo4e_version)| {
                serde_json::json!({
                    "dtm_from": dtm_from,
                    "dtm_to": dtm_to,
                    "messwert": messwert,
                    "bo4e_version": bo4e_version,
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

    /// Read the Mehr-/Mindermengen imbalance report for a MaLo and month.
    ///
    /// Returns the aggregated Mehr-/Mindermengen (MMM) imbalance for a given
    /// billing month.  The report is used by `invoicd` to compute the
    /// monthly selbstausgestellt INVOIC 31006 MMM amount.  Returns an error when no data exists yet.
    #[tool(
        description = "Get Mehr-/Mindermengen imbalance report for a MaLo and billing month",
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

        let from_ts = time::OffsetDateTime::new_utc(from, time::Time::MIDNIGHT);
        let to_ts = time::OffsetDateTime::new_utc(to, time::Time::MIDNIGHT);

        let row = sqlx::query_as::<_, (Option<f64>, Option<f64>, i64)>(
            r#"
            SELECT
                SUM(CASE WHEN messwert::numeric > 0 THEN messwert::numeric ELSE 0 END),
                SUM(CASE WHEN messwert::numeric < 0 THEN ABS(messwert::numeric) ELSE 0 END),
                COUNT(*)
            FROM meter_readings
            WHERE tenant = $1
              AND malo_id = $2
              AND dtm_from >= $3
              AND dtm_to <= $4
            "#,
        )
        .bind(&self.state.tenant)
        .bind(&p.malo_id)
        .bind(from_ts)
        .bind(to_ts)
        .fetch_one(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let (mehr, minder, count) = row;
        if count == 0 {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "no_data: No meter readings for MaLo '{}' in {}-{:02}.",
                p.malo_id, p.year, p.month
            ))]));
        }

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "year": p.year,
            "month": p.month,
            "mehrmengen_kwh": mehr.unwrap_or(0.0),
            "mindermengen_kwh": minder.unwrap_or(0.0),
            "reading_count": count,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Get the aggregated billing-period summary for a MaLo.
    ///
    /// Returns arbeitsmenge (total kWh), spitzenleistung_kw (RLM Strom peak
    /// demand), and Gas conversion factors (brennwert, zustandszahl).
    /// Used by `invoicd` (M16) for RLM plausibility and by `netzbilanzd` (N4)
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

        let from_ts = period_from.midnight().assume_utc();
        let to_ts = period_to
            .next_day()
            .unwrap_or(period_to)
            .midnight()
            .assume_utc();

        // Query pre-aggregated billing periods first, fall back to raw aggregation.
        let pre = sqlx::query(
            r"SELECT arbeitsmenge_kwh, spitzenleistung_kw, brennwert_kwh_per_m3,
                     zustandszahl, messtyp, sparte, quality, computed_at
              FROM meter_billing_periods
              WHERE malo_id = $1 AND period_from = $2 AND period_to = $3",
        )
        .bind(&p.malo_id)
        .bind(period_from)
        .bind(period_to)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        if let Some(row) = pre {
            use sqlx::Row as _;
            ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id,
                "period_from": period_from.to_string(),
                "period_to": period_to.to_string(),
                "messtyp": row.try_get::<String, _>("messtyp").ok(),
                "sparte": row.try_get::<String, _>("sparte").ok(),
                "arbeitsmenge_kwh": row.try_get::<String, _>("arbeitsmenge_kwh").ok(),
                "spitzenleistung_kw": row.try_get::<Option<String>, _>("spitzenleistung_kw").ok().flatten(),
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
            // On-the-fly aggregation from meter_reads.
            let row = sqlx::query_as::<_, (Option<f64>, Option<f64>, i64)>(
                r#"SELECT
                    SUM(quantity_kwh::numeric)::float,
                    MAX(CASE WHEN EXTRACT(EPOCH FROM (dtm_to - dtm_from)) = 900
                              THEN (quantity_kwh::numeric * 4)::float END),
                    COUNT(*)
                FROM meter_reads
                WHERE malo_id = $1 AND dtm_from >= $2 AND dtm_to <= $3"#,
            )
            .bind(&p.malo_id)
            .bind(from_ts)
            .bind(to_ts)
            .fetch_one(&self.state.pool)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            let (total_kwh, spitzenleistung_kw, count) = row;
            if count == 0 {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "no_data: No meter reads for MaLo '{}' in {}/{} — {}.",
                    p.malo_id,
                    p.period_from,
                    p.period_to.as_deref().unwrap_or(&p.period_from),
                    "ensure MSCONS data has been ingested for this period"
                ))]));
            }

            ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id,
                "period_from": period_from.to_string(),
                "period_to": period_to.to_string(),
                "arbeitsmenge_kwh": total_kwh,
                "spitzenleistung_kw": spitzenleistung_kw,
                "read_count": count,
                "source": "on_the_fly",
                "note": "Pre-aggregated summary not available — computed from raw reads. Brennwert/Zustandszahl not yet available.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
        }
    }

    // ── M9: Device history RAG ─────────────────────────────────────────────

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
        let orders = sqlx::query(
            r"SELECT id, anlass, geplant_am, ausfuehrt_bis, status, notiz
              FROM ablese_auftraege
              WHERE malo_id = $1 AND geplant_am >= $2::timestamptz
              ORDER BY geplant_am DESC
              LIMIT 50",
        )
        .bind(&p.malo_id)
        .bind(since)
        .fetch_all(&self.state.pool)
        .await
        .unwrap_or_default();

        // ── Quality warnings from direct push ────────────────────────────
        let push_sessions = sqlx::query(
            r"SELECT session_id, source, obis_code, interval_count,
                     period_from, period_to, quality_summary, created_at
              FROM direct_push_sessions
              WHERE malo_id = $1 AND created_at >= $2
              ORDER BY created_at DESC
              LIMIT 20",
        )
        .bind(&p.malo_id)
        .bind(since)
        .fetch_all(&self.state.pool)
        .await
        .unwrap_or_default();

        // ── Reads with quality warnings ───────────────────────────────────
        let warn_reads = sqlx::query(
            r"SELECT dtm_from, dtm_to, quantity_kwh, quality, quality_warnings
              FROM meter_reads
              WHERE malo_id = $1
                AND dtm_from >= $2
                AND quality_warnings IS NOT NULL
              ORDER BY dtm_from DESC
              LIMIT 20",
        )
        .bind(&p.malo_id)
        .bind(since)
        .fetch_all(&self.state.pool)
        .await
        .unwrap_or_default();

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
                let geplant: Option<time::OffsetDateTime> = r.try_get("geplant_am").ok();
                let notiz: Option<String> = r.try_get("notiz").ok().flatten();
                let geplant_str = geplant.map(|t| t.date().to_string()).unwrap_or_default();
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

        // Quality warnings section
        if warn_reads.is_empty() {
            doc.push_str("## Meter Data Quality Warnings\nNo quality warnings in this period.\n\n");
        } else {
            doc.push_str("## Meter Data Quality Warnings\n\n");
            for r in &warn_reads {
                let dtm_from: Option<time::OffsetDateTime> = r.try_get("dtm_from").ok();
                let qty: String = r.try_get("quantity_kwh").unwrap_or_default();
                let quality: String = r.try_get("quality").unwrap_or_default();
                let warnings: Option<serde_json::Value> =
                    r.try_get("quality_warnings").ok().flatten();

                let ts = dtm_from.map(|t| t.date().to_string()).unwrap_or_default();
                let warn_str = if let Some(w) = &warnings {
                    let gaps = w.get("gaps_detected").and_then(|v| v.as_i64()).unwrap_or(0);
                    let zero_run = w
                        .get("zero_run_length")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    format!("gaps={gaps} zero_run={zero_run}")
                } else {
                    "unknown issue".to_owned()
                };

                doc.push_str(&format!(
                    "- {ts}: {qty} kWh, quality={quality}, warnings: {warn_str}\n",
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
            qw = warn_reads.len(),
        ));

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "days_back": days,
            "document_text": doc,
            "rag_source": format!("msb-{}", p.malo_id),
            "reading_orders_count": orders.len(),
            "push_sessions_count": push_sessions.len(),
            "quality_warnings_count": warn_reads.len(),
            "ingest_hint": "POST this document_text to agentd POST /api/v1/rag/ingest",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// `get_quality_warnings` — M7: query Hampel-filter quality warnings for a MaLo.
    ///
    /// Returns `meter_reads` rows where `quality_warnings->>'has_warnings' = 'true'`
    /// in the given time range, structured by the Hampel-k3-t3 algorithm.
    #[allow(dead_code)]
    #[tool(
        description = "M7: Query Hampel-filter quality warnings for a MaLo in a time range. \
Returns grade (A/B/C/F), outlier/spike timestamps, gaps detected, and coverage %.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_quality_warnings(
        &self,
        Parameters(p): Parameters<GetQualityWarningsParams>,
    ) -> Result<CallToolResult, McpError> {
        use sqlx::Row as _;
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

        let rows = sqlx::query(
            r#"SELECT dtm_from, dtm_to, quality_warnings
               FROM meter_reads
               WHERE malo_id = $1
                 AND dtm_from >= $2
                 AND dtm_from < $3
                 AND quality_warnings IS NOT NULL
                 AND (quality_warnings->>'has_warnings')::boolean = true
               ORDER BY dtm_from
               LIMIT 500"#,
        )
        .bind(&p.malo_id)
        .bind(from_dt)
        .bind(to_dt)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let warnings: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let dtm_from: time::OffsetDateTime = r.get("dtm_from");
                let dtm_to: time::OffsetDateTime = r.get("dtm_to");
                let mut w: serde_json::Value = r
                    .try_get("quality_warnings")
                    .unwrap_or(serde_json::json!({}));
                w["from_ts"] = serde_json::Value::String(dtm_from.to_string());
                w["to_ts"] = serde_json::Value::String(dtm_to.to_string());
                w
            })
            .collect();

        let total = warnings.len();
        let grade_counts = {
            let mut a = 0u32;
            let mut b = 0u32;
            let mut c = 0u32;
            let mut f = 0u32;
            for w in &warnings {
                match w.get("grade").and_then(|g| g.as_str()).unwrap_or("F") {
                    "A" => a += 1,
                    "B" => b += 1,
                    "C" => c += 1,
                    _ => f += 1,
                }
            }
            serde_json::json!({ "A": a, "B": b, "C": c, "F": f })
        };

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "window_from": from_dt.to_string(),
            "window_to": to_dt.to_string(),
            "total_warning_reads": total,
            "grade_counts": grade_counts,
            "algorithm": "hampel_k3_t3",
            "warnings": warnings,
            "hint": "Use POST /api/v1/quality-score/{malo_id} to retroactively rescore this MaLo.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
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
                "1. Use `get_meter_readings` with malo_id and time range to fetch MSCONS data.\n\
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
                 4. Use `get_meter_readings` to verify the ingestion was correct.\n\n\
                 For Iceberg archive queries (bulk historical data):\n\
                 GET /api/v1/archive/{malo_id}?from=...&to=... returns Parquet-backed results.",
            ),
        ]
    }

    #[prompt(
        name = "quality-assessment",
        description = "Step-by-step: assess meter read quality for a MaLo using the Hampel filter (M7)"
    )]
    async fn quality_assessment_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I assess the quality of meter readings for a MaLo?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "M7 quality scoring uses the **Hampel filter** (window k=3, threshold t=3.0 robust sigma).\n\
                 This is state-of-the-art for time-series meter data quality assessment.\n\n\
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
             Stores MSCONS meter data and computes Mehr-/Mindermengen imbalances.\n\
             M7 quality scoring uses the Hampel filter (window k=3, threshold t=3.0 robust σ).\n\
             \n\
             ## Tools\n\
             - `get_timeseries` — read Messwert records for a MaLo in a time range\n\
             - `get_imbalance` — get MMM imbalance report for a MaLo and billing month\n\
             - `get_billing_period` — get MeterBillingPeriod (arbeitsmenge_kwh, brennwert, spitzenleistung)\n\
             - `get_device_history` — M9 RAG: comprehensive device history for LanceDB indexing\n\
             - `get_quality_warnings` — M7: query Hampel-filter quality warnings (grade A/B/C/F)\n\
             \n\
             ## Prompts\n\
             - `analyze-consumption` — step-by-step consumption analysis\n\
             - `submit-mscons` — step-by-step MSCONS ingestion\n\
             - `quality-assessment` — step-by-step M7 Hampel quality assessment\n\
             \n\
             ## Notes\n\
             - `get_timeseries` returns up to 5 000 readings per call.\n\
             - `get_imbalance` aggregates raw readings; use for MMM clearing preview.\n\
             - `get_quality_warnings` queries `quality_warnings JSONB` column; grade F blocks billing.\n\
             - Retroactive rescoring: `POST /api/v1/quality-score/{malo_id}?from=&to=`",
        )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<EdmdMcpState>>,
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
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "Authorization: Bearer <token> required for /mcp",
            )
                .into_response();
        }
    };

    let claims = match state.oidc.verify(&token) {
        Ok(c) => Claims(c),
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "401 Unauthorized: invalid token").into_response();
        }
    };

    if let Err(e) = state
        .cedar
        .check(&claims.principal(), "use-mcp", &state.tenant)
    {
        return (StatusCode::FORBIDDEN, format!("403 Forbidden: {e}")).into_response();
    }

    next.run(request).await
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
