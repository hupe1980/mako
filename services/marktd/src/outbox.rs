//! Durable enqueue of marktd's events — *persist-before-fan-out*.
//!
//! Every domain event marktd produces is written to the service's
//! `event_outbox` in the transaction of the business write that caused it, and
//! `mako_service::subscriptions` fans it out to every matching endpoint — the
//! config-owned in-cluster receivers (`[[subscriptions.endpoints]]`) and the
//! endpoints integrators register — and serves it on `GET /api/v1/events`.
//!
//! The ordering key is the Marktlokation, else the Messlokation: deliveries of
//! one key reach an endpoint in commit order, because the order of a MaLo's
//! changes carries meaning; other keys never wait. The `marktsparte` and
//! `marktrole` extensions travel on the envelope, so an endpoint narrows on
//! them with its `extensions` filter.

use mako_markt::cloudevents::MarktEvent;

/// Persist a [`MarktEvent`] to `event_outbox` within the caller's transaction.
///
/// **Nothing to call afterwards.** The outbox's `AFTER INSERT` trigger raises
/// the `NOTIFY` the subscription worker listens for, and Postgres holds it
/// until this transaction commits.
///
/// # Errors
///
/// Returns [`sqlx::Error`] if the envelope cannot be encoded or the INSERT
/// fails. Callers on the producer path MUST treat this as fatal.
pub async fn enqueue(conn: &mut sqlx::PgConnection, ev: &MarktEvent) -> Result<(), sqlx::Error> {
    let ce = cloud_event(ev).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    let ordering_key = ev.marktmaloid.as_deref().or(ev.marktmeloid.as_deref());
    mako_service::outbox::enqueue_ordered(conn, &ce, ordering_key).await
}

/// The envelope as the shared outbox stores it: the `markt*` and `mako*`
/// extensions flat beside the core attributes, exactly as a subscriber
/// receives the [`MarktEvent`].
///
/// An event whose `marktsparte` is unset but whose `data` names a `sparte`
/// carries it as `marktsparte`: an endpoint narrows on the extension, and an
/// event without it would reach no Sparte-filtered endpoint at all.
fn cloud_event(ev: &MarktEvent) -> Result<mako_service::CloudEvent, serde_json::Error> {
    let mut envelope = serde_json::to_value(ev)?;
    if ev.marktsparte.is_none()
        && let Some(sparte) = ev.data.get("sparte").and_then(serde_json::Value::as_str)
    {
        envelope["marktsparte"] = serde_json::Value::String(sparte.to_owned());
    }
    serde_json::from_value(envelope)
}

#[cfg(test)]
mod tests {
    /// The envelope keeps every extension a subscriber filters or correlates on.
    #[test]
    fn the_envelope_carries_the_markt_extensions() {
        let mut ev = mako_markt::cloudevents::MarktEvent::new(
            "t1",
            mako_events::markt::MALO_UPDATED,
            "51238696012",
            serde_json::json!({ "malo_id": "51238696012" }),
        );
        ev.marktmaloid = Some("51238696012".into());
        ev.marktsparte = Some("STROM".into());
        let ce = super::cloud_event(&ev).expect("converts");
        let wire = serde_json::to_value(&ce).expect("serializes");
        assert_eq!(wire, serde_json::to_value(&ev).expect("serializes"));
        assert_eq!(wire["marktsparte"], "STROM");
        assert_eq!(wire["marktmaloid"], "51238696012");
    }

    /// A `data.sparte` stands in for an unset `marktsparte`.
    #[test]
    fn a_data_sparte_becomes_the_extension() {
        let ev = mako_markt::cloudevents::MarktEvent::new(
            "t1",
            mako_events::markt::MSB_RAHMENVERTRAG_UPDATED,
            "rv-1",
            serde_json::json!({ "sparte": "GAS" }),
        );
        let wire = serde_json::to_value(super::cloud_event(&ev).expect("converts")).expect("json");
        assert_eq!(wire["marktsparte"], "GAS");
    }
}
