//! Durable enqueue for the marktd fan-out — *persist-before-fan-out*.
//!
//! Every domain event that marktd produces is written to the `event_log` table
//! (the full serialized [`MarktEvent`] envelope) **before** any fan-out happens.
//! The fan-out worker ([`crate::fanout`]) is the only consumer, and it reads
//! exclusively from `event_log` / `event_delivery` — there is no in-memory
//! channel that could lose an in-flight event across a crash.
//!
//! This mirrors [`mako_service::outbox`]: the `enqueue` INSERT is idempotent on
//! the CloudEvent `id` (`ON CONFLICT DO NOTHING`) and is **fatal on the producer
//! path** — a producer must propagate the error / fail the request so that no
//! event is ever fanned out unless it is durable.

use mako_markt::cloudevents::MarktEvent;
use tokio::sync::Notify;

/// Persist a [`MarktEvent`] to the durable `event_log` outbox.
///
/// `executor` may be a `&PgPool` (pure-relay producers) or a `&mut PgConnection`
/// / `&mut *tx` (producers that want the event to commit atomically with a
/// preceding business write). The whole envelope is stored so a subscriber
/// receives the exact `MarktEvent` (type, subject, data, extensions).
///
/// `sparte` is derived from the event payload (`data.sparte`) when present, so
/// the fan-out worker can honour a subscriber's `sparten` filter without
/// re-parsing the envelope; otherwise it is `NULL` (matches every sparte).
///
/// `notify` is a low-latency wake-up hint for the fan-out worker — it is **not**
/// correctness-bearing (the worker also polls on an interval), so a missed
/// notification only delays, never drops, delivery.
///
/// # Errors
///
/// Returns [`sqlx::Error`] if the envelope cannot be encoded or the INSERT
/// fails. Callers on the producer path MUST treat this as fatal.
pub async fn enqueue<'e, E>(
    executor: E,
    ev: &MarktEvent,
    notify: &Notify,
) -> Result<(), sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let envelope = serde_json::to_value(ev).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    let sparte = ev.data.get("sparte").and_then(serde_json::Value::as_str);

    sqlx::query(
        "INSERT INTO event_log (event_id, ce_type, marktrole, sparte, envelope)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(&ev.id)
    .bind(&ev.ce_type)
    .bind(ev.marktrole.as_deref())
    .bind(sparte)
    .bind(envelope)
    .execute(executor)
    .await?;

    // Low-latency wake-up hint only; correctness rests on the worker's poll loop.
    notify.notify_one();
    Ok(())
}
