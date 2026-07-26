//! Background sweep workers that produce the `de.obs.*` CloudEvents.
//!
//! `obsd` is otherwise a read-model: it projects inbound `de.mako.*` events and
//! serves queries. These two workers are its only **producers**:
//!
//! - `de.obs.deadline.approaching` — emitted per tracked process whose
//!   regulatory response deadline falls inside the warn window and has not yet
//!   been alerted (consumed by agentd's `deadline-alert-agent`).
//! - `de.obs.stp.parity.alert` — emitted when the §20 EnWG STP parity gap
//!   between affiliate- and non-affiliate-initiated Anmeldungen exceeds the
//!   configured threshold (consumed by agentd's `compliance-agent`).
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

/// Minimum sample size per group before a parity gap is considered meaningful.
const PARITY_MIN_SAMPLE: i64 = 10;
/// Cap on deadline alerts emitted per sweep (protects the fan-out).
const DEADLINE_ALERT_LIMIT: i64 = 200;

/// Emit one `de.obs.*` CloudEvent, fire-and-forget, HMAC-signed when configured.
fn emit_obs_event(rt: &WorkerRuntime, ce_type: &'static str, data: serde_json::Value) {
    let client = Arc::clone(&rt.client);
    let url = Arc::clone(&rt.outbound_url);
    let secret = rt.outbound_secret.clone();
    tokio::spawn(async move {
        let body = serde_json::json!({
            "specversion": "1.0",
            "type":        ce_type,
            "source":      "obsd",
            "id":          Uuid::new_v4().to_string(),
            "time":        OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default(),
            "data":        data,
        });
        let bytes = match serde_json::to_vec(&body) {
            Ok(b) => b,
            Err(e) => {
                warn!(%e, ce_type, "obsd: failed to serialize CloudEvent");
                return;
            }
        };
        let mut req = client
            .post(url.as_str())
            .header("Content-Type", "application/cloudevents+json")
            .body(bytes.clone());
        if let Some(sec) = secret.as_deref() {
            let sig = mako_markt::cloudevents::compute_signature(sec.as_bytes(), &bytes);
            req = req.header("X-Mako-Signature", format!("sha256={sig}"));
        }
        if let Err(e) = req.send().await {
            warn!(%e, ce_type, "obsd: outbound CloudEvent emit failed");
        }
    });
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
                        Ok(n) if n > 0 => info!(emitted = n, "obsd: de.obs.deadline.approaching sweep"),
                        Err(e) => warn!(error = %e, "obsd: deadline sweep failed"),
                        _ => {}
                    }
                }
                _ = shutdown.cancelled() => break,
            }
        }
    });
}

/// Emit `de.obs.deadline.approaching` for every open, not-yet-alerted process
/// whose deadline is within the warn window; returns the number emitted.
pub async fn sweep_deadlines(rt: &WorkerRuntime) -> Result<usize, sqlx::Error> {
    let rows = sqlx::query(
        r"SELECT process_id, pid, family, workflow_name, malo_id, partner_mp_id,
                 deadline_at, deadline_risk, tenant
          FROM process_projections
          WHERE state NOT IN ('completed','rejected','cancelled','aperak_timeout')
            AND deadline_at IS NOT NULL
            AND deadline_alerted_at IS NULL
            AND deadline_at > now()
            AND deadline_at <= now() + make_interval(hours => $1::int)
            AND ($2::text IS NULL OR tenant = $2)
          ORDER BY deadline_at ASC
          LIMIT $3",
    )
    .bind(i32::try_from(rt.deadline_warn_hours).unwrap_or(24))
    .bind(tenant_filter(&rt.tenant))
    .bind(DEADLINE_ALERT_LIMIT)
    .fetch_all(&rt.pool)
    .await?;

    if rows.is_empty() {
        return Ok(0);
    }

    let now = OffsetDateTime::now_utc();
    let mut alerted: Vec<Uuid> = Vec::with_capacity(rows.len());
    for row in &rows {
        let process_id: Uuid = row.try_get("process_id")?;
        let deadline_at: OffsetDateTime = row.try_get("deadline_at")?;
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
            "deadline_risk":  row.try_get::<String, _>("deadline_risk").ok(),
            "tenant":         row.try_get::<String, _>("tenant").ok(),
        });
        emit_obs_event(rt, mako_events::obs::DEADLINE_APPROACHING, data);
        alerted.push(process_id);
    }

    // Mark alerted so the next sweep does not re-emit for these processes.
    sqlx::query(
        "UPDATE process_projections SET deadline_alerted_at = now() WHERE process_id = ANY($1)",
    )
    .bind(&alerted)
    .execute(&rt.pool)
    .await?;

    Ok(alerted.len())
}

// ── §20 EnWG parity sweep ──────────────────────────────────────────────────────

/// Spawn the `de.obs.stp.parity.alert` producer.
pub fn spawn_parity_sweep(rt: WorkerRuntime, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(rt.parity_sweep_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match sweep_parity(&rt).await {
                        Ok(true) => info!("obsd: de.obs.stp.parity.alert emitted (§20 EnWG gap over threshold)"),
                        Err(e) => warn!(error = %e, "obsd: parity sweep failed"),
                        _ => {}
                    }
                }
                _ = shutdown.cancelled() => break,
            }
        }
    });
}

/// Compute the §20 EnWG parity gap; emit an alert when it exceeds the threshold.
/// Returns `true` when an alert was emitted.
pub async fn sweep_parity(rt: &WorkerRuntime) -> Result<bool, sqlx::Error> {
    let rows = sqlx::query(
        r"SELECT initiator_is_affiliate,
                 COUNT(*) AS total,
                 COUNT(*) FILTER (WHERE state = 'completed') AS completed
          FROM process_projections
          WHERE tenant = $1
            AND pid IN (55001, 55016, 44001)
            AND started_at >= now() - make_interval(days => $2::int)
          GROUP BY initiator_is_affiliate",
    )
    .bind(&rt.tenant)
    .bind(rt.parity_window_days)
    .fetch_all(&rt.pool)
    .await?;

    let (mut aff, mut non_aff) = (Group::default(), Group::default());
    for row in &rows {
        let is_aff: bool = row.try_get("initiator_is_affiliate")?;
        let g = Group {
            total: row.try_get("total")?,
            completed: row.try_get("completed")?,
        };
        if is_aff {
            aff = g;
        } else {
            non_aff = g;
        }
    }

    let Some((gap_pp, favored)) = parity_decision(&aff, &non_aff, rt.parity_threshold_pp) else {
        return Ok(false);
    };
    emit_obs_event(
        rt,
        mako_events::obs::STP_PARITY_ALERT,
        serde_json::json!({
            "tenant":        rt.tenant,
            "window_days":   rt.parity_window_days,
            "threshold_pp":  rt.parity_threshold_pp,
            "affiliate":     aff.to_json(),
            "non_affiliate": non_aff.to_json(),
            "parity_gap_pp": gap_pp,
            "favored":       favored,
            "note":          "§20 EnWG Diskriminierungsfreiheit: STP completion-rate gap between \
                              affiliate- and non-affiliate-initiated Anmeldungen exceeds the threshold",
        }),
    );
    Ok(true)
}

/// Pure §20 EnWG parity decision. Returns `Some((signed_gap_pp, favored))` when
/// both groups have enough evidence and the gap exceeds `threshold_pp`; `None`
/// otherwise. A **positive** gap means the affiliate has the higher STP rate —
/// the discrimination concern.
fn parity_decision(aff: &Group, non_aff: &Group, threshold_pp: f64) -> Option<(f64, &'static str)> {
    if aff.total < PARITY_MIN_SAMPLE || non_aff.total < PARITY_MIN_SAMPLE {
        return None;
    }
    let gap_pp = ((aff.rate() - non_aff.rate()) * 1000.0).round() / 10.0;
    if gap_pp.abs() < threshold_pp {
        return None;
    }
    Some((
        gap_pp,
        if gap_pp > 0.0 {
            "affiliate"
        } else {
            "non_affiliate"
        },
    ))
}

#[derive(Default)]
struct Group {
    total: i64,
    completed: i64,
}

impl Group {
    fn rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.completed as f64 / self.total as f64
        }
    }
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total": self.total,
            "completed": self.completed,
            "completion_rate": (self.rate() * 1000.0).round() / 10.0,
        })
    }
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
    use super::{Group, parity_decision};

    fn g(total: i64, completed: i64) -> Group {
        Group { total, completed }
    }

    #[test]
    fn small_samples_never_alert() {
        // Below PARITY_MIN_SAMPLE (10) in either group → no statement.
        assert!(parity_decision(&g(5, 5), &g(100, 50), 5.0).is_none());
        assert!(parity_decision(&g(100, 100), &g(3, 0), 5.0).is_none());
    }

    #[test]
    fn a_gap_within_threshold_does_not_alert() {
        // aff 96%, non-aff 95% → 1.0 pp gap, below the 5 pp threshold.
        assert!(parity_decision(&g(100, 96), &g(100, 95), 5.0).is_none());
    }

    #[test]
    fn affiliate_favoured_beyond_threshold_alerts_positive() {
        // aff 100%, non-aff 80% → +20 pp, affiliate favoured (§20 concern).
        let (gap, favored) = parity_decision(&g(50, 50), &g(50, 40), 5.0).expect("alert");
        assert_eq!(gap, 20.0);
        assert_eq!(favored, "affiliate");
    }

    #[test]
    fn non_affiliate_favoured_beyond_threshold_alerts_negative() {
        // aff 70%, non-aff 90% → -20 pp, non-affiliate favoured.
        let (gap, favored) = parity_decision(&g(100, 70), &g(100, 90), 5.0).expect("alert");
        assert_eq!(gap, -20.0);
        assert_eq!(favored, "non_affiliate");
    }
}
