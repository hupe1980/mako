//! GPKE Anfrage zur Beendigung der Zuordnung — NB-initiated Abmeldeanfrage.
//!
//! GPKE Teil 2: the Netzbetreiber asks the current Lieferant (LFA) to end the
//! Zuordnung of a Marktlokation (e.g. after a Netzbetreiberwechsel or when the
//! NB's records require the assignment to end). The LFA confirms (55011) or
//! rejects (55012). This mirrors the GeLi Gas Abmeldungsanfrage (44010–44012).
//!
//! This module implements **both ends** of the exchange, distinguished by which
//! command opens the process:
//!
//! - **NB initiator** — [`BeendigungZuordnungCommand::Anfragen`] renders the
//!   55010, registers the LFA's 09:00 window, and
//!   [`BeendigungZuordnungCommand::ReceiveAntwort`] takes the 55011/55012 back.
//! - **LFA responder** — [`BeendigungZuordnungCommand::ReceiveAnfrage`] takes
//!   the inbound 55010 and [`BeendigungZuordnungCommand::SendAntwort`] answers
//!   it.
//!
//! # Why the NB side matters
//!
//! The Anfrage is not optional courtesy. GPKE Teil 2 § 2.1.2 SD Lieferbeginn
//! Nr. 1 **Prüfschritt 4** routes an Anmeldung on an already-assigned
//! Marktlokation to Prozessschritt 3, and `E_0623` Prüfschritte 20–50 read the
//! answer: a Widerspruch that is not `A30` refuses the Anmeldung with `A50`.
//! An NB that skips the Anfrage cannot reach that outcome at all — it confirms
//! every Lieferantenwechsel without consulting the incumbent.
//!
//! **Silence is a result, not a timeout.** „Verstreicht die Frist, ohne dass
//! eine Antwort beim NB eingeht, gilt dies als Bestätigung nach Fall a). Nach
//! Ablauf der Frist eingehende Antworten sind für den Fortlauf dieses Prozesses
//! unerheblich." So the 09:00 deadline **completes** the process rather than
//! failing it, and a late 55011/55012 is recorded and ignored.
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom 2.1/2.2)
//!
//! ## Inbound (NB → LFA)
//!
//! | PID   | Process name (AHB)                            | Direction |
//! |-------|-----------------------------------------------|-----------|
//! | 55010 | Anfrage zur Beendigung der Zuordnung (NB→LFA) | NB → LFA  |
//!
//! ## Outbound (LFA → NB)
//!
//! | PID   | Process name (AHB)                            | Derived from   |
//! |-------|-----------------------------------------------|----------------|
//! | 55011 | Bestätigung Beendigung der Zuordnung (LFA→NB) | 55010 accepted |
//! | 55012 | Ablehnung Beendigung der Zuordnung (LFA→NB)   | 55010 rejected |
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE Teil 2** — Beendigung der Zuordnung
//! - **UTILMD S2.1/S2.2** — EDI@Energy message format
//! - **APERAK AHB 1.0 § 2.4.1** — technical acknowledgement, **45 Minuten** on a
//!   weekday for a UTILMD. A separate clock from the business answer window,
//!   which GPKE Teil 2 states as a wall-clock instant on the 1. Werktag nach
//!   dem ÜT.

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-beendigung-zuordnung";

/// Inbound PIDs handled by [`GpkeBeendigungZuordnungWorkflow`].
///
/// | PID   | Process (AHB name)                            | AHB profile  |
/// |-------|-----------------------------------------------|--------------|
/// | 55010 | Anfrage zur Beendigung der Zuordnung (NB→LFA) | S2.1–S2.2 ✅ |
/// | 55011 | Bestätigung Beendigung der Zuordnung (LFA→NB) | S2.1–S2.2 ✅ |
/// | 55012 | Ablehnung Beendigung der Zuordnung (LFA→NB)   | S2.1–S2.2 ✅ |
///
/// 55011 / 55012 are registered because the **NB** sends the Anfrage and
/// has to take the answer back. They were outbound-only while only the LFA side
/// existed, so an inbound one was dead-lettered as `UnknownPid`.
pub const BEENDIGUNG_ZUORDNUNG_PIDS: &[u32] = &[55010, 55011, 55012];

/// The Anfrage the NB sends (NB → LFA).
pub const ANFRAGE_PID: u32 = 55_010;

/// The answers the LFA sends back (LFA → NB).
pub const ANTWORT_PIDS: &[u32] = &[55_011, 55_012];

/// Deadline label for the **LFA's** answer window, as the NB tracks it —
/// 09:00 Uhr des 1. WT nach dem ÜT der Anmeldung (GPKE Teil 2 § 2.1.2 Nr. 4).
///
/// Its own label, distinct from
/// [`BEENDIGUNG_ZUORDNUNG_ANTWORT_WINDOW_LABEL`]: that one is the LFA's clock on
/// its own obligation to answer, and expiry there means *this deployment* was
/// late. Expiry here means the counterparty was, which the Festlegung turns into
/// a Zustimmung rather than a failure — opposite consequences, so they must not
/// share a label.
pub const NB_ANFRAGE_WINDOW_LABEL: &str = "gpke-beendigung-zuordnung-lfa-antwort";

/// Deadline label for the **business** answer window — 09:00 Uhr des 1. WT nach
/// dem ÜT (GPKE Teil 2 § 2.1.2), resolved by `mako_fristen::antwort`.
///
/// Not the APERAK clock: that is 45 Minuten for a UTILMD (APERAK AHB 1.0
/// § 2.4.1) and rides `mako_fristen::APERAK_STROM_WINDOW_LABEL`.
pub const BEENDIGUNG_ZUORDNUNG_ANTWORT_WINDOW_LABEL: &str =
    "gpke-beendigung-zuordnung-antwortfrist";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Beendigung-der-Zuordnung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum BeendigungZuordnungEvent {
    /// PID 55010 Anfrage zur Beendigung der Zuordnung received.
    AnfrageErhalten {
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// GLN of the sending NB.
        sender: MarktpartnerCode,
        /// GLN of the receiving LFA.
        receiver: MarktpartnerCode,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Requested Zuordnungsende date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (55010).
        pruefidentifikator: Pruefidentifikator,
        /// `SG4 IDE+24` DE 7402 — carried into the answer's `SG4 RFF+TN`.
        vorgangsnummer: Option<String>,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound response (55011 or 55012) dispatched to the NB.
    AntwortGesendet {
        /// Response PID actually dispatched.
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung, `false` = Ablehnung — read from the
        /// Antwortcode's published Cluster, never supplied separately.
        accepted: bool,
        /// The answer as sent, kept for the audit trail.
        antwort: crate::lf_antwort::LfAntwort,
    },
    /// Zuordnung ended per the NB Anfrage.
    Beendet,
    /// APERAK 29001 dispatched for technical failure.
    AperakFehlerDispatched {
        /// APERAK PID.
        aperak_pid: Pruefidentifikator,
        /// Error reason.
        reason: String,
        /// Outbound APERAK message reference.
        outbound_ref: MessageRef,
    },
    /// Process rejected due to validation failure or deadline expiry.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired.
    DeadlineExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
    /// **NB side.** The Anfrage zur Beendigung der Zuordnung (55010) was
    /// rendered and queued for the LFA.
    AnfrageGesendet {
        /// Marktlokation or Tranche the Anfrage is about.
        location_id: MaLo,
        /// The NB.
        sender: MarktpartnerCode,
        /// The LFA being asked to release it.
        receiver: MarktpartnerCode,
        /// The Zuordnungsende requested — the Zuordnungsbeginn of the LFN's
        /// Anmeldung (SD Lieferbeginn Nr. 3).
        process_date: String,
        /// `SG4 IDE+24` of the outbound Anfrage.
        vorgangsnummer: String,
        /// The Anmeldung this Anfrage serves, so `processd` can resume the
        /// right decision when the answer lands.
        anmeldung_process_id: String,
    },
    /// **NB side.** The LFA answered (55011 / 55012), or the 09:00 window
    /// lapsed and the Festlegung answered for it.
    LfaAntwortErhalten {
        /// 55011 or 55012; `None` when the window lapsed unanswered.
        response_pid: Option<Pruefidentifikator>,
        /// The `E_0624` Antwortcode, `None` on silence.
        antwortcode: Option<String>,
        /// `true` for a Zustimmung — including the one the Festlegung infers
        /// from silence („gilt dies als Bestätigung nach Fall a)").
        zustimmung: bool,
        /// „Hierbei übermittelt der LFA eine Begründung für den Widerspruch."
        grund: Option<String>,
        /// **Fall b** — the Zuordnungsende the LFA confirmed, when earlier than
        /// the one asked for.
        zuordnungsende: Option<String>,
        /// `true` when no answer arrived before the window closed.
        fristablauf: bool,
    },
}

impl EventPayload for BeendigungZuordnungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageErhalten { .. } => "BeendigungZuordnungAnfrageErhalten",
            Self::ValidationPassed { .. } => "BeendigungZuordnungValidationPassed",
            Self::AntwortGesendet { .. } => "BeendigungZuordnungAntwortGesendet",
            Self::Beendet => "BeendigungZuordnungBeendet",
            Self::AperakFehlerDispatched { .. } => "BeendigungZuordnungAperakFehlerDispatched",
            Self::Rejected { .. } => "BeendigungZuordnungRejected",
            Self::DeadlineExpired { .. } => "BeendigungZuordnungDeadlineExpired",
            Self::AnfrageGesendet { .. } => "BeendigungZuordnungAnfrageGesendet",
            Self::LfaAntwortErhalten { .. } => "BeendigungZuordnungLfaAntwortErhalten",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `AnfrageErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeendigungZuordnungData {
    /// EIC/MaLo code.
    pub location_id: MaLo,
    /// GLN of the NB who initiated the request.
    pub sender: MarktpartnerCode,
    /// GLN of the affected LFA.
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// Requested Zuordnungsende date (`YYYYMMDD`).
    pub process_date: String,
    /// BDEW Prüfidentifikator (55010).
    pub pruefidentifikator: Pruefidentifikator,
    /// `SG4 IDE+24` DE 7402 of the **request**.
    ///
    /// Retained because the answer must carry it back in `SG4 RFF+TN`
    /// („Referenz Vorgangsnummer (aus Anfragenachricht)", Muss on every
    /// Antwortnachricht). It is never reused as the answer's own `IDE+24`.
    pub vorgangsnummer: Option<String>,
}

/// State of a GPKE Beendigung-der-Zuordnung process.
///
/// ```text
/// New → Eingegangen → ValidationPassed → AntwortGesendet → Beendet
///                                       ↘ Rejected
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum BeendigungZuordnungState {
    /// No events yet.
    #[default]
    New,
    /// Anfrage received.
    Eingegangen(BeendigungZuordnungData),
    /// Validation passed; response not yet sent.
    ValidationPassed(BeendigungZuordnungData),
    /// Response dispatched; awaiting Zuordnungsende confirmation.
    AntwortGesendet {
        /// Data from the Anfrage.
        data: BeendigungZuordnungData,
        /// Response PID sent (55011 or 55012).
        response_pid: Pruefidentifikator,
    },
    /// Zuordnung ended.
    Beendet(BeendigungZuordnungData),
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// **NB side.** The Anfrage is out and the LFA's 09:00 window is running.
    AnfrageGesendet {
        /// What went out.
        data: BeendigungZuordnungData,
        /// The Anmeldung this Anfrage serves.
        anmeldung_process_id: String,
    },
    /// **NB side.** Terminal: the LFA answered, or the window lapsed and the
    /// Festlegung answered for it.
    LfaAntwort {
        /// What went out.
        data: BeendigungZuordnungData,
        /// The Anmeldung this Anfrage serves.
        anmeldung_process_id: String,
        /// `true` for a Zustimmung, silence included.
        zustimmung: bool,
        /// The `E_0624` code, `None` on silence.
        antwortcode: Option<String>,
    },
}

impl BeendigungZuordnungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AntwortGesendet { .. } => "AntwortGesendet",
            Self::Beendet(_) => "Beendet",
            Self::Rejected { .. } => "Rejected",
            Self::AnfrageGesendet { .. } => "AnfrageGesendet",
            Self::LfaAntwort { .. } => "LfaAntwort",
        }
    }

    /// Return `Some(&BeendigungZuordnungData)` if the process has been initiated.
    #[must_use]
    pub fn data(&self) -> Option<&BeendigungZuordnungData> {
        match self {
            Self::Eingegangen(d) | Self::ValidationPassed(d) | Self::Beendet(d) => Some(d),
            Self::AntwortGesendet { data, .. }
            | Self::AnfrageGesendet { data, .. }
            | Self::LfaAntwort { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Beendigung-der-Zuordnung workflow.
#[derive(Clone)]
pub enum BeendigungZuordnungCommand {
    /// Inbound UTILMD PID 55010 Anfrage received from the AS4 layer.
    ReceiveAnfrage {
        /// BDEW Prüfidentifikator (55010).
        pid: Pruefidentifikator,
        /// GLN of the NB.
        sender: MarktpartnerCode,
        /// GLN of the LFA.
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Requested Zuordnungsende date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// The `SG4` facts the trees branch on, forwarded to `processd` on
        /// the `de.mako.process.initiated` notification.
        vorgang: Box<crate::lf_antwort::LfVorgangsdaten>,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
    },
    /// Send the outbound UTILMD response (55011 = Bestätigung, 55012 = Ablehnung).
    ///
    /// The LFA answers by **09:00 Uhr des 1. WT nach dem ÜT** — GPKE Teil 2
    /// § 2.1.2 SD Lieferbeginn Prozessschritt 4, resolved by
    /// `mako_fristen::antwort` (trigger PID 55010). It is a clock time on a
    /// Werktag, not a 24-hour duration.
    SendAntwort {
        /// The resolved answer: Antwortcode, its EBD, and the Cluster that
        /// selects the response PID.
        antwort: crate::lf_antwort::LfAntwort,
    },
    /// **NB side.** Render and queue the Anfrage zur Beendigung der Zuordnung
    /// (55010) and start the LFA's 09:00 window.
    ///
    /// GPKE Teil 2 § 2.1.2 Nr. 3, „parallel zu Nr. 2". Issued by `processd`
    /// when `mako_pruefung` answers
    /// `NbEntscheidung::AnfrageErforderlich` — `mako-gpke` is a transport-layer
    /// crate and does not depend on `mako-pruefung`, so the link is a name.
    Anfragen {
        /// The NB's own MP-ID.
        sender: MarktpartnerCode,
        /// The LFA to ask.
        receiver: MarktpartnerCode,
        /// Marktlokations-ID, or the MaLo-ID of the Tranche.
        location_id: MaLo,
        /// `SG5 LOC+Z21` instead of `LOC+Z16`.
        tranche: bool,
        /// The Zuordnungsende to request, `YYYYMMDD`.
        process_date: String,
        /// `SG4 IDE+24` of the outbound Anfrage.
        vorgangsnummer: String,
        /// The Anmeldung this Anfrage serves.
        anmeldung_process_id: String,
        /// The Letztverbraucher, `SG12 NAD+Z09` („Kundenname aus Anmeldung
        /// Lieferant neu", UTILMD AHB Strom Bedingung `[279]`/`[572]`) — Muss
        /// on a verbrauchende oder ruhende Marktlokation.
        kunde_name: Option<String>,
        /// The Neulieferant, `SG12 NAD+VY` (Bedingung `[567]`).
        lfn_mp_id: Option<String>,
    },
    /// **NB side.** The LFA answered the Anfrage (55011 / 55012).
    ReceiveAntwort {
        /// 55011 or 55012.
        response_pid: Pruefidentifikator,
        /// The `E_0624` Antwortcode.
        antwortcode: String,
        /// `true` when the code sits in the Zustimmung cluster.
        zustimmung: bool,
        /// The Begründung a Widerspruch carries.
        grund: Option<String>,
        /// **Fall b** — the Zuordnungsende the LFA confirmed instead.
        zuordnungsende: Option<String>,
    },
    /// **NB side.** The LFA's 09:00 window lapsed unanswered.
    ///
    /// Not a failure: „Verstreicht die Frist, ohne dass eine Antwort beim NB
    /// eingeht, gilt dies als Bestätigung nach Fall a). Nach Ablauf der Frist
    /// eingehende Antworten sind für den Fortlauf dieses Prozesses
    /// unerheblich." The Festlegung supplies the answer.
    AntwortfristAbgelaufen,
    /// Record that the Zuordnung has ended.
    BeendenBestaetigen,
    /// Dispatch APERAK 29001 for technical processing failure.
    DispatchAperakFehler {
        /// Error reason.
        reason: String,
        /// Outbound APERAK message reference.
        outbound_ref: MessageRef,
    },
    /// A registered deadline fired; close the process.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for BeendigungZuordnungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Anfrage-zur-Beendigung-der-Zuordnung workflow (55010 inbound, 55011/55012
/// outbound).
pub struct GpkeBeendigungZuordnungWorkflow;

impl Workflow for GpkeBeendigungZuordnungWorkflow {
    type State = BeendigungZuordnungState;
    type Event = BeendigungZuordnungEvent;
    type Command = BeendigungZuordnungCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                BEENDIGUNG_ZUORDNUNG_ANTWORT_WINDOW_LABEL,
                BeendigungZuordnungState::Eingegangen(_)
                | BeendigungZuordnungState::ValidationPassed(_),
            ) => Some(BeendigungZuordnungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            // The **NB's** window on the LFA. „Verstreicht die Frist, ohne dass
            // eine Antwort beim NB eingeht, gilt dies als Bestätigung nach
            // Fall a)" — so this closes the process with a Zustimmung the
            // Festlegung supplies, and must not take the `TimeoutExpired` arm,
            // which rejects.
            (NB_ANFRAGE_WINDOW_LABEL, BeendigungZuordnungState::AnfrageGesendet { .. }) => {
                Some(BeendigungZuordnungCommand::AntwortfristAbgelaufen)
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            BeendigungZuordnungEvent::AnfrageErhalten {
                location_id,
                sender,
                receiver,
                document_date,
                process_date,
                pruefidentifikator,
                vorgangsnummer,
                ..
            } => BeendigungZuordnungState::Eingegangen(BeendigungZuordnungData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                vorgangsnummer: vorgangsnummer.clone(),
            }),
            BeendigungZuordnungEvent::ValidationPassed { .. } => match state {
                BeendigungZuordnungState::Eingegangen(data) => {
                    BeendigungZuordnungState::ValidationPassed(data)
                }
                other => other,
            },
            BeendigungZuordnungEvent::AntwortGesendet {
                accepted,
                response_pid,
                ..
            } => {
                if *accepted {
                    match state {
                        BeendigungZuordnungState::ValidationPassed(data) => {
                            BeendigungZuordnungState::AntwortGesendet {
                                response_pid: *response_pid,
                                data,
                            }
                        }
                        other => other,
                    }
                } else {
                    BeendigungZuordnungState::Rejected {
                        reason: "Anfrage abgelehnt".to_owned(),
                    }
                }
            }
            BeendigungZuordnungEvent::Beendet => match state {
                BeendigungZuordnungState::AntwortGesendet { data, .. } => {
                    BeendigungZuordnungState::Beendet(data)
                }
                other => other,
            },
            BeendigungZuordnungEvent::AperakFehlerDispatched { reason, .. } => {
                BeendigungZuordnungState::Rejected {
                    reason: format!("APERAK 29001: {reason}"),
                }
            }
            BeendigungZuordnungEvent::AnfrageGesendet {
                location_id,
                sender,
                receiver,
                process_date,
                vorgangsnummer,
                anmeldung_process_id,
            } => BeendigungZuordnungState::AnfrageGesendet {
                data: BeendigungZuordnungData {
                    location_id: location_id.clone(),
                    sender: sender.clone(),
                    receiver: receiver.clone(),
                    document_date: process_date.clone(),
                    process_date: process_date.clone(),
                    pruefidentifikator: Pruefidentifikator::new(ANFRAGE_PID)
                        .unwrap_or_else(|_| unreachable!("55010 is a valid Prüfidentifikator")),
                    vorgangsnummer: Some(vorgangsnummer.clone()),
                },
                anmeldung_process_id: anmeldung_process_id.clone(),
            },
            BeendigungZuordnungEvent::LfaAntwortErhalten {
                zustimmung,
                antwortcode,
                ..
            } => match state {
                BeendigungZuordnungState::AnfrageGesendet {
                    data,
                    anmeldung_process_id,
                } => BeendigungZuordnungState::LfaAntwort {
                    data,
                    anmeldung_process_id,
                    zustimmung: *zustimmung,
                    antwortcode: antwortcode.clone(),
                },
                other => other,
            },
            BeendigungZuordnungEvent::Rejected { reason } => BeendigungZuordnungState::Rejected {
                reason: reason.clone(),
            },
            BeendigungZuordnungEvent::DeadlineExpired { label, .. } => match state {
                BeendigungZuordnungState::Beendet(_)
                | BeendigungZuordnungState::Rejected { .. } => state,
                _ => BeendigungZuordnungState::Rejected {
                    reason: format!("deadline expired: {label}"),
                },
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            BeendigungZuordnungCommand::ReceiveAnfrage {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                vorgang,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, BeendigungZuordnungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !BEENDIGUNG_ZUORDNUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected Anfrage zur Beendigung der Zuordnung PID (55010), got {pid}",
                    )));
                }
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();
                let notify_malo = location_id.clone();
                let notify_termin = process_date.clone();

                let mut events = vec![BeendigungZuordnungEvent::AnfrageErhalten {
                    location_id,
                    sender,
                    receiver,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                    vorgangsnummer: vorgang.vorgangsnummer.clone(),
                }];
                if validation_passed {
                    events.push(BeendigungZuordnungEvent::ValidationPassed { message_ref });
                    // F-038: APERAK BGM+312 (Anerkennungsmeldung) — APERAK AHB 1.0 §2.4.
                    let outbox = vec![
                        // The business notification. `processd`'s LF module
                        // decides this process, and it only ever sees a message
                        // that reaches the ERP fan-out — an APERAK is a
                        // technical acknowledgement, not one.
                        vorgang
                            .process_initiated(
                                pid,
                                &notify_malo,
                                &sender_mp_id,
                                &receiver_gln,
                                &notify_termin,
                                &serde_json::Value::Null,
                            )
                            .caused_by(1),
                        PendingOutbox::new(
                            "APERAK",
                            sender_mp_id.as_str(),
                            serde_json::json!({
                                "sender":        receiver_gln.as_str(),
                                "receiver":      sender_mp_id.as_str(),
                                "pid":           29001_u32,
                                "document_code": "312",
                            }),
                        )
                        .caused_by(1),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                } else {
                    let reason = validation_errors.join("; ");
                    events.push(BeendigungZuordnungEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // F-035: APERAK BGM+313 — APERAK AHB 1.0 §2.1.1.
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_mp_id.as_str(),
                            serde_json::json!({
                                "sender":     receiver_gln.as_str(),
                                "receiver":   sender_mp_id.as_str(),
                                "pid":        29001_u32,
                                "error_code": mako_engine::erc::codes::Z29,
                                "reason":     reason,
                            }),
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                }
            }

            BeendigungZuordnungCommand::SendAntwort { antwort } => {
                let data = match state {
                    BeendigungZuordnungState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                // The Cluster the Antwortcode sits in decides the PID. A caller
                // cannot pick one independently of the other — that is how an
                // Ablehnungscode could otherwise ride a Bestätigung.
                let accepted = antwort.zustimmung;
                let response_code: u32 = if accepted { 55011 } else { 55012 };
                let response_pid = Pruefidentifikator::new(response_code)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;

                // The outbox entry *is* the answer. Without it the event log
                // recorded the process as answered while the counterparty saw
                // nothing but its Frist expire.
                let outbox = vec![
                    crate::lf_antwort::antwort_outbox(
                        response_code,
                        &antwort,
                        &data.location_id,
                        &data.sender,
                        &data.receiver,
                        &data.process_date,
                        data.vorgangsnummer.as_deref(),
                    )
                    .caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![BeendigungZuordnungEvent::AntwortGesendet {
                        response_pid,
                        accepted,
                        antwort,
                    }],
                    outbox,
                ))
            }

            BeendigungZuordnungCommand::Anfragen {
                sender,
                receiver,
                location_id,
                tranche,
                process_date,
                vorgangsnummer,
                anmeldung_process_id,
                kunde_name,
                lfn_mp_id,
            } => {
                if !matches!(state, BeendigungZuordnungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if process_date.trim().is_empty() {
                    return Err(WorkflowError::rejected(
                        "the Anfrage zur Beendigung der Zuordnung names the Zuordnungsende it \
                         asks for (SG4 DTM+93); UTILMD AHB Strom marks it Muss on 55010"
                            .to_owned(),
                    ));
                }
                // `SG5 LOC+Z21` when the Vorgang is about a Tranche. Both carry
                // a MaLo-ID, so the qualifier is the only thing that says which
                // object the LFA is being asked to release.
                let mut payload = serde_json::json!({
                    "direction":         "outbound",
                    "pid":               ANFRAGE_PID,
                    "sender":            sender.as_str(),
                    "receiver":          receiver.as_str(),
                    "malo":              location_id.as_str(),
                    "process_date":      process_date,
                    "vorgangsnummer":    vorgangsnummer,
                    "document_code":     "E02",
                    "lokationstyp":      if tranche { "Z21" } else { "Z16" },
                });
                let obj = payload.as_object_mut().expect("json! built an object");
                // `SG12 NAD+Z09` „Kunde des LF" — Muss on a verbrauchende oder
                // ruhende Marktlokation (UTILMD AHB Strom Bedingung [279]),
                // „Kundenname aus Anmeldung Lieferant neu" ([572]). It is how
                // the LFA tells an Einzug from a Wechsel, which `E_0624`
                // Prüfschritt 30 branches on.
                if let Some(name) = kunde_name.filter(|n| !n.is_empty()) {
                    obj.insert("kunde_name".into(), name.into());
                }
                // `SG12 NAD+VY` — the Neulieferant (Bedingung [567]).
                if let Some(lfn) = lfn_mp_id.filter(|m| !m.is_empty()) {
                    obj.insert(
                        "beteiligte_marktpartner".into(),
                        serde_json::Value::Array(vec![serde_json::Value::String(lfn)]),
                    );
                }
                let outbox = vec![PendingOutbox::new("UTILMD", receiver.as_str(), payload)];
                Ok(WorkflowOutput::with_outbox(
                    vec![BeendigungZuordnungEvent::AnfrageGesendet {
                        location_id,
                        sender,
                        receiver,
                        process_date,
                        vorgangsnummer,
                        anmeldung_process_id,
                    }],
                    outbox,
                ))
            }

            BeendigungZuordnungCommand::ReceiveAntwort {
                response_pid,
                antwortcode,
                zustimmung,
                grund,
                zuordnungsende,
            } => {
                let BeendigungZuordnungState::AnfrageGesendet {
                    data,
                    anmeldung_process_id,
                } = state
                else {
                    // „Nach Ablauf der Frist eingehende Antworten sind für den
                    // Fortlauf dieses Prozesses unerheblich" — a late answer is
                    // recorded by the ingest layer and changes nothing here.
                    return Err(WorkflowError::invalid_state(
                        "AnfrageGesendet",
                        state.label(),
                    ));
                };
                if !ANTWORT_PIDS.contains(&response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an Antwort auf die Anfrage zur Beendigung der Zuordnung \
                         ({ANTWORT_PIDS:?}), got {response_pid}",
                    )));
                }
                Ok(WorkflowOutput::with_outbox(
                    vec![BeendigungZuordnungEvent::LfaAntwortErhalten {
                        response_pid: Some(response_pid),
                        antwortcode: Some(antwortcode.clone()),
                        zustimmung,
                        grund: grund.clone(),
                        zuordnungsende: zuordnungsende.clone(),
                        fristablauf: false,
                    }],
                    vec![lfa_antwort_notification(
                        data,
                        anmeldung_process_id,
                        Some(&antwortcode),
                        zustimmung,
                        grund.as_deref(),
                        zuordnungsende.as_deref(),
                        false,
                    )],
                ))
            }

            BeendigungZuordnungCommand::AntwortfristAbgelaufen => {
                let BeendigungZuordnungState::AnfrageGesendet {
                    data,
                    anmeldung_process_id,
                } = state
                else {
                    // Already answered — the scheduler and the inbound message
                    // raced, and the answer won.
                    return Ok(vec![].into());
                };
                Ok(WorkflowOutput::with_outbox(
                    vec![BeendigungZuordnungEvent::LfaAntwortErhalten {
                        response_pid: None,
                        antwortcode: None,
                        // „gilt dies als Bestätigung nach Fall a)" — the
                        // Festlegung answers for the LFA, so silence is a
                        // Zustimmung and not an unanswered question.
                        zustimmung: true,
                        grund: None,
                        zuordnungsende: None,
                        fristablauf: true,
                    }],
                    vec![lfa_antwort_notification(
                        data,
                        anmeldung_process_id,
                        None,
                        true,
                        None,
                        None,
                        true,
                    )],
                ))
            }

            BeendigungZuordnungCommand::BeendenBestaetigen => {
                if !matches!(state, BeendigungZuordnungState::AntwortGesendet { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.label(),
                    ));
                }
                Ok(vec![BeendigungZuordnungEvent::Beendet].into())
            }

            BeendigungZuordnungCommand::DispatchAperakFehler {
                reason,
                outbound_ref,
            } => {
                match state {
                    BeendigungZuordnungState::Eingegangen(_)
                    | BeendigungZuordnungState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "Eingegangen or ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                let aperak_pid = Pruefidentifikator::new(29_001)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;
                Ok(vec![BeendigungZuordnungEvent::AperakFehlerDispatched {
                    aperak_pid,
                    reason,
                    outbound_ref,
                }]
                .into())
            }

            BeendigungZuordnungCommand::TimeoutExpired { deadline_id, label } => match state {
                BeendigungZuordnungState::Beendet(_)
                | BeendigungZuordnungState::Rejected { .. } => Ok(vec![].into()),
                _ => Ok(
                    vec![BeendigungZuordnungEvent::DeadlineExpired { deadline_id, label }].into(),
                ),
            },
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// The notification `processd` resumes the Anmeldung decision from.
///
/// A Meldung to our own ERP fan-out, not to the market: the LFA's answer is an
/// input to `E_0623` Prüfschritte 30–50, and the process that has to act on it
/// is the Anmeldung's, not this one.
#[allow(clippy::too_many_arguments)]
fn lfa_antwort_notification(
    data: &BeendigungZuordnungData,
    anmeldung_process_id: &str,
    antwortcode: Option<&str>,
    zustimmung: bool,
    grund: Option<&str>,
    zuordnungsende: Option<&str>,
    fristablauf: bool,
) -> PendingOutbox {
    let mut payload = serde_json::json!({
        "type":                 "LfaAntwortAufAbmeldeanfrage",
        "pid":                  ANFRAGE_PID,
        "malo_id":              data.location_id.as_str(),
        "grid_operator":        data.sender.as_str(),
        "lfa_mp_id":            data.receiver.as_str(),
        "anmeldung_process_id": anmeldung_process_id,
        "zustimmung":           zustimmung,
        "fristablauf":          fristablauf,
    });
    let obj = payload.as_object_mut().expect("json! built an object");
    if let Some(c) = antwortcode {
        obj.insert("antwortcode".into(), c.into());
    }
    if let Some(g) = grund {
        obj.insert("grund".into(), g.into());
    }
    if let Some(ende) = zuordnungsende {
        obj.insert("zuordnungsende".into(), ende.into());
    }
    PendingOutbox::new("LfaAntwortAufAbmeldeanfrage", data.sender.as_str(), payload)
}

#[cfg(test)]
mod tests {
    use mako_engine::workflow::Workflow;

    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).unwrap()
    }
    fn mcod(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn malo(s: &str) -> MaLo {
        MaLo::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    fn anfrage_cmd(ok: bool) -> BeendigungZuordnungCommand {
        BeendigungZuordnungCommand::ReceiveAnfrage {
            pid: pid(55010),
            sender: mcod("9900357000004"),
            receiver: mcod("4012345000023"),
            location_id: malo("51238696781"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            message_ref: mref("BEEND-001"),
            vorgang: Box::new(crate::LfVorgangsdaten::default()),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
        }
    }

    fn apply_all(
        init: BeendigungZuordnungState,
        events: &[BeendigungZuordnungEvent],
    ) -> BeendigungZuordnungState {
        events
            .iter()
            .fold(init, GpkeBeendigungZuordnungWorkflow::apply)
    }

    #[test]
    fn happy_path_bestaetigung() {
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &BeendigungZuordnungState::New,
            anfrage_cmd(true),
        )
        .unwrap();
        // AnfrageErhalten + ValidationPassed events; ProcessInitiated + APERAK 312
        // outbox. Both are load-bearing: the APERAK discharges the 45-minute
        // technical clock, the ProcessInitiated is what puts the Vorgang in
        // front of `processd` (or the ERP) at all.
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[0].message_type.as_ref(), "ProcessInitiated");
        assert_eq!(out.outbox[1].payload["document_code"], "312");
        let state = apply_all(BeendigungZuordnungState::New, &out.events);
        assert!(matches!(
            state,
            BeendigungZuordnungState::ValidationPassed(_)
        ));

        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &state,
            BeendigungZuordnungCommand::SendAntwort {
                antwort: crate::lf_antwort::LfAntwort::zustimmung("A36", "E_0624"),
            },
        )
        .unwrap();
        if let BeendigungZuordnungEvent::AntwortGesendet { response_pid, .. } = &out.events[0] {
            assert_eq!(response_pid.as_u32(), 55011);
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = apply_all(state, &out.events);
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &state,
            BeendigungZuordnungCommand::BeendenBestaetigen,
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, BeendigungZuordnungState::Beendet(_)));
    }

    #[test]
    fn ablehnung_yields_55012() {
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &BeendigungZuordnungState::New,
            anfrage_cmd(true),
        )
        .unwrap();
        let state = apply_all(BeendigungZuordnungState::New, &out.events);
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &state,
            BeendigungZuordnungCommand::SendAntwort {
                antwort: crate::lf_antwort::LfAntwort::ablehnung("A35", "E_0624")
                    .with_bemerkung("Widerspruch"),
            },
        )
        .unwrap();
        if let BeendigungZuordnungEvent::AntwortGesendet { response_pid, .. } = &out.events[0] {
            assert_eq!(response_pid.as_u32(), 55012);
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = apply_all(state, &out.events);
        assert!(matches!(state, BeendigungZuordnungState::Rejected { .. }));
    }

    #[test]
    fn validation_failure_emits_aperak_313() {
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &BeendigungZuordnungState::New,
            anfrage_cmd(false),
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["error_code"], "Z29");
        let state = apply_all(BeendigungZuordnungState::New, &out.events);
        assert!(matches!(state, BeendigungZuordnungState::Rejected { .. }));
    }

    #[test]
    fn wrong_pid_rejected() {
        let mut cmd = anfrage_cmd(true);
        if let BeendigungZuordnungCommand::ReceiveAnfrage { pid: p, .. } = &mut cmd {
            *p = pid(55001);
        }
        assert!(
            GpkeBeendigungZuordnungWorkflow::handle(&BeendigungZuordnungState::New, cmd).is_err()
        );
    }
}

#[cfg(test)]
mod nb_initiator_tests {
    use super::*;

    fn mp(v: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(v.to_owned())
    }

    fn anfragen() -> BeendigungZuordnungCommand {
        BeendigungZuordnungCommand::Anfragen {
            sender: mp("9900357000004"),
            receiver: mp("9900111000002"),
            location_id: MaLo::new("51238696781".to_owned()),
            tranche: false,
            process_date: "20261101".to_owned(),
            vorgangsnummer: "ANF-1".to_owned(),
            anmeldung_process_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            kunde_name: Some("Mustermann".to_owned()),
            lfn_mp_id: Some("9900555000005".to_owned()),
        }
    }

    fn after_anfrage() -> BeendigungZuordnungState {
        let out =
            GpkeBeendigungZuordnungWorkflow::handle(&BeendigungZuordnungState::New, anfragen())
                .expect("Anfragen accepted");
        out.events
            .iter()
            .fold(BeendigungZuordnungState::New, |st, e| {
                GpkeBeendigungZuordnungWorkflow::apply(st, e)
            })
    }

    /// The NB-initiated arm: a 55010 carrying the SG12 parties `[279]` requires.
    #[test]
    fn the_nb_renders_the_anfrage_with_its_sg12_parties() {
        let out =
            GpkeBeendigungZuordnungWorkflow::handle(&BeendigungZuordnungState::New, anfragen())
                .expect("Anfragen accepted");
        let p = &out.outbox[0].payload;
        assert_eq!(&*out.outbox[0].message_type, "UTILMD");
        assert_eq!(p["pid"], 55_010);
        // 55010 is an Abmeldung, and it names the Zuordnungsende it asks for.
        assert_eq!(p["document_code"], "E02");
        assert_eq!(p["process_date"], "20261101");
        // Bedingung [279]/[572]: „Kundenname aus Anmeldung Lieferant neu" —
        // how the LFA tells an Einzug from a Wechsel at `E_0624` Prüfschritt 30.
        assert_eq!(p["kunde_name"], "Mustermann");
        // Bedingung [567]: the Neulieferant in `SG12 NAD+VY`.
        assert_eq!(p["beteiligte_marktpartner"][0], "9900555000005");
    }

    #[test]
    fn a_tranche_anfrage_names_the_tranche_qualifier() {
        let BeendigungZuordnungCommand::Anfragen {
            sender,
            receiver,
            location_id,
            process_date,
            vorgangsnummer,
            anmeldung_process_id,
            kunde_name,
            lfn_mp_id,
            ..
        } = anfragen()
        else {
            unreachable!()
        };
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &BeendigungZuordnungState::New,
            BeendigungZuordnungCommand::Anfragen {
                sender,
                receiver,
                location_id,
                tranche: true,
                process_date,
                vorgangsnummer,
                anmeldung_process_id,
                kunde_name,
                lfn_mp_id,
            },
        )
        .expect("accepted");
        assert_eq!(out.outbox[0].payload["lokationstyp"], "Z21");
    }

    /// „Verstreicht die Frist … gilt dies als Bestätigung nach Fall a)."
    /// The lapse is a **Zustimmung**, and it must not take the rejecting
    /// `TimeoutExpired` path.
    #[test]
    fn a_lapsed_window_is_a_zustimmung_not_a_timeout() {
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &after_anfrage(),
            BeendigungZuordnungCommand::AntwortfristAbgelaufen,
        )
        .expect("lapse accepted");
        let BeendigungZuordnungEvent::LfaAntwortErhalten {
            zustimmung,
            fristablauf,
            antwortcode,
            ..
        } = &out.events[0]
        else {
            panic!("expected LfaAntwortErhalten, got {:?}", out.events[0]);
        };
        assert!(zustimmung, "silence releases the Marktlokation");
        assert!(fristablauf);
        assert!(antwortcode.is_none(), "the LFA named no code");
        // `processd` has to hear about it — the Anmeldung is waiting.
        assert_eq!(&*out.outbox[0].message_type, "LfaAntwortAufAbmeldeanfrage");
        assert_eq!(out.outbox[0].payload["zustimmung"], true);
    }

    /// The deadline routes to the lapse command, not to `TimeoutExpired`: the
    /// two have opposite consequences, so they must not share a label.
    #[test]
    fn the_nb_window_and_the_lfa_window_do_not_share_a_label() {
        assert_ne!(
            NB_ANFRAGE_WINDOW_LABEL,
            BEENDIGUNG_ZUORDNUNG_ANTWORT_WINDOW_LABEL
        );
    }

    #[test]
    fn a_widerspruch_reaches_processd_with_its_grund() {
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &after_anfrage(),
            BeendigungZuordnungCommand::ReceiveAntwort {
                response_pid: Pruefidentifikator::new(55_012).expect("valid"),
                antwortcode: "A35".to_owned(),
                zustimmung: false,
                grund: Some("Vertragsbindung bis 31.12.2026".to_owned()),
                zuordnungsende: None,
            },
        )
        .expect("answer accepted");
        let p = &out.outbox[0].payload;
        assert_eq!(p["antwortcode"], "A35");
        assert_eq!(p["zustimmung"], false);
        assert_eq!(p["grund"], "Vertragsbindung bis 31.12.2026");
        assert_eq!(p["fristablauf"], false);
        // The Anmeldung to resume is named, because `E_0623` runs there.
        assert_eq!(
            p["anmeldung_process_id"],
            "11111111-1111-1111-1111-111111111111"
        );
    }

    /// Fall b — the LFA agrees to an *earlier* Zuordnungsende, which is what
    /// the NB's own 55037 must then state.
    #[test]
    fn fall_b_carries_the_earlier_zuordnungsende() {
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &after_anfrage(),
            BeendigungZuordnungCommand::ReceiveAntwort {
                response_pid: Pruefidentifikator::new(55_011).expect("valid"),
                antwortcode: "A34".to_owned(),
                zustimmung: true,
                grund: None,
                zuordnungsende: Some("20261015".to_owned()),
            },
        )
        .expect("answer accepted");
        assert_eq!(out.outbox[0].payload["zuordnungsende"], "20261015");
    }

    /// „Nach Ablauf der Frist eingehende Antworten sind für den Fortlauf dieses
    /// Prozesses unerheblich" — once the lapse closed the process, a late
    /// answer changes nothing.
    #[test]
    fn a_late_answer_is_refused_rather_than_reopening_the_process() {
        let closed = {
            let out = GpkeBeendigungZuordnungWorkflow::handle(
                &after_anfrage(),
                BeendigungZuordnungCommand::AntwortfristAbgelaufen,
            )
            .expect("lapse");
            out.events.iter().fold(after_anfrage(), |st, e| {
                GpkeBeendigungZuordnungWorkflow::apply(st, e)
            })
        };
        let err = GpkeBeendigungZuordnungWorkflow::handle(
            &closed,
            BeendigungZuordnungCommand::ReceiveAntwort {
                response_pid: Pruefidentifikator::new(55_011).expect("valid"),
                antwortcode: "A36".to_owned(),
                zustimmung: true,
                grund: None,
                zuordnungsende: None,
            },
        )
        .expect_err("a late answer must not reopen the process");
        assert!(format!("{err}").contains("AnfrageGesendet"), "{err}");
    }

    /// The Anfrage names the Zuordnungsende it asks for; the AHB marks
    /// `SG4 DTM+93` Muss on a 55010.
    #[test]
    fn an_anfrage_without_a_zuordnungsende_is_refused() {
        let BeendigungZuordnungCommand::Anfragen {
            sender,
            receiver,
            location_id,
            tranche,
            vorgangsnummer,
            anmeldung_process_id,
            kunde_name,
            lfn_mp_id,
            ..
        } = anfragen()
        else {
            unreachable!()
        };
        let err = GpkeBeendigungZuordnungWorkflow::handle(
            &BeendigungZuordnungState::New,
            BeendigungZuordnungCommand::Anfragen {
                sender,
                receiver,
                location_id,
                tranche,
                process_date: String::new(),
                vorgangsnummer,
                anmeldung_process_id,
                kunde_name,
                lfn_mp_id,
            },
        )
        .expect_err("55010 must name a Zuordnungsende");
        assert!(format!("{err}").contains("Zuordnungsende"), "{err}");
    }
}
