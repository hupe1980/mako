//! GPKE Neuanlage — new Marktlokation registration process.
//!
//! Covers the GPKE Neuanlage process per BNetzA BK6-24-174 Anlage 1b (GPKE
//! Teil 2 Kap. 2.2): registration of a **new** Marktlokation not previously in the
//! grid topology. This is distinct from Lieferbeginn (PID 55001) which assumes
//! the location already exists.
//!
//! This module implements the **receiving-party perspective** (Netzbetreiber / NB):
//! the system receives an inbound Anmeldung Neuanlage from the Lieferant/Einspeiser
//! and responds with Bestätigung or Ablehnung.
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom 2.1/2.2)
//!
//! ## Inbound ANFRAGE — routed to `gpke-neuanlage`
//!
//! | PID   | Process name (AHB)                               | Direction |
//! |-------|--------------------------------------------------|-----------|
//! | 55600 | Anmeldung neue verb. MaLo (LF → NB)             | LF → NB   |
//! | 55601 | Anmeldung neue erz. MaLo (LF → NB)              | LF → NB   |
//!
//! ## Outbound ANTWORT — derived by this workflow
//!
//! An inbound one is the answer to the Lieferant's own Anmeldung and routes to
//! `gpke-lf-anmeldung`.
//!
//! | PID   | Process name (AHB)                               | Derived from  |
//! |-------|--------------------------------------------------|---------------|
//! | 55602 | Bestätigung Anmeldung neue verb. MaLo (NB → LF) | 55600 accepted|
//! | 55603 | Bestätigung Anmeldung neue erz. MaLo (NB → LF)  | 55601 accepted|
//! | 55604 | Ablehnung Anmeldung neue verb. MaLo (NB → LF)   | 55600 rejected|
//! | 55605 | Ablehnung Anmeldung neue erz. MaLo (NB → LF)    | 55601 rejected|
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 1b** — GPKE Teil 2 Kap. 2.2 Neuanlage
//! - **UTILMD S2.1/S2.2** — EDI@Energy message format
//! - **APERAK AHB 1.1 Kap. 2.4.1** — technical acknowledgement, **45 Minuten** on a
//!   weekday for a UTILMD. A separate clock from the business answer window,
//!   which GPKE Teil 2 states as a wall-clock instant on the 1. Werktag nach
//!   dem ÜT.

use mako_engine::envelope::{Richtung, Verlauf};
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
pub const WORKFLOW_NAME: &str = "gpke-neuanlage";

/// Inbound ANFRAGE PIDs for Neuanlage handled by [`GpkeNeuanlageWorkflow`].
///
/// | PID   | Process (AHB name)                                  | AHB profile  |
/// |-------|-----------------------------------------------------|--------------|
/// | 55600 | Anmeldung neue verb. MaLo (LF → NB)                | S2.1, S2.2   |
/// | 55601 | Anmeldung neue erz. MaLo (LF → NB)                 | S2.1, S2.2   |
///
/// The answers 55602–55605 are derived here; inbound, they answer the
/// Lieferant's own Anmeldung and route to `gpke-lf-anmeldung`.
pub const NEUANLAGE_PIDS: &[u32] = &[55600, 55601];

/// The Anmeldung neue **erzeugende** Marktlokation, whose Bestätigung 55603
/// restates its Geschäftsvorfall.
pub const NEUANLAGE_ERZEUGEND_PID: u32 = 55601;

/// Deadline label for the **business** answer window — „spätester ÜZ ist 00:00
/// Uhr des 61. WT nach dem ÜT" (GPKE Teil 2 Kap. 2.2.2), resolved by
/// `mako_fristen::antwort`: `E_0608` Prüfschritte 110/590 make identifying a
/// newly commissioned Marktlokation a daily re-check for up to 60 Werktage.
///
/// Not the APERAK clock: that is 45 Minuten for a UTILMD (APERAK AHB 1.1
/// Kap. 2.4.1) and rides `mako_fristen::APERAK_STROM_WINDOW_LABEL`.
///
/// Register immediately after `ValidationPassed`:
/// ```rust,ignore
/// let due = mako_fristen::antwort::antwort_deadline(pid, received_at)
///     .expect("a PID with a published Antwortfrist");
/// let dl = Deadline::new(stream_id, ..., NEUANLAGE_ANTWORT_WINDOW_LABEL, due);
/// deadline_store.register(&dl).await?;
/// ```
pub const NEUANLAGE_ANTWORT_WINDOW_LABEL: &str = "gpke-neuanlage-antwortfrist";

// ── Response PID derivation ───────────────────────────────────────────────────

/// Derive the outbound UTILMD response PID for a Neuanlage Anfrage.
///
/// | Anfrage | accepted | Response PID |
/// |---------|----------|--------------|
/// | 55600   | true     | 55602        |
/// | 55600   | false    | 55604        |
/// | 55601   | true     | 55603        |
/// | 55601   | false    | 55605        |
fn neuanlage_response_pid(anfrage_pid: u32, accepted: bool) -> Option<Pruefidentifikator> {
    let code: u32 = match anfrage_pid {
        55600 => {
            if accepted {
                55602
            } else {
                55604
            }
        }
        55601 => {
            if accepted {
                55603
            } else {
                55605
            }
        }
        _ => return None,
    };
    Pruefidentifikator::new(code).ok()
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Neuanlage workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum NeuanlageEvent {
    /// UTILMD Neuanlage Anfrage received.
    AnmeldungErhalten {
        /// Marktlokation EIC code (to be assigned).
        location_id: MaLo,
        /// GLN of the Lieferant / Einspeiser.
        sender: MarktpartnerCode,
        /// GLN of the Netzbetreiber.
        receiver: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// Requested connection date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (55600 or 55601).
        pruefidentifikator: Pruefidentifikator,
        /// `SG4 IDE+24` DE 7402 of the Anmeldung — the answer's `SG6 RFF+TN`.
        #[serde(default)]
        vorgangsnummer: Option<String>,
        /// `SG4 STS+7` Transaktionsgrund of the Anmeldung, which the answer
        /// restates.
        #[serde(default)]
        transaktionsgrund: Option<String>,
        /// `SG4 STS+7` Ergänzung of the Anmeldung — on a 55601 its
        /// Geschäftsvorfall `ZW0` / `ZW2`, which the Bestätigung 55603 restates.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transaktionsgrund_ergaenzung: Option<String>,
        /// `SG8 SEQ+Z79` DE 1050 — the Produktpaket-ID the Anmeldung offers,
        /// which a Bestätigung names in `SG6 RFF+Z60` unless the NB names
        /// another.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        produktpaket_id: Option<String>,
    },
    /// EDIFACT message passed profile validation (no rule violations).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound UTILMD response (55602–55605) was sent.
    AntwortGesendet {
        /// Derived outbound response PID.
        response_pid: Option<Pruefidentifikator>,
        /// `true` = accepted (Bestätigung), `false` = rejected (Ablehnung).
        accepted: bool,
        /// Rejection reason (set only when `accepted = false`).
        reason: Option<String>,
        /// The Marktlokations-ID the NB identified, where the Anmeldung named
        /// none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        marktlokation: Option<MaLo>,
    },
    /// Neuanlage completed — MaLo is active in the grid.
    Aktiviert,
    /// Process closed due to validation failure, rejection, or deadline expiry.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired before the process completed.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for NeuanlageEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnmeldungErhalten { .. } => "NeuanlageAnmeldungErhalten",
            Self::ValidationPassed { .. } => "NeuanlageValidationPassed",
            Self::AntwortGesendet { .. } => "NeuanlageAntwortGesendet",
            Self::Aktiviert => "NeuanlageAktiviert",
            Self::Rejected { .. } => "NeuanlageRejected",
            Self::DeadlineExpired { .. } => "NeuanlageDeadlineExpired",
        }
    }

    /// An `AntwortGesendet` without a `response_pid` rendered no UTILMD —
    /// `handle` queues the answer only for a resolved PID — so it states none.
    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnmeldungErhalten {
                pruefidentifikator,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AntwortGesendet { response_pid, .. } => response_pid.map(|p| Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(p.as_u32()),
                message_ref: None,
            }),
            Self::Aktiviert | Self::DeadlineExpired { .. } => None,
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
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `AnmeldungErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NeuanlageData {
    /// EIC/MaLo code of the new location.
    pub location_id: MaLo,
    /// GLN of the requesting Lieferant / Einspeiser.
    pub sender: MarktpartnerCode,
    /// GLN of the responsible Netzbetreiber.
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// Requested connection date (`YYYYMMDD`).
    pub process_date: String,
    /// BDEW Prüfidentifikator (55600 or 55601).
    pub pruefidentifikator: Pruefidentifikator,
    /// `SG4 IDE+24` DE 7402 of the Anmeldung. Every answer references it in
    /// `SG6 RFF+TN` (UTILMD AHB Strom 2.2, Muss on 55602–55605).
    #[serde(default)]
    pub vorgangsnummer: Option<String>,
    /// `SG4 STS+7` Transaktionsgrund of the Anmeldung (`E02`), which every
    /// answer restates; the Ergänzung is the answer's own (`ZW3`/`ZW4` on an
    /// Ablehnung, the column fixes it).
    #[serde(default)]
    pub transaktionsgrund: Option<String>,
    /// `SG4 STS+7` Ergänzung of the Anmeldung — `ZW4` on a 55600, the
    /// Geschäftsvorfall `ZW0` / `ZW2` on a 55601.
    #[serde(default)]
    pub transaktionsgrund_ergaenzung: Option<String>,
    /// `SG8 SEQ+Z79` DE 1050 — the Produktpaket-ID the Anmeldung offers.
    #[serde(default)]
    pub produktpaket_id: Option<String>,
}

/// State of a GPKE Neuanlage process.
///
/// # Lifecycle
///
/// ```text
/// New → Eingegangen → ValidationPassed → AntwortGesendet → Aktiviert
///                                      ↘ Rejected
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum NeuanlageState {
    /// No events yet.
    #[default]
    New,
    /// Anfrage received; AHB validation pending.
    Eingegangen(NeuanlageData),
    /// Validation passed; response not yet sent.
    ValidationPassed(NeuanlageData),
    /// Response sent; awaiting MaLo activation.
    AntwortGesendet {
        /// Data from the Anfrage.
        data: NeuanlageData,
        /// Derived outbound response PID.
        response_pid: Option<Pruefidentifikator>,
    },
    /// MaLo is active in the grid.
    Aktiviert(NeuanlageData),
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// GPKE Teil 2 Kap. 2.2.2 gives the NB its answer window on the Anmeldung einer Neuanlage and no consequence of missing it. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<NeuanlageState>,
    },
}

/// Whether a received Neuanlage still holds its Marktlokation against a new one.
///
/// Live while the answer is owed or the accepted Vorgang awaits its
/// activation; settled once refused or activated. A settled process must not
/// take the next Neuanlage for the same Marktlokation — its receive command
/// refuses anything but `New`.
impl mako_engine::workflow::OccupiesBusinessKey for NeuanlageState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // A lapsed window leaves the Vorgang open.
            Self::FristAbgelaufen { offen, .. } => offen.occupies_business_key(),
            Self::Eingegangen(_) | Self::ValidationPassed(_) | Self::AntwortGesendet { .. } => true,
            Self::New | Self::Aktiviert(_) | Self::Rejected { .. } => false,
        }
    }
}

impl NeuanlageState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AntwortGesendet { .. } => "AntwortGesendet",
            Self::Aktiviert(_) => "Aktiviert",
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

    /// Return `Some(&NeuanlageData)` if the process has been initiated.
    #[must_use]
    pub fn data(&self) -> Option<&NeuanlageData> {
        match self {
            Self::FristAbgelaufen { offen, .. } => offen.data(),
            Self::Eingegangen(d) | Self::ValidationPassed(d) | Self::Aktiviert(d) => Some(d),
            Self::AntwortGesendet { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Neuanlage workflow.
#[derive(Clone)]
pub enum NeuanlageCommand {
    /// Inbound UTILMD Neuanlage Anfrage received from the AS4 layer.
    ReceiveAnmeldung {
        /// BDEW Prüfidentifikator (55600 or 55601).
        pid: Pruefidentifikator,
        /// GLN of the sender (LF).
        sender: MarktpartnerCode,
        /// GLN of the receiver (NB).
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// Requested connection date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings (for the `Rejected` event).
        validation_errors: Vec<String>,
        /// The Anmeldung's `SG4` facts — its Vorgangsnummer, Transaktionsgrund
        /// and Ergänzung.
        vorgang: Box<crate::LfVorgangsdaten>,
        /// `SG8 SEQ+Z79` DE 1050 — the Produktpaket-ID the Anmeldung offers.
        produktpaket_id: Option<String>,
    },
    /// Send the outbound UTILMD response to the Lieferant.
    ///
    /// Derive the response PID from the Anfrage PID:
    /// - 55600 → 55602 (accepted) / 55604 (rejected)
    /// - 55601 → 55603 (accepted) / 55605 (rejected)
    ///
    /// The Neuanlage answer window is **00:00 Uhr des 61. Werktags nach dem
    /// ÜT** (BK6-24-174 GPKE Teil 2 Kap. 2.2.2): `E_0608` Prüfschritte 110 und 590
    /// give the NB 60 Werktage of daily re-identification before it may refuse a
    /// newly commissioned Marktlokation. A 24-hour deadline on this process
    /// manufactures a rejection roughly three months early.
    SendAntwort {
        /// The NB's answer: the resolved `E_0608` Antwortcode and its cluster.
        ///
        /// `SG4 STS+E01` is Muss on every Antwortnachricht, and the published
        /// **Cluster** selects the response PID — `A09` / `A18` ride 55602 /
        /// 55603, every other code rides 55604 / 55605.
        antwort: crate::LfAntwort,
        /// The Marktlokations-ID the NB identified. An Anmeldung neue
        /// Marktlokation names it only „Wenn bekannt" (UTILMD AHB Strom 2.2
        /// Kap. 8.4, S. 98, Bedingung `[165]`), and the Bestätigung's `SG5
        /// LOC+Z16` is Muss — so where the Anmeldung named none, the answer
        /// names the one the NB assigned. One that contradicts the
        /// Anmeldung's is refused.
        marktlokation: Option<MaLo>,
        /// The Tranche a Bestätigung 55603 of Geschäftsvorfall 3 (`ZW2`)
        /// names — Muss there (Bedingung `[478]`, UTILMD AHB Strom 2.2 S. 113,
        /// 119), refused on any other answer.
        tranche: Option<Box<NeuanlageTranche>>,
    },
    /// Mark the new Marktlokation as active in the grid.
    Aktivieren,
    /// A registered deadline fired; record expiry and close the process.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for NeuanlageCommand {}

/// The Tranche a Bestätigung Anmeldung neue erzeugende Marktlokation 55603
/// names in Geschäftsvorfall 3 (UTILMD AHB Strom 2.2 Kap. 8.5): `SG5 LOC+Z21`
/// (S. 113) and the `SG8 SEQ+Z15` „Daten der Tranche" (S. 119–120).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NeuanlageTranche {
    /// `SG5 LOC+Z21` DE 3225 — the Tranche's ID, in the format of a
    /// Marktlokations-ID (`[950]`).
    pub tranche_id: MaLo,
    /// `SG10 CCI+Z37` DE 7037 — Basis zur Bildung der Tranchengröße: `ZD1`
    /// prozentual, `ZD2` Aufteilungsfaktor auf Basis von Referenzenträger /
    /// installierter Leistung, `ZH1` Aufteilung auf Technische Ressourcen.
    pub basis: String,
    /// `SG9 QTY+11` DE 6060 — the Tranche's share in per cent, Muss beside
    /// `ZD1` (`[384]`): > 0, < 100, at most two Nachkommastellen (`[914]`,
    /// `[955]`, `[930]`).
    pub prozent: Option<String>,
}

/// `p` as `[914]`, `[955]` and `[930]` admit it: a decimal number (`.` as
/// the decimal mark) above 0 and below 100 with at most two Nachkommastellen.
fn prozent_gueltig(p: &str) -> bool {
    let (ganz, bruch) = p.split_once('.').unwrap_or((p, ""));
    let ziffern = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    !ganz.is_empty()
        && ziffern(ganz)
        && ziffern(bruch)
        && bruch.len() <= 2
        && p.contains('.') != bruch.is_empty()
        && ganz.parse::<u32>().is_ok_and(|g| g < 100)
        && (ganz.bytes().any(|b| b != b'0') || bruch.bytes().any(|b| b != b'0'))
}

/// The Geschäftsvorfall-3 Ergänzung whose Bestätigung names a Tranche.
const GESCHAEFTSVORFALL_3: &str = "ZW2";
/// `SG10 CCI+Z37` `ZD1` — the Tranchengröße is a percentage.
const BASIS_PROZENTUAL: &str = "ZD1";
/// The bases `SG10 CCI+Z37` DE 7037 admits on a 55603 (S. 120).
const TRANCHEN_BASEN: &[&str] = &[BASIS_PROZENTUAL, "ZD2", "ZH1"];

impl NeuanlageTranche {
    /// What keeps it from the column — empty when it can be sent.
    fn maengel(&self) -> Vec<String> {
        let mut m = Vec::new();
        if self.tranche_id.as_str().trim().is_empty() {
            m.push("tranche_id (SG5 LOC+Z21) is empty".to_owned());
        }
        if !TRANCHEN_BASEN.contains(&self.basis.as_str()) {
            m.push(format!(
                "basis {:?} (SG10 CCI+Z37): one of {}",
                self.basis,
                TRANCHEN_BASEN.join("/")
            ));
        }
        match (self.basis == BASIS_PROZENTUAL, self.prozent.as_deref()) {
            (true, None) => {
                m.push("prozent (SG9 QTY+11) is Muss beside ZD1 (Bedingung [384])".to_owned());
            }
            (true, Some(p)) => {
                if !prozent_gueltig(p) {
                    m.push(format!(
                        "prozent {p:?} (SG9 QTY+11): > 0, < 100, at most two \
                         Nachkommastellen (Bedingungen [914], [955], [930])"
                    ));
                }
            }
            (false, Some(_)) => m.push(
                "prozent (SG9 QTY+11) is listed beside ZD1 only (Bedingung [384])".to_owned(),
            ),
            (false, None) => {}
        }
        m
    }

    /// The `SG5` and `SG8` segments it rides as.
    fn stammdaten(&self) -> mako_engine::payload::Stammdaten {
        use mako_engine::payload::{
            Stammdaten, StammdatenBlock, StammdatenLokation, StammdatenSegment,
        };
        let mut segmente = Vec::new();
        if let Some(p) = &self.prozent {
            segmente.push(StammdatenSegment {
                tag: "QTY".to_owned(),
                elemente: vec![vec!["11".to_owned(), p.clone(), "P1".to_owned()]],
            });
        }
        segmente.push(StammdatenSegment {
            tag: "CCI".to_owned(),
            elemente: vec![vec!["Z37".to_owned()], vec![], vec![self.basis.clone()]],
        });
        Stammdaten {
            lokationen: vec![StammdatenLokation {
                qualifier: "Z21".to_owned(),
                id: self.tranche_id.as_str().to_owned(),
            }],
            bloecke: vec![StammdatenBlock {
                seq: "Z15".to_owned(),
                segmente,
            }],
            ..Stammdaten::default()
        }
    }
}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Neuanlage workflow (PIDs 55600/55601).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeNeuanlageWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-neuanlage", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeNeuanlageWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while the NB still owes its answer
/// (`Eingegangen`, `ValidationPassed`).
fn window_open(label: &str, state: &NeuanlageState) -> bool {
    label == NEUANLAGE_ANTWORT_WINDOW_LABEL
        && matches!(
            state,
            NeuanlageState::Eingegangen(_) | NeuanlageState::ValidationPassed(_)
        )
}

impl Workflow for GpkeNeuanlageWorkflow {
    type State = NeuanlageState;
    type Event = NeuanlageEvent;
    type Command = NeuanlageCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[NEUANLAGE_ANTWORT_WINDOW_LABEL];

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| NeuanlageCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            NeuanlageState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            NeuanlageEvent::AnmeldungErhalten {
                location_id,
                sender,
                receiver,
                document_date,
                process_date,
                pruefidentifikator,
                vorgangsnummer,
                transaktionsgrund,
                transaktionsgrund_ergaenzung,
                produktpaket_id,
                ..
            } => NeuanlageState::Eingegangen(NeuanlageData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                vorgangsnummer: vorgangsnummer.clone(),
                transaktionsgrund: transaktionsgrund.clone(),
                transaktionsgrund_ergaenzung: transaktionsgrund_ergaenzung.clone(),
                produktpaket_id: produktpaket_id.clone(),
            }),
            NeuanlageEvent::ValidationPassed { .. } => match state {
                NeuanlageState::Eingegangen(data) => NeuanlageState::ValidationPassed(data),
                other => other,
            },
            NeuanlageEvent::AntwortGesendet {
                accepted,
                response_pid,
                marktlokation,
                ..
            } => {
                if *accepted {
                    match state {
                        NeuanlageState::ValidationPassed(mut data) => {
                            if let Some(malo) = marktlokation {
                                data.location_id = malo.clone();
                            }
                            NeuanlageState::AntwortGesendet {
                                response_pid: *response_pid,
                                data,
                            }
                        }
                        other => other,
                    }
                } else {
                    NeuanlageState::Rejected {
                        reason: "Neuanlage abgelehnt".to_owned(),
                    }
                }
            }
            NeuanlageEvent::Aktiviert => match state {
                NeuanlageState::AntwortGesendet { data, .. } => NeuanlageState::Aktiviert(data),
                other => other,
            },
            NeuanlageEvent::Rejected { reason } => NeuanlageState::Rejected {
                reason: reason.clone(),
            },
            NeuanlageEvent::DeadlineExpired { label, .. } => match state {
                NeuanlageState::Eingegangen(_) | NeuanlageState::ValidationPassed(_) => {
                    NeuanlageState::FristAbgelaufen {
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
            NeuanlageCommand::ReceiveAnmeldung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                validation_passed,
                validation_errors,
                vorgang,
                produktpaket_id,
            } => {
                if !matches!(state, NeuanlageState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !NEUANLAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected Neuanlage PID (55600 or 55601), got {pid}",
                    )));
                }
                // Clone before move for APERAK emission in the validation-failed path.
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![NeuanlageEvent::AnmeldungErhalten {
                    location_id,
                    sender,
                    receiver,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                    vorgangsnummer: vorgang.vorgangsnummer.clone(),
                    transaktionsgrund: vorgang.transaktionsgrund.clone(),
                    transaktionsgrund_ergaenzung: vorgang.transaktionsgrund_ergaenzung.clone(),
                    produktpaket_id,
                }];
                if validation_passed {
                    events.push(NeuanlageEvent::ValidationPassed {
                        message_ref: message_ref.clone(),
                    });
                    // APERAK BGM+312 (Anerkennungsmeldung) — mandatory per APERAK AHB 1.1 Kap. 2.4.
                    // Strom UTILMD: 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.1 Kap. 2.4.1).
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
                    events.push(NeuanlageEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // APERAK BGM+313 — mandatory per APERAK AHB 1.1 Kap. 2.1.1.
                    // Strom UTILMD: 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.1 Kap. 2.4.1).
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

            NeuanlageCommand::SendAntwort {
                antwort,
                marktlokation,
                tranche,
            } => {
                let mut data = match state {
                    NeuanlageState::ValidationPassed(d) => d.clone(),
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                let identifiziert = match marktlokation.filter(|m| !m.as_str().is_empty()) {
                    Some(m) if data.location_id.as_str().is_empty() => {
                        data.location_id = m.clone();
                        Some(m)
                    }
                    Some(m) if m != data.location_id => {
                        return Err(WorkflowError::rejected(format!(
                            "the Anmeldung names Marktlokation {}, the answer {m}",
                            data.location_id
                        )));
                    }
                    _ => None,
                };
                let accepted = antwort.zustimmung;
                let response_pid =
                    neuanlage_response_pid(data.pruefidentifikator.as_u32(), accepted);
                // Geschäftsvorfall 3 builds a Tranche, and its Bestätigung
                // names it (`SG5 LOC+Z21` and `SG8 SEQ+Z15` Muss behind `ZW2`,
                // Bedingung `[478]`, S. 113, 119); no other answer lists either.
                let geschaeftsvorfall_3 = accepted
                    && data.pruefidentifikator.as_u32() == NEUANLAGE_ERZEUGEND_PID
                    && antwort
                        .malo_art
                        .as_deref()
                        .or(data.transaktionsgrund_ergaenzung.as_deref())
                        == Some(GESCHAEFTSVORFALL_3);
                match (&tranche, geschaeftsvorfall_3) {
                    (None, true) => {
                        return Err(WorkflowError::rejected(
                            "a ZW2 Bestätigung 55603 names its Tranche: SG5 LOC+Z21 and SG8 \
                             SEQ+Z15 are Muss (UTILMD AHB Strom 2.2 Bedingung [478], S. 113, \
                             119) — tranche"
                                .to_owned(),
                        ));
                    }
                    (Some(_), false) => {
                        return Err(WorkflowError::rejected(
                            "a Tranche (SG5 LOC+Z21, SG8 SEQ+Z15) is listed on a ZW2 \
                             Bestätigung 55603 only (Bedingung [478]) — tranche"
                                .to_owned(),
                        ));
                    }
                    (Some(t), true) => {
                        let m = t.maengel();
                        if !m.is_empty() {
                            return Err(WorkflowError::rejected(format!(
                                "tranche: {}",
                                m.join("; ")
                            )));
                        }
                    }
                    (None, false) => {}
                }
                let events = vec![NeuanlageEvent::AntwortGesendet {
                    response_pid,
                    accepted,
                    reason: antwort.bemerkung.clone(),
                    marktlokation: identifiziert,
                }];

                // The answer is a message, not just an event: an
                // `AntwortGesendet` with no outbox entry records the process as
                // answered while the Lieferant watches its 61-Werktage window
                // expire.
                let mut outbox = Vec::new();
                // `SG5 LOC+Z16` names a Marktlokation the NB has. On the
                // Ablehnung of a neue erzeugende Marktlokation 55605 it is Muss
                // only beside `A12`/`A13` and admitted nowhere else (Bedingung
                // `[43]`, UTILMD AHB Strom 2.2) — any other refusal names none.
                let nennt_marktlokation = !data.location_id.as_str().is_empty()
                    && (response_pid.map(Pruefidentifikator::as_u32) != Some(55_605)
                        || matches!(antwort.antwort_code.as_str(), "A12" | "A13"));
                if let Some(rpid) = response_pid {
                    let mut payload = mako_engine::payload::UtilmdPayload {
                        pid: rpid.as_u32(),
                        sender: data.receiver.as_str().to_owned(),
                        receiver: Some(data.sender.as_str().to_owned()),
                        malo: nennt_marktlokation.then(|| data.location_id.as_str().to_owned()),
                        process_date: Some(data.process_date.clone()),
                        // `SG4 STS+E01++<code>:E_0608` — Muss on every
                        // Antwortnachricht.
                        antwort_code: Some(antwort.antwort_code.clone()),
                        antwort_codeliste: antwort.ebd.clone(),
                        bemerkung: antwort.bemerkung.clone(),
                        // `SG4 STS+7` and `SG6 RFF+TN` — the Anmeldung's
                        // Grund and Vorgangsnummer, Muss on every answer.
                        transaktionsgrund: data.transaktionsgrund.clone(),
                        referenz_vorgangsnummer: data.vorgangsnummer.clone(),
                        ..Default::default()
                    };
                    if accepted {
                        bestaetigung_lokationsdaten(&mut payload, &data, &antwort);
                    }
                    if let Some(t) = &tranche {
                        payload.stammdaten = t.stammdaten();
                    }
                    outbox.push(PendingOutbox::typed(data.sender.as_str(), &payload));
                }
                Ok(WorkflowOutput::with_outbox(events, outbox))
            }

            NeuanlageCommand::Aktivieren => {
                if !matches!(state, NeuanlageState::AntwortGesendet { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.label(),
                    ));
                }
                Ok(vec![NeuanlageEvent::Aktiviert].into())
            }

            NeuanlageCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(vec![].into());
                }
                Ok(vec![NeuanlageEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

/// What a Bestätigung 55602 / 55603 tells the LF of the new Marktlokation
/// beyond the Antwortcode (UTILMD AHB Strom 2.2 Kap. 8.4 and 8.5).
///
/// - `SG4 STS+7` Ergänzung: on the 55602 the NB's classification, `ZW6`
///   „Pauschale Marktlokation" or `ZW7` „Gemessene Marktlokation" (S. 97) —
///   stated by the NB, or `ZW7` when it names Messlokationen; naming none is
///   not a classification, so the Ergänzung stays unstated and the message
///   does not render. On the 55603 the Anmeldung's Geschäftsvorfall `ZW0` /
///   `ZW2` (S. 111), which the NB restates unless it states its own.
/// - `SG6 RFF+Z60`: the Produktpaket the NB will implement (S. 100, 115) —
///   the one the Anmeldung offered unless the NB names another.
/// - `SG5 LOC+Z17` / `SG8 SEQ+Z98` / `SEQ+ZF3`: the Messlokationen and their
///   Messstellenbetreiber, carried as the NB states them; the pre-send check
///   says where the column makes them Muss.
fn bestaetigung_lokationsdaten(
    payload: &mut mako_engine::payload::UtilmdPayload,
    data: &NeuanlageData,
    antwort: &crate::LfAntwort,
) {
    let ergaenzung = antwort.malo_art.clone().or_else(|| {
        if data.pruefidentifikator.as_u32() == NEUANLAGE_ERZEUGEND_PID {
            data.transaktionsgrund_ergaenzung.clone()
        } else {
            (!antwort.messlokationen.is_empty())
                .then(|| crate::wechselprozesse::GEMESSENE_MALO.to_owned())
        }
    });
    if let Some(erg) = ergaenzung {
        payload.transaktionsgrund_ergaenzung = Some(erg);
    }
    payload.geplantes_produktpaket = antwort
        .geplantes_produktpaket
        .clone()
        .or_else(|| data.produktpaket_id.clone());
    payload.messlokationen = antwort
        .messlokationen
        .iter()
        .map(crate::AntwortMesslokation::to_payload)
        .collect();
    if let Some(msb) = &antwort.malo_msb {
        payload.malo_msb = Some(msb.to_payload());
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

    fn anmeldung_cmd(pid_code: u32, ok: bool) -> NeuanlageCommand {
        NeuanlageCommand::ReceiveAnmeldung {
            pid: pid(pid_code),
            sender: mcod("4012345000023"),
            receiver: mcod("9900357000004"),
            location_id: malo("51238696781"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            message_ref: mref("NEUA-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
            vorgang: Box::default(),
            produktpaket_id: None,
        }
    }

    #[test]
    fn verlauf_states_the_inbound_anmeldung_and_the_outbound_answer() {
        assert_eq!(
            NeuanlageEvent::Rejected {
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
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55600, true))
            .unwrap();
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(55_600),
                message_ref: Some("NEUA-001".to_owned()),
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
        assert!(
            out.outbox
                .iter()
                .any(|o| &*o.message_type == "APERAK" && o.caused_by_event_index == 1),
            "the APERAK 312 is caused by ValidationPassed"
        );
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::SendAntwort {
                antwort: crate::LfAntwort::zustimmung("A09", "E_0608"),
                marktlokation: None,
                tranche: None,
            },
        )
        .unwrap();
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(55_602),
                message_ref: None,
            })
        );
        let unrendered = NeuanlageEvent::AntwortGesendet {
            response_pid: None,
            accepted: true,
            reason: None,
            marktlokation: None,
        };
        assert_eq!(unrendered.verlauf(), None, "no PID, no UTILMD rendered");
        assert_eq!(NeuanlageEvent::Aktiviert.verlauf(), None);
    }

    #[test]
    fn neuanlage_55600_happy_path() {
        // Step 1: receive 55600 Anmeldung
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55600, true))
            .unwrap();
        assert_eq!(out.events.len(), 2); // AnmeldungErhalten + ValidationPassed
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        assert!(matches!(state, NeuanlageState::ValidationPassed(_)));

        // Step 2: send Bestätigung
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::SendAntwort {
                antwort: crate::LfAntwort::zustimmung("A09", "E_0608"),
                marktlokation: None,
                tranche: None,
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        if let NeuanlageEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(accepted);
            assert_eq!(response_pid.map(Pruefidentifikator::as_u32), Some(55602));
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = out.events.iter().fold(state, GpkeNeuanlageWorkflow::apply);
        assert!(matches!(state, NeuanlageState::AntwortGesendet { .. }));

        // Step 3: activate
        let out = GpkeNeuanlageWorkflow::handle(&state, NeuanlageCommand::Aktivieren).unwrap();
        let state = out.events.iter().fold(state, GpkeNeuanlageWorkflow::apply);
        assert!(matches!(state, NeuanlageState::Aktiviert(_)));
    }

    #[test]
    fn neuanlage_55601_rejected() {
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55601, true))
            .unwrap();
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::SendAntwort {
                antwort: crate::LfAntwort::ablehnung("A03", "E_0608"),
                marktlokation: None,
                tranche: None,
            },
        )
        .unwrap();
        if let NeuanlageEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(!accepted);
            assert_eq!(response_pid.map(Pruefidentifikator::as_u32), Some(55605));
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = out.events.iter().fold(state, GpkeNeuanlageWorkflow::apply);
        assert!(matches!(state, NeuanlageState::Rejected { .. }));
    }

    #[test]
    fn neuanlage_validation_failure_rejects() {
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55600, false))
            .unwrap();
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        assert!(matches!(state, NeuanlageState::Rejected { .. }));
    }

    #[test]
    fn neuanlage_wrong_pid_rejected() {
        let result = GpkeNeuanlageWorkflow::handle(
            &NeuanlageState::New,
            NeuanlageCommand::ReceiveAnmeldung {
                pid: pid(55001), // wrong PID
                sender: mcod("4012345000023"),
                receiver: mcod("9900357000004"),
                location_id: malo("51238696781"),
                document_date: "20251001".to_owned(),
                process_date: "20260101".to_owned(),
                message_ref: mref("NEUA-X"),
                validation_passed: true,
                validation_errors: vec![],
                vorgang: Box::default(),
                produktpaket_id: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn timeout_in_terminal_state_is_noop() {
        let state = NeuanlageState::Aktiviert(NeuanlageData {
            location_id: malo("51238696781"),
            sender: mcod("4012345000023"),
            receiver: mcod("9900357000004"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            pruefidentifikator: pid(55600),
            vorgangsnummer: None,
            transaktionsgrund: None,
            transaktionsgrund_ergaenzung: None,
            produktpaket_id: None,
        });
        let dl_id = DeadlineId::new();
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::TimeoutExpired {
                deadline_id: dl_id,
                label: NEUANLAGE_ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty());
    }

    /// An Anmeldung that named no Marktlokation is answered with the one the
    /// NB identified — the Bestätigung's `SG5 LOC+Z16` is Muss — and an answer
    /// naming another than the Anmeldung named is refused.
    #[test]
    fn the_answer_names_the_marktlokation_the_nb_identified() {
        let ohne = match anmeldung_cmd(55600, true) {
            NeuanlageCommand::ReceiveAnmeldung {
                pid,
                sender,
                receiver,
                document_date,
                process_date,
                message_ref,
                validation_passed,
                validation_errors,
                vorgang,
                produktpaket_id,
                ..
            } => NeuanlageCommand::ReceiveAnmeldung {
                pid,
                sender,
                receiver,
                location_id: malo(""),
                document_date,
                process_date,
                message_ref,
                validation_passed,
                validation_errors,
                vorgang,
                produktpaket_id,
            },
            other => other,
        };
        let fold = |cmd| {
            let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, cmd).unwrap();
            out.events
                .iter()
                .fold(NeuanlageState::New, GpkeNeuanlageWorkflow::apply)
        };
        let antwort = |marktlokation| NeuanlageCommand::SendAntwort {
            antwort: crate::LfAntwort::zustimmung("A09", "E_0608").with_klassifizierung("ZW6"),
            marktlokation,
            tranche: None,
        };
        let out =
            GpkeNeuanlageWorkflow::handle(&fold(ohne), antwort(Some(malo("51238696012")))).unwrap();
        assert_eq!(out.outbox[0].payload["malo"], "51238696012");

        let mit = fold(anmeldung_cmd(55600, true));
        assert!(
            GpkeNeuanlageWorkflow::handle(&mit, antwort(Some(malo("51238696012")))).is_err(),
            "the Anmeldung named 51238696781"
        );
        let out = GpkeNeuanlageWorkflow::handle(&mit, antwort(None)).unwrap();
        assert_eq!(out.outbox[0].payload["malo"], "51238696781");
    }
}
