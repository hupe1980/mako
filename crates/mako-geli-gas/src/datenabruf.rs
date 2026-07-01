//! GeLi Gas / WiM Gas Datenabruf — Gas-specific data requests.
//!
//! Handles inbound ORDERS messages requesting Gas-specific metered values:
//! - Abrechnungsbrennwert and Zustandszahl (PID 17103, Gas billing values)
//! - MSB Gas Anfrage an NB Strom (PID 17104, cross-domain MSB Gas → NB Strom)
//!
//! Corresponding rejection responses: ORDRSP 19103/19104.
//!
//! ## Regulatory basis
//!
//! - **BK7-24-01-009** (GeLi Gas 3.0) — Gas metered value requests
//! - **BDEW GeLi Gas AHB** — ORDERS/ORDRSP for Gas Datenabruf

use mako_engine::{
    error::WorkflowError,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the GeLi Gas Datenabruf workflow.
pub const WORKFLOW_NAME: &str = "geli-gas-datenabruf";

/// ORDERS PIDs for Gas-specific data requests (LF/MSB → NB).
///
/// | PID   | Description                                               | Direction      |
/// |-------|-----------------------------------------------------------|----------------|
/// | 17103 | Anfrage zur Übermittlung von Abrechnungsbrennwert/Zahl    | LF → NB/MSB   |
/// | 17104 | Anfrage des MSB Gas an den NB Strom                       | MSB Gas → NB  |
pub const ORDERS_ANFRAGE_PIDS: &[u32] = &[17103, 17104];

/// ORDRSP rejection PIDs for Gas Datenabruf (NB → LF/MSB).
///
/// | PID   | Description                                    |
/// |-------|------------------------------------------------|
/// | 19103 | Ablehnung Brennwert und Zustandszahl           |
/// | 19104 | Ablehnung Anfrage vom MSB Gas                  |
pub const ORDRSP_ABLEHNUNG_PIDS: &[u32] = &[19103, 19104];

/// Deadline label for the Gas data-request response window (10 Werktage, GeLi Gas).
pub const ANTWORT_WINDOW_LABEL: &str = "geli-gas-datenabruf-antwort";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas Datenabruf workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeliGasDatanabrufEvent {
    /// ORDERS data request received.
    AnfrageErhalten {
        /// BDEW Prüfidentifikator (17103 or 17104).
        pid: Pruefidentifikator,
        /// Sender (LF or MSB Gas).
        sender: MarktpartnerCode,
        /// Receiver (NB or MSB Strom).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// ORDRSP rejection received (NB → LF/MSB).
    AbgelehntErhalten {
        /// BDEW Prüfidentifikator (19103 or 19104).
        pid: Pruefidentifikator,
        /// Sender (NB or MSB Strom).
        sender: MarktpartnerCode,
        /// Message reference of the rejection.
        message_ref: MessageRef,
    },
    /// MSCONS data delivery confirmed — marks the Datenabruf process as complete.
    ///
    /// Emitted by `NotifyDatenGeliefert` when the MSCONS carrier arrives and the
    /// adapter signals that the requested data has been delivered. Transitions the
    /// process from `AnfrageGesendet` to the terminal `DatenErhalten` state.
    DatenGeliefert,
    /// Deadline expired without a response.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: mako_engine::ids::DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for GeliGasDatanabrufEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageErhalten { .. } => "GeliGasDatanabrufAnfrageErhalten",
            Self::AbgelehntErhalten { .. } => "GeliGasDatanabrufAbgelehnt",
            Self::DatenGeliefert => "GeliGasDatanabrufDatenGeliefert",
            Self::DeadlineExpired { .. } => "GeliGasDatanabrufDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// State of a GeLi Gas Datenabruf process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GeliGasDatanabrufState {
    /// No events yet.
    New,
    /// ORDERS received; waiting for NB response within 10 Werktage.
    AnfrageGesendet {
        /// PID of the initial ORDERS.
        pid: Pruefidentifikator,
        /// Sender of the ORDERS.
        sender: MarktpartnerCode,
    },
    /// Positive data delivery received (via MSCONS, no ORDRSP).
    DatenErhalten,
    /// NB/MSB rejected the request (ORDRSP 19103/19104).
    Abgelehnt,
    /// Deadline expired without a response.
    DeadlineExpired,
}

impl Default for GeliGasDatanabrufState {
    fn default() -> Self {
        Self::New
    }
}

impl GeliGasDatanabrufState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageGesendet { .. } => "AnfrageGesendet",
            Self::DatenErhalten => "DatenErhalten",
            Self::Abgelehnt => "Abgelehnt",
            Self::DeadlineExpired => "DeadlineExpired",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas Datenabruf workflow.
#[derive(Clone)]
pub enum GeliGasDatanabrufCommand {
    /// Inbound ORDERS data request received.
    ReceiveAnfrage {
        /// BDEW Prüfidentifikator (17103 or 17104).
        pid: Pruefidentifikator,
        /// Sender (LF or MSB Gas).
        sender: MarktpartnerCode,
        /// Receiver (NB or MSB Strom).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP rejection received.
    ReceiveAblehnung {
        /// Rejection PID (19103 or 19104).
        pid: Pruefidentifikator,
        /// Sender of the rejection.
        sender: MarktpartnerCode,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Data successfully delivered via MSCONS — marks process as complete.
    NotifyDatenGeliefert,
    /// Deadline expired.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: mako_engine::ids::DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for GeliGasDatanabrufCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas Datenabruf workflow — handles Gas-specific data requests.
pub struct GeliGasDatanabrufWorkflow;

impl Workflow for GeliGasDatanabrufWorkflow {
    type State = GeliGasDatanabrufState;
    type Event = GeliGasDatanabrufEvent;
    type Command = GeliGasDatanabrufCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GeliGasDatanabrufEvent::AnfrageErhalten { pid, sender, .. } => {
                if matches!(state, GeliGasDatanabrufState::New) {
                    GeliGasDatanabrufState::AnfrageGesendet {
                        pid: *pid,
                        sender: sender.clone(),
                    }
                } else {
                    state
                }
            }
            GeliGasDatanabrufEvent::AbgelehntErhalten { .. } => GeliGasDatanabrufState::Abgelehnt,
            GeliGasDatanabrufEvent::DatenGeliefert => GeliGasDatanabrufState::DatenErhalten,
            GeliGasDatanabrufEvent::DeadlineExpired { .. } => {
                if matches!(
                    state,
                    GeliGasDatanabrufState::Abgelehnt | GeliGasDatanabrufState::DatenErhalten
                ) {
                    state
                } else {
                    GeliGasDatanabrufState::DeadlineExpired
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        cmd: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match cmd {
            GeliGasDatanabrufCommand::ReceiveAnfrage {
                pid,
                sender,
                receiver,
                message_ref,
            } => {
                if !ORDERS_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas Datenabruf ORDERS PID ({ORDERS_ANFRAGE_PIDS:?}), got {pid}",
                    )));
                }
                if !matches!(state, GeliGasDatanabrufState::New) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GeliGasDatanabrufEvent::AnfrageErhalten {
                    pid,
                    sender,
                    receiver,
                    message_ref,
                }]
                .into())
            }
            GeliGasDatanabrufCommand::ReceiveAblehnung {
                pid,
                sender,
                message_ref,
            } => {
                if !ORDRSP_ABLEHNUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas Datenabruf ORDRSP rejection PID ({ORDRSP_ABLEHNUNG_PIDS:?}), got {pid}",
                    )));
                }
                if !matches!(state, GeliGasDatanabrufState::AnfrageGesendet { .. }) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GeliGasDatanabrufEvent::AbgelehntErhalten {
                    pid,
                    sender,
                    message_ref,
                }]
                .into())
            }
            GeliGasDatanabrufCommand::NotifyDatenGeliefert => {
                // Data was delivered via MSCONS — transition to DatenErhalten.
                if matches!(state, GeliGasDatanabrufState::AnfrageGesendet { .. }) {
                    Ok(vec![GeliGasDatanabrufEvent::DatenGeliefert].into())
                } else {
                    // Already resolved (Abgelehnt, DatenErhalten, DeadlineExpired) — no-op.
                    Ok(WorkflowOutput::events(vec![]))
                }
            }
            GeliGasDatanabrufCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    GeliGasDatanabrufState::Abgelehnt | GeliGasDatanabrufState::DatenErhalten
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GeliGasDatanabrufEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}
