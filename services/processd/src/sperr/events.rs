//! `de.sperr.*` CloudEvents.
//!
//! The **NB side** of a disconnection: what this grid operator was asked to do
//! and what its field team did. agentd's `sperrd-agent` subscribes to the glob.
//!
//! Delivery is the platform's transactional outbox, and every event is written
//! in the transaction of the state change it announces: an order moved without
//! its event is never moved again, so an event enqueued afterwards on its own
//! is one a failure loses for good. The builders here return the CloudEvent;
//! the transaction that changes the order enqueues it.

use uuid::Uuid;

use crate::sperr::pg::{CreateOrderRequest, Outcome};

fn source(tenant: &str) -> String {
    mako_service::source("processd", tenant)
}

/// A Sperr-/Entsperrauftrag entered the queue — enqueued by
/// [`crate::sperr::pg::create_order_pg`] in the transaction that inserts it.
#[must_use]
pub fn auftrag_eingegangen(
    tenant: &str,
    id: Uuid,
    req: &CreateOrderRequest,
) -> mako_service::CloudEvent {
    mako_service::CloudEvent::new(
        source(tenant),
        mako_events::sperr::AUFTRAG_EINGEGANGEN,
        &req.malo_id,
        serde_json::json!({
            "order_id":       id.to_string(),
            "malo_id":        req.malo_id,
            "lf_mp_id":       req.lf_mp_id,
            "order_type":     req.order_type.as_str(),
            "pid":            req.process_id.as_ref().map(|_| req.order_type.pid()),
            "process_id":     req.process_id,
            "ausfuehrung_am": req.ausfuehrung_am.map(|d| d.to_string()),
            "fruehestens_am": req.fruehestens_am.map(|d| d.to_string()),
            "arbeitszeit":    req.arbeitszeit.map(|a| a.code()),
        }),
    )
}

/// A Sperrversuch that did not succeed and left the order in the queue —
/// enqueued by [`crate::sperr::pg::report_outcome`] with the attempt.
///
/// Not `FEHLGESCHLAGEN`: nothing has been reported to the Lieferant, because
/// GPKE Teil 2 Kap. 3.5.1.2 Nr. 5 still owes them a second visit.
#[must_use]
pub fn versuch_gescheitert(
    tenant: &str,
    id: Uuid,
    malo_id: &str,
    reason: &str,
    sperrversuche: i32,
) -> mako_service::CloudEvent {
    mako_service::CloudEvent::new(
        source(tenant),
        mako_events::sperr::VERSUCH_GESCHEITERT,
        malo_id,
        serde_json::json!({
            "order_id":      id.to_string(),
            "malo_id":       malo_id,
            "grund":         reason,
            "sperrversuche": sperrversuche,
            "verbleibend":   crate::sperr::pg::MAX_SPERRVERSUCHE - sperrversuche,
        }),
    )
}

/// A pending order ran past the Kap. 3.5.1.2 Nr. 1 execution window.
pub async fn ausfuehrung_ueberfaellig(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    id: Uuid,
    malo_id: &str,
    lf_mp_id: &str,
    faellig_am: time::Date,
) -> anyhow::Result<()> {
    let ce = mako_service::CloudEvent::new(
        source(tenant),
        mako_events::sperr::AUSFUEHRUNG_UEBERFAELLIG,
        malo_id,
        serde_json::json!({
            "order_id":   id.to_string(),
            "malo_id":    malo_id,
            "lf_mp_id":   lf_mp_id,
            "faellig_am": faellig_am.to_string(),
            "frist":      "6 WT nach dem frühestmöglichen Sperrtermin \
                           (BK6-24-174 GPKE Teil 2 Kap. 3.5.1.2 Nr. 1)",
        }),
    );
    mako_service::outbox::enqueue(&mut *tx, &ce)
        .await
        .map(|_| ())
        .map_err(Into::into)
}

/// The field team reported a terminal outcome — enqueued by
/// [`crate::sperr::pg::report_outcome`] in the transaction that makes the
/// order terminal.
#[must_use]
pub fn outcome(
    tenant: &str,
    id: Uuid,
    outcome: &Outcome<'_>,
    iftsta_queued: bool,
) -> mako_service::CloudEvent {
    let (ce_type, detail) = match outcome {
        Outcome::Executed {
            at,
            note,
            pruefschritt_code,
        } => (
            mako_events::sperr::AUSGEFUEHRT,
            serde_json::json!({
                "fertigstellung":    at.format(&time::format_description::well_known::Rfc3339).ok(),
                "note":              note,
                "pruefschritt_code": pruefschritt_code,
            }),
        ),
        Outcome::Failed {
            reason,
            pruefschritt_code,
            ..
        } => (
            mako_events::sperr::FEHLGESCHLAGEN,
            serde_json::json!({
                "reason":            reason,
                "pruefschritt_code": pruefschritt_code,
            }),
        ),
    };
    // The subject is the order, not the MaLo: an order id is what every other
    // route in this service takes, and a MaLo can carry several over time.
    mako_service::CloudEvent::new(
        source(tenant),
        ce_type,
        id.to_string(),
        serde_json::json!({
            "order_id":           id.to_string(),
            "status":             outcome.status().as_str(),
            "iftsta_code":        outcome.status().iftsta_code(),
            "iftsta_queued":      iftsta_queued,
            "detail":             detail,
        }),
    )
}

/// A pending order was withdrawn.
///
/// Takes the transaction: the cancellation and its announcement commit together
/// because both are ours to undo — nothing physical has happened.
///
/// # Errors
///
/// Propagates outbox errors so the caller can roll the cancellation back.
pub async fn storniert(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    id: Uuid,
    malo_id: &str,
    lf_mp_id: &str,
) -> Result<(), sqlx::Error> {
    let ce = mako_service::CloudEvent::new(
        source(tenant),
        mako_events::sperr::STORNIERT,
        malo_id,
        serde_json::json!({
            "order_id": id.to_string(),
            "malo_id":  malo_id,
            "lf_mp_id": lf_mp_id,
            "note": "no IFTSTA 21039 is dispatched — a cancelled order was never executed",
        }),
    );
    mako_service::outbox::enqueue(tx, &ce).await
}

/// An IFTSTA 21039 was dead-lettered, or is past its Frist undelivered.
///
/// The one state in this service that needs a human: the Lieferant has not been
/// told the outcome, and their `gpke-sperrung-lf` process cannot close.
///
/// # Errors
///
/// Propagates outbox errors.
pub async fn iftsta_ausstehend(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    order: &crate::sperr::pg::StuckIftsta,
) -> Result<(), sqlx::Error> {
    let ce = mako_service::CloudEvent::new(
        source(tenant),
        mako_events::sperr::IFTSTA_AUSSTEHEND,
        order.id.to_string(),
        serde_json::json!({
            "order_id":   order.id.to_string(),
            "malo_id":    order.malo_id,
            "lf_mp_id":   order.lf_mp_id,
            // The attempts actually made, not the cap: this order may have been
            // escalated on its deadline while still inside its retry budget.
            "attempts":   order.attempts,
            "max_attempts": mako_service::outbox::OutboxConfig::default().max_attempts,
            "last_error": order.last_error,
            "impact": "the Lieferant has not received the Auftragsstatus; their \
                       gpke-sperrung-lf process cannot reach a terminal state",
        }),
    );
    mako_service::outbox::enqueue(tx, &ce).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_emitted_type_is_in_the_catalog() {
        // The catalog is the contract agentd's subscriptions are written
        // against; a type spelled here and not there is a subscription that
        // never fires.
        for t in [
            mako_events::sperr::AUFTRAG_EINGEGANGEN,
            mako_events::sperr::AUSGEFUEHRT,
            mako_events::sperr::FEHLGESCHLAGEN,
            mako_events::sperr::STORNIERT,
            mako_events::sperr::IFTSTA_AUSSTEHEND,
        ] {
            assert!(
                mako_events::all().contains(&t),
                "{t} is not in mako_events::all()"
            );
            assert!(
                mako_events::matches("de.sperr.*", t),
                "{t} does not match the glob agentd's sperrd-agent subscribes to"
            );
        }
    }
}
