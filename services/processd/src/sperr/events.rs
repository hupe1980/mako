//! `de.mako.sperr.*` CloudEvents.
//!
//! The **NB side** of a disconnection: what this grid operator was asked to do
//! and what its field team did. agentd's `sperrd-agent` subscribes to the glob,
//! and so does whatever system carries the orders out.
//!
//! Every type is emitted through a payload bound to it, so its `data` has a
//! published schema (`events/<type>.json`), and every payload echoes the
//! order's `external_ref`: the executing system reads its own reference
//! instead of keeping a table of ours.
//!
//! Delivery is the platform's transactional outbox, and every event is written
//! in the transaction of the state change it announces: an order moved without
//! its event is never moved again, so an event enqueued afterwards on its own
//! is one a failure loses for good. The builders here return the CloudEvent;
//! the transaction that changes the order enqueues it.

use mako_service::{CloudEvent, EventPayload};
use serde::Serialize;
use uuid::Uuid;

use crate::sperr::model::{Arbeitszeit, OrderStatus, OrderType, Responsible};
use crate::sperr::pg::{CreateOrderRequest, Outcome, Treffpunkt};

fn source(tenant: &str) -> String {
    mako_service::source("processd", tenant)
}

/// A CloudEvent of `payload`. A payload here is plain data, so encoding it
/// cannot fail; a failure would be a bug and is reported as the outbox's error.
fn typed<P: EventPayload>(tenant: &str, subject: impl Into<String>, payload: &P) -> CloudEvent {
    let subject = subject.into();
    CloudEvent::typed(source(tenant), subject.clone(), payload).unwrap_or_else(|e| {
        tracing::error!(error = %e, ce_type = P::TYPE, "processd sperr: event payload not encodable");
        CloudEvent::new(source(tenant), P::TYPE, subject, serde_json::Value::Null)
    })
}

fn rfc3339(at: time::OffsetDateTime) -> Option<String> {
    at.format(&time::format_description::well_known::Rfc3339)
        .ok()
}

/// What an executing system needs to plan the visit.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Einsatz {
    /// `sperrung` or `entsperrung`.
    pub order_type: OrderType,
    /// `DTM+203` — the fixed day the Lieferant requires.
    #[schemars(extend("format" = "date"))]
    pub ausfuehrung_am: Option<String>,
    /// `DTM+469` — not before this day.
    #[schemars(extend("format" = "date"))]
    pub fruehestens_am: Option<String>,
    /// The last day of the Netzbetreiber's execution window: 6 Werktage after
    /// the frühestmöglicher Sperrtermin (GPKE Teil 2 Kap. 3.5.1.2 Nr. 1), in
    /// the BDEW-MaKo calendar. Absent for an order that names no date.
    #[schemars(extend("format" = "date"))]
    pub ausfuehrung_faellig_am: Option<String>,
    /// `IMD 7081` on an Entsperrauftrag: `Z53` innerhalb, `Z54` auch außerhalb
    /// der Arbeitszeit.
    pub arbeitszeit: Option<String>,
    /// `SG2 NAD+Z24` — where the technician goes.
    pub treffpunkt: Treffpunkt,
    /// `SG29 FTX+ACB` — the Lieferant's free text.
    pub hinweis: Option<String>,
}

impl Einsatz {
    fn of_request(req: &CreateOrderRequest) -> Self {
        Self {
            order_type: req.order_type,
            ausfuehrung_am: req.ausfuehrung_am.map(|d| d.to_string()),
            fruehestens_am: req.fruehestens_am.map(|d| d.to_string()),
            ausfuehrung_faellig_am: crate::sperr::pg::ausfuehrung_faellig_am(
                req.ausfuehrung_am,
                req.fruehestens_am,
            )
            .map(|d| d.to_string()),
            arbeitszeit: req.arbeitszeit.map(|a| a.code().to_owned()),
            treffpunkt: req.treffpunkt.clone(),
            hinweis: req.hinweis.clone(),
        }
    }
}

/// The `data` of `de.mako.sperr.auftrag.eingegangen`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AuftragEingegangen {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    /// The ordering Lieferant's MP-ID.
    pub lf_mp_id: String,
    /// 17115 or 17117; absent for an order an operator created by hand.
    pub pid: Option<i32>,
    /// The makod process the order came in on.
    pub process_id: Option<String>,
    #[serde(flatten)]
    pub einsatz: Einsatz,
}

impl EventPayload for AuftragEingegangen {
    const TYPE: &'static str = mako_events::sperr::AUFTRAG_EINGEGANGEN;
}

/// A Sperr-/Entsperrauftrag entered the queue — enqueued by
/// [`crate::sperr::pg::create_order_pg`] in the transaction that inserts it.
#[must_use]
pub fn auftrag_eingegangen(tenant: &str, id: Uuid, req: &CreateOrderRequest) -> CloudEvent {
    typed(
        tenant,
        &req.malo_id,
        &AuftragEingegangen {
            order_id: id.to_string(),
            external_ref: req.external_ref.clone(),
            malo_id: req.malo_id.clone(),
            lf_mp_id: req.lf_mp_id.clone(),
            pid: req.process_id.as_ref().map(|_| req.order_type.pid()),
            process_id: req.process_id.clone(),
            einsatz: Einsatz::of_request(req),
        },
    )
}

/// The `data` of `de.mako.sperr.auftrag.faellig`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AuftragFaellig {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    /// The ordering Lieferant's MP-ID.
    pub lf_mp_id: String,
    #[serde(flatten)]
    pub einsatz: Einsatz,
}

impl EventPayload for AuftragFaellig {
    const TYPE: &'static str = mako_events::sperr::AUFTRAG_FAELLIG;
}

/// A pending order's day has come — enqueued by
/// [`crate::sperr::pg::claim_due`] with the stamp that makes it once.
#[must_use]
pub fn auftrag_faellig(tenant: &str, order: &crate::sperr::pg::DueOrder) -> CloudEvent {
    typed(
        tenant,
        order.id.to_string(),
        &AuftragFaellig {
            order_id: order.id.to_string(),
            external_ref: order.external_ref.clone(),
            malo_id: order.malo_id.clone(),
            lf_mp_id: order.lf_mp_id.clone(),
            einsatz: Einsatz {
                order_type: order.order_type,
                ausfuehrung_am: order.ausfuehrung_am.map(|d| d.to_string()),
                fruehestens_am: order.fruehestens_am.map(|d| d.to_string()),
                ausfuehrung_faellig_am: order.ausfuehrung_faellig_am.map(|d| d.to_string()),
                arbeitszeit: order.arbeitszeit.map(|a: Arbeitszeit| a.code().to_owned()),
                treffpunkt: order.treffpunkt.clone(),
                hinweis: order.hinweis.clone(),
            },
        },
    )
}

/// The `data` of `de.mako.sperr.versuch.gescheitert`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct VersuchGescheitert {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    /// Why the visit did not succeed, as reported.
    pub grund: String,
    /// Whom the visit is put down to, where the report said.
    pub responsible: Option<Responsible>,
    /// The attempts made so far.
    pub sperrversuche: i32,
    /// The attempts the Festlegung still allows.
    pub verbleibend: i32,
}

impl EventPayload for VersuchGescheitert {
    const TYPE: &'static str = mako_events::sperr::VERSUCH_GESCHEITERT;
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
    order: &crate::sperr::pg::Betroffen,
    reason: &str,
    responsible: Option<Responsible>,
    sperrversuche: i32,
) -> CloudEvent {
    typed(
        tenant,
        &order.malo_id,
        &VersuchGescheitert {
            order_id: id.to_string(),
            external_ref: order.external_ref.clone(),
            malo_id: order.malo_id.clone(),
            grund: reason.to_owned(),
            responsible,
            sperrversuche,
            verbleibend: crate::sperr::pg::MAX_SPERRVERSUCHE - sperrversuche,
        },
    )
}

/// The `data` of `de.mako.sperr.ausfuehrung.ueberfaellig`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AusfuehrungUeberfaellig {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    pub lf_mp_id: String,
    /// The last day of the window that has passed.
    #[schemars(extend("format" = "date"))]
    pub faellig_am: String,
    /// The window, with its source.
    pub frist: String,
}

impl EventPayload for AusfuehrungUeberfaellig {
    const TYPE: &'static str = mako_events::sperr::AUSFUEHRUNG_UEBERFAELLIG;
}

/// A pending order ran past the Kap. 3.5.1.2 Nr. 1 execution window.
///
/// # Errors
///
/// Propagates outbox errors.
pub async fn ausfuehrung_ueberfaellig(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    id: Uuid,
    order: &crate::sperr::pg::Betroffen,
    faellig_am: time::Date,
) -> anyhow::Result<()> {
    let ce = typed(
        tenant,
        &order.malo_id,
        &AusfuehrungUeberfaellig {
            order_id: id.to_string(),
            external_ref: order.external_ref.clone(),
            malo_id: order.malo_id.clone(),
            lf_mp_id: order.lf_mp_id.clone(),
            faellig_am: faellig_am.to_string(),
            frist: "6 WT nach dem frühestmöglichen Sperrtermin \
                    (BK6-24-174 GPKE Teil 2 Kap. 3.5.1.2 Nr. 1)"
                .to_owned(),
        },
    );
    mako_service::outbox::enqueue(&mut *tx, &ce)
        .await
        .map(|_| ())
        .map_err(Into::into)
}

/// What the field team reported on a carried-out order.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AusfuehrungDetail {
    /// `DTM+293` — the instant of the act, RFC 3339.
    #[schemars(extend("format" = "date-time"))]
    pub fertigstellung: Option<String>,
    pub note: Option<String>,
    /// `SG15 STS DE9013` — the EBD Prüfschritt code.
    pub pruefschritt_code: Option<String>,
}

/// The `data` of `de.mako.sperr.ausgefuehrt`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Ausgefuehrt {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub status: OrderStatus,
    /// `SG15 STS DE4405` — `Z14` erfolgreich.
    pub iftsta_code: Option<String>,
    /// Whether the IFTSTA 21039 committed with the outcome; `false` for an
    /// order an operator created by hand, which owes none.
    pub iftsta_queued: bool,
    pub detail: AusfuehrungDetail,
}

impl EventPayload for Ausgefuehrt {
    const TYPE: &'static str = mako_events::sperr::AUSGEFUEHRT;
}

/// What the field team reported on an order that was not carried out.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct FehlschlagDetail {
    pub reason: String,
    /// `SG15 STS DE9013` — the EBD Prüfschritt code.
    pub pruefschritt_code: Option<String>,
    /// Whom the last visit is put down to, where the report said.
    pub responsible: Option<Responsible>,
}

/// The `data` of `de.mako.sperr.fehlgeschlagen`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Fehlgeschlagen {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub status: OrderStatus,
    /// `SG15 STS DE4405` — `Z13` gescheitert.
    pub iftsta_code: Option<String>,
    /// Whether the IFTSTA 21039 committed with the outcome; `false` for an
    /// order an operator created by hand, which owes none.
    pub iftsta_queued: bool,
    pub detail: FehlschlagDetail,
}

impl EventPayload for Fehlgeschlagen {
    const TYPE: &'static str = mako_events::sperr::FEHLGESCHLAGEN;
}

/// The field team reported a terminal outcome — enqueued by
/// [`crate::sperr::pg::report_outcome`] in the transaction that makes the
/// order terminal.
///
/// The subject is the order, not the MaLo: an order id is what every other
/// route in this service takes, and a MaLo can carry several over time.
#[must_use]
pub fn outcome(
    tenant: &str,
    id: Uuid,
    external_ref: Option<&str>,
    outcome: &Outcome<'_>,
    iftsta_queued: bool,
) -> CloudEvent {
    let status = outcome.status();
    let iftsta_code = status.iftsta_code().map(str::to_owned);
    let external_ref = external_ref.map(str::to_owned);
    match outcome {
        Outcome::Executed {
            at,
            note,
            pruefschritt_code,
        } => typed(
            tenant,
            id.to_string(),
            &Ausgefuehrt {
                order_id: id.to_string(),
                external_ref,
                status,
                iftsta_code,
                iftsta_queued,
                detail: AusfuehrungDetail {
                    fertigstellung: rfc3339(*at),
                    note: note.map(str::to_owned),
                    pruefschritt_code: pruefschritt_code.map(str::to_owned),
                },
            },
        ),
        Outcome::Failed {
            reason,
            pruefschritt_code,
            responsible,
            ..
        } => typed(
            tenant,
            id.to_string(),
            &Fehlgeschlagen {
                order_id: id.to_string(),
                external_ref,
                status,
                iftsta_code,
                iftsta_queued,
                detail: FehlschlagDetail {
                    reason: (*reason).to_owned(),
                    pruefschritt_code: pruefschritt_code.map(str::to_owned),
                    responsible: *responsible,
                },
            },
        ),
    }
}

/// The `data` of `de.mako.sperr.ausfuehrung.nach-storno`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AusfuehrungNachStorno {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub order_type: OrderType,
    /// The instant of the act as reported, RFC 3339.
    #[schemars(extend("format" = "date-time"))]
    pub fertigstellung: Option<String>,
    /// When the order was withdrawn, RFC 3339.
    #[schemars(extend("format" = "date-time"))]
    pub cancelled_at: Option<String>,
    pub note: Option<String>,
}

impl EventPayload for AusfuehrungNachStorno {
    const TYPE: &'static str = mako_events::sperr::AUSFUEHRUNG_NACH_STORNO;
}

/// An execution was reported for a withdrawn order — enqueued by
/// [`crate::sperr::pg::report_outcome`] with the report it keeps.
#[must_use]
pub fn ausfuehrung_nach_storno(
    tenant: &str,
    id: Uuid,
    order: &crate::sperr::pg::Betroffen,
    order_type: OrderType,
    fertigstellung: time::OffsetDateTime,
    cancelled_at: Option<time::OffsetDateTime>,
    note: Option<&str>,
) -> CloudEvent {
    typed(
        tenant,
        id.to_string(),
        &AusfuehrungNachStorno {
            order_id: id.to_string(),
            external_ref: order.external_ref.clone(),
            malo_id: order.malo_id.clone(),
            lf_mp_id: order.lf_mp_id.clone(),
            order_type,
            fertigstellung: rfc3339(fertigstellung),
            cancelled_at: cancelled_at.and_then(rfc3339),
            note: note.map(str::to_owned),
        },
    )
}

/// The `data` of `de.mako.sperr.storniert`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Storniert {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub note: String,
}

impl EventPayload for Storniert {
    const TYPE: &'static str = mako_events::sperr::STORNIERT;
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
    order: &crate::sperr::pg::Betroffen,
) -> Result<(), sqlx::Error> {
    let ce = typed(
        tenant,
        &order.malo_id,
        &Storniert {
            order_id: id.to_string(),
            external_ref: order.external_ref.clone(),
            malo_id: order.malo_id.clone(),
            lf_mp_id: order.lf_mp_id.clone(),
            note: "no IFTSTA 21039 is dispatched — a cancelled order was never executed".to_owned(),
        },
    );
    mako_service::outbox::enqueue(tx, &ce).await
}

/// The `data` of `de.mako.sperr.iftsta.ausstehend`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct IftstaAusstehend {
    #[schemars(extend("format" = "uuid"))]
    pub order_id: String,
    /// The executing system's own reference, where the order carries one.
    pub external_ref: Option<String>,
    pub malo_id: String,
    pub lf_mp_id: String,
    /// The attempts actually made, not the cap: an order escalated on its
    /// deadline may still be inside its retry budget.
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: String,
    pub impact: String,
}

impl EventPayload for IftstaAusstehend {
    const TYPE: &'static str = mako_events::sperr::IFTSTA_AUSSTEHEND;
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
    let ce = typed(
        tenant,
        order.id.to_string(),
        &IftstaAusstehend {
            order_id: order.id.to_string(),
            external_ref: order.external_ref.clone(),
            malo_id: order.malo_id.clone(),
            lf_mp_id: order.lf_mp_id.clone(),
            attempts: order.attempts,
            max_attempts: i32::from(mako_service::outbox::OutboxConfig::default().max_attempts),
            last_error: order.last_error.clone(),
            impact: "the Lieferant has not received the Auftragsstatus; their \
                     gpke-sperrung-lf process cannot reach a terminal state"
                .to_owned(),
        },
    );
    mako_service::outbox::enqueue(tx, &ce).await
}

/// The schema of every `de.mako.sperr.*` payload — the content of `events/`.
#[must_use]
pub fn schemas() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        mako_service::cloudevent::event_schema::<AuftragEingegangen>(),
        mako_service::cloudevent::event_schema::<AuftragFaellig>(),
        mako_service::cloudevent::event_schema::<VersuchGescheitert>(),
        mako_service::cloudevent::event_schema::<AusfuehrungUeberfaellig>(),
        mako_service::cloudevent::event_schema::<Ausgefuehrt>(),
        mako_service::cloudevent::event_schema::<Fehlgeschlagen>(),
        mako_service::cloudevent::event_schema::<AusfuehrungNachStorno>(),
        mako_service::cloudevent::event_schema::<Storniert>(),
        mako_service::cloudevent::event_schema::<IftstaAusstehend>(),
    ]
}

#[cfg(test)]
mod tests {
    /// `events/` holds the schema of every payload, and nothing else.
    #[test]
    fn the_committed_schemas_are_current() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("events");
        if let Err(e) = mako_service::cloudevent::compare_event_schemas(&dir, &super::schemas()) {
            panic!(
                "{e}\nBLESS_MCP_SNAPSHOT=1 cargo test -p processd the_committed_schemas rewrites them"
            );
        }
    }

    #[test]
    fn every_emitted_type_is_in_the_catalog() {
        // The catalog is the contract a subscription is written against; a type
        // spelled here and not there is a subscription that never fires.
        for (t, _) in super::schemas() {
            assert!(
                mako_events::all().contains(&t),
                "{t} is not in mako_events::all()"
            );
            assert!(
                mako_events::matches("de.mako.sperr.*", t),
                "{t} does not match the glob a subscriber of the queue names"
            );
        }
    }
}
