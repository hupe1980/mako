//! The read-model routes: process projections, KPIs, the overdue list, the
//! § 7a Abs. 5 EnWG Gleichbehandlung report and the business gauges.

use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use mako_service::{ApiError, ApiResult, Json};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::obs::pg::TERMINAL_STATE_SQL;
use crate::obs::project::ObsState;
use crate::obs::{domain::ObsQuery, repository::ProcessProjectionRepository};

// ── Router ────────────────────────────────────────────────────────────────────

/// The read-model routes. `POST /webhook` is processd's one inbound door and
/// is mounted by `crate::server`, which hands each event to
/// [`crate::obs::project::project`].
pub fn router(state: ObsState) -> Router {
    Router::new()
        .route("/obs/processes", get(get_processes))
        .route("/obs/processes/{process_id}", get(get_process))
        .route("/obs/kpis", get(get_kpis))
        .route("/obs/overdue", get(get_overdue))
        .route(
            "/api/v1/audit/gleichbehandlung",
            get(get_gleichbehandlung_report),
        )
        .route("/obs/metrics", get(metrics))
        .with_state(state)
}

// ── REST handlers ─────────────────────────────────────────────────────────────

/// The Cedar decision for `action`, or the `403` naming why it was denied.
fn authorize(
    enforcer: &CedarEnforcer,
    claims: &Claims,
    action: &'static str,
    tenant: &str,
) -> ApiResult<()> {
    enforcer
        .check(&claims.principal(), action, tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))
}

/// A read-model query failure: logged with what failed, answered as a 500 that
/// carries an `error_id` and none of the database's text.
fn query_failed(what: &'static str, err: impl Into<anyhow::Error>) -> ApiError {
    ApiError::internal(err.into().context(format!("processd obs: {what} failed")))
}

/// `GET /obs/metrics` — Prometheus-compatible operational metrics.
///
/// Read-model business gauges (process counts + pool stats); the runner's
/// generic `/metrics` (request counters) is mounted separately by `run`.
/// No authentication required; restrict network access at the ingress layer.
async fn metrics(State(state): State<ObsState>) -> impl IntoResponse {
    let mut out = String::with_capacity(512);
    let pool = state.repo.pool();

    let total_processes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM process_projections")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let open_processes: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM process_projections WHERE state NOT IN ({TERMINAL_STATE_SQL})"
    ))
    .fetch_one(pool)
    .await
    .inspect_err(|e| tracing::warn!(%e, "processd obs: open-process gauge query failed"))
    .unwrap_or(0);
    let overdue_processes: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM process_projections \
         WHERE state NOT IN ({TERMINAL_STATE_SQL}) AND deadline_at < now()"
    ))
    .fetch_one(pool)
    .await
    .inspect_err(|e| tracing::warn!(%e, "processd obs: overdue-process gauge query failed"))
    .unwrap_or(0);
    let pool_size = pool.size();
    let pool_idle = pool.num_idle();

    out.push_str(
        "# HELP processd_obs_process_projections_total Total ProcessProjection records.\n",
    );
    out.push_str("# TYPE processd_obs_process_projections_total gauge\n");
    out.push_str(&format!(
        "processd_obs_process_projections_total {total_processes}\n"
    ));
    out.push_str("# HELP processd_obs_open_processes_total Open (in-progress) MaKo processes.\n");
    out.push_str("# TYPE processd_obs_open_processes_total gauge\n");
    out.push_str(&format!(
        "processd_obs_open_processes_total {open_processes}\n"
    ));
    out.push_str(
        "# HELP processd_obs_overdue_processes_total Processes past their regulatory deadline.\n",
    );
    out.push_str("# TYPE processd_obs_overdue_processes_total gauge\n");
    out.push_str(&format!(
        "processd_obs_overdue_processes_total {overdue_processes}\n"
    ));
    out.push_str("# HELP processd_obs_db_pool_size Current PostgreSQL connection pool size.\n");
    out.push_str("# TYPE processd_obs_db_pool_size gauge\n");
    out.push_str(&format!("processd_obs_db_pool_size {pool_size}\n"));
    out.push_str("# HELP processd_obs_db_pool_idle Idle PostgreSQL connections.\n");
    out.push_str("# TYPE processd_obs_db_pool_idle gauge\n");
    out.push_str(&format!("processd_obs_db_pool_idle {pool_idle}\n"));

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        out,
    )
}

#[derive(Debug, Deserialize)]
struct ProcessQueryParams {
    state: Option<String>,
    pid: Option<u32>,
    family: Option<String>,
    partner_mp_id: Option<String>,
    mdm_role: Option<String>,
    since: Option<String>,
    limit: Option<u32>,
}

async fn get_processes(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ObsState>,
    Query(params): Query<ProcessQueryParams>,
) -> ApiResult<Json<serde_json::Value>> {
    use crate::obs::domain::ProcessState;
    use time::format_description::well_known::Rfc3339;

    authorize(&enforcer, &claims, "read-process", &state.tenant)?;

    // An unparseable `state` is a client error, not "no filter": silently
    // dropping it returns every row and reads as a much larger result set than
    // the caller asked for.
    let obs_state = match params.state.as_deref() {
        None => None,
        Some(s) => match ProcessState::from_str_exact(s) {
            Some(st) => Some(st),
            None => {
                return Err(ApiError::bad_request(format!(
                    "unknown state `{s}` — known: {}",
                    ProcessState::ALL.map(ProcessState::as_str).join(", ")
                )));
            }
        },
    };

    // The same holds for `since`: a bound that does not parse is refused, not
    // read as "from the beginning".
    let since = params
        .since
        .as_deref()
        .map(|s| {
            OffsetDateTime::parse(s, &Rfc3339).map_err(|_| {
                ApiError::bad_request(format!("`since` is not an RFC 3339 instant: {s:?}"))
            })
        })
        .transpose()?;

    let q = ObsQuery {
        state: obs_state,
        pid: params.pid,
        family: params.family,
        partner_mp_id: params.partner_mp_id,
        mdm_role: params.mdm_role,
        since,
        tenant: Some(state.tenant.clone()),
        limit: params.limit.unwrap_or(100).min(1000),
    };

    let processes = state
        .repo
        .query(&q)
        .await
        .map_err(|e| query_failed("get_processes", e))?;
    Ok(Json(
        serde_json::to_value(processes).map_err(ApiError::internal)?,
    ))
}

async fn get_process(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ObsState>,
    Path(process_id_str): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-process", &state.tenant)?;
    let process_id: Uuid = process_id_str
        .parse()
        .map_err(|_| ApiError::bad_request("process_id is not a UUID"))?;

    let process = state
        .repo
        .get(process_id, &state.tenant)
        .await
        .map_err(|e| query_failed("get_process", e))?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(
        serde_json::to_value(process).map_err(ApiError::internal)?,
    ))
}

#[derive(Debug, Deserialize)]
struct KpiQueryParams {
    pid: u32,
    period: Option<String>, // "YYYY-MM" format
}

async fn get_kpis(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ObsState>,
    Query(params): Query<KpiQueryParams>,
) -> ApiResult<axum::response::Response> {
    use time::{Date, Month};

    authorize(&enforcer, &claims, "read-kpi", &state.tenant)?;

    let (from, to) = if let Some(period) = params.period.as_deref() {
        let parts: Vec<&str> = period.split('-').collect();
        if parts.len() != 2 {
            return Err(ApiError::bad_request("period must be YYYY-MM"));
        }
        let year: i32 = parts[0]
            .parse()
            .map_err(|_| ApiError::bad_request("period: invalid year"))?;
        let month: u8 = parts[1]
            .parse()
            .map_err(|_| ApiError::bad_request("period: invalid month"))?;
        let month_enum = Month::try_from(month)
            .map_err(|_| ApiError::bad_request("period: month out of range"))?;
        let from = Date::from_calendar_date(year, month_enum, 1)
            .map_err(|_| ApiError::bad_request("period: invalid date"))?;
        let to = {
            let next_year = if month == 12 { year + 1 } else { year };
            let next_month_u8 = if month == 12 { 1 } else { month + 1 };
            let next_month = Month::try_from(next_month_u8).unwrap();
            Date::from_calendar_date(next_year, next_month, 1)
                .map(|d| d.previous_day().unwrap_or(d))
                .unwrap_or(from)
        };
        (from, to)
    } else {
        let today = mako_fristen::heute();
        let from = Date::from_calendar_date(today.year(), today.month(), 1).unwrap();
        (from, today)
    };

    match state
        .repo
        .kpi_report(params.pid, from, to, &state.tenant)
        .await
    {
        // The report's rates are `Option`, so an unclosed or unmeasurable
        // bucket serialises as `null` without this layer patching anything —
        // which is what stops the MCP surface answering 0.0 to the same query.
        Ok(report) => Ok(Json(report).into_response()),
        // No process of this PID in the period: nothing to report on.
        Err(crate::obs::error::ObsError::NoKpiData { .. }) => Err(ApiError::NotFound),
        Err(err) => Err(query_failed("get_kpis", err)),
    }
}

async fn get_overdue(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ObsState>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-overdue", &state.tenant)?;
    let now = OffsetDateTime::now_utc();
    let processes = state
        .repo
        .overdue_processes(now, &state.tenant)
        .await
        .map_err(|e| query_failed("get_overdue", e))?;
    Ok(Json(
        serde_json::to_value(processes).map_err(ApiError::internal)?,
    ))
}

// ── § 7a Abs. 5 EnWG Gleichbehandlungsbericht evidence ───────────────────────

/// The header row of the `format=csv` Gleichbehandlung report — one column per
/// field each data row writes, in that order.
const GLEICHBEHANDLUNG_CSV_HEADER: &str = concat!(
    "pid,affiliate_total,affiliate_completed,affiliate_rejected,affiliate_frist_breached,",
    "affiliate_completion_rate,affiliate_frist_compliance_rate,third_party_total,",
    "third_party_completed,third_party_rejected,third_party_frist_breached,",
    "third_party_completion_rate,third_party_frist_compliance_rate,gap_pp,frist_gap_pp\n",
);

/// Query parameters for `GET /api/v1/audit/gleichbehandlung`.
#[derive(Debug, Deserialize)]
struct GleichbehandlungQuery {
    /// Calendar year to report (default: current year).
    year: Option<i32>,
    /// Output format: `json` (default) or `csv`.
    format: Option<String>,
}

/// `GET /api/v1/audit/gleichbehandlung?year=YYYY[&format=csv|json]`
///
/// Per-PID evidence for the **Gleichbehandlungsbericht** a vertically
/// integrated undertaking's Gleichbehandlungsbeauftragte files with the
/// Bundesnetzagentur by **31 March** for the preceding calendar year
/// (§ 7a Abs. 5 EnWG), and publishes in non-personalised form.
/// Lieferantenwechsel is one of the areas those reports examine.
///
/// It compares how the operator's network arm treated Lieferanten inside its
/// own undertaking against third-party Lieferanten, over the processes the
/// network arm actually answers (`handler::is_nb_answered_lieferanten_process`).
/// The underlying duties are § 6a EnWG (informatorische Entflechtung) and
/// § 20 Abs. 1 Satz 1 EnWG (diskriminierungsfreier Netzzugang).
///
/// **`gap_pp` is `affiliate − third_party`, in percentage points.** A positive
/// gap means the affiliate fared better, which is the concern. This convention
/// is `ParityComparison`'s and is shared with the `de.obs.stp.parity.alert`
/// CloudEvent and the MCP tool: three surfaces read one sign, because a sign
/// computed per surface is a sign that can disagree about which side was
/// favoured.
///
/// **No threshold is asserted.** The Bundesnetzagentur publishes no numeric
/// parity limit for this figure; what counts as a gap worth explaining is the
/// operator's judgement, and the escalation threshold lives in
/// `[obs] parity_threshold_pp`. A gap over a group smaller than
/// `PARITY_MIN_SAMPLE` is reported as `null`, not as a large number.
async fn get_gleichbehandlung_report(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ObsState>,
    Query(params): Query<GleichbehandlungQuery>,
) -> ApiResult<axum::response::Response> {
    use crate::obs::domain::{ParityComparison, ParityGroup};

    authorize(&enforcer, &claims, "read-kpi", &state.tenant)?;

    let year = params.year.unwrap_or_else(|| mako_fristen::heute().year());
    let format = params.format.as_deref().unwrap_or("json");

    let rows = state
        .repo
        .gleichbehandlung_counts(year, &state.tenant)
        .await
        .map_err(|e| query_failed("the Gleichbehandlung report query", e))?;

    // Collate into per-PID affiliate / third-party pairs.
    use std::collections::BTreeMap;
    let mut by_pid: BTreeMap<i32, (ParityGroup, ParityGroup)> = BTreeMap::new();
    for (pid, is_affiliate, total, completed, rejected, frist_breached) in rows {
        let entry = by_pid.entry(pid).or_default();
        let group = if is_affiliate {
            &mut entry.0
        } else {
            &mut entry.1
        };
        group.total += total;
        group.completed += completed;
        group.rejected += rejected;
        group.frist_breached += frist_breached;
    }

    let comparisons: Vec<(i32, ParityComparison)> = by_pid
        .into_iter()
        .map(|(pid, (affiliate, third_party))| (pid, ParityComparison::new(affiliate, third_party)))
        .collect();

    if format == "csv" {
        let mut csv = String::from(GLEICHBEHANDLUNG_CSV_HEADER);
        for (pid, c) in &comparisons {
            // An unstatable rate is an empty CSV cell, never a 0 that reads as a
            // measured "completed none of them".
            let rate = |g: &ParityGroup| {
                g.completion_rate()
                    .map_or_else(String::new, |r| format!("{r:.4}"))
            };
            let frist_rate = |g: &ParityGroup| {
                g.frist_compliance_rate()
                    .map_or_else(String::new, |r| format!("{r:.4}"))
            };
            let gap = c.gap_pp.map_or_else(String::new, |g| format!("{g:.1}"));
            let frist_gap = c
                .frist_gap_pp
                .map_or_else(String::new, |g| format!("{g:.1}"));
            csv.push_str(&format!(
                "{pid},{},{},{},{},{},{},{},{},{},{},{},{},{gap},{frist_gap}
",
                c.affiliate.total,
                c.affiliate.completed,
                c.affiliate.rejected,
                c.affiliate.frist_breached,
                rate(&c.affiliate),
                frist_rate(&c.affiliate),
                c.third_party.total,
                c.third_party.completed,
                c.third_party.rejected,
                c.third_party.frist_breached,
                rate(&c.third_party),
                frist_rate(&c.third_party),
            ));
        }
        return Ok((
            StatusCode::OK,
            [("content-type", "text/csv; charset=utf-8")],
            csv,
        )
            .into_response());
    }

    let by_pid_json: Vec<serde_json::Value> = comparisons
        .iter()
        .map(|(pid, c)| {
            serde_json::json!({
                "pid": pid,
                "process": mako_fristen::antwort::antwort_obligation(pid.unsigned_abs())
                    .map(|o| o.name),
                "affiliate": c.affiliate,
                "third_party": c.third_party,
                "gap_pp": c.gap_pp,
                "frist_gap_pp": c.frist_gap_pp,
                "favours": c.favours(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "year": year,
        "tenant": state.tenant,
        "generated_at": OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        "basis": "§ 7a Abs. 5 EnWG — Gleichbehandlungsbericht, filed by 31 March for the \
                  preceding calendar year; duties under § 6a EnWG (informatorische \
                  Entflechtung) and § 20 Abs. 1 Satz 1 EnWG (diskriminierungsfreier Netzzugang)",
        "gap_convention": "gap_pp = (affiliate completion rate − third-party completion rate) \
                           × 100, and frist_gap_pp the same over the share answered inside the \
                           published Antwortfrist. Positive means the affiliate fared better. \
                           `null` when either group is below the minimum sample size — an \
                           unstatable gap, not a zero one. The two are reported separately \
                           because a rejection can be legitimate while a missed statutory \
                           window is the disparity this filing is asked about.",
        "min_sample": crate::obs::domain::PARITY_MIN_SAMPLE,
        "threshold": "none published. The Bundesnetzagentur sets no numeric parity limit for \
                      this figure; the operator's escalation threshold is [obs] \
                      parity_threshold_pp and is an internal policy, not a regulatory one.",
        "by_pid": by_pid_json,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::{GLEICHBEHANDLUNG_CSV_HEADER, TERMINAL_STATE_SQL};
    use crate::obs::domain::ProcessState;

    /// Every CSV column name is bare, and the header has as many columns as a
    /// data row: a name with leading whitespace is a different column to every
    /// CSV reader the filing is loaded into.
    #[test]
    fn the_csv_header_names_fifteen_bare_columns() {
        let header = GLEICHBEHANDLUNG_CSV_HEADER
            .strip_suffix('\n')
            .expect("one line");
        let columns: Vec<&str> = header.split(',').collect();
        assert_eq!(columns.len(), 15, "{columns:?}");
        for c in &columns {
            assert!(
                !c.is_empty() && c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'),
                "column {c:?} is not a bare name"
            );
        }
    }

    /// The metrics gauges count non-terminal rows; the literal list they
    /// exclude is the projection's terminal set.
    #[test]
    fn open_process_gauge_excludes_exactly_the_terminal_states() {
        for s in ProcessState::ALL {
            assert_eq!(
                TERMINAL_STATE_SQL.contains(&format!("'{}'", s.as_str())),
                s.is_terminal(),
                "{s:?} is on the wrong side of the open-process gauge filter"
            );
        }
    }
}
