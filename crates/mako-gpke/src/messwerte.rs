//! GPKE / WiM MSCONS data delivery workflow — inbound metered data (NB/MSB → LF).
//!
//! Handles inbound MSCONS messages containing metered energy data, load profiles,
//! meter readings, and time-series delivered by the Netzbetreiber (NB) or
//! Messstellenbetreiber (MSB) to the Lieferant (LF).
//!
//! MSCONS is a data-delivery message format used throughout the German energy market
//! for transmitting metered values. When `makod` operates as a **Lieferant (LF)**,
//! inbound MSCONS messages from NB/MSB must be received, validated, and acknowledged.
//!
//! ## Prüfidentifikatoren handled
//!
//! | PID   | Description                                         | Direction     | Domain          |
//! |-------|-----------------------------------------------------|---------------|-----------------|
//! | 13005 | EEG-Überführungszeitreihe                           | NB → LF       | GPKE Teil 2     |
//! | 13006 | Stornierung von Messwerten                          | NB/MSB → LF   | GPKE Teil 2     |
//! | 13015 | Arbeit Leistungsmax. Kalenderj. vor Lieferbeginn    | NB → LF       | GPKE Teil 2     |
//! | 13016 | Energiemenge u. Leistungsmax. (Strom)               | NB/MSB → LF   | GPKE Teil 2/4   |
//! | 13017 | Zählerstand (Strom)                                 | MSB → LF      | GPKE Teil 4     |
//! | 13018 | Lastgang Messlokation, Netzkoppelpunkt, Netzlokation| MSB → LF      | GPKE Teil 4     |
//! | 13019 | Energiemenge (Strom)                                | NB/MSB → LF   | GPKE Teil 2/4   |
//! | 13025 | Lastgang Marktlokation, Tranche                     | MSB → LF      | GPKE Teil 4     |
//! | 13027 | Werte nach Typ 2                                    | MSB → LF      | WiM Strom Teil 2|
//!
//! ## Regulatory basis
//!
//! - **BDEW GPKE Teil 2** — Energiedaten nach Lieferbeginn (BK6-22-024)
//! - **BDEW GPKE Teil 4** — Stamm- und Bewegungsdaten Strom
//! - **MSCONS S1.x** — EDI@Energy metered data format
//!
//! ## Note on CONTRL acknowledgement
//!
//! CONTRL (functional acknowledgement) is sent by the transport layer automatically.
//! The application workflow records receipt and makes the data available for
//! downstream billing and reconciliation. No explicit application-level APERAK is
//! required for MSCONS in GPKE Teil 2/4.

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the MSCONS data delivery workflow.
pub const WORKFLOW_NAME: &str = "gpke-messwerte";

/// MSCONS Prüfidentifikatoren for GPKE/WiM data delivery to LF.
///
/// These PIDs represent energy data, load profiles, and meter readings
/// that the NB or MSB delivers to the LF (inbound from LF's perspective).
///
/// | PID   | Description                                      | Source  |
/// |-------|--------------------------------------------------|---------|
/// | 13015 | Arbeit Leistungsmax. vor Lieferbeginn            | NB      |
/// | 13016 | Energiemenge u. Leistungsmax. Strom              | NB/MSB  |
/// | 13017 | Zählerstand (Strom)                              | MSB     |
/// | 13018 | Lastgang Messlokation                            | MSB     |
/// | 13019 | Energiemenge (Strom)                             | NB/MSB  |
/// | 13025 | Lastgang Marktlokation, Tranche                  | MSB     |
/// | 13027 | Werte nach Typ 2                                 | MSB     |
/// | 13005 | EEG-Überführungszeitreihe                        | NB      |
/// | 13006 | Stornierung von Messwerten                       | NB/MSB  |
pub const MSCONS_PIDS: &[u32] = &[
    13005, 13006, 13015, 13016, 13017, 13018, 13019, 13025, 13027,
];

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when an MSCONS message is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesswerteLieferungData {
    /// BDEW Prüfidentifikator of the inbound MSCONS.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the NB or MSB sender.
    pub sender: MarktpartnerCode,
    /// EIC/MaLo of the supply location for which data was delivered.
    pub location_id: MaLo,
    /// EDIFACT document date (YYYYMMDD).
    pub document_date: String,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the MSCONS data delivery workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MesswerteLieferungEvent {
    /// Inbound MSCONS message received; AHB validation not yet performed.
    MsconsDatenErhalten {
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the sending NB or MSB.
        sender: MarktpartnerCode,
        /// EIC/MaLo of the supply location.
        location_id: MaLo,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// AHB validation passed; data is available for downstream use.
    ValidationPassed {
        /// Message reference of the validated MSCONS.
        message_ref: MessageRef,
    },
    /// AHB validation failed; data is unusable.
    ValidationFailed {
        /// Human-readable validation error summary.
        reason: String,
    },
}

impl EventPayload for MesswerteLieferungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::MsconsDatenErhalten { .. } => "MesswerteLieferungErhalten",
            Self::ValidationPassed { .. } => "MesswerteLieferungValidationPassed",
            Self::ValidationFailed { .. } => "MesswerteLieferungValidationFailed",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of an MSCONS data delivery process stream.
///
/// # Lifecycle
///
/// ```text
/// New → DatenErhalten → ValidationPassed (terminal)
///                     → ValidationFailed (terminal)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum MesswerteLieferungState {
    /// No events yet.
    New,
    /// MSCONS received; awaiting validation result.
    DatenErhalten(MesswerteLieferungData),
    /// Validation passed; data available (terminal success).
    ValidationPassed(MesswerteLieferungData),
    /// Validation failed (terminal failure).
    ValidationFailed {
        /// Validation error reason.
        reason: String,
    },
}

impl Default for MesswerteLieferungState {
    fn default() -> Self {
        Self::New
    }
}

impl MesswerteLieferungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::DatenErhalten(_) => "DatenErhalten",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the MSCONS data delivery workflow.
#[derive(Clone)]
pub enum MesswerteLieferungCommand {
    /// Inbound MSCONS received from NB or MSB.
    ReceiveMscons {
        /// BDEW Prüfidentifikator of the inbound MSCONS.
        pid: Pruefidentifikator,
        /// GLN of the NB or MSB sender.
        sender: MarktpartnerCode,
        /// EIC/MaLo of the delivery location.
        location_id: MaLo,
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

impl CommandPayload for MesswerteLieferungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// MSCONS data delivery workflow — handles inbound MSCONS for LF/NB role.
///
/// Records inbound MSCONS metered-data messages from NB/MSB to LF, validates
/// them against the appropriate AHB profile, and makes the data available for
/// downstream billing and settlement reconciliation.
pub struct GpkeMesswerteLieferungWorkflow;

impl Workflow for GpkeMesswerteLieferungWorkflow {
    type State = MesswerteLieferungState;
    type Event = MesswerteLieferungEvent;
    type Command = MesswerteLieferungCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            MesswerteLieferungEvent::MsconsDatenErhalten {
                pruefidentifikator,
                sender,
                location_id,
                document_date,
                message_ref,
            } => MesswerteLieferungState::DatenErhalten(MesswerteLieferungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                location_id: location_id.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            }),
            MesswerteLieferungEvent::ValidationPassed { .. } => match state {
                MesswerteLieferungState::DatenErhalten(data) => {
                    MesswerteLieferungState::ValidationPassed(data)
                }
                other => other,
            },
            MesswerteLieferungEvent::ValidationFailed { reason } => {
                MesswerteLieferungState::ValidationFailed {
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
            MesswerteLieferungCommand::ReceiveMscons {
                pid,
                sender,
                location_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, MesswerteLieferungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !MSCONS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a handled MSCONS PID",
                    )));
                }
                let mut events = vec![MesswerteLieferungEvent::MsconsDatenErhalten {
                    pruefidentifikator: pid,
                    sender,
                    location_id,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(MesswerteLieferungEvent::ValidationPassed { message_ref });
                } else {
                    events.push(MesswerteLieferungEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
        }
    }
}
