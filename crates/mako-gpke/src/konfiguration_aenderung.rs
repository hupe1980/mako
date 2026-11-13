//! GPKE Teil 3 Konfigurationsänderung — the Bestellungen, Reklamationen and
//! Beendigungen of a Konfiguration, both sides.
//!
//! This module covers the ORDERS that order, change, complain about or end a
//! Konfiguration established via [`crate::konfiguration`], together with the
//! ORDRSP answers. One workflow holds both sides, keyed apart: the party that
//! sends an ORDERS runs [`KonfigurationAenderungCommand::SendAnfrage`], the
//! party that receives one runs [`KonfigurationAenderungCommand::ReceiveAuftrag`]
//! under [`auftrag_key`] — the sender and the ORDERS' `BGM` DE 1004, which the
//! answer's `SG1 RFF+ON` echoes — so a deployment that is both holds its sent
//! ORDERS and the received copy as two processes.
//!
//! # Who receives what
//!
//! Anwendungsübersicht der Prüfidentifikatoren 4.0 lfd. Nr. 22180–23240
//! (GPKE Teil 3): the NB receives 17120, 17122, 17123 and 17133 from the LF
//! (17122 also from the MSB); the MSB receives 17121, 17123, 17128, 17129,
//! 17130 and 17131 from the NB or the LF, and — as the „weiterer MSB" —
//! 17118, 17128, 17129 and 17130 from the MSB; the LF receives 17122 from the
//! NB or the MSB. [`ORDERS_EMPFANG_NB_PIDS`], [`ORDERS_EMPFANG_MSB_PIDS`] and
//! [`ORDERS_EMPFANG_LF_PIDS`] name each role's set. The MSB also receives the
//! Einrichtung der Konfigurationen 17134 / 17135 ([`ORDERS_EINRICHTUNG_PIDS`]),
//! whose sending side is [`crate::konfiguration`].
//!
//! # The recipient's answer
//!
//! [`KonfigurationAenderungCommand::SendAntwort`] answers a received ORDERS on
//! the ORDRSP the AHB pairs with it, out of a tree its column names
//! ([`antwort_auf`]; ORDRSP AHB 1.1b Kap. 4.4–4.7): `SG1
//! RFF+ON` the ORDERS' `BGM` DE 1004, `SG2 AJT` the code with „EBD Nr.", and
//! `SG2 FTX+AAP` where the code needs the problem named (`[28]`, `[72]`–`[74]`).
//! The Ablehnung der Reklamation einer Definition 19123 is refused by name:
//! its column names the UTILTS that carried the Definition, which a received
//! 17122 does not give this process.
//!
//! # Process families
//!
//! | PID   | Direction   | Description |
//! |-------|-------------|-------------|
//! | 17120 | LF → NB     | Bestellung Änderung Prognosegrundlage |
//! | 17122 | LF → NB/MSB | Reklamation einer Definition |
//! | 17123 | LF → NB/MSB | Bestellung Änderung Zählzeitdefinition |
//! | 17133 | LF → NB     | Bestellung Änderung Abrechnungsdaten |
//! | 17128 | LF → MSB    | Reklamation einer Konfiguration |
//! | 17129 | LF → MSB    | Bestellung Beendigung einer Konfiguration |
//! | 17130 | LF → MSB    | Bestellung einer Konfiguration (ohne Angebot) |
//! | 17131 | LF → MSB    | Bestellung Angebot einer Konfiguration |
//! | 17121 | NB → MSB    | Bestellung Änderung (NB forwards, MSB receives) |
//! | 17118 | MSB → weiterer MSB | Bestellung einer Konfigurationsänderung |
//! | 19120 | NB → LF     | Mitteilung zur Änderung |
//! | 19121 | NB → LF     | Mitteilung Änderung Prognosegrundlage |
//! | 19123 | NB/MSB → LF | Ablehnung der Reklamation einer Definition |
//! | 19124 | NB/MSB → LF | Mitteilung Änderung Zählzeitdefinition |
//! | 19127 | weiterer MSB → MSB | Mitteilung zur Konfigurationsänderung (answers 17118) |
//! | 19133 | NB → LF     | Bearbeitungsstand Bestellung Abrechnungsdaten |
//! | 19130 | MSB → LF    | Bearbeitungsstand Reklamation Konfiguration |
//! | 19131 | MSB → LF    | Antwort auf Bestellung Beendigung einer Konfiguration |
//! | 19132 | MSB → LF    | Mitteilung Bestellung Änderung einer Konfiguration |
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE Teil 3** — Geschäftsprozesse Konfiguration (BK6-24-174 Anlage 1c)
//! - Response window: per Use-Case (GPKE Teil 3 Kap. 1.2–1.3); the one this
//!   workflow resolves is the Mitteilung on a 17120, 1 WT (see `antwortfrist`)

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name for the GPKE Konfigurationsänderung process.
pub const WORKFLOW_NAME: &str = "gpke-konfiguration-aenderung";

/// ORDERS PIDs sent by LF to NB for configuration change requests.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17120 | Bestellung Änderung Prognosegrundlage |
/// | 17122 | Reklamation einer Definition |
/// | 17123 | Bestellung Änderung Zählzeitdefinition |
/// | 17133 | Bestellung Änderung Abrechnungsdaten |
pub const ORDERS_ANFRAGE_NB_PIDS: &[u32] = &[17120, 17122, 17123, 17133];

/// ORDERS PIDs sent by LF to MSB for configuration change requests.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17128 | Reklamation einer Konfiguration |
/// | 17129 | Bestellung Beendigung einer Konfiguration |
/// | 17130 | Bestellung einer Konfiguration (ohne Angebot) |
/// | 17131 | Bestellung Angebot einer Konfiguration |
pub const ORDERS_ANFRAGE_MSB_PIDS: &[u32] = &[17128, 17129, 17130, 17131];

/// ORDERS PID sent by NB to MSB (NB-internal forwarding, also received by MSB role).
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17121 | Bestellung Änderung (NB → MSB) |
pub const ORDERS_NB_MSB_PIDS: &[u32] = &[17121];

/// ORDERS PID a MSB sends to the weiterer MSB.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17118 | Bestellung einer Konfigurationsänderung (Anwendungsübersicht 4.0 lfd. Nr. 22600 / 22960) |
pub const ORDERS_MSB_MSB_PIDS: &[u32] = &[17118];

/// Every ORDERS of this workflow, for registration.
pub const ORDERS_ANFRAGE_PIDS: &[u32] = &[
    17118, 17120, 17121, 17122, 17123, 17128, 17129, 17130, 17131, 17133,
];

/// The ORDERS a NB receives here (Anwendungsübersicht 4.0 lfd. Nr. 22180,
/// 22210, 22420, 22430, 21320).
pub const ORDERS_EMPFANG_NB_PIDS: &[u32] = &[17120, 17122, 17123, 17133];

/// The ORDERS a MSB receives here — from the NB or the LF, or as the
/// „weiterer MSB" from the MSB (lfd. Nr. 22540–22600, 22720–22780,
/// 22870–22960, 23010–23160).
pub const ORDERS_EMPFANG_MSB_PIDS: &[u32] = &[17118, 17121, 17123, 17128, 17129, 17130, 17131];

/// The ORDERS a LF receives here — the Reklamation einer Definition from the
/// NB or the MSB (lfd. Nr. 22240, 22270, 22360, 22390).
pub const ORDERS_EMPFANG_LF_PIDS: &[u32] = &[17122];

/// The ORDRSP a MSB receives here from the weiterer MSB (lfd. Nr. 22610,
/// 22620, 22920, 22970, 22980).
pub const ORDRSP_EMPFANG_MSB_PIDS: &[u32] = &[19127, 19130, 19131, 19132];

/// The Einrichtung der Konfigurationen aufgrund einer Zuordnung eines LF —
/// ORDERS 17134 „NB an MSB" (ORDERS AHB 1.1b Kap. 4.17.1, S. 102) and 17135
/// „MSB an MSB" (Kap. 4.17.2, S. 111). The MSB receives both; neither is
/// answered by an ORDRSP: the MSB sets the Konfigurationen up — „spätestens am
/// WT vor dem Beginn des Wirkungszeitraums bis 22:00 Uhr" where that lies in
/// the future — forwards a 17135 to each weiterer MSB „spätester ÜZ ist 1
/// Stunde nach dem ÜZ von Nr. 1", and reports through the Stammdatenänderung
/// (GPKE Teil 3 Kap. 1.3.7.2 Nr. 1–3, S. 82–84). The window anchors on the
/// ORDERS' `DTM+203` and is not registered here.
pub const ORDERS_EINRICHTUNG_PIDS: &[u32] = &[17134, 17135];

/// Whether `pid` is an ORDERS this workflow receives.
#[must_use]
pub fn ist_auftrag(pid: u32) -> bool {
    ORDERS_ANFRAGE_PIDS.contains(&pid) || ORDERS_EINRICHTUNG_PIDS.contains(&pid)
}

/// The ORDRSP that answers a received ORDERS, its `BGM` DE 1001 and the trees
/// its `SG2 AJT` may name (ORDRSP AHB 1.1b Kap. 4.4.2 S. 23–24, 4.4.3 S. 25–26,
/// 4.4.4 S. 27–28, 4.4.5 S. 29–30, 4.4.6 S. 31–32, 4.6 S. 36–37, 4.7 S. 38–39).
///
/// The column picks the tree by the role of the party answered (`SG3 NAD+MR`)
/// or of the answering one (`NAD+MS`); the caller names it, and only the
/// column's trees are admitted. 19132 echoes the Bestellung's own `BGM` (`Z73`
/// Bestellung einer Konfiguration, `Z74` Bestellung eines Angebots).
///
/// | ORDERS | ORDRSP | `BGM` | Trees |
/// |---|---|---|---|
/// | 17130 | 19132 | `Z73` / `Z74` | `E_0526` to a NB, `E_0527` to a MSB, `E_0533` to a LF |
/// | 17123 | 19124 | `Z56` | `E_0523` from a NB, `E_0533` from a MSB |
/// | 17120 | 19121 | `Z30` | `E_0523` |
/// | 17121 | 19120 | `Z68` | `E_0526` |
/// | 17118 | 19127 | `Z31` | `E_0527`, `E_0539` |
/// | 17129 | 19131 | `Z72` | `E_0538` to a NB, `E_0539` to a MSB, `E_0540` to a LF |
/// | 17128 | 19130 | `Z76` | `E_0552` to a NB, `E_0553` to a LF, `E_0554` to a MSB |
///
/// The Ablehnung der Reklamation einer Definition 19123 (17122, `E_0544`–
/// `E_0551`) is not answered here: its column requires `SG1 RFF+ACW` / `RFF+TN`
/// naming the UTILTS that carried the Definition (Kap. 4.5, S. 33–35,
/// `[56]`–`[69]`), which this process does not hold.
#[must_use]
pub fn antwort_auf(
    orders_pid: u32,
    orders_bgm: &str,
) -> Option<(u32, &'static str, &'static [&'static str])> {
    use mako_pruefung::codes as c;
    match orders_pid {
        17_130 => {
            let bgm = match orders_bgm {
                "Z74" => "Z74",
                _ => "Z73",
            };
            Some((
                19_132,
                bgm,
                &[
                    c::EBD_KONFIGURATION_BESTELLUNG,
                    c::EBD_KONFIGURATION_BESTELLUNG_WEITERER_MSB,
                    c::EBD_KONFIGURATION_BESTELLUNG_LF_MSB,
                ],
            ))
        }
        17_123 => Some((
            19_124,
            "Z56",
            &[
                c::EBD_KONFIGURATION_BESTELLUNG_NB,
                c::EBD_KONFIGURATION_BESTELLUNG_LF_MSB,
            ],
        )),
        17_120 => Some((19_121, "Z30", &[c::EBD_KONFIGURATION_BESTELLUNG_NB])),
        17_121 => Some((19_120, "Z68", &[c::EBD_KONFIGURATION_BESTELLUNG])),
        17_118 => Some((
            19_127,
            "Z31",
            &[
                c::EBD_KONFIGURATION_BESTELLUNG_WEITERER_MSB,
                c::EBD_KONFIGURATION_BEENDIGUNG_WEITERER_MSB,
            ],
        )),
        17_129 => Some((
            19_131,
            "Z72",
            &[
                c::EBD_KONFIGURATION_BEENDIGUNG_NB,
                c::EBD_KONFIGURATION_BEENDIGUNG_WEITERER_MSB,
                c::EBD_KONFIGURATION_BEENDIGUNG_LF,
            ],
        )),
        17_128 => Some((
            19_130,
            "Z76",
            &[
                c::EBD_KONFIGURATION_REKLAMATION_NB,
                c::EBD_KONFIGURATION_REKLAMATION_LF,
                c::EBD_KONFIGURATION_REKLAMATION_MSB,
            ],
        )),
        _ => None,
    }
}

/// The key a received ORDERS runs under: the sender's MP-ID and its `BGM` DE
/// 1004. The number is unique only per sender, hence the MP-ID.
#[must_use]
pub fn auftrag_key(sender: &str, dokumentennummer: &str) -> String {
    format!("konfiguration-auftrag:{sender}:{dokumentennummer}")
}

/// The window a received ORDERS `pid` opens for the recipient's answer, when a
/// Festlegung quantifies it here: only the Bestellung Änderung
/// Prognosegrundlage 17120 („spätester ÜT ist der 1. WT nach dem ÜT von Nr.
/// 1", GPKE Teil 3 Kap. 1.3.2.2 Nr. 2) — the same instant the sender's
/// [`ANTWORT_WINDOW_LABEL`] expects.
#[must_use]
pub fn eingang_antwortfrist(
    pid: u32,
    eingang: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    antwortfrist(pid, eingang)
}

/// ORDRSP PIDs from NB back to LF.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 19120 | Mitteilung zur Änderung |
/// | 19121 | Mitteilung Änderung Prognosegrundlage |
/// | 19123 | Ablehnung der Reklamation einer Definition |
/// | 19124 | Mitteilung Änderung Zählzeitdefinition |
/// | 19133 | Bearbeitungsstand Bestellung Abrechnungsdaten |
pub const ORDRSP_NB_LF_PIDS: &[u32] = &[19120, 19121, 19123, 19124, 19133];

/// ORDRSP PID from the weiterer MSB back to the MSB.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 19127 | Mitteilung zur Konfigurationsänderung — answers 17118 (lfd. Nr. 22620) |
pub const ORDRSP_MSB_MSB_PIDS: &[u32] = &[19127];

/// ORDRSP PIDs from MSB back to LF.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 19130 | Bearbeitungsstand Reklamation Konfiguration |
/// | 19131 | Antwort auf Bestellung Beendigung einer Konfiguration |
/// | 19132 | Mitteilung Bestellung Änderung einer Konfiguration |
pub const ORDRSP_MSB_LF_PIDS: &[u32] = &[19130, 19131, 19132];

/// All inbound ORDRSP PIDs for registration routing.
pub const ORDRSP_PIDS: &[u32] = &[
    19120, 19121, 19123, 19124, 19127, 19130, 19131, 19132, 19133,
];

/// IFTSTA PIDs for GPKE Teil 3 Konfigurationsänderung status messages.
///
/// These IFTSTA messages are informational confirmations and status updates
/// sent by NB or MSB back to LF after a ORDERS Konfigurationsänderung.
///
/// | PID   | Description | Direction |
/// |-------|-------------|-----------|
/// | 21043 | Bestellungsantwort / -mitteilung (GPKE Teil 3) | NB → LF · MSB → LF |
/// | 21044 | Bestellungsbeendigung (GPKE Teil 3) | MSB → NB · MSB → LF |
///
/// Source: `site/content/docs/regulatory/pid-reference.md` (generated from BDEW xlsx PID 3.3 + PID 4.0).
/// PID 21042 (WiM / Umsetzungsstatus, „Bestellung (WiM)", MSB → ESA; IFTSTA AHB
/// 2.0g Kap. 6.10) is a WiM Strom Teil 2 message handled by `mako-wim`
/// (`esa-wertebestellung`), not GPKE — it is not registered here.
pub const IFTSTA_PIDS: &[u32] = &[21_043, 21_044];

/// Deadline label for the NB/MSB response window on an outbound ORDERS.
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-konfiguration-aenderung-antwort";

/// Deadline label for the recipient's own answer window on a received ORDERS
/// ([`eingang_antwortfrist`]).
pub const EINGANG_WINDOW_LABEL: &str = "gpke-konfiguration-aenderung-eingang";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Konfigurationsänderung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KonfigurationAenderungEvent {
    /// LF sent an ORDERS configuration change request.
    AnfrageGesendet {
        /// ORDERS Prüfidentifikator (one of ORDERS_ANFRAGE_PIDS).
        orders_pid: Pruefidentifikator,
        /// GLN of the recipient (NB or MSB).
        recipient: MarktpartnerCode,
        /// EIC/MaLo of the affected supply location.
        malo: MaLo,
        /// Message reference of the sent ORDERS.
        message_ref: MessageRef,
    },
    /// Receiving side: an ORDERS arrived; the recipient owes its answer.
    AuftragEingegangen {
        /// ORDERS Prüfidentifikator.
        pid: Pruefidentifikator,
        /// The ORDERS as received.
        auftrag: EingangAuftrag,
    },
    /// Receiving side: the ORDERS failed its AHB check.
    AuftragUngueltig {
        /// The findings.
        reason: String,
    },
    /// Receiving side: the recipient answered with its ORDRSP.
    AntwortGesendet {
        /// The ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// The code.
        antwortcode: String,
        /// The tree it was read from.
        codeliste: String,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// NB or MSB confirmed / acknowledged the change request (positive ORDRSP).
    AntwortErhalten {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// `true` = positive response (Bestätigung/Mitteilung); `false` = rejection.
        accepted: bool,
        /// Optional plain-text reason (present on rejections).
        reason: Option<String>,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Intermediate status received (e.g. 19133 Bearbeitungsstand).
    ZwischenstandErhalten {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// IFTSTA Bestellungsantwort/-beendigung (21043/21044) recorded for audit.
    IftstaErhalten {
        /// IFTSTA Prüfidentifikator (21043/21044).
        pid: Pruefidentifikator,
        /// Message reference of the inbound IFTSTA.
        message_ref: MessageRef,
    },
    /// A registered deadline expired before the ORDRSP arrived.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for KonfigurationAenderungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "KonfigAenderungAnfrageGesendet",
            Self::AuftragEingegangen { .. } => "KonfigAenderungAuftragEingegangen",
            Self::AuftragUngueltig { .. } => "KonfigAenderungAuftragUngueltig",
            Self::AntwortGesendet { .. } => "KonfigAenderungAntwortGesendet",
            Self::IftstaErhalten { .. } => "KonfigAenderungIftstaErhalten",
            Self::AntwortErhalten { .. } => "KonfigAenderungAntwortErhalten",
            Self::ZwischenstandErhalten { .. } => "KonfigAenderungZwischenstandErhalten",
            Self::DeadlineExpired { .. } => "KonfigAenderungDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnfrageGesendet {
                orders_pid,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(orders_pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AntwortErhalten {
                ordrsp_pid,
                message_ref,
                ..
            }
            | Self::ZwischenstandErhalten {
                ordrsp_pid,
                message_ref,
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(ordrsp_pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::IftstaErhalten { pid, message_ref } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AuftragEingegangen { pid, auftrag } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pid.as_u32()),
                message_ref: Some(auftrag.message_ref.as_str().to_owned()),
            }),
            Self::AntwortGesendet {
                ordrsp_pid,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(ordrsp_pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::DeadlineExpired { .. } | Self::AuftragUngueltig { .. } => None,
        }
    }

    fn antwort(&self) -> Option<mako_engine::envelope::Antwortcode> {
        match self {
            Self::AntwortGesendet {
                antwortcode,
                codeliste,
                ..
            } => Some(mako_engine::envelope::Antwortcode {
                code: antwortcode.clone(),
                codeliste: Some(codeliste.clone()),
            }),
            _ => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Data captured when an ORDERS request is sent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnfrageData {
    /// ORDERS Prüfidentifikator sent.
    pub orders_pid: Pruefidentifikator,
    /// Recipient (NB or MSB) GLN.
    pub recipient: MarktpartnerCode,
    /// Affected MaLo.
    pub malo: MaLo,
    /// Message reference of the sent ORDERS.
    pub message_ref: MessageRef,
}

/// An ORDERS as its recipient received it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EingangAuftrag {
    /// The sender — `NAD+MS`.
    pub absender: MarktpartnerCode,
    /// The recipient — `NAD+MR`.
    pub empfaenger: MarktpartnerCode,
    /// The Lokation — the first `LOC` DE 3225, when the ORDERS names one.
    pub lokation: Option<String>,
    /// `BGM` DE 1004, echoed in the answer's `SG1 RFF+ON`.
    pub dokumentennummer: String,
    /// `BGM` DE 1001.
    pub bgm: String,
    /// Interchange reference of the inbound ORDERS.
    pub message_ref: MessageRef,
}

/// Current state of a GPKE Konfigurationsänderung process stream.
///
/// ```text
/// New → AnfrageGesendet → Beantwortet
///                      ↘ DeadlineExpired
/// New → AuftragEingegangen          (receiving side; the answer is owed)
///     ↘ Ungueltig
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum KonfigurationAenderungState {
    /// No ORDERS sent yet (initial state).
    #[default]
    New,
    /// LF sent ORDERS; waiting for ORDRSP or status update.
    AnfrageGesendet(AnfrageData),
    /// Receiving side: an ORDERS arrived and its answer is owed.
    AuftragEingegangen {
        /// ORDERS Prüfidentifikator.
        pid: Pruefidentifikator,
        /// The ORDERS as received.
        auftrag: EingangAuftrag,
    },
    /// Receiving side: the ORDERS failed its AHB check.
    Ungueltig {
        /// The findings.
        reason: String,
    },
    /// Receiving side: the recipient answered.
    AuftragBeantwortet {
        /// The ORDERS Prüfidentifikator.
        pid: Pruefidentifikator,
        /// The ORDERS as received.
        auftrag: EingangAuftrag,
        /// The ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// The code.
        antwortcode: String,
    },
    /// Final response received (ORDRSP accept or reject).
    Beantwortet {
        /// Original request data.
        anfrage: AnfrageData,
        /// ORDRSP PID received.
        ordrsp_pid: Pruefidentifikator,
        /// Whether the response was positive.
        accepted: bool,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// No Festlegung states a consequence of the Konfigurationsänderung going unanswered. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<KonfigurationAenderungState>,
    },
}

impl KonfigurationAenderungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageGesendet(_) => "AnfrageGesendet",
            Self::AuftragEingegangen { .. } => "AuftragEingegangen",
            Self::Ungueltig { .. } => "Ungueltig",
            Self::AuftragBeantwortet { .. } => "AuftragBeantwortet",
            Self::Beantwortet { .. } => "Beantwortet",
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

    /// Whether the process has reached a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Beantwortet { .. } | Self::Ungueltig { .. } | Self::AuftragBeantwortet { .. }
        )
    }
}

impl mako_engine::workflow::OccupiesBusinessKey for KonfigurationAenderungState {
    /// A received ORDERS holds its key while its answer is owed; the key names
    /// one ORDERS, so a repeat of the same number is refused, never a new one.
    fn occupies_business_key(&self) -> bool {
        matches!(self.offen(), Self::AuftragEingegangen { .. })
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Konfigurationsänderung workflow.
#[derive(Clone)]
pub enum KonfigurationAenderungCommand {
    /// LF initiates a configuration change request.
    ///
    /// Creates the outbound ORDERS message via the outbox.
    SendAnfrage {
        /// ORDERS Prüfidentifikator (one of ORDERS_ANFRAGE_PIDS).
        orders_pid: Pruefidentifikator,
        /// GLN of the recipient (NB or MSB).
        recipient: MarktpartnerCode,
        /// EIC/MaLo of the affected supply location.
        malo: MaLo,
        /// Message reference of the outbound ORDERS.
        message_ref: MessageRef,
        /// Optional structured payload for the ORDERS body.
        payload: serde_json::Value,
        /// When the ORDERS leaves — the Übertragungstag the counterparty's
        /// Antwortfrist counts from ([`mako_fristen::antwort::antwort_deadline`]).
        gesendet_am: time::OffsetDateTime,
    },
    /// Receiving side: an inbound ORDERS.
    ReceiveAuftrag {
        /// ORDERS Prüfidentifikator read from `SG1 RFF+Z13`.
        pid: Pruefidentifikator,
        /// The ORDERS as received.
        auftrag: EingangAuftrag,
        /// `true` when the AHB check passed.
        validation_passed: bool,
        /// The findings when it did not.
        validation_errors: Vec<String>,
    },
    /// Receiving side: answer the received ORDERS with its ORDRSP.
    SendAntwort {
        /// The tree the code is read from — one [`antwort_auf`] admits.
        codeliste: String,
        /// The code.
        antwortcode: String,
        /// `SG2 FTX+AAP` — Muss beside `A99`.
        freitext: Option<String>,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP received from NB or MSB.
    ReceiveOrdrsp {
        /// ORDRSP Prüfidentifikator (one of ORDRSP_PIDS).
        ordrsp_pid: Pruefidentifikator,
        /// `true` = positive; `false` = rejection or unresolved status.
        accepted: bool,
        /// Optional rejection reason.
        reason: Option<String>,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Inbound IFTSTA Bestellungsantwort/-beendigung (PIDs 21043/21044) from the
    /// NB or MSB. Informational status on the configuration order; recorded for
    /// audit without changing the lifecycle outcome.
    ReceiveIftsta {
        /// IFTSTA Prüfidentifikator (21043 = Bestellungsantwort/-mitteilung,
        /// 21044 = Bestellungsbeendigung).
        pid: Pruefidentifikator,
        /// Message reference of the inbound IFTSTA.
        message_ref: MessageRef,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for KonfigurationAenderungCommand {}

/// The counterparty's Antwortfrist on an outbound Konfigurationsänderung ORDERS.
///
/// Only the Bestellung Änderung Prognosegrundlage (17120) is resolved: it is
/// ordered through the Use-Case „Bestellung einer Konfiguration vom LF an NB"
/// (GPKE Teil 3 Kap. 1.1.2), whose Mitteilung is due „spätester ÜT ist
/// der 1. WT nach dem ÜT von Nr. 1" (Kap. 1.3.2.2 Nr. 2,
/// [`mako_fristen::antwort::KONFIGURATION_MITTEILUNG_WERKTAGE`]). The other
/// Bestellungen and Reklamationen of Teil 3 carry Fristen of their own that
/// this workflow does not resolve, and register no window.
fn antwortfrist(
    orders_pid: u32,
    gesendet_am: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    (orders_pid == 17_120).then(|| {
        mako_fristen::antwort::FristShape::EndOfWerktag(
            mako_fristen::antwort::KONFIGURATION_MITTEILUNG_WERKTAGE,
        )
        .due_at(gesendet_am, mako_fristen::HolidayCalendar::BdewMaKo)
    })
}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Konfigurationsänderung workflow — LF-initiated config change requests
/// (GPKE Teil 3, BK6-24-174 Anlage 1c).
pub struct GpkeKonfigurationAenderungWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while the Anfrage waits for its ORDRSP
/// (`AnfrageGesendet`).
fn window_open(label: &str, state: &KonfigurationAenderungState) -> bool {
    match label {
        ANTWORT_WINDOW_LABEL => matches!(state, KonfigurationAenderungState::AnfrageGesendet(_)),
        EINGANG_WINDOW_LABEL => {
            matches!(
                state,
                KonfigurationAenderungState::AuftragEingegangen { .. }
            )
        }
        _ => false,
    }
}

/// Rejection ORDRSP PIDs (NB/MSB rejects LF's request).
const REJECTION_PIDS: &[u32] = &[19123, 19130];

/// PIDs that represent intermediate status updates (not final responses).
const ZWISCHENSTAND_PIDS: &[u32] = &[19133];

impl Workflow for GpkeKonfigurationAenderungWorkflow {
    type State = KonfigurationAenderungState;
    type Event = KonfigurationAenderungEvent;
    type Command = KonfigurationAenderungCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL, EINGANG_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| {
            KonfigurationAenderungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            KonfigurationAenderungState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            KonfigurationAenderungEvent::AnfrageGesendet {
                orders_pid,
                recipient,
                malo,
                message_ref,
            } => KonfigurationAenderungState::AnfrageGesendet(AnfrageData {
                orders_pid: *orders_pid,
                recipient: recipient.clone(),
                malo: malo.clone(),
                message_ref: message_ref.clone(),
            }),
            KonfigurationAenderungEvent::AuftragEingegangen { pid, auftrag } => {
                KonfigurationAenderungState::AuftragEingegangen {
                    pid: *pid,
                    auftrag: auftrag.clone(),
                }
            }
            KonfigurationAenderungEvent::AuftragUngueltig { reason } => {
                KonfigurationAenderungState::Ungueltig {
                    reason: reason.clone(),
                }
            }
            KonfigurationAenderungEvent::AntwortGesendet {
                ordrsp_pid,
                antwortcode,
                ..
            } => match state {
                KonfigurationAenderungState::AuftragEingegangen { pid, auftrag } => {
                    KonfigurationAenderungState::AuftragBeantwortet {
                        pid,
                        auftrag,
                        ordrsp_pid: *ordrsp_pid,
                        antwortcode: antwortcode.clone(),
                    }
                }
                other => other,
            },
            KonfigurationAenderungEvent::ZwischenstandErhalten { .. } => state, // no-op, informational
            KonfigurationAenderungEvent::IftstaErhalten { .. } => state, // no-op, informational
            KonfigurationAenderungEvent::AntwortErhalten {
                ordrsp_pid,
                accepted,
                ..
            } => match state {
                KonfigurationAenderungState::AnfrageGesendet(anfrage) => {
                    KonfigurationAenderungState::Beantwortet {
                        anfrage,
                        ordrsp_pid: *ordrsp_pid,
                        accepted: *accepted,
                    }
                }
                other => other,
            },
            KonfigurationAenderungEvent::DeadlineExpired { label, .. } => match state {
                KonfigurationAenderungState::AnfrageGesendet(_)
                | KonfigurationAenderungState::AuftragEingegangen { .. } => {
                    KonfigurationAenderungState::FristAbgelaufen {
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
            KonfigurationAenderungCommand::SendAnfrage {
                orders_pid,
                recipient,
                malo,
                message_ref,
                payload,
                gesendet_am,
            } => {
                if !matches!(state, KonfigurationAenderungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_ANFRAGE_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid GPKE Konfigurationsänderung ORDERS PID: {orders_pid}",
                    )));
                }
                let event = KonfigurationAenderungEvent::AnfrageGesendet {
                    orders_pid,
                    recipient: recipient.clone(),
                    malo: malo.clone(),
                    message_ref: message_ref.clone(),
                };
                // The column's own fields come in `payload`, laid over the
                // head at the top, where the renderer reads them.
                let body = crate::outbound::mit_spalte(
                    &mako_engine::payload::OrdersPayload {
                        pid: Some(orders_pid.as_u32()),
                        location: Some(malo.as_str().to_owned()),
                        message_ref: Some(message_ref.as_str().to_owned()),
                        ..Default::default()
                    },
                    &payload,
                )?;
                let outbox = vec![PendingOutbox::typed(recipient.as_str(), &body)];
                // The counterparty's Antwortfrist, from the published row for
                // this ORDERS; a PID no Festlegung quantifies registers nothing.
                let frist: Vec<PendingDeadline> = antwortfrist(orders_pid.as_u32(), gesendet_am)
                    .map(|due| PendingDeadline::new(ANTWORT_WINDOW_LABEL, due))
                    .into_iter()
                    .collect();
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    vec![event],
                    outbox,
                    frist,
                ))
            }

            KonfigurationAenderungCommand::ReceiveAuftrag {
                pid,
                auftrag,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, KonfigurationAenderungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ist_auftrag(pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not an ORDERS of the GPKE Teil 3 Konfiguration: {pid}",
                    )));
                }
                let mut events =
                    vec![KonfigurationAenderungEvent::AuftragEingegangen { pid, auftrag }];
                if !validation_passed {
                    events.push(KonfigurationAenderungEvent::AuftragUngueltig {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            KonfigurationAenderungCommand::SendAntwort {
                codeliste,
                antwortcode,
                freitext,
                message_ref,
            } => {
                let KonfigurationAenderungState::AuftragEingegangen { pid, auftrag } = state else {
                    return Err(WorkflowError::invalid_state(
                        "AuftragEingegangen",
                        state.label(),
                    ));
                };
                if pid.as_u32() == 17_122 {
                    return Err(WorkflowError::validation(
                        "the Ablehnung der Reklamation einer Definition 19123 names the UTILTS \
                         that carried the Definition in SG1 RFF+ACW / RFF+TN (ORDRSP AHB 1.1b \
                         Kap. 4.5, [56]–[69]); this process does not hold it",
                    ));
                }
                let (ordrsp, bgm, baeume) =
                    antwort_auf(pid.as_u32(), &auftrag.bgm).ok_or_else(|| {
                        WorkflowError::validation(format!(
                            "ORDERS {pid} is not answered by an ORDRSP of this process"
                        ))
                    })?;
                if !baeume.contains(&codeliste.as_str()) {
                    return Err(WorkflowError::validation(format!(
                        "{codeliste:?} does not answer ORDERS {pid} (admitted: {baeume:?})"
                    )));
                }
                let code =
                    mako_pruefung::codes::lookup(&codeliste, &antwortcode).ok_or_else(|| {
                        WorkflowError::validation(format!(
                            "{antwortcode:?} is not published in {codeliste}"
                        ))
                    })?;
                // `A06` „konnte nicht sofort korrigiert werden" carries the
                // geplanter Behebungszeitpunkt `DTM+292` (ORDRSP AHB 1.1b Kap.
                // 4.7, S. 38, `[46]`–`[48]`), which the answer does not carry.
                if ordrsp == 19_130 && antwortcode == "A06" {
                    return Err(WorkflowError::validation(
                        "A06 on 19130 requires the geplanter Behebungszeitpunkt DTM+292 \
                         (ORDRSP AHB 1.1b Kap. 4.7, [46]–[48]), which this answer does not carry",
                    ));
                }
                if code.braucht_bemerkung && freitext.as_deref().is_none_or(|t| t.trim().is_empty())
                {
                    return Err(WorkflowError::validation(format!(
                        "{antwortcode:?} of {codeliste} requires the problem named in SG2 \
                         FTX+AAP ([28])"
                    )));
                }
                let ordrsp_pid = Pruefidentifikator::new(ordrsp)
                    .map_err(|e| WorkflowError::validation(format!("answer PID {ordrsp}: {e}")))?;
                let outbox = vec![
                    PendingOutbox::typed(
                        auftrag.absender.as_str(),
                        &mako_engine::payload::OrdrspPayload {
                            pid: Some(ordrsp),
                            sender: Some(auftrag.empfaenger.as_str().to_owned()),
                            receiver: Some(auftrag.absender.as_str().to_owned()),
                            message_ref: Some(message_ref.as_str().to_owned()),
                            document_code: Some(bgm.to_string()),
                            document_id: Some(message_ref.as_str().to_owned()),
                            order_reference: Some(auftrag.dokumentennummer.clone()),
                            antwort_code: Some(antwortcode.clone()),
                            antwort_codeliste: Some(codeliste.clone()),
                            antwort_freitext: freitext,
                            ..Default::default()
                        },
                    )
                    .caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![KonfigurationAenderungEvent::AntwortGesendet {
                        ordrsp_pid,
                        antwortcode,
                        codeliste,
                        message_ref,
                    }],
                    outbox,
                ))
            }

            KonfigurationAenderungCommand::ReceiveIftsta { pid, message_ref } => {
                // IFTSTA Bestellungsantwort/-beendigung (21043/21044) is
                // informational — accept in any state and record for audit.
                Ok(vec![KonfigurationAenderungEvent::IftstaErhalten { pid, message_ref }].into())
            }

            KonfigurationAenderungCommand::ReceiveOrdrsp {
                ordrsp_pid,
                accepted,
                reason,
                message_ref,
            } => {
                // Accept ORDRSP in AnfrageGesendet or (defensively) in terminal states
                if state.is_terminal() {
                    // Late-arriving ORDRSP after deadline — record as informational
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !matches!(state, KonfigurationAenderungState::AnfrageGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageGesendet",
                        state.label(),
                    ));
                }
                if !ORDRSP_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid GPKE Konfigurationsänderung ORDRSP PID: {ordrsp_pid}",
                    )));
                }
                // Intermediate status updates (e.g. 19133 Bearbeitungsstand)
                // don't close the process — they're informational.
                if ZWISCHENSTAND_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Ok(vec![KonfigurationAenderungEvent::ZwischenstandErhalten {
                        ordrsp_pid,
                        message_ref,
                    }]
                    .into());
                }
                // Determine acceptance: rejection PIDs are always negative.
                let is_accepted = accepted && !REJECTION_PIDS.contains(&ordrsp_pid.as_u32());
                Ok(vec![KonfigurationAenderungEvent::AntwortErhalten {
                    ordrsp_pid,
                    accepted: is_accepted,
                    reason,
                    message_ref,
                }]
                .into())
            }

            KonfigurationAenderungCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(
                    vec![KonfigurationAenderungEvent::DeadlineExpired { deadline_id, label }]
                        .into(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ANTWORT_WINDOW_LABEL, GpkeKonfigurationAenderungWorkflow, KonfigurationAenderungCommand,
        KonfigurationAenderungState, MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator,
        Workflow as _,
    };
    use super::{
        ORDERS_ANFRAGE_MSB_PIDS, ORDERS_ANFRAGE_NB_PIDS, ORDERS_ANFRAGE_PIDS,
        ORDERS_EMPFANG_LF_PIDS, ORDERS_EMPFANG_MSB_PIDS, ORDERS_EMPFANG_NB_PIDS,
        ORDERS_MSB_MSB_PIDS, ORDERS_NB_MSB_PIDS, ORDRSP_MSB_LF_PIDS, ORDRSP_MSB_MSB_PIDS,
        ORDRSP_NB_LF_PIDS, ORDRSP_PIDS,
    };

    /// A Bestellung Änderung Prognosegrundlage starts the NB's 1-WT Mitteilung
    /// window (GPKE Teil 3 Kap. 1.3.2.2 Nr. 2); a Reklamation einer Definition
    /// starts none here.
    #[test]
    fn a_17120_registers_the_mitteilung_window() {
        let sent = time::macros::datetime!(2026-10-07 10:00 UTC);
        let anfrage = |pid: u32| KonfigurationAenderungCommand::SendAnfrage {
            orders_pid: Pruefidentifikator::new(pid).expect("valid PID"),
            recipient: MarktpartnerCode::new("9900000000003"),
            malo: MaLo::new("51238696012"),
            message_ref: MessageRef::new("ORDERS-1"),
            payload: serde_json::json!({}),
            gesendet_am: sent,
        };
        let out = GpkeKonfigurationAenderungWorkflow::handle(
            &KonfigurationAenderungState::default(),
            anfrage(17_120),
        )
        .expect("accepted");
        assert_eq!(out.deadlines.len(), 1);
        assert_eq!(out.deadlines[0].label, ANTWORT_WINDOW_LABEL);
        assert_eq!(
            out.deadlines[0].due_at,
            mako_fristen::end_of_day_berlin(time::macros::date!(2026 - 10 - 08))
        );
        let out = GpkeKonfigurationAenderungWorkflow::handle(
            &KonfigurationAenderungState::default(),
            anfrage(17_122),
        )
        .expect("accepted");
        assert!(out.deadlines.is_empty());
    }

    /// The registration lists are the union of the per-direction ones.
    ///
    /// Two statements of the same PID set: the per-direction constants say who
    /// sends what to whom, and the flat ones are what
    /// `register_pids_with_roles` iterates. Nothing in the type system connects
    /// them, so a PID added to one is silently absent from the other — added to
    /// a direction list only, it never routes and every message carrying it is
    /// dead-lettered as `UnknownPid`; added to the flat list only, it routes
    /// while no direction claims it.
    fn union(parts: &[&[u32]]) -> Vec<u32> {
        let mut all: Vec<u32> = parts.iter().flat_map(|p| p.iter().copied()).collect();
        all.sort_unstable();
        all.dedup();
        all
    }

    fn sorted(pids: &[u32]) -> Vec<u32> {
        let mut v = pids.to_vec();
        v.sort_unstable();
        v
    }

    #[test]
    fn the_orders_registration_list_is_the_union_of_its_directions() {
        assert_eq!(
            sorted(ORDERS_ANFRAGE_PIDS),
            union(&[
                ORDERS_ANFRAGE_NB_PIDS,
                ORDERS_ANFRAGE_MSB_PIDS,
                ORDERS_NB_MSB_PIDS,
                ORDERS_MSB_MSB_PIDS,
            ]),
        );
    }

    /// Every ORDERS some role receives, and every role's set inside the
    /// workflow's.
    #[test]
    fn the_receiving_sets_cover_every_orders() {
        assert_eq!(
            sorted(ORDERS_ANFRAGE_PIDS),
            union(&[
                ORDERS_EMPFANG_NB_PIDS,
                ORDERS_EMPFANG_MSB_PIDS,
                ORDERS_EMPFANG_LF_PIDS,
            ]),
        );
    }

    #[test]
    fn the_ordrsp_registration_list_is_the_union_of_its_directions() {
        assert_eq!(
            sorted(ORDRSP_PIDS),
            union(&[ORDRSP_NB_LF_PIDS, ORDRSP_MSB_LF_PIDS, ORDRSP_MSB_MSB_PIDS]),
        );
    }

    /// A Prüfidentifikator belongs to exactly one direction.
    #[test]
    fn no_pid_is_claimed_by_two_directions() {
        for lists in [
            [
                ORDERS_ANFRAGE_NB_PIDS,
                ORDERS_ANFRAGE_MSB_PIDS,
                ORDERS_NB_MSB_PIDS,
                ORDERS_MSB_MSB_PIDS,
            ]
            .as_slice(),
            [ORDRSP_NB_LF_PIDS, ORDRSP_MSB_LF_PIDS, ORDRSP_MSB_MSB_PIDS].as_slice(),
        ] {
            let flat: Vec<u32> = lists.iter().flat_map(|p| p.iter().copied()).collect();
            let mut unique = flat.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(
                flat.len(),
                unique.len(),
                "a PID appears in two direction lists: {flat:?}"
            );
        }
    }

    #[test]
    fn verlauf_states_orders_out_and_ordrsp_and_iftsta_in() {
        use super::{KonfigurationAenderungEvent, Richtung, Verlauf};
        use mako_engine::{ids::DeadlineId, workflow::EventPayload as _};

        let sent = time::macros::datetime!(2026-10-07 10:00 UTC);
        let out = GpkeKonfigurationAenderungWorkflow::handle(
            &KonfigurationAenderungState::default(),
            KonfigurationAenderungCommand::SendAnfrage {
                orders_pid: Pruefidentifikator::new(17_120).expect("valid PID"),
                recipient: MarktpartnerCode::new("9900000000003"),
                malo: MaLo::new("51238696012"),
                message_ref: MessageRef::new("ORDERS-1"),
                payload: serde_json::json!({}),
                gesendet_am: sent,
            },
        )
        .expect("accepted");
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(17_120),
                message_ref: Some("ORDERS-1".to_owned()),
            })
        );
        let iftsta = KonfigurationAenderungEvent::IftstaErhalten {
            pid: Pruefidentifikator::new(21_043).expect("valid PID"),
            message_ref: MessageRef::new("IFTSTA-1"),
        };
        assert_eq!(
            iftsta.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(21_043),
                message_ref: Some("IFTSTA-1".to_owned()),
            })
        );
        let expired = KonfigurationAenderungEvent::DeadlineExpired {
            deadline_id: DeadlineId::new(),
            label: ANTWORT_WINDOW_LABEL.into(),
        };
        assert_eq!(expired.verlauf(), None);
    }

    /// A received ORDERS opens the recipient's process: 17118 (MSB an
    /// weiterer MSB) is one the workflow receives, and a 17120 opens the NB's
    /// 1-WT window (GPKE Teil 3 Kap. 1.3.2.2 Nr. 2), which a lapse leaves owed.
    #[test]
    fn a_received_orders_opens_the_recipients_process() {
        use super::{EINGANG_WINDOW_LABEL, EingangAuftrag, eingang_antwortfrist};
        use mako_engine::ids::DeadlineId;
        let eingang = time::macros::datetime!(2026-10-07 10:00 UTC);
        assert_eq!(
            eingang_antwortfrist(17_120, eingang),
            Some(mako_fristen::end_of_day_berlin(time::macros::date!(
                2026 - 10 - 08
            )))
        );
        assert_eq!(eingang_antwortfrist(17_118, eingang), None);
        for pid in [
            17_118_u32, 17_120, 17_121, 17_122, 17_123, 17_128, 17_133, 17_134, 17_135,
        ] {
            let out = GpkeKonfigurationAenderungWorkflow::handle(
                &KonfigurationAenderungState::default(),
                KonfigurationAenderungCommand::ReceiveAuftrag {
                    pid: Pruefidentifikator::new(pid).expect("valid PID"),
                    auftrag: EingangAuftrag {
                        absender: MarktpartnerCode::new("9900000000003"),
                        empfaenger: MarktpartnerCode::new("9900555000005"),
                        lokation: Some("DE00056266802AO6G56M11SN51G21M24S".to_owned()),
                        dokumentennummer: "KONF-1".to_owned(),
                        bgm: "Z73".to_owned(),
                        message_ref: MessageRef::new("UNH-1"),
                    },
                    validation_passed: true,
                    validation_errors: vec![],
                },
            )
            .unwrap_or_else(|e| panic!("{pid}: {e}"));
            let offen = out.events.iter().fold(
                KonfigurationAenderungState::default(),
                GpkeKonfigurationAenderungWorkflow::apply,
            );
            assert!(
                matches!(
                    offen,
                    KonfigurationAenderungState::AuftragEingegangen { .. }
                ),
                "{pid}: {offen:?}"
            );
            let fired = GpkeKonfigurationAenderungWorkflow::handle(
                &offen,
                KonfigurationAenderungCommand::TimeoutExpired {
                    deadline_id: DeadlineId::new(),
                    label: EINGANG_WINDOW_LABEL.into(),
                },
            )
            .expect("accepted");
            let lapsed = fired
                .events
                .iter()
                .fold(offen, GpkeKonfigurationAenderungWorkflow::apply);
            assert!(
                matches!(lapsed, KonfigurationAenderungState::FristAbgelaufen { .. }),
                "{pid}: {lapsed:?}"
            );
            assert!(!lapsed.is_terminal());
        }
    }

    /// A received 17130 is answered on 19132 out of `E_0526` / `E_0527`; a
    /// tree the column does not name, and a PID whose column names only
    /// uncatalogued trees, are refused.
    #[test]
    fn a_received_orders_is_answered_on_its_ordrsp() {
        use super::EingangAuftrag;
        let offen = |pid: u32| KonfigurationAenderungState::AuftragEingegangen {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            auftrag: EingangAuftrag {
                absender: MarktpartnerCode::new("9900000000003"),
                empfaenger: MarktpartnerCode::new("9900555000005"),
                lokation: None,
                dokumentennummer: "KONF-1".to_owned(),
                bgm: "Z73".to_owned(),
                message_ref: MessageRef::new("UNH-1"),
            },
        };
        let antwort = |tree: &str, code: &str| KonfigurationAenderungCommand::SendAntwort {
            codeliste: tree.to_owned(),
            antwortcode: code.to_owned(),
            freitext: None,
            message_ref: MessageRef::new("ORDRSP-1"),
        };
        let out =
            GpkeKonfigurationAenderungWorkflow::handle(&offen(17_130), antwort("E_0526", "A07"))
                .expect("answered");
        let p = &out.outbox[0].payload;
        assert_eq!(p["pid"], 19_132);
        assert_eq!(p["document_code"], "Z73");
        assert_eq!(p["order_reference"], "KONF-1");
        let ende = out
            .events
            .iter()
            .fold(offen(17_130), GpkeKonfigurationAenderungWorkflow::apply);
        assert!(ende.is_terminal());
        assert!(
            GpkeKonfigurationAenderungWorkflow::handle(&offen(17_130), antwort("E_0539", "A03"))
                .is_err()
        );
        assert!(
            GpkeKonfigurationAenderungWorkflow::handle(&offen(17_118), antwort("E_0527", "A99"))
                .is_err(),
            "A99 names the problem"
        );
        assert!(
            GpkeKonfigurationAenderungWorkflow::handle(&offen(17_120), antwort("E_0526", "A07"))
                .is_err(),
            "19121's column names E_0523 alone"
        );
    }

    /// Each column the AHB pairs with a received ORDERS answers out of the
    /// trees it names (ORDRSP AHB 1.1b Kap. 4.4.3, 4.4.6, 4.6, 4.7).
    #[test]
    fn every_column_answers_out_of_its_own_trees() {
        use super::EingangAuftrag;
        let offen = |pid: u32| KonfigurationAenderungState::AuftragEingegangen {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            auftrag: EingangAuftrag {
                absender: MarktpartnerCode::new("9900000000003"),
                empfaenger: MarktpartnerCode::new("9900555000005"),
                lokation: None,
                dokumentennummer: "KONF-2".to_owned(),
                bgm: "Z73".to_owned(),
                message_ref: MessageRef::new("UNH-2"),
            },
        };
        let antwort = |tree: &str, code: &str, freitext: Option<&str>| {
            KonfigurationAenderungCommand::SendAntwort {
                codeliste: tree.to_owned(),
                antwortcode: code.to_owned(),
                freitext: freitext.map(str::to_owned),
                message_ref: MessageRef::new("ORDRSP-2"),
            }
        };
        for (orders, tree, code, ordrsp, bgm) in [
            (17_120, "E_0523", "A17", 19_121, "Z30"),
            (17_123, "E_0523", "A12", 19_124, "Z56"),
            (17_123, "E_0533", "A07", 19_124, "Z56"),
            (17_130, "E_0533", "A07", 19_132, "Z73"),
            (17_129, "E_0538", "A03", 19_131, "Z72"),
            (17_129, "E_0540", "A04", 19_131, "Z72"),
            (17_128, "E_0552", "A05", 19_130, "Z76"),
            (17_128, "E_0553", "A02", 19_130, "Z76"),
            (17_128, "E_0554", "A03", 19_130, "Z76"),
        ] {
            let out = GpkeKonfigurationAenderungWorkflow::handle(
                &offen(orders),
                antwort(tree, code, None),
            )
            .unwrap_or_else(|e| panic!("{orders} {tree} {code}: {e}"));
            let p = &out.outbox[0].payload;
            assert_eq!(p["pid"], ordrsp, "{orders}");
            assert_eq!(p["document_code"], bgm, "{orders}");
            assert_eq!(p["antwort_codeliste"], tree, "{orders}");
            assert_eq!(p["order_reference"], "KONF-2", "{orders}");
        }
        assert!(
            GpkeKonfigurationAenderungWorkflow::handle(
                &offen(17_123),
                antwort("E_0526", "A07", None)
            )
            .is_err(),
            "19124 names E_0523 / E_0533 only"
        );
        assert!(
            GpkeKonfigurationAenderungWorkflow::handle(
                &offen(17_128),
                antwort("E_0552", "A04", None)
            )
            .is_err(),
            "A04 names the problem in FTX+AAP"
        );
        assert!(
            GpkeKonfigurationAenderungWorkflow::handle(
                &offen(17_128),
                antwort("E_0552", "A04", Some("Zähler defekt"))
            )
            .is_ok()
        );
        assert!(
            GpkeKonfigurationAenderungWorkflow::handle(
                &offen(17_128),
                antwort("E_0552", "A06", None)
            )
            .is_err(),
            "A06 needs DTM+292"
        );
        let refused = GpkeKonfigurationAenderungWorkflow::handle(
            &offen(17_122),
            antwort("E_0544", "A01", None),
        )
        .expect_err("19123 is refused by name");
        assert!(refused.to_string().contains("RFF+ACW"), "{refused}");
    }
}
