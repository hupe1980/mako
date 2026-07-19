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
//!  └─ AllocationReceived   [terminal — no response required]
//! ```
//!
//! # Regulatory basis
//!
//! - **Kooperationsvereinbarung Gas (KoV)** — allocation reporting deadlines
//! - **BNetzA BK7-24-01-008** — GaBi Gas 2.1 ruling
//! - **DVGW ALOCAT 5.11a** — message format (valid from 2024-10-01)

use mako_engine::{
    error::WorkflowError,
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
}

impl EventPayload for AllocationEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AllocationReceived { .. } => "GaBiGasAllocationReceived",
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
    /// ALOCAT received and recorded (terminal).
    AllocationReceived(Box<AllocationData>),
}

impl AllocationState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AllocationReceived(_) => "AllocationReceived",
        }
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

    fn apply(_state: Self::State, event: &Self::Event) -> Self::State {
        match event {
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
            } => AllocationState::AllocationReceived(Box::new(AllocationData {
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
                if !matches!(state, AllocationState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
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
        }
    }
}
