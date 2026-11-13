//! WiM Strom/Gas Technikänderung — device/measurement-point change requests.
//!
//! This module handles ORDERS-based requests for technical changes at a
//! Messlokation:
//!
//! | PID   | Direction  | Description |
//! |-------|------------|-------------|
//! | 17011 | NB / LF → MSB | Beauftragung resp. Bestellung zur Änderung der Technik |
//! | 19005 | MSB → NB/LF | Bestätigung Auftrag Änderung Technik |
//! | 19006 | MSB → NB/LF | Ablehnung Auftrag Änderung Technik |
//!
//! # Both sides
//!
//! mako sends the Auftrag as NB or LF ([`TechnikAenderungCommand::SendAuftrag`])
//! and receives it as the MSB ([`TechnikAenderungCommand::ReceiveAuftrag`]).
//! The received Auftrag runs under [`auftrag_key`] — the sender and the
//! ORDERS' Dokumentennummer — so a deployment that is NB and MSB at once holds
//! its own sent Auftrag and the received copy as two processes. The MSB
//! answers with [`TechnikAenderungCommand::SendAntwort`]. 17118 is not part of
//! this workflow: the Anwendungsübersicht 4.0 (lfd. Nr. 22600–23180) files it
//! under GPKE Teil 3 „Bestellung einer Konfiguration" / „Bestellung Beendigung
//! einer Konfiguration", answered by ORDRSP 19127, and `mako-gpke` owns it.
//!
//! **17121 is not here.** „Bestellung Änderung (NB an MSB)" belongs to GPKE
//! Teil 3 and is answered with ORDRSP 19120 out of `E_0526`; `mako-gpke` owns
//! it.
//!
//! The ESA Ab-/Bestellung PIDs (ORDERS 17007, ORDRSP 19011/19012/19013/19014)
//! belong to [`crate::wertebestellung`], which models their own lifecycle.
//!
//! # Two documents, one message pair
//!
//! WiM Strom Teil 1 Kap. 3.3 has the NB or the LF order the change outright;
//! the BDEW *AWH Prozesse zur Änderung der Technik an Lokationen* V1.1 puts a
//! REQOTE 35005 / QUOTES 15005 round in front of the same order. Both end in
//! ORDERS 17011 → ORDRSP 19005/19006, so **four** Entscheidungsbäume share one
//! answer PID pair and the sender's Marktrolle resolves only half of it — see
//! [`mako_pruefung::msb::technik`], which is where that resolution lives.
//!
//! The leg after the answer is the **Durchführung**: the MSB goes to the
//! Lokation and reports back only if the visit failed, on IFTSTA 21027 (to the
//! NB) resp. 21025 (to the LF) out of `E_0286`.
//!
//! # Regulatory basis
//!
//! - **BK6-24-174** — WiM Strom Teil 1 Kap. 3.3 (Messlokationsänderung)
//! - **BDEW AWH Prozesse zur Änderung der Technik an Lokationen** V1.1 (31.03.2025)
//! - **Entscheidungsbaum-Diagramme und Codelisten 4.3** Kap. 8.6, 8.7, 9.1, 9.2
//! - Antwortfrist: **10 Werktage** (WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2
//!   Nr. 2), against a Mindestvorlauffrist of **20 Werktagen** on the direct
//!   Beauftragung (Nr. 1). Both are anchored differently: the answer window runs
//!   forward from the ÜT, the Vorlauffrist backward from the gewünschter
//!   Änderungstermin — see [`mako_fristen::vorlauf`]. The AWH Bestellung has no
//!   Vorlauffrist Prüfschritt at all; its Umsetzungszeitraum was agreed in the
//!   Angebot.

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name for the WiM Technikänderung process.
pub const WORKFLOW_NAME: &str = "wim-technik-aenderung";

/// ORDERS PIDs for technical change requests.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17011 | Beauftragung zur Änderung der Technik (Messlokationsänderung Strom) |
pub const ORDERS_PIDS: &[u32] = &[17011];

/// ORDRSP PIDs received in response to technical change requests.
///
/// | PID   | Description | EBD |
/// |-------|-------------|-----|
/// | 19005 | Bestätigung Auftrag Änderung Technik | `E_0249` / `E_0250` |
/// | 19006 | Ablehnung Auftrag Änderung Technik | `E_0249` / `E_0250` |
///
/// Three neighbouring ORDRSP PIDs answer other processes (ORDRSP AHB 1.1b
/// Kap. 4.11–4.13) and are not part of this set:
///
/// - **19003 / 19004** „Fortführungsbestätigung / Ablehnung Fortführung"
///   answer the Weiterverpflichtung ORDERS 17002 out of `E_0203` —
///   [`crate::weiterverpflichtung`].
/// - **19007** „Ablehnung Anforderung von Werten" answers a Werteanforderung.
///
/// # Four trees, one PID pair
///
/// 19005/19006 are shared by `E_0249`/`E_0250` (WiM Teil 1, direct
/// Beauftragung) and `E_0279`/`E_0283` (AWH, Bestellung nach Angebot). The
/// sender's Marktrolle separates the columns; the ORDERS' Zuordnung zu einem
/// Objekt (`ZO-T15` against `ZG-T24`) separates the rows. Resolve with
/// [`mako_pruefung::codes::aenderung_der_technik_baum`] — never against the
/// answer PID, and never on the Marktrolle alone: `A02` is the Zustimmung of
/// `E_0249` and an Ablehnung of `E_0279`.
pub const ORDRSP_PIDS: &[u32] = &[19005, 19006];

/// Positive ORDRSP PIDs (confirmation).
const ORDRSP_BESTAETIGUNG_PIDS: &[u32] = &[19005];

/// The ORDERS the MSB receives here: the Beauftragung Änderung der Technik
/// 17011, „NB, LF an MSB" (ORDERS AHB 1.1b Kap. 4.12, S. 83).
pub const RECEIVE_ORDERS_PIDS: &[u32] = &[17_011];

/// Deadline label for the response window (10 Werktage, WiM Strom).
pub const ANTWORT_WINDOW_LABEL: &str = "wim-technik-aenderung-antwort";

/// Deadline label for the MSB's own answer window on a received Auftrag —
/// „Unverzüglich, jedoch spätester ÜT ist der 10. WT nach dem ÜT von Nr. 1"
/// (BK6-24-174 WiM Teil 1 Kap. 3.3.1.2 / 3.3.2.2 Nr. 2, S. 53), the 17011 row
/// of [`mako_fristen::antwort`].
pub const EINGANG_WINDOW_LABEL: &str = "wim-technik-aenderung-eingang";

/// ORDRSP 19005 „Bestätigung Auftrag Änderung Technik", MSB → NB, LF.
pub const BESTAETIGUNG_PID: u32 = 19_005;

/// ORDRSP 19006 „Ablehnung Auftrag Änderung Technik", MSB → NB, LF.
pub const ABLEHNUNG_PID: u32 = 19_006;

/// `BGM` DE 1001 `Z12` „Änderung der Technik der Lokation" — the direct
/// Beauftragung of WiM Teil 1 (ORDERS AHB 1.1b Kap. 4.12, Hinweis `[592]`).
pub const BGM_DIREKTE_BEAUFTRAGUNG: &str = "Z12";

/// `BGM` DE 1001 `Z93` „Bestellung eines Angebots Änderung der Technik der
/// Lokation" — the AWH Bestellung after a QUOTES (Hinweis `[591]`).
pub const BGM_BESTELLUNG_NACH_ANGEBOT: &str = "Z93";

/// A Produkt a 17011 `Z12` orders for a Messlokation — Codeliste der
/// Konfigurationen 1.4 Kap. 7 (S. 58), Ebene „Messlokation": the code, and
/// whether a Lieferant may order it (the NB may order every one).
pub const MESSLOKATION_PRODUKTE: &[(&str, &str, bool)] = &[
    ("9991000002305", "Einbau iMS", true),
    ("9991000002313", "Einbau kME / rLM", true),
    ("9991000002321", "Einbau Wandler", false),
    (
        PRODUKT_WEITERE_ENERGIEFLUSSRICHTUNG,
        "weitere Energieflussrichtung",
        false,
    ),
    ("9991000002347", "Vergleichsmessung", false),
    ("9991000002355", "Änderung der Messebene", false),
];

/// „weitere Energieflussrichtung" — the Produkt whose 17011 names the
/// Marktlokation (`SG3 RFF+Z18`) and its MSB (`SG2 NAD+DEB`), ORDERS AHB 1.1b
/// Kap. 4.12, Bedingungen `[152]`/`[571]`.
pub const PRODUKT_WEITERE_ENERGIEFLUSSRICHTUNG: &str = "9991000002339";

/// The Beauftragungsart the received ORDERS' `BGM` DE 1001 states, which with
/// the Besteller's Marktrolle selects the answer's tree.
#[must_use]
pub fn beauftragungsart(bgm_1001: &str) -> Option<mako_pruefung::codes::TechnikBeauftragung> {
    use mako_pruefung::codes::TechnikBeauftragung;
    match bgm_1001 {
        BGM_DIREKTE_BEAUFTRAGUNG => Some(TechnikBeauftragung::DirekteBeauftragung),
        BGM_BESTELLUNG_NACH_ANGEBOT => Some(TechnikBeauftragung::BestellungNachAngebot),
        _ => None,
    }
}

/// The key a received Auftrag runs under: the sender's MP-ID and the ORDERS'
/// Dokumentennummer (`BGM` DE 1004).
///
/// Not the Messlokation: the answer leg is not built, so a process keyed on
/// the Lokation would stay open and refuse the next Auftrag for it. The number
/// is unique only per sender, hence the MP-ID.
#[must_use]
pub fn auftrag_key(sender: &str, dokumentennummer: &str) -> String {
    format!("technik-auftrag:{sender}:{dokumentennummer}")
}

/// When the MSB's answer to the received ORDERS `pid` is due, from its row in
/// [`mako_fristen::antwort`].
#[must_use]
pub fn eingang_antwortfrist(
    pid: u32,
    eingang: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    RECEIVE_ORDERS_PIDS
        .contains(&pid)
        .then(|| mako_fristen::antwort::antwort_deadline(pid, eingang))
        .flatten()
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Technikänderung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TechnikAenderungEvent {
    /// ORDERS technical change request sent.
    AuftragGesendet {
        /// ORDERS Prüfidentifikator.
        orders_pid: Pruefidentifikator,
        /// Recipient GLN (MSB or NB).
        recipient: MarktpartnerCode,
        /// Affected MeLo/MaLo.
        location_id: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// MSB side: an ORDERS Auftrag arrived.
    AuftragEingegangen {
        /// ORDERS Prüfidentifikator.
        orders_pid: Pruefidentifikator,
        /// The NB or LF that sent it — `NAD+MS`.
        sender: MarktpartnerCode,
        /// The Messlokation — `SG2 LOC+172`.
        location_id: Option<String>,
        /// Interchange reference of the inbound ORDERS.
        message_ref: MessageRef,
        /// What the MSB's answer echoes from the ORDERS.
        kopf: AuftragKopf,
    },
    /// MSB side: the MSB answered with ORDRSP 19005 / 19006.
    AntwortGesendet {
        /// 19005 or 19006 — picked by the code's Cluster.
        ordrsp_pid: Pruefidentifikator,
        /// The code.
        antwortcode: String,
        /// The tree it was read from (`E_0249`, `E_0250`, `E_0279`, `E_0283`).
        codeliste: String,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// MSB side: the received ORDERS failed its AHB check.
    AuftragUngueltig {
        /// The findings.
        reason: String,
    },
    /// ORDRSP received — confirmation.
    AuftragBestaetigt {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Message reference.
        message_ref: MessageRef,
        /// `SG2 AJT` — the MSB's code and the tree it names.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        antwort: Option<Antwortcode>,
    },
    /// ORDRSP received — rejection.
    AuftragAbgelehnt {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
        /// `SG2 AJT` — the MSB's code and the tree it names.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        antwort: Option<Antwortcode>,
    },
    /// Deadline expired before ORDRSP arrived.
    DeadlineExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for TechnikAenderungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AuftragGesendet { .. } => "TechnikAenderungAuftragGesendet",
            Self::AuftragEingegangen { .. } => "TechnikAenderungAuftragEingegangen",
            Self::AntwortGesendet { .. } => "TechnikAenderungAntwortGesendet",
            Self::AuftragUngueltig { .. } => "TechnikAenderungAuftragUngueltig",
            Self::AuftragBestaetigt { .. } => "TechnikAenderungAuftragBestaetigt",
            Self::AuftragAbgelehnt { .. } => "TechnikAenderungAuftragAbgelehnt",
            Self::DeadlineExpired { .. } => "TechnikAenderungDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        let (richtung, pid, message_ref) = match self {
            Self::AuftragGesendet {
                orders_pid,
                message_ref,
                ..
            } => (Richtung::Ausgang, orders_pid, message_ref),
            Self::AuftragEingegangen {
                orders_pid,
                message_ref,
                ..
            } => (Richtung::Eingang, orders_pid, message_ref),
            Self::AntwortGesendet {
                ordrsp_pid,
                message_ref,
                ..
            } => (Richtung::Ausgang, ordrsp_pid, message_ref),
            Self::AuftragBestaetigt {
                ordrsp_pid,
                message_ref,
                ..
            }
            | Self::AuftragAbgelehnt {
                ordrsp_pid,
                message_ref,
                ..
            } => (Richtung::Eingang, ordrsp_pid, message_ref),
            Self::DeadlineExpired { .. } | Self::AuftragUngueltig { .. } => return None,
        };
        Some(Verlauf {
            richtung,
            pid: Some(pid.as_u32()),
            message_ref: Some(message_ref.as_str().to_owned()),
        })
    }

    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AntwortGesendet {
                antwortcode,
                codeliste,
                ..
            } => Some(Antwortcode {
                code: antwortcode.clone(),
                codeliste: Some(codeliste.clone()),
            }),
            Self::AuftragBestaetigt { antwort, .. } | Self::AuftragAbgelehnt { antwort, .. } => {
                antwort.clone()
            }
            _ => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Request data.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuftragData {
    /// ORDERS PID.
    pub orders_pid: Pruefidentifikator,
    /// Recipient GLN.
    pub recipient: MarktpartnerCode,
    /// Location identifier.
    pub location_id: Option<String>,
    /// Message reference.
    pub message_ref: MessageRef,
}

/// A received Auftrag (MSB side).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EingangData {
    /// ORDERS PID.
    pub orders_pid: Pruefidentifikator,
    /// The NB or LF that sent it.
    pub sender: MarktpartnerCode,
    /// The Messlokation.
    pub location_id: Option<String>,
    /// Interchange reference of the inbound ORDERS.
    pub message_ref: MessageRef,
    /// What the MSB's answer echoes from the ORDERS.
    pub kopf: AuftragKopf,
}

/// What the MSB's ORDRSP 19005 / 19006 echoes from the received ORDERS 17011
/// (ORDRSP AHB 1.1b Kap. 4.12, S. 50–53).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AuftragKopf {
    /// The MSB the Auftrag was sent to — `NAD+MR`; the answer's sender.
    pub empfaenger: String,
    /// `BGM` DE 1004 — echoed in `SG1 RFF+ON` (Hinweis `[513]` / `[543]`).
    pub dokumentennummer: String,
    /// `BGM` DE 1001 — `Z12` or `Z93`, echoed in the answer's `BGM`.
    pub beauftragung: String,
    /// `DTM+203` Ausführungsdatum on a `Z12` Auftrag (`CCYYMMDDHHMM`), Muss on
    /// the answer under `[83]`.
    pub ausfuehrungsdatum: Option<String>,
}

/// Current state of a WiM Technikänderung process.
///
/// ```text
/// New → AuftragGesendet → Bestaetigt
///                      ↘ Abgelehnt
///                      ↘ FristAbgelaufen
/// New → AuftragEingegangen → Beantwortet       (MSB side)
///                         ↘ FristAbgelaufen
///     ↘ Ungueltig
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum TechnikAenderungState {
    /// No ORDERS sent yet.
    #[default]
    New,
    /// ORDERS sent; awaiting ORDRSP.
    AuftragGesendet(AuftragData),
    /// MSB side: an Auftrag arrived; the MSB owes the ORDRSP 19005/19006.
    AuftragEingegangen(EingangData),
    /// MSB side: the MSB answered the received Auftrag.
    Beantwortet {
        /// The received Auftrag.
        eingang: EingangData,
        /// 19005 or 19006.
        ordrsp_pid: Pruefidentifikator,
        /// The code.
        antwortcode: String,
    },
    /// MSB side: the received Auftrag failed its AHB check.
    Ungueltig {
        /// The findings.
        reason: String,
    },
    /// MSB/NB confirmed the change.
    Bestaetigt {
        /// Original request data.
        auftrag: AuftragData,
        /// Confirmation ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
    },
    /// MSB/NB rejected the change.
    Abgelehnt {
        /// Original request data.
        auftrag: AuftragData,
        /// Rejection ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// No Festlegung states a consequence of the Änderung der Technik going unanswered. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<TechnikAenderungState>,
    },
}

impl TechnikAenderungState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AuftragGesendet(_) => "AuftragGesendet",
            Self::AuftragEingegangen(_) => "AuftragEingegangen",
            Self::Beantwortet { .. } => "Beantwortet",
            Self::Ungueltig { .. } => "Ungueltig",
            Self::Bestaetigt { .. } => "Bestaetigt",
            Self::Abgelehnt { .. } => "Abgelehnt",
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

    /// Whether terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Bestaetigt { .. }
                | Self::Abgelehnt { .. }
                | Self::Ungueltig { .. }
                | Self::Beantwortet { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Technikänderung workflow.
#[derive(Clone)]
pub enum TechnikAenderungCommand {
    /// Send an ORDERS technical change request.
    SendAuftrag {
        /// ORDERS PID.
        orders_pid: Pruefidentifikator,
        /// Recipient GLN.
        recipient: MarktpartnerCode,
        /// Location ID (optional).
        location_id: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
        /// ORDERS body payload.
        payload: serde_json::Value,
        /// When the ORDERS leaves — the Übertragungstag the counterparty's
        /// Antwortfrist counts from ([`mako_fristen::antwort::antwort_deadline`]).
        gesendet_am: time::OffsetDateTime,
    },
    /// MSB side: an inbound ORDERS Auftrag (17011).
    ReceiveAuftrag {
        /// ORDERS PID read from `SG1 RFF+Z13`.
        orders_pid: Pruefidentifikator,
        /// The NB or LF that sent it — `NAD+MS`.
        sender: MarktpartnerCode,
        /// The Messlokation — `SG2 LOC+172`.
        location_id: Option<String>,
        /// Interchange reference of the inbound ORDERS.
        message_ref: MessageRef,
        /// What the MSB's answer echoes from the ORDERS.
        kopf: AuftragKopf,
        /// `true` when the AHB check passed.
        validation_passed: bool,
        /// The findings when it did not.
        validation_errors: Vec<String>,
    },
    /// MSB side: answer the received Auftrag with ORDRSP 19005 / 19006.
    SendAntwort {
        /// Who ordered — with the ORDERS' `BGM` DE 1001 it selects the tree
        /// ([`mako_pruefung::codes::aenderung_der_technik_baum`]).
        besteller: mako_pruefung::codes::TechnikBesteller,
        /// The code; its Cluster picks 19005 or 19006.
        antwortcode: String,
        /// `SG2 FTX+AAP` — Muss beside `A99` (`[28]`).
        freitext: Option<String>,
        /// `SG6 CTA`/`COM` — Muss on both answers (S. 53).
        ansprechpartner: crate::geraeteubernahme::Ansprechpartner,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP received.
    ReceiveOrdrsp {
        /// ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason (for negative responses).
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
        /// `SG2 AJT` — the MSB's code and the tree it names.
        antwort: Option<Antwortcode>,
    },
    /// Deadline fired.
    TimeoutExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Label.
        label: Box<str>,
    },
}

impl CommandPayload for TechnikAenderungCommand {}

// ── The MSB's answer ──────────────────────────────────────────────────────────

/// The MSB's ORDRSP 19005 / 19006 to a received 17011 (ORDRSP AHB 1.1b Kap.
/// 4.12, S. 50–53): `BGM` the ORDERS' DE 1001, `DTM+203` the ORDERS'
/// Ausführungsdatum on a `Z12` Auftrag (`[83]`), `SG1 RFF+ON` its DE 1004,
/// `SG2 AJT` the code with „EBD Nr." of the tree the Besteller and the
/// Beauftragungsart select, `SG2 FTX+AAP` beside `A99` (`[28]`) and `SG6
/// CTA`/`COM` the Ansprechpartner.
///
/// On a `Z93` Bestellung the Ablehnungen `A04` (a realistic Umsetzungszeitraum
/// in `DTM+469`/`DTM+472`, `[78]`/`[79]`) and `A05` (the PRICAT in `SG1
/// RFF+ACW`, `[80]`/`[81]`) carry facts this workflow does not hold, and are
/// refused by name rather than sent without them.
fn antwort(
    eingang: &EingangData,
    besteller: mako_pruefung::codes::TechnikBesteller,
    antwortcode: String,
    freitext: Option<&str>,
    ansprechpartner: &crate::geraeteubernahme::Ansprechpartner,
    message_ref: MessageRef,
) -> Result<WorkflowOutput<TechnikAenderungEvent>, WorkflowError> {
    use mako_pruefung::codes::TechnikBeauftragung;
    let kopf = &eingang.kopf;
    let art = beauftragungsart(&kopf.beauftragung).ok_or_else(|| {
        WorkflowError::validation(format!(
            "the Auftrag's BGM DE 1001 {:?} is neither Z12 nor Z93 — no tree answers it",
            kopf.beauftragung
        ))
    })?;
    let tree = mako_pruefung::codes::aenderung_der_technik_baum(besteller, art);
    let code = mako_pruefung::codes::lookup(tree, &antwortcode).ok_or_else(|| {
        WorkflowError::validation(format!("{antwortcode:?} is not published in {tree}"))
    })?;
    let ordrsp_pid = match code.ist_zustimmung() {
        Some(true) => BESTAETIGUNG_PID,
        Some(false) => ABLEHNUNG_PID,
        None => {
            return Err(WorkflowError::validation(format!(
                "{antwortcode:?} of {tree} is neither a Zustimmung nor an Ablehnung"
            )));
        }
    };
    if art == TechnikBeauftragung::BestellungNachAngebot && matches!(code.code, "A04" | "A05") {
        return Err(WorkflowError::validation(format!(
            "{antwortcode} of {tree} needs {} on the 19006 (ORDRSP AHB 1.1b Kap. 4.12), \
             which this answer does not carry",
            if code.code == "A04" {
                "the realistic Umsetzungszeitraum (DTM+469/DTM+472)"
            } else {
                "the PRICAT's BGM DE 1004 (SG1 RFF+ACW)"
            }
        )));
    }
    if code.braucht_bemerkung && freitext.is_none_or(|t| t.trim().is_empty()) {
        return Err(WorkflowError::validation(format!(
            "{antwortcode:?} of {tree} requires the problem named in SG2 FTX+AAP ([28])"
        )));
    }
    if ansprechpartner.name.trim().is_empty() || ansprechpartner.kommunikation.is_empty() {
        return Err(WorkflowError::validation(
            "SG6 CTA/COM is Muss on 19005/19006: the Ansprechpartner needs a name and a channel",
        ));
    }
    if art == TechnikBeauftragung::DirekteBeauftragung && kopf.ausfuehrungsdatum.is_none() {
        return Err(WorkflowError::validation(
            "a Z12 answer echoes the Auftrag's DTM+203 Ausführungsdatum ([83]); the received \
             Auftrag carried none",
        ));
    }
    let pid = Pruefidentifikator::new(ordrsp_pid)
        .map_err(|e| WorkflowError::validation(format!("answer PID {ordrsp_pid}: {e}")))?;
    let payload = mako_engine::payload::OrdrspPayload {
        pid: Some(ordrsp_pid),
        sender: Some(kopf.empfaenger.clone()),
        receiver: Some(eingang.sender.as_str().to_owned()),
        message_ref: Some(message_ref.as_str().to_owned()),
        document_code: Some(kopf.beauftragung.clone()),
        document_id: Some(message_ref.as_str().to_owned()),
        order_reference: Some(kopf.dokumentennummer.clone()),
        antwort_code: Some(antwortcode.clone()),
        antwort_codeliste: Some(tree.to_owned()),
        antwort_freitext: freitext.map(str::to_owned),
        ansprechpartner: Some(ansprechpartner.to_payload()),
        sparte: Some(mako_engine::types::Sparte::Strom),
        ausfuehrungsdatum: (art == TechnikBeauftragung::DirekteBeauftragung)
            .then(|| kopf.ausfuehrungsdatum.clone())
            .flatten(),
        ..Default::default()
    };
    let outbox = vec![PendingOutbox::typed(eingang.sender.as_str(), &payload).caused_by(0)];
    Ok(WorkflowOutput::with_outbox(
        vec![TechnikAenderungEvent::AntwortGesendet {
            ordrsp_pid: pid,
            antwortcode,
            codeliste: tree.to_owned(),
            message_ref,
        }],
        outbox,
    ))
}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Technikänderung workflow — device/config change requests.
pub struct WimTechnikAenderungWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The counterparty's answer window is open while the sent Auftrag waits for
/// its ORDRSP (`AuftragGesendet`); the MSB's own while a received one waits
/// for the MSB's (`AuftragEingegangen`).
fn window_open(label: &str, state: &TechnikAenderungState) -> bool {
    match label {
        ANTWORT_WINDOW_LABEL => matches!(state, TechnikAenderungState::AuftragGesendet(_)),
        EINGANG_WINDOW_LABEL => matches!(state, TechnikAenderungState::AuftragEingegangen(_)),
        _ => false,
    }
}

impl Workflow for WimTechnikAenderungWorkflow {
    type State = TechnikAenderungState;
    type Event = TechnikAenderungEvent;
    type Command = TechnikAenderungCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL, EINGANG_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| TechnikAenderungCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            TechnikAenderungState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            TechnikAenderungEvent::AuftragGesendet {
                orders_pid,
                recipient,
                location_id,
                message_ref,
            } => TechnikAenderungState::AuftragGesendet(AuftragData {
                orders_pid: *orders_pid,
                recipient: recipient.clone(),
                location_id: location_id.clone(),
                message_ref: message_ref.clone(),
            }),
            TechnikAenderungEvent::AuftragEingegangen {
                orders_pid,
                sender,
                location_id,
                message_ref,
                kopf,
            } => TechnikAenderungState::AuftragEingegangen(EingangData {
                orders_pid: *orders_pid,
                sender: sender.clone(),
                location_id: location_id.clone(),
                message_ref: message_ref.clone(),
                kopf: kopf.clone(),
            }),
            TechnikAenderungEvent::AntwortGesendet {
                ordrsp_pid,
                antwortcode,
                ..
            } => match state {
                TechnikAenderungState::AuftragEingegangen(eingang) => {
                    TechnikAenderungState::Beantwortet {
                        eingang,
                        ordrsp_pid: *ordrsp_pid,
                        antwortcode: antwortcode.clone(),
                    }
                }
                other => other,
            },
            TechnikAenderungEvent::AuftragUngueltig { reason } => {
                TechnikAenderungState::Ungueltig {
                    reason: reason.clone(),
                }
            }
            TechnikAenderungEvent::AuftragBestaetigt { ordrsp_pid, .. } => match state {
                TechnikAenderungState::AuftragGesendet(auftrag) => {
                    TechnikAenderungState::Bestaetigt {
                        auftrag,
                        ordrsp_pid: *ordrsp_pid,
                    }
                }
                other => other,
            },
            TechnikAenderungEvent::AuftragAbgelehnt {
                ordrsp_pid, reason, ..
            } => match state {
                TechnikAenderungState::AuftragGesendet(auftrag) => {
                    TechnikAenderungState::Abgelehnt {
                        auftrag,
                        ordrsp_pid: *ordrsp_pid,
                        reason: reason.clone(),
                    }
                }
                other => other,
            },
            TechnikAenderungEvent::DeadlineExpired { label, .. } => match state {
                TechnikAenderungState::AuftragGesendet(_)
                | TechnikAenderungState::AuftragEingegangen(_) => {
                    TechnikAenderungState::FristAbgelaufen {
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
            TechnikAenderungCommand::SendAuftrag {
                orders_pid,
                recipient,
                location_id,
                message_ref,
                payload,
                gesendet_am,
            } => {
                if !matches!(state, TechnikAenderungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid WiM Technikänderung ORDERS PID: {orders_pid}",
                    )));
                }
                let event = TechnikAenderungEvent::AuftragGesendet {
                    orders_pid,
                    recipient: recipient.clone(),
                    location_id: location_id.clone(),
                    message_ref: message_ref.clone(),
                };
                // The column's fields arrive from the caller; a key the ORDERS
                // does not declare is refused here, at the command.
                let mut body: mako_engine::payload::OrdersPayload = if payload.is_null() {
                    mako_engine::payload::OrdersPayload::default()
                } else {
                    serde_json::from_value(payload)
                        .map_err(|e| WorkflowError::rejected(format!("ORDERS {orders_pid}: {e}")))?
                };
                body.pid = Some(orders_pid.as_u32());
                body.location = location_id.or(body.location);
                body.message_ref = Some(message_ref.as_str().to_owned());
                body.document_id = Some(message_ref.as_str().to_owned());
                let outbox = vec![PendingOutbox::typed(recipient.as_str(), &body)];
                // The counterparty's Antwortfrist, from the published row for
                // this ORDERS; a PID no Festlegung quantifies registers nothing.
                let frist: Vec<PendingDeadline> =
                    mako_fristen::antwort::antwort_deadline(orders_pid.as_u32(), gesendet_am)
                        .map(|due| PendingDeadline::new(ANTWORT_WINDOW_LABEL, due))
                        .into_iter()
                        .collect();
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    vec![event],
                    outbox,
                    frist,
                ))
            }

            TechnikAenderungCommand::ReceiveAuftrag {
                orders_pid,
                sender,
                location_id,
                message_ref,
                kopf,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, TechnikAenderungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !RECEIVE_ORDERS_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not an ORDERS the MSB receives in the Technikänderung: {orders_pid}",
                    )));
                }
                let mut events = vec![TechnikAenderungEvent::AuftragEingegangen {
                    orders_pid,
                    sender,
                    location_id,
                    message_ref,
                    kopf,
                }];
                if !validation_passed {
                    events.push(TechnikAenderungEvent::AuftragUngueltig {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            TechnikAenderungCommand::SendAntwort {
                besteller,
                antwortcode,
                freitext,
                ansprechpartner,
                message_ref,
            } => {
                let TechnikAenderungState::AuftragEingegangen(eingang) = state else {
                    return Err(WorkflowError::invalid_state(
                        "AuftragEingegangen",
                        state.label(),
                    ));
                };
                antwort(
                    eingang,
                    besteller,
                    antwortcode,
                    freitext.as_deref(),
                    &ansprechpartner,
                    message_ref,
                )
            }

            TechnikAenderungCommand::ReceiveOrdrsp {
                ordrsp_pid,
                reason,
                message_ref,
                antwort,
            } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !matches!(state, TechnikAenderungState::AuftragGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AuftragGesendet",
                        state.label(),
                    ));
                }
                if !ORDRSP_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid WiM Technikänderung ORDRSP PID: {ordrsp_pid}",
                    )));
                }
                let event = if ORDRSP_BESTAETIGUNG_PIDS.contains(&ordrsp_pid.as_u32()) {
                    TechnikAenderungEvent::AuftragBestaetigt {
                        ordrsp_pid,
                        message_ref,
                        antwort,
                    }
                } else {
                    TechnikAenderungEvent::AuftragAbgelehnt {
                        ordrsp_pid,
                        reason,
                        message_ref,
                        antwort,
                    }
                };
                Ok(vec![event].into())
            }

            TechnikAenderungCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![TechnikAenderungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auftrag(pid: u32, gesendet_am: time::OffsetDateTime) -> TechnikAenderungCommand {
        TechnikAenderungCommand::SendAuftrag {
            orders_pid: Pruefidentifikator::new(pid).expect("valid PID"),
            recipient: MarktpartnerCode::new("9900000000003"),
            location_id: None,
            message_ref: MessageRef::new("ORDERS-1"),
            payload: serde_json::json!({}),
            gesendet_am,
        }
    }

    /// A 17011 starts the MSB's 10-WT answer window (WiM Teil 1 Kap. 3.3); a
    /// 17118 is GPKE Teil 3's and is not sent here.
    #[test]
    fn the_auftrag_registers_the_published_answer_window() {
        let sent = time::macros::datetime!(2026-10-07 10:00 UTC);
        let out = WimTechnikAenderungWorkflow::handle(
            &TechnikAenderungState::default(),
            auftrag(17_011, sent),
        )
        .expect("accepted");
        assert_eq!(out.deadlines.len(), 1);
        assert_eq!(out.deadlines[0].label, ANTWORT_WINDOW_LABEL);
        // The ORDERS reads back as its typed payload, the Belegnummer under
        // the key the renderer reads.
        use mako_engine::payload::OutboundPayload as _;
        let typed = mako_engine::payload::OrdersPayload::from_payload(&out.outbox[0].payload)
            .expect("an ORDERS payload");
        assert_eq!(typed.message_ref.as_deref(), Some("ORDERS-1"));
        assert_eq!(
            Some(out.deadlines[0].due_at),
            mako_fristen::antwort::antwort_deadline(17_011, sent)
        );

        assert!(
            WimTechnikAenderungWorkflow::handle(
                &TechnikAenderungState::default(),
                auftrag(17_118, sent),
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod eingang_tests {
    use super::*;

    fn receive(pid: u32) -> TechnikAenderungCommand {
        TechnikAenderungCommand::ReceiveAuftrag {
            orders_pid: Pruefidentifikator::new(pid).expect("valid PID"),
            sender: MarktpartnerCode::new("9900000000003"),
            location_id: Some("DE00056266802AO6G56M11SN51G21M24S".to_owned()),
            message_ref: MessageRef::new("ORD-17011"),
            kopf: AuftragKopf {
                empfaenger: "9900555000005".to_owned(),
                dokumentennummer: "TECH-4711".to_owned(),
                beauftragung: BGM_DIREKTE_BEAUFTRAGUNG.to_owned(),
                ausfuehrungsdatum: Some("202612312300".to_owned()),
            },
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn fold(
        state: TechnikAenderungState,
        out: &WorkflowOutput<TechnikAenderungEvent>,
    ) -> TechnikAenderungState {
        out.events
            .iter()
            .fold(state, WimTechnikAenderungWorkflow::apply)
    }

    /// The MSB's 10-WT window runs from the received Auftrag, and a lapse
    /// leaves the answer owed — no Festlegung gives it an outcome.
    #[test]
    fn a_received_auftrag_owes_its_answer_within_ten_werktage() {
        let eingang = time::macros::datetime!(2026-10-07 10:00 UTC);
        assert_eq!(
            eingang_antwortfrist(17_011, eingang),
            mako_fristen::antwort::antwort_deadline(17_011, eingang)
        );
        assert!(eingang_antwortfrist(17_011, eingang).is_some());
        assert_eq!(eingang_antwortfrist(17_118, eingang), None);

        let out = WimTechnikAenderungWorkflow::handle(&TechnikAenderungState::New, receive(17_011))
            .expect("accepted");
        assert_eq!(
            out.events[0].verlauf().map(|v| (v.richtung, v.pid)),
            Some((Richtung::Eingang, Some(17_011)))
        );
        let offen = fold(TechnikAenderungState::New, &out);
        assert!(matches!(
            offen,
            TechnikAenderungState::AuftragEingegangen(_)
        ));

        // The requester's label does not touch the MSB's process.
        let out = WimTechnikAenderungWorkflow::handle(
            &offen,
            TechnikAenderungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .expect("accepted");
        assert!(out.events.is_empty());

        let out = WimTechnikAenderungWorkflow::handle(
            &offen,
            TechnikAenderungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: EINGANG_WINDOW_LABEL.into(),
            },
        )
        .expect("accepted");
        let lapsed = fold(offen, &out);
        assert!(
            matches!(&lapsed, TechnikAenderungState::FristAbgelaufen { offen, .. }
                if matches!(**offen, TechnikAenderungState::AuftragEingegangen(_))),
            "{lapsed:?}"
        );
        assert!(!lapsed.is_terminal());
    }

    fn ansprechpartner() -> crate::geraeteubernahme::Ansprechpartner {
        crate::geraeteubernahme::Ansprechpartner {
            name: "Technik-Hotline".to_owned(),
            kommunikation: vec![crate::geraeteubernahme::Kommunikationsweg::Telefon(
                "+4922112345".to_owned(),
            )],
        }
    }

    fn antwort(
        besteller: mako_pruefung::codes::TechnikBesteller,
        code: &str,
    ) -> TechnikAenderungCommand {
        TechnikAenderungCommand::SendAntwort {
            besteller,
            antwortcode: code.to_owned(),
            freitext: None,
            ansprechpartner: ansprechpartner(),
            message_ref: MessageRef::new("ORDRSP-1"),
        }
    }

    /// The Besteller's tree decides: `A02` is the Zustimmung of `E_0249`
    /// (19005) and a code `E_0249` does not publish is refused; the answer
    /// echoes the Auftrag's `BGM` DE 1001 / 1004 and `DTM+203` and ends the
    /// MSB's process.
    #[test]
    fn an_answered_auftrag_reaches_its_end_state() {
        use mako_pruefung::codes::TechnikBesteller;
        let offen = fold(
            TechnikAenderungState::New,
            &WimTechnikAenderungWorkflow::handle(&TechnikAenderungState::New, receive(17_011))
                .expect("accepted"),
        );
        let out = WimTechnikAenderungWorkflow::handle(
            &offen,
            antwort(TechnikBesteller::Netzbetreiber, "A02"),
        )
        .expect("answered");
        let p = &out.outbox[0].payload;
        assert_eq!(p["pid"], BESTAETIGUNG_PID);
        assert_eq!(p["antwort_codeliste"], "E_0249");
        assert_eq!(p["document_code"], "Z12");
        assert_eq!(p["order_reference"], "TECH-4711");
        assert_eq!(p["ausfuehrungsdatum"], "202612312300");
        assert_eq!(p["sender"], "9900555000005");
        assert_eq!(
            out.events[0].antwort().map(|a| a.code),
            Some("A02".to_owned())
        );
        let ende = fold(offen.clone(), &out);
        assert!(ende.is_terminal(), "{ende:?}");

        // The LF's tree: `A03` (Vollmacht fehlt) is an Ablehnung of `E_0250`
        // and unpublished in `E_0249`.
        let out = WimTechnikAenderungWorkflow::handle(
            &offen,
            antwort(TechnikBesteller::Lieferant, "A03"),
        )
        .expect("refused");
        assert_eq!(out.outbox[0].payload["pid"], ABLEHNUNG_PID);
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "E_0250");
        use mako_engine::payload::OutboundPayload as _;
        let typed = mako_engine::payload::OrdrspPayload::from_payload(&out.outbox[0].payload)
            .expect("an ORDRSP payload");
        assert!(typed.ansprechpartner.is_some() && typed.order_reference.is_some());
        assert!(
            WimTechnikAenderungWorkflow::handle(
                &offen,
                antwort(TechnikBesteller::Netzbetreiber, "A03")
            )
            .is_err()
        );
    }

    /// 17118 is GPKE Teil 3's, answered by ORDRSP 19127: the MSB does not
    /// receive it as a Technikänderung.
    #[test]
    fn only_17011_is_received() {
        assert!(
            WimTechnikAenderungWorkflow::handle(&TechnikAenderungState::New, receive(17_118))
                .is_err()
        );
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
        let auftrag = TechnikAenderungEvent::AuftragGesendet {
            orders_pid: pid(17_011),
            recipient: MarktpartnerCode::new("9900000000001"),
            location_id: None,
            message_ref: MessageRef::new("ORD-1"),
        };
        assert_eq!(
            auftrag.verlauf(),
            Some(stated(Richtung::Ausgang, Some(17_011), Some("ORD-1")))
        );
        let abgelehnt = TechnikAenderungEvent::AuftragAbgelehnt {
            ordrsp_pid: pid(19_006),
            reason: None,
            message_ref: MessageRef::new("RSP-1"),
            antwort: Some(Antwortcode::new("A01", Some("E_0249"))),
        };
        assert_eq!(
            abgelehnt.verlauf(),
            Some(stated(Richtung::Eingang, Some(19_006), Some("RSP-1")))
        );
        // The MSB's code rides the received answer's envelope.
        assert_eq!(abgelehnt.antwort().map(|a| a.code), Some("A01".to_owned()));
        let abgelaufen = TechnikAenderungEvent::DeadlineExpired {
            deadline_id: DeadlineId::new(),
            label: "x".into(),
        };
        assert_eq!(abgelaufen.verlauf(), None);
    }
}
