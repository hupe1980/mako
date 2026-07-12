//! GPKE Teil 3 Konfigurationsänderung — LF-initiated configuration change requests.
//!
//! This module covers all LF→NB and LF→MSB ORDERS requests for modifying the
//! existing configuration established via [`crate::konfiguration`], together
//! with the corresponding NB/MSB→LF ORDRSP responses.
//!
//! # Process families
//!
//! | PID   | Direction   | Description |
//! |-------|-------------|-------------|
//! | 17120 | LF → NB     | Bestellung Änderung Prognosegrundlage |
//! | 17122 | LF → NB/MSB | Reklamation einer Definition |
//! | 17123 | LF → NB/MSB | Bestellung Änderung Zählzeitdefinition |
//! | 17133 | LF → NB     | Bestellung Änderung Abrechnungsdaten |
//! | 17128 | LF → MSB    | Reklamation einer Konfiguration |
//! | 17129 | LF → MSB    | Bestellung Beendigung einer Konfiguration |
//! | 17130 | LF → MSB    | Bestellung einer Konfiguration (ohne Angebot) |
//! | 17131 | LF → MSB    | Bestellung Angebot einer Konfiguration |
//! | 17121 | NB → MSB    | Bestellung Änderung (NB forwards, MSB receives) |
//! | 19120 | NB → LF     | Mitteilung zur Änderung |
//! | 19121 | NB → LF     | Mitteilung Änderung Prognosegrundlage |
//! | 19123 | NB/MSB → LF | Ablehnung der Reklamation einer Definition |
//! | 19124 | NB/MSB → LF | Mitteilung Änderung Zählzeitdefinition |
//! | 19127 | NB → LF     | Mitteilung zur Konfigurationsänderung |
//! | 19133 | NB → LF     | Bearbeitungsstand Bestellung Abrechnungsdaten |
//! | 19130 | MSB → LF    | Bearbeitungsstand Reklamation Konfiguration |
//! | 19131 | MSB → LF    | Antwort auf Bestellung Beendigung einer Konfiguration |
//! | 19132 | MSB → LF    | Mitteilung Bestellung Änderung einer Konfiguration |
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE Teil 3** — Geschäftsprozesse Konfiguration (BK6-22-024)
//! - Response window: 5 Werktage for NB/MSB to respond (GPKE AWH Teil 3)

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Workflow name for the GPKE Konfigurationsänderung process.
pub const WORKFLOW_NAME: &str = "gpke-konfiguration-aenderung";

/// ORDERS PIDs sent by LF to NB for configuration change requests.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17120 | Bestellung Änderung Prognosegrundlage |
/// | 17122 | Reklamation einer Definition |
/// | 17123 | Bestellung Änderung Zählzeitdefinition |
/// | 17133 | Bestellung Änderung Abrechnungsdaten |
pub const ORDERS_ANFRAGE_NB_PIDS: &[u32] = &[17120, 17122, 17123, 17133];

/// ORDERS PIDs sent by LF to MSB for configuration change requests.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17128 | Reklamation einer Konfiguration |
/// | 17129 | Bestellung Beendigung einer Konfiguration |
/// | 17130 | Bestellung einer Konfiguration (ohne Angebot) |
/// | 17131 | Bestellung Angebot einer Konfiguration |
pub const ORDERS_ANFRAGE_MSB_PIDS: &[u32] = &[17128, 17129, 17130, 17131];

/// ORDERS PID sent by NB to MSB (NB-internal forwarding, also received by MSB role).
///
/// | PID   | Description |
/// |-------|-------------|
/// | 17121 | Bestellung Änderung (NB → MSB) |
pub const ORDERS_NB_MSB_PIDS: &[u32] = &[17121];

/// All LF-sent ORDERS PIDs (NB-directed + MSB-directed) for registration.
pub const ORDERS_ANFRAGE_PIDS: &[u32] = &[
    17120, 17121, 17122, 17123, 17128, 17129, 17130, 17131, 17133,
];

/// ORDRSP PIDs from NB back to LF.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 19120 | Mitteilung zur Änderung |
/// | 19121 | Mitteilung Änderung Prognosegrundlage |
/// | 19123 | Ablehnung der Reklamation einer Definition |
/// | 19124 | Mitteilung Änderung Zählzeitdefinition |
/// | 19127 | Mitteilung zur Konfigurationsänderung |
/// | 19133 | Bearbeitungsstand Bestellung Abrechnungsdaten |
pub const ORDRSP_NB_LF_PIDS: &[u32] = &[19120, 19121, 19123, 19124, 19127, 19133];

/// ORDRSP PIDs from MSB back to LF.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 19130 | Bearbeitungsstand Reklamation Konfiguration |
/// | 19131 | Antwort auf Bestellung Beendigung einer Konfiguration |
/// | 19132 | Mitteilung Bestellung Änderung einer Konfiguration |
pub const ORDRSP_MSB_LF_PIDS: &[u32] = &[19130, 19131, 19132];

/// All inbound ORDRSP PIDs for registration routing.
pub const ORDRSP_PIDS: &[u32] = &[
    19120, 19121, 19123, 19124, 19127, 19130, 19131, 19132, 19133,
];

/// IFTSTA PIDs for GPKE Teil 3 Konfigurationsänderung status messages.
///
/// These IFTSTA messages are informational confirmations and status updates
/// sent by NB or MSB back to LF after a ORDERS Konfigurationsänderung.
///
/// | PID   | Description | Direction |
/// |-------|-------------|-----------|
/// | 21043 | Bestellungsantwort / -mitteilung (GPKE Teil 3) | NB → LF · MSB → LF |
/// | 21044 | Bestellungsbeendigung (GPKE Teil 3) | MSB → NB · MSB → LF |
///
/// Source: `docs/pid-reference.md` (generated from BDEW xlsx PID 3.3 + PID 4.0).
/// PID 21042 (Bestellung WiM, WiM Strom Teil 2, MSB → ESA) is NOT registered
/// here — it has no crate assignment in pid-reference.md and will dead-letter.
pub const IFTSTA_PIDS: &[u32] = &[21_043, 21_044];

/// Deadline label for the NB/MSB response window (5 Werktage, GPKE AWH Teil 3).
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-konfiguration-aenderung-antwort";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Konfigurationsänderung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KonfigurationAenderungEvent {
    /// LF sent an ORDERS configuration change request.
    AnfrageGesendet {
        /// ORDERS Prüfidentifikator (one of ORDERS_ANFRAGE_PIDS).
        orders_pid: Pruefidentifikator,
        /// GLN of the recipient (NB or MSB).
        recipient: MarktpartnerCode,
        /// EIC/MaLo of the affected supply location.
        malo: MaLo,
        /// Message reference of the sent ORDERS.
        message_ref: MessageRef,
    },
    /// NB or MSB confirmed / acknowledged the change request (positive ORDRSP).
    AntwortErhalten {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// `true` = positive response (Bestätigung/Mitteilung); `false` = rejection.
        accepted: bool,
        /// Optional plain-text reason (present on rejections).
        reason: Option<String>,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// Intermediate status received (e.g. 19133 Bearbeitungsstand).
    ZwischenstandErhalten {
        /// ORDRSP Prüfidentifikator.
        ordrsp_pid: Pruefidentifikator,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// A registered deadline expired before the ORDRSP arrived.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for KonfigurationAenderungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "KonfigAenderungAnfrageGesendet",
            Self::AntwortErhalten { .. } => "KonfigAenderungAntwortErhalten",
            Self::ZwischenstandErhalten { .. } => "KonfigAenderungZwischenstandErhalten",
            Self::DeadlineExpired { .. } => "KonfigAenderungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Data captured when an ORDERS request is sent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnfrageData {
    /// ORDERS Prüfidentifikator sent.
    pub orders_pid: Pruefidentifikator,
    /// Recipient (NB or MSB) GLN.
    pub recipient: MarktpartnerCode,
    /// Affected MaLo.
    pub malo: MaLo,
    /// Message reference of the sent ORDERS.
    pub message_ref: MessageRef,
}

/// Current state of a GPKE Konfigurationsänderung process stream.
///
/// ```text
/// New → AnfrageGesendet → Beantwortet
///                      ↘ DeadlineExpired
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum KonfigurationAenderungState {
    /// No ORDERS sent yet (initial state).
    #[default]
    New,
    /// LF sent ORDERS; waiting for ORDRSP or status update.
    AnfrageGesendet(AnfrageData),
    /// Final response received (ORDRSP accept or reject).
    Beantwortet {
        /// Original request data.
        anfrage: AnfrageData,
        /// ORDRSP PID received.
        ordrsp_pid: Pruefidentifikator,
        /// Whether the response was positive.
        accepted: bool,
    },
    /// Deadline expired without a final response.
    DeadlineExpired {
        /// Original request data.
        anfrage: AnfrageData,
    },
}

impl KonfigurationAenderungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageGesendet(_) => "AnfrageGesendet",
            Self::Beantwortet { .. } => "Beantwortet",
            Self::DeadlineExpired { .. } => "DeadlineExpired",
        }
    }

    /// Whether the process has reached a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Beantwortet { .. } | Self::DeadlineExpired { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Konfigurationsänderung workflow.
#[derive(Clone)]
pub enum KonfigurationAenderungCommand {
    /// LF initiates a configuration change request.
    ///
    /// Creates the outbound ORDERS message via the outbox.
    SendAnfrage {
        /// ORDERS Prüfidentifikator (one of ORDERS_ANFRAGE_PIDS).
        orders_pid: Pruefidentifikator,
        /// GLN of the recipient (NB or MSB).
        recipient: MarktpartnerCode,
        /// EIC/MaLo of the affected supply location.
        malo: MaLo,
        /// Message reference of the outbound ORDERS.
        message_ref: MessageRef,
        /// Optional structured payload for the ORDERS body.
        payload: serde_json::Value,
    },
    /// Inbound ORDRSP received from NB or MSB.
    ReceiveOrdrsp {
        /// ORDRSP Prüfidentifikator (one of ORDRSP_PIDS).
        ordrsp_pid: Pruefidentifikator,
        /// `true` = positive; `false` = rejection or unresolved status.
        accepted: bool,
        /// Optional rejection reason.
        reason: Option<String>,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for KonfigurationAenderungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Konfigurationsänderung workflow — LF-initiated config change requests
/// (GPKE Teil 3, BK6-22-024).
pub struct GpkeKonfigurationAenderungWorkflow;

/// Rejection ORDRSP PIDs (NB/MSB rejects LF's request).
const REJECTION_PIDS: &[u32] = &[19123, 19130];

/// PIDs that represent intermediate status updates (not final responses).
const ZWISCHENSTAND_PIDS: &[u32] = &[19133];

impl Workflow for GpkeKonfigurationAenderungWorkflow {
    type State = KonfigurationAenderungState;
    type Event = KonfigurationAenderungEvent;
    type Command = KonfigurationAenderungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (ANTWORT_WINDOW_LABEL, KonfigurationAenderungState::AnfrageGesendet(_)) => {
                Some(KonfigurationAenderungCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            KonfigurationAenderungEvent::AnfrageGesendet {
                orders_pid,
                recipient,
                malo,
                message_ref,
            } => KonfigurationAenderungState::AnfrageGesendet(AnfrageData {
                orders_pid: *orders_pid,
                recipient: recipient.clone(),
                malo: malo.clone(),
                message_ref: message_ref.clone(),
            }),
            KonfigurationAenderungEvent::ZwischenstandErhalten { .. } => state, // no-op, informational
            KonfigurationAenderungEvent::AntwortErhalten {
                ordrsp_pid,
                accepted,
                ..
            } => match state {
                KonfigurationAenderungState::AnfrageGesendet(anfrage) => {
                    KonfigurationAenderungState::Beantwortet {
                        anfrage,
                        ordrsp_pid: *ordrsp_pid,
                        accepted: *accepted,
                    }
                }
                other => other,
            },
            KonfigurationAenderungEvent::DeadlineExpired { .. } => match state {
                KonfigurationAenderungState::AnfrageGesendet(anfrage) => {
                    KonfigurationAenderungState::DeadlineExpired { anfrage }
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
            KonfigurationAenderungCommand::SendAnfrage {
                orders_pid,
                recipient,
                malo,
                message_ref,
                payload,
            } => {
                if !matches!(state, KonfigurationAenderungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_ANFRAGE_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid GPKE Konfigurationsänderung ORDERS PID: {orders_pid}",
                    )));
                }
                let event = KonfigurationAenderungEvent::AnfrageGesendet {
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
                        "malo":       malo.as_str(),
                        "orders_ref": message_ref.as_str(),
                        "payload":    payload,
                    }),
                )];
                Ok(WorkflowOutput::with_outbox(vec![event], outbox))
            }

            KonfigurationAenderungCommand::ReceiveOrdrsp {
                ordrsp_pid,
                accepted,
                reason,
                message_ref,
            } => {
                // Accept ORDRSP in AnfrageGesendet or (defensively) in terminal states
                if state.is_terminal() {
                    // Late-arriving ORDRSP after deadline — record as informational
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !matches!(state, KonfigurationAenderungState::AnfrageGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageGesendet",
                        state.label(),
                    ));
                }
                if !ORDRSP_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not a valid GPKE Konfigurationsänderung ORDRSP PID: {ordrsp_pid}",
                    )));
                }
                // Intermediate status updates (e.g. 19133 Bearbeitungsstand)
                // don't close the process — they're informational.
                if ZWISCHENSTAND_PIDS.contains(&ordrsp_pid.as_u32()) {
                    return Ok(vec![KonfigurationAenderungEvent::ZwischenstandErhalten {
                        ordrsp_pid,
                        message_ref,
                    }]
                    .into());
                }
                // Determine acceptance: rejection PIDs are always negative.
                let is_accepted = accepted && !REJECTION_PIDS.contains(&ordrsp_pid.as_u32());
                Ok(vec![KonfigurationAenderungEvent::AntwortErhalten {
                    ordrsp_pid,
                    accepted: is_accepted,
                    reason,
                    message_ref,
                }]
                .into())
            }

            KonfigurationAenderungCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(
                    vec![KonfigurationAenderungEvent::DeadlineExpired { deadline_id, label }]
                        .into(),
                )
            }
        }
    }
}
