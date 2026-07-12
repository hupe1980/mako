//! WiM Strom/Gas Technikänderung — device/measurement-point change requests.
//!
//! This module handles ORDERS-based requests for technical changes to
//! measurement devices, configuration, and data delivery, including
//! LF→MSB, NB→MSB, and MSB→MSB processes defined in **WiM Strom Teil 1**
//! and **WiM Gas AWH**:
//!
//! | PID   | Direction  | Description |
//! |-------|------------|-------------|
//! | 17003 | LF → MSB   | Beauftragung zur Änderung der Technik (Gas) |
//! | 17007 | LF/NB → NB | Bestellung und Abbestellung von Werten ESA |
//! | 17008 | LF → NB    | Abbestellung von Werten |
//! | 17118 | MSB → MSB  | Bestellung einer Konfigurationsänderung |
//! | 17121 | NB → MSB   | Bestellung Änderung (NB an MSB, GPKE Teil 3) |
//! | 19003 | NB/MSB → LF| Fortführungsbestätigung |
//! | 19004 | NB/MSB → LF| Ablehnung Fortführung |
//! | 19005 | MSB → LF   | Auftragsbestätigung der Änderung der Technik |
//! | 19006 | MSB → LF   | Ablehnung der Änderung der Technik |
//! | 19007 | MSB → LF   | Ablehnung Anforderung Messwerte |
//! | 19011 | NB/MSB → ? | Bestätigung der Ab-/Bestellung von Werten für ESA |
//! | 19012 | NB/MSB → ? | Ablehnung der Ab-/Bestellung von Werten für ESA |
//!
//! # Regulatory basis
//!
//! - **BK6-24-174** — WiM Strom Teil 1 (Messstellenbetrieb)
//! - **BK7-24-01-009** — WiM Gas AWH V2.0
//! - APERAK Frist: **5 Werktage** (WiM Strom), **10 Werktage** (WiM Gas)

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name for the WiM Technikänderung process.
pub const WORKFLOW_NAME: &str = "wim-technik-aenderung";

/// ORDERS PIDs for technical change requests (all directions).
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17003 | Beauftragung Änderung Technik Gas (LF → MSB) |
/// | 17007 | Bestellung und Abbestellung von Werten ESA |
/// | 17008 | Abbestellung von Werten |
/// | 17118 | Bestellung Konfigurationsänderung (MSB → MSB) |
pub const ORDERS_PIDS: &[u32] = &[17003, 17007, 17008, 17118];

/// ORDRSP PIDs received in response to technical change requests.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 19003 | Fortführungsbestätigung |
/// | 19004 | Ablehnung Fortführung |
/// | 19005 | Auftragsbestätigung der Änderung der Technik |
/// | 19006 | Ablehnung der Änderung der Technik |
/// | 19007 | Ablehnung Anforderung Messwerte |
/// | 19011 | Bestätigung der Ab-/Bestellung von Werten für ESA |
/// | 19012 | Ablehnung der Ab-/Bestellung von Werten für ESA |
pub const ORDRSP_PIDS: &[u32] = &[19003, 19004, 19005, 19006, 19007, 19011, 19012];

/// Positive ORDRSP PIDs (confirmation).
const ORDRSP_BESTAETIGUNG_PIDS: &[u32] = &[19003, 19005, 19011];

/// Deadline label for the response window (5 Werktage, WiM Strom).
pub const ANTWORT_WINDOW_LABEL: &str = "wim-technik-aenderung-antwort";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Technikänderung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TechnikAenderungEvent {
    /// ORDERS technical change request sent.
    AuftragGesendet {
        /// ORDERS Prüfidentifikator.
        orders_pid: Pruefidentifikator,
        /// Recipient GLN (MSB or NB).
        recipient: MarktpartnerCode,
        /// Affected MeLo/MaLo.
        location_id: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// ORDRSP received — confirmation.
    AuftragBestaetigt {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// ORDRSP received — rejection.
    AuftragAbgelehnt {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Deadline expired before ORDRSP arrived.
    DeadlineExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for TechnikAenderungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AuftragGesendet { .. } => "TechnikAenderungAuftragGesendet",
            Self::AuftragBestaetigt { .. } => "TechnikAenderungAuftragBestaetigt",
            Self::AuftragAbgelehnt { .. } => "TechnikAenderungAuftragAbgelehnt",
            Self::DeadlineExpired { .. } => "TechnikAenderungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Request data.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuftragData {
    /// ORDERS PID.
    pub orders_pid: Pruefidentifikator,
    /// Recipient GLN.
    pub recipient: MarktpartnerCode,
    /// Location identifier.
    pub location_id: Option<String>,
    /// Message reference.
    pub message_ref: MessageRef,
}

/// Current state of a WiM Technikänderung process.
///
/// ```text
/// New → AuftragGesendet → Bestaetigt
///                      ↘ Abgelehnt
///                      ↘ DeadlineExpired
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum TechnikAenderungState {
    /// No ORDERS sent yet.
    #[default]
    New,
    /// ORDERS sent; awaiting ORDRSP.
    AuftragGesendet(AuftragData),
    /// MSB/NB confirmed the change.
    Bestaetigt {
        /// Original request data.
        auftrag: AuftragData,
        /// Confirmation ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
    },
    /// MSB/NB rejected the change.
    Abgelehnt {
        /// Original request data.
        auftrag: AuftragData,
        /// Rejection ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
    },
    /// Deadline expired.
    DeadlineExpired {
        /// Original request data.
        auftrag: AuftragData,
    },
}

impl TechnikAenderungState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AuftragGesendet(_) => "AuftragGesendet",
            Self::Bestaetigt { .. } => "Bestaetigt",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::DeadlineExpired { .. } => "DeadlineExpired",
        }
    }

    /// Whether terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Bestaetigt { .. } | Self::Abgelehnt { .. } | Self::DeadlineExpired { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Technikänderung workflow.
#[derive(Clone)]
pub enum TechnikAenderungCommand {
    /// Send an ORDERS technical change request.
    SendAuftrag {
        /// ORDERS PID.
        orders_pid: Pruefidentifikator,
        /// Recipient GLN.
        recipient: MarktpartnerCode,
        /// Location ID (optional).
        location_id: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
        /// ORDERS body payload.
        payload: serde_json::Value,
    },
    /// Inbound ORDRSP received.
    ReceiveOrdrsp {
        /// ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason (for negative responses).
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Deadline fired.
    TimeoutExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Label.
        label: Box<str>,
    },
}

impl CommandPayload for TechnikAenderungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Technikänderung workflow — device/config change requests.
pub struct WimTechnikAenderungWorkflow;

impl Workflow for WimTechnikAenderungWorkflow {
    type State = TechnikAenderungState;
    type Event = TechnikAenderungEvent;
    type Command = TechnikAenderungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (ANTWORT_WINDOW_LABEL, TechnikAenderungState::AuftragGesendet(_)) => {
                Some(TechnikAenderungCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            TechnikAenderungEvent::AuftragGesendet {
                orders_pid,
                recipient,
                location_id,
                message_ref,
            } => TechnikAenderungState::AuftragGesendet(AuftragData {
                orders_pid: *orders_pid,
                recipient: recipient.clone(),
                location_id: location_id.clone(),
                message_ref: message_ref.clone(),
            }),
            TechnikAenderungEvent::AuftragBestaetigt { ordrsp_pid, .. } => match state {
                TechnikAenderungState::AuftragGesendet(auftrag) => {
                    TechnikAenderungState::Bestaetigt {
                        auftrag,
                        ordrsp_pid: *ordrsp_pid,
                    }
                }
                other => other,
            },
            TechnikAenderungEvent::AuftragAbgelehnt {
                ordrsp_pid, reason, ..
            } => match state {
                TechnikAenderungState::AuftragGesendet(auftrag) => {
                    TechnikAenderungState::Abgelehnt {
                        auftrag,
                        ordrsp_pid: *ordrsp_pid,
                        reason: reason.clone(),
                    }
                }
                other => other,
            },
            TechnikAenderungEvent::DeadlineExpired { .. } => match state {
                TechnikAenderungState::AuftragGesendet(auftrag) => {
                    TechnikAenderungState::DeadlineExpired { auftrag }
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
            TechnikAenderungCommand::SendAuftrag {
                orders_pid,
                recipient,
                location_id,
                message_ref,
                payload,
            } => {
                if !matches!(state, TechnikAenderungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid WiM Technikänderung ORDERS PID: {orders_pid}",
                    )));
                }
                let event = TechnikAenderungEvent::AuftragGesendet {
                    orders_pid,
                    recipient: recipient.clone(),
                    location_id: location_id.clone(),
                    message_ref: message_ref.clone(),
                };
                let outbox = vec![PendingOutbox::new(
                    "ORDERS",
                    recipient.as_str(),
                    serde_json::json!({
                        "pid":        orders_pid.as_u32(),
                        "location":   location_id,
                        "orders_ref": message_ref.as_str(),
                        "payload":    payload,
                    }),
                )];
                Ok(WorkflowOutput::with_outbox(vec![event], outbox))
            }

            TechnikAenderungCommand::ReceiveOrdrsp {
                ordrsp_pid,
                reason,
                message_ref,
            } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !matches!(state, TechnikAenderungState::AuftragGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AuftragGesendet",
                        state.label(),
                    ));
                }
                if !ORDRSP_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid WiM Technikänderung ORDRSP PID: {ordrsp_pid}",
                    )));
                }
                let event = if ORDRSP_BESTAETIGUNG_PIDS.contains(&ordrsp_pid.as_u32()) {
                    TechnikAenderungEvent::AuftragBestaetigt {
                        ordrsp_pid,
                        message_ref,
                    }
                } else {
                    TechnikAenderungEvent::AuftragAbgelehnt {
                        ordrsp_pid,
                        reason,
                        message_ref,
                    }
                };
                Ok(vec![event].into())
            }

            TechnikAenderungCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![TechnikAenderungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}
