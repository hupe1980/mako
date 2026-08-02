//! WiM **Gas** Geräteübernahme — takeover of metering equipment at a Gas Messlokation.
//!
//! Models the **MSBA side** of **WiM Gas (BK7-24-01-009), AWH V2.0 §4.2**: the new
//! Messstellenbetreiber (MSBN) requests an offer for the technical equipment from
//! the outgoing MSBA, then orders against it. This is the Gas twin of
//! `mako-wim::geraeteubernahme` — identical message flow and Fristen, distinct only
//! in its regulatory basis (WiM Gas AWH V2.0) and Gas EBD decision trees
//! (E_2010 „Anforderung Geräteübernahmeangebot prüfen" / E_2011 „Bestellung prüfen").
//!
//! # Message flow
//!
//! ```text
//! MSBN ──REQOTE 35001 Anforderung Geräteübernahmeangebot──────────────────▶ MSBA
//! MSBN ◀─QUOTES 15001 Geräteübernahmeangebot──── 4 WT nach ÜT von Nr. 1 ──── MSBA
//! MSBN ──ORDERS 17001 Bestellung──────────────── 3 WT nach ÜT von Nr. 2 ───▶ MSBA
//! MSBN ◀─ORDRSP 19001 Bestellbestätigung──────── 2 WT nach ÜT von Nr. 3 ──── MSBA
//!        or     19002 Ablehnung der Bestellung
//! ```
//!
//! Like the Strom twin, this workflow is modelled from the receiving (MSBA/NB)
//! perspective and is driven from the ORDERS leg; the REQOTE 35001 offer-request
//! step is not yet a distinct workflow trigger (a shared follow-up for both Sparten).
//!
//! # Commodity isolation
//!
//! The Geräteübernahme PIDs (17001/17002/17009, QUOTES 15001, ORDRSP 19001/19002)
//! are shared with WiM Strom. `WimGasModule` registers the inbound ORDERS PIDs with
//! `Sparte::Gas` via `PidRouter::register_with_sparte`, and `WimModule` registers
//! them with `Sparte::Strom`, so the AS4 ingest routes each interchange to the
//! correct commodity workflow by the UNB DE0010 recipient Sparte.
//!
//! # Regulatory basis
//!
//! - **WiM Gas AWH V2.0 §4.2** (BDEW/VKU/GEODE/FNB Gas, 04.08.2025) — Geräteübernahme
//! - **BK7-24-01-009** — WiM Gas (BNetzA ruling, 12.09.2025)

use std::collections::HashMap;

use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-gas-geraeteubernahme";

/// Inbound ORDERS PIDs routed to this workflow (WiM Gas AWH V2.0 §4.2).
///
/// | PID | AHB name |
/// |---|---|
/// | 17001 | Bestellung Geräteübernahmeangebot |
/// | 17002 | Weiterverpflichtung MSBA bei Ende Messstellenbetrieb |
/// | 17009 | Ankündigung Gerätewechselabsicht |
///
/// Shared with WiM Strom — registered here with `Sparte::Gas` (see `lib.rs`).
pub const GERAETEUBERNAHME_PIDS: &[u32] = &[17001, 17002, 17009];

/// Anfrage PIDs — trigger a new `WimGasGeraeteubernahmeWorkflow` process.
pub const ANFRAGE_PIDS: &[u32] = &[17001, 17002];

/// ORDERS 17001 — "Bestellung Geräteübernahmeangebot" (AWH V2.0 §4.2 Nr. 3).
pub const BESTELLUNG_PIDS: &[u32] = &[17001];

/// ORDERS 17009 — "Ankündigung Gerätewechselabsicht".
pub const STORNIERUNG_PIDS: &[u32] = &[17009];

/// QUOTES 15001 — "Angebot Geräteübernahme" (AWH V2.0 §4.2 Nr. 2).
pub const ANGEBOT_PID: Pruefidentifikator = Pruefidentifikator::const_new(15001);

/// ORDRSP 19001 — "Bestellbestätigung" (AWH V2.0 §4.2 Nr. 4, positive; EBD E_2011).
pub const BESTAETIGUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19001);

/// ORDRSP 19002 — "Ablehnung der Bestellung" (AWH V2.0 §4.2 Nr. 4, negative; EBD E_2011).
pub const ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19002);

/// Werktage for the Geräteübernahmeangebot (AWH V2.0 §4.2 Nr. 2).
pub const ANGEBOT_FRIST_WT: u32 = 4;

/// Werktage for the Bestellung against a standing Angebot (AWH V2.0 §4.2 Nr. 3).
pub const BESTELLUNG_FRIST_WT: u32 = 3;

/// Werktage for the Bestellbestätigung (AWH V2.0 §4.2 Nr. 4).
pub const BESTAETIGUNG_FRIST_WT: u32 = 2;

/// Deadline label for the ORDRSP response window (2 Werktage).
pub const ORDRSP_DEADLINE_LABEL: &str = "wim-gas-geraeteubernahme-ordrsp-deadline";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Gas Geräteübernahme workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasGeraeteubernahmeEvent {
    /// Phase 1: MSBN Anfrage received (ORDERS 17001/17002/17009).
    AnfrageReceived {
        /// ORDERS PID (17001, 17002 or 17009).
        pid: Pruefidentifikator,
        /// GLN of the incoming MSB (nMSB).
        incoming_msb: MarktpartnerCode,
        /// GLN of the grid operator (NB/aMSB).
        grid_operator: MarktpartnerCode,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// Physical device identifier.
        device_id: DeviceId,
        /// Document date from DTM segment.
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// EDIFACT ORDERS passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Phase 1: Angebot dispatched (QUOTES 15001) or the Anfrage refused.
    AnfrageOrdrspDispatched {
        /// `true` if an Angebot was made, `false` if the Anfrage was refused.
        positive: bool,
        /// Message reference of the dispatched ORDRSP.
        response_ref: MessageRef,
        /// Rejection reason text (only set when `positive = false`).
        reason: Option<String>,
    },
    /// Phase 2: MSBN Bestellung received (ORDERS 17001, AWH V2.0 §4.2 Nr. 3).
    BestellungReceived {
        /// ORDERS PID (17001).
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Phase 2: ORDRSP dispatched (19001 Bestellbestätigung / 19002 Ablehnung).
    BestellungOrdrspDispatched {
        /// `true` if confirmed (19001), `false` if rejected (19002).
        positive: bool,
        /// Message reference of the dispatched ORDRSP.
        response_ref: MessageRef,
        /// Rejection reason text (only set when `positive = false`).
        reason: Option<String>,
    },
    /// Physical device transfer confirmed; commissioning complete.
    Abgeschlossen {
        /// Physical device identifier confirmed at transfer.
        device_id: DeviceId,
    },
    /// Commissioning request cancelled by nMSB via Stornierung ORDERS.
    Storniert {
        /// PID of the Stornierung ORDERS (17009).
        stornierung_pid: Pruefidentifikator,
        /// EDIFACT message reference of the Stornierung.
        message_ref: MessageRef,
    },
    /// Process rejected (validation failure, negative ORDRSP, or deadline).
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline fired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GasGeraeteubernahmeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageReceived { .. } => "WimGasGeraeteubernahmeAnfrageReceived",
            Self::ValidationPassed { .. } => "WimGasGeraeteubernahmeValidationPassed",
            Self::AnfrageOrdrspDispatched { .. } => "WimGasGeraeteubernahmeAnfrageOrdrspDispatched",
            Self::BestellungReceived { .. } => "WimGasGeraeteubernahmeBestellungReceived",
            Self::BestellungOrdrspDispatched { .. } => {
                "WimGasGeraeteubernahmeBestellungOrdrspDispatched"
            }
            Self::Abgeschlossen { .. } => "WimGasGeraeteubernahmeAbgeschlossen",
            Self::Storniert { .. } => "WimGasGeraeteubernahmeStorniert",
            Self::Abgelehnt { .. } => "WimGasGeraeteubernahmeAbgelehnt",
            Self::DeadlineExpired { .. } => "WimGasGeraeteubernahmeDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured from the initial Anfrage ORDERS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasGeraeteubernahmeData {
    /// BDEW Prüfidentifikator (17001, 17002 or 17009).
    pub pid: Pruefidentifikator,
    /// GLN of the incoming Messstellenbetreiber (nMSB).
    pub incoming_msb: MarktpartnerCode,
    /// GLN of the grid operator (Netzbetreiber / aMSB).
    pub grid_operator: MarktpartnerCode,
    /// Messlokation EIC code.
    pub melo_id: MeLo,
    /// Physical device identifier.
    pub device_id: DeviceId,
    /// EDIFACT document date (YYYYMMDD).
    pub document_date: String,
}

/// State of a single WiM Gas Geräteübernahme process stream.
///
/// # Lifecycle
///
/// ```text
/// New → AnfrageReceived → ValidationPassed
///                       ↘ Abgelehnt (validation failed)
///       ValidationPassed → AngebotGesendet (positive Anfrage-ORDRSP)
///                        ↘ Abgelehnt (negative Anfrage-ORDRSP)
///       AngebotGesendet → BestellungReceived → Abgeschlossen (positive Bestellung-ORDRSP)
///                                            ↘ Abgelehnt (negative)
///       Any active state → Storniert (via nMSB Stornierung ORDERS)
///       Any non-terminal → Abgelehnt (deadline expired)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GasGeraeteubernahmeState {
    /// No events yet.
    #[default]
    New,
    /// Anfrage ORDERS received; awaiting validation result.
    AnfrageReceived(GasGeraeteubernahmeData),
    /// Validation passed; awaiting ORDRSP dispatch decision.
    ValidationPassed(GasGeraeteubernahmeData),
    /// Positive Anfrage-ORDRSP dispatched; awaiting Bestellung from nMSB.
    AngebotGesendet(GasGeraeteubernahmeData),
    /// Bestellung ORDERS received; awaiting final ORDRSP dispatch.
    BestellungReceived(GasGeraeteubernahmeData),
    /// Device transfer completed; commissioning successful.
    Abgeschlossen(GasGeraeteubernahmeData),
    /// Process cancelled by nMSB Stornierung.
    Storniert {
        /// Human-readable cancellation reason.
        reason: String,
    },
    /// Process rejected (validation, negative ORDRSP, or deadline).
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl GasGeraeteubernahmeState {
    /// Returns `true` if the process is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Abgeschlossen(_) | Self::Storniert { .. } | Self::Abgelehnt { .. }
        )
    }

    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageReceived(_) => "AnfrageReceived",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AngebotGesendet(_) => "AngebotGesendet",
            Self::BestellungReceived(_) => "BestellungReceived",
            Self::Abgeschlossen(_) => "Abgeschlossen",
            Self::Storniert { .. } => "Storniert",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Gas Geräteübernahme workflow.
#[derive(Clone)]
pub enum GasGeraeteubernahmeCommand {
    /// Phase 1: Inbound ORDERS 17001/17002 — Anfrage Geräteübernahmeangebot.
    ///
    /// Domain fields extracted and EDIFACT validation performed by the
    /// transport boundary **before** constructing this command.
    ReceiveAnfrage {
        /// ORDERS PID (17001 or 17002).
        pid: Pruefidentifikator,
        /// GLN of the message sender (nMSB).
        sender: MarktpartnerCode,
        /// GLN of the message receiver (NB/aMSB).
        receiver: MarktpartnerCode,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// Physical device identifier.
        device_id: DeviceId,
        /// Document date from the ORDERS DTM segment.
        document_date: String,
        /// EDIFACT message reference (UNH/BGM).
        message_ref: MessageRef,
        /// `true` if EDIFACT profile validation succeeded.
        validation_passed: bool,
        /// Validation error messages when `validation_passed = false`.
        validation_errors: Vec<String>,
    },
    /// Dispatch the Angebot (QUOTES 15001) or refuse the Anfrage.
    DispatchAnfrageOrdrsp {
        /// `true` to send an Angebot, `false` to refuse.
        positive: bool,
        /// Message reference assigned to the outbound ORDRSP.
        response_ref: MessageRef,
        /// Rejection reason (required when `positive = false`).
        reason: Option<String>,
    },
    /// Phase 2: Inbound ORDERS 17001 — Bestellung gegen das Angebot.
    ReceiveBestellung {
        /// ORDERS PID (17001).
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Dispatch ORDRSP for Phase 2 (19001 Bestellbestätigung / 19002 Ablehnung).
    DispatchBestellungOrdrsp {
        /// `true` to confirm (19001), `false` to reject (19002).
        positive: bool,
        /// Message reference assigned to the outbound ORDRSP.
        response_ref: MessageRef,
        /// Rejection reason (required when `positive = false`).
        reason: Option<String>,
    },
    /// Confirm that the physical device transfer is complete.
    ConfirmTransfer {
        /// Physical device identifier confirmed at transfer.
        device_id: DeviceId,
    },
    /// MSBN announces a Gerätewechselabsicht via ORDERS 17009.
    ReceiveStornierung {
        /// ORDERS PID (17009).
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for GasGeraeteubernahmeCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Gas Geräteübernahme workflow (WiM Gas AWH V2.0 §4.2).
///
/// Implements the two-phase BDEW WiM Gas commissioning-request process from the
/// **NB/aMSB perspective** — the receiving side of ORDERS messages.
pub struct WimGasGeraeteubernahmeWorkflow;

impl Workflow for WimGasGeraeteubernahmeWorkflow {
    type State = GasGeraeteubernahmeState;
    type Event = GasGeraeteubernahmeEvent;
    type Command = GasGeraeteubernahmeCommand;

    /// Deadline compensation for the ORDRSP response window (2 Werktage).
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        if deadline.label() == ORDRSP_DEADLINE_LABEL && !state.is_terminal() {
            Some(GasGeraeteubernahmeCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            })
        } else {
            None
        }
    }

    #[allow(clippy::too_many_lines)]
    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GasGeraeteubernahmeEvent::AnfrageReceived {
                pid,
                incoming_msb,
                grid_operator,
                melo_id,
                device_id,
                document_date,
                ..
            } => GasGeraeteubernahmeState::AnfrageReceived(GasGeraeteubernahmeData {
                pid: *pid,
                incoming_msb: incoming_msb.clone(),
                grid_operator: grid_operator.clone(),
                melo_id: melo_id.clone(),
                device_id: device_id.clone(),
                document_date: document_date.clone(),
            }),
            GasGeraeteubernahmeEvent::ValidationPassed { .. } => {
                if let GasGeraeteubernahmeState::AnfrageReceived(data) = state {
                    GasGeraeteubernahmeState::ValidationPassed(data)
                } else {
                    state
                }
            }
            GasGeraeteubernahmeEvent::AnfrageOrdrspDispatched {
                positive, reason, ..
            } => {
                if *positive {
                    match state {
                        GasGeraeteubernahmeState::ValidationPassed(data) => {
                            GasGeraeteubernahmeState::AngebotGesendet(data)
                        }
                        _ => state,
                    }
                } else {
                    GasGeraeteubernahmeState::Abgelehnt {
                        reason: reason
                            .clone()
                            .unwrap_or_else(|| "negative ORDRSP".to_owned()),
                    }
                }
            }
            GasGeraeteubernahmeEvent::BestellungReceived { .. } => {
                if let GasGeraeteubernahmeState::AngebotGesendet(data) = state {
                    GasGeraeteubernahmeState::BestellungReceived(data)
                } else {
                    state
                }
            }
            GasGeraeteubernahmeEvent::BestellungOrdrspDispatched {
                positive, reason, ..
            } => {
                if *positive {
                    state // remains BestellungReceived until ConfirmTransfer
                } else {
                    GasGeraeteubernahmeState::Abgelehnt {
                        reason: reason
                            .clone()
                            .unwrap_or_else(|| "negative Bestellung-ORDRSP".to_owned()),
                    }
                }
            }
            GasGeraeteubernahmeEvent::Abgeschlossen { device_id } => {
                if let GasGeraeteubernahmeState::BestellungReceived(mut data) = state {
                    data.device_id = device_id.clone();
                    GasGeraeteubernahmeState::Abgeschlossen(data)
                } else {
                    state
                }
            }
            GasGeraeteubernahmeEvent::Storniert {
                stornierung_pid, ..
            } => GasGeraeteubernahmeState::Storniert {
                reason: format!("Stornierung via PID {stornierung_pid}"),
            },
            GasGeraeteubernahmeEvent::Abgelehnt { reason } => GasGeraeteubernahmeState::Abgelehnt {
                reason: reason.clone(),
            },
            GasGeraeteubernahmeEvent::DeadlineExpired { label, .. } => match state {
                s if s.is_terminal() => s,
                _ => GasGeraeteubernahmeState::Abgelehnt {
                    reason: format!("deadline expired: {label}"),
                },
            },
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            GasGeraeteubernahmeCommand::ReceiveAnfrage {
                pid,
                sender,
                receiver,
                melo_id,
                device_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GasGeraeteubernahmeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !ANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a Gas Geräteübernahme-Anfrage PID (expected {:?})",
                        pid.as_u32(),
                        ANFRAGE_PIDS,
                    )));
                }
                // Clone before move for APERAK emission in the validation-failed path.
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![GasGeraeteubernahmeEvent::AnfrageReceived {
                    pid,
                    incoming_msb: sender,
                    grid_operator: receiver,
                    melo_id,
                    device_id,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(GasGeraeteubernahmeEvent::ValidationPassed { message_ref });
                    // APERAK BGM+312 (Anerkennungsmeldung) — APERAK AHB 1.0 §2.4.
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_mp_id.as_str(),
                            serde_json::json!({
                                "sender":        receiver_gln.as_str(),
                                "receiver":      sender_mp_id.as_str(),
                                "pid":           29001_u32,
                                "document_code": "312",
                            }),
                        )
                        .caused_by(1),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                } else {
                    let reason = validation_errors.join("; ");
                    events.push(GasGeraeteubernahmeEvent::Abgelehnt {
                        reason: reason.clone(),
                    });
                    // APERAK BGM+313 (Ablehnung) — APERAK AHB 1.0 §2.1.1.
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_mp_id.as_str(),
                            serde_json::json!({
                                "sender":     receiver_gln.as_str(),
                                "receiver":   sender_mp_id.as_str(),
                                "pid":        29001_u32,
                                "error_code": mako_engine::erc::codes::Z29,
                                "reason":     reason,
                            }),
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                }
            }

            GasGeraeteubernahmeCommand::DispatchAnfrageOrdrsp {
                positive,
                response_ref,
                reason,
            } => {
                if !matches!(state, GasGeraeteubernahmeState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.status_str(),
                    ));
                }
                Ok(vec![GasGeraeteubernahmeEvent::AnfrageOrdrspDispatched {
                    positive,
                    response_ref,
                    reason,
                }]
                .into())
            }

            GasGeraeteubernahmeCommand::ReceiveBestellung { pid, message_ref } => {
                if !matches!(state, GasGeraeteubernahmeState::AngebotGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AngebotGesendet",
                        state.status_str(),
                    ));
                }
                if !BESTELLUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a Gas Geräteübernahme-Bestellung PID (expected {:?})",
                        pid.as_u32(),
                        BESTELLUNG_PIDS,
                    )));
                }
                Ok(vec![GasGeraeteubernahmeEvent::BestellungReceived { pid, message_ref }].into())
            }

            GasGeraeteubernahmeCommand::DispatchBestellungOrdrsp {
                positive,
                response_ref,
                reason,
            } => {
                if !matches!(state, GasGeraeteubernahmeState::BestellungReceived(_)) {
                    return Err(WorkflowError::invalid_state(
                        "BestellungReceived",
                        state.status_str(),
                    ));
                }
                Ok(vec![GasGeraeteubernahmeEvent::BestellungOrdrspDispatched {
                    positive,
                    response_ref,
                    reason,
                }]
                .into())
            }

            GasGeraeteubernahmeCommand::ConfirmTransfer { device_id } => {
                if !matches!(state, GasGeraeteubernahmeState::BestellungReceived(_)) {
                    return Err(WorkflowError::invalid_state(
                        "BestellungReceived",
                        state.status_str(),
                    ));
                }
                Ok(vec![GasGeraeteubernahmeEvent::Abgeschlossen { device_id }].into())
            }

            GasGeraeteubernahmeCommand::ReceiveStornierung { pid, message_ref } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !STORNIERUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a Gas Geräteübernahme-Stornierung PID (expected {:?})",
                        pid.as_u32(),
                        STORNIERUNG_PIDS,
                    )));
                }
                Ok(vec![GasGeraeteubernahmeEvent::Storniert {
                    stornierung_pid: pid,
                    message_ref,
                }]
                .into())
            }

            GasGeraeteubernahmeCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GasGeraeteubernahmeEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single WiM Gas Geräteübernahme process stream.
#[derive(Debug)]
pub enum GasGeraeteubernahmeRecord {
    /// No `AnfrageReceived` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `AnfrageReceived` event applied; process fields now available.
    Active {
        /// Current lifecycle stage.
        status: &'static str,
        /// Messlokation EIC code from the Anfrage.
        melo_id: MeLo,
        /// GLN of the incoming MSB (nMSB).
        incoming_msb: MarktpartnerCode,
        /// GLN of the grid operator (NB/aMSB).
        grid_operator: MarktpartnerCode,
        /// Physical device identifier (updated on `Abgeschlossen`).
        device_id: DeviceId,
        /// ORDERS PID that initiated the process.
        pid: Pruefidentifikator,
        /// Total events applied.
        event_count: usize,
    },
}

impl GasGeraeteubernahmeRecord {
    /// Current lifecycle status label, suitable for logging and serialisation.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::New { .. } => "New",
            Self::Active { status, .. } => status,
        }
    }

    /// Total events applied to this stream.
    #[must_use]
    pub fn event_count(&self) -> usize {
        match self {
            Self::New { event_count } | Self::Active { event_count, .. } => *event_count,
        }
    }
}

impl Default for GasGeraeteubernahmeRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model tracking WiM Gas Geräteübernahme streams.
#[derive(Debug, Default)]
pub struct GasGeraeteubernahmeProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, GasGeraeteubernahmeRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for GasGeraeteubernahmeProjection {
    fn name(&self) -> &'static str {
        "GasGeraeteubernahmeProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<GasGeraeteubernahmeEvent>() else {
            return;
        };

        match record {
            GasGeraeteubernahmeRecord::New { event_count }
            | GasGeraeteubernahmeRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            GasGeraeteubernahmeEvent::AnfrageReceived {
                pid,
                incoming_msb,
                grid_operator,
                melo_id,
                device_id,
                ..
            } => {
                let count = record.event_count();
                *record = GasGeraeteubernahmeRecord::Active {
                    status: "AnfrageReceived",
                    pid,
                    incoming_msb,
                    grid_operator,
                    melo_id,
                    device_id,
                    event_count: count,
                };
            }
            GasGeraeteubernahmeEvent::ValidationPassed { .. } => {
                if let GasGeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            GasGeraeteubernahmeEvent::AnfrageOrdrspDispatched { positive, .. } => {
                if let GasGeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = if positive {
                        "AngebotGesendet"
                    } else {
                        "Abgelehnt"
                    };
                }
            }
            GasGeraeteubernahmeEvent::BestellungReceived { .. } => {
                if let GasGeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "BestellungReceived";
                }
            }
            GasGeraeteubernahmeEvent::BestellungOrdrspDispatched { positive, .. } => {
                if !positive && let GasGeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "Abgelehnt";
                }
            }
            GasGeraeteubernahmeEvent::Abgeschlossen { device_id } => {
                if let GasGeraeteubernahmeRecord::Active {
                    status,
                    device_id: d,
                    ..
                } = record
                {
                    *status = "Abgeschlossen";
                    *d = device_id;
                }
            }
            GasGeraeteubernahmeEvent::Storniert { .. } => {
                if let GasGeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "Storniert";
                }
            }
            GasGeraeteubernahmeEvent::Abgelehnt { .. }
            | GasGeraeteubernahmeEvent::DeadlineExpired { .. } => {
                if let GasGeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "Abgelehnt";
                }
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::types::MessageRef;

    fn anfrage_cmd(pid: u32) -> GasGeraeteubernahmeCommand {
        GasGeraeteubernahmeCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::new(pid).unwrap(),
            sender: MarktpartnerCode::new("9870057000004"),
            receiver: MarktpartnerCode::new("9880357000004"),
            melo_id: MeLo::new("DE00056789012"),
            device_id: DeviceId::new("BK-G4-1234567890"),
            document_date: "20260101".to_owned(),
            message_ref: MessageRef::new("MSG-ORDERS-GAS-001"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    #[test]
    fn happy_path_phase1_to_phase2_to_abgeschlossen() {
        let state = GasGeraeteubernahmeState::default();
        let events = WimGasGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(17001))
            .expect("Anfrage 17001 must succeed");
        assert_eq!(events.len(), 2); // AnfrageReceived + ValidationPassed
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        assert!(matches!(
            state,
            GasGeraeteubernahmeState::ValidationPassed(_)
        ));

        let events = WimGasGeraeteubernahmeWorkflow::handle(
            &state,
            GasGeraeteubernahmeCommand::DispatchAnfrageOrdrsp {
                positive: true,
                response_ref: MessageRef::new("MSG-QUOTES-001"),
                reason: None,
            },
        )
        .expect("DispatchAnfrageOrdrsp must succeed");
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        assert!(matches!(
            state,
            GasGeraeteubernahmeState::AngebotGesendet(_)
        ));

        let events = WimGasGeraeteubernahmeWorkflow::handle(
            &state,
            GasGeraeteubernahmeCommand::ReceiveBestellung {
                pid: Pruefidentifikator::new(17001).unwrap(),
                message_ref: MessageRef::new("MSG-ORDERS-GAS-002"),
            },
        )
        .expect("ReceiveBestellung must succeed");
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        assert!(matches!(
            state,
            GasGeraeteubernahmeState::BestellungReceived(_)
        ));

        let events = WimGasGeraeteubernahmeWorkflow::handle(
            &state,
            GasGeraeteubernahmeCommand::DispatchBestellungOrdrsp {
                positive: true,
                response_ref: MessageRef::new("MSG-ORDRSP-002"),
                reason: None,
            },
        )
        .expect("DispatchBestellungOrdrsp must succeed");
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        assert!(matches!(
            state,
            GasGeraeteubernahmeState::BestellungReceived(_)
        ));

        let events = WimGasGeraeteubernahmeWorkflow::handle(
            &state,
            GasGeraeteubernahmeCommand::ConfirmTransfer {
                device_id: DeviceId::new("NEW-BK-G4-9999999"),
            },
        )
        .expect("ConfirmTransfer must succeed");
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GasGeraeteubernahmeState::Abgeschlossen(_)));
    }

    #[test]
    fn negative_anfrage_ordrsp_rejects() {
        let state = GasGeraeteubernahmeState::default();
        let events = WimGasGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(17001)).unwrap();
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        let events = WimGasGeraeteubernahmeWorkflow::handle(
            &state,
            GasGeraeteubernahmeCommand::DispatchAnfrageOrdrsp {
                positive: false,
                response_ref: MessageRef::new("MSG-ORDRSP-NEG"),
                reason: Some("MeLo nicht bekannt".to_owned()),
            },
        )
        .unwrap();
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GasGeraeteubernahmeState::Abgelehnt { .. }));
    }

    #[test]
    fn validation_failure_rejects() {
        let state = GasGeraeteubernahmeState::default();
        let events = WimGasGeraeteubernahmeWorkflow::handle(
            &state,
            GasGeraeteubernahmeCommand::ReceiveAnfrage {
                pid: Pruefidentifikator::new(17001).unwrap(),
                sender: MarktpartnerCode::new("9870123456789"),
                receiver: MarktpartnerCode::new("9880987654321"),
                melo_id: MeLo::new("DE00011111111"),
                device_id: DeviceId::new("BK-G4-001"),
                document_date: "20260101".to_owned(),
                message_ref: MessageRef::new("MSG-001"),
                validation_passed: false,
                validation_errors: vec!["mandatory segment missing".to_owned()],
            },
        )
        .unwrap();
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GasGeraeteubernahmeState::Abgelehnt { .. }));
    }

    #[test]
    fn stornierung_from_active_transitions_to_storniert() {
        let state = GasGeraeteubernahmeState::default();
        let events = WimGasGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(17001)).unwrap();
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        let events = WimGasGeraeteubernahmeWorkflow::handle(
            &state,
            GasGeraeteubernahmeCommand::ReceiveStornierung {
                pid: Pruefidentifikator::new(17009).unwrap(),
                message_ref: MessageRef::new("MSG-STORNO-001"),
            },
        )
        .unwrap();
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GasGeraeteubernahmeState::Storniert { .. }));
    }

    #[test]
    fn deadline_on_active_rejects() {
        let state = GasGeraeteubernahmeState::default();
        let events = WimGasGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(17001)).unwrap();
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        let events = WimGasGeraeteubernahmeWorkflow::handle(
            &state,
            GasGeraeteubernahmeCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ORDRSP_DEADLINE_LABEL.into(),
            },
        )
        .unwrap();
        let state = events
            .iter()
            .fold(state, WimGasGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GasGeraeteubernahmeState::Abgelehnt { .. }));
    }

    #[test]
    fn deadline_on_terminal_is_noop() {
        let terminal = GasGeraeteubernahmeState::Abgelehnt {
            reason: "test".to_owned(),
        };
        let events = WimGasGeraeteubernahmeWorkflow::handle(
            &terminal,
            GasGeraeteubernahmeCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: "late-deadline".into(),
            },
        )
        .unwrap();
        assert!(events.is_empty(), "deadline on terminal must be a no-op");
    }

    #[test]
    fn all_anfrage_pids_accepted() {
        for &pid in ANFRAGE_PIDS {
            let state = GasGeraeteubernahmeState::default();
            assert!(
                WimGasGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(pid)).is_ok(),
                "PID {pid} must be accepted",
            );
        }
    }

    #[test]
    fn wrong_pid_family_rejected() {
        let state = GasGeraeteubernahmeState::default();
        let result = WimGasGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(44042));
        assert!(
            result.is_err(),
            "Gas MSB-Wechsel PID must be rejected by Geräteübernahme"
        );
    }
}
