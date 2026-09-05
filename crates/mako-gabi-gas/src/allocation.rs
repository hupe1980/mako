//! GaBi Gas Allocation workflow — ALOCAT.
//!
//! Implements the receive-and-record side of gas quantity allocation governed
//! by the Kooperationsvereinbarung Gas (KoV) and the BNetzA GaBi Gas 2.1
//! framework (BK7-24-01-008).
//!
//! # Process overview
//!
//! An **allocation message** (ALOCAT) reports the allocated gas quantities for a
//! gas day. ALOCAT 5.11a publishes twenty-three Anwendungsfälle across five
//! directions — NB→MGV, MGV→BKV, ENB/ANB→NB, MGV→NB and NB→BKV. No response is
//! required.
//!
//! ```text
//! NB / MGV / ENB / ANB ──(ALOCAT 70001–70023)──→  MGV / BKV / NB
//! ```
//!
//! # Prüfidentifikatoren
//!
//! DVGW publishes real Prüfidentifikatoren in `SG1 RFF+Z13`; the routing list is
//! [`ALLOCATION_PIDS`], pinned to [`dvgw_edi::catalogue_for`] by test.
//! [`AllocationType`] derives the direction from the code.
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ Recorded ──(correction / final ALOCAT)──→ Recorded
//!       └─ FinalOverdue   [§47 KoV XV final-allocation deadline passed with no final]
//! ```
//!
//! No response is sent, but the process is **not** terminal on first receipt:
//! §46/§47 KoV XV let the Netzbetreiber correct an allocation and then confirm a binding
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
    outbox::PendingOutbox,
    types::MessageRef,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

use crate::domain::{GasDay, GasQuantity};

// ── Prüfidentifikator set ─────────────────────────────────────────────────────

/// Every DVGW Prüfidentifikator that routes to the allocation workflow.
///
/// ALOCAT 5.11a publishes twenty-three Anwendungsfälle across five
/// communication directions; `dvgw_edi::catalogue_for` carries the description
/// of each and a test in this module pins this list to it.
pub const ALLOCATION_PIDS: &[u32] = &[
    70001, 70002, 70003, 70004, 70005, 70006, 70007, 70008, 70009, 70010, 70011, 70012, 70013,
    70014, 70015, 70016, 70017, 70018, 70019, 70020, 70021, 70022, 70023,
];

/// Workflow key for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-allocation";

/// Deadline label for the §47 Ziffer 1 KoV XV final-allocation window.
///
/// The binding final allocation is due by the end of month M+2 at 12:00 CET;
/// register a [`mako_engine::deadline::Deadline`] with this label using
/// [`GasDay::finale_allokation_deadline_utc`] when the first ALOCAT for a gas day is
/// recorded. If it fires with no [`AllocationVersion::Final`] on file, the
/// FNB/MGV has missed a binding obligation.
pub const FINAL_ALOCAT_DEADLINE_LABEL: &str = "gabi-gas-final-alocat-deadline";

// ── Allocation type ───────────────────────────────────────────────────────────

/// Who sends this allocation to whom.
///
/// The five directions ALOCAT 5.11a publishes. Derived from the
/// Prüfidentifikator so downstream analysis need not re-parse the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationType {
    /// Netzbetreiber an Marktgebietsverantwortlichen (PIDs 70001–70010).
    NbAnMgv,
    /// Einspeise-/Ausspeisenetzbetreiber an Netzbetreiber (PIDs 70011, 70012).
    EnbAnbAnNb,
    /// Marktgebietsverantwortlicher an Bilanzkreisverantwortlichen (PIDs 70013–70020).
    MgvAnBkv,
    /// Marktgebietsverantwortlicher an Netzbetreiber (PIDs 70021, 70023).
    MgvAnNb,
    /// Netzbetreiber an Bilanzkreisverantwortlichen (PID 70022).
    NbAnBkv,
}

impl AllocationType {
    /// Derive from the Prüfidentifikator.
    ///
    /// Returns `None` for a code outside [`ALLOCATION_PIDS`].
    #[must_use]
    pub fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            70001..=70010 => Some(Self::NbAnMgv),
            70011 | 70012 => Some(Self::EnbAnbAnNb),
            70013..=70020 => Some(Self::MgvAnBkv),
            70021 | 70023 => Some(Self::MgvAnNb),
            70022 => Some(Self::NbAnBkv),
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
/// Source: Kooperationsvereinbarung Gas XV §§ 46, 47.
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
    /// The Prüfidentifikator that identifies the Anwendungsfall (70001–70023).
    pub pruefidentifikator: u32,
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
    /// Per §46/§47 KoV XV: the Netzbetreiber sends a daily allocation and may send
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
        /// The Prüfidentifikator (70001–70023); [`AllocationType`] derives the direction.
        pruefidentifikator: u32,
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

    /// The §47 Ziffer 1 KoV XV final-allocation window closed with no binding final
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
    /// The §47 Ziffer 1 KoV XV window closed with no final allocation on file.
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
    /// correction is admissible after this point (§47 Ziffer 1 KoV XV).
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
        /// The Prüfidentifikator (70001–70023).
        pruefidentifikator: u32,
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

    /// The §47 Ziffer 1 KoV XV final-allocation deadline fired.
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
                pruefidentifikator,
                allocation_type,
                sender_eic,
                receiver_eic,
                gas_day,
                version,
                allocated_quantity,
                clearing_number,
                message_ref,
            } => AllocationState::Recorded(Box::new(AllocationData {
                pruefidentifikator: *pruefidentifikator,
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
                pruefidentifikator,
                sender_eic,
                receiver_eic,
                gas_day,
                version,
                allocated_quantity,
                clearing_number,
                message_ref,
            } => {
                // §47 Ziffer 1 KoV XV admits corrections and then one binding final
                // allocation, so a second ALOCAT for the same gas day is the
                // normal case, not an error. Only a message *after* the final
                // one is refused — the final allocation settles the imbalance.
                if state.is_settled() {
                    return Err(WorkflowError::rejected(
                        "the binding final allocation is already on file; \
                         §47 Ziffer 1 KoV XV admits no further correction",
                    ));
                }
                let allocation_type =
                    AllocationType::from_pid(pruefidentifikator).ok_or_else(|| {
                        WorkflowError::rejected(format!(
                            "PID {pruefidentifikator} is not a valid ALOCAT PID \
                         (expected one of 70001–70023)"
                        ))
                    })?;
                // No outbox entry on any version, including the binding final
                // one — so a settled gas day leaves nothing on the ERP bus while
                // a missed § 47 deadline does. `de.gabi.allocation.completed`
                // (this arm with `AllocationVersion::Final`) and
                // `de.gabi.correction.created` (with `Correction(n)`) are
                // declared in `mako_events::gabi` for exactly that, and the
                // module there records what wiring each still needs.
                Ok(vec![AllocationEvent::AllocationReceived {
                    pruefidentifikator,
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
                // The missed obligation is the FNB's/MGV's, so nothing is sent
                // back on the wire — but the operator has to open a Clearingfall,
                // and that is a decision a human or an agent makes off-platform.
                // The outbox entry carries it out as `de.gabi.alocat.missing`
                // through the same ERP path every other notification uses.
                let notice = PendingOutbox::new(
                    "GabiFinalAllocationOverdue",
                    data.receiver_eic.as_str(),
                    serde_json::json!({
                        "gas_day":        data.gas_day,
                        "deadline_label": label.as_ref(),
                        "sender_eic":     data.sender_eic,
                        "receiver_eic":   data.receiver_eic,
                        "pruefidentifikator":  data.pruefidentifikator,
                    }),
                );
                Ok(WorkflowOutput {
                    events: vec![AllocationEvent::FinalAllocationOverdue {
                        gas_day: data.gas_day,
                        deadline_id,
                        label,
                    }],
                    outbox: vec![notice],
                    deadlines: Vec::new(),
                })
            }
        }
    }
}

#[cfg(test)]
mod pid_catalogue_conformance {
    use super::{ALLOCATION_PIDS, AllocationType};

    /// Pinned to the DVGW catalogue for the same reason as the nomination list:
    /// a drifted copy stops routing a published Anwendungsfall in silence.
    #[test]
    fn the_pid_list_matches_the_dvgw_catalogue() {
        let published: Vec<u32> = dvgw_edi::catalogue_for(dvgw_edi::DvgwMessageType::Alocat)
            .map(|info| info.pid)
            .collect();
        assert_eq!(published, ALLOCATION_PIDS);
    }

    /// Every routed code must resolve to a direction.
    #[test]
    fn every_routed_pid_resolves_to_a_direction() {
        for &pid in ALLOCATION_PIDS {
            assert!(
                AllocationType::from_pid(pid).is_some(),
                "PID {pid} routes here but has no direction"
            );
        }
    }

    /// The direction this crate derives must agree with the one DVGW published.
    #[test]
    fn the_derived_direction_agrees_with_the_published_one() {
        for info in dvgw_edi::catalogue_for(dvgw_edi::DvgwMessageType::Alocat) {
            let derived = AllocationType::from_pid(info.pid).expect("catalogued PID");
            let expected = match info.direction {
                "NB an MGV" => AllocationType::NbAnMgv,
                "ENB/ANB an NB" => AllocationType::EnbAnbAnNb,
                "MGV an BKV" => AllocationType::MgvAnBkv,
                "MGV an NB" => AllocationType::MgvAnNb,
                "NB an BKV" => AllocationType::NbAnBkv,
                other => panic!("PID {} has an unmapped direction {other:?}", info.pid),
            };
            assert_eq!(derived, expected, "PID {} direction disagrees", info.pid);
        }
    }
}
