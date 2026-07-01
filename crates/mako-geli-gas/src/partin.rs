//! GeLi Gas PARTIN Kommunikationsdaten workflow — Gas market participant master data.
//!
//! Handles inbound PARTIN messages that exchange **Gas** market participant
//! communication data (Kommunikationsdaten) between LF Gas, GNB, gMSB, and MGV.
//!
//! ## Why separate from GPKE?
//!
//! Strom and Gas market participants are registered with **different GLNs**.
//! A GNB (Gasnetzbetreiber) has a distinct BDEW Marktpartner-ID from an NB
//! (Netzbetreiber Strom), even when operated by the same legal entity.
//! Processing Gas PARTIN in a Strom-only deployment (`mako-gpke` only)
//! would pollute the PartnerStore with Gas party records that are irrelevant
//! to Strom operations. The commodity boundary is enforced here by registering
//! Gas PARTIN PIDs exclusively in `mako-geli-gas`.
//!
//! ## Prüfidentifikatoren handled
//!
//! | PID   | Description                                    | Direction       |
//! |-------|------------------------------------------------|-----------------|
//! | 37008 | Kommunikationsdaten des LF Gas                 | LF → NB/MSB     |
//! | 37009 | Kommunikationsdaten des NB Gas (GNB)           | NB → LF         |
//! | 37010 | Kommunikationsdaten des MSB Gas (gMSB)         | MSB → LF        |
//! | 37011 | Kommunikationsdaten des Marktgebietsverantw.   | MGV → LF/NB     |
//! | 37012 | Spartenübergreifende Kommunikationsdaten NB Gas | NB → MSB        |
//! | 37013 | Spartenübergreifende Kommunikationsdaten MSB Gas| MSB → MSB       |
//! | 37014 | Spartenübergreifende Kommunikationsdaten MSB Strom (GeLi Gas AHB) | MSB → MSB/NB |
//!
//! ## Commodity isolation
//!
//! - A **Strom-only** deployment loads only `mako-gpke` → Gas PARTIN (37008–37014)
//!   is dead-lettered. Gas party records are never created.
//! - A **Gas-only** deployment loads only `mako-geli-gas` → Strom PARTIN
//!   (37000–37006) is dead-lettered.
//! - A **combined** deployment loads both modules → Strom PARTIN routes to
//!   `gpke-partin`; Gas PARTIN routes to `geli-gas-partin`. Each module updates
//!   the shared PartnerStore with its own commodity's party records.
//!
//! ## Regulatory basis
//!
//! - **BK7-24-01-009** — GeLi Gas 3.0 (PARTIN Gas)
//! - **PARTIN 1.x** — EDI@Energy party information format
//! - **BDEW PARTIN AHB 1.0f** — PIDs 37008–37014, GeLi Gas process family

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the GeLi Gas PARTIN workflow.
pub const WORKFLOW_NAME: &str = "geli-gas-partin";

/// PARTIN Prüfidentifikatoren for Gas market participants.
///
/// Registered exclusively by `GeliGasModule`. A Strom-only deployment (loading
/// only `mako-gpke`) will never register these PIDs — Gas PARTIN is cleanly
/// isolated to the Gas domain.
///
/// | PID   | Market role (PARTIN AHB) |
/// |-------|--------------------------|
/// | 37008 | Lieferant Gas            |
/// | 37009 | Netzbetreiber Gas (GNB)  |
/// | 37010 | MSB Gas (gMSB)           |
/// | 37011 | Marktgebietsverantwortlicher (MGV) |
/// | 37012 | Cross-commodity NB Gas   |
/// | 37013 | Cross-commodity MSB Gas  |
/// | 37014 | Cross-commodity MSB Strom (GeLi Gas AHB scope) |
pub const PARTIN_GAS_PIDS: &[u32] = &[37008, 37009, 37010, 37011, 37012, 37013, 37014];

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a Gas PARTIN message is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasKommunikationsdatenData {
    /// BDEW Prüfidentifikator of the inbound PARTIN.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the sending Gas market participant.
    pub sender: MarktpartnerCode,
    /// EDIFACT document date (YYYYMMDD).
    pub document_date: String,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas PARTIN workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasKommunikationsdatenEvent {
    /// Inbound Gas PARTIN received.
    PartinErhalten {
        /// BDEW Prüfidentifikator (37008–37014).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the sending Gas party.
        sender: MarktpartnerCode,
        /// Document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// AHB validation passed; Gas communication data is usable.
    ValidationPassed {
        /// Message reference of the validated PARTIN.
        message_ref: MessageRef,
    },
    /// AHB validation failed.
    ValidationFailed {
        /// Human-readable validation error summary.
        reason: String,
    },
}

impl EventPayload for GasKommunikationsdatenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::PartinErhalten { .. } => "GasKommunikationsdatenPartinErhalten",
            Self::ValidationPassed { .. } => "GasKommunikationsdatenValidationPassed",
            Self::ValidationFailed { .. } => "GasKommunikationsdatenValidationFailed",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a Gas PARTIN Kommunikationsdaten process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GasKommunikationsdatenState {
    /// No events yet.
    New,
    /// Gas PARTIN received; awaiting validation.
    PartinErhalten(GasKommunikationsdatenData),
    /// Validation passed; Gas party data available (terminal).
    ValidationPassed(GasKommunikationsdatenData),
    /// Validation failed (terminal).
    ValidationFailed {
        /// Validation error reason.
        reason: String,
    },
}

impl Default for GasKommunikationsdatenState {
    fn default() -> Self {
        Self::New
    }
}

impl GasKommunikationsdatenState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::PartinErhalten(_) => "PartinErhalten",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas PARTIN workflow.
#[derive(Clone)]
pub enum GasKommunikationsdatenCommand {
    /// Inbound Gas PARTIN message received.
    ReceivePartin {
        /// BDEW Prüfidentifikator (must be in 37008–37014).
        pid: Pruefidentifikator,
        /// GLN of the sending Gas party.
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

impl CommandPayload for GasKommunikationsdatenCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas PARTIN Kommunikationsdaten workflow.
///
/// Records inbound Gas PARTIN messages (PIDs 37008–37014) exchanging Gas market
/// participant communication data (AS4 endpoints, GLNs, contact addresses)
/// between LF Gas, GNB, gMSB, and MGV.
///
/// This workflow is the Gas-side counterpart to `GpkePartinWorkflow` in
/// `mako-gpke`. The two workflows are commodity-isolated:
/// - `GpkePartinWorkflow` handles PIDs 37000–37006 (Strom).
/// - `GeliGasPartinWorkflow` handles PIDs 37008–37014 (Gas).
/// - In a combined Strom+Gas deployment both are active simultaneously.
/// - In a Strom-only deployment, `GeliGasPartinWorkflow` is never loaded.
/// - In a Gas-only deployment, `GpkePartinWorkflow` is never loaded.
pub struct GeliGasPartinWorkflow;

impl Workflow for GeliGasPartinWorkflow {
    type State = GasKommunikationsdatenState;
    type Event = GasKommunikationsdatenEvent;
    type Command = GasKommunikationsdatenCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GasKommunikationsdatenEvent::PartinErhalten {
                pruefidentifikator,
                sender,
                document_date,
                message_ref,
            } => GasKommunikationsdatenState::PartinErhalten(GasKommunikationsdatenData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            }),
            GasKommunikationsdatenEvent::ValidationPassed { .. } => match state {
                GasKommunikationsdatenState::PartinErhalten(data) => {
                    GasKommunikationsdatenState::ValidationPassed(data)
                }
                other => other,
            },
            GasKommunikationsdatenEvent::ValidationFailed { reason } => {
                GasKommunikationsdatenState::ValidationFailed {
                    reason: reason.clone(),
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            GasKommunikationsdatenCommand::ReceivePartin {
                pid,
                sender,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GasKommunikationsdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !PARTIN_GAS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a Gas PARTIN PID (37008–37014)",
                    )));
                }
                let mut events = vec![GasKommunikationsdatenEvent::PartinErhalten {
                    pruefidentifikator: pid,
                    sender,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(GasKommunikationsdatenEvent::ValidationPassed { message_ref });
                } else {
                    events.push(GasKommunikationsdatenEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
        }
    }
}
