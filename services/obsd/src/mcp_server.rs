//! MCP (Model Context Protocol) server for `obsd`.
//!
//! Exposes process projection, KPI report, and overdue alert reads.
//! Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `get_process`     | Read a process projection by UUID |
//! | `list_overdue`    | List processes past their regulatory deadline |
//! | `get_kpi_report`  | Get BNetzA KPI report for a PID and month |

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
pub struct ObsdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetProcessParams {
    /// UUID of the process projection.
    pub process_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetKpiReportParams {
    /// Prüfidentifikator (e.g. 55001 for GPKE Lieferbeginn).
    pub pid: u32,
    /// Billing month in `YYYY-MM` format (default: current month).
    pub period: Option<String>,
}

// ── MCP handler ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ObsdMcpHandler {
    state: Arc<ObsdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<ObsdMcpHandler>,
}

#[tool_router]
impl ObsdMcpHandler {
    fn new(state: Arc<ObsdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// Read a process projection by UUID.
    ///
    /// Returns the full CQRS read-model entry: PID, state, partner GLNs,
    /// timestamps, and deadline risk score.  The data is a projection of
    /// all `de.mako.*` events received so far for this process.
    #[tool(description = "Read a process projection by UUID")]
    async fn get_process(
        &self,
        Parameters(p): Parameters<GetProcessParams>,
    ) -> Result<CallToolResult, McpError> {
        let process_id: uuid::Uuid = p
            .process_id
            .parse()
            .map_err(|_| McpError::invalid_params("process_id is not a valid UUID", None))?;

        let row = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                i32,
                String,
                Option<String>,
                Option<String>,
                time::OffsetDateTime,
                Option<time::OffsetDateTime>,
                serde_json::Value,
            ),
        >(
            r#"
            SELECT process_id, pid, state, partner_mp_id, mdm_role,
                   initiated_at, completed_at, projection
            FROM process_projections
            WHERE process_id = $1 AND tenant = $2
            "#,
        )
        .bind(process_id)
        .bind(&self.state.tenant)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((
                process_id,
                pid,
                state,
                partner_mp_id,
                mdm_role,
                initiated_at,
                completed_at,
                projection,
            )) => ContentBlock::json(serde_json::json!({
                "process_id": process_id,
                "pid": pid,
                "state": state,
                "partner_mp_id": partner_mp_id,
                "mdm_role": mdm_role,
                "initiated_at": initiated_at,
                "completed_at": completed_at,
                "projection": projection,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "process_not_found: No process projection for id '{}'.",
                p.process_id
            ))])),
        }
    }

    /// List processes that have exceeded their regulatory deadline.
    ///
    /// Returns processes with `deadline_at < now()` and `state NOT IN
    /// ('completed', 'rejected', 'cancelled')`.  These are processes that
    /// must be escalated — missing the APERAK or response deadline is a
    /// regulatory violation under BNetzA monitoring.
    ///
    /// Returns up to 200 results ordered by deadline ascending (most urgent first).
    #[tool(description = "List processes past their regulatory deadline (most urgent first)")]
    async fn list_overdue(&self) -> Result<CallToolResult, McpError> {
        let rows = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                i32,
                String,
                Option<String>,
                time::OffsetDateTime,
                Option<time::OffsetDateTime>,
            ),
        >(
            r#"
            SELECT process_id, pid, state, partner_mp_id, initiated_at, deadline_at
            FROM process_projections
            WHERE tenant = $1
              AND deadline_at IS NOT NULL
              AND deadline_at < NOW()
              AND state NOT IN ('completed', 'rejected', 'cancelled')
            ORDER BY deadline_at ASC
            LIMIT 200
            "#,
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let overdue: Vec<serde_json::Value> = rows
            .into_iter()
            .map(
                |(process_id, pid, state, partner_mp_id, initiated_at, deadline_at)| {
                    serde_json::json!({
                        "process_id": process_id,
                        "pid": pid,
                        "state": state,
                        "partner_mp_id": partner_mp_id,
                        "initiated_at": initiated_at,
                        "deadline_at": deadline_at,
                    })
                },
            )
            .collect();

        ContentBlock::json(serde_json::json!({
            "overdue": overdue,
            "count": overdue.len(),
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Get the BNetzA KPI report for a Prüfidentifikator and billing month.
    ///
    /// Returns regulatory KPIs required by BNetzA monitoring: total processes,
    /// completion rate, median lead time, and APERAK violation count.
    /// Use for preparing the annual BNetzA Marktkommunikationsbericht.
    ///
    /// `pid` — BDEW Prüfidentifikator (e.g. 55001 for GPKE Lieferbeginn).
    /// `period` — `YYYY-MM` (default: current month).
    #[tool(description = "Get BNetzA KPI report for a PID and billing month (YYYY-MM)")]
    async fn get_kpi_report(
        &self,
        Parameters(p): Parameters<GetKpiReportParams>,
    ) -> Result<CallToolResult, McpError> {
        use time::{Date, Month, OffsetDateTime};

        let today = OffsetDateTime::now_utc().date();
        let (year, month_u8) = if let Some(period) = p.period.as_deref() {
            let parts: Vec<&str> = period.splitn(2, '-').collect();
            if parts.len() != 2 {
                return Err(McpError::invalid_params("period must be YYYY-MM", None));
            }
            let y: i32 = parts[0]
                .parse()
                .map_err(|_| McpError::invalid_params("invalid year in period", None))?;
            let m: u8 = parts[1]
                .parse()
                .map_err(|_| McpError::invalid_params("invalid month in period", None))?;
            (y, m)
        } else {
            (today.year(), today.month() as u8)
        };

        let month = Month::try_from(month_u8)
            .map_err(|_| McpError::invalid_params("month out of range 1–12", None))?;
        let from = Date::from_calendar_date(year, month, 1)
            .map_err(|_| McpError::invalid_params("invalid date", None))?;
        let (ny, nm) = if month_u8 == 12 {
            (year + 1, Month::January)
        } else {
            (year, Month::try_from(month_u8 + 1).unwrap())
        };
        let to = Date::from_calendar_date(ny, nm, 1)
            .unwrap()
            .previous_day()
            .unwrap_or(from);

        let from_ts = OffsetDateTime::new_utc(from, time::Time::MIDNIGHT);
        let to_ts = OffsetDateTime::new_utc(to, time::Time::MIDNIGHT);

        let row = sqlx::query_as::<_, (i64, i64, i64, i64, Option<f64>)>(
            r#"
            SELECT
                COUNT(*) AS total,
                COUNT(*) FILTER (WHERE state = 'completed') AS completed,
                COUNT(*) FILTER (WHERE state IN ('rejected', 'cancelled')) AS rejected,
                COUNT(*) FILTER (
                    WHERE deadline_at IS NOT NULL
                      AND deadline_at < COALESCE(completed_at, NOW())
                      AND state NOT IN ('completed', 'rejected', 'cancelled')
                ) AS aperak_violations,
                AVG(EXTRACT(EPOCH FROM (completed_at - initiated_at)) / 3600.0)
                    FILTER (WHERE completed_at IS NOT NULL) AS avg_lead_time_hours
            FROM process_projections
            WHERE tenant = $1
              AND pid = $2
              AND initiated_at >= $3
              AND initiated_at <= $4
            "#,
        )
        .bind(&self.state.tenant)
        .bind(p.pid as i32)
        .bind(from_ts)
        .bind(to_ts)
        .fetch_one(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let (total, completed, rejected, aperak_violations, avg_lead_time_hours) = row;
        let completion_rate = if total > 0 {
            completed as f64 / total as f64
        } else {
            0.0
        };

        ContentBlock::json(serde_json::json!({
            "pid": p.pid,
            "period": format!("{year}-{month_u8:02}"),
            "total": total,
            "completed": completed,
            "rejected_or_cancelled": rejected,
            "aperak_violations": aperak_violations,
            "completion_rate": (completion_rate * 100.0).round() / 100.0,
            "avg_lead_time_hours": avg_lead_time_hours,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[tool_handler]
impl ServerHandler for ObsdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("obsd", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "# obsd — Process Observability\n\
             \n\
             CQRS read model of all `de.mako.*` events.  Deadline risk monitoring \
             and BNetzA KPI reports.\n\
             \n\
             ## Tools\n\
             - `get_process` — read a full process projection by UUID\n\
             - `list_overdue` — list processes past their regulatory deadline (escalation queue)\n\
             - `get_kpi_report` — BNetzA KPI report for a PID and billing month\n\
             \n\
             ## Process states\n\
             `initiated` → `running` → `completed` | `rejected` | `cancelled` | `aperak_timeout`\n\
             \n\
             ## Regulatory\n\
             Overdue processes are a BNetzA compliance risk.  Report them in the annual\n\
             Marktkommunikationsbericht (§ 12 MsbG / BNetzA Monitoring).",
            )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<ObsdMcpState>>,
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

pub fn router(state: Arc<ObsdMcpState>, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)))
        .with_cancellation_token(shutdown);

    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(ObsdMcpHandler::new(state.clone()))
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
