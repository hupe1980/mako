//! GPKE Datenabruf — LF requests data values / reclamations from NB or MSB.
//!
//! This module handles the LF-initiated data-request and reclamation processes
//! defined in **GPKE Teil 2 and Teil 4** (BK6-22-024):
//!
//! | PID   | Direction   | Description |
//! |-------|-------------|-------------|
//! | 17102 | LF → NB/MSB | Anfrage von Werten (data value request) |
//! | 17113 | LF → NB/MSB | Reklamation von Werten/Lastgängen |
//! | 19101 | NB → LF     | Ablehnung Anfrage Stammdaten |
//! | 19102 | NB/MSB → LF | Ablehnung Anfrage Werte |
//! | 19114 | NB/MSB → LF | Ablehnung der Reklamation von Werten |
//!
//! Positive responses to data requests arrive via **MSCONS** (handled by
//! [`crate::messwerte`]); only rejections (ORDRSP 19101/19102/19114) are handled here.
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE Teil 2 / Teil 4** — BK6-22-024
//! - APERAK Frist: 24 wall-clock hours (GPKE Teil 2)

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name for the GPKE Datenabruf process.
pub const WORKFLOW_NAME: &str = "gpke-datenabruf";

/// ORDERS PIDs sent by LF to request or reclaim data.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17004 | Anforderung von Werten (LF → NB/MSB) |
/// | 17102 | Anfrage von Werten (LF → NB/MSB) |
/// | 17113 | Reklamation von Werten/Lastgängen (LF → NB/MSB) |
pub const ORDERS_ANFRAGE_PIDS: &[u32] = &[17004, 17102, 17113];

/// ORDRSP rejection PIDs received by LF from NB or MSB.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 19101 | Ablehnung Anfrage Stammdaten (NB → LF) |
/// | 19102 | Ablehnung Anfrage Werte (NB/MSB → LF) |
/// | 19114 | Ablehnung der Reklamation von Werten (NB/MSB → LF) |
pub const ORDRSP_ABLEHNUNG_PIDS: &[u32] = &[19101, 19102, 19114];

/// Deadline label for the data-request response window (24h, GPKE Teil 2).
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-datenabruf-antwort-24h";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Datenabruf workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DatanabrufEvent {
    /// LF sent an ORDERS data request or reclamation.
    AnfrageGesendet {
        /// ORDERS Prüfidentifikator (17102 or 17113).
        orders_pid: Pruefidentifikator,
        /// Recipient GLN (NB or MSB).
        recipient: MarktpartnerCode,
        /// Affected MaLo (if known at request time).
        malo: Option<MaLo>,
        /// Message reference of the sent ORDERS.
        message_ref: MessageRef,
    },
    /// NB or MSB rejected the data request (ORDRSP 19101/19102/19114).
    AnfrageAbgelehnt {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason from the ORDRSP.
        reason: Option<String>,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Data delivered via MSCONS (informational event; actual data in messwerte workflow).
    DatenGeliefert {
        /// Message reference of the triggering MSCONS notification.
        message_ref: MessageRef,
    },
    /// Deadline expired before the response arrived.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for DatanabrufEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "DatanabrufAnfrageGesendet",
            Self::AnfrageAbgelehnt { .. } => "DatanabrufAnfrageAbgelehnt",
            Self::DatenGeliefert { .. } => "DatanabrufDatenGeliefert",
            Self::DeadlineExpired { .. } => "DatanabrufDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Data captured when the ORDERS request is sent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnfrageData {
    /// ORDERS PID sent.
    pub orders_pid: Pruefidentifikator,
    /// Recipient GLN.
    pub recipient: MarktpartnerCode,
    /// Affected MaLo (optional).
    pub malo: Option<MaLo>,
    /// Message reference.
    pub message_ref: MessageRef,
}

/// Current state of a GPKE Datenabruf process stream.
///
/// ```text
/// New → AnfrageGesendet → DatenErhalten
///                      ↘ Abgelehnt
///                      ↘ DeadlineExpired
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum DatanabrufState {
    /// No request sent yet.
    New,
    /// LF sent the ORDERS; awaiting ORDRSP or MSCONS delivery.
    AnfrageGesendet(AnfrageData),
    /// NB/MSB rejected the request (ORDRSP 19101/19102/19114).
    Abgelehnt {
        /// Original request.
        anfrage: AnfrageData,
        /// Rejection ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
    },
    /// Data was delivered (MSCONS arrived; positive outcome).
    DatenErhalten {
        /// Original request.
        anfrage: AnfrageData,
    },
    /// Deadline expired without response.
    DeadlineExpired {
        /// Original request.
        anfrage: AnfrageData,
    },
}

impl Default for DatanabrufState {
    fn default() -> Self {
        Self::New
    }
}

impl DatanabrufState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageGesendet(_) => "AnfrageGesendet",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::DatenErhalten { .. } => "DatenErhalten",
            Self::DeadlineExpired { .. } => "DeadlineExpired",
        }
    }

    /// Whether the process is terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Abgelehnt { .. } | Self::DatenErhalten { .. } | Self::DeadlineExpired { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Datenabruf workflow.
#[derive(Clone)]
pub enum DatanabrufCommand {
    /// LF sends a data-value request or reclamation.
    SendAnfrage {
        /// ORDERS PID (17102 or 17113).
        orders_pid: Pruefidentifikator,
        /// Recipient GLN.
        recipient: MarktpartnerCode,
        /// Affected MaLo (optional).
        malo: Option<MaLo>,
        /// Message reference of the outbound ORDERS.
        message_ref: MessageRef,
        /// ORDERS payload.
        payload: serde_json::Value,
    },
    /// Inbound ORDRSP rejection received.
    ReceiveAblehnung {
        /// ORDRSP Prüfidentifikator (19101/19102/19114).
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// MSCONS data arrived (positive response via messwerte workflow).
    NotifyDatenGeliefert {
        /// Message reference of the MSCONS trigger.
        message_ref: MessageRef,
    },
    /// Deadline expired.
    TimeoutExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for DatanabrufCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Datenabruf workflow — LF-initiated data requests and reclamations.
pub struct GpkeDatanabrufWorkflow;

impl Workflow for GpkeDatanabrufWorkflow {
    type State = DatanabrufState;
    type Event = DatanabrufEvent;
    type Command = DatanabrufCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (ANTWORT_WINDOW_LABEL, DatanabrufState::AnfrageGesendet(_)) => {
                Some(DatanabrufCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            DatanabrufEvent::AnfrageGesendet {
                orders_pid,
                recipient,
                malo,
                message_ref,
            } => DatanabrufState::AnfrageGesendet(AnfrageData {
                orders_pid: *orders_pid,
                recipient: recipient.clone(),
                malo: malo.clone(),
                message_ref: message_ref.clone(),
            }),
            DatanabrufEvent::AnfrageAbgelehnt {
                ordrsp_pid, reason, ..
            } => match state {
                DatanabrufState::AnfrageGesendet(anfrage) => DatanabrufState::Abgelehnt {
                    anfrage,
                    ordrsp_pid: *ordrsp_pid,
                    reason: reason.clone(),
                },
                other => other,
            },
            DatanabrufEvent::DatenGeliefert { .. } => match state {
                DatanabrufState::AnfrageGesendet(anfrage) => {
                    DatanabrufState::DatenErhalten { anfrage }
                }
                other => other,
            },
            DatanabrufEvent::DeadlineExpired { .. } => match state {
                DatanabrufState::AnfrageGesendet(anfrage) => {
                    DatanabrufState::DeadlineExpired { anfrage }
                }
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            DatanabrufCommand::SendAnfrage {
                orders_pid,
                recipient,
                malo,
                message_ref,
                payload,
            } => {
                if !matches!(state, DatanabrufState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_ANFRAGE_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid Datenabruf ORDERS PID: {orders_pid}",
                    )));
                }
                let event = DatanabrufEvent::AnfrageGesendet {
                    orders_pid,
                    recipient: recipient.clone(),
                    malo: malo.clone(),
                    message_ref: message_ref.clone(),
                };
                let outbox = vec![PendingOutbox::new(
                    "ORDERS",
                    recipient.as_str(),
                    serde_json::json!({
                        "pid":        orders_pid.as_u32(),
                        "malo":       malo.as_ref().map(|m| m.as_str()),
                        "orders_ref": message_ref.as_str(),
                        "payload":    payload,
                    }),
                )];
                Ok(WorkflowOutput::with_outbox(vec![event], outbox))
            }

            DatanabrufCommand::ReceiveAblehnung {
                ordrsp_pid,
                reason,
                message_ref,
            } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !matches!(state, DatanabrufState::AnfrageGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageGesendet",
                        state.label(),
                    ));
                }
                if !ORDRSP_ABLEHNUNG_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid Datenabruf ORDRSP rejection PID: {ordrsp_pid}",
                    )));
                }
                Ok(vec![DatanabrufEvent::AnfrageAbgelehnt {
                    ordrsp_pid,
                    reason,
                    message_ref,
                }]
                .into())
            }

            DatanabrufCommand::NotifyDatenGeliefert { message_ref } => {
                if !matches!(state, DatanabrufState::AnfrageGesendet(_)) {
                    return Ok(WorkflowOutput::events(vec![])); // idempotent
                }
                Ok(vec![DatanabrufEvent::DatenGeliefert { message_ref }].into())
            }

            DatanabrufCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![DatanabrufEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}
