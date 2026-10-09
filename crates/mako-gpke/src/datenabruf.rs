//! GPKE Datenabruf — LF requests data values / reclamations from NB or MSB.
//!
//! This module handles the LF-initiated data-request and reclamation processes
//! defined in **GPKE Teil 4** (BK6-22-024 Anlage 1d, Kap. 3
//! „Geschäftsdatenanfrage"):
//!
//! | PID   | Direction   | Description |
//! |-------|-------------|-------------|
//! | 17004 | LF → NB/MSB | Anforderung von Werten |
//! | 17102 | LF → NB/MSB | Anfrage von Werten (data value request) |
//! | 17113 | LF → NB/MSB | Reklamation von Werten/Lastgängen |
//! | 19101 | NB → LF     | Ablehnung Anfrage Stammdaten |
//! | 19102 | NB/MSB → LF | Ablehnung Anfrage Werte |
//! | 19114 | NB/MSB → LF | Ablehnung der Reklamation von Werten |
//!
//! Positive responses to data requests arrive via **MSCONS** (handled by
//! [`crate::messwerte`]); only rejections (ORDRSP 19101/19102/19114) are handled here.
//!
//! # Regulatory basis
//!
//! - **GPKE Teil 4 § 3.2** (BK6-22-024 Anlage 1d) — „Unverzüglich, jedoch
//!   spätester ÜZ ist 1 WT nach dem ÜZ von Nr. 1"
//! - APERAK Frist: 45 Minuten für ORDERS (APERAK AHB 1.1 § 2.4.1)

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name for the GPKE Datenabruf process.
pub const WORKFLOW_NAME: &str = "gpke-datenabruf";

/// ORDERS PIDs sent by LF to request or reclaim data.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17004 | Anforderung von Werten (LF → NB/MSB) |
/// | 17102 | Anfrage von Werten (LF → NB/MSB) |
/// | 17113 | Reklamation von Werten/Lastgängen (LF → NB/MSB) |
pub const ORDERS_ANFRAGE_PIDS: &[u32] = &[17004, 17102, 17113];

/// ORDRSP rejection PIDs received by LF from NB or MSB.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 19101 | Ablehnung Anfrage Stammdaten (NB → LF) |
/// | 19102 | Ablehnung Anfrage Werte (NB/MSB → LF) |
/// | 19114 | Ablehnung der Reklamation von Werten (NB/MSB → LF) |
pub const ORDRSP_ABLEHNUNG_PIDS: &[u32] = &[19101, 19102, 19114];

/// Deadline label for the answer window on an inbound Geschäftsdatenanfrage.
///
/// **1 Werktag** — „Unverzüglich, jedoch spätester ÜZ ist 1 WT nach dem ÜZ von
/// Nr. 1" (GPKE Teil 4 § 3.2 Prozessschritte 2 und 4), which
/// [`mako_fristen::antwort::GESCHAEFTSDATENANFRAGE_WERKTAGE`] holds. Not 24
/// wall-clock hours, and not a GPKE Teil 2 window: the Geschäftsdatenanfrage
/// lives in Teil 4.
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-datenabruf-antwort";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Datenabruf workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DatenabrufEvent {
    /// LF sent an ORDERS data request or reclamation.
    AnfrageGesendet {
        /// ORDERS Prüfidentifikator (17102 or 17113).
        orders_pid: Pruefidentifikator,
        /// Recipient GLN (NB or MSB).
        recipient: MarktpartnerCode,
        /// Affected MaLo (if known at request time).
        malo: Option<MaLo>,
        /// Message reference of the sent ORDERS.
        message_ref: MessageRef,
    },
    /// NB or MSB rejected the data request (ORDRSP 19101/19102/19114).
    AnfrageAbgelehnt {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason from the ORDRSP.
        reason: Option<String>,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Data delivered via MSCONS (informational event; actual data in messwerte workflow).
    DatenGeliefert {
        /// Message reference of the triggering MSCONS notification.
        message_ref: MessageRef,
    },
    /// Deadline expired before the response arrived.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for DatenabrufEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "DatenabrufAnfrageGesendet",
            Self::AnfrageAbgelehnt { .. } => "DatenabrufAnfrageAbgelehnt",
            Self::DatenGeliefert { .. } => "DatenabrufDatenGeliefert",
            Self::DeadlineExpired { .. } => "DatenabrufDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Data captured when the ORDERS request is sent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnfrageData {
    /// ORDERS PID sent.
    pub orders_pid: Pruefidentifikator,
    /// Recipient GLN.
    pub recipient: MarktpartnerCode,
    /// Affected MaLo (optional).
    pub malo: Option<MaLo>,
    /// Message reference.
    pub message_ref: MessageRef,
}

/// Current state of a GPKE Datenabruf process stream.
///
/// ```text
/// New → AnfrageGesendet → DatenErhalten
///                      ↘ Abgelehnt
///                      ↘ FristAbgelaufen → DatenErhalten | Abgelehnt
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum DatenabrufState {
    /// No request sent yet.
    #[default]
    New,
    /// LF sent the ORDERS; awaiting ORDRSP or MSCONS delivery.
    AnfrageGesendet(AnfrageData),
    /// NB/MSB rejected the request (ORDRSP 19101/19102/19114).
    Abgelehnt {
        /// Original request.
        anfrage: AnfrageData,
        /// Rejection ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
    },
    /// Data was delivered (MSCONS arrived; positive outcome).
    DatenErhalten {
        /// Original request.
        anfrage: AnfrageData,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// No Festlegung states a consequence of the NB or MSB leaving the Anfrage unanswered. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<DatenabrufState>,
    },
}

impl DatenabrufState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageGesendet(_) => "AnfrageGesendet",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::DatenErhalten { .. } => "DatenErhalten",
            Self::FristAbgelaufen { .. } => "FristAbgelaufen",
        }
    }
    /// The state the process is waiting in — itself, unless a window lapsed.
    #[must_use]
    pub fn offen(&self) -> &Self {
        match self {
            Self::FristAbgelaufen { offen, .. } => offen.offen(),
            other => other,
        }
    }

    /// Whether the process is terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Abgelehnt { .. } | Self::DatenErhalten { .. })
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Datenabruf workflow.
#[derive(Clone)]
pub enum DatenabrufCommand {
    /// LF sends a data-value request or reclamation.
    SendAnfrage {
        /// ORDERS PID (17102 or 17113).
        orders_pid: Pruefidentifikator,
        /// Recipient GLN.
        recipient: MarktpartnerCode,
        /// Affected MaLo (optional).
        malo: Option<MaLo>,
        /// Message reference of the outbound ORDERS.
        message_ref: MessageRef,
        /// ORDERS payload.
        payload: serde_json::Value,
        /// When the ORDERS leaves — the Übertragungstag the counterparty's
        /// Antwortfrist counts from ([`mako_fristen::antwort::antwort_deadline`]).
        gesendet_am: time::OffsetDateTime,
    },
    /// Inbound ORDRSP rejection received.
    ReceiveAblehnung {
        /// ORDRSP Prüfidentifikator (19101/19102/19114).
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// MSCONS data arrived (positive response via messwerte workflow).
    NotifyDatenGeliefert {
        /// Message reference of the MSCONS trigger.
        message_ref: MessageRef,
    },
    /// Deadline expired.
    TimeoutExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for DatenabrufCommand {}

/// The counterparty's Antwortfrist on an outbound Datenabruf ORDERS.
///
/// Only the Anfrage von Werten (17102) is a Geschäftsdatenanfrage of GPKE
/// Teil 4 § 3.2, answered „spätester ÜZ ist 1 WT nach dem ÜZ von Nr. 3"
/// ([`mako_fristen::antwort::GESCHAEFTSDATENANFRAGE_WERKTAGE`]). The
/// Anforderung (17004) and the Reklamation von Werten (17113) are WiM Teil 2
/// Use-Cases, which § 3.1 excludes, and register no window here.
fn antwortfrist(
    orders_pid: u32,
    gesendet_am: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    (orders_pid == 17_102).then(|| {
        mako_fristen::antwort::FristShape::EndOfWerktag(
            mako_fristen::antwort::GESCHAEFTSDATENANFRAGE_WERKTAGE,
        )
        .due_at(gesendet_am, mako_fristen::HolidayCalendar::BdewMaKo)
    })
}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Datenabruf workflow — LF-initiated data requests and reclamations.
pub struct GpkeDatenabrufWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while the Anfrage waits for its answer
/// (`AnfrageGesendet`).
fn window_open(label: &str, state: &DatenabrufState) -> bool {
    label == ANTWORT_WINDOW_LABEL && matches!(state, DatenabrufState::AnfrageGesendet(_))
}

impl Workflow for GpkeDatenabrufWorkflow {
    type State = DatenabrufState;
    type Event = DatenabrufEvent;
    type Command = DatenabrufCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| DatenabrufCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            DatenabrufState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            DatenabrufEvent::AnfrageGesendet {
                orders_pid,
                recipient,
                malo,
                message_ref,
            } => DatenabrufState::AnfrageGesendet(AnfrageData {
                orders_pid: *orders_pid,
                recipient: recipient.clone(),
                malo: malo.clone(),
                message_ref: message_ref.clone(),
            }),
            DatenabrufEvent::AnfrageAbgelehnt {
                ordrsp_pid, reason, ..
            } => match state {
                DatenabrufState::AnfrageGesendet(anfrage) => DatenabrufState::Abgelehnt {
                    anfrage,
                    ordrsp_pid: *ordrsp_pid,
                    reason: reason.clone(),
                },
                other => other,
            },
            DatenabrufEvent::DatenGeliefert { .. } => match state {
                DatenabrufState::AnfrageGesendet(anfrage) => {
                    DatenabrufState::DatenErhalten { anfrage }
                }
                other => other,
            },
            DatenabrufEvent::DeadlineExpired { label, .. } => match state {
                DatenabrufState::AnfrageGesendet(_) => DatenabrufState::FristAbgelaufen {
                    label: label.to_string(),
                    offen: Box::new(state),
                },
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // A lapsed window changes nothing about what the process accepts; only
        // the timeout itself asks about the lapse.
        let lapsed = state;
        let state = state.offen();
        match command {
            DatenabrufCommand::SendAnfrage {
                orders_pid,
                recipient,
                malo,
                message_ref,
                payload,
                gesendet_am,
            } => {
                if !matches!(state, DatenabrufState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_ANFRAGE_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid Datenabruf ORDERS PID: {orders_pid}",
                    )));
                }
                let event = DatenabrufEvent::AnfrageGesendet {
                    orders_pid,
                    recipient: recipient.clone(),
                    malo: malo.clone(),
                    message_ref: message_ref.clone(),
                };
                let outbox = vec![PendingOutbox::new(
                    "ORDERS",
                    recipient.as_str(),
                    serde_json::json!({
                        "pid":        orders_pid.as_u32(),
                        "malo":       malo.as_ref().map(|m| m.as_str()),
                        "orders_ref": message_ref.as_str(),
                        "payload":    payload,
                    }),
                )];
                // The counterparty's Antwortfrist, from the published row for
                // this ORDERS; a PID no Festlegung quantifies registers nothing.
                let frist: Vec<PendingDeadline> = antwortfrist(orders_pid.as_u32(), gesendet_am)
                    .map(|due| PendingDeadline::new(ANTWORT_WINDOW_LABEL, due))
                    .into_iter()
                    .collect();
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    vec![event],
                    outbox,
                    frist,
                ))
            }

            DatenabrufCommand::ReceiveAblehnung {
                ordrsp_pid,
                reason,
                message_ref,
            } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !matches!(state, DatenabrufState::AnfrageGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageGesendet",
                        state.label(),
                    ));
                }
                if !ORDRSP_ABLEHNUNG_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid Datenabruf ORDRSP rejection PID: {ordrsp_pid}",
                    )));
                }
                Ok(vec![DatenabrufEvent::AnfrageAbgelehnt {
                    ordrsp_pid,
                    reason,
                    message_ref,
                }]
                .into())
            }

            DatenabrufCommand::NotifyDatenGeliefert { message_ref } => {
                if !matches!(state, DatenabrufState::AnfrageGesendet(_)) {
                    return Ok(WorkflowOutput::events(vec![])); // idempotent
                }
                Ok(vec![DatenabrufEvent::DatenGeliefert { message_ref }].into())
            }

            DatenabrufCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![DatenabrufEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anfrage(pid: u32, gesendet_am: time::OffsetDateTime) -> DatenabrufCommand {
        DatenabrufCommand::SendAnfrage {
            orders_pid: Pruefidentifikator::new(pid).expect("valid PID"),
            recipient: MarktpartnerCode::new("9900000000003"),
            malo: None,
            message_ref: MessageRef::new("ORDERS-1"),
            payload: serde_json::json!({}),
            gesendet_am,
        }
    }

    /// The Anfrage von Werten is a Geschäftsdatenanfrage (GPKE Teil 4 Kap.
    /// 3.2): its answer is due by the end of the next Werktag. The WiM
    /// Anforderung starts no window here.
    #[test]
    fn an_anfrage_von_werten_registers_the_one_werktag_window() {
        // Mittwoch, 07.10.2026 → Ablauf Donnerstag, 08.10.2026.
        let sent = time::macros::datetime!(2026-10-07 10:00 UTC);
        let out =
            GpkeDatenabrufWorkflow::handle(&DatenabrufState::default(), anfrage(17_102, sent))
                .expect("accepted");
        assert_eq!(out.deadlines.len(), 1);
        assert_eq!(out.deadlines[0].label, ANTWORT_WINDOW_LABEL);
        assert_eq!(
            out.deadlines[0].due_at,
            mako_fristen::end_of_day_berlin(time::macros::date!(2026 - 10 - 08))
        );

        let out =
            GpkeDatenabrufWorkflow::handle(&DatenabrufState::default(), anfrage(17_004, sent))
                .expect("accepted");
        assert!(out.deadlines.is_empty());
    }
}
