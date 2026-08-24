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
//!  └─ NominationSent (NOMINT dispatched outbound)
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

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GaBi Gas Nomination workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum NominationEvent {
    /// BKV dispatched a NOMINT nomination to FNB or MGV.
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
///  └─ NominationSent ──── Accepted         (terminal)
///                    ├─── PartiallyAccepted (terminal)
///                    ├─── Rejected          (terminal)
///                    └─── DeadlineExpired   (terminal)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum NominationState {
    /// No NOMINT dispatched yet.
    #[default]
    New,
    /// NOMINT dispatched; awaiting NOMRES from FNB/MGV.
    NominationSent(NominationData),
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
            Self::NominationSent(_) => "NominationSent",
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
        /// EIC code of the sending BKV.
        sender_eic: String,
        /// EIC code of the receiving FNB/MGV.
        receiver_eic: String,
        /// Gas day / nomination period.
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

impl Workflow for GaBiGasNominationWorkflow {
    type State = NominationState;
    type Event = NominationEvent;
    type Command = NominationCommand;

    /// Turn the fired [`NOMRES_DEADLINE_LABEL`] into
    /// [`NominationCommand::NomresDeadlineExpired`].
    ///
    /// The command, the event and the terminal `DeadlineExpired` state were all
    /// there; this hook was not, so a NOMINT that never drew a NOMRES stayed in
    /// `NominationSent` forever and the missed D+1 window was invisible.
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        (deadline.label() == NOMRES_DEADLINE_LABEL
            && matches!(state, NominationState::NominationSent(_)))
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
            } => NominationState::NominationSent(NominationData {
                pruefidentifikator: *pruefidentifikator,
                counterparty: *counterparty,
                sender_eic: sender_eic.clone(),
                receiver_eic: receiver_eic.clone(),
                gas_day: *gas_day,
                nomination_ref: nomination_ref.clone(),
                quantity: nominated_kwh.map(NominationQuantity::submitted),
                corrects_nomination_ref: None, // set by handle() when correcting a prior NOMINT
                correction_sequence: 0,
            }),

            NominationEvent::Accepted { .. } => match state {
                NominationState::NominationSent(mut data) => {
                    data.quantity = data.quantity.map(NominationQuantity::accept_in_full);
                    NominationState::Accepted(data)
                }
                other => other,
            },

            NominationEvent::PartiallyAccepted {
                confirmed_kwh: Some(confirmed),
                ..
            } => match state {
                NominationState::NominationSent(mut data) => {
                    data.quantity = data
                        .quantity
                        .map(|q| q.accept_partial(*confirmed, Some("curtailed by NOMRES".into())));
                    NominationState::PartiallyAccepted(data)
                }
                other => other,
            },

            NominationEvent::PartiallyAccepted { .. } => match state {
                NominationState::NominationSent(data) => NominationState::PartiallyAccepted(data),
                other => other,
            },

            NominationEvent::Rejected { reason, .. } => match state {
                NominationState::NominationSent(data) => NominationState::Rejected {
                    data,
                    reason: reason.clone(),
                },
                other => other,
            },

            NominationEvent::DeadlineExpired { .. } => match state {
                NominationState::NominationSent(data) => NominationState::DeadlineExpired(data),
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
                nominated_kwh,
            } => {
                if !matches!(state, NominationState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let counterparty = NominationCounterparty::from_pid(pruefidentifikator)
                    .ok_or_else(|| {
                        WorkflowError::rejected(format!(
                            "PID {pruefidentifikator} is not a NOMINT Prüfidentifikator \
                             (expected one of 70030–70034)"
                        ))
                    })?;
                Ok(vec![NominationEvent::NominationSent {
                    pruefidentifikator,
                    counterparty,
                    sender_eic,
                    receiver_eic,
                    gas_day,
                    nomination_ref,
                    nominated_kwh,
                }]
                .into())
            }

            NominationCommand::ReceiveNomres {
                nomres_ref,
                acceptance,
                gas_day,
                confirmed_kwh,
                rejection_reason,
            } => {
                let NominationState::NominationSent(sent) = state else {
                    return Err(WorkflowError::invalid_state(
                        "NominationSent",
                        state.label(),
                    ));
                };

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

                let event = match &acceptance {
                    NomresAcceptance::Accepted => NominationEvent::Accepted {
                        nomres_ref,
                        gas_day,
                    },
                    NomresAcceptance::PartiallyAccepted => NominationEvent::PartiallyAccepted {
                        nomres_ref,
                        gas_day,
                        confirmed_kwh,
                    },
                    NomresAcceptance::Rejected | NomresAcceptance::Other(_) => {
                        NominationEvent::Rejected {
                            nomres_ref,
                            reason: rejection_reason
                                .unwrap_or_else(|| acceptance.as_str().to_owned()),
                        }
                    }
                };
                Ok(vec![event].into())
            }

            NominationCommand::NomresDeadlineExpired { deadline_id, label } => {
                if state.is_terminal() {
                    // Deadline fired after NOMRES already received — absorb silently.
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![NominationEvent::DeadlineExpired { deadline_id, label }].into())
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
