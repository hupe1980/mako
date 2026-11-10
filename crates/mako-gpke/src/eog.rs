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
//! `ZE3` Ersatzbelieferung; `ZZD` Übergangsversorgung is a Transaktionsgrund)
//! plus the Bilanzkreis in the Produktpaket `SG8 SEQ+Z79`, the
//! Gruppenzuordnung, the Grundlage nach EnFG and the Kunde des LF. Neither
//! answer names a Lokation: it is tied to its Anmeldung by `SG6 RFF+TN`
//! (UTILMD AHB Strom 2.2 Kap. 8.6, S. 127–140), and the NB finds its process
//! by [`nb_vorgang_key`]. The Anmeldung only states the cause and whether the
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
//!   the published window per PID ([`eog_antwort_due_at`]).
//! - No answer → NB assigns anyway (15:00–16:00 window, default BK).
//!
//! # Regulatory basis
//!
//! - **§36 / §38 / §38a EnWG**, **§§2–3 StromGVV**
//! - **GPKE Teil 2 Kap. 2.3 (BK6-24-174)** — Beginn der Ersatz-/Grundversorgung
//! - **UTILMD AHB Strom S2.1/S2.2 Kap. 8.6**, **EBD E_0615**
//! - **APERAK AHB 1.1 §2.4.1** — Strom UTILMD 45-min APERAK Frist

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::payload::{Stammdaten, UtilmdPayload};
use mako_engine::types::Pruefidentifikator;
use mako_pruefung::codes::EBD_ANMELDUNG_EOG;

/// `A09` „Zustimmung" — the one Zustimmungscode of `E_0615` „Anmeldung E/G
/// prüfen", the E/G's answer to a 55013 (`mako_pruefung::codes::E_0615_CODES`).
const EOG_ZUSTIMMUNG: &str = "A09";
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
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

/// Correlation key of the NB's own EoG process — the one `gpke.eog.anmelden`
/// opens with [`EogCommand::Anmelden`] — and under which each of
/// [`EOG_ANTWORT_PIDS`] resumes it.
///
/// The E/G's process answering an inbound 55013 stays keyed by the bare
/// Marktlokation. Both run the one workflow name on the one Marktlokation
/// wherever a deployment is NB and LF at once: the NB's 55013 reaches its own
/// LF side through the AS4 loopback. Keyed alike, the LF's copy of the 55013
/// finds the NB's process waiting for the answer and is refused as an
/// Anmeldung, and the LF's answer door finds two processes on its
/// Marktlokation. Keyed apart, each door reaches only its own side.
#[must_use]
pub fn nb_anfrage_key(malo_id: &str) -> String {
    format!("nb-anfrage:{malo_id}")
}

/// Correlation key under which the NB's EoG process is found by the answer's
/// `SG6 RFF+TN` — the `SG4 IDE+24` Vorgangsnummer its 55013 carried.
///
/// The 55014/55015 column names no `SG5` Lokation at all (UTILMD AHB Strom 2.2
/// Kap. 8.6, S. 127: `SG5` Marktlokation is listed on the 55013 alone), so the
/// Referenz Vorgangsnummer is the only thing an answer ties to its Anmeldung.
#[must_use]
pub fn nb_vorgang_key(vorgangsnummer: &str) -> String {
    format!("nb-eog-vorgang:{vorgangsnummer}")
}

/// Derive the outbound ANTWORT PID for the EoG Anmeldung.
#[must_use]
pub fn eog_response_pid(accepted: bool) -> u32 {
    if accepted { 55014 } else { 55015 }
}

/// The published answer window for an inbound Anmeldung E/G.
///
/// Strom **55013** states two windows selected by a date in the payload — 15:00
/// Uhr *am ÜT* when the Zuordnungsbeginn lies in the future, and 15:00 Uhr des
/// ersten Werktags when it does not (GPKE Teil 2 Kap. 2.3.2.2 Nr. 2). Gas **44013**
/// is a plain 2-Werktage window (BK7-24-01-009 Kap. 3.3.2).
///
/// Both come from [`mako_fristen::antwort`], which publishes the **tighter** of
/// the two Strom windows: a queue sized by the outer envelope reports a lapsed
/// Frist as still running, and that the NB eventually assigns the E/G anyway
/// (Nr. 3, „aufgrund fehlender Antwort") is a reason to answer in time, not a
/// reason to track a looser clock.
///
/// `None` for a PID that is not an Anmeldung E/G.
#[must_use]
pub fn eog_antwort_due_at(
    pid: u32,
    received_at: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    mako_fristen::antwort::antwort_deadline(pid, received_at)
}

// ── EoG classification ────────────────────────────────────────────────────────

/// Versorgungsart stated by the E/G in the Bestätigung 55014
/// (`SG10 CCI+Z36`, DE 7037).
///
/// **Three codes, not four.** UTILMD AHB Strom 2.2 („Versorgungsart der
/// Marktlokation", Muss on 55014) publishes exactly `ZC9`, `ZD0` and `ZE3` for
/// DE 7037. `ZZD` Übergangsversorgung is a **Transaktionsgrund** — `SG4 STS+7`
/// DE 9013 element 2, alongside `Z36`/`Z37`/`Z39`/`ZC6`/`ZC7`/`ZT6`/`ZT7`
/// (AHB Änd-ID 27001/27002, § 38a EnWG) — and appears in the same element on
/// the 55004 Abmeldung under Bedingung `[686]`. Carrying it here put a qualifier
/// into DE 7037 that the AHB does not define there, which the counterparty
/// rejects, and accepted one inbound that cannot legitimately arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Versorgungsart {
    /// `ZC9` — §38 EnWG Ersatzversorgung (ipso iure, max. 3 months, NSP).
    Ersatzversorgung,
    /// `ZD0` — §36 EnWG Grundversorgung (Haushaltskunden in NSP).
    Grundversorgung,
    /// `ZE3` — vertragliche Ersatzbelieferung (bilateral agreement).
    ///
    /// The § 38a Übergangsversorgung (MSP/HSP, from 01.04.2026) is one of
    /// these: its Grundlage is a bilaterale Vereinbarung. What marks the case
    /// as § 38a is the Transaktionsgrund [`UEBERGANGSVERSORGUNG`], not a
    /// separate Versorgungsart.
    ///
    /// [`UEBERGANGSVERSORGUNG`]: crate::eog::UEBERGANGSVERSORGUNG
    Ersatzbelieferung,
}

/// `SG4 STS+7` DE 9013 — Transaktionsgrund „Übergangsversorgung" (§ 38a EnWG).
///
/// Belongs to the Transaktionsgrund code space of the 55013/55014/55015 and the
/// 55004/55005/55006, **not** to the Versorgungsart in `SG10 CCI+Z36`.
///
/// Restated here rather than imported: `edi-energy` is a dev-dependency of this
/// crate on purpose, and one string is not worth a production dependency on the
/// whole wire library. `the_code_matches_the_wire_table` holds the two together.
pub const UEBERGANGSVERSORGUNG: &str = "ZZD";

impl Versorgungsart {
    /// AHB code (`SG10 CCI+Z36` DE 7037).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Ersatzversorgung => "ZC9",
            Self::Grundversorgung => "ZD0",
            Self::Ersatzbelieferung => "ZE3",
        }
    }

    /// Parse from the AHB code. `ZZD` is deliberately absent — see the type docs.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "ZC9" => Some(Self::Ersatzversorgung),
            "ZD0" => Some(Self::Grundversorgung),
            "ZE3" => Some(Self::Ersatzbelieferung),
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
        /// `SG4 IDE+24` Vorgangsnummer of the 55013 sent.
        vorgangsnummer: Option<String>,
    },
    /// NB (initiator) received the E/G's response (55014/55015).
    AntwortErhalten {
        /// Response PID: 55014 (Bestätigung) or 55015 (Ablehnung).
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung, `false` = Ablehnung.
        accepted: bool,
        /// Versorgungsart from the Bestätigung (`CCI+Z36`: ZC9/ZD0/ZE3).
        versorgungsart: Option<Versorgungsart>,
        /// Bilanzkreis (EIC) from the Bestätigung.
        bilanzkreis: Option<String>,
        /// Rejection reason / EBD code (A02/A04/A05) when rejected.
        reason: Option<String>,
        /// The answer's `SG4 STS+E01` code and Codeliste (`E_0615`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        antwort: Option<mako_engine::envelope::Antwortcode>,
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
        /// `SG4 IDE+24` Vorgangsnummer of the received 55013.
        vorgangsnummer: Option<String>,
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

    /// Both perspectives: the NB sends the 55013 (`Angemeldet`) and takes the
    /// answer back (`AntwortErhalten`); the E/G receives the 55013
    /// (`AnmeldungErhalten`) and answers it (`AntwortGesendet`).
    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::Angemeldet {
                pruefidentifikator, ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: None,
            }),
            Self::AntwortErhalten { response_pid, .. } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(response_pid.as_u32()),
                message_ref: None,
            }),
            Self::AnmeldungErhalten {
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
            Self::ZugeordnetOhneAntwort { .. } | Self::DeadlineExpired { .. } => None,
            // `ValidationPassed` is emitted only beside the APERAK 312
            // Anerkennungsmeldung `handle` queues for it, `Rejected` only beside
            // the APERAK 313 Verarbeitbarkeitsfehlermeldung; an APERAK carries
            // no Prüfidentifikator.
            Self::ValidationPassed { .. } | Self::Rejected { .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: None,
                message_ref: None,
            }),
        }
    }

    /// The E/G's answer to the NB's 55013, with the `E_0615` code it named.
    fn antwort(&self) -> Option<mako_engine::envelope::Antwortcode> {
        match self {
            Self::AntwortErhalten { antwort, .. } => antwort.clone(),
            _ => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured when the process starts (either role).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// `SG4 IDE+24` Vorgangsnummer of the 55013 — on the E/G's side the
    /// number its answer restates in `SG6 RFF+TN`, on the NB's side the number
    /// that answer is matched by ([`nb_vorgang_key`]).
    pub vorgangsnummer: Option<String>,
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
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// GPKE Teil 2 Kap. 2.3.2.2 gives the E/G-Lieferant its answer window and no consequence of the E/G missing it; the NB then assigns on its own (Schritt 3), which reaches this process as the NB's message. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<EogState>,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for EogState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // A lapsed window leaves the Vorgang open.
            Self::FristAbgelaufen { offen, .. } => offen.occupies_business_key(),
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

    /// Return `Some(&EogData)` if the process carries business data.
    #[must_use]
    pub fn data(&self) -> Option<&EogData> {
        match self {
            Self::FristAbgelaufen { offen, .. } => offen.data(),
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
        /// `SG4 STS+7` Transaktionsgrundergänzung — `ZW6` „Pauschale
        /// Marktlokation" or `ZW7` „Gemessene Marktlokation" (UTILMD AHB
        /// Strom 2.2, Anwendungsfall 55013).
        transaktionsgrund_ergaenzung: String,
        /// Haushaltskunde flag (CCI Z15/Z18), if known.
        haushaltskunde: Option<bool>,
        /// The Marktlokation's Stammdaten as the NB holds them — the `SG5`
        /// Messlokationen, the `SG8` groups and the `SG12` Anschriften the
        /// 55013 carries. Read at the transport boundary, never here.
        stammdaten: Stammdaten,
        /// `SG4 IDE+24` DE 7402 — unique per outbound Vorgang, and the number
        /// the E/G's answer names in `SG6 RFF+TN`. Drawn at the boundary.
        vorgangsnummer: String,
    },
    /// NB (initiator): the E/G's response (55014/55015) arrived.
    ReceiveAntwort {
        /// Response PID (55014 or 55015).
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung (55014).
        accepted: bool,
        /// Versorgungsart from the Bestätigung (`CCI+Z36`: ZC9/ZD0/ZE3).
        versorgungsart: Option<Versorgungsart>,
        /// Bilanzkreis (EIC) from the Bestätigung.
        bilanzkreis: Option<String>,
        /// Rejection reason / EBD code (when rejected).
        reason: Option<String>,
        /// The answer's `SG4 STS+E01` code and Codeliste, when it carries one.
        antwort: Option<mako_engine::envelope::Antwortcode>,
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
        /// `SG4 IDE+24` Vorgangsnummer of the 55013.
        vorgangsnummer: Option<String>,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
        /// Receipt instant — the ÜT the answer window counts from.
        received_at: time::OffsetDateTime,
    },
    /// LF/E-G (responder): send the outbound response (55014/55015).
    SendAntwort {
        /// `true` = Bestätigung (55014), `false` = Ablehnung (55015).
        accepted: bool,
        /// Versorgungsart (required for a Bestätigung: ZC9/ZD0/ZE3).
        versorgungsart: Option<Versorgungsart>,
        /// Bilanzkreis (EIC) the MaLo is assigned to — `SG8 SEQ+Z79`
        /// Produktpaket, Muss on the Bestätigung.
        bilanzkreis: Option<String>,
        /// Rejection reason / EBD code (required for an Ablehnung).
        reason: Option<String>,
        /// `SG8 SEQ+Z01 CCI+++Z15`/`Z18` Gruppenzuordnung — Muss on the
        /// Bestätigung; the NB's record of the Marktlokation, read at the
        /// boundary.
        haushaltskunde: Option<bool>,
        /// `SG8 SEQ+Z75 CCI+Z61` Grundlage zur Verringerung der Umlagen nach
        /// EnFG (`ZF9`/`ZG0`/`ZG1`) — Muss on the Bestätigung, the E/G's own
        /// fact about its Kunde.
        enfg_grundlage: Option<String>,
        /// `SG10 CAV` Grund der Privilegierung nach EnFG (`ZU5`…`ZV3`) — Muss
        /// beside `ZF9` (Bedingung `[463]`).
        enfg_grund: Option<String>,
        /// The `SG12` Kunde des LF and his Korrespondenzanschrift, read at the
        /// boundary.
        stammdaten: Stammdaten,
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

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// [`EOG_RESPONSE_WINDOW_LABEL`] is open while an answer is owed — by the E/G
/// (`Eingegangen`, `ValidationPassed`) or to the NB that sent the Anmeldung
/// (`Angemeldet`). The APERAK delivery window riding beside it is not this
/// workflow's to decide.
fn window_open(label: &str, state: &EogState) -> bool {
    label == EOG_RESPONSE_WINDOW_LABEL
        && matches!(
            state,
            EogState::Angemeldet(_) | EogState::Eingegangen(_) | EogState::ValidationPassed(_)
        )
}

impl Workflow for GpkeEogWorkflow {
    type State = EogState;
    type Event = EogEvent;
    type Command = EogCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[EOG_RESPONSE_WINDOW_LABEL];

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| EogCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            EogState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            EogEvent::Angemeldet {
                location_id,
                sender,
                receiver,
                process_date,
                pruefidentifikator,
                transaktionsgrund,
                haushaltskunde,
                vorgangsnummer,
            } => EogState::Angemeldet(EogData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                transaktionsgrund: transaktionsgrund.clone(),
                haushaltskunde: *haushaltskunde,
                vorgangsnummer: vorgangsnummer.clone(),
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
                vorgangsnummer,
                ..
            } => EogState::Eingegangen(EogData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                transaktionsgrund: transaktionsgrund.clone(),
                haushaltskunde: *haushaltskunde,
                vorgangsnummer: vorgangsnummer.clone(),
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
            EogEvent::DeadlineExpired { label, .. } => match state {
                EogState::Eingegangen(_) | EogState::ValidationPassed(_) => {
                    EogState::FristAbgelaufen {
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
            // ── NB initiator ─────────────────────────────────────────────────
            EogCommand::Anmelden {
                pid,
                sender,
                receiver,
                location_id,
                process_date,
                transaktionsgrund,
                transaktionsgrund_ergaenzung,
                haushaltskunde,
                stammdaten,
                vorgangsnummer,
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

                if transaktionsgrund_ergaenzung.trim().is_empty() {
                    return Err(WorkflowError::rejected(
                        "EoG Anmeldung requires a Transaktionsgrundergänzung (SG4 STS+7 \
                         DE 9013, ZW6 Pauschale / ZW7 Gemessene Marktlokation)"
                            .to_owned(),
                    ));
                }

                // Outbound UTILMD 55013 — rendered by the makod EDIFACT
                // renderer from its typed payload.
                let utilmd = PendingOutbox::typed(
                    receiver.as_str(),
                    &UtilmdPayload {
                        pid: pid.as_u32(),
                        sender: sender.as_str().to_owned(),
                        receiver: Some(receiver.as_str().to_owned()),
                        malo: Some(location_id.as_str().to_owned()),
                        process_date: Some(process_date.clone()),
                        transaktionsgrund: Some(transaktionsgrund.clone()),
                        transaktionsgrund_ergaenzung: Some(transaktionsgrund_ergaenzung),
                        vorgangsnummer: Some(vorgangsnummer.clone()),
                        stammdaten,
                        ..UtilmdPayload::default()
                    },
                );
                // Notify observers (processd's read model, ERP) that the statutory fallback
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
                    vorgangsnummer: Some(vorgangsnummer),
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
                antwort,
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
                    // The E/G's code rides beside the verdict.
                    let mut completed = process_completed_outbox(
                        data,
                        versorgungsart,
                        bilanzkreis.as_deref(),
                        false,
                    );
                    if let Some(a) = &antwort {
                        completed.payload["antwortcode"] = serde_json::json!(a.code);
                        completed.payload["codeliste"] = serde_json::json!(a.codeliste);
                    }
                    outbox.push(completed);
                }
                Ok(WorkflowOutput::with_outbox(
                    vec![EogEvent::AntwortErhalten {
                        response_pid,
                        accepted,
                        versorgungsart,
                        bilanzkreis,
                        reason,
                        antwort,
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
                vorgangsnummer,
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
                // Clone before move for the notification and APERAK emission.
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();
                let notify_malo = location_id.clone();
                let notify_termin = process_date.clone();
                let grund = transaktionsgrund.clone();

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
                    vorgangsnummer,
                }];
                if validation_passed {
                    events.push(EogEvent::ValidationPassed {
                        message_ref: message_ref.clone(),
                    });
                    // APERAK BGM+312 (Anerkennung) — Strom UTILMD 45-min Frist
                    // (APERAK AHB 1.1 §2.4.1).
                    let outbox = vec![
                        // The business notification. Without it `processd`'s LF
                        // module never sees the Zuordnung: `makod` delivers a
                        // CloudEvent only for an outbox entry, and an APERAK is
                        // a technical acknowledgement. The E/G's 15:00-Uhr-am-ÜT
                        // Frist would then lapse unanswered and unseen.
                        crate::LfVorgangsdaten {
                            transaktionsgrund: Some(grund.clone()),
                            ..crate::LfVorgangsdaten::default()
                        }
                        .process_initiated(
                            pid,
                            &notify_malo,
                            &sender_mp_id,
                            &receiver_gln,
                            &notify_termin,
                            &serde_json::json!({ "haushaltskunde": haushaltskunde }),
                        )
                        .caused_by(1),
                        PendingOutbox::aperak_anerkennung(
                            receiver_gln.as_str(),
                            sender_mp_id.as_str(),
                            message_ref.as_str(),
                        )
                        .caused_by(1),
                    ];
                    let deadlines: Vec<PendingDeadline> =
                        eog_antwort_due_at(pid.as_u32(), received_at)
                            .map(|due| PendingDeadline::new(EOG_RESPONSE_WINDOW_LABEL, due))
                            .into_iter()
                            .collect();
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

            EogCommand::SendAntwort {
                accepted,
                versorgungsart,
                bilanzkreis,
                reason,
                haushaltskunde,
                enfg_grundlage,
                enfg_grund,
                stammdaten,
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
                        "Bestätigung EOG requires the Versorgungsart (CCI+Z36: ZC9/ZD0/ZE3)"
                            .to_owned(),
                    ));
                }
                // The Bestätigung's column makes `SG8 SEQ+Z79` Produktpaket
                // (the Bilanzkreis), `SEQ+Z01` with the Gruppenzuordnung and
                // `SEQ+Z75` with the EnFG-Grundlage Muss (UTILMD AHB Strom 2.2
                // Kap. 8.6, S. 129, 136, 138).
                if accepted {
                    let fehlend: Vec<&str> = [
                        (
                            bilanzkreis.is_none(),
                            "Bilanzkreis (SG8 SEQ+Z79 Produktpaket)",
                        ),
                        (
                            haushaltskunde.is_none(),
                            "Gruppenzuordnung (SG10 CCI+++Z15/Z18)",
                        ),
                        (
                            enfg_grundlage.is_none(),
                            "Grundlage nach EnFG (SG8 SEQ+Z75 CCI+Z61)",
                        ),
                    ]
                    .into_iter()
                    .filter_map(|(fehlt, was)| fehlt.then_some(was))
                    .collect();
                    if !fehlend.is_empty() {
                        return Err(WorkflowError::rejected(format!(
                            "Bestätigung EOG 55014 requires {}",
                            fehlend.join(", ")
                        )));
                    }
                }
                if !accepted && reason.is_none() {
                    return Err(WorkflowError::rejected(
                        "Ablehnung EOG requires a reason (EBD E_0615: A02/A04/A05)".to_owned(),
                    ));
                }
                let response_pid = Pruefidentifikator::new(eog_response_pid(accepted))
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;

                // `SG4 STS+E01` — Muss on both answers, DE 1131 `E_0615` (UTILMD
                // AHB Strom 2.2, 55014/55015): `A09` Zustimmung on the
                // Bestätigung, the Ablehnungscode the E/G states on the
                // Ablehnung, which must be one `E_0615` publishes.
                let antwort_code = if accepted {
                    EOG_ZUSTIMMUNG.to_owned()
                } else {
                    let code = reason.clone().unwrap_or_default();
                    let ablehnung = mako_pruefung::codes::lookup(EBD_ANMELDUNG_EOG, &code)
                        .is_some_and(|c| c.ist_zustimmung() == Some(false));
                    if !ablehnung {
                        return Err(WorkflowError::validation(format!(
                            "Ablehnung EOG carries an Ablehnungscode of {EBD_ANMELDUNG_EOG} \
                             (A02/A03/A04/A05/A99), got {code:?}"
                        )));
                    }
                    code
                };
                // Outbound UTILMD 55014/55015 to the NB. Neither column names
                // an `SG5` Lokation — the Marktlokation is the 55013's alone
                // (UTILMD AHB Strom 2.2 Kap. 8.6, S. 127) — so the answer is
                // tied to its Anmeldung by `SG6 RFF+TN` (Muss, S. 129) and
                // restates the Anmeldung's `STS+7` Transaktionsgrund (S. 125–126).
                let mut outbox = vec![PendingOutbox::typed(
                    data.sender.as_str(),
                    &UtilmdPayload {
                        pid: response_pid.as_u32(),
                        sender: data.receiver.as_str().to_owned(),
                        receiver: Some(data.sender.as_str().to_owned()),
                        process_date: Some(data.process_date.clone()),
                        referenz_vorgangsnummer: data.vorgangsnummer.clone(),
                        transaktionsgrund: Some(data.transaktionsgrund.clone()),
                        antwort_code: Some(antwort_code),
                        antwort_codeliste: Some(EBD_ANMELDUNG_EOG.to_owned()),
                        bilanzkreis: bilanzkreis.clone().filter(|_| accepted),
                        kundengruppe: haushaltskunde
                            .filter(|_| accepted)
                            .map(|h| if h { "Z15" } else { "Z18" }.to_owned()),
                        versorgungsart: versorgungsart
                            .filter(|_| accepted)
                            .map(|v| v.code().to_owned()),
                        enfg_grundlage: enfg_grundlage.filter(|_| accepted),
                        enfg_grund: enfg_grund.filter(|_| accepted),
                        stammdaten: if accepted {
                            stammdaten
                        } else {
                            Stammdaten::default()
                        },
                        ..UtilmdPayload::default()
                    },
                )];
                if accepted {
                    // The E/G's own marktd records the fallback supply from
                    // this event (it is now the supplier of record), and the
                    // code it answered with rides beside the verdict.
                    let mut completed = process_completed_outbox(
                        data,
                        versorgungsart,
                        bilanzkreis.as_deref(),
                        false,
                    );
                    completed.payload["antwortcode"] = serde_json::json!(EOG_ZUSTIMMUNG);
                    completed.payload["codeliste"] = serde_json::json!(EBD_ANMELDUNG_EOG);
                    outbox.push(completed);
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

            EogCommand::TimeoutExpired { deadline_id, label } => match lapsed {
                _ if !window_open(&label, lapsed) => Ok(vec![].into()),
                // Initiator: silence never blocks the statutory fallback —
                // assign with the pre-deposited default Bilanzkreis
                // (GPKE Teil 2 Kap. 2.3 SD Schritt 3).
                EogState::Angemeldet(data) => Ok(WorkflowOutput::with_outbox(
                    vec![EogEvent::ZugeordnetOhneAntwort { deadline_id }],
                    vec![process_completed_outbox(data, None, None, true)],
                )),
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
            transaktionsgrund_ergaenzung: "ZW6".to_owned(),
            haushaltskunde: Some(true),
            stammdaten: Stammdaten::default(),
            vorgangsnummer: "EOGV0001".to_owned(),
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
            vorgangsnummer: Some("EOGV0001".to_owned()),
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
    fn verlauf_states_both_perspectives() {
        assert_eq!(
            EogEvent::Rejected {
                reason: "x".to_owned(),
            }
            .verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: None,
                message_ref: None,
            }),
            "the APERAK 313 queued beside the rejection"
        );
        let out = GpkeEogWorkflow::handle(&EogState::New, anmelden_cmd()).unwrap();
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(55_013),
                message_ref: None,
            })
        );
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(true)).unwrap();
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(55_013),
                message_ref: Some("EOG-001".to_owned()),
            })
        );
        assert_eq!(
            out.events[1].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: None,
                message_ref: None,
            }),
            "the APERAK 312 queued beside ValidationPassed"
        );
        let antwort = EogEvent::AntwortErhalten {
            response_pid: pid(55_015),
            accepted: false,
            versorgungsart: None,
            bilanzkreis: None,
            reason: Some("A02".to_owned()),
            antwort: None,
        };
        assert_eq!(
            antwort.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(55_015),
                message_ref: None,
            })
        );
        let ohne = EogEvent::ZugeordnetOhneAntwort {
            deadline_id: DeadlineId::new(),
        };
        assert_eq!(ohne.verlauf(), None, "silence is no message");
    }

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
                bilanzkreis: Some("11XGRUNDV-BK---9".to_owned()),
                reason: None,
                antwort: Some(mako_engine::envelope::Antwortcode::new(
                    "A09",
                    Some("E_0615"),
                )),
            },
        )
        .unwrap();
        // ProcessCompleted drives marktd's Ersatzversorgung transition, and
        // carries the E/G's code beside the verdict.
        assert_eq!(out.outbox[0].payload["antwortcode"], "A09");
        assert_eq!(out.outbox[0].payload["codeliste"], "E_0615");
        assert_eq!(
            out.events[0].antwort().map(|a| a.code),
            Some("A09".to_owned())
        );
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
                bilanzkreis: Some("11XGRUNDV-BK---9".to_owned()),
                reason: None,
                antwort: None,
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
                antwort: None,
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

    /// The 55013 carries the Transaktionsgrundergänzung and the
    /// Marktlokation's Stammdaten the command was given, in the typed UTILMD
    /// payload the renderer reads — a key renamed on either side is a compile
    /// error, not a 55013 without its SG8 groups.
    #[test]
    fn initiator_carries_ergaenzung_and_stammdaten_in_the_typed_payload() {
        let stammdaten = Stammdaten {
            lokationen: vec![mako_engine::payload::StammdatenLokation {
                qualifier: "Z17".to_owned(),
                id: "DE0000000000000000000000000000042".to_owned(),
            }],
            ..Stammdaten::default()
        };
        let cmd = match anmelden_cmd() {
            EogCommand::Anmelden {
                pid,
                sender,
                receiver,
                location_id,
                process_date,
                transaktionsgrund,
                haushaltskunde,
                ..
            } => EogCommand::Anmelden {
                pid,
                sender,
                receiver,
                location_id,
                process_date,
                transaktionsgrund,
                transaktionsgrund_ergaenzung: "ZW7".to_owned(),
                haushaltskunde,
                stammdaten: stammdaten.clone(),
                vorgangsnummer: "EOGV0001".to_owned(),
            },
            _ => unreachable!(),
        };
        let out = GpkeEogWorkflow::handle(&EogState::New, cmd).unwrap();
        let payload: UtilmdPayload =
            serde_json::from_value(out.outbox[0].payload.clone()).expect("a typed UTILMD payload");
        assert_eq!(payload.transaktionsgrund_ergaenzung.as_deref(), Some("ZW7"));
        assert_eq!(payload.stammdaten, stammdaten);
    }

    #[test]
    fn initiator_requires_transaktionsgrund_ergaenzung() {
        let cmd = match anmelden_cmd() {
            EogCommand::Anmelden {
                pid,
                sender,
                receiver,
                location_id,
                process_date,
                transaktionsgrund,
                haushaltskunde,
                stammdaten,
                ..
            } => EogCommand::Anmelden {
                pid,
                sender,
                receiver,
                location_id,
                process_date,
                transaktionsgrund,
                transaktionsgrund_ergaenzung: " ".to_owned(),
                haushaltskunde,
                stammdaten,
                vorgangsnummer: "EOGV0001".to_owned(),
            },
            _ => unreachable!(),
        };
        assert!(GpkeEogWorkflow::handle(&EogState::New, cmd).is_err());
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
                transaktionsgrund_ergaenzung: "ZW6".to_owned(),
                haushaltskunde: None,
                stammdaten: Stammdaten::default(),
                vorgangsnummer: "EOGV0001".to_owned(),
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
                transaktionsgrund_ergaenzung: "ZW6".to_owned(),
                haushaltskunde: None,
                stammdaten: Stammdaten::default(),
                vorgangsnummer: "EOGV0001".to_owned(),
            },
        );
        assert!(result.is_err());
    }

    // ── LF/E-G responder ──────────────────────────────────────────────────────

    #[test]
    fn responder_happy_path_bestaetigung() {
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(true)).unwrap();
        assert_eq!(out.events.len(), 2); // AnmeldungErhalten + ValidationPassed
        // The answer window alone: the APERAK window is the ingest door's.
        assert_eq!(out.deadlines.len(), 1);
        let state = apply_all(EogState::New, &out.events);
        assert!(matches!(state, EogState::ValidationPassed(_)));

        let out = GpkeEogWorkflow::handle(
            &state,
            EogCommand::SendAntwort {
                accepted: true,
                versorgungsart: Some(Versorgungsart::Grundversorgung),
                bilanzkreis: Some("11XGRUNDV-BK---9".to_owned()),
                reason: None,
                haushaltskunde: Some(true),
                enfg_grundlage: Some("ZG1".to_owned()),
                enfg_grund: None,
                stammdaten: Stammdaten::default(),
            },
        )
        .unwrap();
        // UTILMD 55014 wire message + ProcessCompleted for the E/G's marktd.
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[0].message_type.as_ref(), "UTILMD");
        assert_eq!(out.outbox[0].payload["pid"], 55014);
        assert_eq!(out.outbox[0].payload["antwort_code"], "A09");
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "E_0615");
        // The column names no Lokation; the answer is tied to its Anmeldung
        // by `RFF+TN` and restates its Transaktionsgrund.
        assert!(out.outbox[0].payload.get("malo").is_none());
        assert_eq!(out.outbox[0].payload["referenz_vorgangsnummer"], "EOGV0001");
        assert_eq!(out.outbox[0].payload["transaktionsgrund"], "ZT7");
        assert_eq!(out.outbox[0].payload["versorgungsart"], "ZD0");
        assert_eq!(out.outbox[0].payload["kundengruppe"], "Z15");
        assert_eq!(out.outbox[0].payload["enfg_grundlage"], "ZG1");
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
                haushaltskunde: None,
                enfg_grundlage: None,
                enfg_grund: None,
                stammdaten: Stammdaten::default(),
            },
        );
        assert!(result.is_err());
    }

    /// The Bestätigung's column makes the Produktpaket, the Gruppenzuordnung
    /// and the Grundlage nach EnFG Muss, so a 55014 without one is not written.
    #[test]
    fn responder_bestaetigung_requires_its_muss_facts() {
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(true)).unwrap();
        let state = apply_all(EogState::New, &out.events);
        let err = GpkeEogWorkflow::handle(
            &state,
            EogCommand::SendAntwort {
                accepted: true,
                versorgungsart: Some(Versorgungsart::Ersatzversorgung),
                bilanzkreis: None,
                reason: None,
                haushaltskunde: None,
                enfg_grundlage: None,
                enfg_grund: None,
                stammdaten: Stammdaten::default(),
            },
        )
        .expect_err("a 55014 without its Muss facts is refused")
        .to_string();
        for named in ["Bilanzkreis", "Gruppenzuordnung", "EnFG"] {
            assert!(err.contains(named), "{named} in {err}");
        }
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
                haushaltskunde: None,
                enfg_grundlage: None,
                enfg_grund: None,
                stammdaten: Stammdaten::default(),
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
                haushaltskunde: None,
                enfg_grundlage: None,
                enfg_grund: None,
                stammdaten: Stammdaten::default(),
            },
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1); // UTILMD 55015 only — no ProcessCompleted
        assert_eq!(out.outbox[0].payload["pid"], 55015);
        // The Ablehnungscode rides `SG4 STS+E01`, not a key no renderer reads.
        assert_eq!(out.outbox[0].payload["antwort_code"], "A05");
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "E_0615");
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

    /// The E/G's own answer window lapsing is a Frist breach, not a
    /// rejection — and the APERAK delivery window beside it decides nothing.
    #[test]
    fn responder_timeout_is_a_frist_breach() {
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
        let lapsed = apply_all(state.clone(), &out.events);
        assert!(matches!(lapsed, EogState::FristAbgelaufen { .. }));

        let aperak = GpkeEogWorkflow::handle(
            &state,
            EogCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: mako_fristen::APERAK_STROM_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(aperak.events.is_empty());
    }

    // ── Versorgungsart codes ──────────────────────────────────────────────────

    /// **The E/G must be told.** `makod` delivers a CloudEvent only for an
    /// outbox entry, and an APERAK is a technical acknowledgement — so without
    /// its own `ProcessInitiated` the inbound 55013 never reaches `processd`'s
    /// LF module and the 15:00-Uhr-am-ÜT Frist lapses unanswered and unseen.
    #[test]
    fn an_inbound_anmeldung_notifies_the_eog_supplier() {
        let out = GpkeEogWorkflow::handle(&EogState::New, receive_anmeldung_cmd(true)).unwrap();
        let notification = out
            .outbox
            .iter()
            .find(|o| &*o.message_type == "ProcessInitiated")
            .expect("the E/G is notified of the Zuordnung");
        assert_eq!(notification.payload["pid"], EOG_ANMELDUNG_PID);
        assert_eq!(notification.payload["malo_id"], "51238696781");
        // The Transaktionsgrund reaches the walk; without it `E_0615` cannot
        // tell a Grundversorgungs- from an Ersatzversorgungsfall.
        assert!(notification.payload.get("transaktionsgrund").is_some());
        assert_eq!(&*notification.recipient, "9900357000011");
    }

    #[test]
    fn versorgungsart_code_roundtrip() {
        for art in [
            Versorgungsart::Ersatzversorgung,
            Versorgungsart::Grundversorgung,
            Versorgungsart::Ersatzbelieferung,
        ] {
            assert_eq!(Versorgungsart::from_code(art.code()), Some(art));
        }
        assert_eq!(Versorgungsart::from_code("E06"), None);
    }

    /// **`ZZD` is a Transaktionsgrund, not a Versorgungsart.** UTILMD AHB Strom
    /// 2.2 publishes exactly `ZC9`, `ZD0` and `ZE3` for `SG10 CCI+Z36` DE 7037;
    /// `ZZD` Übergangsversorgung lives in `SG4 STS+7` DE 9013 element 2, on the
    /// 55013/55014/55015 and on the 55004/55005/55006 under Bedingung `[686]`.
    /// Accepting it here put a qualifier into DE 7037 the AHB does not define,
    /// and the counterparty rejects the message.
    #[test]
    fn zzd_is_not_a_versorgungsart() {
        assert_eq!(Versorgungsart::from_code(UEBERGANGSVERSORGUNG), None);
        assert!(
            [
                Versorgungsart::Ersatzversorgung,
                Versorgungsart::Grundversorgung,
                Versorgungsart::Ersatzbelieferung,
            ]
            .iter()
            .all(|a| a.code() != UEBERGANGSVERSORGUNG)
        );
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
                antwort: None,
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

    /// One wire code, one spelling. `edi-energy` carries the DE 9013 table this
    /// crate restates a single value from, and the two must not drift.
    #[test]
    fn the_code_matches_the_wire_table() {
        assert_eq!(
            super::UEBERGANGSVERSORGUNG,
            edi_energy::utilmd_codes::transaktionsgrund::UEBERGANGSVERSORGUNG
        );
    }
}
