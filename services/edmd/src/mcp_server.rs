//! MCP (Model Context Protocol) server for `edmd`.
//!
//! Exposes meter data time-series and billing-period summaries.
//! Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `get_timeseries`     | Read meter data for a MaLo in a time range |
//! | `get_imbalance`      | Read the Mehr-/Mindermengen imbalance report |
//! | `get_billing_period` | Aggregated billing period summary (arbeitsmenge, spitzenleistung, brennwert) |

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
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
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

// ── MCP handler ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EdmdMcpHandler {
    state: Arc<EdmdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<EdmdMcpHandler>,
}

#[tool_router]
impl EdmdMcpHandler {
    fn new(state: Arc<EdmdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// Read meter data time-series for a MaLo in a given date-time range.
    ///
    /// Returns an array of `Messwert` records (dtm_from, dtm_to, value,
    /// unit, bo4e_version) ordered by `dtm_from` ascending.  Empty array
    /// means no MSCONS data has been received yet for this MaLo and period.
    #[tool(description = "Read meter data (Messwert) for a MaLo between from..to (ISO 8601)")]
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
    #[tool(description = "Get Mehr-/Mindermengen imbalance report for a MaLo and billing month")]
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
        description = "Get aggregated billing period summary for a MaLo (arbeitsmenge, spitzenleistung, brennwert, zustandszahl). Used by invoicd and netzbilanzd."
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
}

#[tool_handler]
impl ServerHandler for EdmdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("edmd", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "# edmd — Energy Data Management\n\
             \n\
             Stores MSCONS meter data and computes Mehr-/Mindermengen imbalances.\n\
             \n\
             ## Tools\n\
             - `get_timeseries` — read Messwert records for a MaLo in a time range\n\
             - `get_imbalance` — get MMM imbalance report for a MaLo and billing month\n\
             \n\
             ## Notes\n\
             - `get_timeseries` returns up to 5 000 readings per call.\n\
             - `get_imbalance` aggregates raw readings; use for MMM clearing preview.",
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
