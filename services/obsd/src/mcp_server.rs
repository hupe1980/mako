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
//! | `list_overdue_processes`  | Processes past their business Antwortfrist |
//! | `get_kpi_report`          | Per-PID KPIs for a calendar month |
//! | `get_parity_report`       | § 7a Abs. 5 EnWG affiliate vs third-party parity |
//! | `get_stp_rate`            | Completions over processes that ended |
//! | `list_processes_by_family`| Drill into one family (gpke/wim/geli-gas/…) |
//!
//! Every tool goes through [`crate::pg::PgProcessProjectionRepository`] where one
//! exists, so this surface and the REST one cannot answer differently.

use std::sync::Arc;

// ── Timestamps on the wire ────────────────────────────────────────────────────

/// Render an instant as RFC 3339, the way every consumer expects to read one.
///
/// `time::OffsetDateTime`'s **derived** `Serialize` produces a nine-element
/// array — `[2027,15,8,0,0,0,0,0,0]` is year 2027, ordinal day 15, 08:00:00 UTC.
/// That is `time`'s internal component order, it is documented nowhere a
/// consumer would look, and dropping one into a `json!` silently ships it.
///
/// It reached the MCP surface here, which is the worst place for it: these tools
/// are read by operators and by agents reasoning about regulatory deadlines. An
/// agent asked whether a Frist has passed was being handed an undocumented
/// integer array and expected to do arithmetic on it.
///
/// A formatting failure yields `null` rather than a fabricated instant. A
/// timestamp a consumer cannot read must not look like one it can.
fn rfc3339(t: time::OffsetDateTime) -> Option<String> {
    use time::format_description::well_known::Rfc3339;
    t.format(&Rfc3339).ok()
}

/// [`rfc3339`] for an optional instant.
fn rfc3339_opt(t: Option<time::OffsetDateTime>) -> Option<String> {
    t.and_then(rfc3339)
}

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
    #[allow(
        dead_code,
        reason = "reached only in some feature or target combinations"
    )]
    tool_router: ToolRouter<ObsdMcpHandler>,
    #[allow(
        dead_code,
        reason = "reached only in some feature or target combinations"
    )]
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
    /// The full read-model entry: PID, family, state, counterparty, timestamps,
    /// the business Antwortfrist and the Festlegung it came from.
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

        let repo = crate::pg::PgProcessProjectionRepository::new(self.state.pool.clone());
        let found = repo
            .get(process_id, &self.state.tenant)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match found {
            Some(r) => ContentBlock::json(projection_json(&r))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "process_not_found: No process projection for id '{}'.",
                p.process_id
            ))])),
        }
    }

    /// Processes past their **business Antwortfrist**.
    ///
    /// Not the APERAK clock: that is minutes, arrives as its own event and lands
    /// in `state = aperak_timeout`, which this list deliberately still includes
    /// — a counterparty that missed the acknowledgement still owes the answer.
    ///
    /// Processes with **no published Frist carry no deadline** and are therefore
    /// absent: unknown, never measured against an instant nobody can cite.
    ///
    /// Every row carries `deadline_source`, the Fundstelle the instant came
    /// from, so a caller can name the Festlegung rather than assert a number.
    #[tool(
        description = "Processes past their business Antwortfrist, most urgent first. Each row \
                       carries deadline_source, the Festlegung the deadline came from. \
                       `saturated` means at least the cap was waiting, never that the cap was \
                       all there was.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_overdue_processes(&self) -> Result<CallToolResult, McpError> {
        use mako_obs::repository::ProcessProjectionRepository as _;

        let repo = crate::pg::PgProcessProjectionRepository::new(self.state.pool.clone());
        let rows = repo
            .overdue_processes(time::OffsetDateTime::now_utc(), &self.state.tenant)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let saturated =
            i64::try_from(rows.len()).unwrap_or(i64::MAX) >= crate::pg::projection::OVERDUE_LIMIT;
        let overdue: Vec<serde_json::Value> = rows
            .iter()
            .map(|p| {
                serde_json::json!({
                    "process_id": p.process_id.to_string(),
                    "pid": p.pid,
                    "family": p.family,
                    "state": p.state.as_str(),
                    "malo_id": p.malo_id,
                    "partner_mp_id": p.partner_mp_id,
                    "started_at": rfc3339(p.started_at),
                    "deadline_at": rfc3339_opt(p.deadline_at),
                    "deadline_source": p.deadline_source,
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "overdue": overdue,
            "count": overdue.len(),
            "saturated": saturated,
            "limit": crate::pg::projection::OVERDUE_LIMIT,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Process KPIs for a Prüfidentifikator and calendar month.
    ///
    /// **Two clocks, two numbers.** `aperak_timeout` counts processes whose
    /// *technical acknowledgement* window lapsed (45 min Strom, next Werktag
    /// 12:00 / 3 Werktage Gas); `frist_breached` counts processes that passed
    /// their *business* Antwortfrist. Reporting the second under the first's
    /// name points an operator at the wrong clock by three orders of magnitude.
    ///
    /// `pid` — BDEW Prüfidentifikator (e.g. 55001 for the GPKE Lieferbeginn).
    /// `period` — `YYYY-MM` (default: current month).
    #[tool(
        description = "Process KPIs for a PID and calendar month (YYYY-MM). Reports the APERAK \
                       acknowledgement clock and the business Antwortfrist as separate numbers. \
                       Rates are null when the bucket contains nothing measurable — never 0.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_kpi_report(
        &self,
        Parameters(p): Parameters<GetKpiReportParams>,
    ) -> Result<CallToolResult, McpError> {
        use mako_obs::repository::ProcessProjectionRepository as _;

        let (from, to) = month_bounds(p.period.as_deref())?;
        let repo = crate::pg::PgProcessProjectionRepository::new(self.state.pool.clone());
        // The repository owns the query, so this tool and `GET /obs/kpis`
        // cannot drift. An inline copy that bounds the period with
        // `started_at <= <last day at 00:00>` silently drops every process
        // started on the last day of the month.
        match repo.kpi_report(p.pid, from, to, &self.state.tenant).await {
            Ok(report) => ContentBlock::json(serde_json::json!({
                "pid": report.pid,
                "period_from": report.period_from.to_string(),
                "period_to": report.period_to.to_string(),
                "total_initiated": report.total_initiated,
                "total_completed": report.total_completed,
                "total_rejected": report.total_rejected,
                "total_failed": report.total_failed,
                // The technical acknowledgement clock.
                "aperak_timeout": report.total_aperak_timeout,
                // The business Antwortfrist. `total_with_frist` is the
                // denominator and is reported because a small one means the
                // bucket is mostly *unmeasured*, which a rate near 1.0 hides.
                "frist_breached": report.total_frist_breached,
                "with_published_frist": report.total_with_frist,
                "frist_compliance_rate": report.frist_compliance_rate,
                "avg_cycle_time_hours": report.avg_cycle_time_hours,
                "p95_cycle_time_hours": report.p95_cycle_time_hours,
                "note": "aperak_timeout is the technical acknowledgement clock; frist_breached \
                         is the business Antwortfrist. They are different obligations. Null \
                         rates mean nothing measurable in the bucket, not perfect performance.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(mako_obs::error::ObsError::NoKpiData { .. }) => {
                Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "no_data: no process with PID {} started between {from} and {to}.",
                    p.pid
                ))]))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// § 7a Abs. 5 EnWG Gleichbehandlung parity report.
    ///
    /// Compares how the operator's network arm treated Lieferanten inside its
    /// own vertically integrated undertaking against third-party Lieferanten,
    /// over the processes the network arm actually answers.
    ///
    /// **`gap_pp` is `affiliate − third_party`.** Positive means the affiliate
    /// fared better, which is the concern. The convention is
    /// `ParityComparison`'s, shared with the REST report and the CloudEvent.
    ///
    /// **No BNetzA threshold exists** for this figure — what counts as a gap
    /// worth explaining is the operator's judgement, configured in
    /// `[worker] parity_threshold_pp`.
    #[tool(
        description = "§ 7a Abs. 5 EnWG parity: affiliate vs third-party Lieferanten completion \
                       rates over the processes the network arm answers. gap_pp = affiliate − \
                       third_party in percentage points; positive means the affiliate fared \
                       better. Null when either group is below the minimum sample — an \
                       unstatable gap, not a zero one. No regulatory threshold exists.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_parity_report(
        &self,
        Parameters(p): Parameters<GetParityReportParams>,
    ) -> Result<CallToolResult, McpError> {
        use mako_obs::domain::{PARITY_MIN_SAMPLE, ParityComparison, ParityGroup};
        use sqlx::Row;

        let days = i32::try_from(p.days.unwrap_or(90)).unwrap_or(90);
        let rows = sqlx::query(&format!(
            r"SELECT initiator_is_affiliate,
                     COUNT(*) AS total,
                     COUNT(*) FILTER (WHERE state = 'completed') AS completed,
                     COUNT(*) FILTER (WHERE state = 'rejected')  AS rejected,
                     COUNT(*) FILTER (
                         WHERE deadline_at IS NOT NULL
                           AND deadline_at < COALESCE(completed_at, now())
                     ) AS frist_breached
              FROM process_projections
              WHERE tenant = $1
                AND pid IN ({pids})
                AND started_at >= now() - make_interval(days => $2::int)
              GROUP BY initiator_is_affiliate",
            pids = crate::worker::parity_pids_sql(),
        ))
        .bind(&self.state.tenant)
        .bind(days)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let (mut affiliate, mut third_party) = (ParityGroup::default(), ParityGroup::default());
        for row in &rows {
            let g = ParityGroup {
                total: row.try_get("total").unwrap_or(0),
                completed: row.try_get("completed").unwrap_or(0),
                rejected: row.try_get("rejected").unwrap_or(0),
                frist_breached: row.try_get("frist_breached").unwrap_or(0),
            };
            if row
                .try_get::<bool, _>("initiator_is_affiliate")
                .unwrap_or(false)
            {
                affiliate = g;
            } else {
                third_party = g;
            }
        }
        let c = ParityComparison::new(affiliate, third_party);

        ContentBlock::json(serde_json::json!({
            "days": days,
            "affiliate": c.affiliate,
            "third_party": c.third_party,
            "gap_pp": c.gap_pp,
            "favours": c.favours(),
            "min_sample": PARITY_MIN_SAMPLE,
            "gap_convention": "affiliate − third_party, percentage points. Positive means the \
                               affiliate fared better.",
            "note": "gap_pp is null when either group has fewer than min_sample processes — the \
                     gap is unstatable, not zero. No BNetzA publication sets a numeric parity \
                     limit; the operator's escalation threshold is [worker] parity_threshold_pp. \
                     Basis: § 7a Abs. 5 EnWG Gleichbehandlungsbericht, filed by 31 March for the \
                     preceding calendar year.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Rolling STP (Straight-Through Processing) rate across all families.
    ///
    /// The denominator is processes that **ended** — completed, rejected or
    /// failed. An `aperak_timeout` is not an ending: the counterparty missed the
    /// acknowledgement window and can still answer, so counting it as a terminal
    /// failure understates the rate for as long as the process stays open.
    #[tool(
        description = "Rolling STP rate over processes that ended in the last N days. \
                       Denominator is completed + rejected + failed; in-flight and \
                       aperak_timeout processes are reported separately because neither has \
                       ended. Null when nothing ended in the window. No regulatory target \
                       exists — compare against your own operating goal.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_stp_rate(
        &self,
        Parameters(p): Parameters<GetStpRateParams>,
    ) -> Result<CallToolResult, McpError> {
        use sqlx::Row;
        let days = i32::try_from(p.days.unwrap_or(30)).unwrap_or(30);
        let row = sqlx::query(
            r"SELECT
                COUNT(*) AS total,
                COUNT(*) FILTER (WHERE state = 'completed')      AS completed,
                COUNT(*) FILTER (WHERE state = 'rejected')       AS rejected,
                COUNT(*) FILTER (WHERE state = 'failed')         AS failed,
                COUNT(*) FILTER (WHERE state = 'aperak_timeout') AS aperak_timeout,
                COUNT(*) FILTER (WHERE state IN ('initiated','running')) AS in_flight,
                COUNT(*) FILTER (
                    WHERE deadline_at IS NOT NULL
                      AND deadline_at < COALESCE(completed_at, now())
                ) AS frist_breached
              FROM process_projections
              WHERE tenant = $1
                AND started_at >= now() - make_interval(days => $2::int)",
        )
        .bind(&self.state.tenant)
        .bind(days)
        .fetch_one(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let n = |name: &str| -> i64 { row.try_get(name).unwrap_or(0) };
        let (completed, rejected, failed) = (n("completed"), n("rejected"), n("failed"));
        let ended = completed + rejected + failed;
        #[expect(
            clippy::cast_precision_loss,
            reason = "a count or ratio reported as a float, never a money amount"
        )]
        // Null, not 0.0: a window in which nothing ended has no rate, and a zero
        // reads as "everything failed".
        let stp_rate =
            (ended > 0).then(|| ((completed as f64 / ended as f64) * 10_000.0).round() / 10_000.0);

        ContentBlock::json(serde_json::json!({
            "days": days,
            "total_started": n("total"),
            "ended": ended,
            "completed": completed,
            "rejected": rejected,
            "failed": failed,
            "in_flight": n("in_flight"),
            "aperak_timeout": n("aperak_timeout"),
            "frist_breached": n("frist_breached"),
            "stp_rate": stp_rate,
            "note": "stp_rate = completed / (completed + rejected + failed). aperak_timeout is \
                     the technical acknowledgement clock and is not an ending — a counterparty \
                     that missed it can still answer. No regulatory STP target exists.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// List processes by workflow family.
    ///
    /// Useful for investigating a drop inside one family (e.g. "all rejected
    /// GPKE processes this week").
    #[tool(
        description = "List processes by family (gpke, wim, geli-gas, wim-gas, gabi-gas, mabis, \
                       invoic-storno, unknown), newest first. Optional state filter: initiated, \
                       running, aperak_timeout, completed, rejected, failed.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_processes_by_family(
        &self,
        Parameters(p): Parameters<ListByFamilyParams>,
    ) -> Result<CallToolResult, McpError> {
        use mako_obs::domain::{ObsQuery, ProcessState};
        use mako_obs::repository::ProcessProjectionRepository as _;

        // An unknown state is refused rather than ignored: dropping the filter
        // returns every row, which reads as "the filter matched everything".
        let state = match p.state.as_deref() {
            None => None,
            Some(s) => Some(ProcessState::from_str_exact(s).ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "unknown state `{s}` — one of {:?}",
                        ProcessState::ALL.map(ProcessState::as_str)
                    ),
                    None,
                )
            })?),
        };

        let limit = p.limit.unwrap_or(50).min(500);
        let repo = crate::pg::PgProcessProjectionRepository::new(self.state.pool.clone());
        let rows = repo
            .query(&ObsQuery {
                state,
                family: Some(p.family.clone()),
                tenant: Some(self.state.tenant.clone()),
                limit,
                ..ObsQuery::default()
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let saturated = u32::try_from(rows.len()).unwrap_or(u32::MAX) >= limit;
        let processes: Vec<serde_json::Value> = rows.iter().map(projection_json).collect();

        ContentBlock::json(serde_json::json!({
            "family": p.family,
            "state_filter": p.state,
            "count": processes.len(),
            "saturated": saturated,
            "limit": limit,
            "processes": processes,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

/// One projection, as every tool on this surface renders it.
///
/// One function so a field added to the read-model reaches all of them, and so
/// two tools cannot disagree about what `state` is called.
fn projection_json(p: &mako_obs::domain::ProcessProjection) -> serde_json::Value {
    serde_json::json!({
        "process_id": p.process_id.to_string(),
        "pid": p.pid,
        "family": p.family,
        "workflow_name": p.workflow_name,
        "state": p.state.as_str(),
        "malo_id": p.malo_id,
        "partner_mp_id": p.partner_mp_id,
        "mdm_role": p.mdm_role,
        "deadline_at": rfc3339_opt(p.deadline_at),
        "deadline_source": p.deadline_source,
        "deadline_risk": p.deadline_risk.as_str(),
        "started_at": rfc3339(p.started_at),
        "last_event_at": rfc3339(p.last_event_at),
        "erc_code": p.erc_code,
        "initiator_is_affiliate": p.initiator_is_affiliate,
    })
}

/// First and last day of a `YYYY-MM` period, or of the current month.
fn month_bounds(period: Option<&str>) -> Result<(time::Date, time::Date), McpError> {
    use time::{Date, Month};

    let today = mako_fristen::heute();
    let (year, month_u8) = match period {
        Some(p) => {
            let (y, m) = p
                .split_once('-')
                .ok_or_else(|| McpError::invalid_params("period must be YYYY-MM", None))?;
            (
                y.parse::<i32>()
                    .map_err(|_| McpError::invalid_params("invalid year in period", None))?,
                m.parse::<u8>()
                    .map_err(|_| McpError::invalid_params("invalid month in period", None))?,
            )
        }
        None => (today.year(), today.month() as u8),
    };

    let month = Month::try_from(month_u8)
        .map_err(|_| McpError::invalid_params("month out of range 1–12", None))?;
    let from = Date::from_calendar_date(year, month, 1)
        .map_err(|_| McpError::invalid_params("invalid date", None))?;
    // The last day of the month, as a *date*: bounding by a timestamp at
    // midnight would drop every process started on it.
    let next_month = if month_u8 == 12 {
        Date::from_calendar_date(year + 1, Month::January, 1)
    } else {
        Date::from_calendar_date(
            year,
            Month::try_from(month_u8 + 1).unwrap_or(Month::December),
            1,
        )
    }
    .map_err(|_| McpError::invalid_params("invalid date", None))?;
    let to = next_month.previous_day().unwrap_or(from);
    Ok((from, to))
}

#[prompt_router]
impl ObsdMcpHandler {
    #[prompt(
        name = "audit-kpi",
        description = "Step-by-step: read a PID's KPIs for a reporting period"
    )]
    async fn audit_kpi_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I generate BNetzA KPI data for regulatory reporting?",
            ),
            PromptMessage::new_text(Role::Assistant, audit_kpi_prompt_text()),
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
            PromptMessage::new_text(Role::Assistant, aperak_prompt_text()),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for ObsdMcpHandler {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("obsd", env!("CARGO_PKG_VERSION")))
        .with_instructions(instructions())
    }
}

/// The APERAK windows as `mako_fristen` computes them, read back off the
/// `aperak_*` functions for a fixed arrival so the sentence cannot state a
/// window the engine does not apply.
struct AperakWindows {
    /// Minutes for a Strom UTILMD or ORDERS.
    strom_minutes: i64,
    /// Berlin hour on the Sunday after a Saturday arrival.
    strom_saturday_hour: u8,
    /// Berlin hour on the next Werktag for every other Strom message.
    strom_allgemein_hour: u8,
    /// Berlin hour on the next Werktag for a Gas Folgeprozess.
    gas_folge_hour: u8,
    /// Werktage for a Gas Initialprozess, to the end of the last.
    gas_initial_werktage: u32,
}

impl AperakWindows {
    fn from_mako_fristen() -> Self {
        use mako_fristen::HolidayCalendar;
        // Monday 2025-01-06 09:00 Berlin, and Saturday 2025-01-04 11:00 Berlin:
        // no holiday near either, so the arithmetic is the rule alone.
        let monday = time::macros::datetime!(2025-01-06 08:00 UTC);
        let saturday = time::macros::datetime!(2025-01-04 10:00 UTC);
        let berlin_hour = |due: time::OffsetDateTime| -> u8 {
            let day = mako_fristen::berlin_date(due);
            (0..24u8)
                .find(|h| {
                    mako_fristen::berlin_at(
                        day,
                        time::Time::from_hms(*h, 0, 0).unwrap_or(time::Time::MIDNIGHT),
                    ) == due
                })
                .unwrap_or(24)
        };
        let initial = mako_fristen::aperak_gas_initialprozess_due_at(monday);
        let start = mako_fristen::berlin_date(monday);
        let gas_initial_werktage = (1..=10u32)
            .find(|n| {
                mako_fristen::end_of_day_berlin(mako_fristen::add_werktage(
                    start,
                    *n,
                    HolidayCalendar::BdewMaKo,
                )) == initial
            })
            .unwrap_or(0);
        Self {
            strom_minutes: (mako_fristen::aperak_strom_due_at(monday) - monday).whole_minutes(),
            strom_saturday_hour: berlin_hour(mako_fristen::aperak_strom_due_at(saturday)),
            strom_allgemein_hour: berlin_hour(mako_fristen::aperak_strom_allgemein_due_at(monday)),
            gas_folge_hour: berlin_hour(mako_fristen::aperak_gas_folgeprozess_due_at(monday)),
            gas_initial_werktage,
        }
    }
}

/// The APERAK Frist in one sentence, rendered from [`AperakWindows`].
fn aperak_frist_sentence() -> String {
    let w = AperakWindows::from_mako_fristen();
    format!(
        "Strom UTILMD/ORDERS {} min after arrival (a Saturday arrival: Sunday {}:00), \
         every other Strom message the next Werktag {}:00; Gas the next Werktag {}:00 \
         for a Folgeprozess, the end of the {}. Werktag for an Initialprozess \
         (APERAK AHB 1.1 § 2.3.1 / § 2.4.1)",
        w.strom_minutes,
        w.strom_saturday_hour,
        w.strom_allgemein_hour,
        w.gas_folge_hour,
        w.gas_initial_werktage
    )
}

/// Representative Antwortfristen — GPKE Anmeldung, Gas Anmeldung, the four
/// WiM Strom PIDs — rendered from `mako_fristen::antwort`.
fn antwort_examples() -> String {
    [55_001, 44_001, 55_039, 55_042, 55_051, 55_168]
        .into_iter()
        .filter_map(mako_fristen::antwort::antwort_obligation)
        .map(|o| format!("{} {} → {}", o.trigger_pid, o.name, o.frist.beschreibung()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The `audit-kpi` prompt's answer.
fn audit_kpi_prompt_text() -> String {
    String::from(
        "1. Use `get_kpi_report` with a PID and a `YYYY-MM` period.\n\
         2. Read `total_initiated`, `total_completed`, `total_rejected`, `total_failed`, \
         `aperak_timeout`, `frist_breached`, `frist_compliance_rate` and \
         `avg_cycle_time_hours`. `aperak_timeout` is the technical acknowledgement clock, \
         `frist_breached` the business Antwortfrist — never report one under the other's \
         name.\n\
         3. No BNetzA threshold exists for these rates. Any target is the operator's own; \
         say whose it is.\n\
         4. Drill into breaches with `list_overdue_processes` for individual cases.",
    )
}

/// The `investigate-aperak-violation` prompt's answer.
fn aperak_prompt_text() -> String {
    format!(
        "1. Use `get_process` with the process_id to see the full projection.\n\
         2. Compare `aperak_deadline_at` with when the APERAK was sent.\n\
         3. The APERAK Frist: {}.\n\
         4. A lapse is the acknowledgement clock, not the business Antwortfrist — \
         document the root cause under that name.",
        aperak_frist_sentence()
    )
}

/// The server instructions an MCP client reads on `initialize`.
fn instructions() -> String {
    format!(
        "# obsd — Process Observability\n\
             \n\
             CQRS read model over every `de.mako.*` event. Antwortfrist monitoring and \
             process KPIs.\n\
             \n\
             ## Two deadline clocks, never one number\n\
             - **APERAK Frist** — the technical acknowledgement: {aperak}. A lapse \
             arrives as its own event and shows as `state = aperak_timeout`.\n\
             - **Antwortfrist** — the business answer, per inbound PID: {antwort}. Stored as \
             `deadline_at`, with `deadline_source` naming the Festlegung.\n\
             \n\
             They differ by orders of magnitude and fail for different reasons. Never report \
             one under the other's name.\n\
             \n\
             ## Tools (6)\n\
             - `get_process` — full projection by UUID\n\
             - `list_overdue_processes` — past their Antwortfrist, most urgent first\n\
             - `get_kpi_report(pid, period)` — per-PID KPIs for a `YYYY-MM` month\n\
             - `get_parity_report(days)` — § 7a Abs. 5 EnWG affiliate vs third-party; \
             `gap_pp` is affiliate − third_party, positive means the affiliate fared better\n\
             - `get_stp_rate(days)` — completions over processes that ended\n\
             - `list_processes_by_family(family, state, limit)`\n\
             \n\
             ## Prompts (2)\n\
             - `audit-kpi` — read a period's KPIs without confusing the two clocks\n\
             - `investigate-aperak-violation` — root-cause a missed APERAK Frist\n\
             \n\
             ## Reading the answers honestly\n\
             - A **null** rate means nothing measurable in the bucket. It is not 0 and not \
             perfect performance.\n\
             - A missing `deadline_at` means no Festlegung publishes a window for that PID: \
             unknown, not compliant.\n\
             - `saturated: true` means at least the cap was waiting — never that the cap was \
             all there was.\n\
             - **No BNetzA threshold exists** for STP rate, parity gap or compliance rate. \
             Any target is the operator's own; say whose it is.\n\
             \n\
             ## Process states\n\
             `initiated` → `running` → `completed` | `rejected` | `failed`; `aperak_timeout` \
             is not terminal — a counterparty that missed the acknowledgement can still answer.",
        aperak = aperak_frist_sentence(),
        antwort = antwort_examples(),
    )
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

#[cfg(test)]
mod instruction_tests {
    use super::*;

    /// The prompts the instructions list are the ones registered.
    #[test]
    fn the_listed_prompts_are_the_registered_ones() {
        let registered: std::collections::BTreeSet<String> = ObsdMcpHandler::prompt_router()
            .list_all()
            .into_iter()
            .map(|p| p.name.to_string())
            .collect();
        let text = instructions();
        let section = text
            .split("## Prompts")
            .nth(1)
            .and_then(|s| s.split("\n## ").next())
            .expect("a Prompts section");
        let listed: std::collections::BTreeSet<String> = section
            .lines()
            .filter_map(|l| l.trim().strip_prefix("- `"))
            .filter_map(|l| l.split('`').next())
            .map(str::to_owned)
            .collect();
        assert_eq!(listed, registered);
    }

    /// Every APERAK sentence the surface states is rendered from `mako_fristen`.
    #[test]
    fn the_aperak_sentences_follow_mako_fristen() {
        let sentence = aperak_frist_sentence();
        let w = AperakWindows::from_mako_fristen();
        assert_eq!(
            w.strom_minutes,
            mako_fristen::APERAK_STROM_UTILMD_ORDERS_MINUTES
        );
        assert_eq!(w.strom_saturday_hour, 12);
        assert_eq!(w.strom_allgemein_hour, 12);
        assert_eq!(w.gas_folge_hour, 12);
        assert_eq!(w.gas_initial_werktage, 3);
        assert!(sentence.contains(&format!("{} min", w.strom_minutes)));
        let prompt = aperak_prompt_text();
        for text in [instructions(), prompt] {
            assert!(
                text.contains(&sentence),
                "the APERAK sentence is the rendered one"
            );
            // The superseded edition, spelled so it is not itself a citation.
            let superseded = concat!("APERAK AHB 1", ".0");
            for wrong in ["weekday", "Mo–Fr", superseded, "3 Werktage, 12"] {
                assert!(!text.contains(wrong), "`{wrong}` in: {text}");
            }
        }
    }

    /// The audit prompt states no unsourced target and no mislabelled §.
    #[test]
    fn the_audit_prompt_carries_no_unsourced_target() {
        let text = audit_kpi_prompt_text();
        assert!(!text.contains("BNetzA targets"), "{text}");
        assert!(!text.contains("Qualitätsbericht"), "{text}");
    }
}
