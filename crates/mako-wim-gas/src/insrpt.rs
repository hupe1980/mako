//! WiM Gas INSRPT Störungsmeldung workflow — fault/inspection reports for Gas MSB.
//!
//! Handles INSRPT messages used by the gMSB (Gas Messstellenbetreiber) process
//! for fault reporting between LF and gMSB in the Gas metering domain.
//!
//! ## Commodity isolation
//!
//! The Strom INSRPT workflow in `mako-wim` handles PIDs 23001–23012 at the
//! Strom APERAK Frist of **5 Werktage** (BK6-24-174). Gas fault reports require
//! the Gas APERAK Frist of **10 Werktage** (BK7-24-01-009).
//!
//! This module handles:
//!
//! | PID   | Description                             | Commodity      |
//! |-------|-----------------------------------------|----------------|
//! | 23001 | Störungsmeldung (LF → gMSB)             | Gas **and** Strom |
//! | 23003 | Ablehnung (gMSB → LF)                   | Gas **and** Strom |
//! | 23004 | Bestätigung (gMSB → LF)                 | Gas **and** Strom |
//! | 23005 | Ablehnung Variante (gMSB → LF)          | Gas **only**   |
//! | 23008 | Ergebnisbericht (gMSB → LF)             | Gas **and** Strom |
//! | 23009 | Ergebnisbericht Variante (gMSB → LF)    | Gas **only**   |
//!
//! **In a combined Strom+Gas deployment:** PIDs 23001/23003/23004/23008 are
//! registered by `mako-wim` (`wim-insrpt`, 5 WT). PIDs 23005/23009 (Gas-only)
//! are always registered here (`wim-gas-insrpt`, 10 WT). The shared PIDs
//! route to the Strom workflow in a combined deployment; only a Gas-only
//! deployment gets full 10 WT routing for all INSRPT messages.
//!
//! **In a Gas-only gMSB deployment:** `mako-wim` is not loaded. All six PIDs
//! are registered here with the correct Gas APERAK Frist of 10 Werktage.
//!
//! ## Regulatory basis
//!
//! - **BK7-24-01-009** — WiM Gas (BNetzA ruling, 12.09.2025)
//! - **INSRPT AHB 1.x** — EDI@Energy inspection report format
//! - **APERAK Frist: 10 Werktage** per BK7-24-01-009 (vs 5 Werktage for WiM Strom)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the WiM Gas INSRPT Störungsmeldung workflow.
pub const WORKFLOW_NAME: &str = "wim-gas-insrpt";

/// Deadline label for the WiM Gas INSRPT response window.
///
/// Per BK7-24-01-009 the gMSB must respond to a Störungsmeldung within
/// **10 Werktage**. Register a [`mako_engine::deadline::Deadline`] with
/// this label immediately after `StorungsmeldungInitiated`.
pub const ANTWORT_WINDOW_LABEL: &str = "wim-gas-insrpt-antwort-10-werktage";

/// Gas-only INSRPT Prüfidentifikatoren (not present in Strom INSRPT AHB).
///
/// These PIDs are always registered by `WimGasModule` — they cannot conflict
/// with `mako-wim` registrations because `mako-wim` does not register them.
pub const INSRPT_GAS_ONLY_PIDS: &[u32] = &[23005, 23009];

/// INSRPT Prüfidentifikatoren shared between WiM Strom and WiM Gas.
///
/// In a **combined Strom+Gas deployment** these are registered by `mako-wim`
/// (`wim-insrpt`, 5 WT). In a **Gas-only deployment** (no `mako-wim`) these
/// must be registered by `WimGasModule` pointing to `wim-gas-insrpt` (10 WT).
///
/// The `WimGasModule` does not automatically register these in a combined
/// deployment because `register()` is last-write-wins — registering here after
/// `mako-wim` would silently overwrite the Strom routing. Operators running a
/// Gas-only gMSB must load only `mako-wim-gas` (not `mako-wim`) to activate
/// the full 10 WT Gas INSRPT routing.
pub const INSRPT_SHARED_PIDS: &[u32] = &[23001, 23003, 23004, 23008];

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a WiM Gas Störungsmeldung is initiated.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasStorungsmeldungData {
    /// BDEW Prüfidentifikator of the INSRPT.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the gMSB the Störungsmeldung was directed to.
    pub msb_mp_id: MarktpartnerCode,
    /// EDIFACT document date (YYYYMMDD).
    pub document_date: String,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Gas INSRPT workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasStorungsmeldungEvent {
    /// LF sent a Störungsmeldung to the gMSB.
    StorungsmeldungInitiated {
        /// Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the gMSB.
        msb_mp_id: MarktpartnerCode,
        /// Document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// gMSB sent an Ablehnung (rejection) — PIDs 23003 or 23005.
    Abgelehnt {
        /// Response INSRPT reference.
        response_ref: MessageRef,
        /// Rejection reason from INSRPT data.
        reason: Option<String>,
    },
    /// gMSB confirmed receipt and will investigate — PID 23004.
    Bestaetigt {
        /// Response INSRPT reference.
        response_ref: MessageRef,
    },
    /// gMSB delivered an Ergebnisbericht — PIDs 23008 or 23009.
    ErgebnisberichtErhalten {
        /// Response INSRPT reference.
        response_ref: MessageRef,
    },
    /// 10-Werktage deadline expired without a response from gMSB.
    DeadlineExpired {
        /// Deadline identifier.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: String,
    },
}

impl EventPayload for GasStorungsmeldungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::StorungsmeldungInitiated { .. } => "GasStorungsmeldungInitiated",
            Self::Abgelehnt { .. } => "GasStorungsmeldungAbgelehnt",
            Self::Bestaetigt { .. } => "GasStorungsmeldungBestaetigt",
            Self::ErgebnisberichtErhalten { .. } => "GasStorungsmeldungErgebnisberichtErhalten",
            Self::DeadlineExpired { .. } => "GasStorungsmeldungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// State of a WiM Gas INSRPT Störungsmeldung process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GasStorungsmeldungState {
    /// No events yet.
    #[default]
    New,
    /// Störungsmeldung sent to gMSB; awaiting response within 10 Werktage.
    AwaitingResponse(GasStorungsmeldungData),
    /// gMSB rejected the Störungsmeldung (terminal).
    Rejected(GasStorungsmeldungData),
    /// gMSB confirmed; Ergebnisbericht expected (terminal or follow-up pending).
    Completed(GasStorungsmeldungData),
    /// Deadline expired without gMSB response (terminal).
    DeadlineExpired,
}

impl GasStorungsmeldungState {
    /// Stable label for the current state variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AwaitingResponse(_) => "AwaitingResponse",
            Self::Rejected(_) => "Rejected",
            Self::Completed(_) => "Completed",
            Self::DeadlineExpired => "DeadlineExpired",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Gas INSRPT workflow.
#[derive(Clone)]
pub enum GasStorungsmeldungCommand {
    /// LF sends a Störungsmeldung to the gMSB.
    SendStorungsmeldung {
        /// Prüfidentifikator (must be in `INSRPT_GAS_ONLY_PIDS` or `INSRPT_SHARED_PIDS`).
        pid: Pruefidentifikator,
        /// GLN of the gMSB.
        msb_mp_id: MarktpartnerCode,
        /// Document date.
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// gMSB response to a Störungsmeldung received.
    ReceiveResponse {
        /// Response PID (23003, 23004, 23005, 23008, or 23009).
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the response.
        response_ref: MessageRef,
        /// Optional rejection reason (only for Ablehnung responses).
        reason: Option<String>,
    },
    /// 10-Werktage deadline expired without response.
    TimeoutExpired {
        /// Deadline identifier.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
        /// Outbox entry to emit (e.g. escalation notification).
        outbox: Option<PendingOutbox>,
    },
}

impl CommandPayload for GasStorungsmeldungCommand {}

// ── All PIDs ──────────────────────────────────────────────────────────────────

/// All PIDs handled by `WimGasInsrptWorkflow`.
///
/// Union of [`INSRPT_GAS_ONLY_PIDS`] and [`INSRPT_SHARED_PIDS`].
const ALL_PIDS: &[u32] = &[23001, 23003, 23004, 23005, 23008, 23009];

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Gas INSRPT Störungsmeldung workflow.
///
/// Models the fault-reporting exchange between LF and gMSB under the Gas APERAK
/// Frist of **10 Werktage** per BK7-24-01-009.
pub struct WimGasInsrptWorkflow;

impl Workflow for WimGasInsrptWorkflow {
    type State = GasStorungsmeldungState;
    type Event = GasStorungsmeldungEvent;
    type Command = GasStorungsmeldungCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GasStorungsmeldungEvent::StorungsmeldungInitiated {
                pruefidentifikator,
                msb_mp_id,
                document_date,
                message_ref,
            } => GasStorungsmeldungState::AwaitingResponse(GasStorungsmeldungData {
                pruefidentifikator: *pruefidentifikator,
                msb_mp_id: msb_mp_id.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            }),
            GasStorungsmeldungEvent::Abgelehnt { .. } => match state {
                GasStorungsmeldungState::AwaitingResponse(data) => {
                    GasStorungsmeldungState::Rejected(data)
                }
                other => other,
            },
            GasStorungsmeldungEvent::Bestaetigt { .. }
            | GasStorungsmeldungEvent::ErgebnisberichtErhalten { .. } => match state {
                GasStorungsmeldungState::AwaitingResponse(data) => {
                    GasStorungsmeldungState::Completed(data)
                }
                other => other,
            },
            GasStorungsmeldungEvent::DeadlineExpired { .. } => {
                GasStorungsmeldungState::DeadlineExpired
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            GasStorungsmeldungCommand::SendStorungsmeldung {
                pid,
                msb_mp_id,
                document_date,
                message_ref,
            } => {
                if !matches!(state, GasStorungsmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ALL_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a Gas INSRPT PID (23001/23003–23005/23008–23009)",
                    )));
                }
                Ok(vec![GasStorungsmeldungEvent::StorungsmeldungInitiated {
                    pruefidentifikator: pid,
                    msb_mp_id,
                    document_date,
                    message_ref,
                }]
                .into())
            }
            GasStorungsmeldungCommand::ReceiveResponse {
                pid,
                response_ref,
                reason,
            } => {
                if !matches!(state, GasStorungsmeldungState::AwaitingResponse(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AwaitingResponse",
                        state.label(),
                    ));
                }
                let event = match pid.as_u32() {
                    // Ablehnung (Gas-specific variant 23005 included)
                    23003 | 23005 => GasStorungsmeldungEvent::Abgelehnt {
                        response_ref,
                        reason,
                    },
                    // Bestätigung
                    23004 => GasStorungsmeldungEvent::Bestaetigt { response_ref },
                    // Ergebnisbericht (Gas-specific variant 23009 included)
                    23008 | 23009 => {
                        GasStorungsmeldungEvent::ErgebnisberichtErhalten { response_ref }
                    }
                    other => {
                        return Err(WorkflowError::rejected(format!(
                            "PID {other} is not a valid Gas INSRPT response PID",
                        )));
                    }
                };
                Ok(vec![event].into())
            }
            GasStorungsmeldungCommand::TimeoutExpired {
                deadline_id,
                label,
                outbox,
            } => {
                if matches!(
                    state,
                    GasStorungsmeldungState::DeadlineExpired
                        | GasStorungsmeldungState::Rejected(_)
                        | GasStorungsmeldungState::Completed(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "AwaitingResponse",
                        state.label(),
                    ));
                }
                let mut output =
                    WorkflowOutput::from(vec![GasStorungsmeldungEvent::DeadlineExpired {
                        deadline_id,
                        label: label.to_string(),
                    }]);
                if let Some(ob) = outbox {
                    output.outbox.push(ob);
                }
                Ok(output)
            }
        }
    }
}
