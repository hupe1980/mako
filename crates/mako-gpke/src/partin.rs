//! GPKE PARTIN Kommunikationsdaten workflow — master data exchange (GPKE Teil 4).
//!
//! Handles inbound PARTIN messages that exchange Strom market participant
//! communication data (Kommunikationsdaten) between LF, NB, MSB, and ÜNB.
//!
//! ## Prüfidentifikatoren handled
//!
//! | PID   | Description                            | Direction       | Domain      |
//! |-------|----------------------------------------|-----------------|-------------|
//! | 37000 | Kommunikationsdaten des LF Strom       | LF → NB/MSB     | GPKE Teil 4 |
//! | 37001 | Kommunikationsdaten des NB Strom       | NB → LF         | GPKE Teil 4 |
//! | 37002 | Kommunikationsdaten des MSB Strom      | MSB → LF        | GPKE Teil 4 |
//! | 37003 | Kommunikationsdaten des MSBN Strom     | MSBN → LF       | GPKE Teil 4 |
//! | 37004 | Kommunikationsdaten des MSBA Strom     | MSBA → LF       | GPKE Teil 4 |
//! | 37005 | Kommunikationsdaten des ÜNB Strom      | ÜNB → LF        | GPKE Teil 4 |
//! | 37006 | Kommunikationsdaten des BKV Strom      | BKV → LF/NB     | GPKE Teil 4 |
//!
//! Gas PARTIN PIDs 37008–37014 are handled by `mako-geli-gas` (`geli-gas-partin`).
//! Gas party GLNs differ from Strom party GLNs; keeping them separate ensures
//! a Strom-only deployment never pollutes the PartnerStore with Gas records.
//!
//! ## Regulatory basis
//!
//! - **BDEW GPKE Teil 4** — Stamm- und Bewegungsdaten Strom
//! - **PARTIN 1.x** — EDI@Energy party information format

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the PARTIN Kommunikationsdaten workflow.
pub const WORKFLOW_NAME: &str = "gpke-partin";

/// PARTIN Prüfidentifikatoren for Strom market participants.
///
/// - 37000: LF sends own communication data (inbound when NB/MSB receives from LF)
/// - 37001: NB sends communication data to LF (LF receives)
/// - 37002: MSB sends communication data to LF (LF receives)
/// - 37003: MSBN sends communication data (multi-party)
/// - 37004: MSBA sends communication data (multi-party)
/// - 37005: ÜNB sends communication data to LF (LF receives)
/// - 37006: BKV Strom sends communication data
///
/// Gas PARTIN PIDs (37008–37014) are owned by `mako-geli-gas` (`geli-gas-partin`).
pub const PARTIN_STROM_PIDS: &[u32] = &[37000, 37001, 37002, 37003, 37004, 37005, 37006];

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a PARTIN message is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KommunikationsdatenData {
    /// BDEW Prüfidentifikator of the inbound PARTIN.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the sending market participant.
    pub sender: MarktpartnerCode,
    /// EDIFACT document date (YYYYMMDD).
    pub document_date: String,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the PARTIN Kommunikationsdaten workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KommunikationsdatenEvent {
    /// Inbound PARTIN received.
    PartinErhalten {
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the sending party.
        sender: MarktpartnerCode,
        /// Document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// AHB validation passed; communication data is usable.
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

impl EventPayload for KommunikationsdatenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::PartinErhalten { .. } => "KommunikationsdatenPartinErhalten",
            Self::ValidationPassed { .. } => "KommunikationsdatenValidationPassed",
            Self::ValidationFailed { .. } => "KommunikationsdatenValidationFailed",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a PARTIN Kommunikationsdaten process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum KommunikationsdatenState {
    /// No events yet.
    #[default]
    New,
    /// PARTIN received; awaiting validation.
    PartinErhalten(KommunikationsdatenData),
    /// Validation passed; data available (terminal).
    ValidationPassed(KommunikationsdatenData),
    /// Validation failed (terminal).
    ValidationFailed {
        /// Validation error reason.
        reason: String,
    },
}

impl KommunikationsdatenState {
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

/// Commands for the PARTIN Kommunikationsdaten workflow.
#[derive(Clone)]
pub enum KommunikationsdatenCommand {
    /// Inbound PARTIN message received.
    ReceivePartin {
        /// BDEW Prüfidentifikator of the inbound PARTIN.
        pid: Pruefidentifikator,
        /// GLN of the sending party.
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

impl CommandPayload for KommunikationsdatenCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// PARTIN Kommunikationsdaten workflow — handles inbound PARTIN for all roles.
///
/// Records inbound PARTIN messages exchanging market participant communication
/// data (AS4 endpoints, GLNs, contact addresses) between LF, NB, MSB, and ÜNB.
pub struct GpkePartinWorkflow;

impl Workflow for GpkePartinWorkflow {
    type State = KommunikationsdatenState;
    type Event = KommunikationsdatenEvent;
    type Command = KommunikationsdatenCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            KommunikationsdatenEvent::PartinErhalten {
                pruefidentifikator,
                sender,
                document_date,
                message_ref,
            } => KommunikationsdatenState::PartinErhalten(KommunikationsdatenData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            }),
            KommunikationsdatenEvent::ValidationPassed { .. } => match state {
                KommunikationsdatenState::PartinErhalten(data) => {
                    KommunikationsdatenState::ValidationPassed(data)
                }
                other => other,
            },
            KommunikationsdatenEvent::ValidationFailed { reason } => {
                KommunikationsdatenState::ValidationFailed {
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
            KommunikationsdatenCommand::ReceivePartin {
                pid,
                sender,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, KommunikationsdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !PARTIN_STROM_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a Strom PARTIN PID (37000–37006); Gas PARTIN (37008–37014) is handled by mako-geli-gas",
                    )));
                }
                let mut events = vec![KommunikationsdatenEvent::PartinErhalten {
                    pruefidentifikator: pid,
                    sender,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(KommunikationsdatenEvent::ValidationPassed { message_ref });
                } else {
                    events.push(KommunikationsdatenEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
        }
    }
}
