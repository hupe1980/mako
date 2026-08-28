//! WiM Strom/Gas Technikänderung — device/measurement-point change requests.
//!
//! This module handles ORDERS-based requests for technical changes at a
//! Messlokation:
//!
//! | PID   | Direction  | Description |
//! |-------|------------|-------------|
//! | 17011 | NB / LF → MSB | Beauftragung resp. Bestellung zur Änderung der Technik |
//! | 17118 | MSB → MSB  | Bestellung einer Konfigurationsänderung |
//! | 19005 | MSB → NB/LF | Bestätigung Auftrag Änderung Technik |
//! | 19006 | MSB → NB/LF | Ablehnung Auftrag Änderung Technik |
//!
//! **17121 is not here.** „Bestellung Änderung (NB an MSB)" belongs to GPKE
//! Teil 3 and is answered with ORDRSP 19120 out of `E_0526`; `mako-gpke` owns
//! it.
//!
//! The ESA Ab-/Bestellung PIDs (ORDERS 17007, ORDRSP 19011/19012/19013/19014)
//! belong to [`crate::wertebestellung`], which models their own lifecycle.
//!
//! # Two documents, one message pair
//!
//! WiM Strom Teil 1 Kap. 3.3 has the NB or the LF order the change outright;
//! the BDEW *AWH Prozesse zur Änderung der Technik an Lokationen* V1.1 puts a
//! REQOTE 35005 / QUOTES 15005 round in front of the same order. Both end in
//! ORDERS 17011 → ORDRSP 19005/19006, so **four** Entscheidungsbäume share one
//! answer PID pair and the sender's Marktrolle resolves only half of it — see
//! [`mako_pruefung::msb::technik`], which is where that resolution lives.
//!
//! The leg after the answer is the **Durchführung**: the MSB goes to the
//! Lokation and reports back only if the visit failed, on IFTSTA 21027 (to the
//! NB) resp. 21025 (to the LF) out of `E_0286`.
//!
//! # Regulatory basis
//!
//! - **BK6-22-024** — WiM Strom Teil 1 Kap. 3.3 (Messlokationsänderung)
//! - **BDEW AWH Prozesse zur Änderung der Technik an Lokationen** V1.1 (31.03.2025)
//! - **Entscheidungsbaum-Diagramme und Codelisten 4.3** Kap. 8.6, 8.7, 9.1, 9.2
//! - Antwortfrist: **10 Werktage** (WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2
//!   Nr. 2), against a Mindestvorlauffrist of **20 Werktagen** on the direct
//!   Beauftragung (Nr. 1). Both are anchored differently: the answer window runs
//!   forward from the ÜT, the Vorlauffrist backward from the gewünschter
//!   Änderungstermin — see [`mako_fristen::vorlauf`]. The AWH Bestellung has no
//!   Vorlauffrist Prüfschritt at all; its Umsetzungszeitraum was agreed in the
//!   Angebot.

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
/// | 17011 | Beauftragung zur Änderung der Technik (Messlokationsänderung Strom) |
/// | 17118 | Bestellung Konfigurationsänderung (MSB → MSB) |
pub const ORDERS_PIDS: &[u32] = &[17011, 17118];

/// ORDRSP PIDs received in response to technical change requests.
///
/// | PID   | Description | EBD |
/// |-------|-------------|-----|
/// | 19005 | Bestätigung Auftrag Änderung Technik | `E_0249` / `E_0250` |
/// | 19006 | Ablehnung Auftrag Änderung Technik | `E_0249` / `E_0250` |
///
/// Three neighbouring ORDRSP PIDs answer other processes (ORDRSP AHB 1.1b
/// §§ 4.11–4.13) and are not part of this set:
///
/// - **19003 / 19004** „Fortführungsbestätigung / Ablehnung Fortführung"
///   answer the Weiterverpflichtung ORDERS 17002 out of `E_0203` —
///   [`crate::weiterverpflichtung`].
/// - **19007** „Ablehnung Anforderung von Werten" answers a Werteanforderung.
///
/// # Four trees, one PID pair
///
/// 19005/19006 are shared by `E_0249`/`E_0250` (WiM Teil 1, direct
/// Beauftragung) and `E_0279`/`E_0283` (AWH, Bestellung nach Angebot). The
/// sender's Marktrolle separates the columns; the ORDERS' Zuordnung zu einem
/// Objekt (`ZO-T15` against `ZG-T24`) separates the rows. Resolve with
/// [`mako_pruefung::codes::aenderung_der_technik_baum`] — never against the
/// answer PID, and never on the Marktrolle alone: `A02` is the Zustimmung of
/// `E_0249` and an Ablehnung of `E_0279`.
pub const ORDRSP_PIDS: &[u32] = &[19005, 19006];

/// Positive ORDRSP PIDs (confirmation).
const ORDRSP_BESTAETIGUNG_PIDS: &[u32] = &[19005];

/// Deadline label for the response window (10 Werktage, WiM Strom).
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
