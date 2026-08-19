//! Retention sweeps for the tables that grow without a business bound.

use std::time::Duration;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

/// How long an inbound CloudEvent ID stays in `processed_events`.
///
/// The table exists only to make `POST <inbound_path>` idempotent against
/// makod's at-least-once webhook channel. makod's outbox gives up after a 72 h
/// retry window, so seven days is well past the last moment a genuine duplicate
/// can arrive, and keeping more would grow the table forever for no benefit.
///
/// This is *not* the § 147 AO record — that is `event_log`, which holds the full
/// envelope and is retained by the operator.
const PROCESSED_EVENTS_TTL: Duration = Duration::from_secs(7 * 24 * 3_600);

/// Interval between sweeps.
const SWEEP_INTERVAL: Duration = Duration::from_secs(3_600);

/// Delete `processed_events` rows older than seven days, hourly, until
/// `shutdown` is cancelled.
pub fn spawn_processed_events_sweep(pool: PgPool, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = interval.tick() => sweep(&pool).await,
            }
        }
    });
}

async fn sweep(pool: &PgPool) {
    let ttl =
        time::Duration::seconds(i64::try_from(PROCESSED_EVENTS_TTL.as_secs()).unwrap_or(i64::MAX));
    let cutoff = time::OffsetDateTime::now_utc() - ttl;

    match sqlx::query("DELETE FROM processed_events WHERE processed_at < $1")
        .bind(cutoff)
        .execute(pool)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(
                rows = r.rows_affected(),
                "processed_events: pruned old entries"
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "processed_events: sweep failed"),
    }
}
