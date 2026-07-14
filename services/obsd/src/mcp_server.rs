//! MCP (Model Context Protocol) server for `obsd`.
//!
//! Exposes process projection, KPI report, and overdue alert reads.
//! Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Tools (6)
//!
//! | Tool | Description |
//! |---|---|
//! | `get_process`             | Read a process projection by UUID |
//! | `list_overdue_processes`  | List MaKo processes past their regulatory deadline |
//! | `get_kpi_report`          | BNetzA KPI report for a PID and billing month |
//! | `get_parity_report`       | §20 EnWG affiliate vs. non-affiliate completion rates |
//! | `get_stp_rate`            | Rolling STP rate across all process families |
//! | `list_processes_by_family`| List processes by workflow family (gpke/wim/geli-gas/…) |

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

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ObsdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetParityReportParams {
    /// Rolling window in days (default: 90).
    pub days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetStpRateParams {
    /// Rolling window in days (default: 30).
    pub days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListByFamilyParams {
    /// Process family: `gpke`, `wim`, `geli-gas`, `wim-gas`, `gabi-gas`, `mabis`, `unknown`.
    pub family: String,
    /// Optional state filter: `initiated`, `running`, `completed`, `rejected`, `cancelled`, `aperak_timeout`.
    pub state: Option<String>,
    /// Maximum results (default: 50, max: 500).
    pub limit: Option<u32>,
}

// ── MCP handler ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ObsdMcpHandler {
    state: Arc<ObsdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<ObsdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<ObsdMcpHandler>,
}

#[tool_router]
impl ObsdMcpHandler {
    fn new(state: Arc<ObsdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    /// Read a process projection by UUID.
    ///
    /// Returns the full CQRS read-model entry: PID, state, partner GLNs,
    /// timestamps, and deadline risk score.  The data is a projection of
    /// all `de.mako.*` events received so far for this process.
    #[tool(
        description = "Read a process projection by UUID",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_process(
        &self,
        Parameters(p): Parameters<GetProcessParams>,
    ) -> Result<CallToolResult, McpError> {
        let process_id: uuid::Uuid = p
            .process_id
            .parse()
            .map_err(|_| McpError::invalid_params("process_id is not a valid UUID", None))?;

        let row = sqlx::query(
            r#"
            SELECT process_id, pid, family, workflow_name, state, malo_id, partner_mp_id,
                   mdm_role, deadline_at, deadline_risk, started_at, last_event_at,
                   erc_code, initiator_is_affiliate
            FROM process_projections
            WHERE process_id = $1 AND tenant = $2
            "#,
        )
        .bind(process_id)
        .bind(&self.state.tenant)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row;
        match row {
            Some(r) => ContentBlock::json(serde_json::json!({
                "process_id": r.try_get::<uuid::Uuid, _>("process_id").map(|v| v.to_string()).unwrap_or_default(),
                "pid": r.try_get::<i32, _>("pid").unwrap_or(0),
                "family": r.try_get::<String, _>("family").unwrap_or_default(),
                "workflow_name": r.try_get::<String, _>("workflow_name").unwrap_or_default(),
                "state": r.try_get::<String, _>("state").unwrap_or_default(),
                "malo_id": r.try_get::<Option<String>, _>("malo_id").unwrap_or(None),
                "partner_mp_id": r.try_get::<Option<String>, _>("partner_mp_id").unwrap_or(None),
                "mdm_role": r.try_get::<Option<String>, _>("mdm_role").unwrap_or(None),
                "deadline_at": r.try_get::<Option<time::OffsetDateTime>, _>("deadline_at").unwrap_or(None),
                "deadline_risk": r.try_get::<String, _>("deadline_risk").unwrap_or_default(),
                "started_at": r.try_get::<time::OffsetDateTime, _>("started_at").ok(),
                "last_event_at": r.try_get::<time::OffsetDateTime, _>("last_event_at").ok(),
                "erc_code": r.try_get::<Option<String>, _>("erc_code").unwrap_or(None),
                "initiator_is_affiliate": r.try_get::<bool, _>("initiator_is_affiliate").unwrap_or(false),
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
    async fn list_overdue_processes(&self) -> Result<CallToolResult, McpError> {
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
            SELECT process_id, pid, state, partner_mp_id, started_at, deadline_at
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
                |(process_id, pid, state, partner_mp_id, started_at, deadline_at)| {
                    serde_json::json!({
                        "process_id": process_id,
                        "pid": pid,
                        "state": state,
                        "partner_mp_id": partner_mp_id,
                        "started_at": started_at,
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
    #[tool(
        description = "Get BNetzA KPI report for a PID and billing month (YYYY-MM)",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
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
                ) AS aperak_violations,
                AVG(EXTRACT(EPOCH FROM (completed_at - started_at)) / 3600.0)
                    FILTER (WHERE completed_at IS NOT NULL) AS avg_lead_time_hours
            FROM process_projections
            WHERE tenant = $1
              AND pid = $2
              AND started_at >= $3
              AND started_at <= $4
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

    /// §20 EnWG Diskriminierungsfreiheitspflicht parity report.
    ///
    /// Compares affiliate (initiator_is_affiliate=true) vs. non-affiliate
    /// completion rates for Lieferbeginn PIDs (55001, 55016, 44001) over the
    /// last `days` days.  BNetzA expects the completion rate gap to be < 2 pp.
    #[tool(
        description = "§20 EnWG parity report: compare affiliate vs. non-affiliate completion rates for Lieferbeginn processes (PIDs 55001, 55016, 44001). Returns stp_rate, completion_rate, and parity_gap_pp. BNetzA target: parity_gap_pp < 2.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_parity_report(
        &self,
        Parameters(p): Parameters<GetParityReportParams>,
    ) -> Result<CallToolResult, McpError> {
        use sqlx::Row;
        let days = p.days.unwrap_or(90) as i32;
        let rows = sqlx::query(
            r#"
            SELECT
                initiator_is_affiliate,
                COUNT(*) AS total,
                COUNT(*) FILTER (WHERE state = 'completed') AS completed,
                COUNT(*) FILTER (WHERE state IN ('rejected','cancelled','aperak_timeout')) AS not_completed
            FROM process_projections
            WHERE tenant = $1
              AND pid IN (55001, 55016, 44001)
              AND started_at >= NOW() - ($2 || ' days')::INTERVAL
            GROUP BY initiator_is_affiliate
            "#,
        )
        .bind(&self.state.tenant)
        .bind(days)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let mut affiliate =
            serde_json::json!({ "total": 0, "completed": 0, "completion_rate": 0.0 });
        let mut non_affiliate =
            serde_json::json!({ "total": 0, "completed": 0, "completion_rate": 0.0 });

        for row in &rows {
            let is_aff: bool = row.try_get("initiator_is_affiliate").unwrap_or(false);
            let total: i64 = row.try_get("total").unwrap_or(0);
            let completed: i64 = row.try_get("completed").unwrap_or(0);
            let rate = if total > 0 {
                completed as f64 / total as f64
            } else {
                0.0
            };
            let entry = serde_json::json!({
                "total": total, "completed": completed,
                "completion_rate": (rate * 1000.0).round() / 1000.0,
            });
            if is_aff {
                affiliate = entry;
            } else {
                non_affiliate = entry;
            }
        }

        let aff_rate = affiliate["completion_rate"].as_f64().unwrap_or(0.0);
        let non_aff_rate = non_affiliate["completion_rate"].as_f64().unwrap_or(0.0);
        let parity_gap_pp = ((non_aff_rate - aff_rate) * 100.0 * 10.0).round() / 10.0;

        ContentBlock::json(serde_json::json!({
            "days": p.days.unwrap_or(90),
            "affiliate": affiliate,
            "non_affiliate": non_affiliate,
            "parity_gap_pp": parity_gap_pp,
            "bnetza_target_pp": 2.0,
            "compliant": parity_gap_pp.abs() < 2.0,
            "note": "affiliate = initiating LF is operator's own subsidiary (§20 EnWG §6b EnWG deployment). Gap > 2 pp requires BNetzA escalation.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Rolling STP (Straight-Through Processing) rate across all process families.
    ///
    /// Returns the fraction of terminal processes that completed without an
    /// APERAK timeout or unrecoverable failure in the last `days` days.
    #[tool(
        description = "Rolling STP (Straight-Through Processing) rate across all MaKo processes for the last N days. Returns stp_rate (0–1), total, completed, rejected, timeout counts. Used by compliance-agent and processd-agent for health monitoring. Target STP >= 0.95.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_stp_rate(
        &self,
        Parameters(p): Parameters<GetStpRateParams>,
    ) -> Result<CallToolResult, McpError> {
        use sqlx::Row;
        let days = p.days.unwrap_or(30) as i32;
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) AS total,
                COUNT(*) FILTER (WHERE state = 'completed') AS completed,
                COUNT(*) FILTER (WHERE state IN ('rejected','cancelled')) AS rejected,
                COUNT(*) FILTER (WHERE state = 'aperak_timeout') AS timeout,
                COUNT(*) FILTER (WHERE state IN ('initiated','running')) AS in_flight
            FROM process_projections
            WHERE tenant = $1
              AND started_at >= NOW() - ($2 || ' days')::INTERVAL
            "#,
        )
        .bind(&self.state.tenant)
        .bind(days)
        .fetch_one(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let total: i64 = row.try_get("total").unwrap_or(0);
        let completed: i64 = row.try_get("completed").unwrap_or(0);
        let rejected: i64 = row.try_get("rejected").unwrap_or(0);
        let timeout: i64 = row.try_get("timeout").unwrap_or(0);
        let in_flight: i64 = row.try_get("in_flight").unwrap_or(0);
        let terminal = completed + rejected + timeout;
        let stp_rate = if terminal > 0 {
            completed as f64 / terminal as f64
        } else {
            0.0
        };

        ContentBlock::json(serde_json::json!({
            "days": days,
            "total": total,
            "terminal": terminal,
            "in_flight": in_flight,
            "completed": completed,
            "rejected": rejected,
            "aperak_timeout": timeout,
            "stp_rate": (stp_rate * 10000.0).round() / 10000.0,
            "stp_pct": (stp_rate * 100.0 * 10.0).round() / 10.0,
            "target_stp": 0.95,
            "compliant": stp_rate >= 0.95,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// List processes by workflow family.
    ///
    /// Useful for investigating STP drops within a specific process family
    /// (e.g. "all rejected GPKE processes this week").
    #[tool(
        description = "List processes by workflow family (gpke, wim, geli-gas, wim-gas, gabi-gas, mabis). Optional state filter. Returns process_id, pid, state, malo_id, partner_mp_id, started_at, deadline_at, erc_code ordered by started_at DESC.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_processes_by_family(
        &self,
        Parameters(p): Parameters<ListByFamilyParams>,
    ) -> Result<CallToolResult, McpError> {
        use sqlx::Row;
        let limit = p.limit.unwrap_or(50).min(500) as i64;
        let rows = sqlx::query(
            r#"
            SELECT process_id, pid, state, malo_id, partner_mp_id,
                   started_at, deadline_at, erc_code, deadline_risk, initiator_is_affiliate
            FROM process_projections
            WHERE tenant = $1
              AND family = $2
              AND ($3::text IS NULL OR state = $3)
            ORDER BY started_at DESC
            LIMIT $4
            "#,
        )
        .bind(&self.state.tenant)
        .bind(&p.family)
        .bind(p.state.as_deref())
        .bind(limit)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let processes: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| serde_json::json!({
                "process_id": r.try_get::<uuid::Uuid, _>("process_id").map(|v| v.to_string()).unwrap_or_default(),
                "pid": r.try_get::<i32, _>("pid").unwrap_or(0),
                "state": r.try_get::<String, _>("state").unwrap_or_default(),
                "malo_id": r.try_get::<Option<String>, _>("malo_id").unwrap_or(None),
                "partner_mp_id": r.try_get::<Option<String>, _>("partner_mp_id").unwrap_or(None),
                "started_at": r.try_get::<time::OffsetDateTime, _>("started_at").ok(),
                "deadline_at": r.try_get::<Option<time::OffsetDateTime>, _>("deadline_at").unwrap_or(None),
                "erc_code": r.try_get::<Option<String>, _>("erc_code").unwrap_or(None),
                "deadline_risk": r.try_get::<String, _>("deadline_risk").unwrap_or_default(),
                "initiator_is_affiliate": r.try_get::<bool, _>("initiator_is_affiliate").unwrap_or(false),
            }))
            .collect();

        ContentBlock::json(serde_json::json!({
            "family": p.family,
            "state_filter": p.state,
            "count": processes.len(),
            "processes": processes,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[prompt_router]
impl ObsdMcpHandler {
    #[prompt(
        name = "audit-kpi",
        description = "Step-by-step: run BNetzA KPI audit for a reporting period"
    )]
    async fn audit_kpi_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I generate BNetzA KPI data for regulatory reporting?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `get_kpi_report` with from/to dates for the reporting period.\n\
                 2. Key KPIs: prozesse_total, completion_rate, aperak_violations, avg_lead_time_hours.\n\
                 3. BNetzA targets: completion_rate >= 99%, aperak_violations <= 0.1%.\n\
                 4. Drill into violations: use `list_overdue_processes` for individual cases.\n\
                 5. Export the JSON response for inclusion in Qualitätsbericht (§35 EnWG).",
            ),
        ]
    }

    #[prompt(
        name = "investigate-aperak-violation",
        description = "Step-by-step: investigate an APERAK deadline violation"
    )]
    async fn investigate_aperak_violation_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "A process shows an APERAK deadline violation. How do I investigate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `get_process` with the process_id to see the full projection.\n\
                 2. Key timing: initiated_at, aperak_deadline_at (initiated + 45 min for UTILMD/ORDERS weekday),\n\
                    aperak_sent_at.\n\
                 3. If aperak_sent_at > aperak_deadline_at: BNetzA violation — document root cause.\n\
                 4. APERAK AHB 1.0: Strom UTILMD/ORDERS weekday -> 45 Minuten;\n\
                    Gas Initialprozesse -> 3 Werktage; Gas Folgeprozesse -> nächster Werktag 12 Uhr.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for ObsdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
            .with_server_info(Implementation::new("obsd", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "# obsd — Process Observability\n\
             \n\
             CQRS read model of all `de.mako.*` events. Deadline risk monitoring \
             and BNetzA KPI reports.\n\
             \n\
             ## Tools (6)\n\
             - `get_process` — full process projection by UUID (state, PIDs, deadlines, ERC)\n\
             - `list_overdue_processes` — MaKo processes past their regulatory deadline (most urgent first)\n\
             - `get_kpi_report(pid, period)` — BNetzA KPI for a PID + billing month\n\
             - `get_parity_report(days)` — §20 EnWG affiliate vs. non-affiliate completion rates; BNetzA target < 2 pp gap\n\
             - `get_stp_rate(days)` — rolling STP rate across all process families; target >= 95%\n\
             - `list_processes_by_family(family, state, limit)` — drill into gpke/wim/geli-gas/wim-gas/gabi-gas/mabis\n\
             \n\
             ## Prompts (2)\n\
             - `audit-kpi` — generate BNetzA KPI report step-by-step\n\
             - `investigate-aperak-violation` — root-cause an APERAK deadline violation\n\
             \n\
             ## Process states\n\
             `initiated` → `running` → `completed` | `rejected` | `cancelled` | `aperak_timeout`\n\
             \n\
             ## Regulatory reference\n\
             Overdue processes and parity gaps are BNetzA compliance risks. \
             Report them in the annual Marktkommunikationsbericht (§12 EnWG)."
            )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<ObsdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
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
