//! WiM Preisanfrage / Angebot — REQOTE/QUOTES price-quote exchange for
//! Messstellenbetreiber device services.
//!
//! Covers the REQOTE/QUOTES process per BDEW WiM documentation:
//! the new MSB (nMSB) requests pricing for device services from the
//! existing MSB, and the existing MSB responds with an Angebot (quote).
//!
//! This module implements both sides of the exchange:
//! - **Anfragend** (MSBN for 35001, LF for 35002, NB/LF for 35004/35005):
//!   `SendReqote` sends the REQOTE (`AnfrageGesendet`) and `ReceiveAngebot`
//!   records the QUOTES that answers it (`AngebotErhalten`).
//! - **Anbietend** (MSBA / MSB): `ReceiveReqote` records the REQOTE
//!   (`AnfrageErhalten`) and `SendAngebot` the QUOTES answer
//!   (`AngebotGesendet`).
//!
//! The two sides never share a state: `Angefragt` is reached only by sending,
//! `Eingegangen`/`ValidationPassed` only by receiving, so a requester cannot
//! answer its own Anfrage and a responder cannot record an Angebot it owes.
//!
//! # Prüfidentifikatoren (REQOTE AHB 1.2 / QUOTES AHB 1.1a)
//!
//! ## Inbound REQOTE
//!
//! The four are *not* one direction — each belongs to a different process.
//!
//! | PID   | Process name (AHB)                                      | Direction     | Prozess |
//! |-------|---------------------------------------------------------|---------------|---------|
//! | 35001 | Anfrage Geräteübernahmeangebot                          | MSBN → MSBA   | WiM Strom Teil 1 |
//! | 35002 | Anfrage Rechnungsabwicklung MSB über LF                  | **LF → MSB**  | WiM Strom Teil 1 |
//! | 35004 | Anfrage einer Konfiguration                             | NB / LF → MSB | GPKE Teil 3 |
//! | 35005 | Anfrage Angebot Änderung Technik                        | NB / LF → MSB | AWH Änderung der Technik |
//!
//! **35003 is not in this set.** It is the ESA "Anfrage von Werten" (ESA → MSB,
//! WiM Strom Teil 2) and belongs to [`crate::wertebestellung`].
//!
//! ## Outbound QUOTES (response)
//!
//! | PID   | Process name (AHB)                              | Derived from |
//! |-------|-------------------------------------------------|--------------|
//! | 15001 | Angebot Geräteübernahme                         | 35001        |
//! | 15002 | Angebot Abrechnung Messstellenbetrieb MSB       | 35002        |
//! | 15004 | Angebot einer Konfiguration                     | 35004        |
//! | 15005 | Angebot Änderung Technik                        | 35005        |
//!
//! # Regulatory basis
//!
//! - **BDEW WiM** — Wechselprozesse im Messwesen Strom
//! - **REQOTE AHB 1.2** / **QUOTES AHB 1.1a** — EDI@Energy message format
//! - **APERAK AHB 1.1** — the technical acknowledgement, on its own clock

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-preisanfrage";

/// Inbound REQOTE Prüfidentifikatoren handled by [`WimPreisanfrageWorkflow`].
///
/// | PID   | Process (AHB)                                | AHB version |
/// |-------|----------------------------------------------|-------------|
/// | 35001 | Anfrage Geräteübernahmeangebot (MSBN → MSBA) | 1.2 ✅      |
/// | 35002 | Anfrage Rechnungsabwicklung MSB über LF       | 1.2 ✅      |
/// | 35004 | Anfrage einer Konfiguration                  | 1.2 ✅      |
/// | 35005 | Anfrage Angebot Änderung Technik             | 1.2 ✅      |
///
/// 35003 is deliberately absent: REQOTE AHB 1.1 Kap. 4.3 assigns it to the ESA
/// "Anfrage von Werten" (WiM Teil 2), which `wertebestellung` owns.
pub const REQOTE_PIDS: &[u32] = &[35001, 35002, 35004, 35005];

/// Inbound QUOTES Prüfidentifikatoren (responses received by the nMSB side).
///
/// | PID   | Process (AHB)                                | AHB version |
/// |-------|----------------------------------------------|-------------|
/// | 15001 | Angebot Geräteübernahme                      | 1.1a ✅     |
/// | 15002 | Angebot Abrechnung Messstellenbetrieb MSB    | 1.1a ✅     |
/// | 15004 | Angebot einer Konfiguration                  | 1.1a ✅     |
/// | 15005 | Angebot Änderung Technik                     | 1.1a ✅     |
///
/// 15003 (Angebot zur Anfrage von Werten, MSB → ESA) answers 35003 and belongs
/// to [`crate::wertebestellung`], not here.
pub const QUOTES_PIDS: &[u32] = &[15001, 15002, 15004, 15005];

/// The answer window for a REQOTE Preisanfrage PID, in Werktage.
///
/// A view on [`mako_fristen::antwort::WIM`], which is the same table `makod`
/// registers the process deadline from and `processd` sizes its operator queue
/// by — two literals for one Frist is how the two come to disagree about
/// whether an obligation was met.
///
/// **There is no single Preisanfrage window.** The four REQOTE PIDs open four
/// different Use-Cases and WiM Strom Teil 1 gives three of them different
/// numbers:
///
/// | PID | Use-Case | Frist | Fundstelle |
/// |---|---|---|---|
/// | 35001 | Anforderung Geräteübernahmeangebot | **4 WT** | Kap. 3.2.2 Nr. 2 |
/// | 35002 | Anfrage Rechnungsabwicklung über den LF | **5 WT** | Kap. 3.6.3.6.2 Nr. 2 |
/// | 35005 | Anfrage Angebot Änderung Technik | **10 WT** | Kap. 3.3.1.2 Nr. 2 |
/// | 35004 | Anfrage einer Konfiguration (GPKE Teil 3) | *unquantified* | — |
///
/// A flat window is wrong in both directions: it gives the
/// Geräteübernahmeangebot a Werktag it does not have and lets a Technikänderung
/// run past its window unflagged.
///
/// `None` means the window is **unknown**, never unbounded: for 35004 because
/// GPKE Teil 3 states it elsewhere, and for **35003** because that is the ESA
/// „Anfrage von Werten", which `crate::wertebestellung` owns and which must
/// never be answered with a PreisblattMessung quote.
#[must_use]
pub fn antwort_frist_werktage(request_pid: u32) -> Option<u32> {
    use mako_fristen::antwort::FristShape;
    if !REQOTE_PIDS.contains(&request_pid) {
        return None;
    }
    match mako_fristen::antwort::antwort_obligation(request_pid)?.frist {
        FristShape::EndOfWerktag(n) => Some(n),
        _ => None,
    }
}

/// Deadline label for the response window — the per-PID window of
/// [`antwort_frist_werktage`].
pub const PREISANFRAGE_DEADLINE_LABEL: &str = "wim-preisanfrage-antwort";

// ── Response PID derivation ───────────────────────────────────────────────────

/// Derive the outbound QUOTES response PID for a given REQOTE anfrage PID.
///
/// | REQOTE | QUOTES response |
/// |--------|-----------------|
/// | 35001  | 15001           |
/// | 35002  | 15002           |
/// | 35004  | 15004           |
/// | 35005  | 15005           |
fn quotes_response_pid(reqote_pid: u32) -> Option<Pruefidentifikator> {
    let code: u32 = match reqote_pid {
        35001 => 15001,
        35002 => 15002,
        35004 => 15004,
        35005 => 15005,
        _ => return None,
    };
    Pruefidentifikator::new(code).ok()
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Preisanfrage workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PreisanfrageEvent {
    /// This party sent the REQOTE Anfrage (requester perspective).
    AnfrageGesendet {
        /// MP-ID of this party, the requester.
        sender: MarktpartnerCode,
        /// MP-ID of the MSB asked for the Angebot.
        receiver: MarktpartnerCode,
        /// EDIFACT message reference of the outbound REQOTE.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (35001/35002/35004/35005).
        pruefidentifikator: Pruefidentifikator,
    },
    /// A REQOTE Anfrage arrived (responding MSB perspective).
    AnfrageErhalten {
        /// GLN of the sender (nMSB).
        sender: MarktpartnerCode,
        /// GLN of the receiver (aMSB).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (35001/35002/35004/35005).
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound QUOTES response dispatched to the counterparty.
    AngebotGesendet {
        /// QUOTES response PID (15001/15002/15004/15005).
        response_pid: Option<Pruefidentifikator>,
        /// EDIFACT message reference of the QUOTES response.
        message_ref: MessageRef,
    },
    /// QUOTES response received (requester perspective).
    AngebotErhalten {
        /// QUOTES Prüfidentifikator (15001/15002/15004/15005).
        response_pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Process completed.
    Abgeschlossen,
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired.
    DeadlineExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for PreisanfrageEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "PreisanfrageAnfrageGesendet",
            Self::AnfrageErhalten { .. } => "PreisanfrageAnfrageErhalten",
            Self::ValidationPassed { .. } => "PreisanfrageValidationPassed",
            Self::AngebotGesendet { .. } => "PreisanfrageAngebotGesendet",
            Self::AngebotErhalten { .. } => "PreisanfrageAngebotErhalten",
            Self::Abgeschlossen => "PreisanfrageAbgeschlossen",
            Self::Rejected { .. } => "PreisanfrageRejected",
            Self::DeadlineExpired { .. } => "PreisanfrageDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            // Only `SendReqote` emits it: the REQOTE leaves this party.
            Self::AnfrageGesendet {
                pruefidentifikator,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            // Only `ReceiveReqote` emits it: the REQOTE arrived.
            Self::AnfrageErhalten {
                pruefidentifikator,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AngebotGesendet {
                response_pid,
                message_ref,
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: response_pid.map(Pruefidentifikator::as_u32),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AngebotErhalten {
                response_pid,
                message_ref,
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(response_pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::ValidationPassed { .. }
            | Self::Abgeschlossen
            | Self::Rejected { .. }
            | Self::DeadlineExpired { .. } => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured from the REQOTE, on either side.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreisanfrageData {
    /// MP-ID of the requester (the REQOTE's sender).
    pub sender: MarktpartnerCode,
    /// MP-ID of the MSB asked for the Angebot (the REQOTE's receiver).
    pub receiver: MarktpartnerCode,
    /// BDEW Prüfidentifikator (35001/35002/35004/35005).
    pub pruefidentifikator: Pruefidentifikator,
}

/// State of a WiM Preisanfrage process.
///
/// # Lifecycle
///
/// ```text
/// responder: New → Eingegangen → ValidationPassed → AngebotAusgetauscht → Abgeschlossen
///                              ↘ Rejected (failed validation)
/// requester: New → Angefragt ─────────────────────→ AngebotAusgetauscht → Abgeschlossen
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum PreisanfrageState {
    /// No events yet.
    #[default]
    New,
    /// This party sent the REQOTE and awaits the QUOTES.
    Angefragt(PreisanfrageData),
    /// REQOTE received; awaiting response.
    Eingegangen(PreisanfrageData),
    /// Validation passed; response not yet sent.
    ValidationPassed(PreisanfrageData),
    /// QUOTES response sent (responder) or received (requester).
    AngebotAusgetauscht {
        /// Data from the anfrage.
        data: PreisanfrageData,
        /// QUOTES response PID.
        response_pid: Option<Pruefidentifikator>,
    },
    /// Process completed.
    Abgeschlossen(PreisanfrageData),
    /// Process rejected.
    Rejected {
        /// Reason.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// WiM Teil 1 gives the MSB its answer window on the Preisanfrage and no consequence of missing it. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<PreisanfrageState>,
    },
}

impl PreisanfrageState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Angefragt(_) => "Angefragt",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AngebotAusgetauscht { .. } => "AngebotAusgetauscht",
            Self::Abgeschlossen(_) => "Abgeschlossen",
            Self::Rejected { .. } => "Rejected",
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

/// A Preisanfrage holds its location while a QUOTES is still owed — by this
/// party or to it. A second Anfrage on the same location beside a running one
/// would leave the answering QUOTES two processes to resume.
impl mako_engine::workflow::OccupiesBusinessKey for PreisanfrageState {
    fn occupies_business_key(&self) -> bool {
        matches!(
            self.offen(),
            Self::Angefragt(_) | Self::Eingegangen(_) | Self::ValidationPassed(_)
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Preisanfrage workflow.
#[derive(Clone)]
pub enum PreisanfrageCommand {
    /// Send a REQOTE Anfrage — this party is the requester.
    ///
    /// Enqueues the REQOTE to `receiver`; the QUOTES that answers it resumes
    /// the process through [`PreisanfrageCommand::ReceiveAngebot`].
    SendReqote {
        /// BDEW Prüfidentifikator (35001/35002/35004/35005).
        pid: Pruefidentifikator,
        /// MP-ID of this party.
        sender: MarktpartnerCode,
        /// MP-ID of the MSB asked for the Angebot.
        receiver: MarktpartnerCode,
        /// The Messlokation or Marktlokation the Anfrage is about (`LOC+172`).
        location: String,
        /// EDIFACT message reference of the outbound REQOTE.
        message_ref: MessageRef,
    },
    /// Inbound REQOTE received.
    ReceiveReqote {
        /// BDEW Prüfidentifikator (35001/35002/35004/35005).
        pid: Pruefidentifikator,
        /// GLN of the sender.
        sender: MarktpartnerCode,
        /// GLN of the receiver.
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if validation passed.
        validation_passed: bool,
        /// Validation errors.
        validation_errors: Vec<String>,
    },
    /// Send the QUOTES Angebot answering the REQOTE — this party is the MSB.
    ///
    /// Enqueues the QUOTES to the REQOTE's sender under the PID that answers
    /// the REQOTE's (QUOTES AHB 1.1a: 35001 → 15001 „MSBA an MSBN", …).
    SendAngebot {
        /// EDIFACT message reference of the QUOTES response.
        message_ref: MessageRef,
        /// The Messlokation or Marktlokation the REQOTE addressed (`LOC+172`),
        /// echoed so the requester resumes its process by it.
        location: String,
    },
    /// Inbound QUOTES received (requester side, answering this party's REQOTE).
    ReceiveAngebot {
        /// QUOTES Prüfidentifikator (15001/15002/15004/15005).
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Mark the exchange as complete.
    Abschliessen,
    /// Deadline expired.
    TimeoutExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for PreisanfrageCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Preisanfrage workflow (REQOTE/QUOTES, PIDs 35001/35002/35004/35005 / 15001/15002/15004/15005).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<WimPreisanfrageWorkflow>(
///     tenant_id,
///     WorkflowId::new("wim-preisanfrage", "FV2025-10-01"),
/// );
/// ```
pub struct WimPreisanfrageWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while the QUOTES is still owed — by this party
/// (`Eingegangen`, `ValidationPassed`) or to it (`Angefragt`).
fn window_open(label: &str, state: &PreisanfrageState) -> bool {
    label == PREISANFRAGE_DEADLINE_LABEL
        && matches!(
            state,
            PreisanfrageState::Angefragt(_)
                | PreisanfrageState::Eingegangen(_)
                | PreisanfrageState::ValidationPassed(_)
        )
}

impl Workflow for WimPreisanfrageWorkflow {
    type State = PreisanfrageState;
    type Event = PreisanfrageEvent;
    type Command = PreisanfrageCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[PREISANFRAGE_DEADLINE_LABEL];

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| PreisanfrageCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            PreisanfrageState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            PreisanfrageEvent::AnfrageGesendet {
                sender,
                receiver,
                pruefidentifikator,
                ..
            } => PreisanfrageState::Angefragt(PreisanfrageData {
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            PreisanfrageEvent::AnfrageErhalten {
                sender,
                receiver,
                pruefidentifikator,
                ..
            } => PreisanfrageState::Eingegangen(PreisanfrageData {
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            PreisanfrageEvent::ValidationPassed { .. } => match state {
                PreisanfrageState::Eingegangen(data) => PreisanfrageState::ValidationPassed(data),
                other => other,
            },
            PreisanfrageEvent::AngebotGesendet { response_pid, .. } => match state {
                PreisanfrageState::ValidationPassed(data) => {
                    PreisanfrageState::AngebotAusgetauscht {
                        response_pid: *response_pid,
                        data,
                    }
                }
                other => other,
            },
            PreisanfrageEvent::AngebotErhalten { response_pid, .. } => match state {
                PreisanfrageState::Angefragt(data) => PreisanfrageState::AngebotAusgetauscht {
                    response_pid: Some(*response_pid),
                    data,
                },
                other => other,
            },
            PreisanfrageEvent::Abgeschlossen => match state {
                PreisanfrageState::AngebotAusgetauscht { data, .. } => {
                    PreisanfrageState::Abgeschlossen(data)
                }
                other => other,
            },
            PreisanfrageEvent::Rejected { reason } => PreisanfrageState::Rejected {
                reason: reason.clone(),
            },
            PreisanfrageEvent::DeadlineExpired { label, .. } => match state {
                PreisanfrageState::Angefragt(_)
                | PreisanfrageState::Eingegangen(_)
                | PreisanfrageState::ValidationPassed(_) => PreisanfrageState::FristAbgelaufen {
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
            PreisanfrageCommand::SendReqote {
                pid,
                sender,
                receiver,
                location,
                message_ref,
            } => {
                if !matches!(state, PreisanfrageState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !REQOTE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected REQOTE PID (35001/35002/35004/35005), got {pid}",
                    )));
                }
                let payload = mako_engine::payload::ReqotePayload {
                    pid: Some(pid.as_u32()),
                    sender: Some(sender.as_str().to_owned()),
                    receiver: Some(receiver.as_str().to_owned()),
                    location: Some(location.clone()),
                    message_ref: Some(message_ref.as_str().to_owned()),
                    ..Default::default()
                };
                Ok(WorkflowOutput::with_outbox(
                    vec![PreisanfrageEvent::AnfrageGesendet {
                        sender,
                        receiver: receiver.clone(),
                        message_ref,
                        pruefidentifikator: pid,
                    }],
                    vec![PendingOutbox::typed(receiver.as_str(), &payload).caused_by(0)],
                ))
            }

            PreisanfrageCommand::ReceiveReqote {
                pid,
                sender,
                receiver,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, PreisanfrageState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !REQOTE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected REQOTE PID (35001/35002/35004/35005), got {pid}",
                    )));
                }
                let mut events = vec![PreisanfrageEvent::AnfrageErhalten {
                    sender,
                    receiver,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(PreisanfrageEvent::ValidationPassed { message_ref });
                } else {
                    events.push(PreisanfrageEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            PreisanfrageCommand::SendAngebot {
                message_ref,
                location,
            } => {
                let data = match state {
                    PreisanfrageState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                // The answering PID comes from the published pairing, never
                // from arithmetic on the REQOTE's; a REQOTE outside the table
                // has no QUOTES to answer it with.
                let response_pid = quotes_response_pid(data.pruefidentifikator.as_u32())
                    .ok_or_else(|| {
                        WorkflowError::rejected(format!(
                            "REQOTE {} has no answering QUOTES PID",
                            data.pruefidentifikator
                        ))
                    })?;
                // The QUOTES goes back the way the REQOTE came: this party —
                // the REQOTE's receiver — to its sender.
                let payload = mako_engine::payload::QuotesPayload {
                    pid: Some(response_pid.as_u32()),
                    sender: Some(data.receiver.as_str().to_owned()),
                    receiver: Some(data.sender.as_str().to_owned()),
                    location: Some(location.clone()),
                    message_ref: Some(message_ref.as_str().to_owned()),
                    ..Default::default()
                };
                Ok(WorkflowOutput::with_outbox(
                    vec![PreisanfrageEvent::AngebotGesendet {
                        response_pid: Some(response_pid),
                        message_ref,
                    }],
                    vec![PendingOutbox::typed(data.sender.as_str(), &payload).caused_by(0)],
                ))
            }

            PreisanfrageCommand::ReceiveAngebot { pid, message_ref } => {
                if !QUOTES_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected QUOTES PID (15001/15002/15004/15005), got {pid}",
                    )));
                }
                // Only the requester receives an Angebot: a responder in
                // `ValidationPassed` owes the QUOTES and cannot record one.
                let PreisanfrageState::Angefragt(data) = state else {
                    return Err(WorkflowError::invalid_state("Angefragt", state.label()));
                };
                if quotes_response_pid(data.pruefidentifikator.as_u32()) != Some(pid) {
                    return Err(WorkflowError::rejected(format!(
                        "QUOTES {pid} does not answer REQOTE {}",
                        data.pruefidentifikator
                    )));
                }
                Ok(vec![PreisanfrageEvent::AngebotErhalten {
                    response_pid: pid,
                    message_ref,
                }]
                .into())
            }

            PreisanfrageCommand::Abschliessen => {
                if !matches!(state, PreisanfrageState::AngebotAusgetauscht { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AngebotAusgetauscht",
                        state.label(),
                    ));
                }
                Ok(vec![PreisanfrageEvent::Abgeschlossen].into())
            }

            PreisanfrageCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(vec![].into());
                }
                Ok(vec![PreisanfrageEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{ids::DeadlineId, workflow::Workflow};

    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).unwrap()
    }
    fn mcod(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    fn apply_all(init: PreisanfrageState, events: &[PreisanfrageEvent]) -> PreisanfrageState {
        events.iter().fold(init, WimPreisanfrageWorkflow::apply)
    }

    #[test]
    fn reqote_happy_path_35001() {
        let out = WimPreisanfrageWorkflow::handle(
            &PreisanfrageState::New,
            PreisanfrageCommand::ReceiveReqote {
                pid: pid(35001),
                sender: mcod("4012345000023"),
                receiver: mcod("9900357000004"),
                message_ref: mref("REQOTE-001"),
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 2);
        let state = apply_all(PreisanfrageState::New, &out.events);
        assert!(matches!(state, PreisanfrageState::ValidationPassed(_)));

        let out = WimPreisanfrageWorkflow::handle(
            &state,
            PreisanfrageCommand::SendAngebot {
                message_ref: mref("QUOTES-001"),
                location: "51238696012".to_owned(),
            },
        )
        .unwrap();
        if let PreisanfrageEvent::AngebotGesendet { response_pid, .. } = &out.events[0] {
            assert_eq!(response_pid.map(|p| p.as_u32()), Some(15001));
        } else {
            panic!("expected AngebotGesendet");
        }
    }

    /// The Angebot is a message, not a note in the event log: `SendAngebot`
    /// queues the QUOTES 15001 („MSBA an MSBN", QUOTES AHB 1.1a) back to the
    /// REQOTE's sender, echoing the location the requester resumes by.
    #[test]
    fn the_angebot_queues_the_quotes_back_to_the_requester() {
        let out = WimPreisanfrageWorkflow::handle(
            &PreisanfrageState::New,
            PreisanfrageCommand::ReceiveReqote {
                pid: pid(35001),
                sender: mcod("9900000000002"),
                receiver: mcod("9900357000004"),
                message_ref: mref("REQOTE-IN-1"),
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .expect("the REQOTE is recorded");
        let state = apply_all(PreisanfrageState::New, &out.events);
        let out = WimPreisanfrageWorkflow::handle(
            &state,
            PreisanfrageCommand::SendAngebot {
                message_ref: mref("QUOTES-OUT-1"),
                location: "51238696012".to_owned(),
            },
        )
        .expect("the MSB answers");
        assert_eq!(out.outbox.len(), 1, "the QUOTES is queued");
        let q = &out.outbox[0];
        assert_eq!(&*q.message_type, "QUOTES");
        assert_eq!(&*q.recipient, "9900000000002");
        assert_eq!(q.payload["pid"], 15001);
        assert_eq!(q.payload["sender"], "9900357000004");
        assert_eq!(q.payload["receiver"], "9900000000002");
        assert_eq!(q.payload["location"], "51238696012");
        use mako_engine::payload::OutboundPayload as _;
        let typed = mako_engine::payload::QuotesPayload::from_payload(&q.payload)
            .expect("a QUOTES payload");
        assert_eq!(typed.pid, Some(15001));
    }

    #[test]
    fn reqote_wrong_pid_rejected() {
        let result = WimPreisanfrageWorkflow::handle(
            &PreisanfrageState::New,
            PreisanfrageCommand::ReceiveReqote {
                pid: pid(55001),
                sender: mcod("4012345000023"),
                receiver: mcod("9900357000004"),
                message_ref: mref("X"),
                validation_passed: true,
                validation_errors: vec![],
            },
        );
        assert!(result.is_err());
    }

    fn send_reqote(p: u32) -> PreisanfrageCommand {
        PreisanfrageCommand::SendReqote {
            pid: pid(p),
            sender: mcod("9900357000004"),
            receiver: mcod("9900000000002"),
            location: "51238696012".to_owned(),
            message_ref: mref("REQOTE-OUT-1"),
        }
    }

    fn angefragt(p: u32) -> PreisanfrageState {
        let out = WimPreisanfrageWorkflow::handle(&PreisanfrageState::New, send_reqote(p))
            .expect("the requester sends its REQOTE");
        apply_all(PreisanfrageState::New, &out.events)
    }

    /// The requester (MSBN for 35001, REQOTE AHB 1.2 Kap. 4.1.2 „MSBN an
    /// MSBA") sends the REQOTE — and the REQOTE leaves this party.
    #[test]
    fn the_requester_sends_its_reqote() {
        let out = WimPreisanfrageWorkflow::handle(&PreisanfrageState::New, send_reqote(35001))
            .expect("accepted");
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(&*out.outbox[0].message_type, "REQOTE");
        assert_eq!(&*out.outbox[0].recipient, "9900000000002");
        assert_eq!(out.outbox[0].payload["pid"], 35001);
        use mako_engine::payload::OutboundPayload as _;
        let typed = mako_engine::payload::ReqotePayload::from_payload(&out.outbox[0].payload)
            .expect("a REQOTE payload");
        assert_eq!(typed.receiver.as_deref(), Some("9900000000002"));
        assert!(matches!(
            out.events[0],
            PreisanfrageEvent::AnfrageGesendet { .. }
        ));
    }

    /// The QUOTES 15001 („MSBA an MSBN", QUOTES AHB 1.1a) answers the
    /// requester's REQOTE, so the requester records it.
    #[test]
    fn the_requester_records_the_quotes_answering_its_reqote() {
        let state = angefragt(35001);
        let out = WimPreisanfrageWorkflow::handle(
            &state,
            PreisanfrageCommand::ReceiveAngebot {
                pid: pid(15001),
                message_ref: mref("QUOTES-IN-1"),
            },
        )
        .expect("the requester records the Angebot");
        assert!(out.outbox.is_empty());
        let state = apply_all(state, &out.events);
        assert!(matches!(
            state,
            PreisanfrageState::AngebotAusgetauscht {
                response_pid: Some(p),
                ..
            } if p.as_u32() == 15001
        ));
    }

    #[test]
    fn a_quotes_for_another_anfrage_is_refused() {
        assert!(
            WimPreisanfrageWorkflow::handle(
                &angefragt(35001),
                PreisanfrageCommand::ReceiveAngebot {
                    pid: pid(15005),
                    message_ref: mref("Q"),
                },
            )
            .is_err()
        );
    }

    /// A responder that received a REQOTE owes the QUOTES; it cannot record
    /// one, and the requester cannot answer its own Anfrage.
    #[test]
    fn the_two_sides_do_not_cross() {
        let responder = apply_all(
            PreisanfrageState::New,
            &WimPreisanfrageWorkflow::handle(
                &PreisanfrageState::New,
                PreisanfrageCommand::ReceiveReqote {
                    pid: pid(35001),
                    sender: mcod("4012345000023"),
                    receiver: mcod("9900357000004"),
                    message_ref: mref("R"),
                    validation_passed: true,
                    validation_errors: vec![],
                },
            )
            .unwrap()
            .events,
        );
        assert!(
            WimPreisanfrageWorkflow::handle(
                &responder,
                PreisanfrageCommand::ReceiveAngebot {
                    pid: pid(15001),
                    message_ref: mref("Q"),
                },
            )
            .is_err()
        );
        assert!(
            WimPreisanfrageWorkflow::handle(
                &angefragt(35001),
                PreisanfrageCommand::SendAngebot {
                    message_ref: mref("Q"),
                    location: "51238696012".to_owned(),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn timeout_in_abgeschlossen_is_noop() {
        let data = PreisanfrageData {
            sender: mcod("4012345000023"),
            receiver: mcod("9900357000004"),
            pruefidentifikator: pid(35001),
        };
        let state = PreisanfrageState::Abgeschlossen(data);
        let dl_id = DeadlineId::new();
        let out = WimPreisanfrageWorkflow::handle(
            &state,
            PreisanfrageCommand::TimeoutExpired {
                deadline_id: dl_id,
                label: PREISANFRAGE_DEADLINE_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty());
    }
}

#[cfg(test)]
mod verlauf_tests {
    use super::*;
    use mako_engine::types::{MessageRef, Pruefidentifikator};

    fn pid(p: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(p).expect("valid PID")
    }

    fn stated(richtung: Richtung, pid: Option<u32>, message_ref: Option<&str>) -> Verlauf {
        Verlauf {
            richtung,
            pid,
            message_ref: message_ref.map(str::to_owned),
        }
    }

    #[test]
    fn verlauf_states_the_messages_and_only_the_messages() {
        let anfrage = PreisanfrageEvent::AnfrageErhalten {
            sender: MarktpartnerCode::new("9900000000001"),
            receiver: MarktpartnerCode::new("9900000000002"),
            message_ref: MessageRef::new("REQ-1"),
            pruefidentifikator: pid(35_001),
        };
        assert_eq!(
            anfrage.verlauf(),
            Some(stated(Richtung::Eingang, Some(35_001), Some("REQ-1")))
        );
        let angebot = PreisanfrageEvent::AngebotGesendet {
            response_pid: None,
            message_ref: MessageRef::new("QUO-1"),
        };
        assert_eq!(
            angebot.verlauf(),
            Some(stated(Richtung::Ausgang, None, Some("QUO-1")))
        );
        let gesendet = PreisanfrageEvent::AnfrageGesendet {
            sender: MarktpartnerCode::new("9900000000002"),
            receiver: MarktpartnerCode::new("9900000000001"),
            message_ref: MessageRef::new("REQ-2"),
            pruefidentifikator: pid(35_005),
        };
        assert_eq!(
            gesendet.verlauf(),
            Some(stated(Richtung::Ausgang, Some(35_005), Some("REQ-2")))
        );
        let erhalten = PreisanfrageEvent::AngebotErhalten {
            response_pid: pid(15_005),
            message_ref: MessageRef::new("QUO-2"),
        };
        assert_eq!(
            erhalten.verlauf(),
            Some(stated(Richtung::Eingang, Some(15_005), Some("QUO-2")))
        );
        assert_eq!(PreisanfrageEvent::Abgeschlossen.verlauf(), None);
    }
}
