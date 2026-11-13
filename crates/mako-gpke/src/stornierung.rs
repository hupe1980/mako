//! GPKE Stornierung — cancellation of a still unanswered request (PIDs 55022–55024).
//!
//! A party to an Ursprungsnachricht — in this workflow the LF that sent an
//! Anmeldung, Abmeldung or Kündigung to the NB — cancels it with a UTILMD
//! Strom 55022 „Anfrage nach Stornierung". The receiver answers „zurück an den
//! Absender" with a **UTILMD** of its own (UTILMD AHB Strom 2.2 Kap. 9.6
//! „Stornierungsmeldung", S. 782–787):
//! - 55023 „Bestätigung Anfrage Stornierung" — `SG4 STS+E01` from Codeliste
//!   `S_0086`, whose only code is `E15` „Zustimmung ohne Korrekturen";
//! - 55024 „Ablehnung Anfrage Stornierung" — `SG4 STS+E01` from Codeliste
//!   `S_0087`: `E14` „Ablehnung Sonstiges" (with `FTX+ACB`), `Z14` „Ablehnung
//!   (Doppelmeldung)", `ZE1` „Ablehnung – zu stornierender Vorgang wurde schon
//!   beantwortet" (EBD 4.3 Kap. 6.27.1 / 6.27.2, S. 315).
//!
//! That answer is the business answer; it is not the APERAK. APERAK AHB 1.1
//! Kap. 2.4 has the receiver of every Strom Geschäftsvorfall state the result of
//! its Verarbeitbarkeitsprüfung by APERAK — Anerkennungsmeldung or
//! Verarbeitbarkeitsfehlermeldung — so a processable 55022 is acknowledged with
//! APERAK 29002 on receipt, and the 55023/55024 follows when the NB decides.
//!
//! # The window is the original message's, not a window of its own
//!
//! **No Festlegung states an answer window for 55022.** GPKE Teil 4 Kap. 5
//! gives the Stornierung a *precondition* instead — „auslösende Meldung wurde
//! noch nicht beantwortet" — and defers everything else to the EDI@Energy
//! „Allgemeine Festlegungen", which quantify nothing either (Kap. 7.3). The AWH
//! GeLi Gas says the same for Gas in so many words: „Eine Stornierung der
//! Anmeldung kann bis zum Eingang der Antwortnachricht erfolgen" (Kap. 2.5.2
//! Nr. 2).
//!
//! So the Stornierung's real boundary is the **original** message's
//! Antwortfrist: once the NB has answered the Anmeldung there is nothing left
//! to storno, only a Rückabwicklung, which GPKE Teil 4 Kap. 5 makes a manual
//! process requiring every party's consent. Size the deadline with
//! [`mako_fristen::antwort::antwort_deadline`] on the **original** PID and its
//! arrival instant — never with a duration invented for the Storno itself.
//!
//! # Process flow
//!
//! ```text
//! LF ──── 55022 Anfrage nach Stornierung ────►
//!    ◄─── APERAK 29002 Anerkennungsmeldung   ── NB, within 45 Minuten
//!    ◄─── UTILMD 55023 Bestätigung            ── NB, before it answers the original
//!     or ◄─ UTILMD 55024 Ablehnung            ──
//! ```
//!
//! # BGM qualifier semantics
//!
//! `BGM` DE 1001 names the **category of the cancelled message**, not the
//! Stornierung — „Code ist gemäß der Kategorie der zu stornierenden Meldung zu
//! wählen" (Bedingung `[500]`, Muss in all three columns): `E01` Anmeldungen,
//! `E02` Abmeldungen, `E35` Kündigung. The answer therefore repeats the
//! 55022's code. The Stornierung itself is `SG4 STS+7++E05`, in the Anfrage
//! and both answers.
//!
//! # Regulatory basis
//!
//! - **BDEW UTILMD AHB Strom 2.2 Kap. 9.6** — the three Anwendungsfälle
//!   55022–55024; their `STS+E01` codes come from Codelisten `S_0086` /
//!   `S_0087` (Entscheidungsbaum-Diagramme und Codelisten 4.3 Kap. 6.27)
//! - **BNetzA BK6-22-024 Anlage 1d, GPKE Teil 4 Kap. 5** — Stornierung und
//!   Rückabwicklung. It scopes the Stornierung to the Teil-2 Use-Cases
//!   Kündigung, Lieferbeginn, Neuanlage, Beginn der Ersatz-/Grundversorgung,
//!   Herstellung einer 100 % LF-Zuordnung and both Lieferende directions, plus
//!   the WiM Teil 1 Kündigung Messstellenbetrieb — and states no Frist.
//! - **EDI@Energy Allgemeine Festlegungen 6.1d Kap. 7.3** — a Stornierung is a new
//!   message carrying Transaktionsgrund „Stornierung"; also no Frist.
//! - **APERAK AHB 1.1 Kap. 2.4 / Kap. 2.4.1** — Anerkennungs- or
//!   Verarbeitbarkeitsfehlermeldung for the 55022, within 45 Minuten for a UTILMD

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Stable workflow name used as the `WorkflowId.name` in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "gpke-stornierung";

/// Deadline label for the window in which the Stornierung must be decided.
///
/// It is **not** an APERAK window — the APERAK is a separate 45-minute clock —
/// and it is not a duration of its own. No Festlegung publishes one; the
/// boundary is the *original* message's Antwortfrist, because a Stornierung is
/// only admissible while that message is unanswered (GPKE Teil 4 Kap. 5).
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`,
/// sized from the **original** PID and its arrival instant:
///
/// ```rust,ignore
/// let due = mako_fristen::antwort::antwort_deadline(original_pid, original_received)
///     .expect("the storniert message has a published Antwortfrist");
/// let deadline = Deadline::new(process.stream_id().clone(), ..., STORNIERUNG_ANTWORT_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const STORNIERUNG_ANTWORT_WINDOW_LABEL: &str = "gpke-stornierung-antwort";

/// PIDs handled by the GPKE Stornierung workflow (UTILMD Strom).
///
/// - 55022: Anfrage nach Stornierung (Beteiligte aus Ursprungsnachricht → its receiver)
/// - 55023: Bestätigung Anfrage Stornierung (zurück an den Absender)
/// - 55024: Ablehnung Anfrage Stornierung (zurück an den Absender)
pub const STORNIERUNG_PIDS: &[u32] = &[55022, 55023, 55024];

/// 55023 „Bestätigung Anfrage Stornierung" (UTILMD AHB Strom 2.2 Kap. 9.6).
pub const BESTAETIGUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(55023);

/// 55024 „Ablehnung Anfrage Stornierung" (UTILMD AHB Strom 2.2 Kap. 9.6).
pub const ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(55024);

/// Codeliste of the 55023's `SG4 STS+E01` DE 1131 (UTILMD AHB Strom 2.2 Kap. 9.6).
pub const BESTAETIGUNG_CODELISTE: &str = "S_0086";

/// Codeliste of the 55024's `SG4 STS+E01` DE 1131 (UTILMD AHB Strom 2.2 Kap. 9.6).
pub const ABLEHNUNG_CODELISTE: &str = "S_0087";

/// `E15` „Zustimmung ohne Korrekturen" — the only code of `S_0086`
/// (Entscheidungsbaum-Diagramme und Codelisten 4.3 Kap. 6.27.1, S. 315).
pub const ZUSTIMMUNG_CODE: &str = "E15";

/// Why the NB refuses a Stornierung — the three codes of Codeliste `S_0087`
/// (Entscheidungsbaum-Diagramme und Codelisten 4.3 Kap. 6.27.2, S. 315).
///
/// A closed set, so a code the Codeliste does not publish cannot ride a 55024.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "code")]
pub enum StornoAblehnung {
    /// `E14` „Ablehnung Sonstiges" — „Bei Verwendung von E14 muss in FTX+ACB,
    /// C108 als Freitext eine Erläuterung eingefügt werden"; the 55024 column
    /// makes `SG4 FTX` Muss under the same condition (Bedingung `[49]`).
    #[serde(rename = "E14")]
    Sonstiges {
        /// The `FTX+ACB` Erläuterung.
        bemerkung: String,
    },
    /// `Z14` „Ablehnung (Doppelmeldung)" — „Die Meldung liegt schon vor."
    #[serde(rename = "Z14")]
    Doppelmeldung,
    /// `ZE1` „Ablehnung – zu stornierender Vorgang wurde schon beantwortet".
    #[serde(rename = "ZE1")]
    SchonBeantwortet,
}

impl StornoAblehnung {
    /// The `SG4 STS+E01` DE 9013 code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Sonstiges { .. } => "E14",
            Self::Doppelmeldung => "Z14",
            Self::SchonBeantwortet => "ZE1",
        }
    }

    /// The `FTX+ACB` Erläuterung, present exactly on `E14`.
    #[must_use]
    pub fn bemerkung(&self) -> Option<&str> {
        match self {
            Self::Sonstiges { bemerkung } => Some(bemerkung),
            Self::Doppelmeldung | Self::SchonBeantwortet => None,
        }
    }
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Stornierung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GpkeStornierungEvent {
    /// PID 55022 (Anfrage nach Stornierung) received and accepted for processing.
    StornierungReceived {
        /// Prüfidentifikator (must be 55022).
        pruefidentifikator: Pruefidentifikator,
        /// MP-ID of the message sender (the LF that sent the Ursprungsnachricht).
        sender: MarktpartnerCode,
        /// MP-ID of the NB receiving the request.
        receiver: MarktpartnerCode,
        /// The 55022's own Vorgangsnummer (`SG4 IDE+24` DE 7402) — the answer
        /// carries it back in `SG4 SG6 RFF+TN`. The cancelled Vorgang is named
        /// by the 55022's `RFF+ACW`, not here.
        vorgang_id: MaLo,
        /// `BGM` DE 1001 of the 55022 — the category of the cancelled message
        /// (`E01` / `E02` / `E35`), which the answer repeats.
        document_code: String,
        /// EDIFACT document date string (YYYYMMDDHHMMZZZ from DTM+137+303).
        document_date: String,
        /// EDIFACT message reference (UNH 0062).
        message_ref: MessageRef,
    },
    /// AHB profile validation passed — no rule violations.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// AHB profile validation failed — answered with APERAK 29001.
    ValidationFailed {
        /// Human-readable list of validation issue strings.
        errors: Vec<String>,
    },
    /// The NB's UTILMD answer to the 55022 was queued.
    ///
    /// Bestätigung → `Completed`, Ablehnung → `Rejected`.
    AntwortGesendet {
        /// 55023 (Bestätigung) or 55024 (Ablehnung) — derived from `ablehnung`,
        /// never chosen beside it.
        response_pid: Pruefidentifikator,
        /// The `S_0087` ground of a 55024; `None` on a 55023.
        ablehnung: Option<StornoAblehnung>,
    },
    /// Process terminated because the Antwortfrist expired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl EventPayload for GpkeStornierungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::StornierungReceived { .. } => "GpkeStornierungReceived",
            Self::ValidationPassed { .. } => "GpkeStornierungValidationPassed",
            Self::ValidationFailed { .. } => "GpkeStornierungValidationFailed",
            Self::AntwortGesendet { .. } => "GpkeStornierungAntwortGesendet",
            Self::DeadlineExpired { .. } => "GpkeStornierungDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::StornierungReceived {
                pruefidentifikator,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AntwortGesendet { response_pid, .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(response_pid.as_u32()),
                message_ref: None,
            }),
            Self::ValidationPassed { .. }
            | Self::ValidationFailed { .. }
            | Self::DeadlineExpired { .. } => None,
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Business data recorded at `StornierungReceived` time and carried throughout.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GpkeStornierungData {
    /// Prüfidentifikator of the inbound 55022 message.
    pub pruefidentifikator: Pruefidentifikator,
    /// MP-ID of the sender (the LF requesting the cancellation).
    pub sender: MarktpartnerCode,
    /// MP-ID of the NB receiving the request.
    pub receiver: MarktpartnerCode,
    /// The 55022's own Vorgangsnummer (`SG4 IDE+24` DE 7402), returned in the
    /// answer's `SG4 SG6 RFF+TN` „Referenz Vorgangsnummer (aus Anfragenachricht)".
    pub vorgang_id: MaLo,
    /// `BGM` DE 1001 of the 55022, repeated on the answer (Bedingung `[500]`).
    pub document_code: String,
    /// EDIFACT document date from DTM+137.
    pub document_date: String,
    /// EDIFACT message reference from the 55022 message.
    pub message_ref: MessageRef,
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a GPKE Stornierung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → Initiated → ValidationPassed → Completed   (55023 sent)
///                                    ↘ Rejected    (55024 sent)
///     ↘ ValidationFailed → Rejected
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GpkeStornierungState {
    /// No events yet.
    #[default]
    New,
    /// 55022 received; awaiting validation result.
    Initiated(GpkeStornierungData),
    /// AHB validation passed; the NB's answer window is running — the
    /// **original** message's Antwortfrist, per
    /// [`STORNIERUNG_ANTWORT_WINDOW_LABEL`].
    ValidationPassed(GpkeStornierungData),
    /// 55023 Bestätigung sent; the cancelled Vorgang is not answered any more.
    Completed(GpkeStornierungData),
    /// Process rejected — validation failure or 55024 Ablehnung.
    Rejected {
        /// Human-readable reason for the rejection.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// The Stornierung is answered within the original message's Antwortfrist, and no Festlegung states a consequence of the NB missing it. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<GpkeStornierungState>,
    },
}

/// Whether an Anfrage Stornierung still holds its Vorgang against a new one.
///
/// Live while its answer is owed; settled once answered — accepted
/// (`Completed`) or refused. A settled process must not take the next
/// Anfrage, whose receive command refuses anything but `New`.
impl mako_engine::workflow::OccupiesBusinessKey for GpkeStornierungState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // A lapsed window leaves the Vorgang open.
            Self::FristAbgelaufen { offen, .. } => offen.occupies_business_key(),
            Self::Initiated(_) | Self::ValidationPassed(_) => true,
            Self::New | Self::Completed(_) | Self::Rejected { .. } => false,
        }
    }
}

impl GpkeStornierungState {
    /// Stable status string for metrics and projections.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Initiated(_) => "Initiated",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Completed(_) => "Completed",
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

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Stornierung workflow.
///
/// All domain values must be pre-extracted and validated at the transport boundary.
/// `Workflow::handle()` is pure — no I/O.
#[derive(Clone)]
pub enum GpkeStornierungCommand {
    /// Inbound UTILMD Strom PID 55022 accepted from the AS4 layer.
    ReceiveUtilmd {
        /// Must be 55022.
        pid: Pruefidentifikator,
        /// MP-ID of the sender (the LF requesting cancellation).
        sender: MarktpartnerCode,
        /// MP-ID of the receiver (NB).
        receiver: MarktpartnerCode,
        /// The 55022's own Vorgangsnummer, `SG4 IDE+24` DE 7402.
        vorgang_id: MaLo,
        /// `BGM` DE 1001 of the 55022 (`E01` / `E02` / `E35`).
        document_code: String,
        /// EDIFACT document date from DTM+137.
        document_date: String,
        /// EDIFACT message reference from UNH.
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed with no errors.
        validation_passed: bool,
        /// Human-readable validation error strings (empty when `validation_passed = true`).
        validation_errors: Vec<String>,
    },
    /// The NB answers the 55022 with UTILMD 55023 (`ablehnung: None`) or
    /// 55024 (`Some`).
    ///
    /// The window is the **original** message's Antwortfrist, which
    /// [`STORNIERUNG_ANTWORT_WINDOW_LABEL`] carries: a Stornierung is
    /// admissible only while that message is unanswered (GPKE Teil 4 Kap. 5),
    /// and no Festlegung publishes a duration of its own for this step.
    SendAntwort {
        /// The `S_0087` ground of an Ablehnung; `None` confirms with `E15`.
        ablehnung: Option<StornoAblehnung>,
    },
    /// The Antwortfrist fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline (matches `STORNIERUNG_ANTWORT_WINDOW_LABEL`).
        label: Box<str>,
    },
}

impl CommandPayload for GpkeStornierungCommand {}

/// The UTILMD 55023 / 55024 outbox entry answering the 55022 in `data`.
///
/// Every field is a column of UTILMD AHB Strom 2.2 Kap. 9.6: `BGM` repeats the
/// 55022's category, `STS+7++E05` the Transaktionsgrund, `STS+E01` the code
/// with its Codeliste, `RFF+TN` the 55022's Vorgangsnummer. The column carries
/// no `SG5 LOC` and no `SG4 DTM`, so the payload names neither.
fn antwort_outbox(
    data: &GpkeStornierungData,
    ablehnung: Option<&StornoAblehnung>,
) -> PendingOutbox {
    let (pid, code, codeliste) = match ablehnung {
        None => (BESTAETIGUNG_PID, ZUSTIMMUNG_CODE, BESTAETIGUNG_CODELISTE),
        Some(a) => (ABLEHNUNG_PID, a.code(), ABLEHNUNG_CODELISTE),
    };
    PendingOutbox::typed(
        data.sender.as_str(),
        &mako_engine::payload::UtilmdPayload {
            pid: pid.as_u32(),
            // The answer travels back the way the request came.
            sender: data.receiver.as_str().to_owned(),
            receiver: Some(data.sender.as_str().to_owned()),
            document_code: Some(data.document_code.clone()),
            transaktionsgrund: Some("E05".to_owned()),
            antwort_code: Some(code.to_owned()),
            antwort_codeliste: Some(codeliste.to_owned()),
            bemerkung: ablehnung
                .and_then(StornoAblehnung::bemerkung)
                .map(str::to_owned),
            referenz_vorgangsnummer: Some(data.vorgang_id.as_str().to_owned()),
            ..Default::default()
        },
    )
}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Stornierung workflow (PIDs 55022–55024).
///
/// The receiver of the original message is the process owner: PID 55022
/// (Anfrage nach Stornierung) arrives from whoever sent it, and is answered
/// with UTILMD 55023 (Bestätigung) or 55024 (Ablehnung).
///
/// The window is the **original** message's Antwortfrist, because a Stornierung
/// is only admissible while that message is unanswered (GPKE Teil 4 Kap. 5).
/// See the module header — no Festlegung publishes a window for 55022 itself.
pub struct GpkeStornierungWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while the NB still owes its 55023 / 55024
/// (`Initiated`, `ValidationPassed`).
fn window_open(label: &str, state: &GpkeStornierungState) -> bool {
    label == STORNIERUNG_ANTWORT_WINDOW_LABEL
        && matches!(
            state,
            GpkeStornierungState::Initiated(_) | GpkeStornierungState::ValidationPassed(_)
        )
}

impl Workflow for GpkeStornierungWorkflow {
    type State = GpkeStornierungState;
    type Event = GpkeStornierungEvent;
    type Command = GpkeStornierungCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[STORNIERUNG_ANTWORT_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| GpkeStornierungCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            GpkeStornierungState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            GpkeStornierungEvent::StornierungReceived {
                pruefidentifikator,
                sender,
                receiver,
                vorgang_id,
                document_code,
                document_date,
                message_ref,
            } => GpkeStornierungState::Initiated(GpkeStornierungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                receiver: receiver.clone(),
                vorgang_id: vorgang_id.clone(),
                document_code: document_code.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            }),

            GpkeStornierungEvent::ValidationPassed { .. } => {
                if let GpkeStornierungState::Initiated(data) = state {
                    GpkeStornierungState::ValidationPassed(data)
                } else {
                    state
                }
            }

            GpkeStornierungEvent::ValidationFailed { errors } => GpkeStornierungState::Rejected {
                reason: errors.join("; "),
            },

            GpkeStornierungEvent::AntwortGesendet { ablehnung, .. } => match state {
                GpkeStornierungState::ValidationPassed(data) => match ablehnung {
                    None => GpkeStornierungState::Completed(data),
                    Some(a) => GpkeStornierungState::Rejected {
                        reason: format!("Ablehnung Anfrage Stornierung ({})", a.code()),
                    },
                },
                _ => state,
            },

            GpkeStornierungEvent::DeadlineExpired { label, .. } => match state {
                GpkeStornierungState::Initiated(_) | GpkeStornierungState::ValidationPassed(_) => {
                    GpkeStornierungState::FristAbgelaufen {
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
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // A lapsed window changes nothing about what the process accepts; only
        // the timeout itself asks about the lapse.
        let lapsed = state;
        let state = state.offen();
        match command {
            GpkeStornierungCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                vorgang_id,
                document_code,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GpkeStornierungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !STORNIERUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "unsupported GPKE Stornierung PID {pid} (expected one of: {STORNIERUNG_PIDS:?})",
                    )));
                }
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();
                let mut events = vec![GpkeStornierungEvent::StornierungReceived {
                    pruefidentifikator: pid,
                    sender,
                    receiver,
                    vorgang_id,
                    document_code,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(GpkeStornierungEvent::ValidationPassed {
                        message_ref: message_ref.clone(),
                    });
                    // APERAK 29002 Anerkennungsmeldung — in the Sparte Strom the
                    // receiver states the result of its Verarbeitbarkeitsprüfung
                    // either way (APERAK AHB 1.1 Kap. 2.4), within 45 Minuten for a
                    // UTILMD (Kap. 2.4.1). The 55023/55024 is the separate business
                    // answer that `SendAntwort` queues.
                    let outbox = vec![
                        PendingOutbox::aperak_anerkennung(
                            receiver_gln.as_str(),
                            sender_mp_id.as_str(),
                            message_ref.as_str(),
                        )
                        .caused_by(1),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                } else {
                    let reason = validation_errors.join("; ");
                    events.push(GpkeStornierungEvent::ValidationFailed {
                        errors: validation_errors,
                    });
                    // APERAK BGM+313 — mandatory per APERAK AHB 1.1 Kap. 2.1.1.
                    // It has to be emitted here: `ValidationFailed` folds to
                    // `Rejected`, which is terminal, and `SendAntwort` requires
                    // `ValidationPassed` — so a refusal raised later is
                    // unreachable and the counterparty gets no answer.
                    let outbox = vec![
                        PendingOutbox::aperak_fehler(
                            receiver_gln.as_str(),
                            sender_mp_id.as_str(),
                            message_ref.as_str(),
                            mako_engine::erc::codes::Z29,
                            reason,
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                }
            }

            GpkeStornierungCommand::SendAntwort { ablehnung } => {
                let data = match state {
                    GpkeStornierungState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.status_str(),
                        ));
                    }
                };
                if let Some(StornoAblehnung::Sonstiges { bemerkung }) = &ablehnung
                    && bemerkung.trim().is_empty()
                {
                    return Err(WorkflowError::validation(
                        "E14 „Ablehnung Sonstiges\" needs its FTX+ACB Erläuterung \
                         (Codeliste S_0087)",
                    ));
                }
                let response_pid = if ablehnung.is_some() {
                    ABLEHNUNG_PID
                } else {
                    BESTAETIGUNG_PID
                };
                // The outbox entry *is* the answer: without it the event log
                // records the Stornierung as answered while the LF sees nothing.
                let outbox = vec![antwort_outbox(data, ablehnung.as_ref()).caused_by(0)];
                Ok(WorkflowOutput::with_outbox(
                    vec![GpkeStornierungEvent::AntwortGesendet {
                        response_pid,
                        ablehnung,
                    }],
                    outbox,
                ))
            }

            GpkeStornierungCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(WorkflowOutput::events(vec![
                    GpkeStornierungEvent::DeadlineExpired { deadline_id, label },
                ]))
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::ids::DeadlineId;

    use super::*;

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).expect("valid PID")
    }

    fn stornierung_cmd(p: u32, validation_passed: bool) -> GpkeStornierungCommand {
        GpkeStornierungCommand::ReceiveUtilmd {
            pid: pid(p),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9907317000007"),
            vorgang_id: MaLo::new("STORNO0000A"),
            document_code: "E01".to_owned(),
            document_date: "20251001000000+00".to_owned(),
            message_ref: MessageRef::new("00001"),
            validation_passed,
            validation_errors: if validation_passed {
                vec![]
            } else {
                vec!["missing DTM".to_owned()]
            },
        }
    }

    #[test]
    fn verlauf_states_the_inbound_55022_and_the_outbound_answer() {
        let output = GpkeStornierungWorkflow::handle(
            &GpkeStornierungState::New,
            stornierung_cmd(55022, true),
        )
        .unwrap();
        assert_eq!(
            output.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(55_022),
                message_ref: Some("00001".to_owned()),
            })
        );
        assert_eq!(
            output.events[1].verlauf(),
            None,
            "ValidationPassed is no message"
        );
        let state = output
            .events
            .iter()
            .fold(GpkeStornierungState::New, GpkeStornierungWorkflow::apply);
        for (ablehnung, expected) in [
            (None, 55_023),
            (Some(StornoAblehnung::Doppelmeldung), 55_024),
        ] {
            let out = GpkeStornierungWorkflow::handle(
                &state,
                GpkeStornierungCommand::SendAntwort { ablehnung },
            )
            .unwrap();
            assert_eq!(
                out.events[0].verlauf(),
                Some(Verlauf {
                    richtung: Richtung::Ausgang,
                    pid: Some(expected),
                    message_ref: None,
                })
            );
        }
    }

    fn validated() -> GpkeStornierungState {
        GpkeStornierungWorkflow::handle(&GpkeStornierungState::New, stornierung_cmd(55022, true))
            .unwrap()
            .events
            .iter()
            .fold(GpkeStornierungState::New, GpkeStornierungWorkflow::apply)
    }

    /// A processable 55022 is acknowledged by APERAK 29002 on receipt — the
    /// Sparte Strom states the Verarbeitbarkeitsprüfung either way (APERAK AHB
    /// 1.1 Kap. 2.4) — and nothing else goes out until the NB decides.
    #[test]
    fn a_processable_55022_is_acknowledged_by_aperak_29002() {
        let out = GpkeStornierungWorkflow::handle(
            &GpkeStornierungState::New,
            stornierung_cmd(55022, true),
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1, "exactly the Anerkennungsmeldung");
        assert_eq!(out.outbox[0].message_type.as_ref(), "APERAK");
        assert_eq!(out.outbox[0].payload["pid"], 29002);
    }

    /// The Bestätigung is a UTILMD 55023, not an APERAK: UTILMD AHB Strom 2.2
    /// Kap. 9.6 answers „zurück an den Absender" with `STS+E01` code `E15` from
    /// Codeliste `S_0086`, `BGM` repeating the 55022's category and `RFF+TN`
    /// its Vorgangsnummer.
    #[test]
    fn a_bestaetigung_queues_utilmd_55023() {
        let out = GpkeStornierungWorkflow::handle(
            &validated(),
            GpkeStornierungCommand::SendAntwort { ablehnung: None },
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1);
        let entry = &out.outbox[0];
        assert_eq!(entry.message_type.as_ref(), "UTILMD");
        assert_eq!(entry.recipient.as_ref(), "4012345000023", "back to the LF");
        let p = &entry.payload;
        assert_eq!(p["pid"], 55_023);
        assert_eq!(p["sender"], "9907317000007");
        assert_eq!(p["receiver"], "4012345000023");
        assert_eq!(p["document_code"], "E01");
        assert_eq!(p["transaktionsgrund"], "E05");
        assert_eq!(p["antwort_code"], "E15");
        assert_eq!(p["antwort_codeliste"], "S_0086");
        assert_eq!(p["referenz_vorgangsnummer"], "STORNO0000A");
        assert!(p.get("malo").is_none(), "the 9.6 columns carry no SG5 LOC");
    }

    /// The Ablehnung is a UTILMD 55024 with an `S_0087` code; `E14` carries
    /// its `FTX+ACB` Erläuterung (Codelisten 4.3 Kap. 6.27.2).
    #[test]
    fn an_ablehnung_queues_utilmd_55024() {
        let out = GpkeStornierungWorkflow::handle(
            &validated(),
            GpkeStornierungCommand::SendAntwort {
                ablehnung: Some(StornoAblehnung::Sonstiges {
                    bemerkung: "Vorgang bereits in Bearbeitung".to_owned(),
                }),
            },
        )
        .unwrap();
        let p = &out.outbox[0].payload;
        assert_eq!(out.outbox[0].message_type.as_ref(), "UTILMD");
        assert_eq!(p["pid"], 55_024);
        assert_eq!(p["antwort_code"], "E14");
        assert_eq!(p["antwort_codeliste"], "S_0087");
        assert_eq!(p["bemerkung"], "Vorgang bereits in Bearbeitung");

        let out = GpkeStornierungWorkflow::handle(
            &validated(),
            GpkeStornierungCommand::SendAntwort {
                ablehnung: Some(StornoAblehnung::SchonBeantwortet),
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["antwort_code"], "ZE1");
        assert!(out.outbox[0].payload.get("bemerkung").is_none());
    }

    /// `E14` without its Erläuterung is refused before the wire.
    #[test]
    fn an_e14_without_erlaeuterung_is_refused() {
        let err = GpkeStornierungWorkflow::handle(
            &validated(),
            GpkeStornierungCommand::SendAntwort {
                ablehnung: Some(StornoAblehnung::Sonstiges {
                    bemerkung: "  ".to_owned(),
                }),
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn receive_valid_55022_transitions_to_validation_passed() {
        let state = GpkeStornierungState::New;
        let output = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, true)).unwrap();
        assert_eq!(
            output.events.len(),
            2,
            "StornierungReceived + ValidationPassed"
        );
        assert!(matches!(
            output.events[0],
            GpkeStornierungEvent::StornierungReceived { .. }
        ));
        assert!(matches!(
            output.events[1],
            GpkeStornierungEvent::ValidationPassed { .. }
        ));

        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(state, GpkeStornierungState::ValidationPassed(_)));
    }

    /// A refused Stornierung must answer with the negative APERAK.
    ///
    /// APERAK AHB 1.1 Kap. 2.1.1 makes it mandatory, and it has to be emitted on
    /// this transition: `ValidationFailed` folds to `Rejected`, which is
    /// terminal, and `SendAntwort` requires `ValidationPassed` — so a refusal
    /// raised anywhere later is unreachable and the counterparty waits with no
    /// answer at all. The GeLi Gas twin has always emitted it here.
    #[test]
    fn a_refused_stornierung_answers_with_the_negative_aperak() {
        let out = GpkeStornierungWorkflow::handle(
            &GpkeStornierungState::New,
            stornierung_cmd(55022, false),
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1, "exactly one APERAK");
        assert_eq!(out.outbox[0].message_type.as_ref(), "APERAK");
        assert_eq!(out.outbox[0].payload["error_code"], "Z29");
    }

    #[test]
    fn receive_invalid_55022_transitions_to_rejected() {
        let state = GpkeStornierungState::New;
        let output =
            GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, false)).unwrap();
        assert_eq!(
            output.events.len(),
            2,
            "StornierungReceived + ValidationFailed"
        );

        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(state, GpkeStornierungState::Rejected { .. }));
    }

    #[test]
    fn a_bestaetigung_completes_the_process() {
        let state = GpkeStornierungState::New;
        let output = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(state, GpkeStornierungState::ValidationPassed(_)));

        let output = GpkeStornierungWorkflow::handle(
            &state,
            GpkeStornierungCommand::SendAntwort { ablehnung: None },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(
            matches!(state, GpkeStornierungState::Completed(_)),
            "a 55023 must complete the Stornierung; got: {state:?}"
        );
    }

    #[test]
    fn an_ablehnung_rejects_the_process() {
        let state = GpkeStornierungState::New;
        let output = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);

        let output = GpkeStornierungWorkflow::handle(
            &state,
            GpkeStornierungCommand::SendAntwort {
                ablehnung: Some(StornoAblehnung::SchonBeantwortet),
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(state, GpkeStornierungState::Rejected { .. }));
    }

    /// The NB's own answer window lapsing is a Frist breach, not a rejection.
    #[test]
    fn deadline_expired_is_a_frist_breach() {
        let state = GpkeStornierungState::New;
        let output = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);

        let output = GpkeStornierungWorkflow::handle(
            &state,
            GpkeStornierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: STORNIERUNG_ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(
            state,
            GpkeStornierungState::FristAbgelaufen { .. }
        ));
    }

    #[test]
    fn reject_unsupported_pid() {
        let state = GpkeStornierungState::New;
        let result = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55021, true));
        assert!(result.is_err(), "PID 55021 must be rejected");
    }

    #[test]
    fn deadline_is_noop_for_completed_process() {
        let data = GpkeStornierungData {
            pruefidentifikator: pid(55022),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9907317000007"),
            vorgang_id: MaLo::new("STORNO0000A"),
            document_code: "E01".to_owned(),
            document_date: "20251001000000+00".to_owned(),
            message_ref: MessageRef::new("00001"),
        };
        let state = GpkeStornierungState::Completed(data);
        let output = GpkeStornierungWorkflow::handle(
            &state,
            GpkeStornierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: STORNIERUNG_ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(
            output.events.is_empty(),
            "deadline must be no-op for Completed state"
        );
    }
}
