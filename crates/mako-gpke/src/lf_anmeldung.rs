//! GPKE LF-side Anmeldung workflow — Lieferbeginn, Lieferende, Kündigung.
//!
//! When `makod` operates as a **Lieferant (LF)**, the ERP instructs the engine
//! to initiate a GPKE process outbound to the Netzbetreiber (NB) or the old
//! Lieferant (LFA for Kündigung). This module implements that LF-side
//! outbound-first workflow.
//!
//! ## Process flow
//!
//! ```text
//! ERP → POST /api/v1/commands          (gpke.lieferbeginn.anmelden, role LF)
//!       ↓
//!   [InitiateAnmeldung]
//!       ↓ emits Initiated + UTILMD outbox entry
//! AS4 sender → UTILMD 55001 to NB
//!       ↓
//! AS4 inbound ← UTILMD 55002/55003 from NB
//!       ↓
//!   [HandleAntwort]
//!       ↓ emits AntwortReceived
//!       ↓ (on acceptance) emits Activated
//! ERP webhook ← ErpEvent::AperakAccepted / AperakRejected (via outbox)
//! ```
//!
//! ## Prüfidentifikatoren
//!
//! | Outbound (LF → NB)               | PID   | Inbound response (NB → LF) | PID   |
//! |----------------------------------|-------|-------------------------------|-------|
//! | Anmeldung verb. MaLo             | 55001 | Bestätigung Anmeldung         | 55002 |
//! |                                  |       | Ablehnung Anmeldung           | 55003 |
//! | Abmeldung                        | 55004 | Bestätigung Abmeldung         | 55005 |
//! |                                  |       | Ablehnung Abmeldung           | 55006 |
//! | Kündigung Lieferbeginn           | 55016 | Bestätigung Kündigung        | 55017 |
//! |                                  |       | Ablehnung Kündigung          | 55018 |
//! | Anmeldung Lieferbeginn erz. MaLo | 55077 | Bestätigung erz. MaLo         | 55078 |
//! |                                  |       | Ablehnung erz. MaLo           | 55080 |
//!
//! ## Regulatory basis
//!
//! - **BDEW GPKE** — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität
//! - **BK6-24-174** — the Festlegung that states the NB's answer window: a
//!   wall-clock instant on the 1. Werktag nach dem ÜT, per Prüfidentifikator
//!   (GPKE Teil 2). BK6-22-024 still carries GPKE **Teil 4**, but none of the
//!   Teil-2 Zuordnungsprozesse, and no 24-hour window anywhere.
//! - **UTILMD S2.1/S2.2** — EDI@Energy message format

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::payload::Stammdaten;
use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name for the GPKE LF-side Anmeldung workflow.
///
/// Used as the `workflow_name` in [`WorkflowId`] when spawning a new process
/// and as the registration key in the PID router (see [`GpkeModule`]).
///
/// [`WorkflowId`]: mako_engine::version::WorkflowId
/// [`GpkeModule`]: crate::GpkeModule
pub const WORKFLOW_NAME: &str = "gpke-lf-anmeldung";

/// Outbound request PIDs that trigger a new `GpkeLfAnmeldungWorkflow` process
/// when the ERP calls `POST /api/v1/commands`.
///
/// These are LF→NB/LFA direction only; the corresponding NB→LF response PIDs
/// ([`ANTWORT_PIDS_LF`]) complete the conversation.
pub const ANFRAGE_PIDS_LF: &[u32] = &[
    55001, // Anmeldung verb. MaLo (LF → NB)
    55004, // Abmeldung            (LF → NB)
    55016, // Kündigung Lieferbeginn (LFN → LFA)
    55077, // Anmeldung Lieferbeginn erz. MaLo (LFN → NB, BK6-24-174)
];

/// The Anfrage-PIDs whose `SG8 SEQ+Z79` Produktpaket is **Muss**.
///
/// UTILMD AHB Strom 2.2 Kap. 5.3 lists the six Anwendungsfälle that carry one;
/// the two this workflow sends are the Anmeldungen. An Abmeldung (55004) and a
/// Kündigung (55016) register nothing and carry none.
///
/// The Codeliste der Konfigurationen 1.4 Kap. 6.1.1 makes the Bilanzkreis
/// product (`9991000002082`) unconditional inside the package.
pub const ANMELDUNG_PIDS_MIT_BILANZKREIS: &[u32] = &[
    55001, // Anmeldung verb. MaLo
    55077, // Anmeldung erz. MaLo
];

/// The Anmeldung verbrauchende Marktlokation — LF an NB.
pub const ANMELDUNG_PID: u32 = 55001;
/// The Anmeldung erzeugende Marktlokation — LF an NB.
pub const ANMELDUNG_ERZEUGEND_PID: u32 = 55077;

/// The `SG4 STS+7` Transaktionsgründe the Anmeldung 55001 admits — `E01`
/// Ein-/Auszug (Umzug), `E03` Wechsel — and the Ergänzungen beside them,
/// `ZW4` verbrauchende and `ZAP` ruhende Marktlokation; the segment is Muss
/// (UTILMD AHB Strom 2.2 Kap. 8.2, S. 73). The two Gründe are different
/// Anmeldungen, so neither is a default for the other.
pub const ANMELDUNG_GRUENDE: &[&str] = &["E01", "E03"];
/// See [`ANMELDUNG_GRUENDE`].
pub const ANMELDUNG_ERGAENZUNGEN: &[&str] = &["ZW4", "ZAP"];
/// The 55001 Ergänzung where the caller states none: `ZW4`. The LF names a
/// ruhende Marktlokation with `ZW4` as well, „Somit muss der anmeldende LF
/// nicht zwingend wissen, dass es sich bei der angemeldeten Marktlokation um
/// eine zum Zeitpunkt der Anmeldung ruhende Marktlokation handelt" (UTILMD AHB
/// Strom 2.2 Kap. 5.5.2.4, S. 49).
pub const ANMELDUNG_ERGAENZUNG: &str = "ZW4";
/// The 55077's one Transaktionsgrund, `E03` Wechsel (UTILMD AHB Strom 2.2
/// Kap. 8.3, S. 90); its Ergänzung is one of the [`GESCHAEFTSVORFAELLE`].
pub const ANMELDUNG_ERZEUGEND_GRUND: &str = "E03";

/// The Abmeldung — LF an NB.
pub const ABMELDUNG_PID: u32 = 55004;
/// The Kündigung — „LFN an LFA": it goes to the Altlieferant, never the NB.
pub const KUENDIGUNG_PID: u32 = 55016;

/// The `SG4 STS+7` Transaktionsgründe the Abmeldung 55004 admits (UTILMD AHB
/// Strom 2.2 Kap. 5.4, S. 165–166).
pub const ABMELDUNG_GRUENDE: &[&str] = &[
    "E01", "Z33", "Z41", "ZG9", "ZH1", "ZH2", "ZT4", "ZT5", "ZZD",
];

/// The 55004 Gründe that name the confirmed Beginn instead of the Ende:
/// „Aufhebung einer zukünftigen Zuordnung" makes `DTM+92` Muss (Bedingung
/// `[7]`, filled „mit dem ursprünglich vom NB bestätigten Beginnzeitpunkt",
/// `[577]`) and `DTM+93` only under every other Grund (`[11]`) — S. 165.
pub const ABMELDUNG_AUFHEBUNG_GRUENDE: &[&str] = &["ZG9", "ZH1", "ZH2"];

/// The Lokationsart Ergänzungen the 55004 admits: `ZW3` erzeugende, `ZW4`
/// verbrauchende Marktlokation, `ZW5` Tranche, `ZAP` ruhende Marktlokation
/// (S. 166). Each selects its own `SG5 LOC` (`[479]`/`[480]`, `[481]`,
/// `[96]`), so none is a default for another.
pub const ABMELDUNG_ERGAENZUNGEN: &[&str] = &["ZW3", "ZW4", "ZW5", "ZAP"];

/// The 55016's one Transaktionsgrund, `E03` Wechsel, and its Ergänzungen
/// `ZW3`/`ZW4`/`ZW5` (UTILMD AHB Strom 2.2 Kap. 5.3).
pub const KUENDIGUNG_GRUND: &str = "E03";
/// See [`KUENDIGUNG_GRUND`].
pub const KUENDIGUNG_ERGAENZUNGEN: &[&str] = &["ZW3", "ZW4", "ZW5"];

/// The Anmeldung neue verbrauchende Marktlokation — LF an NB (UTILMD AHB
/// Strom 2.2 Kap. 8.4, S. 95).
pub const NEUANLAGE_VERBRAUCHEND_PID: u32 = 55_600;
/// The Anmeldung neue erzeugende Marktlokation — LF an NB (UTILMD AHB Strom
/// 2.2 Kap. 8.5, S. 109).
pub const NEUANLAGE_ERZEUGEND_PID: u32 = 55_601;
/// The Anmeldungen neue Marktlokation this workflow sends.
pub const NEUANLAGE_PIDS_LF: &[u32] = &[NEUANLAGE_VERBRAUCHEND_PID, NEUANLAGE_ERZEUGEND_PID];
/// Both columns' one Transaktionsgrund, `E02` „Einzug in Neuanlage" (S. 97,
/// 110).
pub const NEUANLAGE_GRUND: &str = "E02";
/// The Ergänzungen each column admits: `ZW4` „Verbrauchende Marktlokation" on
/// the 55600 (S. 97); on the 55601 the Geschäftsvorfälle `ZW0` (Anmeldung
/// 100 %) and `ZW2` — the column lists no `ZW1` (S. 110).
#[must_use]
pub fn neuanlage_ergaenzungen(pid: u32) -> &'static [&'static str] {
    match pid {
        NEUANLAGE_VERBRAUCHEND_PID => &["ZW4"],
        NEUANLAGE_ERZEUGEND_PID => &[GESCHAEFTSVORFALL_1, GESCHAEFTSVORFALL_3],
        _ => &[],
    }
}

/// The NB's answers to an Anmeldung neue Marktlokation, routed back to this
/// workflow: `(answer, Anmeldung it answers, Bestätigung?)` — 55602/55604
/// answer the 55600, 55603/55605 the 55601 (Kap. 8.4, 8.5).
pub const NEUANLAGE_ANTWORTEN: &[(u32, u32, bool)] = &[
    (55_602, NEUANLAGE_VERBRAUCHEND_PID, true),
    (55_603, NEUANLAGE_ERZEUGEND_PID, true),
    (55_604, NEUANLAGE_VERBRAUCHEND_PID, false),
    (55_605, NEUANLAGE_ERZEUGEND_PID, false),
];
/// The PIDs of [`NEUANLAGE_ANTWORTEN`].
pub const NEUANLAGE_ANTWORT_PIDS_LF: &[u32] = &[55_602, 55_603, 55_604, 55_605];

/// Correlation key under which the LF's Anmeldung neue Marktlokation is found
/// by the NB's answer: the `SG4 IDE+24` Vorgangsnummer it carried, which every
/// answer restates in `SG6 RFF+TN` (Muss on 55602–55605, S. 99, 114).
///
/// The Marktlokation is no key: the Anmeldung names it only „Wenn bekannt"
/// (Bedingung `[165]`, S. 98, 112) and the Bestätigung names the one the NB
/// identified.
#[must_use]
pub fn neuanlage_vorgang_key(vorgangsnummer: &str) -> String {
    format!("lf-neuanlage-vorgang:{vorgangsnummer}")
}

/// The Anfrage-PIDs whose `SG4 STS+7` Ergänzung is a **Geschäftsvorfall**.
///
/// The erzeugende Anmeldung admits `ZW0`/`ZW1`/`ZW2` and nothing else, where
/// the verbrauchende one admits `ZW4`/`ZAP`. There is no default among the
/// three: a 100 %-Zuordnung, a handover of an existing Tranche and the building
/// of a new one are different acts, and only the third carries a Tranchengröße.
pub const ANMELDUNG_PIDS_MIT_GESCHAEFTSVORFALL: &[u32] = &[
    55077, // Anmeldung erz. MaLo
];

/// `ZW0` — Geschäftsvorfall 1, „vollständige (100%ige) Zuordnung".
pub const GESCHAEFTSVORFALL_1: &str = "ZW0";
/// `ZW1` — Geschäftsvorfall 2, Zuordnung zu einer **bestehenden** Tranche.
pub const GESCHAEFTSVORFALL_2: &str = "ZW1";
/// `ZW2` — Geschäftsvorfall 3, Zuordnung zu einer **neu zu bildenden** Tranche.
///
/// The one that makes the Tranchengröße (`9991000002090`) mandatory, per
/// Codeliste der Konfigurationen 1.4 Kap. 6.1.1.
pub const GESCHAEFTSVORFALL_3: &str = "ZW2";

/// The three Geschäftsvorfälle an erzeugende Anmeldung may state.
pub const GESCHAEFTSVORFAELLE: &[&str] = &[
    GESCHAEFTSVORFALL_1,
    GESCHAEFTSVORFALL_2,
    GESCHAEFTSVORFALL_3,
];

/// Inbound response PIDs (NB → LF or LFA → LF) routed back to this workflow.
///
/// These must be registered in the PID router so the AS4 inbound layer can
/// route them by conversation ID to the correct `GpkeLfAnmeldungWorkflow`
/// instance.
pub const ANTWORT_PIDS_LF: &[u32] = &[
    55002, // Bestätigung Anmeldung verb. MaLo (NB → LF)
    55003, // Ablehnung Anmeldung verb. MaLo   (NB → LF)
    55005, // Bestätigung Abmeldung            (NB → LF)
    55006, // Ablehnung Abmeldung              (NB → LF)
    55017, // Bestätigung Kündigung                (LFA → LFN)
    55018, // Ablehnung Kündigung                  (LFA → LFN)
    55078, // Bestätigung Anmeldung erz. MaLo       (NB → LFN)
    55080, // Ablehnung Anmeldung erz. MaLo         (NB → LFN); 55079 unassigned
];

/// Deadline label for the NB/LFA response window.
///
/// Sized by `mako_fristen::antwort` — a clock time on the 1. Werktag
/// after the ÜT, per PID. Never a flat 24 hours: that approximation expires a
/// Friday arrival on Saturday and calls a Tuesday-evening one healthy nine
/// hours after its Frist lapsed.
///
/// After sending the outbound ANFRAGE, the LF registers a deadline with this
/// label. If no ANTWORT arrives before the window this Prüfidentifikator
/// publishes, the scheduler fires `on_deadline` → `TimeoutExpired` to
/// transition the process to `Rejected`.
///
/// ```rust,ignore
/// let due = mako_fristen::antwort::antwort_deadline(pid, sent_at)
///     .expect("a PID with a published Antwortfrist");
/// let deadline = Deadline::new(process.stream_id().clone(), ..., NB_RESPONSE_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const NB_RESPONSE_WINDOW_LABEL: &str = "nb-response-window";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE LF-side Anmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum LfAnmeldungEvent {
    /// LF-side Anmeldung initiated — outbound UTILMD queued for AS4 delivery.
    Initiated {
        /// PID of the outbound Anfrage (55001, 55004, 55016, or 55077).
        pruefidentifikator: Pruefidentifikator,
        /// MaLo / supply point identifier.
        location_id: MaLo,
        /// Our own market-partner ID (the Lieferant).
        sender: MarktpartnerCode,
        /// Counterparty market-partner ID (NB or LFA).
        receiver: MarktpartnerCode,
        /// Requested supply start / end / cancellation date.
        process_date: String,
    },
    /// Counterparty (NB or LFA) responded — accepted or rejected.
    AntwortReceived {
        /// PID of the inbound response (55002/55003, 55005/55006, 55017, 55018, 55078, 55080).
        response_pid: Pruefidentifikator,
        /// `true` if the request was accepted.
        accepted: bool,
        /// Rejection reason (only set when `accepted = false`).
        reason: Option<String>,
        /// Message reference from the inbound response UTILMD.
        response_ref: MessageRef,
        /// `SG4 STS+E01` — the answer's code and Codeliste (`A02` in
        /// `E_0622`), which is what the refusal means; `None` where the
        /// response carries none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        antwort: Option<Antwortcode>,
    },
    /// The NB named the Marktlokation of an Anmeldung neue Marktlokation — the
    /// `SG5 LOC+Z16` of its Bestätigung, Muss there (UTILMD AHB Strom 2.2 S.
    /// 98, 112).
    MarktlokationBenannt {
        /// The Marktlokations-ID.
        marktlokation: MaLo,
    },
    /// Accepted Lieferbeginn Anmeldung activated (supply now live).
    Activated,
    /// A registered deadline expired before the NB responded.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for LfAnmeldungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "LfAnmeldungInitiated",
            Self::AntwortReceived { .. } => "LfAnmeldungAntwortReceived",
            Self::MarktlokationBenannt { .. } => "LfAnmeldungMarktlokationBenannt",
            Self::Activated => "LfAnmeldungActivated",
            Self::DeadlineExpired { .. } => "LfAnmeldungDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::Initiated {
                pruefidentifikator, ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: None,
            }),
            Self::AntwortReceived {
                response_pid,
                response_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(response_pid.as_u32()),
                message_ref: Some(response_ref.as_str().to_owned()),
            }),
            Self::MarktlokationBenannt { .. } | Self::Activated | Self::DeadlineExpired { .. } => {
                None
            }
        }
    }

    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AntwortReceived { antwort, .. } => antwort.clone(),
            _ => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `Initiated` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LfAnmeldungData {
    /// PID of the outbound Anfrage (55001, 55004, 55016, or 55077).
    pub pruefidentifikator: Pruefidentifikator,
    /// MaLo / supply point identifier.
    pub location_id: MaLo,
    /// Our own market-partner ID (the Lieferant).
    pub sender: MarktpartnerCode,
    /// Counterparty market-partner ID (NB or LFA).
    pub receiver: MarktpartnerCode,
    /// Requested supply start / end / cancellation date.
    pub process_date: String,
}

/// Process state for the GPKE LF-side Anmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub enum LfAnmeldungState {
    /// Initial state before `InitiateAnmeldung` is received.
    #[default]
    New,
    /// UTILMD Anfrage sent; awaiting NB/LFA response.
    Pending(LfAnmeldungData),
    /// NB or LFA accepted the Anmeldung; supply active (Lieferbeginn only).
    Active(LfAnmeldungData),
    /// NB rejected.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// GPKE Teil 2 Kap. 2.1.2 gives the NB its answer window on the Anmeldung and no consequence of the NB missing it. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<LfAnmeldungState>,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for LfAnmeldungState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // A lapsed window leaves the Vorgang open.
            Self::FristAbgelaufen { offen, .. } => offen.occupies_business_key(),
            // Awaiting the NB's answer, or supply is running — a second
            // Anmeldung for the same MaLo would put two live processes on one
            // supply point.
            Self::Pending(_) | Self::Active(_) => true,
            // `New` never started. `Rejected` is terminal: the NB refused, and
            // the LF's corrected Anmeldung is a normal GPKE flow that must be
            // allowed rather than answered with `duplicate_process`.
            Self::New | Self::Rejected { .. } => false,
        }
    }
}

impl LfAnmeldungState {
    fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Pending(_) => "Pending",
            Self::Active(_) => "Active",
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

/// What a 55001 states about the Lieferant's Kunde — each Muss in the column
/// (UTILMD AHB Strom 2.2 Kap. 8.2), each read at the boundary from the
/// Lieferant's contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kundendaten {
    /// `SG8 SEQ+Z01 SG10 CCI+++Z15`/`Z18` Gruppenzuordnung (nach EnWG), S. 83.
    pub haushaltskunde: bool,
    /// `SG8 SEQ+Z75 SG10 CCI+Z61` Grundlage zur Verringerung der Umlagen nach
    /// EnFG — `ZF9`, `ZG0` or `ZG1`, S. 85.
    pub enfg_grundlage: String,
    /// `SG10 CAV` Grund der Privilegierung nach EnFG, beside `ZF9` (`[463]`).
    pub enfg_grund: Option<String>,
    /// `SG12 NAD+Z09` Kunde des Lieferanten and `NAD+Z04` his
    /// Korrespondenzanschrift, S. 86.
    pub stammdaten: Stammdaten,
}

/// An Anmeldung neue Marktlokation 55600 / 55601 as the Lieferant sends it
/// (GPKE Teil 2 Kap. 2.2; UTILMD AHB Strom 2.2 Kap. 8.4, 8.5).
#[derive(Debug, Clone)]
pub struct NeuanlageAnmeldung {
    /// [`NEUANLAGE_VERBRAUCHEND_PID`] or [`NEUANLAGE_ERZEUGEND_PID`].
    pub pid: Pruefidentifikator,
    /// Our own market-partner ID (the Lieferant).
    pub sender: MarktpartnerCode,
    /// The Netzbetreiber, named by the caller: a Marktlokation not yet
    /// registered has no record to resolve it from.
    pub receiver: MarktpartnerCode,
    /// `SG5 LOC+Z16` — Soll „Wenn bekannt" (Bedingung `[165]`, S. 98, 112).
    pub marktlokation: Option<MaLo>,
    /// `SG4 DTM+92` Datum Vertragsbeginn, Muss.
    pub process_date: String,
    /// `SG4 IDE+24` — the NB's answer restates it in `SG6 RFF+TN`.
    pub vorgangsnummer: String,
    /// `SG8 SEQ+Z79` Produktpaket — the Bilanzkreis, Muss (Kap. 5.3).
    pub bilanzkreis: String,
    /// `SG4 STS+7` Ergänzung — one of [`neuanlage_ergaenzungen`].
    pub transaktionsgrund_ergaenzung: String,
    /// `SG8 SEQ+Z79` Produkt `9991000002090` — Muss in Geschäftsvorfall 3
    /// (`ZW2`) per Codeliste der Konfigurationen 1.4 Kap. 6.1.1.
    pub tranchengroesse: Option<mako_engine::payload::Tranchengroesse>,
    /// The Kunde's facts — Muss on the 55600 (`SG8 SEQ+Z01` Gruppenzuordnung,
    /// `SG12 NAD+Z09`/`Z04`, S. 105–107), listed on no 55601.
    pub kundendaten: Option<Kundendaten>,
}

/// Commands for the GPKE LF-side Anmeldung workflow.
#[derive(Clone)]
pub enum LfAnmeldungCommand {
    /// ERP instructs the engine to register a new Marktlokation with its
    /// Netzbetreiber — an Anmeldung neue Marktlokation 55600 / 55601.
    InitiateNeuanlage(Box<NeuanlageAnmeldung>),
    /// The NB's answer to an Anmeldung neue Marktlokation — 55602 / 55604 to
    /// a 55600, 55603 / 55605 to a 55601.
    HandleNeuanlageAntwort {
        /// PID of the inbound answer.
        response_pid: Pruefidentifikator,
        /// Optional reason from the answer's `FTX`.
        reason: Option<String>,
        /// Message reference from the inbound answer.
        response_ref: MessageRef,
        /// `SG4 STS+E01` DE 9013 and DE 1131 of the answer.
        antwort: Option<Antwortcode>,
        /// `SG5 LOC+Z16` — the Marktlokation the NB names, Muss on a
        /// Bestätigung (S. 98, 112).
        marktlokation: Option<MaLo>,
    },
    /// ERP instructs the engine to initiate a Lieferbeginn, Lieferende, or
    /// Kündigung Anmeldung as the Lieferant.
    ///
    /// The engine:
    /// 1. Records the `Initiated` event.
    /// 2. Enqueues a `PendingOutbox` entry with `message_type = "UTILMD"` and
    ///    the structured domain payload. The AS4 sender serialises this to
    ///    wire-format EDIFACT and delivers it to the NB via AS4.
    InitiateAnmeldung {
        /// Outbound request PID (55001, 55004, 55016, or 55077).
        pid: Pruefidentifikator,
        /// Our own market-partner ID (the Lieferant, from `--tenant-id`).
        sender: MarktpartnerCode,
        /// Counterparty market-partner ID (NB or LFA), resolved from the MaLo cache.
        receiver: MarktpartnerCode,
        /// Supply point identifier.
        location_id: MaLo,
        /// Requested process date (Lieferbeginn-/Lieferende-/Kündigungs-Datum).
        process_date: String,
        /// `SG4 STS+7` DE 9013 Transaktionsgrund — Muss on every Anfrage
        /// here, and refused unless its column admits it:
        /// [`ANMELDUNG_GRUENDE`], [`ANMELDUNG_ERZEUGEND_GRUND`],
        /// [`ABMELDUNG_GRUENDE`], [`KUENDIGUNG_GRUND`].
        transaktionsgrund: Option<String>,
        /// `SG8 SEQ+Z79` Produktpaket — the Bilanzkreis the LF registers the
        /// Marktlokation into.
        ///
        /// **Muss on 55001 and 55077** (UTILMD AHB Strom 2.2 Kap. 5.3): „ohne
        /// die Angabe eines für den LF gültigen Bilanzkreises `[kann]` der NB den
        /// LF der Marktlokation bzw. Tranche nicht zuordnen". `None` on 55004
        /// and 55016, which register nothing.
        bilanzkreis: Option<String>,
        /// `SG4 STS+7` DE 9013 element 3 — the Transaktionsgrundergänzung.
        ///
        /// Muss on every Anfrage here, from its column: on 55077 one of the
        /// three Geschäftsvorfälle, none of which is a default for another;
        /// on 55001 [`ANMELDUNG_ERGAENZUNGEN`], which the caller fills with
        /// [`ANMELDUNG_ERGAENZUNG`] when it states none.
        transaktionsgrund_ergaenzung: Option<String>,
        /// `SG8 SEQ+Z79` Produkt `9991000002090` — the **Tranchengröße**.
        ///
        /// Muss in Geschäftsvorfall 3 („`STS+7++xxx+ZW2`") per Codeliste der
        /// Konfigurationen 1.4 Kap. 6.1.1, and meaningless outside it. Carried
        /// verbatim to the renderer, which knows the three Produkteigenschaften
        /// and the Bedingungen on each.
        tranchengroesse: Option<serde_json::Value>,
        /// The Kunde's facts — **Muss on 55001**, `None` on every other
        /// Anfrage here, whose columns list none of them.
        kundendaten: Option<Box<Kundendaten>>,
    },
    /// Inbound NB/LFA response (55002/55003, 55005/55006, 55017, 55018, 55078, 55080) received via AS4.
    ///
    /// Dispatched by the AS4 inbound layer after extracting the domain fields
    /// from the UTILMD response message.
    HandleAntwort {
        /// PID of the inbound response.
        response_pid: Pruefidentifikator,
        /// `true` if the NB accepted; `false` if rejected.
        accepted: bool,
        /// Optional rejection reason from the UTILMD text segment.
        reason: Option<String>,
        /// Message reference from the inbound response.
        response_ref: MessageRef,
        /// `SG4 STS+E01` DE 9013 and DE 1131 of the response.
        antwort: Option<Antwortcode>,
    },
    /// Mark the accepted Lieferbeginn supply relationship as active.
    ///
    /// Typically dispatched after the ERP confirms supply activation downstream.
    Activate,
    /// A registered deadline fired — the NB let the published answer window
    /// lapse without a Bestätigung or Ablehnung.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for LfAnmeldungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE LF-side outbound Anmeldung workflow.
///
/// Handles the Lieferant's perspective of initiating a supplier-change process:
/// sends the UTILMD Anfrage to the NB and tracks the NB's response.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeLfAnmeldungWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-lf-anmeldung", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeLfAnmeldungWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The NB's answer window is open while the Anmeldung waits for it
/// (`Pending`).
fn window_open(label: &str, state: &LfAnmeldungState) -> bool {
    label == NB_RESPONSE_WINDOW_LABEL && matches!(state, LfAnmeldungState::Pending(_))
}

impl Workflow for GpkeLfAnmeldungWorkflow {
    type State = LfAnmeldungState;
    type Event = LfAnmeldungEvent;
    type Command = LfAnmeldungCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[NB_RESPONSE_WINDOW_LABEL];

    /// Deadline compensation for the NB/LFA answer window.
    ///
    /// | Label | State guard | Command emitted | Fundstelle |
    /// |---|---|---|---|
    /// | [`NB_RESPONSE_WINDOW_LABEL`] | `Pending` | `TimeoutExpired` | BK6-24-174 GPKE Teil 2 — the per-PID Antwortfrist |
    ///
    /// The label must be exactly this constant. `makod` registered
    /// `nb-response-window-24h` here, which fell through to `_ => None`: the
    /// deadline fired, nothing happened, and the process sat in `Pending`
    /// forever. `services/makod/tests/it/deadline_labels.rs` pins the two ends
    /// together.
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| LfAnmeldungCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            LfAnmeldungState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            LfAnmeldungEvent::Initiated {
                pruefidentifikator,
                location_id,
                sender,
                receiver,
                process_date,
            } => LfAnmeldungState::Pending(LfAnmeldungData {
                pruefidentifikator: *pruefidentifikator,
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                process_date: process_date.clone(),
            }),
            LfAnmeldungEvent::AntwortReceived {
                accepted, reason, ..
            } => {
                if *accepted {
                    match state {
                        LfAnmeldungState::Pending(data) => LfAnmeldungState::Active(data),
                        other => other,
                    }
                } else {
                    LfAnmeldungState::Rejected {
                        reason: reason.clone().unwrap_or_else(|| "Ablehnung".to_owned()),
                    }
                }
            }
            LfAnmeldungEvent::MarktlokationBenannt { marktlokation } => match state {
                LfAnmeldungState::Pending(mut data) => {
                    data.location_id = marktlokation.clone();
                    LfAnmeldungState::Pending(data)
                }
                other => other,
            },
            LfAnmeldungEvent::Activated => match state {
                LfAnmeldungState::Active(data) => LfAnmeldungState::Active(data),
                other => other,
            },
            LfAnmeldungEvent::DeadlineExpired { label, .. } => match state {
                LfAnmeldungState::Pending(_) => LfAnmeldungState::FristAbgelaufen {
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
        // A lapsed window changes nothing about what the process accepts; only
        // the timeout itself asks about the lapse.
        let lapsed = state;
        let state = state.offen();
        match command {
            LfAnmeldungCommand::InitiateAnmeldung {
                pid,
                sender,
                receiver,
                location_id,
                process_date,
                transaktionsgrund,
                bilanzkreis,
                transaktionsgrund_ergaenzung,
                tranchengroesse,
                kundendaten,
            } => {
                if !matches!(state, LfAnmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ANFRAGE_PIDS_LF.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an LF Anfrage PID (55001, 55004, 55016, 55077), got {pid}",
                    )));
                }
                // An Anmeldung without a Bilanzkreis is not a message the NB
                // can act on. UTILMD AHB Strom 2.2 Kap. 5.3 makes `SG8 SEQ+Z79`
                // Muss on 55001 and 55077 and names the reason: „ohne die
                // Angabe eines für den LF gültigen Bilanzkreises [kann] der NB
                // den LF der Marktlokation bzw. Tranche nicht zuordnen."
                // Refusing here beats sending an AHB-invalid Anmeldung and
                // waiting a Werktag for the APERAK.
                if ANMELDUNG_PIDS_MIT_BILANZKREIS.contains(&pid.as_u32())
                    && bilanzkreis.as_deref().is_none_or(str::is_empty)
                {
                    return Err(WorkflowError::rejected(format!(
                        "Anmeldung {pid} ohne Bilanzkreis: die UTILMD AHB Strom 2.2 \
                         Kap. 5.3 macht das Produktpaket SG8 SEQ+Z79 mit dem Produkt-Code \
                         9991000002082 (Bilanzkreis) zur Muss-Angabe — ohne einen für den \
                         LF gültigen Bilanzkreis kann der NB die Zuordnung nicht vornehmen.",
                    )));
                }

                // The erzeugende Anmeldung states which Geschäftsvorfall it
                // is. Its column admits `ZW0`/`ZW1`/`ZW2` and nothing else, so
                // an unstated one cannot be defaulted: the renderer would fall
                // back to the verbrauchende `ZW4`, which 55077 forbids, and the
                // NB would refuse the message.
                if ANMELDUNG_PIDS_MIT_GESCHAEFTSVORFALL.contains(&pid.as_u32()) {
                    let erg = transaktionsgrund_ergaenzung.as_deref().unwrap_or("");
                    if !GESCHAEFTSVORFAELLE.contains(&erg) {
                        return Err(WorkflowError::rejected(format!(
                            "Anmeldung {pid} ohne Geschäftsvorfall: SG4 STS+7 DE 9013 \
                             lässt für die erzeugende Marktlokation nur {} zu — \
                             ZW0 (100%ige Zuordnung), ZW1 (bestehende Tranche) und \
                             ZW2 (neu zu bildende Tranche) sind verschiedene Vorgänge.",
                            GESCHAEFTSVORFAELLE.join("/"),
                        )));
                    }
                    // Codeliste der Konfigurationen 1.4 Kap. 6.1.1: „Im
                    // Geschäftsvorfall 3 … ist zwingend dieses Produkt
                    // anzugeben." No AHB Bedingung says so, so nothing
                    // downstream would catch its absence.
                    if erg == GESCHAEFTSVORFALL_3 && tranchengroesse.is_none() {
                        return Err(WorkflowError::rejected(format!(
                            "Anmeldung {pid} im Geschäftsvorfall 3 ohne Tranchengröße: \
                             die Codeliste der Konfigurationen 1.4 Kap. 6.1.1 macht den \
                             Produkt-Code 9991000002090 bei STS+7++xxx+ZW2 zur \
                             Muss-Angabe, weil die neu zu bildende Tranche sonst keine \
                             Größe hat.",
                        )));
                    }
                }

                // Every Anfrage this workflow sends makes `SG4 STS+7` Muss
                // with a Transaktionsgrund and an Ergänzung from its column; a
                // missing or foreign code is a message the NB or the LFA
                // refuses.
                let (gruende, ergaenzungen) = match pid.as_u32() {
                    ANMELDUNG_PID => (ANMELDUNG_GRUENDE, ANMELDUNG_ERGAENZUNGEN),
                    ANMELDUNG_ERZEUGEND_PID => {
                        (&[ANMELDUNG_ERZEUGEND_GRUND][..], GESCHAEFTSVORFAELLE)
                    }
                    ABMELDUNG_PID => (ABMELDUNG_GRUENDE, ABMELDUNG_ERGAENZUNGEN),
                    _ => (&[KUENDIGUNG_GRUND][..], KUENDIGUNG_ERGAENZUNGEN),
                };
                let grund = transaktionsgrund.as_deref().unwrap_or("");
                let erg = transaktionsgrund_ergaenzung.as_deref().unwrap_or("");
                if !gruende.contains(&grund) || !ergaenzungen.contains(&erg) {
                    return Err(WorkflowError::rejected(format!(
                        "{pid}: SG4 STS+7 ist Muss — Transaktionsgrund {} und \
                         Ergänzung {} (UTILMD AHB Strom 2.2), nicht {grund:?}/{erg:?}",
                        gruende.join("/"),
                        ergaenzungen.join("/"),
                    )));
                }
                // The 55001 names the Kunde: the Gruppenzuordnung, the
                // Grundlage nach EnFG and the Kunde des Lieferanten with his
                // Korrespondenzanschrift are Muss there (UTILMD AHB Strom 2.2
                // Kap. 8.2, S. 83–86).
                if pid.as_u32() == ANMELDUNG_PID && kundendaten.is_none() {
                    return Err(WorkflowError::rejected(format!(
                        "{pid}: SG8 SEQ+Z01 Gruppenzuordnung, SG8 SEQ+Z75 Grundlage nach EnFG \
                         und SG12 NAD+Z09/Z04 Kunde des Lieferanten sind Muss (UTILMD AHB \
                         Strom 2.2 Kap. 8.2)"
                    )));
                }
                let kundendaten = kundendaten.filter(|_| pid.as_u32() == ANMELDUNG_PID);
                let dtm_qualifier = (pid.as_u32() == ABMELDUNG_PID
                    && transaktionsgrund
                        .as_deref()
                        .is_some_and(|g| ABMELDUNG_AUFHEBUNG_GRUENDE.contains(&g)))
                .then(|| "92".to_owned());

                let event = LfAnmeldungEvent::Initiated {
                    pruefidentifikator: pid,
                    location_id: location_id.clone(),
                    sender: sender.clone(),
                    receiver: receiver.clone(),
                    process_date: process_date.clone(),
                };

                // Enqueue the outbound UTILMD as a PendingOutbox entry.
                //
                // The AS4 sender in `makod` picks up entries with
                // `message_type = "UTILMD"` and serialises the payload
                // to wire-format EDIFACT before handing it to the AS4
                // transport layer.
                // `document_date` and `message_ref` are intentionally omitted:
                // the renderer derives them at dispatch time (today / causation_event_id).
                // The Tranchengröße arrives as the ERP stated it; a shape the
                // Codeliste does not name is refused here, before the wire.
                let tranchengroesse = tranchengroesse
                    .map(serde_json::from_value::<mako_engine::payload::Tranchengroesse>)
                    .transpose()
                    .map_err(|e| {
                        WorkflowError::validation(format!(
                            "tranchengroesse: a string (prozentuale Aufteilung) or \
                             {{art, wert}} — {e}"
                        ))
                    })?;
                let outbox = PendingOutbox::typed(
                    receiver.as_str(),
                    &mako_engine::payload::UtilmdPayload {
                        pid: pid.as_u32(),
                        sender: sender.as_str().to_owned(),
                        receiver: Some(receiver.as_str().to_owned()),
                        malo: Some(location_id.as_str().to_owned()),
                        process_date: Some(process_date),
                        // SG4 STS Transaktionsgrund (E01 Ein-/Auszug, E03 Wechsel).
                        transaktionsgrund,
                        // `SG8 SEQ+Z79` / `SG10 CAV+ZV4` — the Bilanzkreis.
                        bilanzkreis,
                        // `SG4 STS+7` DE 9013 element 3 — the Geschäftsvorfall on
                        // the erzeugende Anmeldung, `ZW4` elsewhere.
                        transaktionsgrund_ergaenzung,
                        // `SG8 SEQ+Z79` Produkt 9991000002090 — Muss on ZW2.
                        tranchengroesse,
                        // `92` on an Aufhebung 55004 ([7]/[11]).
                        dtm_qualifier,
                        // The 55001's Kunde: `SG8 SEQ+Z01`, `SEQ+Z75`, `SG12`.
                        kundengruppe: kundendaten
                            .as_ref()
                            .map(|k| if k.haushaltskunde { "Z15" } else { "Z18" }.to_owned()),
                        enfg_grundlage: kundendaten.as_ref().map(|k| k.enfg_grundlage.clone()),
                        enfg_grund: kundendaten.as_ref().and_then(|k| k.enfg_grund.clone()),
                        stammdaten: kundendaten.map(|k| k.stammdaten).unwrap_or_default(),
                        ..Default::default()
                    },
                );

                Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
            }

            LfAnmeldungCommand::HandleAntwort {
                response_pid,
                accepted,
                reason,
                response_ref,
                antwort,
            } => {
                if !matches!(state, LfAnmeldungState::Pending(_)) {
                    return Err(WorkflowError::invalid_state("Pending", state.label()));
                }
                if !ANTWORT_PIDS_LF.contains(&response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an LF Antwort PID (55002/55003, 55005/55006, 55017, 55018, 55078, 55080), got {response_pid}",
                    )));
                }

                Ok(WorkflowOutput::with_outbox(
                    vec![LfAnmeldungEvent::AntwortReceived {
                        response_pid,
                        accepted,
                        reason: reason.clone(),
                        response_ref,
                        antwort: antwort.clone(),
                    }],
                    vec![process_completed(
                        state,
                        response_pid,
                        accepted,
                        reason.as_deref(),
                        antwort.as_ref(),
                        None,
                    )],
                ))
            }

            LfAnmeldungCommand::InitiateNeuanlage(anmeldung) => {
                if !matches!(state, LfAnmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                initiate_neuanlage(*anmeldung)
            }

            LfAnmeldungCommand::HandleNeuanlageAntwort {
                response_pid,
                reason,
                response_ref,
                antwort,
                marktlokation,
            } => {
                let LfAnmeldungState::Pending(data) = state else {
                    return Err(WorkflowError::invalid_state("Pending", state.label()));
                };
                // The answer must be one to the Anmeldung this process sent: a
                // 55602 answers a 55600 and nothing else.
                let Some(&(_, _, accepted)) =
                    NEUANLAGE_ANTWORTEN.iter().find(|(a, anmeldung, _)| {
                        *a == response_pid.as_u32()
                            && *anmeldung == data.pruefidentifikator.as_u32()
                    })
                else {
                    return Err(WorkflowError::rejected(format!(
                        "{response_pid} does not answer the Anmeldung {} this process sent",
                        data.pruefidentifikator
                    )));
                };
                let benannt = marktlokation
                    .filter(|m| accepted && !m.as_str().is_empty() && *m != data.location_id);
                let mut events = Vec::new();
                if let Some(m) = benannt.clone() {
                    events.push(LfAnmeldungEvent::MarktlokationBenannt { marktlokation: m });
                }
                events.push(LfAnmeldungEvent::AntwortReceived {
                    response_pid,
                    accepted,
                    reason: reason.clone(),
                    response_ref,
                    antwort: antwort.clone(),
                });
                let completed = process_completed(
                    state,
                    response_pid,
                    accepted,
                    reason.as_deref(),
                    antwort.as_ref(),
                    benannt.as_ref(),
                )
                .caused_by(events.len() - 1);
                Ok(WorkflowOutput::with_outbox(events, vec![completed]))
            }

            LfAnmeldungCommand::Activate => {
                if !matches!(state, LfAnmeldungState::Active(_)) {
                    return Err(WorkflowError::invalid_state("Active", state.label()));
                }
                // Already active — emit a no-op or idempotent event.
                // In practice Activate is sent after ERP confirms supply.
                Ok(vec![LfAnmeldungEvent::Activated].into())
            }

            LfAnmeldungCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![LfAnmeldungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

/// The Anmeldung neue Marktlokation a [`NeuanlageAnmeldung`] states, refused
/// where its column cannot carry it (UTILMD AHB Strom 2.2 Kap. 8.4, 8.5).
fn initiate_neuanlage(
    a: NeuanlageAnmeldung,
) -> Result<WorkflowOutput<LfAnmeldungEvent>, WorkflowError> {
    let pid = a.pid.as_u32();
    if !NEUANLAGE_PIDS_LF.contains(&pid) {
        return Err(WorkflowError::rejected(format!(
            "expected an Anmeldung neue Marktlokation (55600, 55601), got {pid}"
        )));
    }
    let ergaenzungen = neuanlage_ergaenzungen(pid);
    if !ergaenzungen.contains(&a.transaktionsgrund_ergaenzung.as_str()) {
        return Err(WorkflowError::rejected(format!(
            "{pid}: SG4 STS+7 ist Muss — Transaktionsgrund {NEUANLAGE_GRUND} und Ergänzung {} \
             (UTILMD AHB Strom 2.2 Kap. 8.4/8.5), nicht {:?}",
            ergaenzungen.join("/"),
            a.transaktionsgrund_ergaenzung,
        )));
    }
    // Kap. 5.3 makes the Produktpaket with the Bilanzkreis Muss on 55600 and
    // 55601 as on every Anmeldung.
    if a.bilanzkreis.trim().is_empty() {
        return Err(WorkflowError::rejected(format!(
            "{pid}: das Produktpaket SG8 SEQ+Z79 mit dem Bilanzkreis ist Muss \
             (UTILMD AHB Strom 2.2 Kap. 5.3)"
        )));
    }
    if a.transaktionsgrund_ergaenzung == GESCHAEFTSVORFALL_3 && a.tranchengroesse.is_none() {
        return Err(WorkflowError::rejected(format!(
            "{pid} im Geschäftsvorfall 3 ohne Tranchengröße: die Codeliste der \
             Konfigurationen 1.4 Kap. 6.1.1 macht den Produkt-Code 9991000002090 bei \
             STS+7++xxx+ZW2 zur Muss-Angabe"
        )));
    }
    if a.vorgangsnummer.trim().is_empty() {
        return Err(WorkflowError::rejected(format!(
            "{pid}: SG4 IDE+24 Vorgangsnummer ist Muss — die Antwort des NB nennt sie in \
             SG6 RFF+TN"
        )));
    }
    // The 55600 names the Kunde and his Gruppenzuordnung, both Muss (S. 104,
    // 106–107); the 55601 the Kunde at most (Kann, S. 122) and no
    // Gruppenzuordnung.
    let verbrauchend = pid == NEUANLAGE_VERBRAUCHEND_PID;
    if verbrauchend && a.kundendaten.is_none() {
        return Err(WorkflowError::rejected(
            "55600: SG8 SEQ+Z01 Gruppenzuordnung und SG12 NAD+Z09/Z04 Kunde des \
             Lieferanten sind Muss (UTILMD AHB Strom 2.2 Kap. 8.4)"
                .to_owned(),
        ));
    }
    let kundendaten = a.kundendaten;
    let location_id = a.marktlokation.clone().unwrap_or_else(|| MaLo::new(""));
    let event = LfAnmeldungEvent::Initiated {
        pruefidentifikator: a.pid,
        location_id,
        sender: a.sender.clone(),
        receiver: a.receiver.clone(),
        process_date: a.process_date.clone(),
    };
    let outbox = PendingOutbox::typed(
        a.receiver.as_str(),
        &mako_engine::payload::UtilmdPayload {
            pid,
            sender: a.sender.as_str().to_owned(),
            receiver: Some(a.receiver.as_str().to_owned()),
            malo: a
                .marktlokation
                .map(|m| m.as_str().to_owned())
                .filter(|m| !m.is_empty()),
            process_date: Some(a.process_date),
            vorgangsnummer: Some(a.vorgangsnummer),
            transaktionsgrund: Some(NEUANLAGE_GRUND.to_owned()),
            transaktionsgrund_ergaenzung: Some(a.transaktionsgrund_ergaenzung),
            bilanzkreis: Some(a.bilanzkreis),
            tranchengroesse: a.tranchengroesse,
            kundengruppe: kundendaten
                .as_ref()
                .filter(|_| verbrauchend)
                .map(|k| if k.haushaltskunde { "Z15" } else { "Z18" }.to_owned()),
            stammdaten: kundendaten.map(|k| k.stammdaten).unwrap_or_default(),
            ..Default::default()
        },
    );
    Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
}

/// The `ProcessCompleted` notification an answer leaves for marktd and the ERP.
///
/// `marktlokation` is the one the NB named in a Bestätigung of an Anmeldung
/// neue Marktlokation, where the process held none or another.
fn process_completed(
    state: &LfAnmeldungState,
    response_pid: Pruefidentifikator,
    accepted: bool,
    reason: Option<&str>,
    antwort: Option<&Antwortcode>,
    marktlokation: Option<&MaLo>,
) -> PendingOutbox {
    // Carry malo_id so marktd's event_ingest can derive
    // VersorgungsStatus without relying on the CE subject (a process
    // UUID). Mirrors the GeLi Gas LFN workflow — without this outbox
    // entry no `de.mako.process.completed` is ever emitted for a
    // Strom supplier change, so `lf_mp_id_next` stays announced and
    // never gets promoted to the active Lieferant.
    let malo_id_str = match (marktlokation, state) {
        (Some(m), _) => m.as_str().to_owned(),
        (None, LfAnmeldungState::Pending(data)) => data.location_id.as_str().to_owned(),
        _ => String::new(),
    };
    // `accepted`/`outcome` are the verdict, and they are the only
    // thing that distinguishes a Bestätigung from an Ablehnung
    // downstream: both leave through `de.mako.process.completed`,
    // whose type says only that the process is terminal. A consumer
    // keying on the type alone activates supply for a Marktlokation
    // the NB refused.
    //
    // `reason` rides along so the refusal explains itself where it
    // is read; without it a consumer knows a Lieferbeginn failed and
    // cannot say why.
    // The date the Anfrage asked for — the Lieferbeginn on an
    // Anmeldung, the Lieferende on an Abmeldung — and the Lieferant
    // it is about, this one: without them marktd dates an ending
    // the day the answer arrived and ends every assignment of a
    // tranchierte Marktlokation.
    let (process_date, lieferant) = match state {
        LfAnmeldungState::Pending(data) => (
            Some(data.process_date.clone()),
            Some(data.sender.as_str().to_owned()),
        ),
        _ => (None, None),
    };
    let lieferant_key = if matches!(response_pid.as_u32(), 55_005 | 55_006) {
        "old_supplier"
    } else {
        "new_supplier"
    };
    let mut completed = serde_json::json!({
        "pid":      response_pid.as_u32(),
        "malo_id":  malo_id_str,
        "process_date": process_date,
        "accepted": accepted,
        "outcome":  if accepted { "accepted" } else { "rejected" },
        "reason":   reason,
        "antwortcode": antwort.map(|a| a.code.as_str()),
        "codeliste":   antwort.and_then(|a| a.codeliste.as_deref()),
    });
    completed[lieferant_key] = serde_json::json!(lieferant);
    PendingOutbox::new("ProcessCompleted", "", completed)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{
        types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
        workflow::Workflow,
    };

    use super::*;

    fn make_initiate(pid: u32) -> LfAnmeldungCommand {
        // The Abmeldung and the Kündigung carry their Muss STS+7.
        let (grund, erg) = match pid {
            ABMELDUNG_PID => (Some("ZT5".to_owned()), Some("ZW4".to_owned())),
            KUENDIGUNG_PID => (Some(KUENDIGUNG_GRUND.to_owned()), Some("ZW4".to_owned())),
            ANMELDUNG_PID => (Some("E03".to_owned()), Some("ZW4".to_owned())),
            _ => (Some(ANMELDUNG_ERZEUGEND_GRUND.to_owned()), None),
        };
        LfAnmeldungCommand::InitiateAnmeldung {
            transaktionsgrund_ergaenzung: erg,
            tranchengroesse: None,
            pid: Pruefidentifikator::new(pid).unwrap(),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            location_id: MaLo::new("10001234558"),
            process_date: "2026-10-01".to_owned(),
            transaktionsgrund: grund,
            bilanzkreis: Some("11XBK-LF-------9".to_owned()),
            kundendaten: (pid == ANMELDUNG_PID).then(|| Box::new(kundendaten())),
        }
    }

    /// A Haushaltskunde without an EnFG privilege.
    fn kundendaten() -> Kundendaten {
        Kundendaten {
            haushaltskunde: true,
            enfg_grundlage: "ZG1".to_owned(),
            enfg_grund: None,
            stammdaten: mako_engine::payload::Stammdaten::default(),
        }
    }

    fn with_codes(pid: u32, grund: Option<&str>, erg: Option<&str>) -> LfAnmeldungCommand {
        let LfAnmeldungCommand::InitiateAnmeldung {
            pid: p,
            sender,
            receiver,
            location_id,
            process_date,
            bilanzkreis,
            tranchengroesse,
            kundendaten,
            ..
        } = make_initiate(pid)
        else {
            unreachable!()
        };
        LfAnmeldungCommand::InitiateAnmeldung {
            pid: p,
            sender,
            receiver,
            location_id,
            process_date,
            transaktionsgrund: grund.map(str::to_owned),
            bilanzkreis,
            transaktionsgrund_ergaenzung: erg.map(str::to_owned),
            tranchengroesse,
            kundendaten,
        }
    }

    /// `SG4 STS+7` is Muss on the 55004 and the 55016, Grund and Ergänzung
    /// each from its column; anything else is refused before the wire.
    #[test]
    fn the_abmeldung_and_kuendigung_need_their_column_codes() {
        let state = LfAnmeldungState::New;
        for (pid, grund, erg) in [
            (ABMELDUNG_PID, None, Some("ZW4")),
            (ABMELDUNG_PID, Some("ZT5"), None),
            (ABMELDUNG_PID, Some("E03"), Some("ZW4")),
            (ABMELDUNG_PID, Some("ZT5"), Some("ZW0")),
            (KUENDIGUNG_PID, Some("ZT5"), Some("ZW4")),
            (KUENDIGUNG_PID, Some("E03"), Some("ZAP")),
        ] {
            assert!(
                GpkeLfAnmeldungWorkflow::handle(&state, with_codes(pid, grund, erg)).is_err(),
                "{pid} {grund:?}/{erg:?}"
            );
        }
    }

    /// The Anmeldung 55001 makes `SG4 STS+7` Muss with `E01`/`E03` and
    /// `ZW4`/`ZAP` (UTILMD AHB Strom 2.2 Kap. 8.2, S. 73); the erzeugende
    /// 55077 admits `E03` alone (Kap. 8.3, S. 90). An Anmeldung without its
    /// Grund, or with one its column does not list, never reaches the wire.
    #[test]
    fn an_anmeldung_needs_a_grund_its_column_admits() {
        let state = LfAnmeldungState::New;
        for (grund, erg) in [("E01", "ZW4"), ("E03", "ZW4"), ("E03", "ZAP")] {
            let out = GpkeLfAnmeldungWorkflow::handle(
                &state,
                with_codes(ANMELDUNG_PID, Some(grund), Some(erg)),
            )
            .unwrap_or_else(|e| panic!("55001 {grund}/{erg}: {e}"));
            assert_eq!(out.outbox[0].payload["transaktionsgrund"], grund);
            assert_eq!(out.outbox[0].payload["transaktionsgrund_ergaenzung"], erg);
        }
        for (pid, grund, erg) in [
            (ANMELDUNG_PID, None, Some("ZW4")),
            (ANMELDUNG_PID, Some(""), Some("ZW4")),
            (ANMELDUNG_PID, Some("E02"), Some("ZW4")),
            (ANMELDUNG_PID, Some("ZT5"), Some("ZW4")),
            (ANMELDUNG_PID, Some("E03"), None),
            (ANMELDUNG_PID, Some("E03"), Some("ZW3")),
            (ANMELDUNG_ERZEUGEND_PID, Some("E01"), Some("ZW0")),
            (ANMELDUNG_ERZEUGEND_PID, None, Some("ZW0")),
        ] {
            assert!(
                GpkeLfAnmeldungWorkflow::handle(&state, with_codes(pid, grund, erg)).is_err(),
                "{pid} {grund:?}/{erg:?}"
            );
        }
    }

    /// An Aufhebung 55004 names the confirmed Beginn (`DTM+92`, `[7]`); every
    /// other Grund the Ende (`[11]`).
    #[test]
    fn an_aufhebung_55004_names_the_beginn() {
        let state = LfAnmeldungState::New;
        for (grund, q) in [
            ("ZG9", Some("92")),
            ("ZH2", Some("92")),
            ("ZT5", None),
            ("E01", None),
        ] {
            let out = GpkeLfAnmeldungWorkflow::handle(
                &state,
                with_codes(ABMELDUNG_PID, Some(grund), Some("ZW4")),
            )
            .expect("written");
            let payload = &out.outbox[0].payload;
            assert_eq!(
                payload.get("dtm_qualifier").and_then(|v| v.as_str()),
                q,
                "{grund}"
            );
        }
    }

    /// An erzeugende Anmeldung with its Geschäftsvorfall, and — on
    /// Geschäftsvorfall 3 — the Tranchengröße the Codeliste demands.
    fn make_erzeugende(erg: &str, groesse: Option<serde_json::Value>) -> LfAnmeldungCommand {
        let LfAnmeldungCommand::InitiateAnmeldung {
            pid,
            sender,
            receiver,
            location_id,
            process_date,
            transaktionsgrund,
            bilanzkreis,
            ..
        } = make_initiate(55077)
        else {
            unreachable!("make_initiate builds an InitiateAnmeldung")
        };
        LfAnmeldungCommand::InitiateAnmeldung {
            pid,
            sender,
            receiver,
            location_id,
            process_date,
            transaktionsgrund,
            bilanzkreis,
            transaktionsgrund_ergaenzung: Some(erg.to_owned()),
            tranchengroesse: groesse,
            kundendaten: None,
        }
    }

    /// 55077's `SG4 STS+7` DE 9013 column admits `ZW0`/`ZW1`/`ZW2` and nothing
    /// else. Without one the renderer would fall back to the verbrauchende
    /// `ZW4`, which that column forbids — the NB refuses the message and the LF
    /// loses the Anmeldefrist.
    #[test]
    fn an_erzeugende_anmeldung_states_its_geschaeftsvorfall() {
        let err = GpkeLfAnmeldungWorkflow::handle(&LfAnmeldungState::New, make_initiate(55077))
            .expect_err("55077 without a Geschäftsvorfall must be refused");
        assert!(format!("{err}").contains("Geschäftsvorfall"), "{err}");

        for erg in GESCHAEFTSVORFAELLE {
            let groesse = (*erg == GESCHAEFTSVORFALL_3).then(|| serde_json::json!("33.33"));
            let out = GpkeLfAnmeldungWorkflow::handle(
                &LfAnmeldungState::New,
                make_erzeugende(erg, groesse),
            )
            .unwrap_or_else(|e| panic!("{erg} is a lawful Geschäftsvorfall: {e}"));
            assert_eq!(out.outbox[0].payload["transaktionsgrund_ergaenzung"], *erg);
        }
    }

    /// Codeliste der Konfigurationen 1.4 Kap. 6.1.1: „Im Geschäftsvorfall 3 …
    /// ist zwingend dieses Produkt anzugeben." No AHB Bedingung says so, so the
    /// message would validate clean and still be refused by the NB.
    #[test]
    fn geschaeftsvorfall_3_carries_a_tranchengroesse() {
        let err = GpkeLfAnmeldungWorkflow::handle(
            &LfAnmeldungState::New,
            make_erzeugende(GESCHAEFTSVORFALL_3, None),
        )
        .expect_err("ZW2 without a Tranchengröße must be refused");
        assert!(format!("{err}").contains("9991000002090"), "{err}");

        let out = GpkeLfAnmeldungWorkflow::handle(
            &LfAnmeldungState::New,
            make_erzeugende(GESCHAEFTSVORFALL_3, Some(serde_json::json!("33.33"))),
        )
        .expect("a stated Tranchengröße is accepted");
        assert_eq!(out.outbox[0].payload["tranchengroesse"], "33.33");

        // The other two Geschäftsvorfälle build no Tranche and carry none.
        for erg in [GESCHAEFTSVORFALL_1, GESCHAEFTSVORFALL_2] {
            let out =
                GpkeLfAnmeldungWorkflow::handle(&LfAnmeldungState::New, make_erzeugende(erg, None))
                    .expect("no Tranchengröße is owed");
            assert!(out.outbox[0].payload["tranchengroesse"].is_null());
        }
    }

    #[test]
    fn initiate_lieferbeginn_transitions_to_pending() {
        let state = LfAnmeldungState::New;
        let out = GpkeLfAnmeldungWorkflow::handle(&state, make_initiate(55001)).unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.outbox.len(), 1, "must enqueue UTILMD outbox entry");

        let new_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(new_state, LfAnmeldungState::Pending(_)));
    }

    #[test]
    fn initiate_abmeldung_transitions_to_pending() {
        let state = LfAnmeldungState::New;
        let out = GpkeLfAnmeldungWorkflow::handle(&state, make_initiate(55004)).unwrap();
        let new_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(new_state, LfAnmeldungState::Pending(_)));
    }

    #[test]
    fn initiate_kuendigung_transitions_to_pending() {
        let state = LfAnmeldungState::New;
        let out = GpkeLfAnmeldungWorkflow::handle(&state, make_initiate(55016)).unwrap();
        let new_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(new_state, LfAnmeldungState::Pending(_)));
    }

    #[test]
    fn nb_acceptance_transitions_to_active() {
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);

        let cmd = LfAnmeldungCommand::HandleAntwort {
            response_pid: Pruefidentifikator::new(55002).unwrap(), // Bestätigung Anmeldung
            accepted: true,
            reason: None,
            response_ref: MessageRef::new("NB-RESP-001"),
            antwort: None,
        };
        let out = GpkeLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(out.events.len(), 1);
        let final_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(final_state, LfAnmeldungState::Active(_)));
    }

    /// `HandleAntwort` must enqueue the `ProcessCompleted` outbox entry that
    /// drives marktd's `VersorgungsStatus` transition.
    ///
    /// Without it no `de.mako.process.completed` is emitted for a Strom
    /// supplier change, so `lf_mp_id_next` is announced and never promoted —
    /// the MaLo stays `Unbeliefert` forever. The Gas twin
    /// (`geli-gas-lf-anmeldung`) always emitted this; GPKE did not.
    ///
    /// marktd keys on the PID: 55002 confirms, 55003 clears the announcement.
    #[test]
    fn handle_antwort_emits_process_completed_for_marktd() {
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);

        for (response_pid, accepted, outcome) in
            [(55002u32, true, "accepted"), (55003, false, "rejected")]
        {
            let out = GpkeLfAnmeldungWorkflow::handle(
                &state,
                LfAnmeldungCommand::HandleAntwort {
                    response_pid: Pruefidentifikator::new(response_pid).unwrap(),
                    accepted,
                    reason: None,
                    response_ref: MessageRef::new("NB-RESP-001"),
                    antwort: None,
                },
            )
            .unwrap();

            assert_eq!(out.outbox.len(), 1, "{response_pid}: one outbox entry");
            let entry = &out.outbox[0];
            assert_eq!(entry.message_type.as_ref(), "ProcessCompleted");
            assert_eq!(
                entry.payload["pid"].as_u64().unwrap(),
                u64::from(response_pid)
            );
            assert_eq!(
                entry.payload["malo_id"].as_str().unwrap(),
                "10001234558",
                "{response_pid}: marktd resolves the MaLo from the payload, not the CE subject",
            );
            assert_eq!(entry.payload["outcome"].as_str().unwrap(), outcome);
        }
    }

    #[test]
    fn nb_rejection_transitions_to_rejected() {
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);

        let cmd = LfAnmeldungCommand::HandleAntwort {
            response_pid: Pruefidentifikator::new(55003).unwrap(), // Ablehnung Anmeldung
            accepted: false,
            reason: Some("MaLo nicht in Netzgebiet".to_owned()),
            response_ref: MessageRef::new("NB-RESP-002"),
            antwort: None,
        };
        let out = GpkeLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        let final_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(final_state, LfAnmeldungState::Rejected { .. }));
    }

    #[test]
    fn invalid_pid_is_rejected() {
        let state = LfAnmeldungState::New;
        // 55003 is a response PID, not an Anfrage PID
        let err = GpkeLfAnmeldungWorkflow::handle(&state, make_initiate(55003));
        assert!(err.is_err());
    }

    /// The NB's silence is a Frist breach the LF sees, not a rejection: no
    /// Festlegung gives it an outcome, and the NB's late answer still lands.
    #[test]
    fn timeout_on_pending_is_a_frist_breach() {
        use mako_engine::ids::DeadlineId;
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);

        let cmd = LfAnmeldungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: NB_RESPONSE_WINDOW_LABEL.into(),
        };
        let out = GpkeLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        let final_state = GpkeLfAnmeldungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(
            final_state,
            LfAnmeldungState::FristAbgelaufen { .. }
        ));
        assert!(
            mako_engine::workflow::OccupiesBusinessKey::occupies_business_key(&final_state),
            "a lapsed Anmeldung still holds its MaLo"
        );
    }

    #[test]
    fn timeout_on_active_is_noop() {
        use mako_engine::ids::DeadlineId;
        let initiated_event = LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
            location_id: MaLo::new("10001234558"),
            sender: MarktpartnerCode::new("4012345000009"),
            receiver: MarktpartnerCode::new("9900123456789"),
            process_date: "2026-10-01".to_owned(),
        };
        let state = GpkeLfAnmeldungWorkflow::apply(LfAnmeldungState::New, &initiated_event);
        let accepted_event = LfAnmeldungEvent::AntwortReceived {
            response_pid: Pruefidentifikator::new(55003).unwrap(),
            accepted: true,
            reason: None,
            response_ref: MessageRef::new("REF-002"),
            antwort: None,
        };
        let state = GpkeLfAnmeldungWorkflow::apply(state, &accepted_event);
        assert!(matches!(state, LfAnmeldungState::Active(_)));

        let cmd = LfAnmeldungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: NB_RESPONSE_WINDOW_LABEL.into(),
        };
        let out = GpkeLfAnmeldungWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(out.events.len(), 0, "timeout is no-op on Active");
    }
}
