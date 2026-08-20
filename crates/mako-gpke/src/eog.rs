//! GPKE Ersatz-/Grundversorgung (EoG) — Zuordnung by the Netzbetreiber.
//!
//! Covers the statutory fallback-supply process (GPKE Teil 2 Kap. 2.3):
//! every consuming Marktlokation must be assigned to exactly one
//! Bilanzkreis at all times. When a MaLo draws energy without an
//! assignable supply contract, the NB assigns it to the **E/G**
//! (Ersatz-/Grundversorger, §36 Abs. 2 EnWG) via UTILMD
//! "Anmeldung / Zuordnung EOG" — untermonatlich, into the future **and
//! retroactively**.
//!
//! This module implements **both perspectives**:
//!
//! - **NB (initiator)** — detects the supply gap, sends PID 55013 to the
//!   Grundversorger, and awaits Bestätigung 55014 / Ablehnung 55015. If
//!   the E/G does not answer in time, the NB **assigns anyway** using the
//!   default Bilanzkreis the E/G deposited via GPKE Teil 4 ("Übermittlung
//!   von Informationen") — silence never blocks the statutory fallback.
//! - **LF/E-G (responder)** — receives an inbound 55013 and answers with
//!   55014 (stating Ersatz- vs. Grundversorgung and the Bilanzkreis) or
//!   55015 (EBD E_0615 grounds: A02 keine Zuständigkeit, A04
//!   Doppelmeldung, A05 kein EoG-Fall).
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom S2.1 Kap. 8.6)
//!
//! | PID   | Process name (AHB)              | Direction |
//! |-------|---------------------------------|-----------|
//! | 55013 | Anmeldung / Zuordnung EOG       | NB → LF   |
//! | 55014 | Bestätigung EOG Anmeldung       | LF → NB   |
//! | 55015 | Ablehnung EOG Anmeldung         | LF → NB   |
//!
//! Pre-LFW24 these were 11013–11015; the Gas twin is 44013–44015
//! (`mako-geli-gas`, `GasProcessVariant::EogAnmeldung`). PIDs 55010–55012
//! are the **separate** "Anfrage zur Beendigung der Zuordnung"
//! (NB Abmeldeanfrage) use case, not EoG, and are handled by
//! [`super::beendigung_zuordnung::GpkeBeendigungZuordnungWorkflow`].
//!
//! # Transaktionsgrund (SG4 STS DE9013, Anmeldung)
//!
//! `Z02` Kündigung Lieferantenrahmenvertrag · `Z36` EoG aus Ein-/Auszug ·
//! `Z37` EoG wegen Einzug in Neuanlage · `Z39` EoG aus vorübergehendem
//! Anschluss · `ZC6` EoG aus Bilanzkreisschließung · `ZC7` EoG aufgrund
//! Erlöschen der Zuordnungsermächtigung · `ZT6`/`ZT7` EoG wegen Kündigung
//! durch LF/Kunde · `E06` vertragliche Ersatzbelieferung (bilateral, only
//! outside the statutory 3-month window or above Niederspannung) · `ZZD`
//! Übergangsversorgung (§38a EnWG, from 2026-04-01).
//!
//! # Ersatz- vs. Grundversorgung — decided by the E/G, not the NB
//!
//! The **Bestätigung 55014** carries the classification (SG10 CCI+Z36
//! "Versorgungsart": `ZC9` Ersatzversorgung / `ZD0` Grundversorgung /
//! `ZE3` Ersatzbelieferung / `ZZD` Übergangsversorgung) plus the
//! Bilanzkreis. The Anmeldung only states the cause and whether the
//! Anschlussnutzer is a Haushaltskunde (CCI `Z15`/`Z18`), which drives
//! the E/G's classification: §38 Ersatzversorgung applies ipso iure to
//! every NSP-Letztverbraucher; Grundversorgung (§36) only to
//! Haushaltskunden.
//!
//! After **three months** (§38 Abs. 4 S. 1 EnWG — counted from the
//! (possibly retroactive) Zuordnungsbeginn, not from detection) the
//! Ersatzversorgung ends by law; for Haushaltskunden the transition into
//! Grundversorgung happens **automatically without a market message**
//! (GPKE Teil 2 Kap. 2.3.2.1). The `processd` EoG timer owns that clock.
//!
//! # Fristen (GPKE Teil 2 Kap. 2.3, SD Schritte 1–3)
//!
//! - Anmeldung: future Zuordnungsbeginn → by 13:00 of the last Werktag
//!   before it; otherwise **unverzüglich** (retroactive allowed).
//! - Antwort: **15:00 at the ÜT** (future case) or 15:00 of the first
//!   Werktag after the ÜT. Modeled here as a deadline at 15:00
//!   Europe/Berlin of the next Werktag ([`eog_antwort_due_at`]).
//! - No answer → NB assigns anyway (15:00–16:00 window, default BK).
//!
//! # Regulatory basis
//!
//! - **§36 / §38 / §38a EnWG**, **§§2–3 StromGVV**
//! - **GPKE Teil 2 Kap. 2.3 (BK6-24-174)** — Beginn der Ersatz-/Grundversorgung
//! - **UTILMD AHB Strom S2.1/S2.2 Kap. 8.6**, **EBD E_0615**
//! - **APERAK AHB 1.0 §2.4.1** — Strom UTILMD 45-min APERAK Frist

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
    APERAK_STROM_WINDOW_LABEL, HolidayCalendar, aperak_strom_due_at, deadline_at_werktage,
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-eog";

/// Inbound Anfrage PID handled by [`GpkeEogWorkflow`] in the responder role.
pub const EOG_ANMELDUNG_PID: u32 = 55013;

/// All inbound PIDs routed to [`GpkeEogWorkflow`]:
/// 55013 spawns the responder role; 55014/55015 resume the initiator role.
pub const EOG_PIDS: &[u32] = &[55013, 55014, 55015];

/// Response PIDs (LF → NB): Bestätigung / Ablehnung.
pub const EOG_ANTWORT_PIDS: &[u32] = &[55014, 55015];

/// Deadline label for the E/G answer window (GPKE Teil 2 Kap. 2.3 SD Schritt 2).
pub const EOG_RESPONSE_WINDOW_LABEL: &str = "gpke-eog-response-window";

/// Derive the outbound ANTWORT PID for the EoG Anmeldung.
#[must_use]
pub fn eog_response_pid(accepted: bool) -> u32 {
    if accepted { 55014 } else { 55015 }
}

/// Antwort deadline: 15:00 Europe/Berlin of the first Werktag after receipt.
///
/// GPKE distinguishes a same-day 15:00 window (future Zuordnungsbeginn)
/// from 15:00 of the first Werktag after the ÜT. The next-Werktag bound is
/// the outer envelope of both and is used uniformly here; missing it never
/// blocks the Zuordnung (the NB assigns with the default Bilanzkreis).
#[must_use]
pub fn eog_antwort_due_at(received_at: time::OffsetDateTime) -> time::OffsetDateTime {
    // deadline_at_werktage anchors Berlin-local end-of-Werktag semantics;
    // one Werktag out is the outer envelope of the 15:00 windows.
    deadline_at_werktage(received_at, 1, HolidayCalendar::BdewMaKo)
}

// ── EoG classification ────────────────────────────────────────────────────────

/// Versorgungsart stated by the E/G in the Bestätigung 55014
/// (SG10 CCI+Z36, DE7037).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Versorgungsart {
    /// `ZC9` — §38 EnWG Ersatzversorgung (ipso iure, max. 3 months, NSP).
    Ersatzversorgung,
    /// `ZD0` — §36 EnWG Grundversorgung (Haushaltskunden in NSP).
    Grundversorgung,
    /// `ZE3` — vertragliche Ersatzbelieferung (bilateral agreement).
    Ersatzbelieferung,
    /// `ZZD` — §38a EnWG Übergangsversorgung (MSP/MD, from 2026-04-01).
    Uebergangsversorgung,
}

impl Versorgungsart {
    /// AHB code (SG10 CCI+Z36 DE7037).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Ersatzversorgung => "ZC9",
            Self::Grundversorgung => "ZD0",
            Self::Ersatzbelieferung => "ZE3",
            Self::Uebergangsversorgung => "ZZD",
        }
    }

    /// Parse from the AHB code.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "ZC9" => Some(Self::Ersatzversorgung),
            "ZD0" => Some(Self::Grundversorgung),
            "ZE3" => Some(Self::Ersatzbelieferung),
            "ZZD" => Some(Self::Uebergangsversorgung),
            _ => None,
        }
    }

    /// Stable wire label (used in outbox payloads and CloudEvents).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ersatzversorgung => "ERSATZVERSORGUNG",
            Self::Grundversorgung => "GRUNDVERSORGUNG",
            Self::Ersatzbelieferung => "ERSATZBELIEFERUNG",
            Self::Uebergangsversorgung => "UEBERGANGSVERSORGUNG",
        }
    }
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE EoG workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum EogEvent {
    /// NB (initiator) dispatched the UTILMD 55013 Zuordnung to the E/G.
    Angemeldet {
        /// Marktlokation.
        location_id: MaLo,
        /// GLN of the initiating NB.
        sender: MarktpartnerCode,
        /// GLN of the E/G (receiving LF).
        receiver: MarktpartnerCode,
        /// Zuordnungsbeginn (`YYYYMMDD`) — may be retroactive.
        process_date: String,
        /// BDEW Prüfidentifikator (55013).
        pruefidentifikator: Pruefidentifikator,
        /// SG4 STS Transaktionsgrund (e.g. `Z37`, `ZC7`).
        transaktionsgrund: String,
        /// Whether the Anschlussnutzer is a Haushaltskunde (CCI Z15/Z18),
        /// if known.
        haushaltskunde: Option<bool>,
    },
    /// NB (initiator) received the E/G's response (55014/55015).
    AntwortErhalten {
        /// Response PID: 55014 (Bestätigung) or 55015 (Ablehnung).
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung, `false` = Ablehnung.
        accepted: bool,
        /// Versorgungsart from the Bestätigung (CCI ZC9/ZD0/ZE3/ZZD).
        versorgungsart: Option<Versorgungsart>,
        /// Bilanzkreis (EIC) from the Bestätigung.
        bilanzkreis: Option<String>,
        /// Rejection reason / EBD code (A02/A04/A05) when rejected.
        reason: Option<String>,
    },
    /// NB (initiator): answer window expired — Zuordnung executed anyway
    /// with the E/G's pre-deposited default Bilanzkreis (GPKE Teil 2
    /// Kap. 2.3 SD Schritt 3).
    ZugeordnetOhneAntwort {
        /// The expired deadline.
        deadline_id: DeadlineId,
    },
    /// LF/E-G (responder): inbound PID 55013 Zuordnung received.
    AnmeldungErhalten {
        /// Marktlokation.
        location_id: MaLo,
        /// GLN of the sending NB.
        sender: MarktpartnerCode,
        /// GLN of the receiving LF (E/G).
        receiver: MarktpartnerCode,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Zuordnungsbeginn (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (55013).
        pruefidentifikator: Pruefidentifikator,
        /// SG4 STS Transaktionsgrund.
        transaktionsgrund: String,
        /// Haushaltskunde flag (CCI Z15/Z18), if transmitted.
        haushaltskunde: Option<bool>,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// LF/E-G (responder): outbound response (55014/55015) dispatched.
    AntwortGesendet {
        /// Response PID: 55014 (Bestätigung) or 55015 (Ablehnung).
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung, `false` = Ablehnung.
        accepted: bool,
        /// Versorgungsart stated in the Bestätigung.
        versorgungsart: Option<Versorgungsart>,
        /// Bilanzkreis (EIC) stated in the Bestätigung.
        bilanzkreis: Option<String>,
        /// Rejection reason / EBD code (A02/A04/A05) when rejected.
        reason: Option<String>,
    },
    /// Process rejected (validation failure or responder answer timeout).
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired (responder-side bookkeeping).
    DeadlineExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for EogEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Angemeldet { .. } => "EogAngemeldet",
            Self::AntwortErhalten { .. } => "EogAntwortErhalten",
            Self::ZugeordnetOhneAntwort { .. } => "EogZugeordnetOhneAntwort",
            Self::AnmeldungErhalten { .. } => "EogAnmeldungErhalten",
            Self::ValidationPassed { .. } => "EogValidationPassed",
            Self::AntwortGesendet { .. } => "EogAntwortGesendet",
            Self::Rejected { .. } => "EogRejected",
            Self::DeadlineExpired { .. } => "EogDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured when the process starts (either role).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EogData {
    /// Marktlokation.
    pub location_id: MaLo,
    /// GLN of the NB.
    pub sender: MarktpartnerCode,
    /// GLN of the E/G (LF).
    pub receiver: MarktpartnerCode,
    /// Zuordnungsbeginn (`YYYYMMDD`) — may be retroactive; anchors the
    /// §38 Abs. 4 three-month maximum.
    pub process_date: String,
    /// BDEW Prüfidentifikator (55013).
    pub pruefidentifikator: Pruefidentifikator,
    /// SG4 STS Transaktionsgrund.
    pub transaktionsgrund: String,
    /// Haushaltskunde flag (CCI Z15/Z18), if known.
    pub haushaltskunde: Option<bool>,
}

/// State of a GPKE EoG process.
///
/// # Lifecycle
///
/// ```text
/// NB (initiator):    New → Angemeldet → Zugeordnet            (55014 / timeout)
///                                      ↘ Abgelehnt             (55015)
/// LF/E-G (responder): New → Eingegangen → ValidationPassed
///                         → AntwortGesendet                    (55014/55015)
///                         ↘ Rejected (failed validation / answer timeout)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum EogState {
    /// No events yet.
    #[default]
    New,
    /// Initiator: UTILMD 55013 dispatched; awaiting the E/G's response.
    Angemeldet(EogData),
    /// Initiator: Zuordnung effective — either confirmed (55014) or
    /// executed without an answer (default Bilanzkreis).
    Zugeordnet {
        /// Data from the Anmeldung.
        data: EogData,
        /// Versorgungsart stated by the E/G. `None` when the Zuordnung was
        /// executed without an answer (classification then defaults to
        /// Ersatzversorgung ipso iure, §38 Abs. 1 EnWG).
        versorgungsart: Option<Versorgungsart>,
        /// Bilanzkreis (EIC). `None` = the pre-deposited default BK applies.
        bilanzkreis: Option<String>,
        /// `true` when assigned after the answer window expired.
        ohne_antwort: bool,
    },
    /// Initiator: E/G rejected (55015, EBD A02/A04/A05).
    Abgelehnt {
        /// Rejection reason.
        reason: String,
    },
    /// Responder: inbound Zuordnung received.
    Eingegangen(EogData),
    /// Responder: validation passed; response not yet sent.
    ValidationPassed(EogData),
    /// Responder: response dispatched.
    AntwortGesendet {
        /// Data from the Zuordnung.
        data: EogData,
        /// Response PID sent (55014 or 55015).
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung.
        accepted: bool,
        /// Versorgungsart stated in the Bestätigung.
        versorgungsart: Option<Versorgungsart>,
    },
    /// Process rejected (validation failure or answer timeout).
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for EogState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // Initiator side: awaiting the E/G, or the Zuordnung is effective
            // and the supply relationship is live.
            Self::Angemeldet(_) | Self::Zugeordnet { .. } => true,
            // Responder side: an inbound Zuordnung is being worked. Once the
            // answer is dispatched the responder's obligation is met, but the
            // Zuordnung it confirmed is live, so it still holds the MaLo.
            Self::Eingegangen(_) | Self::ValidationPassed(_) | Self::AntwortGesendet { .. } => true,
            // Terminal.
            Self::New | Self::Abgelehnt { .. } | Self::Rejected { .. } => false,
        }
    }
}

impl EogState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Angemeldet(_) => "Angemeldet",
            Self::Zugeordnet { .. } => "Zugeordnet",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AntwortGesendet { .. } => "AntwortGesendet",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Return `Some(&EogData)` if the process carries business data.
    #[must_use]
    pub fn data(&self) -> Option<&EogData> {
        match self {
            Self::Angemeldet(d) | Self::Eingegangen(d) | Self::ValidationPassed(d) => Some(d),
            Self::Zugeordnet { data, .. } | Self::AntwortGesendet { data, .. } => Some(data),
            Self::New | Self::Abgelehnt { .. } | Self::Rejected { .. } => None,
        }
    }

    /// `true` when the process is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Zugeordnet { .. }
                | Self::Abgelehnt { .. }
                | Self::AntwortGesendet { .. }
                | Self::Rejected { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE EoG workflow.
#[derive(Clone)]
pub enum EogCommand {
    /// NB (initiator): dispatch the UTILMD 55013 Zuordnung to the E/G.
    ///
    /// Triggered by the `gpke.eog.anmelden` ERP command (typically issued
    /// by the `processd` gap-closure automation).
    Anmelden {
        /// BDEW Prüfidentifikator (55013).
        pid: Pruefidentifikator,
        /// GLN of the initiating NB.
        sender: MarktpartnerCode,
        /// GLN of the E/G (Grundversorger).
        receiver: MarktpartnerCode,
        /// Marktlokation.
        location_id: MaLo,
        /// Zuordnungsbeginn (`YYYYMMDD`) — may be retroactive.
        process_date: String,
        /// SG4 STS Transaktionsgrund (Z02/Z36/Z37/Z39/ZC6/ZC7/ZT6/ZT7/E06/ZZD).
        transaktionsgrund: String,
        /// Haushaltskunde flag (CCI Z15/Z18), if known.
        haushaltskunde: Option<bool>,
    },
    /// NB (initiator): the E/G's response (55014/55015) arrived.
    ReceiveAntwort {
        /// Response PID (55014 or 55015).
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung (55014).
        accepted: bool,
        /// Versorgungsart from the Bestätigung (CCI ZC9/ZD0/ZE3/ZZD).
        versorgungsart: Option<Versorgungsart>,
        /// Bilanzkreis (EIC) from the Bestätigung.
        bilanzkreis: Option<String>,
        /// Rejection reason / EBD code (when rejected).
        reason: Option<String>,
    },
    /// LF/E-G (responder): inbound UTILMD 55013 received from the AS4 layer.
    ReceiveAnmeldung {
        /// BDEW Prüfidentifikator (55013).
        pid: Pruefidentifikator,
        /// GLN of the NB.
        sender: MarktpartnerCode,
        /// GLN of the LF (E/G).
        receiver: MarktpartnerCode,
        /// Marktlokation.
        location_id: MaLo,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Zuordnungsbeginn (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// SG4 STS Transaktionsgrund.
        transaktionsgrund: String,
        /// Haushaltskunde flag (CCI Z15/Z18), if transmitted.
        haushaltskunde: Option<bool>,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
        /// Receipt timestamp (drives the APERAK + answer deadlines).
        received_at: time::OffsetDateTime,
    },
    /// LF/E-G (responder): send the outbound response (55014/55015).
    SendAntwort {
        /// `true` = Bestätigung (55014), `false` = Ablehnung (55015).
        accepted: bool,
        /// Versorgungsart (required for a Bestätigung: ZC9/ZD0/ZE3/ZZD).
        versorgungsart: Option<Versorgungsart>,
        /// Bilanzkreis (EIC) the MaLo is assigned to (Bestätigung).
        bilanzkreis: Option<String>,
        /// Rejection reason / EBD code (required for an Ablehnung).
        reason: Option<String>,
    },
    /// A registered deadline fired.
    ///
    /// Initiator (`Angemeldet`): executes the Zuordnung without an answer
    /// (default Bilanzkreis) instead of failing — GPKE Teil 2 Kap. 2.3
    /// SD Schritt 3. Responder: closes the process as `Rejected` (the NB
    /// has assigned with the default BK on its side).
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for EogCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// Build the `ProcessCompleted` outbox payload that drives the marktd
/// VersorgungsStatus transition and the processd §38 timer.
fn process_completed_outbox(
    data: &EogData,
    versorgungsart: Option<Versorgungsart>,
    bilanzkreis: Option<&str>,
    ohne_antwort: bool,
) -> PendingOutbox {
    // Without an answer the classification defaults to Ersatzversorgung —
    // §38 Abs. 1 EnWG applies ipso iure to every NSP-Letztverbraucher.
    let art = versorgungsart.unwrap_or(Versorgungsart::Ersatzversorgung);
    PendingOutbox::new(
        "ProcessCompleted",
        "",
        serde_json::json!({
            "pid":               EOG_ANMELDUNG_PID,
            "malo_id":           data.location_id.as_str(),
            "new_supplier":      data.receiver.as_str(),
            "grid_operator":     data.sender.as_str(),
            "process_date":      data.process_date,
            "eog_art":           art.as_str(),
            "transaktionsgrund": data.transaktionsgrund,
            "haushaltskunde":    data.haushaltskunde,
            "bilanzkreis":       bilanzkreis,
            "ohne_antwort":      ohne_antwort,
        }),
    )
}

/// GPKE Ersatz-/Grundversorgung workflow (PIDs 55013–55015).
pub struct GpkeEogWorkflow;

impl Workflow for GpkeEogWorkflow {
    type State = EogState;
    type Event = EogEvent;
    type Command = EogCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                EOG_RESPONSE_WINDOW_LABEL | APERAK_STROM_WINDOW_LABEL,
                EogState::Angemeldet(_) | EogState::Eingegangen(_) | EogState::ValidationPassed(_),
            ) => Some(EogCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            EogEvent::Angemeldet {
                location_id,
                sender,
                receiver,
                process_date,
                pruefidentifikator,
                transaktionsgrund,
                haushaltskunde,
            } => EogState::Angemeldet(EogData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                transaktionsgrund: transaktionsgrund.clone(),
                haushaltskunde: *haushaltskunde,
            }),
            EogEvent::AntwortErhalten {
                accepted,
                versorgungsart,
                bilanzkreis,
                reason,
                ..
            } => match state {
                EogState::Angemeldet(data) => {
                    if *accepted {
                        EogState::Zugeordnet {
                            data,
                            versorgungsart: *versorgungsart,
                            bilanzkreis: bilanzkreis.clone(),
                            ohne_antwort: false,
                        }
                    } else {
                        EogState::Abgelehnt {
                            reason: reason
                                .clone()
                                .unwrap_or_else(|| "EoG Zuordnung abgelehnt".to_owned()),
                        }
                    }
                }
                other => other,
            },
            EogEvent::ZugeordnetOhneAntwort { .. } => match state {
                EogState::Angemeldet(data) => EogState::Zugeordnet {
                    data,
                    versorgungsart: None,
                    bilanzkreis: None,
                    ohne_antwort: true,
                },
                other => other,
            },
            EogEvent::AnmeldungErhalten {
                location_id,
                sender,
                receiver,
                process_date,
                pruefidentifikator,
                transaktionsgrund,
                haushaltskunde,
                ..
            } => EogState::Eingegangen(EogData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                transaktionsgrund: transaktionsgrund.clone(),
                haushaltskunde: *haushaltskunde,
            }),
            EogEvent::ValidationPassed { .. } => match state {
                EogState::Eingegangen(data) => EogState::ValidationPassed(data),
                other => other,
            },
            EogEvent::AntwortGesendet {
                response_pid,
                accepted,
                versorgungsart,
                ..
            } => match state {
                EogState::ValidationPassed(data) => EogState::AntwortGesendet {
                    data,
                    response_pid: *response_pid,
                    accepted: *accepted,
                    versorgungsart: *versorgungsart,
                },
                other => other,
            },
            EogEvent::Rejected { reason } => EogState::Rejected {
                reason: reason.clone(),
            },
            EogEvent::DeadlineExpired { label, .. } => {
                if state.is_terminal() {
                    state
                } else {
                    EogState::Rejected {
                        reason: format!("deadline expired: {label}"),
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
            // ── NB initiator ─────────────────────────────────────────────────
            EogCommand::Anmelden {
                pid,
                sender,
                receiver,
                location_id,
                process_date,
                transaktionsgrund,
                haushaltskunde,
            } => {
                if !matches!(state, EogState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if pid.as_u32() != EOG_ANMELDUNG_PID {
                    return Err(WorkflowError::rejected(format!(
                        "expected EoG Anmeldung PID ({EOG_ANMELDUNG_PID}), got {pid}",
                    )));
                }
                if transaktionsgrund.trim().is_empty() {
                    return Err(WorkflowError::rejected(
                        "EoG Anmeldung requires a Transaktionsgrund (SG4 STS DE9013)".to_owned(),
                    ));
                }

                // Outbound UTILMD 55013 — rendered by the makod EDIFACT
                // renderer (`message_type = "UTILMD"`).
                let utilmd = PendingOutbox::new(
                    "UTILMD",
                    receiver.as_str(),
                    serde_json::json!({
                        "direction":         "outbound",
                        "pid":               pid.as_u32(),
                        "sender":            sender.as_str(),
                        "receiver":          receiver.as_str(),
                        "malo":              location_id.as_str(),
                        "process_date":      process_date,
                        "transaktionsgrund": transaktionsgrund,
                    }),
                );
                // Notify observers (obsd, ERP) that the statutory fallback
                // process has been initiated.
                let initiated = PendingOutbox::new(
                    "ProcessInitiated",
                    receiver.as_str(),
                    serde_json::json!({
                        "pid":               pid.as_u32(),
                        "malo_id":           location_id.as_str(),
                        "new_supplier":      receiver.as_str(),
                        "grid_operator":     sender.as_str(),
                        "process_date":      process_date,
                        "transaktionsgrund": transaktionsgrund,
                    }),
                );

                let event = EogEvent::Angemeldet {
                    location_id,
                    sender,
                    receiver,
                    process_date,
                    pruefidentifikator: pid,
                    transaktionsgrund,
                    haushaltskunde,
                };
                Ok(WorkflowOutput::with_outbox(
                    vec![event],
                    vec![utilmd, initiated],
                ))
            }

            EogCommand::ReceiveAntwort {
                response_pid,
                accepted,
                versorgungsart,
                bilanzkreis,
                reason,
            } => {
                let data = match state {
                    EogState::Angemeldet(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state("Angemeldet", state.label()));
                    }
                };
                if !EOG_ANTWORT_PIDS.contains(&response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected EoG Antwort PID (55014/55015), got {response_pid}",
                    )));
                }
                let mut outbox = Vec::new();
                if accepted {
                    outbox.push(process_completed_outbox(
                        data,
                        versorgungsart,
                        bilanzkreis.as_deref(),
                        false,
                    ));
                }
                Ok(WorkflowOutput::with_outbox(
                    vec![EogEvent::AntwortErhalten {
                        response_pid,
                        accepted,
                        versorgungsart,
                        bilanzkreis,
                        reason,
                    }],
                    outbox,
                ))
            }

            // ── LF/E-G responder ─────────────────────────────────────────────
            EogCommand::ReceiveAnmeldung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                transaktionsgrund,
                haushaltskunde,
                validation_passed,
                validation_errors,
                received_at,
            } => {
                if !matches!(state, EogState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if pid.as_u32() != EOG_ANMELDUNG_PID {
                    return Err(WorkflowError::rejected(format!(
                        "expected EoG Anmeldung PID ({EOG_ANMELDUNG_PID}), got {pid}",
                    )));
                }
                // Clone before move for APERAK emission.
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![EogEvent::AnmeldungErhalten {
                    location_id,
                    sender,
                    receiver,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                    transaktionsgrund,
                    haushaltskunde,
                }];
                if validation_passed {
                    events.push(EogEvent::ValidationPassed { message_ref });
                    // APERAK BGM+312 (Anerkennung) — Strom UTILMD 45-min Frist
                    // (APERAK AHB 1.0 §2.4.1).
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
                            APERAK_STROM_WINDOW_LABEL,
                            aperak_strom_due_at(received_at),
                        ),
                        PendingDeadline::new(
                            EOG_RESPONSE_WINDOW_LABEL,
                            eog_antwort_due_at(received_at),
                        ),
                    ];
                    Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events, outbox, deadlines,
                    ))
                } else {
                    let reason = if validation_errors.is_empty() {
                        "AHB validation failed".to_owned()
                    } else {
                        validation_errors.join("; ")
                    };
                    events.push(EogEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // APERAK BGM+313 (Verarbeitbarkeitsfehler).
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

            EogCommand::SendAntwort {
                accepted,
                versorgungsart,
                bilanzkreis,
                reason,
            } => {
                let data = match state {
                    EogState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                if accepted && versorgungsart.is_none() {
                    return Err(WorkflowError::rejected(
                        "Bestätigung EOG requires the Versorgungsart (CCI ZC9/ZD0/ZE3/ZZD)"
                            .to_owned(),
                    ));
                }
                if !accepted && reason.is_none() {
                    return Err(WorkflowError::rejected(
                        "Ablehnung EOG requires a reason (EBD E_0615: A02/A04/A05)".to_owned(),
                    ));
                }
                let response_pid = Pruefidentifikator::new(eog_response_pid(accepted))
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;

                // Outbound UTILMD 55014/55015 to the NB.
                let mut outbox = vec![PendingOutbox::new(
                    "UTILMD",
                    data.sender.as_str(),
                    serde_json::json!({
                        "direction":      "outbound",
                        "pid":            response_pid.as_u32(),
                        "sender":         data.receiver.as_str(),
                        "receiver":       data.sender.as_str(),
                        "malo":           data.location_id.as_str(),
                        "process_date":   data.process_date,
                        "versorgungsart": versorgungsart.map(Versorgungsart::code),
                        "bilanzkreis":    bilanzkreis,
                        "reason":         reason,
                    }),
                )];
                if accepted {
                    // The E/G's own marktd records the fallback supply from
                    // this event (it is now the supplier of record).
                    outbox.push(process_completed_outbox(
                        data,
                        versorgungsart,
                        bilanzkreis.as_deref(),
                        false,
                    ));
                }
                Ok(WorkflowOutput::with_outbox(
                    vec![EogEvent::AntwortGesendet {
                        response_pid,
                        accepted,
                        versorgungsart,
                        bilanzkreis,
                        reason,
                    }],
                    outbox,
                ))
            }

            EogCommand::TimeoutExpired { deadline_id, label } => match state {
                // Initiator: silence never blocks the statutory fallback —
                // assign with the pre-deposited default Bilanzkreis
                // (GPKE Teil 2 Kap. 2.3 SD Schritt 3).
                EogState::Angemeldet(data) if label.as_ref() == EOG_RESPONSE_WINDOW_LABEL => {
                    Ok(WorkflowOutput::with_outbox(
                        vec![EogEvent::ZugeordnetOhneAntwort { deadline_id }],
                        vec![process_completed_outbox(data, None, None, true)],
                    ))
                }
                s if s.is_terminal() => Ok(vec![].into()),
                _ => Ok(vec![EogEvent::DeadlineExpired { deadline_id, label }].into()),
            },
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

    fn anmelden_cmd() -> EogCommand {
        EogCommand::Anmelden {
            pid: pid(55013),
            sender: mcod("9900357000004"),
            receiver: mcod("9900357000011"),
            location_id: malo("51238696781"),
            process_date: "20260615".to_owned(), // retroactive Zuordnungsbeginn
            transaktionsgrund: "ZT7".to_owned(), // Kündigung durch Kunde
            haushaltskunde: Some(true),
        }
    }

    fn receive_anmeldung_cmd(ok: bool) -> EogCommand {
        EogCommand::ReceiveAnmeldung {
            pid: pid(55013),
            sender: mcod("9900357000004"),
            receiver: mcod("9900357000011"),
            location_id: malo("51238696781"),
            document_date: "20260701".to_owned(),
            process_date: "20260615".to_owned(),
            message_ref: mref("EOG-001"),
            transaktionsgrund: "ZT7".to_owned(),
            haushaltskunde: Some(true),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
            received_at: now(),
        }
    }

    fn apply_all(init: EogState, events: &[EogEvent]) -> EogState {
        events.iter().fold(init, GpkeEogWorkflow::apply)
    }

    // ── NB initiator ──────────────────────────────────────────────────────────

    #[test]
    fn initiator_happy_path_zugeordnet() {
        let out = GpkeEogWorkflow::handle(&EogState::New, anmelden_cmd()).unwrap();
        assert_eq!(out.events.len(), 1);
        // UTILMD 55013 wire message + ProcessInitiated notification.
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[0].message_type.as_ref(), "UTILMD");
        assert_eq!(out.outbox[0].payload["transaktionsgrund"], "ZT7");
        assert_eq!(out.outbox[1].message_type.as_ref(), "ProcessInitiated");
        let state = apply_all(EogState::New, &out.events);
        assert!(matches!(state, EogState::Angemeldet(_)));

        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::ReceiveAntwort {
                response_pid: pid(55014),
                accepted: true,
                versorgungsart: Some(Versorgungsart::Ersatzversorgung),
                bilanzkreis: Some("11XGRUNDV-BK--I".to_owned()),
                reason: None,
            },
        )
        .unwrap();
        // ProcessCompleted drives marktd's Ersatzversorgung transition.
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].message_type.as_ref(), "ProcessCompleted");
        let payload = &out.outbox[0].payload;
        assert_eq!(payload["pid"], 55013);
        assert_eq!(payload["eog_art"], "ERSATZVERSORGUNG");
        assert_eq!(payload["process_date"], "20260615");
        assert_eq!(payload["ohne_antwort"], false);
        let state = apply_all(state, &out.events);
        assert!(matches!(
            state,
            EogState::Zugeordnet {
                versorgungsart: Some(Versorgungsart::Ersatzversorgung),
                ohne_antwort: false,
                ..
            }
        ));
    }

    #[test]
    fn initiator_grundversorgung_classification_from_antwort() {
        let out = GpkeEogWorkflow::handle(&EogState::New, anmelden_cmd()).unwrap();
        let state = apply_all(EogState::New, &out.events);
        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::ReceiveAntwort {
                response_pid: pid(55014),
                accepted: true,
                versorgungsart: Some(Versorgungsart::Grundversorgung),
                bilanzkreis: Some("11XGRUNDV-BK--I".to_owned()),
                reason: None,
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["eog_art"], "GRUNDVERSORGUNG");
    }

    #[test]
    fn initiator_ablehnung() {
        let out = GpkeEogWorkflow::handle(&EogState::New, anmelden_cmd()).unwrap();
        let state = apply_all(EogState::New, &out.events);
        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::ReceiveAntwort {
                response_pid: pid(55015),
                accepted: false,
                versorgungsart: None,
                bilanzkreis: None,
                reason: Some("A02".to_owned()),
            },
        )
        .unwrap();
        assert!(out.outbox.is_empty());
        let state = apply_all(state, &out.events);
        assert!(matches!(state, EogState::Abgelehnt { .. }));
    }

    #[test]
    fn initiator_timeout_assigns_with_default_bk() {
        // GPKE Teil 2 Kap. 2.3 SD Schritt 3: silence never blocks the
        // statutory fallback supply.
        let out = GpkeEogWorkflow::handle(&EogState::New, anmelden_cmd()).unwrap();
        let state = apply_all(EogState::New, &out.events);
        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: EOG_RESPONSE_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].message_type.as_ref(), "ProcessCompleted");
        // Classification defaults to Ersatzversorgung (ipso iure, §38 Abs. 1).
        assert_eq!(out.outbox[0].payload["eog_art"], "ERSATZVERSORGUNG");
        assert_eq!(out.outbox[0].payload["ohne_antwort"], true);
        let state = apply_all(state, &out.events);
        assert!(matches!(
            state,
            EogState::Zugeordnet {
                versorgungsart: None,
                ohne_antwort: true,
                ..
            }
        ));
    }

    #[test]
    fn initiator_requires_transaktionsgrund() {
        let result = GpkeEogWorkflow::handle(
            &EogState::New,
            EogCommand::Anmelden {
                pid: pid(55013),
                sender: mcod("9900357000004"),
                receiver: mcod("9900357000011"),
                location_id: malo("51238696781"),
                process_date: "20260701".to_owned(),
                transaktionsgrund: "  ".to_owned(),
                haushaltskunde: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn initiator_wrong_pid_rejected() {
        let result = GpkeEogWorkflow::handle(
            &EogState::New,
            EogCommand::Anmelden {
                pid: pid(55001),
                sender: mcod("9900357000004"),
                receiver: mcod("9900357000011"),
                location_id: malo("51238696781"),
                process_date: "20260701".to_owned(),
                transaktionsgrund: "ZT7".to_owned(),
                haushaltskunde: None,
            },
        );
        assert!(result.is_err());
    }

    // ── LF/E-G responder ──────────────────────────────────────────────────────

    #[test]
    fn responder_happy_path_bestaetigung() {
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(true)).unwrap();
        assert_eq!(out.events.len(), 2); // AnmeldungErhalten + ValidationPassed
        assert_eq!(out.deadlines.len(), 2); // APERAK 45-min + answer window
        let state = apply_all(EogState::New, &out.events);
        assert!(matches!(state, EogState::ValidationPassed(_)));

        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::SendAntwort {
                accepted: true,
                versorgungsart: Some(Versorgungsart::Grundversorgung),
                bilanzkreis: Some("11XGRUNDV-BK--I".to_owned()),
                reason: None,
            },
        )
        .unwrap();
        // UTILMD 55014 wire message + ProcessCompleted for the E/G's marktd.
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[0].message_type.as_ref(), "UTILMD");
        assert_eq!(out.outbox[0].payload["pid"], 55014);
        assert_eq!(out.outbox[0].payload["versorgungsart"], "ZD0");
        assert_eq!(out.outbox[1].message_type.as_ref(), "ProcessCompleted");
        assert_eq!(out.outbox[1].payload["eog_art"], "GRUNDVERSORGUNG");
        let state = apply_all(state, &out.events);
        assert!(
            matches!(state, EogState::AntwortGesendet { response_pid, accepted: true, .. }
                if response_pid.as_u32() == 55014)
        );
    }

    #[test]
    fn responder_bestaetigung_requires_versorgungsart() {
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(true)).unwrap();
        let state = apply_all(EogState::New, &out.events);
        let result = GpkeEogWorkflow::handle(
            &state,
            EogCommand::SendAntwort {
                accepted: true,
                versorgungsart: None,
                bilanzkreis: None,
                reason: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn responder_ablehnung_requires_reason() {
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(true)).unwrap();
        let state = apply_all(EogState::New, &out.events);
        let result = GpkeEogWorkflow::handle(
            &state,
            EogCommand::SendAntwort {
                accepted: false,
                versorgungsart: None,
                bilanzkreis: None,
                reason: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn responder_ablehnung() {
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(true)).unwrap();
        let state = apply_all(EogState::New, &out.events);
        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::SendAntwort {
                accepted: false,
                versorgungsart: None,
                bilanzkreis: None,
                reason: Some("A05".to_owned()),
            },
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1); // UTILMD 55015 only — no ProcessCompleted
        assert_eq!(out.outbox[0].payload["pid"], 55015);
        let state = apply_all(state, &out.events);
        assert!(
            matches!(state, EogState::AntwortGesendet { response_pid, accepted: false, .. }
                if response_pid.as_u32() == 55015)
        );
    }

    #[test]
    fn responder_validation_failure_rejects_with_aperak() {
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(false)).unwrap();
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].message_type.as_ref(), "APERAK");
        let state = apply_all(EogState::New, &out.events);
        assert!(matches!(state, EogState::Rejected { .. }));
    }

    #[test]
    fn responder_timeout_rejects() {
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(true)).unwrap();
        let state = apply_all(EogState::New, &out.events);
        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: EOG_RESPONSE_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, EogState::Rejected { .. }));
    }

    // ── Versorgungsart codes ──────────────────────────────────────────────────

    #[test]
    fn versorgungsart_code_roundtrip() {
        for art in [
            Versorgungsart::Ersatzversorgung,
            Versorgungsart::Grundversorgung,
            Versorgungsart::Ersatzbelieferung,
            Versorgungsart::Uebergangsversorgung,
        ] {
            assert_eq!(Versorgungsart::from_code(art.code()), Some(art));
        }
        assert_eq!(Versorgungsart::from_code("E06"), None);
    }

    #[test]
    fn timeout_in_terminal_state_is_noop() {
        let out = GpkeEogWorkflow::handle(&EogState::New, anmelden_cmd()).unwrap();
        let state = apply_all(EogState::New, &out.events);
        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::ReceiveAntwort {
                response_pid: pid(55014),
                accepted: true,
                versorgungsart: Some(Versorgungsart::Ersatzversorgung),
                bilanzkreis: None,
                reason: None,
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);

        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: EOG_RESPONSE_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty());
    }
}
