//! GPKE UTILTS Konfigurationsdaten workflow — tariff/meter configuration exchange.
//!
//! Handles inbound UTILTS messages that convey metering configuration definitions
//! used in GPKE Teil 3 (NB/MSB sends Zählzeitdefinitionen, Schaltzeitdefinitionen,
//! and Leistungskurvendefinitionen to LF) and WiM Strom Teil 2 (Berechnungsformel).
//!
//! When `makod` operates as a **Lieferant (LF)**, the NB or MSB sends UTILTS
//! configuration data that the LF needs for billing calculation and metering
//! point administration.
//!
//! ## Prüfidentifikatoren handled
//!
//! | PID   | Description                                  | Direction   | Domain        |
//! |-------|----------------------------------------------|-------------|---------------|
//! | 25001 | Berechnungsformel                            | NB → LF     | WiM Strom/NBW |
//! | 25004 | Übermittlung Übersicht Zählzeitdefinitionen  | NB/MSB → LF | GPKE Teil 3   |
//! | 25005 | Übermittlung ausgerollte Zählzeitdefinition  | NB/MSB → LF | GPKE Teil 3   |
//! | 25006 | Übermittlung Übersicht Schaltzeitdefinitionen| NB/MSB → LF | GPKE Teil 3   |
//! | 25007 | Übermittlung Leistungskurvendefinitionen     | NB/MSB → LF | GPKE Teil 3   |
//! | 25008 | Übermittlung ausgerollte Schaltzeitdefinition| NB/MSB → LF | GPKE Teil 3   |
//! | 25009 | Übermittlung ausgerollte Leistungskurve      | NB/MSB → LF | GPKE Teil 3   |
//! | 25010 | Übermittlung Zählzeitdefinitionen (combined) | NB/MSB → LF | GPKE Teil 3   |
//!
//! ## Regulatory basis
//!
//! - **BDEW GPKE Teil 3** — Konfigurationseinrichtung (BK6-22-024)
//! - **WiM Strom Teil 2 / AWH NBW** — Berechnungsformel
//! - **UTILTS S1.x** — EDI@Energy utility time series format

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the UTILTS Konfigurationsdaten workflow.
pub const WORKFLOW_NAME: &str = "gpke-utilts";

/// UTILTS Prüfidentifikatoren for configuration data delivery to LF.
///
/// | PID   | Description                                  |
/// |-------|----------------------------------------------|
/// | 25001 | Berechnungsformel (NB → LF)                  |
/// | 25004 | Übersicht Zählzeitdefinitionen               |
/// | 25005 | Ausgerollte Zählzeitdefinition               |
/// | 25006 | Übersicht Schaltzeitdefinitionen             |
/// | 25007 | Übersicht Leistungskurvendefinitionen        |
/// | 25008 | Ausgerollte Schaltzeitdefinition             |
/// | 25009 | Ausgerollte Leistungskurvendefinition        |
/// | 25010 | Zählzeitdefinitionen (combined)              |
pub const UTILTS_PIDS: &[u32] = &[25001, 25004, 25005, 25006, 25007, 25008, 25009, 25010];

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a UTILTS configuration message is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UtiltsKonfigData {
    /// BDEW Prüfidentifikator of the inbound UTILTS.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the sending NB or MSB.
    pub sender: MarktpartnerCode,
    /// EDIFACT document date (YYYYMMDD).
    pub document_date: String,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the UTILTS Konfigurationsdaten workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum UtiltsKonfigEvent {
    /// Inbound UTILTS configuration data received.
    UtiltsErhalten {
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the sender (NB or MSB).
        sender: MarktpartnerCode,
        /// Document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// AHB validation passed; configuration data is usable.
    ValidationPassed {
        /// Message reference of the validated UTILTS.
        message_ref: MessageRef,
    },
    /// AHB validation failed.
    ValidationFailed {
        /// Human-readable validation error summary.
        reason: String,
    },
}

impl EventPayload for UtiltsKonfigEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::UtiltsErhalten { .. } => "UtiltsKonfigErhalten",
            Self::ValidationPassed { .. } => "UtiltsKonfigValidationPassed",
            Self::ValidationFailed { .. } => "UtiltsKonfigValidationFailed",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a UTILTS Konfigurationsdaten process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum UtiltsKonfigState {
    /// No events yet.
    #[default]
    New,
    /// UTILTS received; awaiting validation.
    UtiltsErhalten(UtiltsKonfigData),
    /// Validation passed; configuration data available (terminal).
    ValidationPassed(UtiltsKonfigData),
    /// Validation failed (terminal).
    ValidationFailed {
        /// Validation error reason.
        reason: String,
    },
}

impl UtiltsKonfigState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::UtiltsErhalten(_) => "UtiltsErhalten",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the UTILTS Konfigurationsdaten workflow.
#[derive(Clone)]
pub enum UtiltsKonfigCommand {
    /// Inbound UTILTS configuration data received from NB or MSB.
    ReceiveUtilts {
        /// BDEW Prüfidentifikator of the inbound UTILTS.
        pid: Pruefidentifikator,
        /// GLN of the NB or MSB sender.
        sender: MarktpartnerCode,
        /// EDIFACT document date.
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if AHB validation passed.
        validation_passed: bool,
        /// Human-readable validation errors (if any).
        validation_errors: Vec<String>,
    },
}

impl CommandPayload for UtiltsKonfigCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// UTILTS Konfigurationsdaten workflow — handles inbound UTILTS for GPKE Teil 3.
///
/// Records inbound UTILTS messages that convey Zählzeit-, Schaltzeit-, and
/// Leistungskurvendefinitionen from NB/MSB to LF for billing and metering
/// point administration.
pub struct GpkeUtiltsWorkflow;

impl Workflow for GpkeUtiltsWorkflow {
    type State = UtiltsKonfigState;
    type Event = UtiltsKonfigEvent;
    type Command = UtiltsKonfigCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            UtiltsKonfigEvent::UtiltsErhalten {
                pruefidentifikator,
                sender,
                document_date,
                message_ref,
            } => UtiltsKonfigState::UtiltsErhalten(UtiltsKonfigData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            }),
            UtiltsKonfigEvent::ValidationPassed { .. } => match state {
                UtiltsKonfigState::UtiltsErhalten(data) => {
                    UtiltsKonfigState::ValidationPassed(data)
                }
                other => other,
            },
            UtiltsKonfigEvent::ValidationFailed { reason } => UtiltsKonfigState::ValidationFailed {
                reason: reason.clone(),
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            UtiltsKonfigCommand::ReceiveUtilts {
                pid,
                sender,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, UtiltsKonfigState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !UTILTS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a handled UTILTS PID",
                    )));
                }
                let mut events = vec![UtiltsKonfigEvent::UtiltsErhalten {
                    pruefidentifikator: pid,
                    sender,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(UtiltsKonfigEvent::ValidationPassed { message_ref });
                } else {
                    events.push(UtiltsKonfigEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
        }
    }
}
