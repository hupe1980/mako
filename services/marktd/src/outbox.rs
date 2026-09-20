//! Durable enqueue for the marktd fan-out — *persist-before-fan-out*.
//!
//! Every domain event that marktd produces is written to the `event_log` table
//! (the full serialized [`MarktEvent`] envelope) **before** any fan-out happens.
//! The fan-out worker ([`crate::fanout`]) is the only consumer, and it reads
//! exclusively from `event_log` / `event_delivery` — there is no in-memory
//! channel that could lose an in-flight event across a crash.
//!
//! This mirrors [`mako_service::outbox`] in both halves: the `enqueue` INSERT is
//! idempotent on the CloudEvent `id` (`ON CONFLICT DO NOTHING`) and is **fatal
//! on the producer path** — a producer must propagate the error / fail the
//! request so that no event is ever fanned out unless it is durable — and the
//! worker's wake-up is a Postgres `NOTIFY` raised by an `AFTER INSERT` trigger
//! on the table, so it is delivered on the producer's COMMIT and reaches every
//! replica rather than only the process that wrote the row.

use mako_markt::cloudevents::MarktEvent;

/// Persist a [`MarktEvent`] to the durable `event_log` outbox.
///
/// `executor` may be a `&PgPool` (pure-relay producers) or a `&mut PgConnection`
/// / `&mut *tx` (producers that want the event to commit atomically with a
/// preceding business write). The whole envelope is stored so a subscriber
/// receives the exact `MarktEvent` (type, subject, data, extensions).
///
/// `sparte` comes from the `marktsparte` CloudEvents extension (falling back to
/// a `data.sparte` payload field) so the fan-out worker can honour a subscriber's
/// `sparten` filter without re-parsing the envelope. `NULL` means the event is
/// not Sparte-scoped and matches every filter — so a producer that knows the
/// Sparte must set it, or the filter silently passes everything.
///
/// **Nothing to call afterwards.** The `event_log_notify` trigger raises a
/// `NOTIFY` on [`crate::fanout::NOTIFY_CHANNEL`] that Postgres holds until this
/// transaction commits, and the fan-out worker is listening, so delivery starts
/// on the commit rather than at the next poll. Keeping that wake-up in the
/// database is what makes it impossible to take one too early: on a `&mut *tx`
/// executor the row is invisible to every other connection until the commit, so
/// an in-process hint raised here would be spent on a snapshot without the
/// event and the delivery would wait out a whole
/// [`crate::fanout::FanoutConfig::poll_interval`] with nothing logged.
///
/// # Errors
///
/// Returns [`sqlx::Error`] if the envelope cannot be encoded or the INSERT
/// fails. Callers on the producer path MUST treat this as fatal.
pub async fn enqueue<'e, E>(executor: E, ev: &MarktEvent) -> Result<(), sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let envelope = serde_json::to_value(ev).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    let sparte = ev
        .marktsparte
        .as_deref()
        .or_else(|| ev.data.get("sparte").and_then(serde_json::Value::as_str));

    // Aggregate this event is about. Deliveries to one subscriber are ordered
    // within an ordering_key and independent across keys, so the key is the thing
    // whose event *order* carries meaning: the Marktlokation, else the
    // Messlokation. Events tied to neither are unordered.
    let ordering_key = ev.marktmaloid.as_deref().or(ev.marktmeloid.as_deref());

    sqlx::query(
        "INSERT INTO event_log (event_id, ce_type, marktrole, sparte, ordering_key, envelope)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(&ev.id)
    .bind(&ev.ce_type)
    .bind(ev.marktrole.as_deref())
    .bind(sparte)
    .bind(ordering_key)
    .bind(envelope)
    .execute(executor)
    .await?;

    Ok(())
}
