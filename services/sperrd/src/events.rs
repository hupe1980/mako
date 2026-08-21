//! `de.sperr.*` CloudEvents.
//!
//! The **NB side** of a disconnection: what this grid operator was asked to do
//! and what its field team did. agentd's `sperrd-agent` subscribes to the glob.
//!
//! Delivery is the platform's transactional outbox — written in the same
//! transaction as the state change it announces where that is possible, and
//! after it where the state change is a fact about the physical world that must
//! not be rolled back by an enqueue failure.

use sqlx::PgPool;
use uuid::Uuid;

use crate::pg::{CreateOrderRequest, Outcome};

fn source(tenant: &str) -> String {
    mako_service::source("sperrd", tenant)
}

/// A Sperr-/Entsperrauftrag entered the queue.
///
/// Enqueued after the insert rather than inside it: the order is already
/// visible to the field team and a lost announcement is replayable, while
/// rolling the order back because the outbox was unavailable would drop a
/// market message the Lieferant is waiting on.
pub async fn auftrag_eingegangen(pool: &PgPool, tenant: &str, id: Uuid, req: &CreateOrderRequest) {
    let ce = mako_service::CloudEvent::new(
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
    );
    enqueue(pool, &ce, id).await;
}

/// A Sperrversuch that did not succeed and left the order in the queue.
///
/// Not `FEHLGESCHLAGEN`: nothing has been reported to the Lieferant, because
/// GPKE Teil 2 § 3.5.1.2 Nr. 5 still owes them a second visit.
pub async fn versuch_gescheitert(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
    malo_id: &str,
    reason: &str,
    sperrversuche: i32,
) {
    let ce = mako_service::CloudEvent::new(
        source(tenant),
        mako_events::sperr::VERSUCH_GESCHEITERT,
        malo_id,
        serde_json::json!({
            "order_id":      id.to_string(),
            "malo_id":       malo_id,
            "grund":         reason,
            "sperrversuche": sperrversuche,
            "verbleibend":   crate::pg::MAX_SPERRVERSUCHE - sperrversuche,
        }),
    );
    enqueue(pool, &ce, id).await;
}

/// A pending order ran past the § 3.5.1.2 Nr. 1 execution window.
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
                           (BK6-24-174 GPKE Teil 2 § 3.5.1.2 Nr. 1)",
        }),
    );
    mako_service::outbox::enqueue(&mut *tx, &ce)
        .await
        .map(|_| ())
        .map_err(Into::into)
}

/// The field team reported a terminal outcome.
pub async fn outcome(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
    outcome: &Outcome<'_>,
    iftsta_dispatched: bool,
) {
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
    let ce = mako_service::CloudEvent::new(
        source(tenant),
        ce_type,
        id.to_string(),
        serde_json::json!({
            "order_id":           id.to_string(),
            "status":             outcome.status().as_str(),
            "iftsta_code":        outcome.status().iftsta_code(),
            "iftsta_dispatched":  iftsta_dispatched,
            "detail":             detail,
        }),
    );
    enqueue(pool, &ce, id).await;
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

/// An IFTSTA 21039 exhausted its retry budget.
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
    id: Uuid,
    malo_id: &str,
    lf_mp_id: &str,
    last_error: &str,
) -> Result<(), sqlx::Error> {
    let ce = mako_service::CloudEvent::new(
        source(tenant),
        mako_events::sperr::IFTSTA_AUSSTEHEND,
        id.to_string(),
        serde_json::json!({
            "order_id":   id.to_string(),
            "malo_id":    malo_id,
            "lf_mp_id":   lf_mp_id,
            "attempts":   crate::pg::IFTSTA_MAX_ATTEMPTS,
            "last_error": last_error,
            "impact": "the Lieferant has not received the Auftragsstatus; their \
                       gpke-sperrung-lf process cannot reach a terminal state",
        }),
    );
    mako_service::outbox::enqueue(tx, &ce).await
}

/// Enqueue an announcement whose state change has already committed.
async fn enqueue(pool: &PgPool, ce: &mako_service::CloudEvent, id: Uuid) {
    let res = async {
        let mut tx = pool.begin().await?;
        mako_service::outbox::enqueue(&mut tx, ce).await?;
        tx.commit().await
    }
    .await;
    if let Err(e) = res {
        tracing::error!(
            order_id = %id, ce_type = %ce.ce_type, error = %e,
            "sperrd: state change committed but the CloudEvent was NOT announced — replay it"
        );
    }
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
