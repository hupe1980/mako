//! WiM INSRPT Störungsmeldung workflow — fault/inspection reports (Strom).
//!
//! Handles INSRPT messages for fault and inspection reporting between LF and MSB
//! in the WiM Strom Teil 2 process.
//!
//! ## Prüfidentifikatoren handled
//!
//! | PID   | Description            | Direction  | Domain              |
//! |-------|------------------------|------------|---------------------|
//! | 23001 | Störungsmeldung        | LF → MSB   | WiM Strom Teil 2    |
//! | 23003 | Ablehnung              | MSB → LF   | WiM Strom Teil 2    |
//! | 23004 | Bestätigung            | MSB → LF   | WiM Strom Teil 2    |
//! | 23008 | Ergebnisbericht        | MSB → LF   | WiM Strom Teil 2    |
//! | 23011 | Informationsmeldung    | MSB → LF   | WiM Strom Teil 2    |
//! | 23012 | Informationsmeldung    | MSB → LF   | WiM Strom Teil 2    |
//!
//! Gas-only INSRPT PIDs 23005 and 23009 are handled by `mako-wim-gas`
//! (`wim-gas-insrpt`) with the Gas APERAK Frist of **10 Werktage**.
//!
//! ## Regulatory basis
//!
//! - **BK6-24-174** — WiM Strom (APERAK Frist 5 Werktage)
//! - **INSRPT 1.x** — EDI@Energy inspection report format

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the INSRPT Störungsmeldung workflow.
pub const WORKFLOW_NAME: &str = "wim-insrpt";

/// PIDs for outbound INSRPT from LF to MSB (LF initiates Störungsmeldung).
///
/// - 23001: Störungsmeldung (LF → MSB)
pub const INSRPT_ANFRAGE_PIDS: &[u32] = &[23001];

/// PIDs for inbound INSRPT responses from MSB to LF (Strom + shared).
///
/// - 23003: Ablehnung (MSB → LF) — WiM Strom and Gas
/// - 23004: Bestätigung (MSB → LF) — WiM Strom and Gas
/// - 23008: Ergebnisbericht (MSB → LF) — WiM Strom and Gas
/// - 23011: Informationsmeldung (MSB → LF) — WiM Strom Teil 2 only
/// - 23012: Informationsmeldung (MSB → LF) — WiM Strom Teil 2 only
///
/// Gas-only PIDs 23005 (Ablehnung Gas) and 23009 (Ergebnisbericht Gas) are
/// registered by `mako-wim-gas` (`wim-gas-insrpt`) with the Gas APERAK Frist
/// of 10 Werktage. In a combined Strom+Gas deployment `mako-wim` registers
/// the shared PIDs here (5 WT); `mako-wim-gas` adds 23005/23009 (10 WT).
pub const INSRPT_ANTWORT_PIDS: &[u32] = &[23003, 23004, 23008, 23011, 23012];

/// Deadline label for the MSB response window.
///
/// The MSB must respond within **5 Werktage** per BK6-24-174 (WiM Strom) /
/// **10 Werktage** per BK7-24-01-009 (WiM Gas).
pub const ANTWORT_WINDOW_LABEL: &str = "wim-insrpt-antwort";

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a Störungsmeldung is initiated.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorungsmeldungData {
    /// BDEW Prüfidentifikator of the INSRPT.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the MSB the Störungsmeldung was sent to.
    pub msb_mp_id: MarktpartnerCode,
    /// EDIFACT document date.
    pub document_date: String,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the INSRPT Störungsmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StorungsmeldungEvent {
    /// LF sent a Störungsmeldung (INSRPT 23001) to MSB.
    StorungsmeldungGesendet {
        /// Prüfidentifikator (always 23001).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the receiving MSB.
        msb_mp_id: MarktpartnerCode,
        /// Document date.
        document_date: String,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Inbound INSRPT response received from MSB.
    AntwortErhalten {
        /// Response PID (23003/23004/23005/23008/23009/23011/23012).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the responding MSB.
        sender: MarktpartnerCode,
        /// `true` for Bestätigung (23004), `false` for Ablehnung (23003/23005).
        is_confirmation: bool,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// MSB sent an informational INSRPT (23011/23012, no explicit confirmation).
    InformationsmeldungErhalten {
        /// PID (23011 or 23012).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the sending MSB.
        sender: MarktpartnerCode,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Deadline expired before MSB responded.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for StorungsmeldungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::StorungsmeldungGesendet { .. } => "InsrptStorungsmeldungGesendet",
            Self::AntwortErhalten { .. } => "InsrptAntwortErhalten",
            Self::InformationsmeldungErhalten { .. } => "InsrptInformationsmeldungErhalten",
            Self::DeadlineExpired { .. } => "InsrptDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of an INSRPT Störungsmeldung process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum StorungsmeldungState {
    /// No events yet.
    New,
    /// Störungsmeldung sent; awaiting MSB response.
    StorungsmeldungGesendet(StorungsmeldungData),
    /// MSB confirmed (terminal success).
    Bestaetigt(StorungsmeldungData),
    /// MSB rejected (terminal failure).
    Abgelehnt(StorungsmeldungData),
    /// MSB sent Ergebnisbericht (terminal: process completed with findings).
    Ergebnisbericht(StorungsmeldungData),
    /// Deadline expired before MSB responded (terminal).
    DeadlineExpired {
        /// Label of the expired deadline.
        label: String,
    },
}

impl Default for StorungsmeldungState {
    fn default() -> Self {
        Self::New
    }
}

impl StorungsmeldungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::StorungsmeldungGesendet(_) => "StorungsmeldungGesendet",
            Self::Bestaetigt(_) => "Bestaetigt",
            Self::Abgelehnt(_) => "Abgelehnt",
            Self::Ergebnisbericht(_) => "Ergebnisbericht",
            Self::DeadlineExpired { .. } => "DeadlineExpired",
        }
    }

    /// Returns `true` if this is a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Bestaetigt(_)
                | Self::Abgelehnt(_)
                | Self::Ergebnisbericht(_)
                | Self::DeadlineExpired { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the INSRPT Störungsmeldung workflow.
#[derive(Clone)]
pub enum StorungsmeldungCommand {
    /// ERP instructs LF to send a Störungsmeldung to MSB.
    SendStorungsmeldung {
        /// Prüfidentifikator (always 23001).
        pid: Pruefidentifikator,
        /// GLN of the receiving MSB.
        msb_mp_id: MarktpartnerCode,
        /// Document date.
        document_date: String,
        /// Message reference of the outbound INSRPT.
        message_ref: MessageRef,
    },
    /// Inbound INSRPT response received from MSB (23003/23004/23005/23008/23009).
    ReceiveAntwort {
        /// Response PID.
        pid: Pruefidentifikator,
        /// GLN of the MSB.
        sender: MarktpartnerCode,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Inbound informational INSRPT received from MSB (23011/23012).
    ReceiveInformationsmeldung {
        /// PID (23011 or 23012).
        pid: Pruefidentifikator,
        /// GLN of the sending MSB.
        sender: MarktpartnerCode,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Deadline expired.
    TimeoutExpired {
        /// Unique ID.
        deadline_id: DeadlineId,
        /// Label.
        label: Box<str>,
    },
}

impl CommandPayload for StorungsmeldungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// INSRPT Störungsmeldung workflow — LF/NB sends fault report, MSB responds.
pub struct WimInsrptWorkflow;

impl Workflow for WimInsrptWorkflow {
    type State = StorungsmeldungState;
    type Event = StorungsmeldungEvent;
    type Command = StorungsmeldungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        if deadline.label() == ANTWORT_WINDOW_LABEL && !state.is_terminal() {
            return Some(StorungsmeldungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            });
        }
        None
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            StorungsmeldungEvent::StorungsmeldungGesendet {
                pruefidentifikator,
                msb_mp_id,
                document_date,
                message_ref,
            } => StorungsmeldungState::StorungsmeldungGesendet(StorungsmeldungData {
                pruefidentifikator: *pruefidentifikator,
                msb_mp_id: msb_mp_id.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            }),
            StorungsmeldungEvent::AntwortErhalten {
                pruefidentifikator,
                is_confirmation,
                ..
            } => match state {
                StorungsmeldungState::StorungsmeldungGesendet(data) => {
                    // 23008/23009 = Ergebnisbericht; 23004 = Bestätigung; 23003/23005 = Ablehnung
                    match pruefidentifikator.as_u32() {
                        23008 | 23009 => StorungsmeldungState::Ergebnisbericht(data),
                        _ if *is_confirmation => StorungsmeldungState::Bestaetigt(data),
                        _ => StorungsmeldungState::Abgelehnt(data),
                    }
                }
                other => other,
            },
            StorungsmeldungEvent::InformationsmeldungErhalten { .. } => {
                // Informational; no state change (already terminal or out of sequence).
                state
            }
            StorungsmeldungEvent::DeadlineExpired { label, .. } => {
                if state.is_terminal() {
                    state
                } else {
                    StorungsmeldungState::DeadlineExpired {
                        label: label.to_string(),
                    }
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            StorungsmeldungCommand::SendStorungsmeldung {
                pid,
                msb_mp_id,
                document_date,
                message_ref,
            } => {
                if !matches!(state, StorungsmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !INSRPT_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected INSRPT PID 23001 for Störungsmeldung, got {pid}",
                    )));
                }
                let outbox = PendingOutbox::new(
                    "INSRPT",
                    msb_mp_id.as_str(),
                    serde_json::json!({
                        "type":         "Stoerungsmeldung",
                        "pid":          pid.as_u32(),
                        "message_ref":  message_ref.as_str(),
                    }),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![StorungsmeldungEvent::StorungsmeldungGesendet {
                        pruefidentifikator: pid,
                        msb_mp_id,
                        document_date,
                        message_ref,
                    }],
                    vec![outbox],
                ))
            }

            StorungsmeldungCommand::ReceiveAntwort {
                pid,
                sender,
                message_ref,
            } => {
                if !INSRPT_ANTWORT_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a handled INSRPT response PID",
                    )));
                }
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                let is_confirmation = matches!(pid.as_u32(), 23004 | 23008);
                Ok(vec![StorungsmeldungEvent::AntwortErhalten {
                    pruefidentifikator: pid,
                    sender,
                    is_confirmation,
                    message_ref,
                }]
                .into())
            }

            StorungsmeldungCommand::ReceiveInformationsmeldung {
                pid,
                sender,
                message_ref,
            } => Ok(vec![StorungsmeldungEvent::InformationsmeldungErhalten {
                pruefidentifikator: pid,
                sender,
                message_ref,
            }]
            .into()),

            StorungsmeldungCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![StorungsmeldungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}
