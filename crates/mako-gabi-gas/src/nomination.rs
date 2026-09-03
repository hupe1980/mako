//! GaBi Gas Nomination workflow — NOMINT / NOMRES (BKV ↔ FNB / MGV).
//!
//! Implements the gas nomination and confirmation cycle governed by the
//! Kooperationsvereinbarung Gas (KoV) and the BNetzA GaBi Gas 2.1 framework
//! (BK7-24-01-008).
//!
//! # Process overview
//!
//! The BKV submits a **nomination** (NOMINT) to the FNB or MGV by D-1 13:00 CET.
//! The FNB / MGV responds with a **nomination response** (NOMRES) confirming,
//! curtailing, or rejecting the submitted quantities.
//!
//! ```text
//! Transportkunde ──(NOMINT 70030–70034)──→  NB / MGV
//! NB / MGV ──(NOMRES 70035–70039)──→  Transportkunde
//! ```
//!
//! # Prüfidentifikatoren
//!
//! DVGW publishes real Prüfidentifikatoren in `SG1 RFF+Z13`; `dvgw-edi` reads
//! them off the wire. The catalogue below is a projection of
//! [`dvgw_edi::catalogue_for`] and is pinned to it by a test in this module.
//!
//! | PID | Message | Anwendungsfall | Richtung |
//! |---|---|---|---|
//! | 70030 | NOMINT | Nominierung an einem physikalischen Punkt (ungebündelt) | Transportkunde an NB |
//! | 70031 | NOMINT | Nominierung an einem virtuellen Handelspunkt | Transportkunde an MGV |
//! | 70032 | NOMINT | Flexibilitätsübertragung | Transportkunde an NB |
//! | 70033 | NOMINT | Gebündelte Nominierung | Transportkunde an NB |
//! | 70034 | NOMINT | Nominierungsweitergabe zwischen Netzbetreibern | NB an NB |
//! | 70035 | NOMRES | Matching Benachrichtigung | NB an Transportkunde |
//! | 70036 | NOMRES | Bestätigung | NB an Transportkunde |
//! | 70037 | NOMRES | VHP Matching Benachrichtigung | MGV an Transportkunde |
//! | 70038 | NOMRES | VHP Bestätigung | MGV an Transportkunde |
//! | 70039 | NOMRES | Bestätigung Flexibilitätsübertragung | NB an Transportkunde |
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ Open (the NOMINT this tenant sent, or the one it received)
//!       ├─ Accepted   (NOMRES status = Accepted)           [terminal]
//!       ├─ PartiallyAccepted (NOMRES with curtailment)      [terminal]
//!       ├─ Rejected   (NOMRES status = Rejected)            [terminal]
//!       └─ DeadlineExpired (no response before D+1)         [terminal]
//! ```
//!
//! # Regulatory basis
//!
//! - **Kooperationsvereinbarung Gas (KoV)** — nomination deadlines, curtailment rules
//! - **BNetzA BK7-24-01-008** — GaBi Gas 2.1 ruling
//! - **DVGW NOMINT 4.6 FK** / **NOMRES 4.7 FK** — message format (valid from 2026-02-01)

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::MessageRef,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

use crate::domain::{GasDay, NominationQuantity};

// ── Prüfidentifikator set ─────────────────────────────────────────────────────

/// Every DVGW Prüfidentifikator that routes to the nomination workflow.
///
/// See the module docs for the Anwendungsfall behind each code.
pub const NOMINATION_PIDS: &[u32] = &[
    70030, 70031, 70032, 70033, 70034, 70035, 70036, 70037, 70038, 70039,
];

/// Outbound NOMINT — the Transportkunde nominates.
pub const NOMINT_PIDS: &[u32] = &[70030, 70031, 70032, 70033, 70034];

/// Inbound NOMRES — the NB or MGV answers.
pub const NOMRES_PIDS: &[u32] = &[70035, 70036, 70037, 70038, 70039];

/// Workflow key for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-nomination";

/// Deadline label for the NOMRES response window.
///
/// Per the Kooperationsvereinbarung Gas, the FNB/MGV must respond to a
/// nomination by **15:00 CET on gas day D-1** (i.e. within ~2 h of the
/// nomination deadline). Register a [`mako_engine::deadline::Deadline`] with
/// this label immediately after the `NominationSent` event is persisted.
pub const NOMRES_DEADLINE_LABEL: &str = "gabi-gas-nomres-response-deadline";

// ── Direction / counterparty role ─────────────────────────────────────────────

/// Whether this nomination is directed to an FNB or MGV.
///
/// Derived from the NOMINT role qualifier (Z01 = FNB, Z02 = MGV) and stored
/// in every event for auditability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NominationCounterparty {
    /// The network operator (FNB/VNB) — nominations at a physical point.
    Fnb,
    /// The Marktgebietsverantwortlicher — nominations at the virtual trading point.
    Mgv,
}

impl NominationCounterparty {
    /// Derive from the Prüfidentifikator.
    ///
    /// The virtual-trading-point Anwendungsfälle (70031 nominate at the VHP,
    /// 70037/70038 answer for it) are the MGV's; the physical-point ones are the
    /// network operator's. Returns `None` for a code outside the nomination set.
    #[must_use]
    pub fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            70031 | 70037 | 70038 => Some(Self::Mgv),
            70030 | 70032 | 70033 | 70034 | 70035 | 70036 | 70039 => Some(Self::Fnb),
            _ => None,
        }
    }
}

// ── Acceptance status (mirrors NomresStatus from dvgw-edi) ───────────────────

/// Overall acceptance status of the NOMRES received from FNB/MGV.
///
/// This is a domain-layer re-encoding of `dvgw_edi::messages::nomres::NomresStatus`
/// so that the process event log is independent of the parsing library.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NomresAcceptance {
    /// Nomination accepted in full.
    Accepted,
    /// Nomination partially accepted (quantities curtailed by FNB/MGV).
    PartiallyAccepted,
    /// Nomination rejected.
    Rejected,
    /// Status not mapped to a known variant (raw code preserved).
    Other(String),
}

impl NomresAcceptance {
    /// Human-readable display string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Accepted => "Accepted",
            Self::PartiallyAccepted => "PartiallyAccepted",
            Self::Rejected => "Rejected",
            Self::Other(code) => code.as_str(),
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when the BKV submits a NOMINT nomination.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NominationData {
    /// Which end of the nomination this process holds.
    pub richtung: NominationRichtung,
    /// The Prüfidentifikator that initiated this nomination (70030–70034).
    pub pruefidentifikator: u32,
    /// Whether the counterparty is an FNB or MGV.
    pub counterparty: NominationCounterparty,
    /// EIC code of the sending BKV.
    pub sender_eic: String,
    /// EIC code of the receiving FNB/MGV.
    pub receiver_eic: String,
    /// Gas day for this nomination.
    pub gas_day: GasDay,
    /// NOMINT document reference (from BGM element 1 — used for NOMRES correlation).
    pub nomination_ref: MessageRef,
    /// Nominated quantity with optional NOMRES acceptance breakdown.
    ///
    /// `None` when the nomination message did not carry an explicit quantity
    /// (e.g. a cancellation or renomination-to-zero).
    pub quantity: Option<NominationQuantity>,

    /// The positions this tenant nominated — what its NOMINT states, empty
    /// for a nomination received from a counterparty.
    pub positions: Vec<NominationPosition>,

    /// Reference to the prior NOMINT that this re-nomination corrects.
    ///
    /// Per KoV §3.2: the BKV may submit corrections within the intraday
    /// re-nomination window. Each correcting NOMINT references the previous
    /// NOMINT's `nomination_ref` via this field, creating an auditable
    /// nomination correction chain.
    ///
    /// `None` for the initial (day-ahead D-1 13:00 CET) nomination.
    pub corrects_nomination_ref: Option<MessageRef>,

    /// Sequence number of this nomination in the correction chain.
    ///
    /// 0 = initial day-ahead nomination, 1 = first intraday correction, etc.
    pub correction_sequence: u32,
}

/// Which end of the nomination a process holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NominationRichtung {
    /// This tenant nominated (BKV): the NOMINT went out, the NOMRES is owed.
    Gesendet,
    /// A Transportkunde nominated at this tenant (NB/MGV): the NOMRES is ours to send.
    Empfangen,
}

/// One `LIN` position of a NOMINT: a point, a direction, the Bilanzkreise and
/// the rates per period (NOMINT 4.6 §2 — `LOC`, `QTY`, `SG41 NAD`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NominationPosition {
    /// `LOC` DE 3227 — `Z19` Netzkopplungspunkt, `172` Meldepunkt, `Z17` Marktlokation.
    pub ort_qualifier: String,
    /// `LOC` DE 3225 — the point's code.
    pub ort: String,
    /// `QTY` DE 6063 — `Z02` Einspeisung or `Z03` Ausspeisung.
    pub richtung: String,
    /// `SG41 NAD+ZEU` — Bilanzkreis des internen Transportkunden.
    pub bilanzkreis_intern: String,
    /// `SG41 NAD+ZES` — Bilanzkreis des externen Transportkunden, where the
    /// nomination names one.
    pub bilanzkreis_extern: Option<String>,
    /// The rates, one `LOC`/`DTM+2`/`QTY` group per period.
    pub mengen: Vec<NominationMenge>,
}

/// One rate of a nomination: kWh/h over `[von, bis)`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NominationMenge {
    /// Start of the period, UTC.
    pub von: time::OffsetDateTime,
    /// End of the period, exclusive, UTC.
    pub bis: time::OffsetDateTime,
    /// `QTY` DE 6060 in `KW1` — kilowatt-hours per hour.
    pub kwh_pro_h: rust_decimal::Decimal,
}

impl NominationMenge {
    /// The energy of this rate over its period, in kWh.
    #[must_use]
    pub fn energie_kwh(&self) -> rust_decimal::Decimal {
        let seconds = rust_decimal::Decimal::from((self.bis - self.von).whole_seconds().max(0));
        self.kwh_pro_h * seconds / rust_decimal::Decimal::from(3600)
    }
}

/// A re-nomination names the nomination it corrects (`RFF+AGO`) and when
/// that one was processed (`DTM+9`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Renominierung {
    /// The corrected nomination's Dokumentennummer.
    pub nomination_ref: MessageRef,
    /// When the corrected nomination was processed, UTC.
    pub processed_at: time::OffsetDateTime,
}

/// The process key a nomination is filed under — the business key the NOMRES
/// answering it also resolves to (`dvgw_edi::CorrelationKey::nominierung`).
///
/// The pair has no published Zuordnungstupel, so the sender assembling the key
/// and the receiver reading it off the wire must land on the same string; both
/// build it through `dvgw-edi` rather than formatting it twice.
///
/// The key names the nomination's **first** position, which is what the
/// receiver reads: `LOC` of the first `LIN` and its Bilanzkreise.
#[must_use]
pub fn nomination_process_key(gas_day: GasDay, positions: &[NominationPosition]) -> String {
    let first = positions.first();
    dvgw_edi::CorrelationKey::nominierung(
        gas_day.date,
        first.map_or("", |p| p.ort.as_str()),
        first.map_or("", |p| p.bilanzkreis_intern.as_str()),
        first
            .and_then(|p| p.bilanzkreis_extern.as_deref())
            .unwrap_or_default(),
    )
    .to_string()
}

/// Σ(rate × duration) across `positions`, but only when they state one
/// direction: a purchase and a sale in one message are a net position, not a
/// total, so the figure is withheld rather than netted.
#[must_use]
pub fn single_direction_energy(positions: &[NominationPosition]) -> Option<rust_decimal::Decimal> {
    let mut by_direction: std::collections::BTreeMap<&str, rust_decimal::Decimal> =
        std::collections::BTreeMap::new();
    for p in positions {
        let kwh: rust_decimal::Decimal = p.mengen.iter().map(NominationMenge::energie_kwh).sum();
        *by_direction.entry(p.richtung.as_str()).or_default() += kwh;
    }
    (by_direction.len() == 1)
        .then(|| by_direction.into_values().next())
        .flatten()
}

/// The outbox payload `makod` renders as the NOMRES: the nomination's own
/// positions, each labelled with what the answer makes of it (`IMD` DE 7009).
#[must_use]
pub fn nomres_payload(
    pruefidentifikator: u32,
    sender_eic: &str,
    receiver_eic: &str,
    gas_day: GasDay,
    nomres_ref: &MessageRef,
    positions: &[NominationPosition],
    imd_label: &str,
) -> serde_json::Value {
    let mut payload = nomint_payload(
        pruefidentifikator,
        sender_eic,
        receiver_eic,
        gas_day,
        nomres_ref,
        positions,
        None,
    );
    if let Some(items) = payload["positions"].as_array_mut() {
        for item in items {
            item["description"] = serde_json::Value::String(imd_label.to_owned());
        }
    }
    payload
}

/// The outbox payload `makod` renders as the NOMINT (its `dvgw` renderer).
#[must_use]
pub fn nomint_payload(
    pruefidentifikator: u32,
    sender_eic: &str,
    receiver_eic: &str,
    gas_day: GasDay,
    nomination_ref: &MessageRef,
    positions: &[NominationPosition],
    corrects: Option<&Renominierung>,
) -> serde_json::Value {
    let rfc3339 = |t: time::OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    };
    let positions: Vec<serde_json::Value> = positions
        .iter()
        .map(|p| {
            let mut parties =
                vec![serde_json::json!({ "role": "ZEU", "code": p.bilanzkreis_intern })];
            if let Some(extern_) = &p.bilanzkreis_extern {
                parties.push(serde_json::json!({ "role": "ZES", "code": extern_ }));
            }
            serde_json::json!({
                "location": { "qualifier": p.ort_qualifier, "code": p.ort },
                "quantities": p.mengen.iter().map(|m| serde_json::json!({
                    "qualifier": p.richtung,
                    "value": m.kwh_pro_h.normalize().to_string(),
                    "period": { "start": rfc3339(m.von), "end": rfc3339(m.bis) },
                })).collect::<Vec<_>>(),
                "parties": parties,
            })
        })
        .collect();
    let mut payload = serde_json::json!({
        "pid": pruefidentifikator,
        "sender": sender_eic,
        "receiver": receiver_eic,
        "document_number": nomination_ref.as_str(),
        "message_ref": nomination_ref.as_str(),
        "validity_period": { "start": rfc3339(gas_day.start_utc()), "end": rfc3339(gas_day.end_utc()) },
        "positions": positions,
    });
    if let Some(c) = corrects {
        payload["original_nomination"] = serde_json::json!({
            "reference": c.nomination_ref.as_str(),
            "processed_at": rfc3339(c.processed_at),
        });
    }
    payload
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GaBi Gas Nomination workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum NominationEvent {
    /// This tenant nominated: the NOMINT is in the outbox.
    NominationSent {
        /// The Prüfidentifikator (70030–70034).
        pruefidentifikator: u32,
        /// Whether the counterparty is FNB or MGV.
        counterparty: NominationCounterparty,
        /// EIC code of the sending BKV.
        sender_eic: String,
        /// EIC code of the receiving FNB/MGV.
        receiver_eic: String,
        /// Gas day / nomination period (DTM 137).
        gas_day: GasDay,
        /// NOMINT document reference.
        nomination_ref: MessageRef,
        /// Nominated energy in kWh, integrated over the nominated periods.
        ///
        /// A DVGW `QTY` is a rate in kWh/h, so this is Σ(rate × duration) for the
        /// direction the nomination states. `None` when no quantity could be
        /// integrated — a curtailment then cannot be detected, and the workflow
        /// records that rather than assuming none.
        nominated_kwh: Option<rust_decimal::Decimal>,
        /// What the NOMINT states, position by position.
        positions: Vec<NominationPosition>,
        /// The nomination this one corrects, for a re-nomination.
        corrects: Option<Renominierung>,
    },
    /// A Transportkunde's NOMINT arrived at this tenant (NB/MGV).
    NominationReceived {
        /// The Prüfidentifikator (70030–70034).
        pruefidentifikator: u32,
        /// Whether this tenant answers as FNB or MGV.
        counterparty: NominationCounterparty,
        /// EIC code of the nominating BKV.
        sender_eic: String,
        /// EIC code of this tenant.
        receiver_eic: String,
        /// Gas day / nomination period.
        gas_day: GasDay,
        /// NOMINT document reference.
        nomination_ref: MessageRef,
        /// Nominated energy in kWh, integrated over the nominated periods.
        nominated_kwh: Option<rust_decimal::Decimal>,
        /// What the NOMINT states, position by position.
        positions: Vec<NominationPosition>,
    },
    /// This tenant answered a received NOMINT; the NOMRES is in the outbox.
    NomresSent {
        /// The answering Prüfidentifikator (70035–70039).
        pruefidentifikator: u32,
        /// NOMRES document reference.
        nomres_ref: MessageRef,
        /// What the answer decides.
        acceptance: NomresAcceptance,
        /// Confirmed energy in kWh, where the answer states one figure.
        confirmed_kwh: Option<rust_decimal::Decimal>,
    },
    /// FNB/MGV accepted the nomination in full.
    Accepted {
        /// NOMRES message reference.
        nomres_ref: MessageRef,
        /// Gas day confirmed by the FNB/MGV.
        gas_day: GasDay,
    },
    /// FNB/MGV partially accepted the nomination (curtailment applied).
    PartiallyAccepted {
        /// NOMRES message reference.
        nomres_ref: MessageRef,
        /// Gas day confirmed by the FNB/MGV.
        gas_day: GasDay,
        /// Energy actually confirmed, in kWh — less than what was nominated.
        ///
        /// `None` when the counterparty stated a partial acceptance without a
        /// figure this could integrate; the curtailed amount is then unknown
        /// rather than zero.
        confirmed_kwh: Option<rust_decimal::Decimal>,
    },
    /// FNB/MGV rejected the nomination.
    Rejected {
        /// NOMRES message reference.
        nomres_ref: MessageRef,
        /// Human-readable rejection reason.
        reason: String,
    },
    /// No NOMRES received before the response deadline.
    DeadlineExpired {
        /// Deadline identifier for audit.
        deadline_id: DeadlineId,
        /// Deadline label (always [`NOMRES_DEADLINE_LABEL`]).
        label: String,
    },
}

impl EventPayload for NominationEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::NominationSent { .. } => "GaBiGasNominationSent",
            Self::NominationReceived { .. } => "GaBiGasNominationReceived",
            Self::NomresSent { .. } => "GaBiGasNomresSent",
            Self::Accepted { .. } => "GaBiGasNominationAccepted",
            Self::PartiallyAccepted { .. } => "GaBiGasNominationPartiallyAccepted",
            Self::Rejected { .. } => "GaBiGasNominationRejected",
            Self::DeadlineExpired { .. } => "GaBiGasNominationDeadlineExpired",
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Current state of a GaBi Gas Nomination process stream.
///
/// # Lifecycle
///
/// ```text
/// New
///  └─ Open ──── Accepted          (terminal)
///          ├─── PartiallyAccepted (terminal)
///          ├─── Rejected          (terminal)
///          └─── DeadlineExpired   (terminal)
/// ```
///
/// `Open` holds either end: the NOMINT this tenant sent and awaits the NOMRES
/// for, or the one a Transportkunde sent this tenant, which owes the NOMRES
/// ([`NominationRichtung`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum NominationState {
    /// No NOMINT yet.
    #[default]
    New,
    /// A NOMINT is on file and its NOMRES outstanding.
    Open(NominationData),
    /// NOMRES received — nomination accepted in full (terminal).
    Accepted(NominationData),
    /// NOMRES received — nomination partially accepted, curtailment applied (terminal).
    PartiallyAccepted(NominationData),
    /// NOMRES received — nomination rejected (terminal).
    Rejected {
        /// Nomination data captured at submission time.
        data: NominationData,
        /// Human-readable rejection reason.
        reason: String,
    },
    /// No NOMRES received before the D-1 15:00 deadline (terminal).
    DeadlineExpired(NominationData),
}

impl NominationState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Open(_) => "Open",
            Self::Accepted(_) => "Accepted",
            Self::PartiallyAccepted(_) => "PartiallyAccepted",
            Self::Rejected { .. } => "Rejected",
            Self::DeadlineExpired(_) => "DeadlineExpired",
        }
    }

    /// Returns `true` if no further commands can be applied.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Accepted(_)
                | Self::PartiallyAccepted(_)
                | Self::Rejected { .. }
                | Self::DeadlineExpired(_)
        )
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands for the GaBi Gas Nomination workflow.
///
/// [`Workflow::handle`] is pure — no I/O.
#[derive(Clone)]
pub enum NominationCommand {
    /// The Transportkunde is dispatching a NOMINT nomination (PIDs 70030–70034).
    ///
    /// Constructed by the outbound dispatch layer in `makod` after the BKV
    /// submits a nomination via the Commands API.
    SendNomination {
        /// The Prüfidentifikator (70030–70034).
        pruefidentifikator: u32,
        /// EIC code of the sending BKV — this tenant.
        sender_eic: String,
        /// EIC code of the receiving FNB/MGV.
        receiver_eic: String,
        /// Gas day / nomination period.
        gas_day: GasDay,
        /// NOMINT document reference (`BGM` DE 1004).
        nomination_ref: MessageRef,
        /// What is nominated: the NOMINT's positions. At least one.
        positions: Vec<NominationPosition>,
        /// The nomination this one corrects, for a re-nomination.
        corrects: Option<Renominierung>,
    },

    /// A Transportkunde's NOMINT arrived (PIDs 70030–70034) — this tenant is
    /// the NB or MGV it is addressed to and owes the NOMRES.
    ///
    /// Constructed by the DVGW adapter in `makod`.
    ReceiveNomint {
        /// The Prüfidentifikator (70030–70034).
        pruefidentifikator: u32,
        /// EIC code of the nominating BKV.
        sender_eic: String,
        /// EIC code of this tenant.
        receiver_eic: String,
        /// Gas day / nomination period.
        gas_day: GasDay,
        /// NOMINT document reference.
        nomination_ref: MessageRef,
        /// What the NOMINT states, position by position — the answer is built
        /// from it, so it is read off the wire rather than reconstructed.
        positions: Vec<NominationPosition>,
        /// Nominated energy in kWh, integrated over the nominated periods.
        ///
        /// A DVGW `QTY` is a rate in kWh/h, so this is Σ(rate × duration) for the
        /// direction the nomination states. `None` when no quantity could be
        /// integrated — a curtailment then cannot be detected, and the workflow
        /// records that rather than assuming none.
        nominated_kwh: Option<rust_decimal::Decimal>,
    },

    /// Answer a received NOMINT: the NOMRES this tenant owes as the NB or MGV.
    ///
    /// The answer restates the nomination's own positions under the `IMD`
    /// label that says what they are — `16G` Bestätigt where it decides,
    /// `17G` Nominiert where it only reports the state of the match.
    SendNomres {
        /// The answering Prüfidentifikator (70035–70039).
        pruefidentifikator: u32,
        /// NOMRES document reference.
        nomres_ref: MessageRef,
        /// What the answer decides.
        acceptance: NomresAcceptance,
        /// The positions as the match produced them, where it curtailed the
        /// nomination. `None` confirms it as stated.
        confirmed: Option<Vec<NominationPosition>>,
    },

    /// Inbound NOMRES received from the NB or MGV (PIDs 70035–70039).
    ///
    /// Constructed by the DVGW adapter in `makod` when a NOMRES arrives on the
    /// inbound channel. The `nomination_ref` must match the one in the outbound
    /// NOMINT to correlate correctly.
    ReceiveNomres {
        /// NOMRES message reference.
        nomres_ref: MessageRef,
        /// Overall acceptance status from the leading STS segment.
        acceptance: NomresAcceptance,
        /// Gas day confirmed by the FNB/MGV.
        gas_day: GasDay,
        /// Confirmed energy in kWh, integrated over the confirmed periods.
        ///
        /// Compared against the nomination's own figure to detect a curtailment:
        /// NOMRES has no status segment, so a partial acceptance shows up **only**
        /// as a reduced quantity. `None` leaves the acceptance as stated.
        confirmed_kwh: Option<rust_decimal::Decimal>,
        /// Human-readable rejection reason (populated when `acceptance = Rejected`).
        rejection_reason: Option<String>,
    },

    /// NOMRES response deadline expired — no response from FNB/MGV.
    NomresDeadlineExpired {
        /// Deadline identifier for audit.
        deadline_id: DeadlineId,
        /// Deadline label (always [`NOMRES_DEADLINE_LABEL`]).
        label: String,
    },
}

impl CommandPayload for NominationCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GaBi Gas Nomination workflow.
///
/// Tracks the lifecycle of a single NOMINT submission and its corresponding
/// NOMRES reply for the BKV → FNB/MGV nomination cycle (KoV §5).
pub struct GaBiGasNominationWorkflow;

fn nomint_counterparty(pid: u32) -> Result<NominationCounterparty, WorkflowError> {
    NominationCounterparty::from_pid(pid).ok_or_else(|| {
        WorkflowError::rejected(format!(
            "PID {pid} is not a NOMINT Prüfidentifikator (expected one of 70030–70034)"
        ))
    })
}

impl Workflow for GaBiGasNominationWorkflow {
    type State = NominationState;
    type Event = NominationEvent;
    type Command = NominationCommand;

    /// Turn the fired [`NOMRES_DEADLINE_LABEL`] into
    /// [`NominationCommand::NomresDeadlineExpired`].
    ///
    /// The command, the event and the terminal `DeadlineExpired` state were all
    /// there; this hook was not, so a NOMINT that never drew a NOMRES stayed in
    /// `Open` forever and the missed D+1 window was invisible.
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        (deadline.label() == NOMRES_DEADLINE_LABEL && matches!(state, NominationState::Open(_)))
            .then(|| NominationCommand::NomresDeadlineExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().to_owned(),
            })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            NominationEvent::NominationSent {
                pruefidentifikator,
                counterparty,
                sender_eic,
                receiver_eic,
                gas_day,
                nomination_ref,
                nominated_kwh,
                positions,
                corrects,
            } => NominationState::Open(NominationData {
                richtung: NominationRichtung::Gesendet,
                pruefidentifikator: *pruefidentifikator,
                counterparty: *counterparty,
                sender_eic: sender_eic.clone(),
                receiver_eic: receiver_eic.clone(),
                gas_day: *gas_day,
                nomination_ref: nomination_ref.clone(),
                quantity: nominated_kwh.map(NominationQuantity::submitted),
                positions: positions.clone(),
                corrects_nomination_ref: corrects.as_ref().map(|c| c.nomination_ref.clone()),
                correction_sequence: u32::from(corrects.is_some()),
            }),
            NominationEvent::NominationReceived {
                pruefidentifikator,
                counterparty,
                sender_eic,
                receiver_eic,
                gas_day,
                nomination_ref,
                nominated_kwh,
                positions,
            } => NominationState::Open(NominationData {
                richtung: NominationRichtung::Empfangen,
                pruefidentifikator: *pruefidentifikator,
                counterparty: *counterparty,
                sender_eic: sender_eic.clone(),
                receiver_eic: receiver_eic.clone(),
                gas_day: *gas_day,
                nomination_ref: nomination_ref.clone(),
                quantity: nominated_kwh.map(NominationQuantity::submitted),
                positions: positions.clone(),
                corrects_nomination_ref: None,
                correction_sequence: 0,
            }),
            NominationEvent::NomresSent {
                acceptance,
                confirmed_kwh,
                ..
            } => match state {
                NominationState::Open(mut data) => {
                    data.quantity = match (data.quantity, confirmed_kwh) {
                        (Some(q), Some(confirmed)) => Some(q.accept_partial(*confirmed, None)),
                        (Some(q), None) => Some(q.accept_in_full()),
                        (None, _) => None,
                    };
                    match acceptance {
                        NomresAcceptance::Accepted => NominationState::Accepted(data),
                        NomresAcceptance::PartiallyAccepted => {
                            NominationState::PartiallyAccepted(data)
                        }
                        NomresAcceptance::Rejected | NomresAcceptance::Other(_) => {
                            NominationState::Rejected {
                                data,
                                reason: acceptance.as_str().to_owned(),
                            }
                        }
                    }
                }
                other => other,
            },

            NominationEvent::Accepted { .. } => match state {
                NominationState::Open(mut data) => {
                    data.quantity = data.quantity.map(NominationQuantity::accept_in_full);
                    NominationState::Accepted(data)
                }
                other => other,
            },

            NominationEvent::PartiallyAccepted {
                confirmed_kwh: Some(confirmed),
                ..
            } => match state {
                NominationState::Open(mut data) => {
                    data.quantity = data
                        .quantity
                        .map(|q| q.accept_partial(*confirmed, Some("curtailed by NOMRES".into())));
                    NominationState::PartiallyAccepted(data)
                }
                other => other,
            },

            NominationEvent::PartiallyAccepted { .. } => match state {
                NominationState::Open(data) => NominationState::PartiallyAccepted(data),
                other => other,
            },

            NominationEvent::Rejected { reason, .. } => match state {
                NominationState::Open(data) => NominationState::Rejected {
                    data,
                    reason: reason.clone(),
                },
                other => other,
            },

            NominationEvent::DeadlineExpired { .. } => match state {
                NominationState::Open(data) => NominationState::DeadlineExpired(data),
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            NominationCommand::SendNomination {
                pruefidentifikator,
                sender_eic,
                receiver_eic,
                gas_day,
                nomination_ref,
                positions,
                corrects,
            } => {
                if !matches!(state, NominationState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let counterparty = nomint_counterparty(pruefidentifikator)?;
                if positions.is_empty() || positions.iter().any(|p| p.mengen.is_empty()) {
                    return Err(WorkflowError::rejected(
                        "a NOMINT states at least one position with at least one Menge",
                    ));
                }
                let nominated_kwh = single_direction_energy(&positions);
                let outbox = PendingOutbox::new(
                    "NOMINT",
                    receiver_eic.as_str(),
                    nomint_payload(
                        pruefidentifikator,
                        &sender_eic,
                        &receiver_eic,
                        gas_day,
                        &nomination_ref,
                        &positions,
                        corrects.as_ref(),
                    ),
                );
                Ok(WorkflowOutput {
                    events: vec![NominationEvent::NominationSent {
                        pruefidentifikator,
                        counterparty,
                        sender_eic,
                        receiver_eic,
                        gas_day,
                        nomination_ref,
                        nominated_kwh,
                        positions,
                        corrects,
                    }],
                    outbox: vec![outbox],
                    deadlines: Vec::new(),
                })
            }

            NominationCommand::ReceiveNomint {
                pruefidentifikator,
                sender_eic,
                receiver_eic,
                gas_day,
                nomination_ref,
                positions,
                nominated_kwh,
            } => {
                if !matches!(state, NominationState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let counterparty = nomint_counterparty(pruefidentifikator)?;
                Ok(vec![NominationEvent::NominationReceived {
                    pruefidentifikator,
                    counterparty,
                    sender_eic,
                    receiver_eic,
                    gas_day,
                    nomination_ref,
                    nominated_kwh,
                    positions,
                }]
                .into())
            }

            NominationCommand::SendNomres {
                pruefidentifikator,
                nomres_ref,
                acceptance,
                confirmed,
            } => {
                let NominationState::Open(open) = state else {
                    return Err(WorkflowError::invalid_state("Open", state.label()));
                };
                if open.richtung != NominationRichtung::Empfangen {
                    return Err(WorkflowError::rejected(
                        "this process holds a nomination this tenant sent; the NOMRES \
                         answering it comes from the counterparty",
                    ));
                }
                if !NOMRES_PIDS.contains(&pruefidentifikator) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pruefidentifikator} is not a NOMRES Prüfidentifikator \
                         (expected one of 70035–70039)"
                    )));
                }
                // `16G` Bestätigt where the answer decides, `17G` Nominiert
                // where it only reports the state of the match.
                let label = match acceptance {
                    NomresAcceptance::Accepted | NomresAcceptance::PartiallyAccepted => "16G",
                    NomresAcceptance::Rejected | NomresAcceptance::Other(_) => "17G",
                };
                let positions = confirmed.as_ref().unwrap_or(&open.positions);
                if positions.is_empty() {
                    return Err(WorkflowError::rejected(
                        "the answer restates the nomination's positions, and none are on \
                         file — pass `confirmed`",
                    ));
                }
                let confirmed_kwh = single_direction_energy(positions);
                let outbox = PendingOutbox::new(
                    "NOMRES",
                    open.sender_eic.as_str(),
                    nomres_payload(
                        pruefidentifikator,
                        &open.receiver_eic,
                        &open.sender_eic,
                        open.gas_day,
                        &nomres_ref,
                        positions,
                        label,
                    ),
                );
                Ok(WorkflowOutput {
                    events: vec![NominationEvent::NomresSent {
                        pruefidentifikator,
                        nomres_ref,
                        acceptance,
                        confirmed_kwh,
                    }],
                    outbox: vec![outbox],
                    deadlines: Vec::new(),
                })
            }

            NominationCommand::ReceiveNomres {
                nomres_ref,
                acceptance,
                gas_day,
                confirmed_kwh,
                rejection_reason,
            } => {
                let NominationState::Open(sent) = state else {
                    return Err(WorkflowError::invalid_state("Open", state.label()));
                };
                if sent.richtung != NominationRichtung::Gesendet {
                    return Err(WorkflowError::rejected(
                        "this process holds a nomination addressed to this tenant; it owes \
                         the NOMRES rather than receiving one",
                    ));
                }

                // NOMRES has no status segment: a curtailment shows up **only** as
                // a confirmed quantity below the nominated one. So a stated
                // acceptance is upgraded to a partial one when the numbers say so
                // — recording a curtailed nomination as fully accepted leaves the
                // BKV's portfolio short by the difference with nothing pointing
                // at it.
                let nominated = sent.quantity.as_ref().map(|q| q.submitted_kwh);
                let curtailed = matches!(
                    (nominated, confirmed_kwh),
                    (Some(nominated), Some(confirmed)) if confirmed < nominated
                );
                let acceptance = match acceptance {
                    NomresAcceptance::Accepted if curtailed => NomresAcceptance::PartiallyAccepted,
                    other => other,
                };

                // What the answer decides is the ERP's business: a shortfall
                // leaves the portfolio short by the difference, and a refusal
                // means nothing flows at all. Recording either without saying
                // so leaves the fact in the event stream and nowhere else.
                let notice = |kind: &'static str, data: serde_json::Value| {
                    PendingOutbox::new(kind, sent.sender_eic.as_str(), data)
                };
                let nominated = sent.quantity.as_ref().map(|q| q.submitted_kwh);
                let parties = serde_json::json!({
                    "gas_day": gas_day,
                    "sender_eic": sent.sender_eic,
                    "receiver_eic": sent.receiver_eic,
                    "pruefidentifikator": sent.pruefidentifikator,
                    "nomination_ref": sent.nomination_ref,
                });
                let with = |extra: serde_json::Value| {
                    let mut data = parties.clone();
                    if let (Some(map), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
                        map.extend(extra.clone());
                    }
                    data
                };
                let (event, outbox) = match &acceptance {
                    NomresAcceptance::Accepted => (
                        NominationEvent::Accepted {
                            nomres_ref,
                            gas_day,
                        },
                        Vec::new(),
                    ),
                    NomresAcceptance::PartiallyAccepted => {
                        let curtailed = nominated
                            .zip(confirmed_kwh)
                            .map(|(nominated, confirmed)| nominated - confirmed);
                        let data = with(serde_json::json!({
                            "nominated_kwh": nominated,
                            "confirmed_kwh": confirmed_kwh,
                            "curtailed_kwh": curtailed,
                        }));
                        (
                            NominationEvent::PartiallyAccepted {
                                nomres_ref,
                                gas_day,
                                confirmed_kwh,
                            },
                            vec![notice("GabiNominationCurtailed", data)],
                        )
                    }
                    NomresAcceptance::Rejected | NomresAcceptance::Other(_) => {
                        let reason =
                            rejection_reason.unwrap_or_else(|| acceptance.as_str().to_owned());
                        let data = with(serde_json::json!({
                            "reason": reason,
                            "nominated_kwh": nominated,
                        }));
                        (
                            NominationEvent::Rejected { nomres_ref, reason },
                            vec![notice("GabiNominationRejected", data)],
                        )
                    }
                };
                Ok(WorkflowOutput {
                    events: vec![event],
                    outbox,
                    deadlines: Vec::new(),
                })
            }

            NominationCommand::NomresDeadlineExpired { deadline_id, label } => {
                if state.is_terminal() {
                    // Deadline fired after NOMRES already received — absorb silently.
                    return Ok(WorkflowOutput::events(vec![]));
                }
                // At gas-day start the nomination's status is unknown: the
                // counterparty owes an answer and has not sent one. That is an
                // operator call, so it leaves the platform rather than sitting
                // in the stream.
                let outbox = match state {
                    NominationState::Open(data) => vec![PendingOutbox::new(
                        "GabiNomresMissing",
                        data.sender_eic.as_str(),
                        serde_json::json!({
                            "gas_day": data.gas_day,
                            "sender_eic": data.sender_eic,
                            "receiver_eic": data.receiver_eic,
                            "pruefidentifikator": data.pruefidentifikator,
                            "nomination_ref": data.nomination_ref,
                            "deadline_label": label,
                        }),
                    )],
                    _ => Vec::new(),
                };
                Ok(WorkflowOutput {
                    events: vec![NominationEvent::DeadlineExpired { deadline_id, label }],
                    outbox,
                    deadlines: Vec::new(),
                })
            }
        }
    }
}

#[cfg(test)]
mod pid_catalogue_conformance {
    use super::{NOMINATION_PIDS, NOMINT_PIDS, NOMRES_PIDS, NominationCounterparty};

    /// The lists above are a projection of the DVGW catalogue. A second copy
    /// that drifts is how a published Anwendungsfall silently stops routing, so
    /// they are pinned to the source rather than merely reviewed.
    #[test]
    fn the_pid_lists_match_the_dvgw_catalogue() {
        for (message_type, expected) in [
            (dvgw_edi::DvgwMessageType::Nomint, NOMINT_PIDS),
            (dvgw_edi::DvgwMessageType::Nomres, NOMRES_PIDS),
        ] {
            let published: Vec<u32> = dvgw_edi::catalogue_for(message_type)
                .map(|info| info.pid)
                .collect();
            assert_eq!(
                published, expected,
                "{message_type} routing list has drifted from the DVGW catalogue"
            );
        }
        let union: Vec<u32> = NOMINT_PIDS.iter().chain(NOMRES_PIDS).copied().collect();
        assert_eq!(union, NOMINATION_PIDS);
    }

    /// Every routed code must resolve to a counterparty, or the workflow rejects
    /// a message DVGW publishes.
    #[test]
    fn every_routed_pid_resolves_to_a_counterparty() {
        for &pid in NOMINATION_PIDS {
            assert!(
                NominationCounterparty::from_pid(pid).is_some(),
                "PID {pid} routes here but has no counterparty"
            );
        }
    }
}
