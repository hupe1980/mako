//! GeLi Gas / WiM Gas Datenabruf — Gas-specific data requests.
//!
//! Handles inbound ORDERS messages requesting Gas-specific metered values:
//! - Abrechnungsbrennwert and Zustandszahl (PID 17103, Gas billing values)
//! - MSB Gas Anfrage an NB Strom (PID 17104, cross-domain MSB Gas → NB Strom)
//!
//! Corresponding rejection responses: ORDRSP 19103/19104.
//!
//! ## Regulatory basis
//!
//! - **BK7-24-01-009** (GeLi Gas 3.0) — Gas metered value requests
//! - **BDEW GeLi Gas AHB** — ORDERS/ORDRSP for Gas Datenabruf

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    types::{MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the GeLi Gas Datenabruf workflow.
pub const WORKFLOW_NAME: &str = "geli-gas-datenabruf";

/// ORDERS PIDs for Gas-specific data requests (LF/MSB → NB).
///
/// | PID   | Description                                               | Direction      |
/// |-------|-----------------------------------------------------------|----------------|
/// | 17103 | Anfrage zur Übermittlung von Abrechnungsbrennwert/Zahl    | LF → NB/MSB   |
/// | 17104 | Anfrage des MSB Gas an den NB Strom                       | MSB Gas → NB  |
pub const ORDERS_ANFRAGE_PIDS: &[u32] = &[17103, 17104];

/// ORDRSP rejection PIDs for Gas Datenabruf (NB → LF/MSB).
///
/// | PID   | Description                                    |
/// |-------|------------------------------------------------|
/// | 19103 | Ablehnung Brennwert und Zustandszahl           |
/// | 19104 | Ablehnung Anfrage vom MSB Gas                  |
pub const ORDRSP_ABLEHNUNG_PIDS: &[u32] = &[19103, 19104];

/// Deadline label for the Gas data-request response window.
///
/// On a 17103 it is due at [`mako_fristen::antwort::brennwert_zustandszahl_due`]:
/// „Die Übermittlung erfolgt spätestens 10 Werktage nach Eingang der Anfrage
/// des Lieferanten, aber nicht vor dem 10. Werktag des Monats der auf den
/// Monat folgt, in den das Ende der durch den Lieferanten zuvor mitgeteilten
/// Zeitspanne fällt" (BK7-24-01-009 GeLi Gas 3.0 Kap. 4.2, S. 13) — the later
/// of the two bounds, so a request about the running or the previous month is
/// due on the 10. Werktag of the month after the Zeitspanne. Both sides
/// register it: the LF from the instant it sends, the NB from the instant the
/// Anfrage arrives.
///
/// On a 17104 (Anfrage des MSB Gas an den NB Strom) it is the Geschäfts­
/// datenanfrage window of GPKE Teil 4 Kap. 3.2 Nr. 2 (S. 30) — „spätester ÜZ
/// ist 1 WT nach dem ÜZ von Nr. 1"
/// ([`mako_fristen::antwort::GESCHAEFTSDATENANFRAGE_WERKTAGE`]); the PID
/// overview 4.0 files 17104 under that Use-Case.
pub const ANTWORT_WINDOW_LABEL: &str = "geli-gas-datenabruf-antwort";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas Datenabruf workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeliGasDatenabrufEvent {
    /// ORDERS data request sent: `InitiateAnfrage` queues the outbound
    /// ORDERS beside this event.
    AnfrageGesendet {
        /// BDEW Prüfidentifikator of the ORDERS sent (17103 or 17104).
        pid: Pruefidentifikator,
        /// Sender (our own LF or MSB Gas).
        sender: MarktpartnerCode,
        /// Receiver (NB or MSB Strom).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference of the ORDERS sent.
        message_ref: MessageRef,
    },
    /// ORDERS data request received.
    AnfrageErhalten {
        /// BDEW Prüfidentifikator (17103 or 17104).
        pid: Pruefidentifikator,
        /// Sender (LF or MSB Gas).
        sender: MarktpartnerCode,
        /// Receiver (NB or MSB Strom).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// ORDRSP rejection received (NB → LF/MSB).
    AbgelehntErhalten {
        /// BDEW Prüfidentifikator (19103 or 19104).
        pid: Pruefidentifikator,
        /// Sender (NB or MSB Strom).
        sender: MarktpartnerCode,
        /// Message reference of the rejection.
        message_ref: MessageRef,
    },
    /// MSCONS data delivery confirmed — marks the Datenabruf process as complete.
    ///
    /// Emitted by `NotifyDatenGeliefert` when the MSCONS carrier arrives and the
    /// adapter signals that the requested data has been delivered. Transitions the
    /// process from `AnfrageGesendet` to the terminal `DatenErhalten` state.
    DatenGeliefert,
    /// Deadline expired without a response.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: mako_engine::ids::DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for GeliGasDatenabrufEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "GeliGasDatenabrufAnfrageGesendet",
            Self::AnfrageErhalten { .. } => "GeliGasDatenabrufAnfrageErhalten",
            Self::AbgelehntErhalten { .. } => "GeliGasDatenabrufAbgelehnt",
            Self::DatenGeliefert => "GeliGasDatenabrufDatenGeliefert",
            Self::DeadlineExpired { .. } => "GeliGasDatenabrufDeadlineExpired",
        }
    }

    /// The ORDERS out or in and the ORDRSP Ablehnung in are messages.
    /// `DatenGeliefert` is the adapter's signal that an MSCONS arrived; it
    /// carries neither PID nor reference, and the MSCONS itself is recorded
    /// by the process that received it.
    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnfrageGesendet {
                pid, message_ref, ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AnfrageErhalten {
                pid, message_ref, ..
            }
            | Self::AbgelehntErhalten {
                pid, message_ref, ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::DatenGeliefert | Self::DeadlineExpired { .. } => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// State of a GeLi Gas Datenabruf process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GeliGasDatenabrufState {
    /// No events yet.
    #[default]
    New,
    /// ORDERS sent or received; waiting for the NB's answer
    /// ([`ANTWORT_WINDOW_LABEL`]).
    AnfrageGesendet {
        /// PID of the initial ORDERS.
        pid: Pruefidentifikator,
        /// Sender of the ORDERS.
        sender: MarktpartnerCode,
    },
    /// Positive data delivery received (via MSCONS, no ORDRSP).
    DatenErhalten,
    /// NB/MSB rejected the request (ORDRSP 19103/19104).
    Abgelehnt,
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// No Festlegung states a consequence of the Datenabruf going unanswered. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<GeliGasDatenabrufState>,
    },
}

/// Whether a Datenabruf still holds its location: live while the request
/// waits, settled once the data are delivered or the request refused.
impl mako_engine::workflow::OccupiesBusinessKey for GeliGasDatenabrufState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // A lapsed window leaves the request open.
            Self::FristAbgelaufen { offen, .. } => offen.occupies_business_key(),
            Self::AnfrageGesendet { .. } => true,
            Self::New | Self::DatenErhalten | Self::Abgelehnt => false,
        }
    }
}

impl GeliGasDatenabrufState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageGesendet { .. } => "AnfrageGesendet",
            Self::DatenErhalten => "DatenErhalten",
            Self::Abgelehnt => "Abgelehnt",
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
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas Datenabruf workflow.
#[derive(Clone)]
pub enum GeliGasDatenabrufCommand {
    /// ERP instructs the LF to initiate a Gas data request (ORDERS 17103/17104 outbound).
    ///
    /// Spawns a new `GeliGasDatenabrufWorkflow` and enqueues the outbound
    /// ORDERS message; the door registers the NB's answer window
    /// ([`mako_fristen::antwort::brennwert_zustandszahl_due`]).
    ///
    /// Use PID 17103 to request Abrechnungsbrennwert + Zustandszahl.
    InitiateAnfrage {
        /// ORDERS PID (17103 = Brennwert/Zustandszahl, 17104 = MSB Gas → NB Strom).
        pid: Pruefidentifikator,
        /// Our own GLN (the Lieferant).
        sender: MarktpartnerCode,
        /// GNB / NB GLN (resolved from MaLo cache).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference (generated at dispatch time).
        message_ref: MessageRef,
        /// Messlokation the Brennwert is asked for — the ORDERS `LOC+172`
        /// carries „die ID der Messlokation" (ORDERS AHB 1.1b Kap. 4.1.3).
        messlokation: MeLo,
        /// First Gastag of the Zeitspanne the Brennwert is asked for
        /// (`SG29 DTM+163`).
        zeitspanne_von: time::Date,
        /// Last Gastag of the Zeitspanne (`SG29 DTM+164` is the beginning of
        /// the Gastag after it) — it sets the earliest day the NB may answer
        /// (GeLi Gas 3.0 Kap. 4.2).
        zeitspanne_bis: time::Date,
    },
    /// Inbound ORDERS data request received.
    ReceiveAnfrage {
        /// BDEW Prüfidentifikator (17103 or 17104).
        pid: Pruefidentifikator,
        /// Sender (LF or MSB Gas).
        sender: MarktpartnerCode,
        /// Receiver (NB or MSB Strom).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP rejection received.
    ReceiveAblehnung {
        /// Rejection PID (19103 or 19104).
        pid: Pruefidentifikator,
        /// Sender of the rejection.
        sender: MarktpartnerCode,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Data successfully delivered via MSCONS — marks process as complete.
    NotifyDatenGeliefert,
    /// Deadline expired.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: mako_engine::ids::DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for GeliGasDatenabrufCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas Datenabruf workflow — handles Gas-specific data requests.
pub struct GeliGasDatenabrufWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while the Anfrage waits for its answer
/// (`AnfrageGesendet`).
fn window_open(label: &str, state: &GeliGasDatenabrufState) -> bool {
    label == ANTWORT_WINDOW_LABEL && matches!(state, GeliGasDatenabrufState::AnfrageGesendet { .. })
}

impl Workflow for GeliGasDatenabrufWorkflow {
    type State = GeliGasDatenabrufState;
    type Event = GeliGasDatenabrufEvent;
    type Command = GeliGasDatenabrufCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| GeliGasDatenabrufCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            GeliGasDatenabrufState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            GeliGasDatenabrufEvent::AnfrageGesendet { pid, sender, .. }
            | GeliGasDatenabrufEvent::AnfrageErhalten { pid, sender, .. } => {
                if matches!(state, GeliGasDatenabrufState::New) {
                    GeliGasDatenabrufState::AnfrageGesendet {
                        pid: *pid,
                        sender: sender.clone(),
                    }
                } else {
                    state
                }
            }
            GeliGasDatenabrufEvent::AbgelehntErhalten { .. } => GeliGasDatenabrufState::Abgelehnt,
            GeliGasDatenabrufEvent::DatenGeliefert => GeliGasDatenabrufState::DatenErhalten,
            GeliGasDatenabrufEvent::DeadlineExpired { label, .. } => match state {
                GeliGasDatenabrufState::AnfrageGesendet { .. } => {
                    GeliGasDatenabrufState::FristAbgelaufen {
                        label: label.to_string(),
                        offen: Box::new(state),
                    }
                }
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        cmd: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // A lapsed window changes nothing about what the process accepts; only
        // the timeout itself asks about the lapse.
        let lapsed = state;
        let state = state.offen();
        match cmd {
            GeliGasDatenabrufCommand::InitiateAnfrage {
                pid,
                sender,
                receiver,
                message_ref,
                messlokation,
                zeitspanne_von,
                zeitspanne_bis,
            } => {
                if zeitspanne_von > zeitspanne_bis {
                    return Err(WorkflowError::rejected(format!(
                        "the Zeitspanne begins {zeitspanne_von} after it ends {zeitspanne_bis}",
                    )));
                }
                if !ORDERS_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas Datenabruf ORDERS PID ({ORDERS_ANFRAGE_PIDS:?}), got {pid}",
                    )));
                }
                if !matches!(state, GeliGasDatenabrufState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let event = GeliGasDatenabrufEvent::AnfrageGesendet {
                    pid,
                    sender: sender.clone(),
                    receiver: receiver.clone(),
                    message_ref: message_ref.clone(),
                };
                let outbox = mako_engine::outbox::PendingOutbox::new(
                    "ORDERS",
                    receiver.as_str(),
                    serde_json::json!({
                        "direction":  "outbound",
                        "pid":        pid.as_u32(),
                        "sender":     sender.as_str(),
                        "receiver":   receiver.as_str(),
                        "message_ref": message_ref.as_str(),
                        "location":   messlokation.as_str(),
                        "zeitspanne_von": zeitspanne_von.to_string(),
                        "zeitspanne_bis": zeitspanne_bis.to_string(),
                    }),
                );
                Ok(mako_engine::workflow::WorkflowOutput::with_outbox(
                    vec![event],
                    vec![outbox],
                ))
            }
            GeliGasDatenabrufCommand::ReceiveAnfrage {
                pid,
                sender,
                receiver,
                message_ref,
            } => {
                if !ORDERS_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas Datenabruf ORDERS PID ({ORDERS_ANFRAGE_PIDS:?}), got {pid}",
                    )));
                }
                if !matches!(state, GeliGasDatenabrufState::New) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GeliGasDatenabrufEvent::AnfrageErhalten {
                    pid,
                    sender,
                    receiver,
                    message_ref,
                }]
                .into())
            }
            GeliGasDatenabrufCommand::ReceiveAblehnung {
                pid,
                sender,
                message_ref,
            } => {
                if !ORDRSP_ABLEHNUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas Datenabruf ORDRSP rejection PID ({ORDRSP_ABLEHNUNG_PIDS:?}), got {pid}",
                    )));
                }
                if !matches!(state, GeliGasDatenabrufState::AnfrageGesendet { .. }) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GeliGasDatenabrufEvent::AbgelehntErhalten {
                    pid,
                    sender,
                    message_ref,
                }]
                .into())
            }
            GeliGasDatenabrufCommand::NotifyDatenGeliefert => {
                // Data was delivered via MSCONS — transition to DatenErhalten.
                if matches!(state, GeliGasDatenabrufState::AnfrageGesendet { .. }) {
                    Ok(vec![GeliGasDatenabrufEvent::DatenGeliefert].into())
                } else {
                    // Already resolved (Abgelehnt, DatenErhalten, DeadlineExpired) — no-op.
                    Ok(WorkflowOutput::events(vec![]))
                }
            }
            GeliGasDatenabrufCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GeliGasDatenabrufEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).unwrap()
    }

    /// A Zeitspanne that ends before it begins names no period the GNB could
    /// answer for.
    #[test]
    fn a_zeitspanne_ending_before_it_begins_is_refused() {
        let refused = GeliGasDatenabrufWorkflow::handle(
            &GeliGasDatenabrufState::New,
            GeliGasDatenabrufCommand::InitiateAnfrage {
                pid: pid(17_103),
                sender: MarktpartnerCode::new("9800000000004"),
                receiver: MarktpartnerCode::new("9800000000011"),
                message_ref: MessageRef::new("ORD-1"),
                messlokation: MeLo::new("DE00056266802AO6G56M11SN51G21M24S"),
                zeitspanne_von: time::Date::from_calendar_date(2026, time::Month::October, 31)
                    .unwrap(),
                zeitspanne_bis: time::Date::from_calendar_date(2026, time::Month::October, 1)
                    .unwrap(),
            },
        );
        assert!(refused.is_err(), "{:?}", refused.map(|o| o.events.len()));
    }

    #[test]
    fn verlauf_states_the_orders_sent_and_the_ordrsp_received() {
        let out = GeliGasDatenabrufWorkflow::handle(
            &GeliGasDatenabrufState::New,
            GeliGasDatenabrufCommand::InitiateAnfrage {
                pid: pid(17_103),
                sender: MarktpartnerCode::new("9800000000004"),
                receiver: MarktpartnerCode::new("9800000000011"),
                message_ref: MessageRef::new("ORD-1"),
                messlokation: MeLo::new("DE00056266802AO6G56M11SN51G21M24S"),
                zeitspanne_von: time::Date::from_calendar_date(2026, time::Month::October, 1)
                    .unwrap(),
                zeitspanne_bis: time::Date::from_calendar_date(2026, time::Month::October, 31)
                    .unwrap(),
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["pid"], 17_103);
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(17_103),
                message_ref: Some("ORD-1".to_owned()),
            }),
            "InitiateAnfrage queues the ORDERS: it is sent, not received"
        );
        let state = out.events.iter().fold(
            GeliGasDatenabrufState::New,
            GeliGasDatenabrufWorkflow::apply,
        );
        assert!(matches!(
            state,
            GeliGasDatenabrufState::AnfrageGesendet { .. }
        ));

        let out = GeliGasDatenabrufWorkflow::handle(
            &state,
            GeliGasDatenabrufCommand::ReceiveAblehnung {
                pid: pid(19_103),
                sender: MarktpartnerCode::new("9800000000011"),
                message_ref: MessageRef::new("RSP-1"),
            },
        )
        .unwrap();
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(19_103),
                message_ref: Some("RSP-1".to_owned()),
            })
        );

        let out = GeliGasDatenabrufWorkflow::handle(
            &GeliGasDatenabrufState::New,
            GeliGasDatenabrufCommand::ReceiveAnfrage {
                pid: pid(17_104),
                sender: MarktpartnerCode::new("9800000000004"),
                receiver: MarktpartnerCode::new("9900357000004"),
                message_ref: MessageRef::new("ORD-2"),
            },
        )
        .unwrap();
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(17_104),
                message_ref: Some("ORD-2".to_owned()),
            })
        );
        assert_eq!(GeliGasDatenabrufEvent::DatenGeliefert.verlauf(), None);
    }
}
