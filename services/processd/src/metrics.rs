//! processd's own Prometheus metrics, on the shared registry.
//!
//! `mako_service::run` mounts `GET /metrics` and the `mako_http_*` request
//! metrics; everything below registers on that same default registry and is
//! served from that one endpoint. Nothing here queries the database inside a
//! scrape — the gauges are sampled on a timer, so a slow database delays a
//! sample instead of hanging the scrape.
//!
//! ## Counters vs gauges
//!
//! The STP decision counts are a **counter**: they only ever go up, and
//! Prometheus needs them monotonic to compute a rate. They are incremented at
//! the moment a decision is made rather than re-counted from the table, so a
//! restart starts from zero (which `rate()` and `increase()` handle) instead of
//! silently re-counting history.
//!
//! The queue depths are **gauges** sampled on a timer, because "how many
//! processes are waiting for an operator right now" is a level, not a rate.

use std::sync::OnceLock;
use std::time::Duration;

use prometheus::{IntCounterVec, IntGauge, register_int_counter_vec, register_int_gauge};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

/// How often the queue gauges are re-sampled.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(60);

static DECISIONS: OnceLock<IntCounterVec> = OnceLock::new();
static QUEUE_PENDING: OnceLock<IntGauge> = OnceLock::new();
static QUEUE_OVERDUE: OnceLock<IntGauge> = OnceLock::new();
static EOG_OPEN: OnceLock<IntGauge> = OnceLock::new();

fn decisions() -> &'static IntCounterVec {
    DECISIONS.get_or_init(|| {
        register_int_counter_vec!(
            "processd_decisions_total",
            "NB Anmeldung STP decisions, by outcome and Prüfidentifikator. \
             `rate(processd_decisions_total{decision=\"Accept\"}[1d]) / \
             rate(processd_decisions_total[1d])` is the STP rate the ≥ 95 % target refers to.",
            &["decision", "pid"]
        )
        .expect("processd_decisions_total registration")
    })
}

/// Record one NB STP decision. Called where the decision is made.
pub fn record_decision(decision: &str, pid: u32) {
    decisions()
        .with_label_values(&[decision, &pid.to_string()])
        .inc();
}

fn gauges() -> (&'static IntGauge, &'static IntGauge, &'static IntGauge) {
    (
        QUEUE_PENDING.get_or_init(|| {
            register_int_gauge!(
                "processd_approval_queue_pending",
                "Queue entries waiting for an operator decision. Each one is a market \
                 process whose answer deadline is running."
            )
            .expect("processd_approval_queue_pending registration")
        }),
        QUEUE_OVERDUE.get_or_init(|| {
            register_int_gauge!(
                "processd_approval_queue_overdue",
                "Pending queue entries already past `expires_at` — the answer deadline \
                 has been missed and the market message is unanswered. Alert on > 0."
            )
            .expect("processd_approval_queue_overdue registration")
        }),
        EOG_OPEN.get_or_init(|| {
            register_int_gauge!(
                "processd_eog_open",
                "Ersatz-/Grundversorgung cases not yet closed (§ 36/§ 38 EnWG). Rising \
                 without falling means supply gaps are being detected but not resolved."
            )
            .expect("processd_eog_open registration")
        }),
    )
}

/// Sample the gauges once a minute until `shutdown` is cancelled.
pub fn spawn_sampler(pool: PgPool, tenant: String, shutdown: CancellationToken) {
    // Register eagerly so a dashboard sees 0 rather than a gap before the first
    // sample lands.
    let _ = gauges();
    let _ = decisions();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = interval.tick() => sample(&pool, &tenant).await,
            }
        }
    });
}

async fn sample(pool: &PgPool, tenant: &str) {
    let (pending, overdue, eog_open) = gauges();

    // One round trip: the three answer the same operational question and a
    // scrape reads them together, so they should describe one instant.
    let row: Result<(i64, i64, i64), _> = sqlx::query_as(
        "SELECT (SELECT count(*) FROM approval_queue
                  WHERE tenant = $1 AND status = 'Pending'),
                (SELECT count(*) FROM approval_queue
                  WHERE tenant = $1 AND status = 'Pending' AND expires_at < now()),
                (SELECT count(*) FROM eog_activations
                  WHERE tenant = $1 AND status <> 'closed')",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await;

    match row {
        Ok((p, o, e)) => {
            pending.set(p);
            overdue.set(o);
            eog_open.set(e);
        }
        // Keep the previous sample: a database blip should not read on a
        // dashboard as "the queue drained".
        Err(e) => tracing::warn!(error = %e, "metrics: gauge sample failed"),
    }
}
