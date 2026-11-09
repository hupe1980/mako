//! GPKE Geschäftsdatenanfrage — Anfrage von Stammdaten Strom (ORDERS 17132),
//! answered by UTILMD or refused by ORDRSP 19101.
//!
//! GPKE Teil 4 (Anlage 1d zu BK6-22-024) Kap. 3: a LF or MSB asks the NB for
//! the Stammdaten of a Markt- or Messlokation. The NB checks the Anfrage and
//! either transmits the data or refuses.
//!
//! ```text
//! LF / MSB ──── ORDERS 17132 Anfrage Stammdaten (Strom) ────► NB
//!          ◄─── UTILMD 55035 / 55095 (to a LF) · 55060 (to a MSB) ───┤  1 WT
//!          ◄─── ORDRSP 19101 Ablehnung der Anfrage Stammdaten ───────┘
//! ```
//!
//! | PID   | Message | Direction | Meaning |
//! |-------|---------|-----------|---------|
//! | 17132 | ORDERS  | LF · MSB → NB | Anfrage Stammdaten (Strom) |
//! | 55035 | UTILMD  | NB → LF   | Antwort auf GDA verb. MaLo |
//! | 55095 | UTILMD  | NB → LF   | Antwort auf GDA erz. MaLo |
//! | 55060 | UTILMD  | NB → MSB  | Antwort auf GDA |
//! | 19101 | ORDRSP  | NB → LF · MSB | Ablehnung der Anfrage Stammdaten (`E_0441` to a LF, `E_0443` to a MSB) |
//!
//! The answer is a **UTILMD**, never an ORDERS: ORDERS AHB 1.1b Kap. 4.1.1
//! (S. 7) — „Die Stammdaten werden mittels UTILMD übertragen. Mittels der
//! ORDRSP ist es möglich, eine Anfrage abzuweisen." The ORDERS PIDs next to
//! 17132 are other Anwendungsfälle with owners of their own (17102 Anfrage
//! Werte, 17103 Brennwert, 17118 Konfigurationsänderung, 17126 Stammdaten
//! Messlokation Gas, …) and never answer this Anfrage.
//!
//! # One workflow, both sides
//!
//! The process holds either side, and which one follows from its first event:
//!
//! - **Anfragender** (LF / MSB): [`GdaCommand::SendAnfrage`] records the 17132
//!   it sent; the NB's UTILMD resumes it ([`GdaCommand::ReceiveAntwort`]) or
//!   the 19101 does ([`GdaCommand::ReceiveAblehnung`]).
//! - **NB**: an inbound 17132 opens it ([`GdaCommand::ReceiveAnfrage`]) and
//!   the 1-WT window; [`GdaCommand::SendAntwort`] or
//!   [`GdaCommand::SendAblehnung`] records the answer.
//!
//! # Correlation
//!
//! Every message of the exchange names the Anfrage by the ORDERS'
//! Dokumentennummer (`BGM` DE 1004): the ORDRSP 19101 in `SG1 RFF+ON`
//! (ORDRSP AHB 1.1b Kap. 4.1.2, Hinweis \[521\] „Wert aus BGM+Z14 … DE1004 der
//! ORDERS"), the UTILMD answer in `SG6 RFF+AAV` „Nummer der Anfrage" (UTILMD
//! AHB Strom 2.2 Kap. 9.5). The UTILMD's first `LOC` is the Netzlokation, so
//! a key read from it would never meet the Anfrage — the process is keyed on
//! the Anfrage-Nummer.
//!
//! The Anfragender chooses that number, so it is unique only among the
//! Anfragen of one sender, and the two sides are keyed apart:
//! [`anfragender_key`] holds the number this party issued,
//! [`netzbetreiber_key`] the number together with the Anfragender's MP-ID.
//! Two Anfragender sending the same number reach two NB processes, and a
//! deployment that is LF and NB at once holds its own sent Anfrage and the
//! received copy of it as two processes — on one key the received 17132 would
//! meet the open `Angefragt` process and be refused.
//!
//! # The outbound legs
//!
//! Each `Send*` command queues the message it records: the ORDERS 17132
//! (`BGM+Z14`, the Anfrage-Nummer in DE 1004, `SG2 LOC+172` the Lokation —
//! ORDERS AHB 1.1b Kap. 4.1.1.1, S. 8), the UTILMD answer (`BGM+E44`,
//! `SG4 STS+7` with the PID's Transaktionsgrund, `SG6 RFF+AAV` the
//! Anfrage-Nummer — UTILMD AHB Strom 2.2,
//! konsolidierte Lesefassung 29.06.2026, Kap. 9.5, S. 599) and the ORDRSP
//! 19101 (`BGM+Z14`, `SG1 RFF+ON` the Anfrage-Nummer, `SG2 AJT` the code with
//! its EBD — ORDRSP AHB 1.1b Kap. 4.1.2). The Stammdaten of the answer are
//! the NB's record of the Lokationsbündel, which the NB's boundary reads
//! from its master data and sets down segment by segment; they travel whole
//! ([`Stammdaten`]) and the AHB profile checks what renders.
//!
//! # Regulatory basis
//!
//! - **GPKE Teil 4 Kap. 3.2** (Anlage 1d zu BK6-22-024, S. 30) — Nr. 2 Antwort
//!   „Unverzüglich, jedoch spätester ÜZ ist 1 WT nach dem ÜZ von Nr. 1", held
//!   by [`mako_fristen::antwort::GESCHAEFTSDATENANFRAGE_WERKTAGE`].
//! - **Anwendungsübersicht der Prüfidentifikatoren 4.0** lfd. Nr. 25540–25640
//!   (S. 55).
//! - **APERAK AHB 1.1 § 2.4.1** — 45 Minuten for a Strom ORDERS.

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
pub use mako_engine::payload::Stammdaten;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    payload::UtilmdPayload,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-geschaeftsdatenanfrage";

/// ORDERS 17132 — Anfrage Stammdaten (Strom), LF · MSB → NB.
pub const ANFRAGE_PID: Pruefidentifikator = Pruefidentifikator::const_new(17132);

/// UTILMD answers to a 17132 (Anwendungsübersicht 4.0 lfd. Nr. 25590–25610):
/// 55035 Antwort auf GDA verb. MaLo and 55095 erz. MaLo (NB → LF), 55060
/// Antwort auf GDA (NB → MSB).
pub const ANTWORT_PIDS: &[u32] = &[55035, 55095, 55060];

/// ORDRSP 19101 — Ablehnung der Anfrage Stammdaten, NB → LF · MSB.
pub const ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19101);

/// Every PID this workflow receives: the Anfrage (NB side) and both answers
/// (Anfragender side).
pub const GDA_PIDS: &[u32] = &[17132, 55035, 55095, 55060, 19101];

/// Deadline label for the NB's answer window on an inbound 17132 —
/// [`mako_fristen::antwort::GESCHAEFTSDATENANFRAGE_WERKTAGE`] (GPKE Teil 4
/// Kap. 3.2 Nr. 2).
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-gda-antwort";

/// `BGM` DE 1001 of the ORDERS 17132 and of the ORDRSP 19101 — ORDERS AHB 1.1b
/// Kap. 4.1.1.1, ORDRSP AHB 1.1b Kap. 4.1.2.
pub const BGM_ANFRAGE_STAMMDATEN: &str = "Z14";

/// `BGM` DE 1001 of the UTILMD answers — UTILMD AHB Strom 2.2 Kap. 9.5.
pub const BGM_ANTWORT: &str = "E44";

/// Correlation key of the Anfragender's process: the Anfrage-Nummer it issued.
#[must_use]
pub fn anfragender_key(anfrage_nummer: &str) -> String {
    format!("gda-anfrage:{anfrage_nummer}")
}

/// Correlation key of the NB's process: the Anfragender's MP-ID and its
/// Anfrage-Nummer, which is unique only among that sender's Anfragen.
#[must_use]
pub fn netzbetreiber_key(anfragender: &str, anfrage_nummer: &str) -> String {
    format!("gda-eingang:{anfragender}:{anfrage_nummer}")
}

/// The `SG4 STS+7` Transaktionsgrund of a UTILMD answer — UTILMD AHB Strom 2.2
/// Kap. 9.5: `ZY7` on 55035, `ZY6` on 55095, `ZY4` on 55060.
#[must_use]
pub fn transaktionsgrund(antwort_pid: u32) -> Option<&'static str> {
    match antwort_pid {
        55_035 => Some("ZY7"),
        55_095 => Some("ZY6"),
        55_060 => Some("ZY4"),
        _ => None,
    }
}

/// Who sent the Anfrage, which decides the Entscheidungsbaum the NB's
/// Ablehnung cites in `SG2 AJT` DE 1082.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Anfragender {
    /// A Lieferant — `E_0441` „Geschäftsdatenanfrage zu Stammdaten prüfen"
    /// (EBD 4.2 Kap. 6.25.1, S. 314).
    Lf,
    /// A Messstellenbetreiber — `E_0443` (EBD 4.2 Kap. 6.26.1, S. 316, „AD:
    /// Geschäftsdatenanfrage von MSB an NB", Prüfende Rolle NB).
    ///
    /// The two documents disagree. ORDRSP AHB 1.1b (S. 7, 19101 `SG2 AJT`
    /// DE 1082) offers `E_0443` under `[14] ∧ [493]` „MP-ID in NAD+MR aus
    /// Sparte Gas" and `E_3004` under `[14]` alone. EBD 4.3 puts `E_0443` in
    /// its Strom chapter (Kap. 6.26.1) and `E_3004` in its Gas chapter
    /// (Kap. 13.20.1), and its change log scopes them „NB der Sparte Strom"
    /// and „NB der Sparte Gas". This workflow is Strom, so it follows the EBD.
    Msb,
}

impl Anfragender {
    /// The EBD the Ablehnung cites.
    #[must_use]
    pub fn codeliste(self) -> &'static str {
        match self {
            Self::Lf => "E_0441",
            Self::Msb => "E_0443",
        }
    }

    /// The Ablehnungscodes that EBD publishes: `E_0441` `A01` „Keine
    /// Berechtigung"; `E_0443` `A01` „Keine Berechtigung", `A02` „Kein iMS
    /// verbaut" and `A03` „An der genannten Adresse betreibt der NB keinen
    /// Stromanschluss" (EBD 4.3 S. 312 / S. 314).
    #[must_use]
    pub fn ablehnungscodes(self) -> &'static [&'static str] {
        match self {
            Self::Lf => &["A01"],
            Self::Msb => &["A01", "A02", "A03"],
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// The Anfrage both sides hold.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GdaAnfrage {
    /// The Anfragender (LF or MSB) — `NAD+MS` of the 17132.
    pub anfragender: MarktpartnerCode,
    /// The NB — `NAD+MR` of the 17132.
    pub netzbetreiber: MarktpartnerCode,
    /// ID of the Markt- or Messlokation (`SG2 LOC+172`).
    pub lokation_id: String,
    /// The Anfrage-Nummer — the 17132's `BGM` DE 1004.
    pub anfrage_nummer: MessageRef,
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events of the Geschäftsdatenanfrage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GdaEvent {
    /// This party (LF / MSB) sent the ORDERS 17132.
    AnfrageGesendet {
        /// ORDERS PID — 17132.
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: GdaAnfrage,
    },
    /// The NB received an ORDERS 17132.
    AnfrageEingegangen {
        /// ORDERS PID — 17132.
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: GdaAnfrage,
        /// Interchange reference of the inbound ORDERS.
        message_ref: MessageRef,
    },
    /// The inbound 17132 failed AHB validation; the APERAK answers it, not
    /// this process.
    AnfrageUngueltig {
        /// Validation errors.
        reason: String,
    },
    /// The NB sent the UTILMD answer.
    AntwortGesendet {
        /// UTILMD PID — 55035, 55095 or 55060.
        antwort_pid: Pruefidentifikator,
        /// Reference of the outbound UTILMD.
        message_ref: MessageRef,
    },
    /// The NB sent the ORDRSP 19101.
    AblehnungGesendet {
        /// ORDRSP PID — 19101.
        pid: Pruefidentifikator,
        /// `SG2 AJT` DE 4465 — the code.
        antwortcode: String,
        /// `SG2 AJT` DE 1082 — the tree it is drawn from: `E_0441` to a LF,
        /// `E_0443` to a MSB.
        codeliste: String,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// The Anfragender received the NB's UTILMD answer.
    AntwortErhalten {
        /// UTILMD PID — 55035, 55095 or 55060.
        antwort_pid: Pruefidentifikator,
        /// Reference of the inbound UTILMD.
        message_ref: MessageRef,
    },
    /// The Anfragender received the NB's ORDRSP 19101.
    AblehnungErhalten {
        /// ORDRSP PID — 19101.
        pid: Pruefidentifikator,
        /// `SG2 AJT` code, when the message carried one.
        antwortcode: Option<String>,
        /// `SG2 AJT` DE 1082 — the tree the NB named, as it arrived.
        codeliste: Option<String>,
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// A registered deadline fired while its window was open.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GdaEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "GdaAnfrageGesendet",
            Self::AnfrageEingegangen { .. } => "GdaAnfrageEingegangen",
            Self::AnfrageUngueltig { .. } => "GdaAnfrageUngueltig",
            Self::AntwortGesendet { .. } => "GdaAntwortGesendet",
            Self::AblehnungGesendet { .. } => "GdaAblehnungGesendet",
            Self::AntwortErhalten { .. } => "GdaAntwortErhalten",
            Self::AblehnungErhalten { .. } => "GdaAblehnungErhalten",
            Self::DeadlineExpired { .. } => "GdaDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        let stated = |richtung, pid: &Pruefidentifikator, message_ref: &MessageRef| {
            Some(Verlauf {
                richtung,
                pid: Some(pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            })
        };
        match self {
            Self::AnfrageGesendet { pid, anfrage } => {
                stated(Richtung::Ausgang, pid, &anfrage.anfrage_nummer)
            }
            Self::AnfrageEingegangen {
                pid, message_ref, ..
            } => stated(Richtung::Eingang, pid, message_ref),
            Self::AntwortGesendet {
                antwort_pid,
                message_ref,
            } => stated(Richtung::Ausgang, antwort_pid, message_ref),
            Self::AblehnungGesendet {
                pid, message_ref, ..
            } => stated(Richtung::Ausgang, pid, message_ref),
            Self::AntwortErhalten {
                antwort_pid,
                message_ref,
            } => stated(Richtung::Eingang, antwort_pid, message_ref),
            Self::AblehnungErhalten {
                pid, message_ref, ..
            } => stated(Richtung::Eingang, pid, message_ref),
            Self::AnfrageUngueltig { .. } | Self::DeadlineExpired { .. } => None,
        }
    }
    /// The ORDRSP 19101's `SG2 AJT` code and its DE 1082 tree — `E_0441`
    /// towards a LF, `E_0443` towards a MSB, as the Anfragender decides.
    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AblehnungGesendet {
                antwortcode,
                codeliste,
                ..
            } => Some(Antwortcode::new(
                antwortcode.clone(),
                Some(codeliste.clone()),
            )),
            Self::AblehnungErhalten {
                antwortcode,
                codeliste,
                ..
            } => antwortcode
                .as_ref()
                .map(|c| Antwortcode::new(c.clone(), codeliste.clone())),
            _ => None,
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// State of one Geschäftsdatenanfrage.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GdaState {
    /// No events yet.
    #[default]
    New,
    /// Anfragender side: the 17132 is out, the NB's answer is awaited.
    Angefragt(GdaAnfrage),
    /// NB side: the 17132 arrived and passed validation; the answer is owed.
    Offen(GdaAnfrage),
    /// The NB answered with the Stammdaten.
    Beantwortet {
        /// The Anfrage.
        anfrage: GdaAnfrage,
        /// UTILMD PID of the answer.
        antwort_pid: Pruefidentifikator,
    },
    /// The NB refused the Anfrage (ORDRSP 19101).
    Abgelehnt {
        /// The Anfrage.
        anfrage: GdaAnfrage,
        /// `SG2 AJT` code, when known.
        antwortcode: Option<String>,
    },
    /// The inbound 17132 failed AHB validation.
    Ungueltig {
        /// Validation errors.
        reason: String,
    },
    /// The NB's answer window lapsed while the answer was still owed.
    ///
    /// GPKE Teil 4 Kap. 3.2 gives the window and no consequence of missing it,
    /// so the lapse is a Frist breach for the operator, not an outcome: a late
    /// answer is still accepted and moves the process on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<GdaState>,
    },
}

impl GdaState {
    /// `true` once the Anfrage is answered, refused or invalid.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Beantwortet { .. } | Self::Abgelehnt { .. } | Self::Ungueltig { .. }
        )
    }

    /// Stable label of the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Angefragt(_) => "Angefragt",
            Self::Offen(_) => "Offen",
            Self::Beantwortet { .. } => "Beantwortet",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::Ungueltig { .. } => "Ungueltig",
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

/// An Anfrage holds its key while it waits for its answer, on either side.
impl mako_engine::workflow::OccupiesBusinessKey for GdaState {
    fn occupies_business_key(&self) -> bool {
        matches!(self.offen(), Self::Angefragt(_) | Self::Offen(_))
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands of the Geschäftsdatenanfrage.
#[derive(Debug, Clone)]
pub enum GdaCommand {
    /// Anfragender: send the ORDERS 17132 and record it.
    SendAnfrage {
        /// The Anfrage.
        anfrage: GdaAnfrage,
    },
    /// NB: an inbound ORDERS 17132, adapted at the transport boundary.
    ReceiveAnfrage {
        /// ORDERS PID read from `RFF+Z13`.
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: GdaAnfrage,
        /// Interchange reference of the inbound ORDERS.
        message_ref: MessageRef,
        /// `true` if AHB validation passed.
        validation_passed: bool,
        /// Validation errors when it did not.
        validation_errors: Vec<String>,
    },
    /// NB: send the UTILMD answer and record it.
    SendAntwort {
        /// UTILMD PID — 55035, 55095 or 55060.
        antwort_pid: Pruefidentifikator,
        /// Reference of the outbound UTILMD.
        message_ref: MessageRef,
        /// `SG4 STS+7` Transaktionsgrundergänzung on 55035 (`ZW4` / `ZAP`);
        /// `None` leaves it to the column's default.
        transaktionsgrund_ergaenzung: Option<String>,
        /// The Stammdaten the answer transmits.
        stammdaten: Stammdaten,
    },
    /// NB: send the ORDRSP 19101 and record it.
    SendAblehnung {
        /// Who sent the Anfrage — picks `E_0441` or `E_0443`.
        anfragender: Anfragender,
        /// `SG2 AJT` code from that EBD.
        antwortcode: String,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Anfragender: the NB's UTILMD answer arrived.
    ReceiveAntwort {
        /// UTILMD PID read from the message.
        antwort_pid: Pruefidentifikator,
        /// Reference of the inbound UTILMD.
        message_ref: MessageRef,
    },
    /// Anfragender: the NB's ORDRSP arrived.
    ReceiveAblehnung {
        /// ORDRSP PID read from the message.
        pid: Pruefidentifikator,
        /// `SG2 AJT` code, when present.
        antwortcode: Option<String>,
        /// `SG2 AJT` DE 1082, when present.
        codeliste: Option<String>,
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for GdaCommand {}

// ── Outbound messages ─────────────────────────────────────────────────────────

/// The ORDERS 17132 to the NB — ORDERS AHB 1.1b Kap. 4.1.1.1 (S. 8): `BGM+Z14`
/// with the Anfrage-Nummer in DE 1004, `SG2 NAD+DP` and `LOC+172` the Markt-
/// or Messlokation.
fn anfrage_outbox(anfrage: &GdaAnfrage) -> PendingOutbox {
    PendingOutbox::new(
        "ORDERS",
        anfrage.netzbetreiber.as_str(),
        serde_json::json!({
            "pid": ANFRAGE_PID.as_u32(),
            "sender": anfrage.anfragender.as_str(),
            "document_code": BGM_ANFRAGE_STAMMDATEN,
            "document_id": anfrage.anfrage_nummer.as_str(),
            "location": anfrage.lokation_id,
        }),
    )
}

/// The UTILMD answer to the Anfragender — UTILMD AHB Strom 2.2 Kap. 9.5.
fn antwort_outbox(
    anfrage: &GdaAnfrage,
    antwort_pid: Pruefidentifikator,
    message_ref: &MessageRef,
    transaktionsgrund_ergaenzung: Option<String>,
    stammdaten: Stammdaten,
) -> PendingOutbox {
    PendingOutbox::typed(
        anfrage.anfragender.as_str(),
        &UtilmdPayload {
            pid: antwort_pid.as_u32(),
            sender: anfrage.netzbetreiber.as_str().to_owned(),
            receiver: Some(anfrage.anfragender.as_str().to_owned()),
            message_ref: Some(message_ref.as_str().to_owned()),
            document_code: Some(BGM_ANTWORT.to_owned()),
            transaktionsgrund: transaktionsgrund(antwort_pid.as_u32()).map(str::to_owned),
            transaktionsgrund_ergaenzung,
            anfrage_referenz: Some(anfrage.anfrage_nummer.as_str().to_owned()),
            stammdaten,
            ..Default::default()
        },
    )
}

/// The ORDRSP 19101 to the Anfragender — ORDRSP AHB 1.1b Kap. 4.1.2: `BGM+Z14`,
/// `SG1 RFF+ON` the ORDERS' `BGM` DE 1004 (Hinweis `[521]`), `SG2 AJT` the code
/// with its EBD.
fn ablehnung_outbox(
    anfrage: &GdaAnfrage,
    anfragender: Anfragender,
    antwortcode: &str,
    message_ref: &MessageRef,
) -> PendingOutbox {
    PendingOutbox::new(
        "ORDRSP",
        anfrage.anfragender.as_str(),
        serde_json::json!({
            "pid": ABLEHNUNG_PID.as_u32(),
            "sender": anfrage.netzbetreiber.as_str(),
            "receiver": anfrage.anfragender.as_str(),
            "message_ref": message_ref.as_str(),
            "document_code": BGM_ANFRAGE_STAMMDATEN,
            "document_id": message_ref.as_str(),
            "order_reference": anfrage.anfrage_nummer.as_str(),
            "antwort_code": antwortcode,
            "antwort_codeliste": anfragender.codeliste(),
        }),
    )
}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// The Geschäftsdatenanfrage Strom (ORDERS 17132 → UTILMD / ORDRSP 19101).
pub struct GpkeGeschaeftsdatenanfrageWorkflow;

/// The one guard a fired deadline passes: the NB's answer window is open
/// while the answer is owed.
fn window_open(label: &str, state: &GdaState) -> bool {
    label == ANTWORT_WINDOW_LABEL && matches!(state, GdaState::Offen(_))
}

fn require_pid(pid: Pruefidentifikator, expected: Pruefidentifikator) -> Result<(), WorkflowError> {
    if pid == expected {
        Ok(())
    } else {
        Err(WorkflowError::rejected(format!(
            "PID {} is not part of the Geschäftsdatenanfrage here (expected {expected})",
            pid.as_u32()
        )))
    }
}

fn require_antwort_pid(pid: Pruefidentifikator) -> Result<(), WorkflowError> {
    if ANTWORT_PIDS.contains(&pid.as_u32()) {
        Ok(())
    } else {
        Err(WorkflowError::rejected(format!(
            "PID {} is not a UTILMD answer to a Geschäftsdatenanfrage (expected one of {ANTWORT_PIDS:?})",
            pid.as_u32()
        )))
    }
}

impl Workflow for GpkeGeschaeftsdatenanfrageWorkflow {
    type State = GdaState;
    type Event = GdaEvent;
    type Command = GdaCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];

    /// | Label | State guard | Command | Rule |
    /// |---|---|---|---|
    /// | `"gpke-gda-antwort"` | `Offen` | `TimeoutExpired` | GPKE Teil 4 Kap. 3.2 Nr. 2 — 1 WT |
    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| GdaCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was.
        let state = match state {
            GdaState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            GdaEvent::AnfrageGesendet { anfrage, .. } => GdaState::Angefragt(anfrage.clone()),
            GdaEvent::AnfrageEingegangen { anfrage, .. } => GdaState::Offen(anfrage.clone()),
            GdaEvent::AnfrageUngueltig { reason } => GdaState::Ungueltig {
                reason: reason.clone(),
            },
            GdaEvent::AntwortGesendet { antwort_pid, .. }
            | GdaEvent::AntwortErhalten { antwort_pid, .. } => match state {
                GdaState::Offen(anfrage) | GdaState::Angefragt(anfrage) => GdaState::Beantwortet {
                    anfrage,
                    antwort_pid: *antwort_pid,
                },
                other => other,
            },
            GdaEvent::AblehnungGesendet { antwortcode, .. } => match state {
                GdaState::Offen(anfrage) => GdaState::Abgelehnt {
                    anfrage,
                    antwortcode: Some(antwortcode.clone()),
                },
                other => other,
            },
            GdaEvent::AblehnungErhalten { antwortcode, .. } => match state {
                GdaState::Angefragt(anfrage) => GdaState::Abgelehnt {
                    anfrage,
                    antwortcode: antwortcode.clone(),
                },
                other => other,
            },
            GdaEvent::DeadlineExpired { label, .. } => match state {
                GdaState::Offen(_) => GdaState::FristAbgelaufen {
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
        // Only the timeout asks about the lapse itself.
        let lapsed = state;
        let state = state.offen();
        match command {
            GdaCommand::SendAnfrage { anfrage } => {
                if !matches!(state, GdaState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                let outbox = vec![anfrage_outbox(&anfrage).caused_by(0)];
                Ok(WorkflowOutput::with_outbox(
                    vec![GdaEvent::AnfrageGesendet {
                        pid: ANFRAGE_PID,
                        anfrage,
                    }],
                    outbox,
                ))
            }
            GdaCommand::ReceiveAnfrage {
                pid,
                anfrage,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GdaState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                require_pid(pid, ANFRAGE_PID)?;
                let mut events = vec![GdaEvent::AnfrageEingegangen {
                    pid,
                    anfrage,
                    message_ref,
                }];
                if !validation_passed {
                    events.push(GdaEvent::AnfrageUngueltig {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
            GdaCommand::SendAntwort {
                antwort_pid,
                message_ref,
                transaktionsgrund_ergaenzung,
                stammdaten,
            } => {
                let GdaState::Offen(anfrage) = state else {
                    return Err(WorkflowError::invalid_state("Offen", state.status_str()));
                };
                require_antwort_pid(antwort_pid)?;
                // A Stammdaten-Antwort without Stammdaten is no answer: the
                // column makes `SG5 LOC` and several `SG8` groups Muss.
                if stammdaten.lokationen.is_empty() || stammdaten.bloecke.is_empty() {
                    return Err(WorkflowError::validation(
                        "a Stammdaten-Antwort names its Lokationen (SG5 LOC) and carries its \
                         Stammdaten (SG8) — UTILMD AHB Strom 2.2 Kap. 9.5",
                    ));
                }
                let outbox = vec![
                    antwort_outbox(
                        anfrage,
                        antwort_pid,
                        &message_ref,
                        transaktionsgrund_ergaenzung,
                        stammdaten,
                    )
                    .caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![GdaEvent::AntwortGesendet {
                        antwort_pid,
                        message_ref,
                    }],
                    outbox,
                ))
            }
            GdaCommand::SendAblehnung {
                anfragender,
                antwortcode,
                message_ref,
            } => {
                let GdaState::Offen(anfrage) = state else {
                    return Err(WorkflowError::invalid_state("Offen", state.status_str()));
                };
                if !anfragender
                    .ablehnungscodes()
                    .contains(&antwortcode.as_str())
                {
                    return Err(WorkflowError::validation(format!(
                        "{antwortcode:?} is not an Ablehnungscode of {} (admitted: {:?})",
                        anfragender.codeliste(),
                        anfragender.ablehnungscodes()
                    )));
                }
                let outbox = vec![
                    ablehnung_outbox(anfrage, anfragender, &antwortcode, &message_ref).caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![GdaEvent::AblehnungGesendet {
                        pid: ABLEHNUNG_PID,
                        antwortcode,
                        codeliste: anfragender.codeliste().to_owned(),
                        message_ref,
                    }],
                    outbox,
                ))
            }
            GdaCommand::ReceiveAntwort {
                antwort_pid,
                message_ref,
            } => {
                if !matches!(state, GdaState::Angefragt(_)) {
                    return Err(WorkflowError::invalid_state(
                        "Angefragt",
                        state.status_str(),
                    ));
                }
                require_antwort_pid(antwort_pid)?;
                Ok(vec![GdaEvent::AntwortErhalten {
                    antwort_pid,
                    message_ref,
                }]
                .into())
            }
            GdaCommand::ReceiveAblehnung {
                pid,
                antwortcode,
                codeliste,
                message_ref,
            } => {
                if !matches!(state, GdaState::Angefragt(_)) {
                    return Err(WorkflowError::invalid_state(
                        "Angefragt",
                        state.status_str(),
                    ));
                }
                require_pid(pid, ABLEHNUNG_PID)?;
                Ok(vec![GdaEvent::AblehnungErhalten {
                    pid,
                    antwortcode,
                    codeliste,
                    message_ref,
                }]
                .into())
            }
            GdaCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GdaEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(p: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(p).expect("valid PID")
    }

    fn anfrage() -> GdaAnfrage {
        GdaAnfrage {
            anfragender: MarktpartnerCode::new("9900000000001"),
            netzbetreiber: MarktpartnerCode::new("9900000000002"),
            lokation_id: "51238696012".into(),
            anfrage_nummer: MessageRef::new("GDA-0001"),
        }
    }

    fn run(state: GdaState, cmd: GdaCommand) -> Result<GdaState, WorkflowError> {
        let out = GpkeGeschaeftsdatenanfrageWorkflow::handle(&state, cmd)?;
        Ok(out
            .iter()
            .fold(state, GpkeGeschaeftsdatenanfrageWorkflow::apply))
    }

    fn eingegangen(valid: bool) -> GdaCommand {
        GdaCommand::ReceiveAnfrage {
            pid: ANFRAGE_PID,
            anfrage: anfrage(),
            message_ref: MessageRef::new("UNH-1"),
            validation_passed: valid,
            validation_errors: if valid { vec![] } else { vec!["x".into()] },
        }
    }

    fn stammdaten() -> Stammdaten {
        use mako_engine::payload::{StammdatenBlock, StammdatenLokation, StammdatenSegment};
        Stammdaten {
            lokationen: vec![StammdatenLokation {
                qualifier: "Z16".into(),
                id: "51238696012".into(),
            }],
            bloecke: vec![StammdatenBlock {
                seq: "ZD5".into(),
                segmente: vec![StammdatenSegment {
                    tag: "RFF".into(),
                    elemente: vec![vec!["Z31".into(), "9992000000018".into()]],
                }],
            }],
            sg12: vec![],
            ..Stammdaten::default()
        }
    }

    fn antwort(p: u32) -> GdaCommand {
        GdaCommand::SendAntwort {
            antwort_pid: pid(p),
            message_ref: MessageRef::new("UTILMD-1"),
            transaktionsgrund_ergaenzung: None,
            stammdaten: stammdaten(),
        }
    }

    fn angefragt() -> GdaState {
        run(
            GdaState::New,
            GdaCommand::SendAnfrage { anfrage: anfrage() },
        )
        .unwrap()
    }

    #[test]
    fn nb_answers_with_a_utilmd() {
        let state = run(GdaState::New, eingegangen(true)).unwrap();
        assert!(matches!(state, GdaState::Offen(_)));
        let out = GpkeGeschaeftsdatenanfrageWorkflow::handle(&state, antwort(55_035)).unwrap();
        assert_eq!(out.outbox.len(), 1, "the answer is queued with its record");
        let entry = &out.outbox[0];
        assert_eq!(entry.message_type.as_ref(), "UTILMD");
        assert_eq!(entry.recipient.as_ref(), "9900000000001");
        let p = &entry.payload;
        assert_eq!(p["pid"], 55_035);
        assert_eq!(p["sender"], "9900000000002");
        assert_eq!(p["document_code"], "E44");
        assert_eq!(p["transaktionsgrund"], "ZY7");
        assert_eq!(p["anfrage_referenz"], "GDA-0001");
        assert_eq!(p["stammdaten"]["bloecke"][0]["seq"], "ZD5");
        let state = out
            .iter()
            .fold(state, GpkeGeschaeftsdatenanfrageWorkflow::apply);
        assert!(matches!(state, GdaState::Beantwortet { .. }));
        assert!(state.is_terminal());
    }

    /// The Anfragender's command sends the ORDERS 17132 it records, with the
    /// Anfrage-Nummer in `BGM` DE 1004.
    #[test]
    fn the_anfrage_queues_the_orders_17132() {
        let out = GpkeGeschaeftsdatenanfrageWorkflow::handle(
            &GdaState::New,
            GdaCommand::SendAnfrage { anfrage: anfrage() },
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1);
        let entry = &out.outbox[0];
        assert_eq!(entry.message_type.as_ref(), "ORDERS");
        assert_eq!(entry.recipient.as_ref(), "9900000000002");
        assert_eq!(entry.payload["pid"], 17_132);
        assert_eq!(entry.payload["document_code"], "Z14");
        assert_eq!(entry.payload["document_id"], "GDA-0001");
        assert_eq!(entry.payload["location"], "51238696012");
    }

    /// An answer without Stammdaten is refused rather than sent empty.
    #[test]
    fn an_answer_without_stammdaten_is_refused() {
        let state = run(GdaState::New, eingegangen(true)).unwrap();
        let refused = GpkeGeschaeftsdatenanfrageWorkflow::handle(
            &state,
            GdaCommand::SendAntwort {
                antwort_pid: pid(55_035),
                message_ref: MessageRef::new("UTILMD-1"),
                transaktionsgrund_ergaenzung: None,
                stammdaten: Stammdaten::default(),
            },
        );
        assert!(refused.is_err());
    }

    /// Each UTILMD answer carries its own Transaktionsgrund.
    #[test]
    fn each_answer_pid_has_its_transaktionsgrund() {
        assert_eq!(transaktionsgrund(55_035), Some("ZY7"));
        assert_eq!(transaktionsgrund(55_095), Some("ZY6"));
        assert_eq!(transaktionsgrund(55_060), Some("ZY4"));
        assert_eq!(transaktionsgrund(55_036), None);
    }

    /// The two sides of one Anfrage-Nummer never share a key, and two senders'
    /// equal numbers do not either.
    #[test]
    fn the_two_sides_are_keyed_apart() {
        assert_ne!(
            anfragender_key("X"),
            netzbetreiber_key("9900000000001", "X")
        );
        assert_ne!(
            netzbetreiber_key("9900000000001", "X"),
            netzbetreiber_key("9900000000003", "X")
        );
    }

    /// **The Ablehnung names the tree the recipient's role decides**, so the
    /// Verlauf reads the wording: `A01` is „Keine Berechtigung" in both trees,
    /// and the event of each Anfragender names its own.
    #[test]
    fn the_ablehnung_names_the_tree_of_its_recipient() {
        let state = run(GdaState::New, eingegangen(true)).unwrap();
        for (anfragender, code, tree) in [
            (Anfragender::Lf, "A01", "E_0441"),
            (Anfragender::Msb, "A01", "E_0443"),
            (Anfragender::Msb, "A03", "E_0443"),
        ] {
            let out = GpkeGeschaeftsdatenanfrageWorkflow::handle(
                &state,
                GdaCommand::SendAblehnung {
                    anfragender,
                    antwortcode: code.into(),
                    message_ref: MessageRef::new("ORDRSP-1"),
                },
            )
            .unwrap();
            let antwort = out.events[0]
                .antwort()
                .expect("the Ablehnung states its code");
            assert_eq!(antwort.code, code, "{anfragender:?}");
            assert_eq!(antwort.codeliste.as_deref(), Some(tree), "{anfragender:?}");
            assert!(
                mako_pruefung::codes::lies(tree, code).is_some(),
                "{tree} {code} has its wording"
            );
        }
    }

    /// The codes a refusal is admitted with are exactly the ones its tree
    /// publishes in the catalogue — the list here cannot drift from the EBD.
    #[test]
    fn the_admitted_codes_are_the_catalogue_of_each_tree() {
        for anfragender in [Anfragender::Lf, Anfragender::Msb] {
            let tree = anfragender.codeliste();
            let katalog: Vec<&str> = mako_pruefung::codes::CODELISTEN
                .iter()
                .find(|(id, _)| *id == tree)
                .unwrap_or_else(|| panic!("{tree} is catalogued"))
                .1
                .iter()
                .map(|c| c.code)
                .collect();
            assert_eq!(anfragender.ablehnungscodes(), katalog.as_slice(), "{tree}");
        }
    }

    #[test]
    fn nb_refuses_with_19101() {
        let state = run(GdaState::New, eingegangen(true)).unwrap();
        for (anfragender, code, codeliste) in [
            (Anfragender::Lf, "A01", "E_0441"),
            (Anfragender::Msb, "A02", "E_0443"),
        ] {
            let out = GpkeGeschaeftsdatenanfrageWorkflow::handle(
                &state,
                GdaCommand::SendAblehnung {
                    anfragender,
                    antwortcode: code.into(),
                    message_ref: MessageRef::new("ORDRSP-1"),
                },
            )
            .unwrap();
            let entry = &out.outbox[0];
            assert_eq!(entry.message_type.as_ref(), "ORDRSP");
            assert_eq!(entry.payload["pid"], 19_101);
            assert_eq!(entry.payload["order_reference"], "GDA-0001");
            assert_eq!(entry.payload["antwort_code"], code);
            assert_eq!(entry.payload["antwort_codeliste"], codeliste);
            let after = out
                .iter()
                .fold(state.clone(), GpkeGeschaeftsdatenanfrageWorkflow::apply);
            assert!(matches!(after, GdaState::Abgelehnt { .. }));
        }
        // `A02` „Kein iMS verbaut" is `E_0443`'s alone.
        assert!(
            GpkeGeschaeftsdatenanfrageWorkflow::handle(
                &state,
                GdaCommand::SendAblehnung {
                    anfragender: Anfragender::Lf,
                    antwortcode: "A02".into(),
                    message_ref: MessageRef::new("ORDRSP-1"),
                },
            )
            .is_err()
        );
    }

    /// An ORDERS is not an answer: the Stammdaten travel as UTILMD (ORDERS AHB
    /// 1.1b Kap. 4.1.1), so the NB may not close the Anfrage with 17102–17133.
    #[test]
    fn an_orders_pid_is_no_answer() {
        let state = run(GdaState::New, eingegangen(true)).unwrap();
        for orders in [17_102, 17_103, 17_118, 17_126, 17_133] {
            let refused = GpkeGeschaeftsdatenanfrageWorkflow::handle(&state, antwort(orders));
            assert!(refused.is_err(), "{orders} must not answer a 17132");
        }
    }

    /// Only 17132 opens the process on the NB side.
    #[test]
    fn only_17132_opens_it() {
        for other in [17_101, 17_103, 17_126] {
            let refused = GpkeGeschaeftsdatenanfrageWorkflow::handle(
                &GdaState::New,
                GdaCommand::ReceiveAnfrage {
                    pid: pid(other),
                    anfrage: anfrage(),
                    message_ref: MessageRef::new("UNH-1"),
                    validation_passed: true,
                    validation_errors: vec![],
                },
            );
            assert!(
                refused.is_err(),
                "{other} is not a Geschäftsdatenanfrage Strom"
            );
        }
    }

    #[test]
    fn invalid_anfrage_is_terminal() {
        let state = run(GdaState::New, eingegangen(false)).unwrap();
        assert!(matches!(state, GdaState::Ungueltig { .. }));
    }

    #[test]
    fn anfragender_resumes_on_each_utilmd_answer() {
        for answer in ANTWORT_PIDS {
            let state = run(
                angefragt(),
                GdaCommand::ReceiveAntwort {
                    antwort_pid: pid(*answer),
                    message_ref: MessageRef::new("UTILMD-1"),
                },
            )
            .unwrap();
            assert!(matches!(state, GdaState::Beantwortet { .. }), "{answer}");
        }
    }

    #[test]
    fn anfragender_resumes_on_19101() {
        let state = run(
            angefragt(),
            GdaCommand::ReceiveAblehnung {
                pid: ABLEHNUNG_PID,
                antwortcode: Some("A02".into()),
                codeliste: Some("E_0443".into()),
                message_ref: MessageRef::new("ORDRSP-1"),
            },
        )
        .unwrap();
        assert!(matches!(state, GdaState::Abgelehnt { .. }));
    }

    /// An answer nobody asked for is refused, not absorbed.
    #[test]
    fn an_answer_without_an_anfrage_is_refused() {
        let refused = GpkeGeschaeftsdatenanfrageWorkflow::handle(
            &GdaState::New,
            GdaCommand::ReceiveAntwort {
                antwort_pid: pid(55_035),
                message_ref: MessageRef::new("UTILMD-1"),
            },
        );
        assert!(refused.is_err());
    }

    #[test]
    fn lapse_is_a_frist_breach_and_a_late_answer_still_closes() {
        let state = run(GdaState::New, eingegangen(true)).unwrap();
        let state = run(
            state,
            GdaCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(matches!(state, GdaState::FristAbgelaufen { .. }));
        let state = run(state, antwort(55_060)).unwrap();
        assert!(matches!(state, GdaState::Beantwortet { .. }));
    }

    #[test]
    fn deadline_on_the_anfragender_side_is_a_noop() {
        let out = GpkeGeschaeftsdatenanfrageWorkflow::handle(
            &angefragt(),
            GdaCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn verlauf_states_each_message_with_its_direction() {
        let stated = |richtung, p: u32, r: &str| {
            Some(Verlauf {
                richtung,
                pid: Some(p),
                message_ref: Some(r.to_owned()),
            })
        };
        assert_eq!(
            GdaEvent::AnfrageGesendet {
                pid: ANFRAGE_PID,
                anfrage: anfrage(),
            }
            .verlauf(),
            stated(Richtung::Ausgang, 17_132, "GDA-0001")
        );
        assert_eq!(
            GdaEvent::AnfrageEingegangen {
                pid: ANFRAGE_PID,
                anfrage: anfrage(),
                message_ref: MessageRef::new("UNH-1"),
            }
            .verlauf(),
            stated(Richtung::Eingang, 17_132, "UNH-1")
        );
        assert_eq!(
            GdaEvent::AntwortGesendet {
                antwort_pid: pid(55_035),
                message_ref: MessageRef::new("U-1"),
            }
            .verlauf(),
            stated(Richtung::Ausgang, 55_035, "U-1")
        );
        assert_eq!(
            GdaEvent::AblehnungGesendet {
                pid: ABLEHNUNG_PID,
                antwortcode: "A01".into(),
                codeliste: "E_0441".into(),
                message_ref: MessageRef::new("O-1"),
            }
            .verlauf(),
            stated(Richtung::Ausgang, 19_101, "O-1")
        );
        assert_eq!(
            GdaEvent::AntwortErhalten {
                antwort_pid: pid(55_095),
                message_ref: MessageRef::new("U-2"),
            }
            .verlauf(),
            stated(Richtung::Eingang, 55_095, "U-2")
        );
        assert_eq!(
            GdaEvent::AblehnungErhalten {
                pid: ABLEHNUNG_PID,
                antwortcode: None,
                codeliste: None,
                message_ref: MessageRef::new("O-2"),
            }
            .verlauf(),
            stated(Richtung::Eingang, 19_101, "O-2")
        );
        assert_eq!(
            GdaEvent::AnfrageUngueltig { reason: "x".into() }.verlauf(),
            None
        );
    }

    #[test]
    fn an_ablehnung_states_its_code_and_a_utilmd_answer_states_none() {
        let gesendet = GdaEvent::AblehnungGesendet {
            pid: pid(19101),
            antwortcode: "A02".into(),
            codeliste: "E_0443".into(),
            message_ref: MessageRef::new("ORDRSP-1"),
        };
        assert_eq!(
            gesendet.antwort(),
            Some(Antwortcode::new("A02", Some("E_0443")))
        );
        let ohne_code = GdaEvent::AblehnungErhalten {
            pid: pid(19101),
            antwortcode: None,
            codeliste: Some("E_0441".into()),
            message_ref: MessageRef::new("ORDRSP-2"),
        };
        assert_eq!(ohne_code.antwort(), None);
        let utilmd = GdaEvent::AntwortErhalten {
            antwort_pid: pid(55035),
            message_ref: MessageRef::new("UTILMD-1"),
        };
        assert_eq!(utilmd.antwort(), None);
    }
}
