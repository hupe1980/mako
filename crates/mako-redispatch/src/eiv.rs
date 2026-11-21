//! The Einsatzverantwortliche (EIV) — Redispatch 2.0 from the plant's side.
//!
//! The rest of this crate is the grid operators' side: it *receives* what the
//! EIV sends. This module is the EIV itself, registered by [`crate::EivModule`]
//! for a deployment that declares the role, and never by [`crate::RedispatchModule`].
//!
//! # What the EIV sends and receives
//!
//! Every document the EIV sends goes to the **Data Provider** (`A39`), which
//! forwards it to the Netzbetreiber; the DP is the ANB unless the ANB hands the
//! role to a third party (`BilAReM` Kap. 1, „Data Provider (DP)"). The
//! Anwendungstabellen name the EIV as sender (`SenderRole` `A27`) in these
//! columns:
//!
//! | Document | Anwendungsfall | EIV → | Workflow |
//! |---|---|---|---|
//! | `PlannedResourceScheduleDocument` `A14` / `Z11` | Planungsdaten / Probeplanungsdaten im Planwertmodell mit DP | DP | [`planungsdaten`] |
//! | `Unavailability_MarketDocument` `A76` / `A80` | Nichtbeanspruchbarkeiten an NB mit DP | DP | [`nichtbeanspruchbarkeit`] |
//! | `Stammdaten` `Z02` | initiale Stammdaten; Stammdatenänderung vom EIV (verantwortlich) | DP | [`stammdaten`] |
//! | `Kostenblatt` `Z05` | Planungsdaten im Planwertmodell mit DP | DP | [`kostenblatt`] |
//! | `ActivationDocument` `A41` | Rückmeldung zur Umsetzbarkeit auf den Abruf im Aufforderungsfall | DP | [`abruf`] |
//!
//! Sources: `PlannedResourceScheduleDocument` AWT 1.0f S. 3; `Unavailability_MarketDocument`
//! AWT 1.1b S. 3; `Stammdaten` AWT 1.4b S. 3; `Kostenblatt` AWT 1.0d S. 3;
//! `ActivationDocument` AWT 1.1f S. 35.
//!
//! What it receives: the activation (`A96`) from the DP in the Aufforderungsfall
//! and in the Duldungsfall (`ActivationDocument` AWT 1.1f S. 3 and S. 7,
//! Prozessschritt 2, `ReceiverRole` `A27`), and an `AcknowledgementDocument`
//! for every Übertragungsdatei it sent.
//!
//! # The acknowledgement decides whether a sending counts
//!
//! The receiver of an Übertragungsdatei answers its syntax with exactly one
//! `AcknowledgementDocument` within [`crate::fristen::ACK_FRIST`]. For the
//! sender the Formatbeschreibung fixes what silence means:
//! „Eine nicht empfangene AcknowledgementDocument-Nachricht bedeutet, dass die
//! Ursprungsnachricht beim Empfänger nicht bearbeitet wird." — and finding the
//! cause is the sender's „Initiativ-Aufgabe" (`AcknowledgementDocument` FB 1.0g
//! S. 11). A negative acknowledgement (`A02` with its Fehlercode) rejects the
//! whole file (S. 14). Both therefore end a sending in a [`Klaerung`], sourced,
//! rather than in `FristAbgelaufen`.
//!
//! # An activation is a person's decision
//!
//! In the Aufforderungsfall the anweisende Netzbetreiber asks the EIV to change
//! a resource's output and to steer it (`BilAReM` Kap. 1, „Aufforderungsfall").
//! That changes how a plant runs, so [`abruf`] holds the activation in
//! [`AbrufState::EntscheidungOffen`] until a person releases the Rückmeldung
//! ([`AbrufCommand::Freigeben`]); nothing answers it unattended. The window for
//! that answer is not published — BK6-23-241 Tenorziffer 7 leaves the
//! Prozessbeschreibungen to the ÜNB — so it is the operator's configured value,
//! registered by the host at receipt under [`abruf::ANTWORTFENSTER_LABEL`]. A
//! lapse records [`AbrufState::FristAbgelaufen`]; no answer is invented.

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::{PROCESS_FAILED, PendingOutbox},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use serde::{Deserialize, Serialize};

// ── Role codes ────────────────────────────────────────────────────────────────

/// The market role codes the Anwendungstabellen use in `SenderRole` /
/// `ReceiverRole`.
pub mod rollen {
    /// Einsatzverantwortlicher.
    pub const EIV: &str = "A27";
    /// Data Provider — the receiver of every document the EIV sends.
    pub const DATA_PROVIDER: &str = "A39";
    /// Netzbetreiber.
    pub const NETZBETREIBER: &str = "A18";
    /// Betreiber der technischen Ressource — receives the activation the EIV
    /// forwards in the Duldungsfall (`ActivationDocument` AWT 1.1f S. 7).
    pub const BTR: &str = "A21";
}

/// Whether a document whose `ReceiverRole` is `receiver_role` is addressed to
/// the EIV.
///
/// The one question that separates the two sides of a deployment: a document
/// addressed to `A27` must never reach a Netzbetreiber workflow, and one
/// addressed to anyone else must never reach an EIV workflow.
#[must_use]
pub fn an_eiv_adressiert(receiver_role: &str) -> bool {
    receiver_role == rollen::EIV
}

// ── Sendings ──────────────────────────────────────────────────────────────────

/// One Übertragungsdatei the EIV sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sendung {
    /// The document's identification (`DocumentIdentification` / `mRID`).
    pub mrid: String,
    /// Its `DocumentType` / `type` code (`A14`, `A76`, `Z02`, …).
    pub dokumenttyp: String,
    /// The EIV's MP-ID.
    pub sender: String,
    /// The Data Provider's MP-ID.
    pub empfaenger: String,
    /// When it was handed over for sending (RFC 3339, UTC).
    pub gesendet_at: String,
}

/// One `Reason` of an `AcknowledgementDocument`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quittungsgrund {
    /// The `ReasonCode` (`A01`, `A02`, `Z12` … `Z18`).
    pub code: String,
    /// The `ReasonText`, when given.
    pub text: Option<String>,
}

/// Why a sending needs the operator: the receiver did not process it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "art", rename_all = "snake_case")]
pub enum Klaerung {
    /// A negative `AcknowledgementDocument` (`A02`) rejected the whole file.
    Abgelehnt {
        /// The acknowledgement's identification.
        ack_mrid: String,
        /// Its reasons.
        gruende: Vec<Quittungsgrund>,
    },
    /// No acknowledgement arrived within [`crate::fristen::ACK_FRIST`]: the
    /// receiver does not process the file (`AcknowledgementDocument` FB 1.0g
    /// S. 11).
    KeineQuittung {
        /// The lapsed window's label.
        label: String,
    },
}

impl Klaerung {
    /// The reason text a Klärfall notice carries, naming the document.
    #[must_use]
    pub fn grund(&self, dokument: &str, mrid: &str) -> String {
        match self {
            Self::Abgelehnt { ack_mrid, gruende } => {
                let codes: Vec<String> = gruende
                    .iter()
                    .map(|g| match &g.text {
                        Some(t) => format!("{} ({t})", g.code),
                        None => g.code.clone(),
                    })
                    .collect();
                format!(
                    "{dokument} {mrid} abgelehnt: AcknowledgementDocument {ack_mrid} mit {} \
                     — der Empfänger verarbeitet die Übertragungsdatei nicht \
                     (AcknowledgementDocument FB 1.0g S. 14)",
                    codes.join(", ")
                )
            }
            Self::KeineQuittung { .. } => format!(
                "{dokument} {mrid}: kein AcknowledgementDocument innerhalb von 3 Minuten — der \
                 Empfänger bearbeitet die Übertragungsdatei nicht (AcknowledgementDocument FB \
                 1.0g S. 11)"
            ),
        }
    }
}

/// State of one EIV sending.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum SendungState {
    /// Nothing sent yet.
    #[default]
    New,
    /// Sent; the acknowledgement is outstanding.
    Gesendet(Sendung),
    /// A positive acknowledgement (`A01`) arrived: the receiver processes it.
    Bestaetigt {
        /// The sending.
        sendung: Sendung,
        /// The acknowledgement's identification.
        ack_mrid: String,
    },
    /// The receiver did not process it; the operator has to find out why.
    Klaerfall {
        /// The sending.
        sendung: Sendung,
        /// What happened.
        klaerung: Klaerung,
    },
}

impl SendungState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Gesendet(_) => "Gesendet",
            Self::Bestaetigt { .. } => "Bestaetigt",
            Self::Klaerfall { .. } => "Klaerfall",
        }
    }

    /// The sending, once there is one.
    #[must_use]
    pub fn sendung(&self) -> Option<&Sendung> {
        match self {
            Self::New => None,
            Self::Gesendet(s)
            | Self::Bestaetigt { sendung: s, .. }
            | Self::Klaerfall { sendung: s, .. } => Some(s),
        }
    }
}

/// Events of an EIV sending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SendungEvent {
    /// The document was handed over for sending.
    Gesendet(Sendung),
    /// The receiver's `AcknowledgementDocument` arrived.
    QuittungEmpfangen {
        /// Its identification.
        ack_mrid: String,
        /// `true` for `A01`, `false` for `A02`.
        angenommen: bool,
        /// Its reasons.
        gruende: Vec<Quittungsgrund>,
    },
    /// The acknowledgement window lapsed with nothing received.
    QuittungAusgeblieben {
        /// The deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

/// The shared shape; each workflow stores its own prefixed newtype.
impl EventPayload for SendungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Gesendet(_) => "EivSendungGesendet",
            Self::QuittungEmpfangen { .. } => "EivSendungQuittungEmpfangen",
            Self::QuittungAusgeblieben { .. } => "EivSendungQuittungAusgeblieben",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::Gesendet(s) => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: None,
                message_ref: Some(s.mrid.clone()),
            }),
            Self::QuittungEmpfangen { ack_mrid, .. } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: None,
                message_ref: Some(ack_mrid.clone()),
            }),
            Self::QuittungAusgeblieben { .. } => None,
        }
    }
}

/// Commands of an EIV sending.
#[derive(Debug, Clone)]
pub enum SendungCommand {
    /// Record the document handed over for sending — built and validated at
    /// the transport boundary before this command exists.
    Senden(Sendung),
    /// The receiver's `AcknowledgementDocument` arrived.
    QuittungEmpfangen {
        /// Its identification.
        ack_mrid: String,
        /// Its `ReceivingDocumentIdentification`.
        quittierte_mrid: String,
        /// `true` for `A01`, `false` for `A02`.
        angenommen: bool,
        /// Its reasons.
        gruende: Vec<Quittungsgrund>,
    },
    /// A deadline fired.
    TimeoutExpired {
        /// The deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

impl CommandPayload for SendungCommand {}

/// The acknowledgement window is open while the sending is unacknowledged.
fn window_open(label: &str, state: &SendungState, ack_label: &str) -> bool {
    label == ack_label && matches!(state, SendungState::Gesendet(_))
}

fn sendung_apply(state: SendungState, event: &SendungEvent) -> SendungState {
    match event {
        SendungEvent::Gesendet(s) => match state {
            SendungState::New => SendungState::Gesendet(s.clone()),
            other => other,
        },
        SendungEvent::QuittungEmpfangen {
            ack_mrid,
            angenommen,
            gruende,
        } => {
            let Some(sendung) = state.sendung().cloned() else {
                return state;
            };
            if *angenommen {
                SendungState::Bestaetigt {
                    sendung,
                    ack_mrid: ack_mrid.clone(),
                }
            } else {
                SendungState::Klaerfall {
                    sendung,
                    klaerung: Klaerung::Abgelehnt {
                        ack_mrid: ack_mrid.clone(),
                        gruende: gruende.clone(),
                    },
                }
            }
        }
        SendungEvent::QuittungAusgeblieben { label, .. } => match state {
            SendungState::Gesendet(sendung) => SendungState::Klaerfall {
                sendung,
                klaerung: Klaerung::KeineQuittung {
                    label: label.to_string(),
                },
            },
            other => other,
        },
    }
}

/// The `ProcessFailed` notice a Klärfall raises, naming the document and why.
fn klaerfall_notice(dokument: &str, sendung: &Sendung, klaerung: &Klaerung) -> PendingOutbox {
    PendingOutbox::new(
        PROCESS_FAILED,
        sendung.empfaenger.as_str(),
        serde_json::json!({
            "reason": klaerung.grund(dokument, &sendung.mrid),
            "undelivered_message_type": dokument,
            "recipient": sendung.empfaenger,
        }),
    )
    .caused_by(0)
}

fn sendung_handle(
    state: &SendungState,
    command: SendungCommand,
    ack_label: &str,
    dokument: &str,
    dokumenttypen: &[&str],
) -> Result<WorkflowOutput<SendungEvent>, WorkflowError> {
    match command {
        SendungCommand::Senden(sendung) => match state {
            SendungState::New => {
                if !dokumenttypen.contains(&sendung.dokumenttyp.as_str()) {
                    return Err(WorkflowError::rejected(format!(
                        "{dokument} DocumentType {:?} is not one the EIV sends ({dokumenttypen:?})",
                        sendung.dokumenttyp
                    )));
                }
                if sendung.mrid.trim().is_empty() || sendung.empfaenger.trim().is_empty() {
                    return Err(WorkflowError::rejected(format!(
                        "{dokument}: a sending needs its identification and its receiver"
                    )));
                }
                Ok(vec![SendungEvent::Gesendet(sendung)].into())
            }
            // The same document again is the same sending.
            other if other.sendung().is_some_and(|s| s.mrid == sendung.mrid) => Ok(vec![].into()),
            other => Err(WorkflowError::rejected(format!(
                "{dokument}: this process already carries document {:?}",
                other.sendung().map(|s| s.mrid.as_str()).unwrap_or_default()
            ))),
        },

        SendungCommand::QuittungEmpfangen {
            ack_mrid,
            quittierte_mrid,
            angenommen,
            gruende,
        } => {
            let Some(sendung) = state.sendung() else {
                return Err(WorkflowError::rejected(format!(
                    "{dokument}: an acknowledgement for {quittierte_mrid:?} before anything was sent"
                )));
            };
            if sendung.mrid != quittierte_mrid {
                return Err(WorkflowError::rejected(format!(
                    "{dokument}: acknowledgement {ack_mrid:?} answers {quittierte_mrid:?}, not {:?}",
                    sendung.mrid
                )));
            }
            match state {
                // A late acknowledgement is still the receiver's answer
                // (FB 1.0g S. 14: it does not breach the Geschäftsvorfall).
                SendungState::Gesendet(_)
                | SendungState::Klaerfall {
                    klaerung: Klaerung::KeineQuittung { .. },
                    ..
                } => {
                    let event = SendungEvent::QuittungEmpfangen {
                        ack_mrid: ack_mrid.clone(),
                        angenommen,
                        gruende: gruende.clone(),
                    };
                    if angenommen {
                        return Ok(vec![event].into());
                    }
                    let klaerung = Klaerung::Abgelehnt { ack_mrid, gruende };
                    Ok(WorkflowOutput::with_outbox(
                        vec![event],
                        vec![klaerfall_notice(dokument, sendung, &klaerung)],
                    ))
                }
                // Exactly one acknowledgement answers each file (FB 1.0g
                // S. 11); a second one changes nothing.
                _ => Ok(vec![].into()),
            }
        }

        SendungCommand::TimeoutExpired { deadline_id, label } => {
            if !window_open(&label, state, ack_label) {
                return Ok(vec![].into());
            }
            let Some(sendung) = state.sendung() else {
                return Ok(vec![].into());
            };
            let klaerung = Klaerung::KeineQuittung {
                label: label.to_string(),
            };
            Ok(WorkflowOutput::with_outbox(
                vec![SendungEvent::QuittungAusgeblieben { deadline_id, label }],
                vec![klaerfall_notice(dokument, sendung, &klaerung)],
            ))
        }
    }
}

macro_rules! sendung_workflow {
    (
        $(#[$meta:meta])*
        $name:ident,
        $event:ident,
        $module:ident,
        $prefix:literal $(,)?
    ) => {
        /// Event newtype of one EIV sending workflow, so its event types carry
        /// the workflow's own prefix.
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $event(pub SendungEvent);

        impl EventPayload for $event {
            fn event_type(&self) -> &'static str {
                match &self.0 {
                    SendungEvent::Gesendet(_) => concat!($prefix, "Gesendet"),
                    SendungEvent::QuittungEmpfangen { .. } => concat!($prefix, "QuittungEmpfangen"),
                    SendungEvent::QuittungAusgeblieben { .. } => {
                        concat!($prefix, "QuittungAusgeblieben")
                    }
                }
            }

            fn verlauf(&self) -> Option<Verlauf> {
                self.0.verlauf()
            }
        }

        $(#[$meta])*
        pub struct $name;

        impl $name {
            /// The name this workflow's processes are spawned and resumed under.
            pub const WORKFLOW_NAME: &'static str = $module::WORKFLOW_NAME;
        }

        impl Workflow for $name {
            type State = SendungState;
            type Event = $event;
            type Command = SendungCommand;

            const DEADLINE_LABELS: &'static [&'static str] = &[$module::ACK_WINDOW_LABEL];
            // The receiver owes the acknowledgement by a fixed instant; its
            // lapse is a breach, which the Klärfall notice announces.
            const FRIST_LABELS: &'static [&'static str] = &[$module::ACK_WINDOW_LABEL];

            fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
                window_open(deadline.label(), state, $module::ACK_WINDOW_LABEL).then(|| {
                    SendungCommand::TimeoutExpired {
                        deadline_id: deadline.deadline_id(),
                        label: deadline.label().into(),
                    }
                })
            }

            fn apply(state: Self::State, event: &Self::Event) -> Self::State {
                sendung_apply(state, &event.0)
            }

            fn handle(
                state: &Self::State,
                command: Self::Command,
            ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
                let output = sendung_handle(
                    state,
                    command,
                    $module::ACK_WINDOW_LABEL,
                    $module::DOKUMENT,
                    $module::DOKUMENTTYPEN,
                )?;
                Ok(WorkflowOutput::with_outbox(
                    output.events.into_iter().map($event).collect(),
                    output.outbox,
                ))
            }
        }
    };
}

/// `PlannedResourceScheduleDocument` from the EIV.
pub mod planungsdaten {
    /// Stable workflow name.
    pub const WORKFLOW_NAME: &str = "redispatch-eiv-planungsdaten";
    /// The DP's acknowledgement window ([`crate::fristen::ACK_FRIST`]).
    pub const ACK_WINDOW_LABEL: &str = "redispatch-eiv-planungsdaten-ack-window";
    /// The document's name.
    pub const DOKUMENT: &str = "PlannedResourceScheduleDocument";
    /// `A14` Planungsdaten, `Z11` Probeplanungsdaten für die Prognosegüteprüfung
    /// — the two columns with the EIV as sender (AWT 1.0f S. 3).
    pub const DOKUMENTTYPEN: &[&str] = &["A14", "Z11"];
}

/// `Unavailability_MarketDocument` from the EIV.
pub mod nichtbeanspruchbarkeit {
    /// Stable workflow name.
    pub const WORKFLOW_NAME: &str = "redispatch-eiv-nichtbeanspruchbarkeit";
    /// The DP's acknowledgement window ([`crate::fristen::ACK_FRIST`]).
    pub const ACK_WINDOW_LABEL: &str = "redispatch-eiv-nichtbeanspruchbarkeit-ack-window";
    /// The document's name.
    pub const DOKUMENT: &str = "Unavailability_MarketDocument";
    /// `A76` / `A80` — the `type` codes of the column „Übermittlung von
    /// Nichtbeanspruchbarkeiten an NB mit DP" (AWT 1.1b S. 3).
    pub const DOKUMENTTYPEN: &[&str] = &["A76", "A80"];
}

/// `Stammdaten` from the EIV.
pub mod stammdaten {
    /// Stable workflow name.
    pub const WORKFLOW_NAME: &str = "redispatch-eiv-stammdaten";
    /// The DP's acknowledgement window ([`crate::fristen::ACK_FRIST`]).
    pub const ACK_WINDOW_LABEL: &str = "redispatch-eiv-stammdaten-ack-window";
    /// The document's name.
    pub const DOKUMENT: &str = "Stammdaten";
    /// `Z02` — the initiale Stammdaten and the Stammdatenänderung vom EIV
    /// (verantwortlich), both with the EIV as sender (AWT 1.4b S. 3).
    pub const DOKUMENTTYPEN: &[&str] = &["Z02"];
}

/// `Kostenblatt` from the EIV.
pub mod kostenblatt {
    /// Stable workflow name.
    pub const WORKFLOW_NAME: &str = "redispatch-eiv-kostenblatt";
    /// The DP's acknowledgement window ([`crate::fristen::ACK_FRIST`]).
    pub const ACK_WINDOW_LABEL: &str = "redispatch-eiv-kostenblatt-ack-window";
    /// The document's name.
    pub const DOKUMENT: &str = "Kostenblatt";
    /// `Z05` — the column with the EIV as sender, im Planwertmodell mit DP
    /// (AWT 1.0d S. 3).
    pub const DOKUMENTTYPEN: &[&str] = &["Z05"];
}

sendung_workflow!(
    /// The EIV sends its plan for a steuerbare Ressource to the DP and tracks
    /// the acknowledgement.
    PlanungsdatenSendungWorkflow,
    PlanungsdatenSendungEvent,
    planungsdaten,
    "EivPlanungsdaten",
);

sendung_workflow!(
    /// The EIV announces a planned or unplanned Nichtbeanspruchbarkeit to the
    /// DP and tracks the acknowledgement.
    NichtbeanspruchbarkeitSendungWorkflow,
    NichtbeanspruchbarkeitSendungEvent,
    nichtbeanspruchbarkeit,
    "EivNichtbeanspruchbarkeit",
);

sendung_workflow!(
    /// The EIV sends the Stammdaten it is responsible for to the DP and tracks
    /// the acknowledgement.
    StammdatenSendungWorkflow,
    StammdatenSendungEvent,
    stammdaten,
    "EivStammdaten",
);

sendung_workflow!(
    /// The EIV sends its Kostenblatt to the DP and tracks the acknowledgement.
    KostenblattSendungWorkflow,
    KostenblattSendungEvent,
    kostenblatt,
    "EivKostenblatt",
);

// ── Activation ────────────────────────────────────────────────────────────────

/// The activation answered by a person — `ActivationDocument` `A96` from the
/// DP, answered with the Rückmeldung zur Umsetzbarkeit (`A41`).
pub mod abruf {
    /// Stable workflow name.
    pub const WORKFLOW_NAME: &str = "redispatch-eiv-abruf";
    /// The window for the Rückmeldung zur Umsetzbarkeit. **Operator-configured**:
    /// no published document fixes it (BK6-23-241 Tenorziffer 7), and the host
    /// refuses to boot an EIV deployment without the value.
    pub const ANTWORTFENSTER_LABEL: &str = "redispatch-eiv-abruf-antwortfenster";
    /// The DP's acknowledgement of the Rückmeldung ([`crate::fristen::ACK_FRIST`]).
    pub const RUECKMELDUNG_ACK_WINDOW_LABEL: &str = "redispatch-eiv-abruf-rueckmeldung-ack-window";
    /// The received document's `DocumentType`.
    pub const ABRUF_DOKUMENTTYP: &str = "A96";
    /// The answer's `DocumentType` (AWT 1.1f S. 35).
    pub const RUECKMELDUNG_DOKUMENTTYP: &str = "A41";

    /// The one guard a fired deadline passes.
    pub(crate) fn window_open(label: &str, state: &super::AbrufState) -> bool {
        use super::AbrufState;
        let offen = state.offen();
        match label {
            ANTWORTFENSTER_LABEL => matches!(offen, AbrufState::EntscheidungOffen(_)),
            RUECKMELDUNG_ACK_WINDOW_LABEL => matches!(offen, AbrufState::Freigegeben(_)),
            _ => false,
        }
    }
}

/// The received activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbrufEingang {
    /// The `ActivationDocument`'s `DocumentIdentification` — the source
    /// document the decision is about.
    pub mrid: String,
    /// The DP's MP-ID (`SenderIdentification`).
    pub sender: String,
    /// The EIV's MP-ID (`ReceiverIdentification`).
    pub empfaenger: String,
    /// The `ResourceObject` (SR-ID) of each time series.
    pub ressourcen: Vec<String>,
    /// The `ActivationTimeInterval`.
    pub zeitraum: String,
    /// When it arrived (RFC 3339, UTC).
    pub eingegangen_at: String,
}

/// A person's release of the Rückmeldung zur Umsetzbarkeit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freigabe {
    /// The Rückmeldung's `DocumentIdentification`.
    pub rueckmeldung_mrid: String,
    /// The principal who released it.
    pub freigegeben_von: String,
    /// When (RFC 3339, UTC).
    pub freigegeben_at: String,
}

/// Everything an activation process holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbrufAkte {
    /// The received activation.
    pub eingang: AbrufEingang,
    /// The release, once a person gave it.
    pub freigabe: Option<Freigabe>,
}

/// State of an activation at the EIV.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum AbrufState {
    /// Nothing received.
    #[default]
    New,
    /// Received; waiting for a person's decision.
    EntscheidungOffen(AbrufAkte),
    /// Released and sent; the DP's acknowledgement is outstanding.
    Freigegeben(AbrufAkte),
    /// The DP acknowledged the Rückmeldung (`A01`).
    Bestaetigt {
        /// The process.
        akte: AbrufAkte,
        /// The acknowledgement's identification.
        ack_mrid: String,
    },
    /// The DP did not process the Rückmeldung.
    Klaerfall {
        /// The process.
        akte: AbrufAkte,
        /// What happened.
        klaerung: Klaerung,
    },
    /// The answer window lapsed, and nothing gives the lapse an outcome. Every
    /// command the waiting state accepts is still accepted: a late answer is
    /// still an answer.
    FristAbgelaufen {
        /// The lapsed window's label.
        label: String,
        /// The state the process was waiting in.
        offen: Box<AbrufState>,
    },
}

impl AbrufState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::EntscheidungOffen(_) => "EntscheidungOffen",
            Self::Freigegeben(_) => "Freigegeben",
            Self::Bestaetigt { .. } => "Bestaetigt",
            Self::Klaerfall { .. } => "Klaerfall",
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

    /// The process's record, once something was received.
    #[must_use]
    pub fn akte(&self) -> Option<&AbrufAkte> {
        match self.offen() {
            Self::New | Self::FristAbgelaufen { .. } => None,
            Self::EntscheidungOffen(a)
            | Self::Freigegeben(a)
            | Self::Bestaetigt { akte: a, .. }
            | Self::Klaerfall { akte: a, .. } => Some(a),
        }
    }
}

/// Events of an activation at the EIV.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AbrufEvent {
    /// The `ActivationDocument` arrived.
    AbrufEmpfangen(AbrufEingang),
    /// A person released the Rückmeldung zur Umsetzbarkeit and it was sent.
    RueckmeldungGesendet(Freigabe),
    /// The DP's acknowledgement of the Rückmeldung arrived.
    QuittungEmpfangen {
        /// Its identification.
        ack_mrid: String,
        /// `true` for `A01`.
        angenommen: bool,
        /// Its reasons.
        gruende: Vec<Quittungsgrund>,
    },
    /// The DP's acknowledgement of the Rückmeldung did not arrive in time.
    QuittungAusgeblieben {
        /// The deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
    /// The answer window lapsed.
    DeadlineExpired {
        /// The deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

impl EventPayload for AbrufEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AbrufEmpfangen(_) => "EivAbrufEmpfangen",
            Self::RueckmeldungGesendet(_) => "EivAbrufRueckmeldungGesendet",
            Self::QuittungEmpfangen { .. } => "EivAbrufQuittungEmpfangen",
            Self::QuittungAusgeblieben { .. } => "EivAbrufQuittungAusgeblieben",
            Self::DeadlineExpired { .. } => "EivAbrufDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        let (richtung, mrid) = match self {
            Self::AbrufEmpfangen(e) => (Richtung::Eingang, &e.mrid),
            Self::RueckmeldungGesendet(f) => (Richtung::Ausgang, &f.rueckmeldung_mrid),
            Self::QuittungEmpfangen { ack_mrid, .. } => (Richtung::Eingang, ack_mrid),
            Self::QuittungAusgeblieben { .. } | Self::DeadlineExpired { .. } => return None,
        };
        Some(Verlauf {
            richtung,
            pid: None,
            message_ref: Some(mrid.clone()),
        })
    }
}

/// Commands of an activation at the EIV.
#[derive(Debug, Clone)]
pub enum AbrufCommand {
    /// The `ActivationDocument` arrived.
    Empfangen(AbrufEingang),
    /// A person released the Rückmeldung zur Umsetzbarkeit. The only command
    /// that answers an activation; the host builds it from a person's request
    /// and never from a timer.
    Freigeben(Freigabe),
    /// The DP's acknowledgement of the Rückmeldung arrived.
    QuittungEmpfangen {
        /// Its identification.
        ack_mrid: String,
        /// Its `ReceivingDocumentIdentification`.
        quittierte_mrid: String,
        /// `true` for `A01`.
        angenommen: bool,
        /// Its reasons.
        gruende: Vec<Quittungsgrund>,
    },
    /// A deadline fired.
    TimeoutExpired {
        /// The deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

impl CommandPayload for AbrufCommand {}

fn abruf_apply(state: AbrufState, event: &AbrufEvent) -> AbrufState {
    // A lapsed window leaves the process waiting where it was; the next event
    // moves it on from there.
    let state = match state {
        AbrufState::FristAbgelaufen { offen, .. }
            if !matches!(event, AbrufEvent::DeadlineExpired { .. }) =>
        {
            *offen
        }
        other => other,
    };
    match event {
        AbrufEvent::AbrufEmpfangen(eingang) => match state {
            AbrufState::New => AbrufState::EntscheidungOffen(AbrufAkte {
                eingang: eingang.clone(),
                freigabe: None,
            }),
            other => other,
        },
        AbrufEvent::RueckmeldungGesendet(freigabe) => match state {
            AbrufState::EntscheidungOffen(mut akte) => {
                akte.freigabe = Some(freigabe.clone());
                AbrufState::Freigegeben(akte)
            }
            other => other,
        },
        AbrufEvent::QuittungEmpfangen {
            ack_mrid,
            angenommen,
            gruende,
        } => match state {
            AbrufState::Freigegeben(akte)
            | AbrufState::Klaerfall {
                akte,
                klaerung: Klaerung::KeineQuittung { .. },
            } => {
                if *angenommen {
                    AbrufState::Bestaetigt {
                        akte,
                        ack_mrid: ack_mrid.clone(),
                    }
                } else {
                    AbrufState::Klaerfall {
                        akte,
                        klaerung: Klaerung::Abgelehnt {
                            ack_mrid: ack_mrid.clone(),
                            gruende: gruende.clone(),
                        },
                    }
                }
            }
            other => other,
        },
        AbrufEvent::QuittungAusgeblieben { label, .. } => match state {
            AbrufState::Freigegeben(akte) => AbrufState::Klaerfall {
                akte,
                klaerung: Klaerung::KeineQuittung {
                    label: label.to_string(),
                },
            },
            other => other,
        },
        AbrufEvent::DeadlineExpired { label, .. } => {
            let offen = match state {
                AbrufState::FristAbgelaufen { offen, .. } => *offen,
                other => other,
            };
            AbrufState::FristAbgelaufen {
                label: label.to_string(),
                offen: Box::new(offen),
            }
        }
    }
}

const RUECKMELDUNG: &str = "ActivationDocument (Rückmeldung zur Umsetzbarkeit)";

fn rueckmeldung_sendung(akte: &AbrufAkte, freigabe: &Freigabe) -> Sendung {
    Sendung {
        mrid: freigabe.rueckmeldung_mrid.clone(),
        dokumenttyp: abruf::RUECKMELDUNG_DOKUMENTTYP.to_owned(),
        sender: akte.eingang.empfaenger.clone(),
        empfaenger: akte.eingang.sender.clone(),
        gesendet_at: freigabe.freigegeben_at.clone(),
    }
}

/// The DP's acknowledgement of the released Rückmeldung.
fn abruf_quittung(
    state: &AbrufState,
    ack_mrid: String,
    quittierte_mrid: &str,
    angenommen: bool,
    gruende: Vec<Quittungsgrund>,
) -> Result<WorkflowOutput<AbrufEvent>, WorkflowError> {
    let Some((akte, freigabe)) = state
        .akte()
        .and_then(|a| a.freigabe.as_ref().map(|f| (a, f)))
    else {
        return Err(WorkflowError::rejected(format!(
            "acknowledgement {ack_mrid:?} before any Rückmeldung was sent"
        )));
    };
    if freigabe.rueckmeldung_mrid != quittierte_mrid {
        return Err(WorkflowError::rejected(format!(
            "acknowledgement {ack_mrid:?} answers {quittierte_mrid:?}, not the Rückmeldung {:?}",
            freigabe.rueckmeldung_mrid
        )));
    }
    match state {
        AbrufState::Freigegeben(_)
        | AbrufState::Klaerfall {
            klaerung: Klaerung::KeineQuittung { .. },
            ..
        } => {
            let event = AbrufEvent::QuittungEmpfangen {
                ack_mrid: ack_mrid.clone(),
                angenommen,
                gruende: gruende.clone(),
            };
            if angenommen {
                return Ok(vec![event].into());
            }
            let klaerung = Klaerung::Abgelehnt { ack_mrid, gruende };
            Ok(WorkflowOutput::with_outbox(
                vec![event],
                vec![klaerfall_notice(
                    RUECKMELDUNG,
                    &rueckmeldung_sendung(akte, freigabe),
                    &klaerung,
                )],
            ))
        }
        _ => Ok(vec![].into()),
    }
}

fn abruf_handle(
    lapsed: &AbrufState,
    command: AbrufCommand,
) -> Result<WorkflowOutput<AbrufEvent>, WorkflowError> {
    let state = lapsed.offen();
    match command {
        AbrufCommand::Empfangen(eingang) => match state {
            AbrufState::New => {
                if eingang.mrid.trim().is_empty() {
                    return Err(WorkflowError::rejected(
                        "an activation without its DocumentIdentification cannot be decided",
                    ));
                }
                Ok(vec![AbrufEvent::AbrufEmpfangen(eingang)].into())
            }
            // A repeat of the same file is the same activation.
            other if other.akte().is_some_and(|a| a.eingang.mrid == eingang.mrid) => {
                Ok(vec![].into())
            }
            _ => Err(WorkflowError::rejected(format!(
                "this process already carries activation {:?}",
                state
                    .akte()
                    .map(|a| a.eingang.mrid.as_str())
                    .unwrap_or_default()
            ))),
        },

        AbrufCommand::Freigeben(freigabe) => match state {
            AbrufState::EntscheidungOffen(_) => {
                if freigabe.freigegeben_von.trim().is_empty() {
                    return Err(WorkflowError::rejected(
                        "an activation is answered only on a named person's release",
                    ));
                }
                if freigabe.rueckmeldung_mrid.trim().is_empty() {
                    return Err(WorkflowError::rejected(
                        "the release names no Rückmeldung zur Umsetzbarkeit",
                    ));
                }
                Ok(vec![AbrufEvent::RueckmeldungGesendet(freigabe)].into())
            }
            AbrufState::Freigegeben(a) | AbrufState::Bestaetigt { akte: a, .. }
                if a.freigabe
                    .as_ref()
                    .is_some_and(|f| f.rueckmeldung_mrid == freigabe.rueckmeldung_mrid) =>
            {
                Ok(vec![].into())
            }
            other => Err(WorkflowError::rejected(format!(
                "a Rückmeldung is released only while the decision is open, not in {}",
                other.label()
            ))),
        },

        AbrufCommand::QuittungEmpfangen {
            ack_mrid,
            quittierte_mrid,
            angenommen,
            gruende,
        } => abruf_quittung(state, ack_mrid, &quittierte_mrid, angenommen, gruende),

        AbrufCommand::TimeoutExpired { deadline_id, label } => {
            if !abruf::window_open(&label, lapsed) {
                return Ok(vec![].into());
            }
            if &*label == abruf::RUECKMELDUNG_ACK_WINDOW_LABEL {
                let Some((akte, freigabe)) = state
                    .akte()
                    .and_then(|a| a.freigabe.as_ref().map(|f| (a, f)))
                else {
                    return Ok(vec![].into());
                };
                let klaerung = Klaerung::KeineQuittung {
                    label: label.to_string(),
                };
                return Ok(WorkflowOutput::with_outbox(
                    vec![AbrufEvent::QuittungAusgeblieben { deadline_id, label }],
                    vec![klaerfall_notice(
                        RUECKMELDUNG,
                        &rueckmeldung_sendung(akte, freigabe),
                        &klaerung,
                    )],
                ));
            }
            Ok(vec![AbrufEvent::DeadlineExpired { deadline_id, label }].into())
        }
    }
}

/// The EIV's side of an activation: receive it, hold it for a person's
/// decision, send the released Rückmeldung zur Umsetzbarkeit and track its
/// acknowledgement.
pub struct AbrufWorkflow;

impl AbrufWorkflow {
    /// The name this workflow's processes are spawned and resumed under.
    pub const WORKFLOW_NAME: &'static str = abruf::WORKFLOW_NAME;
}

impl Workflow for AbrufWorkflow {
    type State = AbrufState;
    type Event = AbrufEvent;
    type Command = AbrufCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[
        abruf::ANTWORTFENSTER_LABEL,
        abruf::RUECKMELDUNG_ACK_WINDOW_LABEL,
    ];
    const FRIST_LABELS: &'static [&'static str] = &[
        abruf::ANTWORTFENSTER_LABEL,
        abruf::RUECKMELDUNG_ACK_WINDOW_LABEL,
    ];

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        abruf::window_open(deadline.label(), state).then(|| AbrufCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        abruf_apply(state, event)
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        abruf_handle(state, command)
    }
}

/// Every EIV workflow name, in [`crate::EivModule`]'s registration order.
pub const WORKFLOW_NAMES: &[&str] = &[
    planungsdaten::WORKFLOW_NAME,
    nichtbeanspruchbarkeit::WORKFLOW_NAME,
    stammdaten::WORKFLOW_NAME,
    kostenblatt::WORKFLOW_NAME,
    abruf::WORKFLOW_NAME,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn sendung(typ: &str) -> Sendung {
        Sendung {
            mrid: "PRSD-1".into(),
            dokumenttyp: typ.into(),
            sender: "9900000000003".into(),
            empfaenger: "9900000000010".into(),
            gesendet_at: "2026-11-21T10:00:00Z".into(),
        }
    }

    fn run<W: Workflow>(state: W::State, cmd: W::Command) -> (W::State, WorkflowOutput<W::Event>) {
        let out = W::handle(&state, cmd).expect("accepted");
        let next = out.events.iter().fold(state, |s, e| W::apply(s, e));
        (next, out)
    }

    fn gesendet() -> SendungState {
        run::<PlanungsdatenSendungWorkflow>(
            SendungState::New,
            SendungCommand::Senden(sendung("A14")),
        )
        .0
    }

    fn quittung(angenommen: bool) -> SendungCommand {
        SendungCommand::QuittungEmpfangen {
            ack_mrid: "ACK-1".into(),
            quittierte_mrid: "PRSD-1".into(),
            angenommen,
            gruende: if angenommen {
                vec![Quittungsgrund {
                    code: "A01".into(),
                    text: None,
                }]
            } else {
                vec![
                    Quittungsgrund {
                        code: "A02".into(),
                        text: None,
                    },
                    Quittungsgrund {
                        code: "Z12".into(),
                        text: Some("XSD".into()),
                    },
                ]
            },
        }
    }

    #[test]
    fn a_positive_acknowledgement_records_the_sending() {
        let (state, out) = run::<PlanungsdatenSendungWorkflow>(gesendet(), quittung(true));
        assert!(
            matches!(state, SendungState::Bestaetigt { ref ack_mrid, .. } if ack_mrid == "ACK-1")
        );
        assert!(out.outbox.is_empty());
    }

    #[test]
    fn a_negative_acknowledgement_opens_a_klaerfall_naming_document_and_reason() {
        let (state, out) = run::<PlanungsdatenSendungWorkflow>(gesendet(), quittung(false));
        assert!(matches!(
            state,
            SendungState::Klaerfall {
                klaerung: Klaerung::Abgelehnt { .. },
                ..
            }
        ));
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(&*out.outbox[0].message_type, PROCESS_FAILED);
        let reason = out.outbox[0].payload["reason"].as_str().unwrap();
        assert!(
            reason.contains("PRSD-1") && reason.contains("Z12"),
            "{reason}"
        );
    }

    #[test]
    fn a_missing_acknowledgement_is_a_klaerfall_not_a_silent_success() {
        let deadline_id = DeadlineId::new();
        let (state, out) = run::<PlanungsdatenSendungWorkflow>(
            gesendet(),
            SendungCommand::TimeoutExpired {
                deadline_id,
                label: planungsdaten::ACK_WINDOW_LABEL.into(),
            },
        );
        assert!(matches!(
            state,
            SendungState::Klaerfall {
                klaerung: Klaerung::KeineQuittung { .. },
                ..
            }
        ));
        assert_eq!(out.outbox.len(), 1);
        // A late positive acknowledgement is still the receiver's answer.
        let (state, _) = run::<PlanungsdatenSendungWorkflow>(state, quittung(true));
        assert!(matches!(state, SendungState::Bestaetigt { .. }));
    }

    #[test]
    fn the_window_closes_on_the_acknowledgement() {
        let (state, _) = run::<PlanungsdatenSendungWorkflow>(gesendet(), quittung(true));
        let out = PlanungsdatenSendungWorkflow::handle(
            &state,
            SendungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: planungsdaten::ACK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty());
    }

    #[test]
    fn an_acknowledgement_for_another_document_is_refused() {
        let cmd = SendungCommand::QuittungEmpfangen {
            ack_mrid: "ACK-9".into(),
            quittierte_mrid: "OTHER".into(),
            angenommen: true,
            gruende: vec![],
        };
        assert!(PlanungsdatenSendungWorkflow::handle(&gesendet(), cmd).is_err());
    }

    #[test]
    fn each_workflow_sends_only_its_own_document_types() {
        assert!(
            PlanungsdatenSendungWorkflow::handle(
                &SendungState::New,
                SendungCommand::Senden(sendung("A76"))
            )
            .is_err()
        );
        assert!(
            NichtbeanspruchbarkeitSendungWorkflow::handle(
                &SendungState::New,
                SendungCommand::Senden(sendung("A76"))
            )
            .is_ok()
        );
        assert!(
            StammdatenSendungWorkflow::handle(
                &SendungState::New,
                SendungCommand::Senden(sendung("Z02"))
            )
            .is_ok()
        );
        assert!(
            KostenblattSendungWorkflow::handle(
                &SendungState::New,
                SendungCommand::Senden(sendung("Z05"))
            )
            .is_ok()
        );
    }

    fn eingang() -> AbrufEingang {
        AbrufEingang {
            mrid: "ACO-1".into(),
            sender: "9900000000010".into(),
            empfaenger: "9900000000003".into(),
            ressourcen: vec!["C1234567890".into()],
            zeitraum: "2026-11-21T10:00Z/2026-11-21T11:00Z".into(),
            eingegangen_at: "2026-11-21T09:30:00Z".into(),
        }
    }

    fn freigabe(von: &str) -> Freigabe {
        Freigabe {
            rueckmeldung_mrid: "ACR-1".into(),
            freigegeben_von: von.into(),
            freigegeben_at: "2026-11-21T09:35:00Z".into(),
        }
    }

    fn empfangen() -> AbrufState {
        run::<AbrufWorkflow>(AbrufState::New, AbrufCommand::Empfangen(eingang())).0
    }

    /// The invariant: receipt alone answers nothing. The activation waits for
    /// a person, and every deadline that can fire before the decision leaves
    /// it waiting — none of them sends a Rückmeldung.
    #[test]
    fn an_activation_is_never_answered_without_a_person() {
        let (state, out) =
            run::<AbrufWorkflow>(AbrufState::New, AbrufCommand::Empfangen(eingang()));
        assert!(matches!(state, AbrufState::EntscheidungOffen(_)));
        assert!(out.outbox.is_empty());
        for label in AbrufWorkflow::DEADLINE_LABELS {
            let out = AbrufWorkflow::handle(
                &state,
                AbrufCommand::TimeoutExpired {
                    deadline_id: DeadlineId::new(),
                    label: (*label).into(),
                },
            )
            .unwrap();
            assert!(
                !out.events
                    .iter()
                    .any(|e| matches!(e, AbrufEvent::RueckmeldungGesendet(_))),
                "{label} answered the activation"
            );
            assert!(out.outbox.is_empty(), "{label} sent something");
        }
    }

    #[test]
    fn a_lapsed_answer_window_records_the_lapse_and_still_takes_the_decision() {
        let (state, _) = run::<AbrufWorkflow>(
            empfangen(),
            AbrufCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: abruf::ANTWORTFENSTER_LABEL.into(),
            },
        );
        assert!(matches!(state, AbrufState::FristAbgelaufen { .. }));
        let (state, _) =
            run::<AbrufWorkflow>(state, AbrufCommand::Freigeben(freigabe("disponent")));
        assert!(matches!(state, AbrufState::Freigegeben(_)));
    }

    #[test]
    fn a_release_without_a_person_is_refused() {
        assert!(
            AbrufWorkflow::handle(&empfangen(), AbrufCommand::Freigeben(freigabe(" "))).is_err()
        );
    }

    #[test]
    fn the_released_rueckmeldung_is_tracked_to_its_acknowledgement() {
        let (state, _) =
            run::<AbrufWorkflow>(empfangen(), AbrufCommand::Freigeben(freigabe("disponent")));
        let (state, out) = run::<AbrufWorkflow>(
            state,
            AbrufCommand::QuittungEmpfangen {
                ack_mrid: "ACK-2".into(),
                quittierte_mrid: "ACR-1".into(),
                angenommen: false,
                gruende: vec![Quittungsgrund {
                    code: "Z16".into(),
                    text: None,
                }],
            },
        );
        assert!(matches!(state, AbrufState::Klaerfall { .. }));
        assert_eq!(out.outbox.len(), 1);
    }

    #[test]
    fn only_a27_is_the_eiv() {
        assert!(an_eiv_adressiert("A27"));
        for other in ["A18", "A39", "A21", "A08", "Z01", ""] {
            assert!(!an_eiv_adressiert(other), "{other}");
        }
    }
}
