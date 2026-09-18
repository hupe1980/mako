//! Durable emission of edmd's `CloudEvents`.
//!
//! Every event edmd publishes goes through `mako_service::outbox`, so it
//! survives the receiver being restarted rather than living only for the three
//! attempts inside the request that produced it.
//!
//! The distinction matters most on the metering path.
//! `de.messwert.reading.direct.stored` is what tells `billingd` to recompute, so
//! a batch that is stored and never announced is consumption that is never
//! billed — and nothing re-derives it: there is no watermark, no sweep and no
//! reconciliation that would notice. A fleet-wide push during a `billingd`
//! rollout is exactly when that happens.
//!
//! Emission is **not** conditioned on a webhook being configured. Gating the
//! emission on the delivery target means a webhook added later cannot recover
//! the events of the period before it, because nothing recorded them; gating
//! only the delivery leaves them in `event_outbox`, which is also where an
//! operator finds what a dead-letter exhausted.

use sqlx::PgPool;

use mako_service::CloudEvent;

/// Enqueue `ce` for delivery.
///
/// Enqueue is idempotent on the `CloudEvent` id, so a retried request cannot
/// double-announce.
///
/// A failure to reach the outbox is logged rather than returned: every call
/// site has already committed its business write, so there is nothing left to
/// refuse and the caller's own error path would only turn a recorded event into
/// a failed request. Where the caller *does* still hold the transaction that
/// carries the write, [`emit_tx`] keeps the two atomic instead.
pub(crate) async fn emit(pool: &PgPool, ce: &CloudEvent) {
    let mut conn = match pool.acquire().await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::error!(
                error = %e, ce_type = %ce.ce_type,
                "edmd: no connection to enqueue CloudEvent — event not persisted"
            );
            return;
        }
    };
    if let Err(e) = mako_service::outbox::enqueue(&mut conn, ce).await {
        tracing::error!(
            error = %e, ce_type = %ce.ce_type,
            "edmd: CloudEvent could not be written to the outbox — event not persisted"
        );
    }
}

/// Enqueue `ce` **inside the caller's transaction**, so the event and the
/// business write commit together or not at all.
///
/// # Errors
///
/// Returns `sqlx::Error` so the caller can abort its transaction: an event that
/// cannot be recorded must take the write with it, which is the whole point of
/// enqueuing here rather than through [`emit`].
pub(crate) async fn emit_tx(
    conn: &mut sqlx::PgConnection,
    ce: &CloudEvent,
) -> Result<(), sqlx::Error> {
    mako_service::outbox::enqueue(conn, ce).await
}
