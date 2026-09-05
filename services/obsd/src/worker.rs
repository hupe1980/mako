//! Background sweep workers that produce the `de.obs.*` CloudEvents.
//!
//! `obsd` is otherwise a read-model: it projects inbound `de.mako.*` events and
//! serves queries. These two workers are its only **producers**:
//!
//! - `de.obs.deadline.approaching` — emitted per tracked process whose
//!   regulatory response deadline falls inside the warn window and has not yet
//!   been alerted (consumed by agentd's `deadline-alert-agent`).
//! - `de.obs.stp.parity.alert` — emitted when the § 7a Abs. 5 EnWG completion-rate
//!   gap between affiliate- and third-party-initiated Lieferanten processes passes
//!   the **operator-configured** threshold (consumed by agentd's
//!   `compliance-agent`). No BNetzA publication sets a numeric parity limit for
//!   this figure, so the threshold is an internal escalation policy.
//!
//! Both emit fire-and-forget CloudEvents to the configured outbound webhook
//! (`marktd`'s event-ingest fan-out in production), HMAC-signed when a secret
//! is set. The workers only run when `webhook.outbound_url` is configured.

use std::sync::Arc;
use std::time::Duration;

use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Immutable knobs shared by the workers.
#[derive(Clone)]
pub struct WorkerRuntime {
    pub pool: PgPool,
    pub client: Arc<reqwest::Client>,
    pub outbound_url: Arc<String>,
    pub outbound_secret: Option<Arc<String>>,
    pub tenant: String,
    pub deadline_sweep_secs: u64,
    pub deadline_warn_hours: i64,
    pub parity_sweep_secs: u64,
    pub parity_threshold_pp: f64,
    pub parity_window_days: i32,
}

/// Cap on deadline alerts emitted per sweep (protects the fan-out).
///
/// A tick that returns this many rows is reported as **saturated**: at least the
/// cap was waiting, never that the cap was all there was.
const DEADLINE_ALERT_LIMIT: i64 = 200;

/// Emit one `de.obs.*` CloudEvent, HMAC-signed when configured. Returns `true`
/// when the event reached the outbound webhook.
///
/// The deadline sweep stamps `deadline_alerted_at` from the outcome, so this
/// awaits the POST: a spawned emit let a downed webhook target lose the
/// warning permanently while the row was marked as alerted.
///
/// `subject` is the business subject the event concerns (a process id for the
/// per-process deadline alert); `None` for the tenant-level parity alert, which
/// has no single subject.
async fn emit_obs_event(
    rt: &WorkerRuntime,
    ce_type: &'static str,
    subject: Option<String>,
    data: serde_json::Value,
) -> bool {
    let source = mako_service::source("obsd", &rt.tenant);
    let ce = match subject {
        Some(s) => mako_service::CloudEvent::new(source, ce_type, s, data),
        None => {
            mako_service::CloudEvent::new(source, ce_type, String::new(), data).without_subject()
        }
    };
    match mako_service::post_ce_with_retry(
        &rt.client,
        rt.outbound_url.as_str(),
        &ce,
        rt.outbound_secret.as_deref().map(|s| s.as_bytes()),
    )
    .await
    {
        Ok(()) => true,
        Err(e) => {
            warn!(%e, ce_type, "obsd: outbound CloudEvent emit failed");
            false
        }
    }
}

// ── Deadline sweep ─────────────────────────────────────────────────────────────

/// Spawn the `de.obs.deadline.approaching` producer.
pub fn spawn_deadline_sweep(rt: WorkerRuntime, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(rt.deadline_sweep_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match sweep_deadlines(&rt).await {
                        // A healthy plane sweeps silently, so a non-silent
                        // sweep means something happened.
                        Ok(o) if o.is_quiet() => {}
                        Ok(o) if o.needs_attention() => warn!(
                            emitted = o.emitted,
                            considered = o.considered,
                            undelivered = o.undelivered,
                            saturated = o.saturated,
                            limit = DEADLINE_ALERT_LIMIT,
                            "obsd: deadline sweep needs attention — a saturated tick means at \
                             least the cap was waiting, never that the cap was all there was"
                        ),
                        Ok(o) => info!(emitted = o.emitted, "obsd: de.obs.deadline.approaching sweep"),
                        Err(e) => warn!(error = %e, "obsd: deadline sweep failed"),
                    }
                }
                _ = shutdown.cancelled() => break,
            }
        }
    });
}

/// The outcome of one deadline sweep.
///
/// `saturated` is reported rather than inferred, because a full batch reads
/// exactly like a normal one: **a saturated tick means at least the cap was
/// waiting — never that the cap was all there was.** A worker that logs only
/// `emitted = 200` on a backlog of ten thousand is telling an operator the
/// wrong thing in a calm voice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepOutcome {
    /// Alerts that reached the outbound webhook and were stamped.
    pub emitted: usize,
    /// Rows the sweep looked at.
    pub considered: usize,
    /// Alerts whose POST failed; deliberately left unstamped so the next tick
    /// retries them.
    pub undelivered: usize,
    /// The batch came back full.
    pub saturated: bool,
}

impl SweepOutcome {
    /// A tick worth no log line: nothing was due and nothing failed.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.considered == 0 && self.undelivered == 0
    }

    /// A tick an operator should see.
    #[must_use]
    pub fn needs_attention(&self) -> bool {
        self.saturated || self.undelivered > 0
    }
}

/// Emit `de.obs.deadline.approaching` for every open, not-yet-alerted process
/// whose Antwortfrist is within the warn window.
///
/// Deadlines **already past** when first seen are included, carrying
/// `"breached": true` — bounding on `deadline_at > now()` would mean an outage
/// longer than the warn window produces no event at all, in the case where the
/// alert matters most.
///
/// `aperak_timeout` rows are **not** excluded: a counterparty that missed the
/// 45-minute acknowledgement still owes the business answer, and is the one most
/// likely to miss it. Excluding them would also disagree with
/// `list_overdue_processes`, which does not.
pub async fn sweep_deadlines(rt: &WorkerRuntime) -> Result<SweepOutcome, sqlx::Error> {
    let rows = sqlx::query(&format!(
        r"SELECT process_id, pid, family, workflow_name, malo_id, partner_mp_id,
                 deadline_at, deadline_source, deadline_risk, tenant
          FROM process_projections
          WHERE state NOT IN ({terminal})
            AND deadline_at IS NOT NULL
            AND deadline_alerted_at IS NULL
            AND deadline_at <= now() + make_interval(hours => $1::int)
            AND ($2::text IS NULL OR tenant = $2)
          ORDER BY deadline_at ASC
          LIMIT $3",
        terminal = crate::pg::projection::TERMINAL_STATE_SQL,
    ))
    .bind(i32::try_from(rt.deadline_warn_hours).unwrap_or(24))
    .bind(tenant_filter(&rt.tenant))
    .bind(DEADLINE_ALERT_LIMIT)
    .fetch_all(&rt.pool)
    .await?;

    let mut outcome = SweepOutcome {
        considered: rows.len(),
        saturated: i64::try_from(rows.len()).unwrap_or(i64::MAX) >= DEADLINE_ALERT_LIMIT,
        ..SweepOutcome::default()
    };
    if rows.is_empty() {
        return Ok(outcome);
    }

    let now = OffsetDateTime::now_utc();
    let mut alerted: Vec<Uuid> = Vec::with_capacity(rows.len());
    for row in &rows {
        let process_id: Uuid = row.try_get("process_id")?;
        let deadline_at: OffsetDateTime = row.try_get("deadline_at")?;
        #[allow(clippy::cast_precision_loss)]
        let hours_remaining =
            ((deadline_at - now).whole_minutes() as f64 / 60.0 * 10.0).round() / 10.0;
        let data = serde_json::json!({
            "process_id":     process_id.to_string(),
            "pid":            row.try_get::<i32, _>("pid").ok(),
            "family":         row.try_get::<String, _>("family").ok(),
            "workflow_name":  row.try_get::<String, _>("workflow_name").ok(),
            "malo_id":        row.try_get::<Option<String>, _>("malo_id").ok().flatten(),
            "partner_mp_id":  row.try_get::<Option<String>, _>("partner_mp_id").ok().flatten(),
            "due_at":         deadline_at.format(&Rfc3339).unwrap_or_default(),
            "hours_remaining": hours_remaining,
            "breached":       deadline_at <= now,
            "deadline_risk":  row.try_get::<String, _>("deadline_risk").ok(),
            // The Fundstelle travels with the alert, so a recipient — an
            // operator or agentd's deadline specialist — can name the
            // Festlegung rather than trusting an instant.
            "deadline_source": row.try_get::<Option<String>, _>("deadline_source").ok().flatten(),
            "tenant":         row.try_get::<String, _>("tenant").ok(),
        });
        // Only a delivered alert may be stamped — otherwise a downed webhook
        // target silently consumes the warning and it is never re-emitted.
        if emit_obs_event(
            rt,
            mako_events::obs::DEADLINE_APPROACHING,
            Some(process_id.to_string()),
            data,
        )
        .await
        {
            alerted.push(process_id);
        } else {
            outcome.undelivered += 1;
        }
    }

    if !alerted.is_empty() {
        // Mark alerted so the next sweep does not re-emit for these processes.
        sqlx::query(
            "UPDATE process_projections SET deadline_alerted_at = now() WHERE process_id = ANY($1)",
        )
        .bind(&alerted)
        .execute(&rt.pool)
        .await?;
    }
    outcome.emitted = alerted.len();
    Ok(outcome)
}

// ── § 7a Abs. 5 EnWG parity sweep ─────────────────────────────────────────────

/// Spawn the `de.obs.stp.parity.alert` producer.
pub fn spawn_parity_sweep(rt: WorkerRuntime, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(rt.parity_sweep_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match sweep_parity(&rt).await {
                        Ok(true) => info!(
                            "obsd: de.obs.stp.parity.alert emitted — the completion-rate gap \
                             between affiliate and third-party Lieferanten passed the \
                             operator's configured threshold"
                        ),
                        Err(e) => warn!(error = %e, "obsd: parity sweep failed"),
                        Ok(false) => {}
                    }
                }
                _ = shutdown.cancelled() => break,
            }
        }
    });
}

/// The PIDs the parity comparison covers, as SQL.
///
/// Read off the Antwortfrist table rather than typed out: the comparison is over
/// the processes the operator's **network arm answers for a Lieferant**, which
/// is `answered_by == "NB"` in the GPKE and GeLi Gas families. A hand-written
/// list would include the Kündigung, which the NB never answers.
pub fn parity_pids_sql() -> String {
    use mako_fristen::antwort::Family;
    let pids: Vec<String> = mako_fristen::antwort::all()
        .filter(|o| o.answered_by == "NB" && matches!(o.family, Family::Gpke | Family::GeliGas))
        .map(|o| o.trigger_pid.to_string())
        .collect();
    pids.join(",")
}

/// Compute the § 7a Abs. 5 parity gap; emit an alert when it passes the
/// **operator-configured** threshold. Returns `true` when an alert was emitted.
///
/// The threshold is `[worker] parity_threshold_pp` and is an internal escalation
/// policy: the Bundesnetzagentur publishes no numeric parity limit for this
/// figure.
pub async fn sweep_parity(rt: &WorkerRuntime) -> Result<bool, sqlx::Error> {
    use mako_obs::domain::{ParityComparison, ParityGroup};

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
        pids = parity_pids_sql(),
    ))
    .bind(&rt.tenant)
    .bind(rt.parity_window_days)
    .fetch_all(&rt.pool)
    .await?;

    let (mut affiliate, mut third_party) = (ParityGroup::default(), ParityGroup::default());
    for row in &rows {
        let g = ParityGroup {
            total: row.try_get("total")?,
            completed: row.try_get("completed")?,
            rejected: row.try_get("rejected")?,
            frist_breached: row.try_get("frist_breached")?,
        };
        if row.try_get::<bool, _>("initiator_is_affiliate")? {
            affiliate = g;
        } else {
            third_party = g;
        }
    }

    let comparison = ParityComparison::new(affiliate, third_party);
    // `None` means the gap is unstatable — a group below the minimum sample.
    // Not "no gap": one affiliate process that happened to be rejected must not
    // page anybody with a hundred-percentage-point finding.
    if comparison.exceeds(rt.parity_threshold_pp) != Some(true) {
        return Ok(false);
    }

    Ok(emit_obs_event(
        rt,
        mako_events::obs::STP_PARITY_ALERT,
        None,
        serde_json::json!({
            "tenant":        rt.tenant,
            "window_days":   rt.parity_window_days,
            "threshold_pp":  rt.parity_threshold_pp,
            "affiliate":     comparison.affiliate,
            "third_party":   comparison.third_party,
            "gap_pp":        comparison.gap_pp,
            "frist_gap_pp":  comparison.frist_gap_pp,
            "favours":       comparison.favours(),
            "min_sample":    mako_obs::domain::PARITY_MIN_SAMPLE,
            "gap_convention": "gap_pp = (affiliate completion rate − third-party completion \
                               rate) × 100, and frist_gap_pp the same over the share answered \
                               inside the published Antwortfrist. Positive means the affiliate \
                               fared better; either alone crossing the threshold raises this \
                               alert.",
            "basis": "§ 7a Abs. 5 EnWG Gleichbehandlung, over the Lieferanten processes the \
                      network arm answers. The threshold is the operator's own escalation \
                      policy; no BNetzA publication sets a numeric parity limit.",
        }),
    )
    .await)
}

fn tenant_filter(tenant: &str) -> Option<String> {
    if tenant.is_empty() {
        None
    } else {
        Some(tenant.to_owned())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_obs::domain::{PARITY_MIN_SAMPLE, ParityComparison, ParityGroup};

    fn g(total: i64, completed: i64) -> ParityGroup {
        ParityGroup {
            total,
            completed,
            ..ParityGroup::default()
        }
    }

    #[test]
    fn small_samples_never_alert() {
        assert_eq!(
            ParityComparison::new(g(5, 5), g(100, 50)).exceeds(5.0),
            None
        );
        assert_eq!(
            ParityComparison::new(g(100, 100), g(3, 0)).exceeds(5.0),
            None
        );
    }

    #[test]
    fn a_gap_within_threshold_does_not_alert() {
        assert_eq!(
            ParityComparison::new(g(100, 96), g(100, 95)).exceeds(5.0),
            Some(false)
        );
    }

    /// A favoured affiliate is a **positive** gap — the convention the REST
    /// report and the MCP tool share.
    #[test]
    fn affiliate_favoured_beyond_threshold_alerts_positive() {
        let c = ParityComparison::new(g(50, 50), g(50, 40));
        assert_eq!(c.gap_pp, Some(20.0));
        assert_eq!(c.favours(), Some("affiliate"));
        assert_eq!(c.exceeds(5.0), Some(true));
    }

    #[test]
    fn third_party_favoured_beyond_threshold_alerts_negative() {
        let c = ParityComparison::new(g(100, 70), g(100, 90));
        assert_eq!(c.gap_pp, Some(-20.0));
        assert_eq!(c.favours(), Some("third_party"));
    }

    /// Equal completion rates do not mean equal treatment.
    ///
    /// § 7a Abs. 5 asks whether the network arm treats its affiliate's
    /// Lieferanten processes differently. A rejection can be entirely
    /// legitimate, so two groups can complete identically while one of them is
    /// routinely answered outside its published Antwortfrist — which is the
    /// disparity the filing is actually about, and the one mako can measure
    /// exactly because the window comes from `mako-fristen`.
    #[test]
    fn a_fristen_disparity_is_visible_where_the_completion_rate_is_not() {
        let breached = |total: i64, completed: i64, frist_breached: i64| ParityGroup {
            total,
            completed,
            frist_breached,
            ..ParityGroup::default()
        };
        // Both arms complete everything; the third party is answered late four
        // times in ten.
        let c = ParityComparison::new(breached(100, 100, 0), breached(100, 100, 40));
        assert_eq!(c.gap_pp, Some(0.0), "completion rates are identical");
        assert_eq!(
            c.frist_gap_pp,
            Some(40.0),
            "…and the affiliate is 40 pp better on the statutory window"
        );
        assert_eq!(
            c.exceeds(5.0),
            Some(true),
            "a Fristen disparity alone raises the alert"
        );
    }

    /// The Frist gap is held to the same minimum sample as the completion gap.
    #[test]
    fn a_small_sample_states_no_fristen_gap_either() {
        let c = ParityComparison::new(
            ParityGroup {
                total: PARITY_MIN_SAMPLE - 1,
                frist_breached: PARITY_MIN_SAMPLE - 1,
                ..ParityGroup::default()
            },
            g(100, 100),
        );
        assert_eq!(c.frist_gap_pp, None);
        assert_eq!(c.exceeds(5.0), None, "neither gap is statable");
    }

    /// The comparison covers exactly the processes the network arm answers for
    /// a Lieferant, taken from the Antwortfrist table.
    #[test]
    fn the_parity_pid_set_is_the_nb_answered_lieferanten_processes() {
        let sql = parity_pids_sql();
        let pids: Vec<&str> = sql.split(',').collect();
        for expected in ["55001", "55077", "55004", "44001", "44004"] {
            assert!(pids.contains(&expected), "{expected} missing from {sql}");
        }
        // 55016 is the Kündigung — answered by the old supplier, never the NB.
        assert!(
            !pids.contains(&"55016"),
            "the Kündigung is not an NB process"
        );
        // 55039 is WiM: an MSB process, not a Lieferanten one.
        assert!(!pids.contains(&"55039"));
        assert!(!sql.is_empty(), "an empty IN () list is a SQL syntax error");
    }

    /// The minimum sample is the domain's, not a second copy in this worker.
    ///
    /// Without one, a single affiliate process that happened to be rejected
    /// reads as a 100-percentage-point finding.
    #[test]
    fn the_minimum_sample_comes_from_the_domain() {
        let c = ParityComparison::new(g(PARITY_MIN_SAMPLE - 1, 0), g(100, 100));
        assert_eq!(c.gap_pp, None, "one short of the floor states no gap");

        let c = ParityComparison::new(g(PARITY_MIN_SAMPLE, 0), g(100, 100));
        assert_eq!(
            c.gap_pp,
            Some(-100.0),
            "exactly at the floor the gap becomes statable"
        );
    }

    /// A quiet tick says nothing; a saturated one says what it means.
    #[test]
    fn sweep_outcome_is_quiet_only_when_nothing_happened() {
        assert!(SweepOutcome::default().is_quiet());
        let saturated = SweepOutcome {
            emitted: 200,
            considered: 200,
            undelivered: 0,
            saturated: true,
        };
        assert!(!saturated.is_quiet());
        assert!(saturated.needs_attention());

        let normal = SweepOutcome {
            emitted: 3,
            considered: 3,
            undelivered: 0,
            saturated: false,
        };
        assert!(!normal.needs_attention(), "three alerts is a normal tick");

        let lost = SweepOutcome {
            emitted: 0,
            considered: 1,
            undelivered: 1,
            saturated: false,
        };
        assert!(
            lost.needs_attention(),
            "an undelivered warning is exactly what an operator must see"
        );
    }
}
