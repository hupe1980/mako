//! GeLi Gas Stammdatenänderung — master-data change process family (44109–44182).
//!
//! The Gas twin of `mako-gpke::stammdatenaenderung`, but with a **genuine
//! Zustimmung/Ablehnung regime** — unlike Strom (asynchronous quality feedback),
//! GeLi Gas lets the Berechtigter reject a change:
//!
//! - **E15** — Zustimmung ohne Korrekturen (accept → applied to `marktd`).
//! - **E13** — Ablehnung (Bilanzierungsproblem: BK unbekannt / nicht in der
//!   Zuordnungsermächtigung).
//! - **E17** — Ablehnung wegen Fristüberschreitung — **also** raised when a
//!   *bilanzierungsrelevante* change's Änderungsdatum is not a Monatserster
//!   (GeLiGas AWH V1.2 §4.3.2: bila.rel. changes only zum Monatserster with a
//!   one-month lead). Non-bila.rel. changes may take effect unverzüglich.
//!
//! Frist for the Antwort: „unverzüglich, spätestens jedoch bis zum Ablauf des
//! **10. WT** nach Eingang der Änderung" (AWH GeLi Gas § 4.3.2 Nr. 2 / Nr. 4),
//! published as `mako_fristen::antwort::STAMMDATEN_ANTWORT_WERKTAGE_GAS`. Five
//! times the Strom window, and genuinely so — not the LFW24 regime.
//!
//! ## Scope
//!
//! The **change families G1–G7** (NB↔LF, NB↔MSB, MSB↔LF, MSB↔wMSB — one Antwort
//! PID per direction, outcome carried as the SG4 STS code) are modeled in full:
//! inbound change → apply MaLo attributes → Zustimmung/Ablehnung. The **Anfrage
//! families G8–G10** (a Berechtigter *requests* master data — GeLiGas AWH §5.12–
//! 5.13) run the request round-trip: an inbound Anfrage spawns the data owner's
//! [`GasStammdatenCommand::ReceiveAnfrage`], which **auto-answers** with the
//! mapped Antwort PID ([`ANFRAGE_ANTWORT_PAIRS`]) carrying the requested
//! Marktlokation's current master data (data-return), or an APERAK reject on a
//! structurally-invalid Anfrage. WiM Gas Verpflichtungsanfrage (44168–44170) and
//! „Ende MSB" (44183) belong to other workflows and are excluded.
//!
//! # Regulatory basis
//!
//! - **GeLiGas AWH V1.2 (26.03.2026)** § 4.3 (Stammdatenänderung), § 4.3.2 Nr. 2/4 (Frist)
//! - **UTILMD AHB Gas 1.1** — object → PID map
//! - **EBD 4.2** §13.10–13.13 (E_3010–E_3013, codes E15/E13/E17/…)
//! - **APERAK AHB 1.0** — Gas Folgeprozess APERAK (nächster Werktag 12:00)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use mako_fristen::{
    APERAK_GAS_FOLGEPROZESS_LABEL, HolidayCalendar, aperak_gas_folgeprozess_due_at,
    deadline_at_werktage,
};

// ── PID model ─────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "geli-gas-stammdatenaenderung";

/// Deadline label for the 10-Werktage Antwort window.
///
/// „Es ist hierfür ein Konzept anzuwenden, dass … die Änderung, Bearbeitung und
/// Übermittlung von geänderten Stammdaten in vorzugebenden Fristen, die
/// insgesamt **10 Werktage** nicht überschreiten, … sicherstellt" — GeLi Gas
/// 3.0 Kap. 4.3. Five times the Strom window
/// ([`mako_fristen::antwort::STAMMDATEN_RUECKMELDUNG_WERKTAGE`]) and genuinely
/// so: Gas gives the Berechtigter a real Zustimmung/Ablehnung, where Strom has
/// only asynchronous quality feedback.
pub const ANTWORT_WINDOW_LABEL: &str = "geli-gas-stammdaten-antwort-window";

/// Change-family `(Änderung PID, Antwort PID, bilanzierungsrelevant)` rows
/// (G1–G7). The Antwort PID is shared across sender directions; the E15/E13/E17
/// outcome is carried in the message, not in the PID.
pub const STAMMDATEN_PAIRS: &[(u32, u32, bool)] = &[
    // G1 — nicht-bilanzierungsrelevante Änderung vom NB
    (44112, 44115, false), // NB → LF
    (44113, 44115, false), // NB → MSB
    // G2 — bilanzierungsrelevante Änderung vom NB (Monatserster)
    (44123, 44124, true), // NB → LF
    // G3 — Änderung der Marktlokationsstruktur vom NB
    (44175, 44176, false), // NB → LF
    // G4 — nicht-bilanzierungsrelevante Änderung vom LF
    (44109, 44111, false), // LF → NB
    // G5 — bilanzierungsrelevante Änderung vom LF (Monatserster)
    (44120, 44121, true), // LF → NB
    // G6 — Änderung vom MSB (mit Abhängigkeiten)
    (44116, 44119, false), // MSB → NB
    (44117, 44119, false), // MSB → LF
    // G7 — Änderung vom MSB (ohne Abhängigkeiten)
    (44159, 44161, false), // MSB → NB
    (44160, 44161, false), // NB → LF (weitergeleitet)
];

/// G8–G10 Anfrage-family PIDs — the full set routed to this workflow (requests
/// plus their Antwort/Ablehnung PIDs). The request PIDs we answer as data owner
/// are in [`ANFRAGE_ANTWORT_PAIRS`]; the remaining Antwort/Ablehnung PIDs are the
/// requester-side responses (initiating an Anfrage ourselves is a follow-up).
/// Excludes 44168–44170 (WiM Gas Verpflichtungsanfrage) and 44183 („Ende MSB").
pub const ANFRAGE_PIDS: &[u32] = &[
    // G8 — Anfrage zur Stammdatenänderung an NB (+ Antworten/Ablehnungen)
    44139, 44140, 44142, 44156, 44157, 44180, 44181, 44182, // G9 — Anfrage an LF
    44150, 44151, 44152, 44137, 44138, // G10 — Anfrage an MSB
    44162, 44163, 44164, 44143, 44145, 44146, 44165, 44167, 44147, 44149, 44166, 44148,
];

/// Stammdatenanfrage round-trip rows `(Anfrage PID, Antwort PID)` — the data
/// owner answers an inbound Anfrage with the requested master data (GeLiGas AWH
/// V1.2 §5.12–5.13, „Stammdatenanfrage vom Berechtigten aus gestartet").
///
/// The Antwort is a **data-return**: the current master data of the requested
/// Marktlokation is rendered into the Antwort PID. An Ablehnung (where a family
/// defines one) is carried as the SG4 STS status on the Antwort PID.
pub const ANFRAGE_ANTWORT_PAIRS: &[(u32, u32)] = &[
    // §5.12.1 — nicht bila.rel. Anfrage an NB
    (44139, 44142), // LF → NB
    (44140, 44142), // MSB → NB
    // §5.12.4 — nicht bila.rel. Anfrage an LF
    (44137, 44138), // → LF
    // §5.12.5 — bila.rel. Anfrage an LF
    (44150, 44151), // → LF (Ablehnung 44152 on the Antwort STS)
    // §5.12.3 — Anfrage der Marktlokationsstruktur / SDÄ
    (44156, 44157),
    (44180, 44181), // (Ablehnung 44182)
    // §5.13.5 — Anfrage an MSB
    (44162, 44163), // LF → MSB (Ablehnung 44164)
    (44165, 44166), // NB → MSB (Ablehnung 44167)
];

/// Return the Antwort (data-return) PID for an inbound Anfrage PID.
#[must_use]
pub fn antwort_for_anfrage(anfrage_pid: u32) -> Option<u32> {
    ANFRAGE_ANTWORT_PAIRS
        .iter()
        .find(|(a, _)| *a == anfrage_pid)
        .map(|(_, r)| *r)
}

/// `true` when `pid` is a Stammdatenanfrage request PID this deployment answers.
#[must_use]
pub fn is_anfrage_request_pid(pid: u32) -> bool {
    ANFRAGE_ANTWORT_PAIRS.iter().any(|(a, _)| *a == pid)
}

/// `true` when `pid` is a Stammdatenanfrage **data-return** PID — the answer a
/// requester receives, not a change it must apply.
///
/// mako implements only the answering side of the G8–G10 Anfrage round-trip
/// (there is no `SendAnfrage` command), so no counterparty should send one of
/// these unsolicited. It is separated out anyway because the alternative is
/// worse than dropping it: these PIDs are neither `is_antwort_pid` nor
/// `is_anfrage_request_pid`, so without this predicate they fall through to the
/// Änderung branch and a *data-return* would be **applied as a master-data
/// change**.
#[must_use]
pub fn is_anfrage_response_pid(pid: u32) -> bool {
    ANFRAGE_ANTWORT_PAIRS.iter().any(|(_, r)| *r == pid)
}

/// Return `(Antwort PID, bilanzierungsrelevant)` for an inbound Änderung PID.
#[must_use]
pub fn antwort_for(aenderung_pid: u32) -> Option<(u32, bool)> {
    STAMMDATEN_PAIRS
        .iter()
        .find(|(a, _, _)| *a == aenderung_pid)
        .map(|(_, r, bila)| (*r, *bila))
}

/// `true` when `pid` is an Änderung PID (inbound change → apply + Antwort).
#[must_use]
pub fn is_aenderung_pid(pid: u32) -> bool {
    STAMMDATEN_PAIRS.iter().any(|(a, _, _)| *a == pid)
}

/// `true` when `pid` is an Antwort PID (resumes a change we initiated).
#[must_use]
pub fn is_antwort_pid(pid: u32) -> bool {
    STAMMDATEN_PAIRS.iter().any(|(_, r, _)| *r == pid)
}

/// `true` when `datum` (`YYYYMMDD`) is a Monatserster.
#[must_use]
pub fn is_monatserster(datum: &str) -> bool {
    datum.len() == 8 && datum.ends_with("01")
}

// ── Antwort outcome ─────────────────────────────────────────────────────────────

/// The Antwort outcome (EBD E_3010 family). Gas — unlike Strom — can reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GasAntwort {
    /// E15 — Zustimmung ohne Korrekturen (change applied).
    Zustimmung,
    /// E13 — Ablehnung wegen Bilanzierungsproblem.
    AblehnungBilanzierung,
    /// E17 — Ablehnung wegen Fristüberschreitung / kein Monatserster.
    AblehnungFrist,
}

impl GasAntwort {
    /// EBD status code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Zustimmung => "E15",
            Self::AblehnungBilanzierung => "E13",
            Self::AblehnungFrist => "E17",
        }
    }

    /// `true` for the accept outcome.
    #[must_use]
    pub fn is_zustimmung(self) -> bool {
        matches!(self, Self::Zustimmung)
    }
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas Stammdatenänderung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasStammdatenEvent {
    /// Inbound Änderung received (we are the Berechtigter).
    AenderungErhalten {
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// GLN of the Verantwortlicher.
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Änderung PID.
        pruefidentifikator: Pruefidentifikator,
        /// `true` for bilanzierungsrelevante changes (Monatserster required).
        bilanzierungsrelevant: bool,
        /// Änderungsdatum (`YYYYMMDD`).
        aenderungsdatum: String,
        /// The MaLo attribute patch to apply **iff** the Antwort is Zustimmung.
        patch: serde_json::Value,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Structural validation passed.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Our Antwort dispatched (Zustimmung or Ablehnung).
    AntwortGesendet {
        /// Antwort PID.
        response_pid: Pruefidentifikator,
        /// E15 / E13 / E17.
        antwort: GasAntwort,
    },
    /// Verantwortlicher: we dispatched an Änderung (e.g. LF → NB 44109).
    AenderungGesendet {
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// GLN of the Verantwortlicher (us).
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Änderung PID.
        pruefidentifikator: Pruefidentifikator,
        /// Bila-relevance.
        bilanzierungsrelevant: bool,
        /// Änderungsdatum.
        aenderungsdatum: String,
    },
    /// Initiator: the Berechtigter's Antwort arrived.
    AntwortErhalten {
        /// Antwort PID.
        response_pid: Pruefidentifikator,
        /// E15 / E13 / E17.
        antwort: GasAntwort,
    },
    /// Data owner: a Stammdatenanfrage was answered with the requested master
    /// data (auto data-return).
    AnfrageBeantwortet {
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// Anfrage PID.
        anfrage_pid: Pruefidentifikator,
        /// Antwort (data-return) PID.
        antwort_pid: Pruefidentifikator,
    },
    /// Structural validation failed — APERAK dispatched.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired without an Antwort.
    DeadlineExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for GasStammdatenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AenderungErhalten { .. } => "GasStammdatenAenderungErhalten",
            Self::ValidationPassed { .. } => "GasStammdatenValidationPassed",
            Self::AntwortGesendet { .. } => "GasStammdatenAntwortGesendet",
            Self::AenderungGesendet { .. } => "GasStammdatenAenderungGesendet",
            Self::AntwortErhalten { .. } => "GasStammdatenAntwortErhalten",
            Self::AnfrageBeantwortet { .. } => "GasStammdatenAnfrageBeantwortet",
            Self::Rejected { .. } => "GasStammdatenRejected",
            Self::DeadlineExpired { .. } => "GasStammdatenDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured when the process starts (either role).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasStammdatenData {
    /// Gas-Marktlokation.
    pub location_id: MaLo,
    /// GLN of the Verantwortlicher.
    pub sender: MarktpartnerCode,
    /// GLN of the Berechtigter.
    pub receiver: MarktpartnerCode,
    /// Änderung PID.
    pub pruefidentifikator: Pruefidentifikator,
    /// Bila-relevance.
    pub bilanzierungsrelevant: bool,
    /// Änderungsdatum (`YYYYMMDD`).
    pub aenderungsdatum: String,
    /// The MaLo attribute patch, applied on Zustimmung. Empty for the
    /// initiator side and for non-MaLo objects.
    #[serde(default)]
    pub patch: serde_json::Value,
}

/// State of a GeLi Gas Stammdatenänderung process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GasStammdatenState {
    /// No events yet.
    #[default]
    New,
    /// Berechtigter: inbound Änderung received.
    Eingegangen(GasStammdatenData),
    /// Berechtigter: validation passed; Antwort not yet sent.
    ValidationPassed(GasStammdatenData),
    /// Berechtigter: Antwort dispatched (terminal).
    Beantwortet {
        /// Data from the Änderung.
        data: GasStammdatenData,
        /// Outcome.
        antwort: GasAntwort,
    },
    /// Data owner: a Stammdatenanfrage was answered (terminal, data-return sent).
    AnfrageBeantwortet {
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// Anfrage PID.
        anfrage_pid: Pruefidentifikator,
        /// Antwort (data-return) PID.
        antwort_pid: Pruefidentifikator,
    },
    /// Verantwortlicher: Änderung dispatched; awaiting Antwort.
    Gesendet(GasStammdatenData),
    /// Verantwortlicher: closed by the Berechtigter's Antwort.
    Abgeschlossen {
        /// Data from the Änderung.
        data: GasStammdatenData,
        /// Outcome.
        antwort: GasAntwort,
    },
    /// Process rejected (structural validation failure or timeout).
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl GasStammdatenState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Beantwortet { .. } => "Beantwortet",
            Self::AnfrageBeantwortet { .. } => "AnfrageBeantwortet",
            Self::Gesendet(_) => "Gesendet",
            Self::Abgeschlossen { .. } => "Abgeschlossen",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// `true` when terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Beantwortet { .. }
                | Self::AnfrageBeantwortet { .. }
                | Self::Abgeschlossen { .. }
                | Self::Rejected { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas Stammdatenänderung workflow.
#[derive(Clone)]
pub enum GasStammdatenCommand {
    /// Berechtigter: inbound UTILMD G Änderung received.
    ReceiveAenderung {
        /// Änderung PID.
        pid: Pruefidentifikator,
        /// GLN of the Verantwortlicher.
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// Änderungsdatum (`YYYYMMDD`).
        aenderungsdatum: String,
        /// The applied MaLo attribute patch (JSON; only for MaLo objects).
        patch: serde_json::Value,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if structural validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
        /// Receipt timestamp (drives the APERAK + Antwort deadlines).
        received_at: time::OffsetDateTime,
    },
    /// Berechtigter: send the Antwort (Zustimmung or Ablehnung).
    ///
    /// The ingest auto-path applies the Monatserster rule for bila.rel.
    /// changes and defaults otherwise to Zustimmung; an operator may override.
    SendAntwort {
        /// E15 / E13 / E17.
        antwort: GasAntwort,
    },
    /// Verantwortlicher: dispatch an Änderung (ERP-initiated, e.g. LF → NB).
    SendAenderung {
        /// Änderung PID.
        pid: Pruefidentifikator,
        /// GLN of the Verantwortlicher (us).
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// Änderungsdatum (`YYYYMMDD`).
        aenderungsdatum: String,
    },
    /// Verantwortlicher: the Berechtigter's Antwort arrived.
    ReceiveAntwort {
        /// Antwort PID.
        response_pid: Pruefidentifikator,
        /// E15 / E13 / E17.
        antwort: GasAntwort,
    },
    /// Data owner: inbound Stammdatenanfrage received — answer with the
    /// requested master data (auto data-return) or an Ablehnung.
    ReceiveAnfrage {
        /// Anfrage PID.
        pid: Pruefidentifikator,
        /// GLN of the Berechtigter (requester).
        sender: MarktpartnerCode,
        /// GLN of the data owner (us).
        receiver: MarktpartnerCode,
        /// Gas-Marktlokation the data is requested for.
        location_id: MaLo,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if structural validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
        /// Receipt timestamp (drives the APERAK + Antwort deadlines).
        received_at: time::OffsetDateTime,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for GasStammdatenCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// Build the `ProcessCompleted` outbox payload that drives the marktd apply
/// (only on Zustimmung of a MaLo change).
fn apply_outbox(data: &GasStammdatenData, patch: &serde_json::Value) -> PendingOutbox {
    PendingOutbox::new(
        "ProcessCompleted",
        "",
        serde_json::json!({
            "pid":              data.pruefidentifikator.as_u32(),
            "malo_id":          data.location_id.as_str(),
            "objekt":           "MARKTLOKATION",
            "aenderungsdatum":  data.aenderungsdatum,
            "stammdaten_patch": patch,
        }),
    )
}

/// GeLi Gas Stammdatenänderung workflow (PIDs 44109–44182).
pub struct GeliGasStammdatenaenderungWorkflow;

impl Workflow for GeliGasStammdatenaenderungWorkflow {
    type State = GasStammdatenState;
    type Event = GasStammdatenEvent;
    type Command = GasStammdatenCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                ANTWORT_WINDOW_LABEL | APERAK_GAS_FOLGEPROZESS_LABEL,
                GasStammdatenState::Eingegangen(_)
                | GasStammdatenState::ValidationPassed(_)
                | GasStammdatenState::Gesendet(_),
            ) => Some(GasStammdatenCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GasStammdatenEvent::AenderungErhalten {
                location_id,
                sender,
                receiver,
                pruefidentifikator,
                bilanzierungsrelevant,
                aenderungsdatum,
                patch,
                ..
            } => GasStammdatenState::Eingegangen(GasStammdatenData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
                bilanzierungsrelevant: *bilanzierungsrelevant,
                aenderungsdatum: aenderungsdatum.clone(),
                patch: patch.clone(),
            }),
            GasStammdatenEvent::ValidationPassed { .. } => match state {
                GasStammdatenState::Eingegangen(data) => GasStammdatenState::ValidationPassed(data),
                other => other,
            },
            GasStammdatenEvent::AntwortGesendet { antwort, .. } => match state {
                GasStammdatenState::ValidationPassed(data) => GasStammdatenState::Beantwortet {
                    data,
                    antwort: *antwort,
                },
                other => other,
            },
            GasStammdatenEvent::AenderungGesendet {
                location_id,
                sender,
                receiver,
                pruefidentifikator,
                bilanzierungsrelevant,
                aenderungsdatum,
            } => GasStammdatenState::Gesendet(GasStammdatenData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
                bilanzierungsrelevant: *bilanzierungsrelevant,
                aenderungsdatum: aenderungsdatum.clone(),
                patch: serde_json::Value::Null,
            }),
            GasStammdatenEvent::AntwortErhalten { antwort, .. } => match state {
                GasStammdatenState::Gesendet(data) => GasStammdatenState::Abgeschlossen {
                    data,
                    antwort: *antwort,
                },
                other => other,
            },
            GasStammdatenEvent::AnfrageBeantwortet {
                location_id,
                anfrage_pid,
                antwort_pid,
            } => GasStammdatenState::AnfrageBeantwortet {
                location_id: location_id.clone(),
                anfrage_pid: *anfrage_pid,
                antwort_pid: *antwort_pid,
            },
            GasStammdatenEvent::Rejected { reason } => GasStammdatenState::Rejected {
                reason: reason.clone(),
            },
            GasStammdatenEvent::DeadlineExpired { label, .. } => {
                if state.is_terminal() {
                    state
                } else {
                    GasStammdatenState::Rejected {
                        reason: format!("Antwort-Frist verstrichen: {label}"),
                    }
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            // ── Berechtigter: inbound Änderung ───────────────────────────────
            GasStammdatenCommand::ReceiveAenderung {
                pid,
                sender,
                receiver,
                location_id,
                aenderungsdatum,
                patch,
                message_ref,
                validation_passed,
                validation_errors,
                received_at,
            } => {
                if !matches!(state, GasStammdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let Some((_, bilanzierungsrelevant)) = antwort_for(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a GeLi Gas Änderung PID",
                    )));
                };
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![GasStammdatenEvent::AenderungErhalten {
                    location_id,
                    sender,
                    receiver,
                    pruefidentifikator: pid,
                    bilanzierungsrelevant,
                    aenderungsdatum: aenderungsdatum.clone(),
                    patch: patch.clone(),
                    message_ref: message_ref.clone(),
                }];

                if !validation_passed {
                    let reason = if validation_errors.is_empty() {
                        "structural validation failed".to_owned()
                    } else {
                        validation_errors.join("; ")
                    };
                    events.push(GasStammdatenEvent::Rejected {
                        reason: reason.clone(),
                    });
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
                    return Ok(WorkflowOutput::with_outbox(events, outbox));
                }

                events.push(GasStammdatenEvent::ValidationPassed { message_ref });
                let outbox = vec![
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
                let deadlines = vec![
                    PendingDeadline::new(
                        APERAK_GAS_FOLGEPROZESS_LABEL,
                        aperak_gas_folgeprozess_due_at(received_at),
                    ),
                    PendingDeadline::new(
                        ANTWORT_WINDOW_LABEL,
                        deadline_at_werktage(
                            received_at,
                            mako_fristen::antwort::STAMMDATEN_ANTWORT_WERKTAGE_GAS,
                            HolidayCalendar::BdewMaKo,
                        ),
                    ),
                ];
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    events, outbox, deadlines,
                ))
            }

            // ── Data owner: inbound Stammdatenanfrage → auto data-return ──────
            GasStammdatenCommand::ReceiveAnfrage {
                pid,
                sender,
                receiver,
                location_id,
                message_ref: _,
                validation_passed,
                validation_errors,
                received_at: _,
            } => {
                if !matches!(state, GasStammdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let Some(antwort_code) = antwort_for_anfrage(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a GeLi Gas Stammdatenanfrage PID",
                    )));
                };
                let antwort_pid = Pruefidentifikator::new(antwort_code)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;
                let requester = sender.clone();
                let owner = receiver.clone();

                if !validation_passed {
                    let reason = if validation_errors.is_empty() {
                        "structural validation failed".to_owned()
                    } else {
                        validation_errors.join("; ")
                    };
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            requester.as_str(),
                            serde_json::json!({
                                "sender":     owner.as_str(),
                                "receiver":   requester.as_str(),
                                "pid":        29001_u32,
                                "error_code": mako_engine::erc::codes::Z29,
                                "reason":     reason.clone(),
                            }),
                        )
                        .caused_by(0),
                    ];
                    return Ok(WorkflowOutput::with_outbox(
                        vec![GasStammdatenEvent::Rejected { reason }],
                        outbox,
                    ));
                }

                // Auto data-return: APERAK 312 receipt + the Antwort PID carrying
                // the requested Marktlokation's current master data (rendered by
                // makod's outbox worker from marktd).
                let outbox = vec![
                    PendingOutbox::new(
                        "APERAK",
                        requester.as_str(),
                        serde_json::json!({
                            "sender":        owner.as_str(),
                            "receiver":      requester.as_str(),
                            "pid":           29001_u32,
                            "document_code": "312",
                        }),
                    )
                    .caused_by(0),
                    PendingOutbox::new(
                        "UTILMD",
                        requester.as_str(),
                        serde_json::json!({
                            "direction":   "outbound",
                            "pid":         antwort_pid.as_u32(),
                            "sender":      owner.as_str(),
                            "receiver":    requester.as_str(),
                            "malo":        location_id.as_str(),
                            "objekt":      "MARKTLOKATION",
                            "data_return": true,
                        }),
                    )
                    .caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![GasStammdatenEvent::AnfrageBeantwortet {
                        location_id,
                        anfrage_pid: pid,
                        antwort_pid,
                    }],
                    outbox,
                ))
            }

            GasStammdatenCommand::SendAntwort { antwort } => {
                let data = match state {
                    GasStammdatenState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                // Monatserster guard: a bila.rel. change dated other than the
                // first of a month must be rejected E17, never accepted.
                let antwort = if data.bilanzierungsrelevant
                    && antwort.is_zustimmung()
                    && !is_monatserster(&data.aenderungsdatum)
                {
                    GasAntwort::AblehnungFrist
                } else {
                    antwort
                };
                let (response_code, _) =
                    antwort_for(data.pruefidentifikator.as_u32()).ok_or_else(|| {
                        WorkflowError::rejected(format!(
                            "no Antwort PID for Änderung {}",
                            data.pruefidentifikator
                        ))
                    })?;
                let response_pid = Pruefidentifikator::new(response_code)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;

                let mut outbox = vec![PendingOutbox::new(
                    "UTILMD",
                    data.sender.as_str(),
                    serde_json::json!({
                        "direction":    "outbound",
                        "pid":          response_pid.as_u32(),
                        "sender":       data.receiver.as_str(),
                        "receiver":     data.sender.as_str(),
                        "malo":         data.location_id.as_str(),
                        "process_date": data.aenderungsdatum,
                        "pruefung":      antwort.code(),
                    }),
                )];
                // Apply to marktd only on Zustimmung (the patch captured at
                // ingest). Ablehnung → no apply.
                if antwort.is_zustimmung() && data.patch.as_object().is_some_and(|o| !o.is_empty())
                {
                    outbox.push(apply_outbox(data, &data.patch));
                }
                Ok(WorkflowOutput::with_outbox(
                    vec![GasStammdatenEvent::AntwortGesendet {
                        response_pid,
                        antwort,
                    }],
                    outbox,
                ))
            }

            // ── Verantwortlicher: outbound Änderung ──────────────────────────
            GasStammdatenCommand::SendAenderung {
                pid,
                sender,
                receiver,
                location_id,
                aenderungsdatum,
            } => {
                if !matches!(state, GasStammdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let Some((_, bilanzierungsrelevant)) = antwort_for(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a GeLi Gas Änderung PID",
                    )));
                };
                // A bila.rel. change must carry a Monatserster Änderungsdatum.
                if bilanzierungsrelevant && !is_monatserster(&aenderungsdatum) {
                    return Err(WorkflowError::rejected(format!(
                        "bilanzierungsrelevante Änderung requires a Monatserster \
                         Änderungsdatum (YYYYMM01), got {aenderungsdatum}",
                    )));
                }
                let utilmd = PendingOutbox::new(
                    "UTILMD",
                    receiver.as_str(),
                    serde_json::json!({
                        "direction":    "outbound",
                        "pid":          pid.as_u32(),
                        "sender":       sender.as_str(),
                        "receiver":     receiver.as_str(),
                        "malo":         location_id.as_str(),
                        "process_date": aenderungsdatum,
                    }),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![GasStammdatenEvent::AenderungGesendet {
                        location_id,
                        sender,
                        receiver,
                        pruefidentifikator: pid,
                        bilanzierungsrelevant,
                        aenderungsdatum,
                    }],
                    vec![utilmd],
                ))
            }

            GasStammdatenCommand::ReceiveAntwort {
                response_pid,
                antwort,
            } => {
                if !matches!(state, GasStammdatenState::Gesendet(_)) {
                    return Err(WorkflowError::invalid_state("Gesendet", state.label()));
                }
                if !is_antwort_pid(response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {response_pid} is not a GeLi Gas Antwort PID",
                    )));
                }
                Ok(vec![GasStammdatenEvent::AntwortErhalten {
                    response_pid,
                    antwort,
                }]
                .into())
            }

            GasStammdatenCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    Ok(vec![].into())
                } else if label.as_ref() == ANTWORT_WINDOW_LABEL {
                    Ok(vec![GasStammdatenEvent::DeadlineExpired { deadline_id, label }].into())
                } else {
                    Ok(vec![].into())
                }
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
    fn malo(s: &str) -> MaLo {
        MaLo::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }
    fn now() -> time::OffsetDateTime {
        time::macros::datetime!(2026-07-01 10:00:00 UTC)
    }

    fn receive_cmd(aenderung_pid: u32, datum: &str, ok: bool) -> GasStammdatenCommand {
        GasStammdatenCommand::ReceiveAenderung {
            pid: pid(aenderung_pid),
            sender: mcod("9800357000004"),
            receiver: mcod("9800357000011"),
            location_id: malo("51238696781"),
            aenderungsdatum: datum.to_owned(),
            patch: serde_json::json!({ "gasqualitaet": "HGAS" }),
            message_ref: mref("GSD-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
            received_at: now(),
        }
    }

    fn apply_all(init: GasStammdatenState, events: &[GasStammdatenEvent]) -> GasStammdatenState {
        events
            .iter()
            .fold(init, GeliGasStammdatenaenderungWorkflow::apply)
    }

    #[test]
    fn classification() {
        assert_eq!(antwort_for(44112), Some((44115, false)));
        assert_eq!(antwort_for(44123), Some((44124, true))); // bila.rel.
        assert!(is_aenderung_pid(44109));
        assert!(is_antwort_pid(44115));
        // WiM Gas Verpflichtungsanfrage is excluded.
        assert!(!is_aenderung_pid(44168));
        assert!(antwort_for(44168).is_none());
    }

    #[test]
    fn no_duplicate_aenderung_pids() {
        let mut seen = std::collections::HashSet::new();
        for (a, _, _) in STAMMDATEN_PAIRS {
            assert!(seen.insert(*a), "duplicate Änderung PID {a}");
        }
        // Anfrage PIDs must not overlap the change families.
        for a in ANFRAGE_PIDS {
            assert!(!is_aenderung_pid(*a) && !is_antwort_pid(*a), "overlap {a}");
        }
    }

    #[test]
    fn nicht_bila_change_accepted_e15() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44112, "20260715", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::SendAntwort {
                antwort: GasAntwort::Zustimmung,
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["pid"], 44115);
        assert_eq!(out.outbox[0].payload["pruefung"], "E15");
        // Zustimmung → apply outbox present.
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[1].message_type.as_ref(), "ProcessCompleted");
    }

    #[test]
    fn bila_change_non_monatserster_rejected_e17() {
        // 44123 is bila.rel.; dated 2026-07-15 (not a Monatserster) → E17 even
        // when the operator asks for Zustimmung.
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44123, "20260715", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::SendAntwort {
                antwort: GasAntwort::Zustimmung,
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["pruefung"], "E17");
        // Ablehnung → no apply outbox.
        assert_eq!(out.outbox.len(), 1);
    }

    #[test]
    fn bila_change_monatserster_accepted() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44123, "20260801", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::SendAntwort {
                antwort: GasAntwort::Zustimmung,
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["pruefung"], "E15");
    }

    #[test]
    fn explicit_ablehnung_bilanzierung_e13() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44112, "20260715", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::SendAntwort {
                antwort: GasAntwort::AblehnungBilanzierung,
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["pruefung"], "E13");
        assert_eq!(out.outbox.len(), 1); // no apply on Ablehnung
    }

    #[test]
    fn initiator_bila_requires_monatserster() {
        let result = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            GasStammdatenCommand::SendAenderung {
                pid: pid(44120), // bila.rel. LF → NB
                sender: mcod("9800357000011"),
                receiver: mcod("9800357000004"),
                location_id: malo("51238696781"),
                aenderungsdatum: "20260715".to_owned(), // not Monatserster
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn initiator_send_receive_roundtrip() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            GasStammdatenCommand::SendAenderung {
                pid: pid(44109), // nicht-bila LF → NB
                sender: mcod("9800357000011"),
                receiver: mcod("9800357000004"),
                location_id: malo("51238696781"),
                aenderungsdatum: "20260715".to_owned(),
            },
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        assert!(matches!(state, GasStammdatenState::Gesendet(_)));
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::ReceiveAntwort {
                response_pid: pid(44111),
                antwort: GasAntwort::Zustimmung,
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasStammdatenState::Abgeschlossen { .. }));
    }

    #[test]
    fn validation_failure_aperak_313() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44112, "20260715", false),
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["error_code"], "Z29");
        let state = apply_all(GasStammdatenState::New, &out.events);
        assert!(matches!(state, GasStammdatenState::Rejected { .. }));
    }

    #[test]
    fn antwort_timeout_rejects() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44112, "20260715", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasStammdatenState::Rejected { .. }));
    }

    #[test]
    fn monatserster_helper() {
        assert!(is_monatserster("20260801"));
        assert!(!is_monatserster("20260815"));
        assert!(!is_monatserster("2026080")); // wrong length
    }

    fn anfrage_cmd(anfrage_pid: u32, ok: bool) -> GasStammdatenCommand {
        GasStammdatenCommand::ReceiveAnfrage {
            pid: pid(anfrage_pid),
            sender: mcod("9800357000011"),   // requester (Berechtigter)
            receiver: mcod("9800357000004"), // us (data owner)
            location_id: malo("51238696781"),
            message_ref: mref("GSA-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
            received_at: now(),
        }
    }

    #[test]
    fn anfrage_antwort_mapping() {
        assert_eq!(antwort_for_anfrage(44139), Some(44142)); // LF → NB
        assert_eq!(antwort_for_anfrage(44140), Some(44142)); // MSB → NB
        assert_eq!(antwort_for_anfrage(44137), Some(44138)); // → LF
        assert_eq!(antwort_for_anfrage(44162), Some(44163)); // LF → MSB
        assert!(is_anfrage_request_pid(44139));
        assert!(!is_anfrage_request_pid(44112)); // an Änderung, not an Anfrage
    }

    #[test]
    fn anfrage_yields_data_return() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            anfrage_cmd(44139, true),
        )
        .unwrap();
        // One event: the Anfrage was answered with the mapped Antwort PID.
        assert!(matches!(
            out.events.as_slice(),
            [GasStammdatenEvent::AnfrageBeantwortet { antwort_pid, .. }]
                if antwort_pid.as_u32() == 44142
        ));
        // Outbox: APERAK 312 receipt + the UTILMD data-return (Antwort PID 44142).
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[0].message_type.as_ref(), "APERAK");
        assert_eq!(out.outbox[0].payload["document_code"], "312");
        assert_eq!(out.outbox[1].message_type.as_ref(), "UTILMD");
        assert_eq!(out.outbox[1].payload["pid"], 44142);
        assert_eq!(out.outbox[1].payload["data_return"], true);
        let state = apply_all(GasStammdatenState::New, &out.events);
        assert!(matches!(
            state,
            GasStammdatenState::AnfrageBeantwortet { .. }
        ));
        assert!(state.is_terminal());
    }

    #[test]
    fn anfrage_validation_failure_rejects() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            anfrage_cmd(44139, false),
        )
        .unwrap();
        assert!(matches!(
            out.events.as_slice(),
            [GasStammdatenEvent::Rejected { .. }]
        ));
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].payload["error_code"], "Z29");
    }
}
