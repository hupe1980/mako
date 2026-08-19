//! marktd's own Prometheus gauges, on the shared registry.
//!
//! `mako_service::run` mounts `GET /metrics` (the shared handler encodes the
//! default Prometheus registry) and the `mako_http_*` request metrics. The
//! gauges below register on that same registry, so they appear on the same
//! endpoint without marktd routing a second `/metrics`.
//!
//! ## Why a refresher and not a query per scrape
//!
//! The previous handler ran three `COUNT(*)` statements — over
//! `event_delivery`, `subscriptions`, and `processed_events` — inside the
//! request. A Prometheus scrape every 15 s therefore meant three sequential
//! scans every 15 s against tables that grow with traffic, and a slow database
//! turned a scrape into a hung request. Sampling on a timer decouples the two:
//! scrapes are always instant, and the sample rate is a knob rather than a
//! function of how many Prometheus replicas point at the service.

use std::time::Duration;

use prometheus::{IntGauge, register_int_gauge};
use sqlx::PgPool;
use std::sync::OnceLock;
use tokio_util::sync::CancellationToken;

/// How often the gauges are re-sampled from the database.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(60);

static DLQ_DEPTH: OnceLock<IntGauge> = OnceLock::new();
static ACTIVE_SUBSCRIPTIONS: OnceLock<IntGauge> = OnceLock::new();
static PENDING_FANOUT: OnceLock<IntGauge> = OnceLock::new();
static PROCESSED_EVENTS: OnceLock<IntGauge> = OnceLock::new();

fn gauges() -> (
    &'static IntGauge,
    &'static IntGauge,
    &'static IntGauge,
    &'static IntGauge,
) {
    (
        DLQ_DEPTH.get_or_init(|| {
            register_int_gauge!(
                "marktd_fanout_dlq_depth",
                "Fan-out deliveries that exhausted their retries and were dead-lettered. \
                 Non-zero means a subscriber is not receiving market events (§ 147 AO / GoBD: \
                 they are retained, never dropped) — inspect via GET /admin/fanout/dlq."
            )
            .expect("marktd_fanout_dlq_depth registration")
        }),
        ACTIVE_SUBSCRIPTIONS.get_or_init(|| {
            register_int_gauge!(
                "marktd_active_subscriptions",
                "Active webhook subscriptions receiving the durable fan-out."
            )
            .expect("marktd_active_subscriptions registration")
        }),
        PENDING_FANOUT.get_or_init(|| {
            register_int_gauge!(
                "marktd_fanout_pending",
                "Events written to the outbox but not yet fanned out to subscribers. \
                 A rising value means the fan-out worker is behind or stopped."
            )
            .expect("marktd_fanout_pending registration")
        }),
        PROCESSED_EVENTS.get_or_init(|| {
            register_int_gauge!(
                "marktd_processed_events",
                "Rows in the inbound idempotency table. Bounded by the retention sweep; \
                 unbounded growth means the sweep is not running."
            )
            .expect("marktd_processed_events registration")
        }),
    )
}

/// Sample the gauges once a minute until `shutdown` is cancelled.
pub fn spawn_sampler(pool: PgPool, shutdown: CancellationToken) {
    // Register eagerly so the metrics exist (at 0) from the first scrape rather
    // than appearing a minute in, which reads as a gap in a dashboard.
    let _ = gauges();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = interval.tick() => sample(&pool).await,
            }
        }
    });
}

async fn sample(pool: &PgPool) {
    let (dlq, subs, pending, processed) = gauges();

    // One round trip for all four: they are sampled together and a scrape reads
    // them together, so splitting them would only add latency and let the
    // values disagree about which instant they describe.
    let row: Result<(i64, i64, i64, i64), _> = sqlx::query_as(
        "SELECT (SELECT count(*) FROM event_delivery WHERE dead_lettered_at IS NOT NULL),
                (SELECT count(*) FROM subscriptions   WHERE active),
                (SELECT count(*) FROM event_log       WHERE fanned_out_at IS NULL),
                (SELECT count(*) FROM processed_events)",
    )
    .fetch_one(pool)
    .await;

    match row {
        Ok((d, s, p, e)) => {
            dlq.set(d);
            subs.set(s);
            pending.set(p);
            processed.set(e);
        }
        // Leave the previous sample in place: a transient database blip should
        // not read on a dashboard as "the dead-letter queue emptied".
        Err(e) => tracing::warn!(error = %e, "metrics: gauge sample failed"),
    }
}
