//! GPKE LF-side Sperrung / Entsperrung workflow (ORDRSP 19116–19119, IFTSTA 21039).
//!
//! When `makod` operates as a **Lieferant (LF)**, the ERP instructs the engine
//! to issue a Sperrauftrag (ORDERS 17115) or Entsperrauftrag (ORDERS 17117) to
//! the Netzbetreiber (NB). This workflow tracks the outbound Sperrauftrag and
//! handles all inbound responses from the NB and MSB.
//!
//! ## Process flow
//!
//! ```text
//! ERP → InitiateSperrung command
//!        ↓ emits SperrungAuftragInitiiert + ORDERS 17115/17117 outbox entry
//! AS4 sender → ORDERS 17115/17117 to NB
//!        ↓
//! AS4 inbound ← ORDRSP 19116 (Bestätigung) or 19117 (Ablehnung) from NB
//!        ↓
//! [on confirmation] NB executes Sperrung
//!        ↓
//! AS4 inbound ← IFTSTA 21039 (Auftragsstatus Sperren) from NB
//! ```
//!
//! For **Stornierung** (cancellation before execution):
//!
//! ```text
//! ERP → SendStornierung command
//!        ↓ emits StornierungGesendet + ORDCHG 39000 outbox entry
//! AS4 sender → ORDCHG 39000 to NB
//!        ↓
//! AS4 inbound ← ORDRSP 19128 (Bestätigung) or 19129 (Ablehnung) from NB
//! ```
//!
//! ## Prüfidentifikatoren
//!
//! | Direction   | PID   | Description                                    |
//! |-------------|-------|------------------------------------------------|
//! | Outbound LF | 17115 | Sperrauftrag (LF → NB)                        |
//! | Outbound LF | 17117 | Entsperrauftrag (LF → NB)                     |
//! | Inbound NB  | 19116 | Bestätigung Sperr-/Entsperrauftrag (NB → LF)  |
//! | Inbound NB  | 19117 | Ablehnung Sperr-/Entsperrauftrag (NB → LF)    |
//! | Inbound NB  | 19128 | Bestätigung Stornierung (NB → LF)             |
//! | Inbound NB  | 19129 | Ablehnung Stornierung (NB → LF)               |
//! | Inbound NB  | 21039 | Auftragsstatus Sperren (NB → LF)              |
//!
//! ## Regulatory basis
//!
//! - **AWH Sperrprozesse Gas / GPKE Teil 2** — BNetzA BK6-22-024
//! - NB must respond within **24 wall-clock hours**

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the LF-side Sperrung workflow.
pub const WORKFLOW_NAME: &str = "gpke-sperrung-lf";

/// Outbound PIDs that start a new `GpkeSperrungLfWorkflow` (LF sends to NB).
///
/// These are registered as inbound PIDs **only if** makod is running as NB
/// (for the NB-role `gpke-sperrung` workflow). The LF sends these outbound.
pub const SPERRUNG_ANFRAGE_PIDS: &[u32] = &[
    17115, // Sperrauftrag (LF → NB)
    17117, // Entsperrauftrag (LF → NB)
];

/// ORDRSP inbound response PIDs the LF receives from the NB after sending Sperrauftrag.
///
/// - 19116: Bestätigung Sperr-/Entsperrauftrag (NB → LF)
/// - 19117: Ablehnung Sperr-/Entsperrauftrag (NB → LF)
pub const ORDRSP_SPERRUNG_PIDS: &[u32] = &[19116, 19117];

/// ORDRSP inbound response PIDs the LF receives from the NB after Stornierung.
///
/// - 19128: Bestätigung Stornierung Sperr-/Entsperrauftrag (NB → LF)
/// - 19129: Ablehnung Stornierung Sperr-/Entsperrauftrag (NB → LF)
pub const ORDRSP_STORNO_PIDS: &[u32] = &[19128, 19129];

/// IFTSTA inbound PID: Auftragsstatus (Sperren/Entsperren) from NB → LF.
///
/// 21039 is sent by the NB after executing the Sperrung/Entsperrung.
pub const IFTSTA_SPERRUNG_PID: u32 = 21039;

/// Deadline label for the 24-hour NB response window.
///
/// BK6-22-024: the NB must send ORDRSP within **24 wall-clock hours** of receipt.
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-sperrung-lf-antwort-24h";

// ── Domain data ───────────────────────────────────────────────────────────────

/// Business data captured when the LF initiates a Sperrauftrag.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SperrungAuftragData {
    /// EIC/MaLo of the supply location.
    pub location_id: MaLo,
    /// GLN of the NB that will execute the Sperrung.
    pub nb_gln: MarktpartnerCode,
    /// BDEW Prüfidentifikator of the outbound ORDERS (17115 or 17117).
    pub pruefidentifikator: Pruefidentifikator,
    /// EDIFACT message reference of the outbound ORDERS.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE LF-side Sperrung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SperrungLfEvent {
    /// LF initiated a Sperrauftrag (ORDERS 17115 or Entsperrauftrag 17117).
    ///
    /// The ORDERS message is dispatched via the outbox at the same time.
    AuftragInitiiert {
        /// EIC/MaLo of the supply location.
        location_id: MaLo,
        /// GLN of the NB.
        nb_gln: MarktpartnerCode,
        /// Prüfidentifikator (17115 = Sperrauftrag, 17117 = Entsperrauftrag).
        pruefidentifikator: Pruefidentifikator,
        /// EDIFACT message reference of the outbound ORDERS.
        message_ref: MessageRef,
    },
    /// ORDRSP 19116 (Bestätigung) or 19117 (Ablehnung) received from NB.
    OrdrsepEmpfangen {
        /// 19116 = Bestätigung, 19117 = Ablehnung.
        pruefidentifikator: Pruefidentifikator,
        /// `true` for 19116 (NB accepts and will execute), `false` for 19117 (NB rejects).
        is_confirmed: bool,
        /// EDIFACT message reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// GLN of the sending NB.
        sender: MarktpartnerCode,
    },
    /// IFTSTA 21039 received from NB: Auftragsstatus Sperren (NB executed the Sperrung).
    IftstaAuftragsstatus {
        /// Prüfidentifikator (always 21039).
        pruefidentifikator: Pruefidentifikator,
        /// EDIFACT message reference of the inbound IFTSTA.
        message_ref: MessageRef,
        /// GLN of the sending NB.
        sender: MarktpartnerCode,
    },
    /// LF sent a Stornierung (ORDCHG 39000) to cancel the pending Sperrauftrag.
    StornierungGesendet {
        /// EDIFACT message reference of the outbound ORDCHG.
        message_ref: MessageRef,
    },
    /// ORDRSP 19128 (Bestätigung) or 19129 (Ablehnung Stornierung) received from NB.
    StornoOrdrsepEmpfangen {
        /// 19128 = Bestätigung, 19129 = Ablehnung.
        pruefidentifikator: Pruefidentifikator,
        /// `true` for 19128 (NB accepted cancellation), `false` for 19129 (cancellation rejected).
        is_confirmed: bool,
        /// EDIFACT message reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// GLN of the sending NB.
        sender: MarktpartnerCode,
    },
    /// A registered deadline expired before the NB responded.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for SperrungLfEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AuftragInitiiert { .. } => "SperrungLfAuftragInitiiert",
            Self::OrdrsepEmpfangen { .. } => "SperrungLfOrdrsepEmpfangen",
            Self::IftstaAuftragsstatus { .. } => "SperrungLfIftstaAuftragsstatus",
            Self::StornierungGesendet { .. } => "SperrungLfStornierungGesendet",
            Self::StornoOrdrsepEmpfangen { .. } => "SperrungLfStornoOrdrsepEmpfangen",
            Self::DeadlineExpired { .. } => "SperrungLfDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a GPKE LF-side Sperrung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → AuftragGesendet → OrdrsepErhalten(confirmed=true) → Ausgefuehrt (IFTSTA 21039)
///                       → OrdrsepErhalten(confirmed=false) → Abgelehnt (terminal)
///             ↘ StornierungGesendet → StornoBestaetigt (terminal)
///                                   → StornoAbgelehnt (terminal)
///             ↘ DeadlineExpired (terminal)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum SperrungLfState {
    /// No events yet.
    New,
    /// Sperrauftrag sent outbound; awaiting NB's ORDRSP.
    AuftragGesendet(SperrungAuftragData),
    /// NB confirmed the Sperrauftrag (ORDRSP 19116); awaiting execution (IFTSTA 21039).
    OrdrsepBestaetigt(SperrungAuftragData),
    /// NB rejected the Sperrauftrag (ORDRSP 19117, terminal).
    OrdrsepAbgelehnt {
        /// Rejection reason from ORDRSP.
        reason: Option<String>,
    },
    /// IFTSTA 21039 received: NB has executed the Sperrung (terminal success).
    Ausgefuehrt(SperrungAuftragData),
    /// Stornierung sent; awaiting NB's Storno-ORDRSP (19128/19129).
    StornierungGesendet(SperrungAuftragData),
    /// NB accepted the Stornierung (ORDRSP 19128, terminal).
    StornoBestaetigt(SperrungAuftragData),
    /// NB rejected the Stornierung (ORDRSP 19129, terminal — must proceed with Sperrung).
    StornoAbgelehnt(SperrungAuftragData),
    /// A deadline expired before the NB responded (terminal).
    DeadlineExpired {
        /// Label of the expired deadline.
        label: String,
    },
}

impl Default for SperrungLfState {
    fn default() -> Self {
        Self::New
    }
}

impl SperrungLfState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AuftragGesendet(_) => "AuftragGesendet",
            Self::OrdrsepBestaetigt(_) => "OrdrsepBestaetigt",
            Self::OrdrsepAbgelehnt { .. } => "OrdrsepAbgelehnt",
            Self::Ausgefuehrt(_) => "Ausgefuehrt",
            Self::StornierungGesendet(_) => "StornierungGesendet",
            Self::StornoBestaetigt(_) => "StornoBestaetigt",
            Self::StornoAbgelehnt(_) => "StornoAbgelehnt",
            Self::DeadlineExpired { .. } => "DeadlineExpired",
        }
    }

    /// Returns `true` if this is a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::OrdrsepAbgelehnt { .. }
                | Self::Ausgefuehrt(_)
                | Self::StornoBestaetigt(_)
                | Self::StornoAbgelehnt(_)
                | Self::DeadlineExpired { .. }
        )
    }

    /// Return `Some(&SperrungAuftragData)` if the process has domain data.
    #[must_use]
    pub fn auftrag_data(&self) -> Option<&SperrungAuftragData> {
        match self {
            Self::AuftragGesendet(d)
            | Self::OrdrsepBestaetigt(d)
            | Self::Ausgefuehrt(d)
            | Self::StornierungGesendet(d)
            | Self::StornoBestaetigt(d)
            | Self::StornoAbgelehnt(d) => Some(d),
            Self::New | Self::OrdrsepAbgelehnt { .. } | Self::DeadlineExpired { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE LF-side Sperrung workflow.
#[derive(Clone)]
pub enum SperrungLfCommand {
    /// ERP instructs the LF to issue a Sperrauftrag (17115) or Entsperrauftrag (17117) to NB.
    ///
    /// Emits `AuftragInitiiert` + an outbox entry for the ORDERS message.
    InitiateSperrung {
        /// Prüfidentifikator (17115 = Sperrauftrag, 17117 = Entsperrauftrag).
        pid: Pruefidentifikator,
        /// GLN of the NB to send the Sperrauftrag to.
        nb_gln: MarktpartnerCode,
        /// EIC/MaLo of the supply location.
        location_id: MaLo,
        /// EDIFACT message reference of the outbound ORDERS.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP 19116/19117 from NB received by the AS4 layer.
    ReceiveOrdrsp {
        /// 19116 = Bestätigung, 19117 = Ablehnung.
        pid: Pruefidentifikator,
        /// `true` for 19116, `false` for 19117.
        is_confirmed: bool,
        /// EDIFACT message reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// GLN of the NB sender.
        sender: MarktpartnerCode,
        /// Optional rejection reason from the ORDRSP (19117 only).
        reason: Option<String>,
    },
    /// Inbound IFTSTA 21039 from NB: Auftragsstatus after Sperrung execution.
    ReceiveIftsta {
        /// Prüfidentifikator (always 21039).
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the inbound IFTSTA.
        message_ref: MessageRef,
        /// GLN of the NB sender.
        sender: MarktpartnerCode,
    },
    /// ERP instructs the LF to stornieren the pending Sperrauftrag.
    ///
    /// Emits `StornierungGesendet` + outbox entry for ORDCHG 39000.
    SendStornierung {
        /// EDIFACT message reference of the outbound ORDCHG.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP 19128/19129 from NB received after Stornierung.
    ReceiveStornoOrdrsp {
        /// 19128 = Bestätigung, 19129 = Ablehnung.
        pid: Pruefidentifikator,
        /// `true` for 19128, `false` for 19129.
        is_confirmed: bool,
        /// EDIFACT message reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// GLN of the NB sender.
        sender: MarktpartnerCode,
    },
    /// Registered deadline expired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl CommandPayload for SperrungLfCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE LF-side Sperrung / Entsperrung workflow.
///
/// Handles the Lieferant's outbound Sperrauftrag lifecycle and all inbound
/// responses from the Netzbetreiber (ORDRSP 19116/19117, IFTSTA 21039) and
/// Stornierung responses (ORDRSP 19128/19129).
pub struct GpkeSperrungLfWorkflow;

impl Workflow for GpkeSperrungLfWorkflow {
    type State = SperrungLfState;
    type Event = SperrungLfEvent;
    type Command = SperrungLfCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        if deadline.label() == ANTWORT_WINDOW_LABEL && !state.is_terminal() {
            return Some(SperrungLfCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            });
        }
        None
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            SperrungLfEvent::AuftragInitiiert {
                location_id,
                nb_gln,
                pruefidentifikator,
                message_ref,
            } => SperrungLfState::AuftragGesendet(SperrungAuftragData {
                location_id: location_id.clone(),
                nb_gln: nb_gln.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: message_ref.clone(),
            }),
            SperrungLfEvent::OrdrsepEmpfangen {
                is_confirmed,
                pruefidentifikator: _,
                message_ref: _,
                sender: _,
            } => match state {
                SperrungLfState::AuftragGesendet(data) => {
                    if *is_confirmed {
                        SperrungLfState::OrdrsepBestaetigt(data)
                    } else {
                        SperrungLfState::OrdrsepAbgelehnt { reason: None }
                    }
                }
                other => other,
            },
            SperrungLfEvent::IftstaAuftragsstatus { .. } => match state {
                SperrungLfState::OrdrsepBestaetigt(data)
                | SperrungLfState::AuftragGesendet(data) => SperrungLfState::Ausgefuehrt(data),
                other => other,
            },
            SperrungLfEvent::StornierungGesendet { .. } => match state {
                SperrungLfState::AuftragGesendet(data)
                | SperrungLfState::OrdrsepBestaetigt(data) => {
                    SperrungLfState::StornierungGesendet(data)
                }
                other => other,
            },
            SperrungLfEvent::StornoOrdrsepEmpfangen {
                is_confirmed,
                pruefidentifikator: _,
                message_ref: _,
                sender: _,
            } => match state {
                SperrungLfState::StornierungGesendet(data) => {
                    if *is_confirmed {
                        SperrungLfState::StornoBestaetigt(data)
                    } else {
                        SperrungLfState::StornoAbgelehnt(data)
                    }
                }
                other => other,
            },
            SperrungLfEvent::DeadlineExpired { label, .. } => {
                if state.is_terminal() {
                    state
                } else {
                    SperrungLfState::DeadlineExpired {
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
            SperrungLfCommand::InitiateSperrung {
                pid,
                nb_gln,
                location_id,
                message_ref,
            } => {
                if !matches!(state, SperrungLfState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !SPERRUNG_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Sperrung PID (17115 or 17117), got {pid}",
                    )));
                }
                let outbox = PendingOutbox::new(
                    "ORDERS",
                    nb_gln.as_str(),
                    serde_json::json!({
                        "type":          "SperrungAuftrag",
                        "pid":           pid.as_u32(),
                        "location_id":   location_id.as_str(),
                        "message_ref":   message_ref.as_str(),
                    }),
                );
                let event = SperrungLfEvent::AuftragInitiiert {
                    location_id,
                    nb_gln,
                    pruefidentifikator: pid,
                    message_ref,
                };
                Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
            }

            SperrungLfCommand::ReceiveOrdrsp {
                pid,
                is_confirmed,
                message_ref,
                sender,
                reason: _,
            } => {
                if !ORDRSP_SPERRUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Sperrung-ORDRSP PID (19116 or 19117), got {pid}",
                    )));
                }
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![SperrungLfEvent::OrdrsepEmpfangen {
                    pruefidentifikator: pid,
                    is_confirmed,
                    message_ref,
                    sender,
                }]
                .into())
            }

            SperrungLfCommand::ReceiveIftsta {
                pid,
                message_ref,
                sender,
            } => {
                if pid.as_u32() != IFTSTA_SPERRUNG_PID {
                    return Err(WorkflowError::rejected(format!(
                        "expected IFTSTA PID 21039, got {pid}",
                    )));
                }
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![SperrungLfEvent::IftstaAuftragsstatus {
                    pruefidentifikator: pid,
                    message_ref,
                    sender,
                }]
                .into())
            }

            SperrungLfCommand::SendStornierung { message_ref } => {
                if !matches!(
                    state,
                    SperrungLfState::AuftragGesendet(_) | SperrungLfState::OrdrsepBestaetigt(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "AuftragGesendet or OrdrsepBestaetigt",
                        state.label(),
                    ));
                }
                let nb_gln = state
                    .auftrag_data()
                    .map(|d| d.nb_gln.as_str().to_owned())
                    .unwrap_or_default();
                let outbox = PendingOutbox::new(
                    "ORDCHG",
                    nb_gln.as_str(),
                    serde_json::json!({
                        "type":        "SperrungStornierung",
                        "pid":         39000_u32,
                        "message_ref": message_ref.as_str(),
                    }),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![SperrungLfEvent::StornierungGesendet { message_ref }],
                    vec![outbox],
                ))
            }

            SperrungLfCommand::ReceiveStornoOrdrsp {
                pid,
                is_confirmed,
                message_ref,
                sender,
            } => {
                if !ORDRSP_STORNO_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Storno-ORDRSP PID (19128 or 19129), got {pid}",
                    )));
                }
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![SperrungLfEvent::StornoOrdrsepEmpfangen {
                    pruefidentifikator: pid,
                    is_confirmed,
                    message_ref,
                    sender,
                }]
                .into())
            }

            SperrungLfCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![SperrungLfEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}
