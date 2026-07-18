//! GaBi Gas Nomination workflow — NOMINT / NOMRES (BKV ↔ FNB / MGV).
//!
//! Implements the gas nomination and confirmation cycle governed by the
//! Kooperationsvereinbarung Gas (KoV) and the BNetzA GaBi Gas 2.0 framework
//! (BK7-14-020).
//!
//! # Process overview
//!
//! The BKV submits a **nomination** (NOMINT) to the FNB or MGV by D-1 13:00 CET.
//! The FNB / MGV responds with a **nomination response** (NOMRES) confirming,
//! curtailing, or rejecting the submitted quantities.
//!
//! ```text
//! BKV ──(NOMINT 90011/90012)──→  FNB / MGV
//! FNB / MGV ──(NOMRES 90021/90022)──→  BKV
//! ```
//!
//! # Synthetic Prüfidentifikatoren
//!
//! DVGW messages carry no BGM Prüfidentifikator. The `dvgw-edi` crate assigns
//! synthetic PIDs from the range 90000–90999:
//!
//! | PID   | Message | Direction          | Role qualifier |
//! |-------|---------|--------------------|----------------|
//! | 90011 | NOMINT  | BKV → FNB          | Z01            |
//! | 90012 | NOMINT  | BKV → MGV          | Z02            |
//! | 90021 | NOMRES  | FNB → BKV          | Z01            |
//! | 90022 | NOMRES  | MGV → BKV          | Z02            |
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
//! - **BNetzA BK7-14-020** — GaBi Gas 2.0 ruling
//! - **DVGW NOMINT 4.6 FK** / **NOMRES 4.7 FK** — message format (valid from 2026-02-01)

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::MessageRef,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

use crate::domain::{GasDay, NominationQuantity};

// ── Synthetic PID set ─────────────────────────────────────────────────────────

/// All synthetic PIDs for the NOMINT/NOMRES nomination cycle.
///
/// | PID   | Message | Direction   |
/// |-------|---------|-------------|
/// | 90011 | NOMINT  | BKV → FNB  |
/// | 90012 | NOMINT  | BKV → MGV  |
/// | 90021 | NOMRES  | FNB → BKV  |
/// | 90022 | NOMRES  | MGV → BKV  |
pub const NOMINATION_PIDS: &[u32] = &[90011, 90012, 90021, 90022];

/// Synthetic PIDs for outbound NOMINT (BKV → FNB or BKV → MGV).
pub const NOMINT_PIDS: &[u32] = &[90011, 90012];

/// Synthetic PIDs for inbound NOMRES (FNB/MGV → BKV).
pub const NOMRES_PIDS: &[u32] = &[90021, 90022];

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
    /// FNB (Fernleitungsnetzbetreiber) — synthetic PID 90011.
    Fnb,
    /// MGV (Marktgebietsverantwortlicher) — synthetic PID 90012.
    Mgv,
}

impl NominationCounterparty {
    /// Derive from a synthetic PID.
    ///
    /// Returns `None` for unrecognised PIDs.
    #[must_use]
    pub fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            90011 | 90021 => Some(Self::Fnb),
            90012 | 90022 => Some(Self::Mgv),
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
    /// Synthetic PID that initiated this nomination (90011 = FNB, 90012 = MGV).
    pub synthetic_pid: u32,
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
        /// Synthetic PID (90011 = FNB, 90012 = MGV).
        synthetic_pid: u32,
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
    /// BKV is dispatching a NOMINT nomination (PIDs 90011 or 90012).
    ///
    /// Constructed by the outbound dispatch layer in `makod` after the BKV
    /// submits a nomination via the Commands API.
    SendNomination {
        /// Synthetic PID (90011 = FNB, 90012 = MGV).
        synthetic_pid: u32,
        /// EIC code of the sending BKV.
        sender_eic: String,
        /// EIC code of the receiving FNB/MGV.
        receiver_eic: String,
        /// Gas day / nomination period.
        gas_day: GasDay,
        /// NOMINT document reference.
        nomination_ref: MessageRef,
    },

    /// Inbound NOMRES received from FNB/MGV (PIDs 90021 or 90022).
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

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            NominationEvent::NominationSent {
                synthetic_pid,
                counterparty,
                sender_eic,
                receiver_eic,
                gas_day,
                nomination_ref,
            } => NominationState::NominationSent(NominationData {
                synthetic_pid: *synthetic_pid,
                counterparty: *counterparty,
                sender_eic: sender_eic.clone(),
                receiver_eic: receiver_eic.clone(),
                gas_day: *gas_day,
                nomination_ref: nomination_ref.clone(),
                quantity: None, // populated later when quantity is parsed from NOMINT payload
                corrects_nomination_ref: None, // set by handle() when correcting a prior NOMINT
                correction_sequence: 0,
            }),

            NominationEvent::Accepted { .. } => match state {
                NominationState::NominationSent(data) => NominationState::Accepted(data),
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
                synthetic_pid,
                sender_eic,
                receiver_eic,
                gas_day,
                nomination_ref,
            } => {
                if !matches!(state, NominationState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let counterparty =
                    NominationCounterparty::from_pid(synthetic_pid).ok_or_else(|| {
                        WorkflowError::rejected(format!(
                            "PID {synthetic_pid} is not a valid NOMINT PID \
                             (expected 90011 or 90012)"
                        ))
                    })?;
                Ok(vec![NominationEvent::NominationSent {
                    synthetic_pid,
                    counterparty,
                    sender_eic,
                    receiver_eic,
                    gas_day,
                    nomination_ref,
                }]
                .into())
            }

            NominationCommand::ReceiveNomres {
                nomres_ref,
                acceptance,
                gas_day,
                rejection_reason,
            } => {
                if !matches!(state, NominationState::NominationSent(_)) {
                    return Err(WorkflowError::invalid_state(
                        "NominationSent",
                        state.label(),
                    ));
                }
                let event = match &acceptance {
                    NomresAcceptance::Accepted => NominationEvent::Accepted {
                        nomres_ref,
                        gas_day,
                    },
                    NomresAcceptance::PartiallyAccepted => NominationEvent::PartiallyAccepted {
                        nomres_ref,
                        gas_day,
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
