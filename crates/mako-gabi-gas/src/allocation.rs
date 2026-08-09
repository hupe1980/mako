//! GaBi Gas Allocation workflow — ALOCAT (FNB / MGV / VNB → BKV).
//!
//! Implements the receive-and-record side of gas quantity allocation governed
//! by the Kooperationsvereinbarung Gas (KoV) and the BNetzA GaBi Gas 2.1
//! framework (BK7-24-01-008).
//!
//! # Process overview
//!
//! The FNB, MGV, or VNB sends an **allocation message** (ALOCAT) to the BKV
//! reporting the final allocated gas quantities for a given gas day or period.
//! No response is required.
//!
//! ```text
//! FNB / MGV / VNB ──(ALOCAT 90001/90002/90003)──→  BKV
//! ```
//!
//! # Synthetic Prüfidentifikatoren
//!
//! DVGW messages carry no BGM Prüfidentifikator. The `dvgw-edi` crate assigns
//! synthetic PIDs from the range 90000–90999:
//!
//! | PID   | Message | Direction          | Qualifier |
//! |-------|---------|--------------------|-----------|
//! | 90001 | ALOCAT  | FNB → BKV (daily)  | Z15       |
//! | 90002 | ALOCAT  | MGV → BKV (monthly)| Z16       |
//! | 90003 | ALOCAT  | VNB → FNB (sub-day)| Z17       |
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ Recorded ──(correction / final ALOCAT)──→ Recorded
//!       └─ FinalOverdue   [KoV §6.4 M+2 deadline passed with no final]
//! ```
//!
//! No response is sent, but the process is **not** terminal on first receipt:
//! KoV §6.4 lets the FNB/MGV correct an allocation and then confirm a binding
//! final one, and only the final allocation settles the imbalance.
//!
//! # Regulatory basis
//!
//! - **Kooperationsvereinbarung Gas (KoV)** — allocation reporting deadlines
//! - **BNetzA BK7-24-01-008** — GaBi Gas 2.1 ruling
//! - **DVGW ALOCAT 5.11a** — message format (valid from 2024-10-01)

use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    types::MessageRef,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

use crate::domain::{GasDay, GasQuantity};

// ── Synthetic PID set ─────────────────────────────────────────────────────────

/// All synthetic PIDs for the ALOCAT allocation message.
///
/// | PID   | Message | Sender | Direction           |
/// |-------|---------|--------|---------------------|
/// | 90001 | ALOCAT  | FNB    | FNB → BKV (daily)   |
/// | 90002 | ALOCAT  | MGV    | MGV → BKV (monthly) |
/// | 90003 | ALOCAT  | VNB    | VNB → FNB (sub-day) |
pub const ALLOCATION_PIDS: &[u32] = &[90001, 90002, 90003];

/// Workflow key for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-allocation";

/// Deadline label for the KoV §6.4 final-allocation window.
///
/// The binding final allocation is due by the end of month M+2 at 12:00 CET;
/// register a [`mako_engine::deadline::Deadline`] with this label using
/// [`GasDay::final_alocat_deadline_utc`] when the first ALOCAT for a gas day is
/// recorded. If it fires with no [`AllocationVersion::Final`] on file, the
/// FNB/MGV has missed a binding obligation.
pub const FINAL_ALOCAT_DEADLINE_LABEL: &str = "gabi-gas-final-alocat-deadline";

// ── Allocation type ───────────────────────────────────────────────────────────

/// Which category of allocation this ALOCAT message represents.
///
/// Derived from the synthetic PID to allow downstream analysis without
/// re-parsing the raw message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationType {
    /// Daily allocation from FNB to BKV (synthetic PID 90001, qualifier Z15).
    FnbDailyToBkv,
    /// Monthly allocation from MGV to BKV (synthetic PID 90002, qualifier Z16).
    MgvMonthlyToBkv,
    /// Sub-daily allocation from VNB to FNB (synthetic PID 90003, qualifier Z17).
    VnbSubDailyToFnb,
}

impl AllocationType {
    /// Derive from a synthetic PID.
    ///
    /// Returns `None` for unrecognised PIDs.
    #[must_use]
    pub fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            90001 => Some(Self::FnbDailyToBkv),
            90002 => Some(Self::MgvMonthlyToBkv),
            90003 => Some(Self::VnbSubDailyToFnb),
            _ => None,
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Version / sequence of an ALOCAT message.
///
/// Per KoV, the FNB/MGV may send corrected allocations after the initial
/// delivery. The `AllocationVersion` tracks which sequence this is:
/// - `Initial` = first allocation for a gas day
/// - `Correction(n)` = nth correction (n ≥ 1)
/// - `Final` = confirmed final allocation (no further corrections expected)
///
/// Source: Kooperationsvereinbarung Gas (KoV) §6.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationVersion {
    /// First allocation message for this gas day / period.
    Initial,
    /// Corrected allocation. `n` = correction sequence number (1-based).
    Correction(u32),
    /// Final confirmed allocation — no further corrections expected.
    Final,
}

impl AllocationVersion {
    /// `true` when this is not the initial allocation (corrected or final).
    #[must_use]
    pub fn is_revision(&self) -> bool {
        !matches!(self, Self::Initial)
    }
}

/// Data captured when an ALOCAT allocation message is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationData {
    /// Synthetic PID that identifies the allocation category (90001/90002/90003).
    pub synthetic_pid: u32,
    /// Category of this allocation (FNB daily, MGV monthly, or VNB sub-daily).
    pub allocation_type: AllocationType,
    /// EIC code of the sending party (FNB / MGV / VNB).
    pub sender_eic: String,
    /// EIC code of the receiving party (BKV / FNB).
    pub receiver_eic: String,
    /// Gas day or allocation period.
    pub gas_day: GasDay,
    /// Version of this allocation (initial, correction, or final).
    ///
    /// Per KoV §6.4: the FNB/MGV sends an initial allocation and may send
    /// corrections within the correction window. The final allocation is
    /// binding for imbalance settlement.
    pub version: AllocationVersion,
    /// Allocated gas quantity for this gas day.
    ///
    /// `None` when the ALOCAT message does not include an explicit quantity
    /// (e.g. a cancellation/withdrawal message). Stored as `GasQuantity`
    /// to preserve m³ + Brennwert context alongside the kWh_Hs billing value.
    pub allocated_quantity: Option<GasQuantity>,
    /// Clearing number from the leading RFF segment (if present).
    pub clearing_number: Option<String>,
    /// ALOCAT document message reference (from UNH).
    pub message_ref: MessageRef,
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GaBi Gas Allocation workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AllocationEvent {
    /// An ALOCAT allocation message was received from FNB, MGV, or VNB.
    AllocationReceived {
        /// Synthetic PID (90001 = FNB daily, 90002 = MGV monthly, 90003 = VNB sub-day).
        synthetic_pid: u32,
        /// Category of this allocation.
        allocation_type: AllocationType,
        /// EIC code of the sending party (FNB / MGV / VNB).
        sender_eic: String,
        /// EIC code of the receiving party (BKV / FNB).
        receiver_eic: String,
        /// Gas day or allocation period.
        gas_day: GasDay,
        /// Version of this allocation.
        version: AllocationVersion,
        /// Allocated quantity in kWh_Hs (with optional m³ volume).
        allocated_quantity: Option<GasQuantity>,
        /// Clearing number from the leading RFF segment (if present).
        clearing_number: Option<String>,
        /// ALOCAT document message reference.
        message_ref: MessageRef,
    },

    /// The KoV §6.4 final-allocation window closed with no binding final
    /// ALOCAT on file. The imbalance for this gas day cannot be settled.
    FinalAllocationOverdue {
        /// Gas day whose final allocation is missing.
        gas_day: GasDay,
        /// Deadline that fired, for audit.
        deadline_id: DeadlineId,
        /// Deadline label (always [`FINAL_ALOCAT_DEADLINE_LABEL`]).
        label: Box<str>,
    },
}

impl EventPayload for AllocationEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AllocationReceived { .. } => "GaBiGasAllocationReceived",
            Self::FinalAllocationOverdue { .. } => "GaBiGasFinalAllocationOverdue",
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Current state of a GaBi Gas Allocation process stream.
///
/// # Lifecycle
///
/// ```text
/// New
///  └─ AllocationReceived    (terminal — no response required)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum AllocationState {
    /// No ALOCAT received yet.
    #[default]
    New,
    /// At least one ALOCAT recorded. The payload is the **most recent** one —
    /// its [`AllocationVersion`] says whether that is the initial allocation, a
    /// correction, or the binding final.
    Recorded(Box<AllocationData>),
    /// The KoV §6.4 window closed with no final allocation on file.
    FinalOverdue(Box<AllocationData>),
}

impl AllocationState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Recorded(_) => "Recorded",
            Self::FinalOverdue(_) => "FinalOverdue",
        }
    }

    /// The most recent allocation on file, if any.
    #[must_use]
    pub fn latest(&self) -> Option<&AllocationData> {
        match self {
            Self::New => None,
            Self::Recorded(d) | Self::FinalOverdue(d) => Some(d),
        }
    }

    /// `true` once the binding final allocation has been recorded. No further
    /// correction is admissible after this point (KoV §6.4).
    #[must_use]
    pub fn is_settled(&self) -> bool {
        matches!(
            self.latest().map(|d| d.version),
            Some(AllocationVersion::Final)
        )
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands for the GaBi Gas Allocation workflow.
///
/// [`Workflow::handle`] is pure — no I/O.
#[derive(Clone)]
pub enum AllocationCommand {
    /// An inbound ALOCAT was received from FNB, MGV, or VNB.
    ///
    /// Constructed by the DVGW adapter in `makod` when an ALOCAT arrives on
    /// the inbound channel.
    ReceiveAlocat {
        /// Synthetic PID (90001 / 90002 / 90003).
        synthetic_pid: u32,
        /// EIC code of the sending party (FNB / MGV / VNB).
        sender_eic: String,
        /// EIC code of the receiving party (BKV / FNB).
        receiver_eic: String,
        /// Gas day or allocation period.
        gas_day: GasDay,
        /// Version of this allocation (initial / correction / final).
        ///
        /// Callers should determine the version from the ALOCAT message
        /// sequence number (UNH DE 0062) or explicit correction qualifier.
        version: AllocationVersion,
        /// Allocated quantity (if present in the ALOCAT).
        allocated_quantity: Option<GasQuantity>,
        /// Clearing number from the leading RFF segment (if present).
        clearing_number: Option<String>,
        /// ALOCAT document message reference.
        message_ref: MessageRef,
    },

    /// The KoV §6.4 final-allocation deadline fired.
    TimeoutExpired {
        /// Deadline identifier, for audit.
        deadline_id: DeadlineId,
        /// Deadline label (always [`FINAL_ALOCAT_DEADLINE_LABEL`]).
        label: Box<str>,
    },
}

impl CommandPayload for AllocationCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GaBi Gas Allocation workflow.
///
/// Records a single ALOCAT allocation message from FNB, MGV, or VNB.
/// No response is required — this is a receive-and-record workflow.
pub struct GaBiGasAllocationWorkflow;

impl Workflow for GaBiGasAllocationWorkflow {
    type State = AllocationState;
    type Event = AllocationEvent;
    type Command = AllocationCommand;

    /// Fire the timeout only while a final allocation is still outstanding.
    /// A settled or already-overdue stream must not re-raise it.
    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (FINAL_ALOCAT_DEADLINE_LABEL, AllocationState::New)
            | (FINAL_ALOCAT_DEADLINE_LABEL, AllocationState::Recorded(_))
                if !state.is_settled() =>
            {
                Some(AllocationCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            AllocationEvent::FinalAllocationOverdue { .. } => match state {
                AllocationState::Recorded(d) => AllocationState::FinalOverdue(d),
                other => other,
            },
            AllocationEvent::AllocationReceived {
                synthetic_pid,
                allocation_type,
                sender_eic,
                receiver_eic,
                gas_day,
                version,
                allocated_quantity,
                clearing_number,
                message_ref,
            } => AllocationState::Recorded(Box::new(AllocationData {
                synthetic_pid: *synthetic_pid,
                allocation_type: *allocation_type,
                sender_eic: sender_eic.clone(),
                receiver_eic: receiver_eic.clone(),
                gas_day: *gas_day,
                version: *version,
                allocated_quantity: allocated_quantity.clone(),
                clearing_number: clearing_number.clone(),
                message_ref: message_ref.clone(),
            })),
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            AllocationCommand::ReceiveAlocat {
                synthetic_pid,
                sender_eic,
                receiver_eic,
                gas_day,
                version,
                allocated_quantity,
                clearing_number,
                message_ref,
            } => {
                // KoV §6.4 admits corrections and then one binding final
                // allocation, so a second ALOCAT for the same gas day is the
                // normal case, not an error. Only a message *after* the final
                // one is refused — the final allocation settles the imbalance.
                if state.is_settled() {
                    return Err(WorkflowError::rejected(
                        "the binding final allocation is already on file; \
                         KoV §6.4 admits no further correction",
                    ));
                }
                let allocation_type = AllocationType::from_pid(synthetic_pid).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "PID {synthetic_pid} is not a valid ALOCAT PID \
                         (expected 90001, 90002, or 90003)"
                    ))
                })?;
                Ok(vec![AllocationEvent::AllocationReceived {
                    synthetic_pid,
                    allocation_type,
                    sender_eic,
                    receiver_eic,
                    gas_day,
                    version,
                    allocated_quantity,
                    clearing_number,
                    message_ref,
                }]
                .into())
            }

            AllocationCommand::TimeoutExpired { deadline_id, label } => {
                // Idempotent: a settled or already-overdue stream records nothing.
                let AllocationState::Recorded(data) = state else {
                    return Ok(WorkflowOutput::events(vec![]));
                };
                if state.is_settled() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![AllocationEvent::FinalAllocationOverdue {
                    gas_day: data.gas_day,
                    deadline_id,
                    label,
                }]
                .into())
            }
        }
    }
}
