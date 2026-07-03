//! GPKE Allokationsliste — LF requests allocation lists and billing basis data.
//!
//! This module handles LF-initiated requests for **Allokationslisten** and
//! **bilanzierte Menge** data as defined in the GPKE / MMM (Mehr-/Mindermenge)
//! process family (BK6-22-024):
//!
//! | PID   | ⚡ | 🔥 | Direction | Description |
//! |-------|---|---|-----------|-------------|
//! | 17110 | — | ✅ | LF → NB   | Anforderung Allokationsliste (Gas-only per ORDERS AHB 1.0; formally belongs to `mako-gabi-gas` `gabi-gas-mmma`) |
//! | 17114 | ✅ | — | NB → ÜNB  | Anforderung der bilanzierten Menge (Strom-only) |
//! | 19110 | — | ✅ | NB → LF   | Ablehnung Allokationsliste (Gas-only; formally belongs to `mako-gabi-gas` `gabi-gas-mmma`) |
//! | 19115 | ✅ | — | NB → LF   | Ablehnung Anforderung bilanzierte Menge (Strom-only) |
//!
//! > **Note on 17110 / 19110**: These PIDs are Gas-only (⚡=— in ORDERS/ORDRSP AHB 1.0)
//! > and are owned by `mako-gabi-gas` `gabi-gas-mmma` per the authoritative PID table.
//! > They are currently also handled in this workflow to avoid breaking the shared
//! > `gpke-allokationsliste` state machine while the Gas-specific workflow is built out.
//! > Once `gabi-gas-mmma` implements its full ORDERS/ORDRSP state machine, 17110/19110
//! > will be removed from this array and routed exclusively via that workflow.
//!
//! Positive responses (actual data) arrive via **MSCONS** (13014 for Strom bilanzierte
//! Menge, further MSCONS PIDs for Strom). PID 13013 (Gas-only Allokationsliste) is
//! handled by `mako-gabi-gas` `gabi-gas-mmma`.
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE / MMM Strom** — BK6-22-024

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name for the GPKE Allokationsliste / bilanzierte Menge process.
pub const WORKFLOW_NAME: &str = "gpke-allokationsliste";

/// ORDERS PIDs sent by LF (or other party) to NB requesting allocation data.
///
/// | PID   | ⚡ | 🔥 | Description |
/// |-------|---|---|-------------|
/// | 17110 | — | ✅ | Anforderung Allokationsliste (Gas-only; formally `gabi-gas-mmma`) |
/// | 17114 | ✅ | — | Anforderung der bilanzierten Menge (Strom-only) |
pub const ORDERS_ANFRAGE_PIDS: &[u32] = &[17110, 17114];

/// ORDRSP rejection PIDs sent by NB back to LF.
///
/// | PID   | ⚡ | 🔥 | Description |
/// |-------|---|---|-------------|
/// | 19110 | — | ✅ | Ablehnung Allokationsliste (Gas-only; formally `gabi-gas-mmma`) |
/// | 19115 | ✅ | — | Ablehnung Anforderung bilanzierte Menge (Strom-only) |
pub const ORDRSP_ABLEHNUNG_PIDS: &[u32] = &[19110, 19115];

/// MSCONS positive-response PIDs (NB sends Strom allocation data to LF).
///
/// These arrive inbound at the LF as the positive answer to ORDERS 17110/17114.
/// They are **MMM Strom** PIDs — NOT GeLi Gas (BK7-24-01-009) PIDs.
///
/// PID 13013 (Marktlokationsscharfe Allokationsliste Gas, Gas-only) has moved to
/// `mako-gabi-gas` `gabi-gas-mmma`. Only the Strom side (13014) is registered here.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 13014 | Marktlokationsscharfe bilanzierte Menge Strom/Gas (MMMA), Strom side |
pub const MSCONS_RESPONSE_PIDS: &[u32] = &[13014];

/// Deadline label for the allocation-list response window.
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-allokationsliste-antwort";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Allokationsliste workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AllokationslisteEvent {
    /// LF sent an ORDERS allocation-list request.
    AnforderungGesendet {
        /// ORDERS Prüfidentifikator (17114 for Strom; 17110 for Gas/shared).
        orders_pid: Pruefidentifikator,
        /// NB GLN.
        nb_gln: MarktpartnerCode,
        /// Affected MaLo (optional).
        malo: Option<MaLo>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// NB rejected the allocation-list request.
    AnforderungAbgelehnt {
        /// ORDRSP Prüfidentifikator (19115 for Strom; 19110 for Gas/shared).
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Allocation list data delivered (via MSCONS).
    DatenGeliefert {
        /// Message reference of the MSCONS.
        message_ref: MessageRef,
    },
    /// Deadline expired before response arrived.
    DeadlineExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for AllokationslisteEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnforderungGesendet { .. } => "AllokationslisteAnforderungGesendet",
            Self::AnforderungAbgelehnt { .. } => "AllokationslisteAnforderungAbgelehnt",
            Self::DatenGeliefert { .. } => "AllokationslisteDatenGeliefert",
            Self::DeadlineExpired { .. } => "AllokationslisteDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Data from the sent ORDERS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnforderungData {
    /// ORDERS PID.
    pub orders_pid: Pruefidentifikator,
    /// NB GLN.
    pub nb_gln: MarktpartnerCode,
    /// Optional MaLo.
    pub malo: Option<MaLo>,
    /// Message reference.
    pub message_ref: MessageRef,
}

/// Current state of a GPKE Allokationsliste process.
///
/// ```text
/// New → AnforderungGesendet → DatenErhalten
///                          ↘ Abgelehnt
///                          ↘ DeadlineExpired
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum AllokationslisteState {
    /// No request sent yet.
    New,
    /// LF sent the request; awaiting MSCONS data or rejection.
    AnforderungGesendet(AnforderungData),
    /// NB rejected the request.
    Abgelehnt {
        /// Original request.
        anforderung: AnforderungData,
        /// Rejection ORDRSP PID.
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
    },
    /// Data arrived via MSCONS.
    DatenErhalten {
        /// Original request.
        anforderung: AnforderungData,
    },
    /// Deadline expired.
    DeadlineExpired {
        /// Original request.
        anforderung: AnforderungData,
    },
}

impl Default for AllokationslisteState {
    fn default() -> Self {
        Self::New
    }
}

impl AllokationslisteState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnforderungGesendet(_) => "AnforderungGesendet",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::DatenErhalten { .. } => "DatenErhalten",
            Self::DeadlineExpired { .. } => "DeadlineExpired",
        }
    }

    /// Whether terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Abgelehnt { .. } | Self::DatenErhalten { .. } | Self::DeadlineExpired { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Allokationsliste workflow.
#[derive(Clone)]
pub enum AllokationslisteCommand {
    /// LF requests an allocation list or billing-basis data.
    SendAnforderung {
        /// ORDERS PID (17114 for Strom; 17110 for Gas/shared).
        orders_pid: Pruefidentifikator,
        /// NB GLN.
        nb_gln: MarktpartnerCode,
        /// Affected MaLo (optional).
        malo: Option<MaLo>,
        /// Message reference.
        message_ref: MessageRef,
        /// ORDERS body payload.
        payload: serde_json::Value,
    },
    /// NB rejected the request.
    ReceiveAblehnung {
        /// ORDRSP PID (19115 for Strom; 19110 for Gas/shared).
        ordrsp_pid: Pruefidentifikator,
        /// Rejection reason.
        reason: Option<String>,
        /// Message reference.
        message_ref: MessageRef,
    },
    /// MSCONS data delivered (positive response).
    NotifyDatenGeliefert {
        /// Message reference.
        message_ref: MessageRef,
    },
    /// Deadline fired.
    TimeoutExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for AllokationslisteCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Allokationsliste workflow — LF requests allocation and billing-basis data.
pub struct GpkeAllokationslisteWorkflow;

impl Workflow for GpkeAllokationslisteWorkflow {
    type State = AllokationslisteState;
    type Event = AllokationslisteEvent;
    type Command = AllokationslisteCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (ANTWORT_WINDOW_LABEL, AllokationslisteState::AnforderungGesendet(_)) => {
                Some(AllokationslisteCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            AllokationslisteEvent::AnforderungGesendet {
                orders_pid,
                nb_gln,
                malo,
                message_ref,
            } => AllokationslisteState::AnforderungGesendet(AnforderungData {
                orders_pid: *orders_pid,
                nb_gln: nb_gln.clone(),
                malo: malo.clone(),
                message_ref: message_ref.clone(),
            }),
            AllokationslisteEvent::AnforderungAbgelehnt {
                ordrsp_pid, reason, ..
            } => match state {
                AllokationslisteState::AnforderungGesendet(anforderung) => {
                    AllokationslisteState::Abgelehnt {
                        anforderung,
                        ordrsp_pid: *ordrsp_pid,
                        reason: reason.clone(),
                    }
                }
                other => other,
            },
            AllokationslisteEvent::DatenGeliefert { .. } => match state {
                AllokationslisteState::AnforderungGesendet(anforderung) => {
                    AllokationslisteState::DatenErhalten { anforderung }
                }
                other => other,
            },
            AllokationslisteEvent::DeadlineExpired { .. } => match state {
                AllokationslisteState::AnforderungGesendet(anforderung) => {
                    AllokationslisteState::DeadlineExpired { anforderung }
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
            AllokationslisteCommand::SendAnforderung {
                orders_pid,
                nb_gln,
                malo,
                message_ref,
                payload,
            } => {
                if !matches!(state, AllokationslisteState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_ANFRAGE_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid Allokationsliste ORDERS PID: {orders_pid}",
                    )));
                }
                let event = AllokationslisteEvent::AnforderungGesendet {
                    orders_pid,
                    nb_gln: nb_gln.clone(),
                    malo: malo.clone(),
                    message_ref: message_ref.clone(),
                };
                let outbox = vec![PendingOutbox::new(
                    "ORDERS",
                    nb_gln.as_str(),
                    serde_json::json!({
                        "pid":        orders_pid.as_u32(),
                        "malo":       malo.as_ref().map(|m| m.as_str()),
                        "orders_ref": message_ref.as_str(),
                        "payload":    payload,
                    }),
                )];
                Ok(WorkflowOutput::with_outbox(vec![event], outbox))
            }

            AllokationslisteCommand::ReceiveAblehnung {
                ordrsp_pid,
                reason,
                message_ref,
            } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !matches!(state, AllokationslisteState::AnforderungGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnforderungGesendet",
                        state.label(),
                    ));
                }
                if !ORDRSP_ABLEHNUNG_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid Allokationsliste rejection ORDRSP PID: {ordrsp_pid}",
                    )));
                }
                Ok(vec![AllokationslisteEvent::AnforderungAbgelehnt {
                    ordrsp_pid,
                    reason,
                    message_ref,
                }]
                .into())
            }

            AllokationslisteCommand::NotifyDatenGeliefert { message_ref } => {
                if !matches!(state, AllokationslisteState::AnforderungGesendet(_)) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![AllokationslisteEvent::DatenGeliefert { message_ref }].into())
            }

            AllokationslisteCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![AllokationslisteEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}
