//! GeLi Gas / WiM Gas Stornierung — GNB-side workflow for cancellation of supply-change
//! and MSB-change requests (PIDs 44022–44024).
//!
//! Per BDEW PID 3.3/4.0 xlsx: PIDs 44022–44024 are multi-domain — both **"GeLi Gas 2.0"**
//! (supply-change cancellation, LFN/LFA role) and **"WiM Gas"** (MSB-change cancellation,
//! MSB role).
//!
//! **One owner, both Use-Cases.** The recipient of a 44022 is „der Empfänger
//! einer Stornierungsanfrage" — the party that received the Ursprungsnachricht —
//! and the workflow resolves which process is meant from `RFF+ACW`, not from the
//! deployment's Marktrolle. So the GNB side registers 44022 whenever the
//! deployment holds the `Nb` role, whether or not it is also an MSB; the LF side
//! registers 44023/44024, which are disjoint. A combined `Nb + Msb` deployment
//! — the common shape, since §41 MsbG puts the gMSB inside the NB's own legal
//! entity in most grids — therefore receives the Anfrage exactly once.
//!
//! **This module is the GNB-side perspective.** For the LFN/LFA perspective (LF initiates
//! outbound 44022, awaits 44023/44024), see [`crate::lf_stornierung`].
//!
//! „Die Anfrage wird an den Empfänger des zu stornierenden Vorgangs gesendet"
//! and both answers go „zurück an den Absender" — as **UTILMD**, never as an
//! APERAK (UTILMD AHB Gas 1.2 Kap. 5.2 „Anwendungsübersicht
//! Stornierungsmeldung", S. 25–28). The GNB answers within no window of its own
//! (see [`STORNIERUNG_RESPONSE_WINDOW_LABEL`]) with either:
//! - 44023 „Bestätigung Anfrage Stornierung" — `SG4 STS+E01` from Codeliste
//!   `G_0004`, whose only code is `E15` „Zustimmung ohne Korrekturen";
//! - 44024 „Ablehnung Anfrage Stornierung" — `SG4 STS+E01` from Codeliste
//!   `G_0003`: `E14` „Ablehnung Sonstiges" (with `FTX+ACB`), `Z14` „Ablehnung
//!   (Doppelmeldung)", `ZE1` „Ablehnung – zu stornierender Vorgang wurde schon
//!   beantwortet" (Entscheidungsbaum-Diagramme und Codelisten 4.3 Kap. 13.2.1 /
//!   13.2.2, S. 714).
//!
//! Note the order: in Gas the **Ablehnung** list is `G_0003` and the
//! Bestätigung `G_0004`.
//!
//! # Process flow
//!
//! ```text
//! LFN/LFA ──── 44022 Anfrage nach Stornierung ────►
//!         ◄─── UTILMD 44023 Bestätigung (G_0004)  ── GNB
//!          or ◄─ UTILMD 44024 Ablehnung (G_0003)  ── GNB
//! ```
//!
//! No APERAK accompanies a processable 44022: in the Sparte Gas the APERAK
//! reports only a Verarbeitbarkeitsfehler (APERAK AHB 1.1 Kap. 2.3).
//!
//! # BGM qualifier semantics
//!
//! `BGM` DE 1001 names the **category of the cancelled message** — „Code ist
//! gemäß der Kategorie der zu stornierenden Meldung zu wählen" (Bedingung
//! `[500]`, Muss in all three columns): `E01` Anmeldungen, `E02` Abmeldungen,
//! `E35` Kündigung. The answer therefore repeats the 44022's code. The
//! Stornierung itself is `SG4 STS+7++E05`, in the Anfrage and both answers.
//!
//! # Regulatory basis
//!
//! - **BDEW UTILMD AHB Gas 1.2 Kap. 5.2** — the three Anwendungsfälle 44022–44024
//! - **BNetzA BK7-24-01-009** — Geschäftsprozesse Lieferantenwechsel Gas 3.0 (eff. 2025-09-24)
//! - **APERAK**: Gas knows only the Verarbeitbarkeitsfehlermeldung — nächster
//!   Werktag 12:00 Uhr for a Folgeprozess, 3 Werktage for an Initialprozess
//!   (APERAK AHB 1.1 Kap. 2.3.1). 44022 is `ZG-T26`, hence a Folgeprozess.

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
pub const WORKFLOW_NAME: &str = "geli-gas-stornierung";

/// Deadline label for the answer to a Stornierungsanfrage.
///
/// **No Festlegung quantifies this window.** GeLi Gas 2.0 Kap. 1.8 — which AWH
/// WiM Gas 2.0 Kap. 2.6 defers to in full — describes the Stornierung as
/// „elektronisch beantwortet" and states no Frist, referring the reader on to
/// the EDI@Energy *Allgemeine Festlegungen*, which quantify nothing either.
///
/// The window this label carries is therefore an **operating convention**, not
/// a regulatory deadline, and the label is deliberately free of a Werktage
/// count so that nothing reports it as one.
pub const STORNIERUNG_RESPONSE_WINDOW_LABEL: &str = "geli-gas-stornierung-response";

/// PIDs handled by the GeLi Gas Stornierung workflow (UTILMD G).
///
/// - 44022: Anfrage nach Stornierung (initiator → GNB) — **inbound on GNB side**
/// - 44023: Bestätigung Anfrage Stornierung (GNB → initiator) — outbound UTILMD
/// - 44024: Ablehnung Anfrage Stornierung (GNB → initiator) — outbound UTILMD
pub const STORNIERUNG_PIDS: &[u32] = &[44022, 44023, 44024];

/// 44023 „Bestätigung Anfrage Stornierung" (UTILMD AHB Gas 1.2 Kap. 5.2).
pub const BESTAETIGUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(44023);

/// 44024 „Ablehnung Anfrage Stornierung" (UTILMD AHB Gas 1.2 Kap. 5.2).
pub const ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(44024);

/// Codeliste of the 44023's `SG4 STS+E01` DE 1131 (UTILMD AHB Gas 1.2 Kap. 5.2).
pub const BESTAETIGUNG_CODELISTE: &str = "G_0004";

/// Codeliste of the 44024's `SG4 STS+E01` DE 1131 (UTILMD AHB Gas 1.2 Kap. 5.2).
pub const ABLEHNUNG_CODELISTE: &str = "G_0003";

/// `E15` „Zustimmung ohne Korrekturen" — the only code of `G_0004`
/// (Entscheidungsbaum-Diagramme und Codelisten 4.3 Kap. 13.2.2, S. 714).
pub const ZUSTIMMUNG_CODE: &str = "E15";

/// Why the GNB refuses a Stornierung — the three codes of Codeliste `G_0003`
/// (Entscheidungsbaum-Diagramme und Codelisten 4.3 Kap. 13.2.1, S. 714).
///
/// A closed set, so a code the Codeliste does not publish cannot ride a 44024.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "code")]
pub enum StornoAblehnung {
    /// `E14` „Ablehnung Sonstiges" — „Bei Verwendung von E14 muss in FTX+ACB,
    /// C108 als Freitext eine Erläuterung eingefügt werden"; the 44024 column
    /// makes `SG4 FTX` Muss under the same condition (Bedingung `[48]`).
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

/// Inbound PID for the GNB-side workflow.
///
/// Only PID 44022 (Anfrage nach Stornierung) is received inbound on the GNB side.
/// PIDs 44023/44024 are **outbound** from GNB — they are dispatched via the outbox
/// and do not need PID-router registration for inbound routing.
pub const ANFRAGE_PID: Pruefidentifikator = Pruefidentifikator::const_new(44022);

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas Stornierung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeliGasStornierungEvent {
    /// PID 44022 (Anfrage nach Stornierung) received and accepted for processing.
    StornierungReceived {
        /// Prüfidentifikator (must be 44022).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the message sender (LFN / LFA).
        sender: MarktpartnerCode,
        /// GLN of the GNB receiving the request.
        receiver: MarktpartnerCode,
        /// The 44022's own Vorgangsnummer (`SG4 IDE+24` DE 7402) — the answer
        /// carries it back in `SG4 SG6 RFF+TN`. The cancelled Vorgang is named
        /// by the 44022's `RFF+ACW`, not here.
        vorgang_id: MaLo,
        /// `BGM` DE 1001 of the 44022 — the category of the cancelled message
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
    /// The GNB's UTILMD answer to the 44022 was queued.
    ///
    /// Bestätigung → `Completed`, Ablehnung → `Rejected`.
    AntwortGesendet {
        /// 44023 (Bestätigung) or 44024 (Ablehnung) — derived from `ablehnung`,
        /// never chosen beside it.
        response_pid: Pruefidentifikator,
        /// The `G_0003` ground of a 44024; `None` on a 44023.
        ablehnung: Option<StornoAblehnung>,
    },
    /// Process terminated because the answer window expired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl EventPayload for GeliGasStornierungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::StornierungReceived { .. } => "GeliGasStornierungReceived",
            Self::ValidationPassed { .. } => "GeliGasStornierungValidationPassed",
            Self::ValidationFailed { .. } => "GeliGasStornierungValidationFailed",
            Self::AntwortGesendet { .. } => "GeliGasStornierungAntwortGesendet",
            Self::DeadlineExpired { .. } => "GeliGasStornierungDeadlineExpired",
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
pub struct GeliGasStornierungData {
    /// Prüfidentifikator of the inbound 44022 message.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the sender (LFN / LFA initiating the cancellation).
    pub sender: MarktpartnerCode,
    /// GLN of the GNB receiving the request.
    pub receiver: MarktpartnerCode,
    /// The 44022's own Vorgangsnummer (`SG4 IDE+24` DE 7402), returned in the
    /// answer's `SG4 SG6 RFF+TN` „Referenz Vorgangsnummer (aus Anfragenachricht)".
    pub vorgang_id: MaLo,
    /// `BGM` DE 1001 of the 44022, repeated on the answer (Bedingung `[500]`).
    pub document_code: String,
    /// EDIFACT document date from DTM+137.
    pub document_date: String,
    /// EDIFACT message reference from the 44022 message.
    pub message_ref: MessageRef,
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a GeLi Gas Stornierung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → Initiated → ValidationPassed → Completed   (44023 sent)
///                                    ↘ Rejected    (44024 sent)
///     ↘ ValidationFailed → Rejected
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GeliGasStornierungState {
    /// No events yet.
    #[default]
    New,
    /// 44022 received; awaiting validation result.
    Initiated(GeliGasStornierungData),
    /// AHB validation passed; the GNB owes an answer.
    ValidationPassed(GeliGasStornierungData),
    /// 44023 Bestätigung sent; „Der stornierte Vorgang wird nicht mehr
    /// beantwortet" (UTILMD AHB Gas 1.2 Kap. 5.2).
    Completed(GeliGasStornierungData),
    /// Process rejected — validation failure or 44024 Ablehnung.
    Rejected {
        /// Human-readable reason for the rejection.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// No Festlegung states a consequence of the GNB leaving the Stornierung unanswered. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<GeliGasStornierungState>,
    },
}

/// Whether an Anfrage Stornierung still holds its Vorgang against a new one.
///
/// Live while its answer is owed; settled once answered — accepted
/// (`Completed`) or refused. A settled process must not take the next
/// Anfrage, whose receive command refuses anything but `New`.
impl mako_engine::workflow::OccupiesBusinessKey for GeliGasStornierungState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // A lapsed window leaves the Vorgang open.
            Self::FristAbgelaufen { offen, .. } => offen.occupies_business_key(),
            Self::Initiated(_) | Self::ValidationPassed(_) => true,
            Self::New | Self::Completed(_) | Self::Rejected { .. } => false,
        }
    }
}

impl GeliGasStornierungState {
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

/// Commands for the GeLi Gas Stornierung workflow.
///
/// All domain values must be pre-extracted and validated at the transport boundary.
/// `Workflow::handle()` is pure — no I/O.
#[derive(Clone)]
pub enum GeliGasStornierungCommand {
    /// Inbound UTILMD G PID 44022 accepted from the AS4 layer.
    ReceiveUtilmd {
        /// Must be 44022.
        pid: Pruefidentifikator,
        /// GLN of the sender (LFN / LFA).
        sender: MarktpartnerCode,
        /// GLN of the receiver (GNB).
        receiver: MarktpartnerCode,
        /// The 44022's own Vorgangsnummer, `SG4 IDE+24` DE 7402.
        vorgang_id: MaLo,
        /// `BGM` DE 1001 of the 44022 (`E01` / `E02` / `E35`).
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
    /// The GNB answers the 44022 with UTILMD 44023 (`ablehnung: None`) or
    /// 44024 (`Some`); the codes come from `G_0004` resp. `G_0003`.
    SendAntwort {
        /// The `G_0003` ground of an Ablehnung; `None` confirms with `E15`.
        ablehnung: Option<StornoAblehnung>,
    },
    /// The answer window expired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline (matches `STORNIERUNG_RESPONSE_WINDOW_LABEL`).
        label: Box<str>,
    },
}

impl CommandPayload for GeliGasStornierungCommand {}

/// The UTILMD 44023 / 44024 outbox entry answering the 44022 in `data`.
///
/// Every field is a column of UTILMD AHB Gas 1.2 Kap. 5.2: `BGM` repeats the
/// 44022's category, `STS+7++E05` the Transaktionsgrund, `STS+E01` the code
/// with its Codeliste, `RFF+TN` the 44022's Vorgangsnummer. The column carries
/// no `SG5 LOC` and no `SG4 DTM`, so the payload names neither.
fn antwort_outbox(
    data: &GeliGasStornierungData,
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

/// GeLi Gas Stornierung workflow (PIDs 44022–44024).
///
/// The GNB (Gasnetzbetreiber) is the process owner. The inbound message is PID 44022
/// (Anfrage nach Stornierung). The GNB responds with UTILMD 44023 (Bestätigung)
/// or 44024 (Ablehnung).
pub struct GeliGasStornierungWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while the GNB still owes its 44023 / 44024
/// (`Initiated`, `ValidationPassed`).
fn window_open(label: &str, state: &GeliGasStornierungState) -> bool {
    label == STORNIERUNG_RESPONSE_WINDOW_LABEL
        && matches!(
            state,
            GeliGasStornierungState::Initiated(_) | GeliGasStornierungState::ValidationPassed(_)
        )
}

impl Workflow for GeliGasStornierungWorkflow {
    type State = GeliGasStornierungState;
    type Event = GeliGasStornierungEvent;
    type Command = GeliGasStornierungCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[STORNIERUNG_RESPONSE_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| GeliGasStornierungCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            GeliGasStornierungState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            GeliGasStornierungEvent::StornierungReceived {
                pruefidentifikator,
                sender,
                receiver,
                vorgang_id,
                document_code,
                document_date,
                message_ref,
            } => GeliGasStornierungState::Initiated(GeliGasStornierungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                receiver: receiver.clone(),
                vorgang_id: vorgang_id.clone(),
                document_code: document_code.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            }),

            GeliGasStornierungEvent::ValidationPassed { .. } => {
                if let GeliGasStornierungState::Initiated(data) = state {
                    GeliGasStornierungState::ValidationPassed(data)
                } else {
                    state
                }
            }

            GeliGasStornierungEvent::ValidationFailed { errors } => {
                GeliGasStornierungState::Rejected {
                    reason: errors.join("; "),
                }
            }

            GeliGasStornierungEvent::AntwortGesendet { ablehnung, .. } => match state {
                GeliGasStornierungState::ValidationPassed(data) => match ablehnung {
                    None => GeliGasStornierungState::Completed(data),
                    Some(a) => GeliGasStornierungState::Rejected {
                        reason: format!("Ablehnung Anfrage Stornierung ({})", a.code()),
                    },
                },
                _ => state,
            },

            GeliGasStornierungEvent::DeadlineExpired { label, .. } => match state {
                GeliGasStornierungState::Initiated(_)
                | GeliGasStornierungState::ValidationPassed(_) => {
                    GeliGasStornierungState::FristAbgelaufen {
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
            GeliGasStornierungCommand::ReceiveUtilmd {
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
                if !matches!(state, GeliGasStornierungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !STORNIERUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "unsupported GeLi Gas Stornierung PID {pid} (expected one of: {STORNIERUNG_PIDS:?})",
                    )));
                }
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![GeliGasStornierungEvent::StornierungReceived {
                    pruefidentifikator: pid,
                    sender,
                    receiver,
                    vorgang_id,
                    document_code,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(GeliGasStornierungEvent::ValidationPassed {
                        message_ref: message_ref.clone(),
                    });
                    // No APERAK: Gas reports only a Verarbeitbarkeitsfehler
                    // (APERAK AHB 1.1 Kap. 2.3). The answer is the 44023/44024
                    // that `SendAntwort` queues.
                    Ok(WorkflowOutput::events(events))
                } else {
                    let reason = validation_errors.join("; ");
                    events.push(GeliGasStornierungEvent::ValidationFailed {
                        errors: validation_errors,
                    });
                    // APERAK BGM+313 — mandatory per APERAK AHB 1.1 Kap. 2.1.1.
                    // APERAK Frist (Gas Folgeprozess): nächster Werktag 12 Uhr
                    // (APERAK AHB 1.1 Kap. 2.3.1) — a clock separate from the
                    // answer window STORNIERUNG_RESPONSE_WINDOW_LABEL carries.
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

            GeliGasStornierungCommand::SendAntwort { ablehnung } => {
                let data = match state {
                    GeliGasStornierungState::ValidationPassed(d) => d,
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
                         (Codeliste G_0003)",
                    ));
                }
                let response_pid = if ablehnung.is_some() {
                    ABLEHNUNG_PID
                } else {
                    BESTAETIGUNG_PID
                };
                // The outbox entry *is* the answer: without it the event log
                // records the Stornierung as answered while the sender sees
                // nothing.
                let outbox = vec![antwort_outbox(data, ablehnung.as_ref()).caused_by(0)];
                Ok(WorkflowOutput::with_outbox(
                    vec![GeliGasStornierungEvent::AntwortGesendet {
                        response_pid,
                        ablehnung,
                    }],
                    outbox,
                ))
            }

            GeliGasStornierungCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(WorkflowOutput::events(vec![
                    GeliGasStornierungEvent::DeadlineExpired { deadline_id, label },
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

    fn stornierung_cmd(p: u32, validation_passed: bool) -> GeliGasStornierungCommand {
        GeliGasStornierungCommand::ReceiveUtilmd {
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
    fn verlauf_states_the_messages() {
        let output = GeliGasStornierungWorkflow::handle(
            &GeliGasStornierungState::New,
            stornierung_cmd(44022, true),
        )
        .unwrap();
        assert_eq!(
            output.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(44022),
                message_ref: Some("00001".to_owned()),
            })
        );
        assert_eq!(output.events[1].verlauf(), None, "ValidationPassed");
        for (ablehnung, expected) in [
            (None, 44_023),
            (Some(StornoAblehnung::Doppelmeldung), 44_024),
        ] {
            let out = GeliGasStornierungWorkflow::handle(
                &validated(),
                GeliGasStornierungCommand::SendAntwort { ablehnung },
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

    fn validated() -> GeliGasStornierungState {
        GeliGasStornierungWorkflow::handle(
            &GeliGasStornierungState::New,
            stornierung_cmd(44022, true),
        )
        .unwrap()
        .events
        .iter()
        .fold(
            GeliGasStornierungState::New,
            GeliGasStornierungWorkflow::apply,
        )
    }

    /// A processable 44022 puts nothing on the wire: Gas has no
    /// Anerkennungsmeldung (APERAK AHB 1.1 Kap. 2.3).
    #[test]
    fn a_processable_44022_sends_no_aperak() {
        let out = GeliGasStornierungWorkflow::handle(
            &GeliGasStornierungState::New,
            stornierung_cmd(44022, true),
        )
        .unwrap();
        assert!(out.outbox.is_empty(), "{:?}", out.outbox);
    }

    /// The Bestätigung is a UTILMD 44023, not an APERAK: UTILMD AHB Gas 1.2
    /// Kap. 5.2 answers „zurück an den Absender" with `STS+E01` code `E15` from
    /// Codeliste `G_0004`, `BGM` repeating the 44022's category and `RFF+TN`
    /// its Vorgangsnummer.
    #[test]
    fn a_bestaetigung_queues_utilmd_44023() {
        let out = GeliGasStornierungWorkflow::handle(
            &validated(),
            GeliGasStornierungCommand::SendAntwort { ablehnung: None },
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1);
        let entry = &out.outbox[0];
        assert_eq!(entry.message_type.as_ref(), "UTILMD");
        assert_eq!(entry.recipient.as_ref(), "4012345000023");
        let p = &entry.payload;
        assert_eq!(p["pid"], 44_023);
        assert_eq!(p["sender"], "9907317000007");
        assert_eq!(p["receiver"], "4012345000023");
        assert_eq!(p["document_code"], "E01");
        assert_eq!(p["transaktionsgrund"], "E05");
        assert_eq!(p["antwort_code"], "E15");
        assert_eq!(p["antwort_codeliste"], "G_0004");
        assert_eq!(p["referenz_vorgangsnummer"], "STORNO0000A");
    }

    /// The Ablehnung is a UTILMD 44024 with a `G_0003` code — the Gas lists
    /// run the other way round from Strom's `S_0086`/`S_0087`.
    #[test]
    fn an_ablehnung_queues_utilmd_44024() {
        let out = GeliGasStornierungWorkflow::handle(
            &validated(),
            GeliGasStornierungCommand::SendAntwort {
                ablehnung: Some(StornoAblehnung::Sonstiges {
                    bemerkung: "Vorgang bereits in Bearbeitung".to_owned(),
                }),
            },
        )
        .unwrap();
        let p = &out.outbox[0].payload;
        assert_eq!(out.outbox[0].message_type.as_ref(), "UTILMD");
        assert_eq!(p["pid"], 44_024);
        assert_eq!(p["antwort_code"], "E14");
        assert_eq!(p["antwort_codeliste"], "G_0003");
        assert_eq!(p["bemerkung"], "Vorgang bereits in Bearbeitung");
    }

    /// `E14` without its Erläuterung is refused before the wire.
    #[test]
    fn an_e14_without_erlaeuterung_is_refused() {
        let err = GeliGasStornierungWorkflow::handle(
            &validated(),
            GeliGasStornierungCommand::SendAntwort {
                ablehnung: Some(StornoAblehnung::Sonstiges {
                    bemerkung: String::new(),
                }),
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn receive_valid_44022_transitions_to_validation_passed() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, true)).unwrap();
        assert_eq!(
            output.events.len(),
            2,
            "StornierungReceived + ValidationPassed"
        );
        assert!(matches!(
            output.events[0],
            GeliGasStornierungEvent::StornierungReceived { .. }
        ));
        assert!(matches!(
            output.events[1],
            GeliGasStornierungEvent::ValidationPassed { .. }
        ));

        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(
            state,
            GeliGasStornierungState::ValidationPassed(_)
        ));
    }

    #[test]
    fn receive_invalid_44022_transitions_to_rejected() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, false)).unwrap();
        assert_eq!(
            output.events.len(),
            2,
            "StornierungReceived + ValidationFailed"
        );

        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(state, GeliGasStornierungState::Rejected { .. }));
    }

    #[test]
    fn a_bestaetigung_completes_the_process() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(
            state,
            GeliGasStornierungState::ValidationPassed(_)
        ));

        let output = GeliGasStornierungWorkflow::handle(
            &state,
            GeliGasStornierungCommand::SendAntwort { ablehnung: None },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(
            matches!(state, GeliGasStornierungState::Completed(_)),
            "a 44023 must complete the Stornierung; got: {state:?}"
        );
    }

    #[test]
    fn an_ablehnung_rejects_the_process() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);

        let output = GeliGasStornierungWorkflow::handle(
            &state,
            GeliGasStornierungCommand::SendAntwort {
                ablehnung: Some(StornoAblehnung::SchonBeantwortet),
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(state, GeliGasStornierungState::Rejected { .. }));
    }

    /// The GNB's own answer window lapsing is a Frist breach, not a rejection.
    #[test]
    fn deadline_expired_is_a_frist_breach() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);

        let output = GeliGasStornierungWorkflow::handle(
            &state,
            GeliGasStornierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: STORNIERUNG_RESPONSE_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(
            state,
            GeliGasStornierungState::FristAbgelaufen { .. }
        ));
    }

    #[test]
    fn reject_unsupported_pid() {
        let state = GeliGasStornierungState::New;
        let result = GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022 - 1, true));
        assert!(result.is_err(), "PID 44021 must be rejected");
    }

    #[test]
    fn deadline_is_noop_for_completed_process() {
        let data = GeliGasStornierungData {
            pruefidentifikator: pid(44022),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9907317000007"),
            vorgang_id: MaLo::new("STORNO0000A"),
            document_code: "E01".to_owned(),
            document_date: "20251001000000+00".to_owned(),
            message_ref: MessageRef::new("00001"),
        };
        let state = GeliGasStornierungState::Completed(data);
        let output = GeliGasStornierungWorkflow::handle(
            &state,
            GeliGasStornierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: STORNIERUNG_RESPONSE_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(
            output.events.is_empty(),
            "deadline must be no-op for Completed state"
        );
    }
}
