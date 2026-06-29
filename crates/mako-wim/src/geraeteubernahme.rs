//! WiM Geräteübernahme — device commissioning request/response (PIDs 17001–17011).
//!
//! Models the **NB/aMSB-side** perspective of the BDEW WiM process by which a
//! new Messstellenbetreiber (nMSB) requests takeover of metering equipment at a
//! Messlokation.
//!
//! # Two-phase process
//!
//! ## Phase 1: Anfrage Geräteübernahmeangebot
//!
//! ```text
//! nMSB → ORDERS 17001 (Anfrage) ───────────────────────────────────────── NB/aMSB
//!                                                                              │
//! nMSB ← ORDRSP 17003 (Bestätigung) or 17004 (Ablehnung) ←── 5 Werktage ───────┘
//! ```
//!
//! ## Phase 2: Bestellung Geräteübernahme (only after Phase 1 accepted)
//!
//! ```text
//! nMSB → ORDERS 17005 (Bestellung) ───────────────────────────────────── NB/aMSB
//!                                                                              │
//! nMSB ← ORDRSP 17007 (Bestätigung) or 17008 (Ablehnung) ←── 5 Werktage ───────┘
//! ```
//!
//! ## Stornierungen
//!
//! | Stornierung-PID | Cancels | Response PIDs |
//! |---|---|---|
//! | 17009 | ORDERS 17001 (Anfrage) | 17010 (Bestätigung) / 17011 (Ablehnung) |
//!
//! # Regulatory basis
//!
//! - **BDEW WiM AHB** — Bestellung Geräteübernahmeangebot
//! - **BNetzA BK6-18-032** — 5 Werktage Frist for ORDRSP
//! - **MsbG** — governing smart-meter rollout obligations

use std::collections::HashMap;

use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    projection::Projection,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constants ─────────────────────────────────────────────────────────────

/// All ORDERS PIDs for the Geräteübernahme process family (WiM Strom Teil 1/Teil 2).
///
/// **PIDs removed (WiM Gas, belong to `mako-wim-gas` per `docs/pid-reference.md`):**
/// - 17001 (Bestellung Geräteübernahmeangebot — WiM Gas)
/// - 17002 (Weiterverpflichtung — WiM Gas)
/// - 17009 (Anzeige Gerätewechselabsicht — WiM Gas)
pub const GERAETEUBERNAHME_PIDS: &[u32] = &[
    17005, // Bestellung Rechnungsabwicklung MSB über LF (WiM Strom Teil 1)
    17011, // Bestellung Angebot Änderung Technik (WiM Strom Teil 1)
];

/// Anfrage PIDs — trigger a new `WimGeraeteubernahmeWorkflow` process.
///
/// **PIDs 17001/17002 were removed** — they belong to WiM Gas (`mako-wim-gas`).
pub const ANFRAGE_PIDS: &[u32] = &[];

/// All ORDERS PIDs that route to `ReceiveAnfrage` across all domains using this workflow.
///
/// WiM Strom `ANFRAGE_PIDS` is empty; WiM Gas PIDs 17001/17002 are routed here by
/// `WimGasModule`. This union set is used by the `handle()` PID guard.
const ANFRAGE_PIDS_ALL: &[u32] = &[17001, 17002];

/// Bestellung PIDs — continue an existing process (confirm the takeover offer).
pub const BESTELLUNG_PIDS: &[u32] = &[17005];

/// Stornierung PIDs — cancel an in-progress request or bestellung.
///
/// **PID 17009 was removed** — it belongs to WiM Gas (`mako-wim-gas`).
pub const STORNIERUNG_PIDS: &[u32] = &[17011];

/// Deadline label for the ORDRSP response window (5 Werktage, BK6-18-032).
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     received_at, 5, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., ORDRSP_DEADLINE_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const ORDRSP_DEADLINE_LABEL: &str = "wim-geraeteubernahme-ordrsp-deadline";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Geräteübernahme workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeraeteubernahmeEvent {
    /// Phase 1: nMSB Anfrage Geräteübernahmeangebot received (ORDERS 17001/17002).
    AnfrageReceived {
        /// ORDERS PID (17001 or 17002).
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
    /// Phase 1: ORDRSP dispatched (17003 positive / 17004 negative).
    AnfrageOrdrspDispatched {
        /// `true` if the offer was accepted (17003), `false` if rejected (17004).
        positive: bool,
        /// Message reference of the dispatched ORDRSP.
        response_ref: MessageRef,
        /// Rejection reason text (only set when `positive = false`).
        reason: Option<String>,
    },
    /// Phase 2: nMSB Bestellung Geräteübernahme received (ORDERS 17005).
    BestellungReceived {
        /// ORDERS PID (17005).
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Phase 2: ORDRSP dispatched (17007 positive / 17008 negative).
    BestellungOrdrspDispatched {
        /// `true` if transfer confirmed (17007), `false` if rejected (17008).
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
        /// PID of the Stornierung ORDERS (17009 or 17011).
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

impl EventPayload for GeraeteubernahmeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageReceived { .. } => "WimGeraeteubernahmeAnfrageReceived",
            Self::ValidationPassed { .. } => "WimGeraeteubernahmeValidationPassed",
            Self::AnfrageOrdrspDispatched { .. } => "WimGeraeteubernahmeAnfrageOrdrspDispatched",
            Self::BestellungReceived { .. } => "WimGeraeteubernahmeBestellungReceived",
            Self::BestellungOrdrspDispatched { .. } => {
                "WimGeraeteubernahmeBestellungOrdrspDispatched"
            }
            Self::Abgeschlossen { .. } => "WimGeraeteubernahmeAbgeschlossen",
            Self::Storniert { .. } => "WimGeraeteubernahmeStorniert",
            Self::Abgelehnt { .. } => "WimGeraeteubernahmeAbgelehnt",
            Self::DeadlineExpired { .. } => "WimGeraeteubernahmeDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured from the initial Anfrage ORDERS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeraeteubernahmeData {
    /// BDEW Prüfidentifikator (17001–17011).
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

/// State of a single WiM Geräteübernahme process stream.
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
pub enum GeraeteubernahmeState {
    /// No events yet.
    New,
    /// Anfrage ORDERS received; awaiting validation result.
    AnfrageReceived(GeraeteubernahmeData),
    /// Validation passed; awaiting ORDRSP dispatch decision.
    ValidationPassed(GeraeteubernahmeData),
    /// Positive Anfrage-ORDRSP dispatched; awaiting Bestellung from nMSB.
    AngebotGesendet(GeraeteubernahmeData),
    /// Bestellung ORDERS received; awaiting final ORDRSP dispatch.
    BestellungReceived(GeraeteubernahmeData),
    /// Device transfer completed; commissioning successful.
    Abgeschlossen(GeraeteubernahmeData),
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

impl Default for GeraeteubernahmeState {
    fn default() -> Self {
        Self::New
    }
}

impl GeraeteubernahmeState {
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

/// Commands for the WiM Geräteübernahme workflow.
#[derive(Clone)]
pub enum GeraeteubernahmeCommand {
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
    /// Dispatch ORDRSP for Phase 1 (PID 17003 positive or 17004 negative).
    ///
    /// **BNetzA BK6-18-032**: ORDRSP must be sent within **5 Werktage** of
    /// receiving the Anfrage. Use `fristen::add_werktage(5, BdewMaKo)`.
    DispatchAnfrageOrdrsp {
        /// `true` to accept (17003), `false` to reject (17004).
        positive: bool,
        /// Message reference assigned to the outbound ORDRSP.
        response_ref: MessageRef,
        /// Rejection reason (required when `positive = false`).
        reason: Option<String>,
    },
    /// Phase 2: Inbound ORDERS 17005 — Bestellung Geräteübernahme.
    ///
    /// Only valid after a positive `AnfrageOrdrspDispatched`.
    ReceiveBestellung {
        /// ORDERS PID (17005).
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Dispatch ORDRSP for Phase 2 (PID 17007 positive or 17008 negative).
    DispatchBestellungOrdrsp {
        /// `true` to confirm transfer (17007), `false` to reject (17008).
        positive: bool,
        /// Message reference assigned to the outbound ORDRSP.
        response_ref: MessageRef,
        /// Rejection reason (required when `positive = false`).
        reason: Option<String>,
    },
    /// Confirm that the physical device transfer is complete.
    ///
    /// Only valid after a positive `BestellungOrdrspDispatched`.
    ConfirmTransfer {
        /// Physical device identifier confirmed at transfer.
        device_id: DeviceId,
    },
    /// nMSB cancels the request via Stornierung ORDERS (17009 or 17011).
    ReceiveStornierung {
        /// Stornierung ORDERS PID (17009 or 17011).
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

impl CommandPayload for GeraeteubernahmeCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Geräteübernahme workflow (PIDs 17001–17011).
///
/// Implements the two-phase BDEW WiM commissioning request process from the
/// **NB/aMSB perspective** — the receiving side of ORDERS messages.
pub struct WimGeraeteubernahmeWorkflow;

impl Workflow for WimGeraeteubernahmeWorkflow {
    type State = GeraeteubernahmeState;
    type Event = GeraeteubernahmeEvent;
    type Command = GeraeteubernahmeCommand;

    /// Deadline compensation for the WiM Geräteübernahme 5-Werktage ORDRSP window.
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"wim-geraeteubernahme-ordrsp-deadline"` | any non-terminal | `TimeoutExpired` | BK6-18-032 — 5 Werktage ORDRSP Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        if deadline.label() == ORDRSP_DEADLINE_LABEL && !state.is_terminal() {
            Some(GeraeteubernahmeCommand::TimeoutExpired {
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
            GeraeteubernahmeEvent::AnfrageReceived {
                pid,
                incoming_msb,
                grid_operator,
                melo_id,
                device_id,
                document_date,
                ..
            } => GeraeteubernahmeState::AnfrageReceived(GeraeteubernahmeData {
                pid: *pid,
                incoming_msb: incoming_msb.clone(),
                grid_operator: grid_operator.clone(),
                melo_id: melo_id.clone(),
                device_id: device_id.clone(),
                document_date: document_date.clone(),
            }),
            GeraeteubernahmeEvent::ValidationPassed { .. } => {
                if let GeraeteubernahmeState::AnfrageReceived(data) = state {
                    GeraeteubernahmeState::ValidationPassed(data)
                } else {
                    state
                }
            }
            GeraeteubernahmeEvent::AnfrageOrdrspDispatched {
                positive, reason, ..
            } => {
                if *positive {
                    match state {
                        GeraeteubernahmeState::ValidationPassed(data) => {
                            GeraeteubernahmeState::AngebotGesendet(data)
                        }
                        _ => state,
                    }
                } else {
                    GeraeteubernahmeState::Abgelehnt {
                        reason: reason
                            .clone()
                            .unwrap_or_else(|| "negative ORDRSP".to_owned()),
                    }
                }
            }
            GeraeteubernahmeEvent::BestellungReceived { .. } => {
                if let GeraeteubernahmeState::AngebotGesendet(data) = state {
                    GeraeteubernahmeState::BestellungReceived(data)
                } else {
                    state
                }
            }
            GeraeteubernahmeEvent::BestellungOrdrspDispatched {
                positive, reason, ..
            } => {
                if *positive {
                    state // remains BestellungReceived until ConfirmTransfer
                } else {
                    GeraeteubernahmeState::Abgelehnt {
                        reason: reason
                            .clone()
                            .unwrap_or_else(|| "negative Bestellung-ORDRSP".to_owned()),
                    }
                }
            }
            GeraeteubernahmeEvent::Abgeschlossen { device_id } => {
                if let GeraeteubernahmeState::BestellungReceived(mut data) = state {
                    data.device_id = device_id.clone();
                    GeraeteubernahmeState::Abgeschlossen(data)
                } else {
                    state
                }
            }
            GeraeteubernahmeEvent::Storniert {
                stornierung_pid, ..
            } => GeraeteubernahmeState::Storniert {
                reason: format!("Stornierung via PID {stornierung_pid}"),
            },
            GeraeteubernahmeEvent::Abgelehnt { reason } => GeraeteubernahmeState::Abgelehnt {
                reason: reason.clone(),
            },
            GeraeteubernahmeEvent::DeadlineExpired { label, .. } => match state {
                s if s.is_terminal() => s,
                _ => GeraeteubernahmeState::Abgelehnt {
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
            GeraeteubernahmeCommand::ReceiveAnfrage {
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
                if !matches!(state, GeraeteubernahmeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                // PID guard — must be a known Anfrage PID from any domain using this workflow.
                if !ANFRAGE_PIDS_ALL.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a Geräteübernahme-Anfrage PID (expected {:?})",
                        pid.as_u32(),
                        ANFRAGE_PIDS_ALL,
                    )));
                }
                let mut events = vec![GeraeteubernahmeEvent::AnfrageReceived {
                    pid,
                    incoming_msb: sender,
                    grid_operator: receiver,
                    melo_id,
                    device_id,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(GeraeteubernahmeEvent::ValidationPassed { message_ref });
                } else {
                    events.push(GeraeteubernahmeEvent::Abgelehnt {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            GeraeteubernahmeCommand::DispatchAnfrageOrdrsp {
                positive,
                response_ref,
                reason,
            } => {
                if !matches!(state, GeraeteubernahmeState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.status_str(),
                    ));
                }
                Ok(vec![GeraeteubernahmeEvent::AnfrageOrdrspDispatched {
                    positive,
                    response_ref,
                    reason,
                }]
                .into())
            }

            GeraeteubernahmeCommand::ReceiveBestellung { pid, message_ref } => {
                if !matches!(state, GeraeteubernahmeState::AngebotGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AngebotGesendet",
                        state.status_str(),
                    ));
                }
                if !BESTELLUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a Geräteübernahme-Bestellung PID (expected {:?})",
                        pid.as_u32(),
                        BESTELLUNG_PIDS,
                    )));
                }
                Ok(vec![GeraeteubernahmeEvent::BestellungReceived { pid, message_ref }].into())
            }

            GeraeteubernahmeCommand::DispatchBestellungOrdrsp {
                positive,
                response_ref,
                reason,
            } => {
                if !matches!(state, GeraeteubernahmeState::BestellungReceived(_)) {
                    return Err(WorkflowError::invalid_state(
                        "BestellungReceived",
                        state.status_str(),
                    ));
                }
                Ok(vec![GeraeteubernahmeEvent::BestellungOrdrspDispatched {
                    positive,
                    response_ref,
                    reason,
                }]
                .into())
            }

            GeraeteubernahmeCommand::ConfirmTransfer { device_id } => {
                // Must be in BestellungReceived AND a positive ORDRSP must have
                // been dispatched (encoded in state — no separate "PositiveOrdrspSent"
                // variant to keep the state machine lean).
                if !matches!(state, GeraeteubernahmeState::BestellungReceived(_)) {
                    return Err(WorkflowError::invalid_state(
                        "BestellungReceived",
                        state.status_str(),
                    ));
                }
                Ok(vec![GeraeteubernahmeEvent::Abgeschlossen { device_id }].into())
            }

            GeraeteubernahmeCommand::ReceiveStornierung { pid, message_ref } => {
                if state.is_terminal() {
                    // Stornierung for an already-terminal process is a no-op;
                    // transport layer should still send a Bestätigung-ORDRSP.
                    return Ok(WorkflowOutput::events(vec![]));
                }
                if !STORNIERUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a Geräteübernahme-Stornierung PID (expected {:?})",
                        pid.as_u32(),
                        STORNIERUNG_PIDS,
                    )));
                }
                Ok(vec![GeraeteubernahmeEvent::Storniert {
                    stornierung_pid: pid,
                    message_ref,
                }]
                .into())
            }

            GeraeteubernahmeCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GeraeteubernahmeEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single WiM Geräteübernahme process stream.
///
/// Uses a type-state design so field access never requires `Option::unwrap`:
/// the `Active` variant carries all domain fields that are structurally
/// guaranteed to exist once the process moves past `New`.
#[derive(Debug)]
pub enum GeraeteubernahmeRecord {
    /// No `AnfrageReceived` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `AnfrageReceived` event applied; process fields now available.
    Active {
        /// Current lifecycle stage (e.g. "AnfrageReceived", "Abgeschlossen").
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

impl GeraeteubernahmeRecord {
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

    /// Domain data for this record if it has been initiated, or `None` if `New`.
    #[must_use]
    pub fn active_data(&self) -> Option<GeraeteubernahmeRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                pid,
                ..
            } => Some(GeraeteubernahmeRecordData {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                pid,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` [`GeraeteubernahmeRecord`].
#[derive(Debug, Clone, Copy)]
pub struct GeraeteubernahmeRecordData<'a> {
    /// Messlokation EIC code from the Anfrage.
    pub melo_id: &'a MeLo,
    /// GLN of the incoming MSB (nMSB).
    pub incoming_msb: &'a MarktpartnerCode,
    /// GLN of the grid operator (NB/aMSB).
    pub grid_operator: &'a MarktpartnerCode,
    /// Physical device identifier.
    pub device_id: &'a DeviceId,
    /// ORDERS PID that initiated the process.
    pub pid: &'a Pruefidentifikator,
}

impl Default for GeraeteubernahmeRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model tracking WiM Geräteübernahme streams.
/// Feed via [`mako_engine::projection::ProjectionRunner`].
#[derive(Debug, Default)]
pub struct GeraeteubernahmeProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, GeraeteubernahmeRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for GeraeteubernahmeProjection {
    fn name(&self) -> &'static str {
        "GeraeteubernahmeProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<GeraeteubernahmeEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            GeraeteubernahmeRecord::New { event_count }
            | GeraeteubernahmeRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            GeraeteubernahmeEvent::AnfrageReceived {
                pid,
                incoming_msb,
                grid_operator,
                melo_id,
                device_id,
                ..
            } => {
                let count = record.event_count();
                *record = GeraeteubernahmeRecord::Active {
                    status: "AnfrageReceived",
                    pid,
                    incoming_msb,
                    grid_operator,
                    melo_id,
                    device_id,
                    event_count: count,
                };
            }
            GeraeteubernahmeEvent::ValidationPassed { .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            GeraeteubernahmeEvent::AnfrageOrdrspDispatched { positive, .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = if positive {
                        "AngebotGesendet"
                    } else {
                        "Abgelehnt"
                    };
                }
            }
            GeraeteubernahmeEvent::BestellungReceived { .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "BestellungReceived";
                }
            }
            GeraeteubernahmeEvent::BestellungOrdrspDispatched { positive, .. } => {
                if !positive {
                    if let GeraeteubernahmeRecord::Active { status, .. } = record {
                        *status = "Abgelehnt";
                    }
                }
            }
            GeraeteubernahmeEvent::Abgeschlossen { device_id } => {
                if let GeraeteubernahmeRecord::Active {
                    status,
                    device_id: d,
                    ..
                } = record
                {
                    *status = "Abgeschlossen";
                    *d = device_id;
                }
            }
            GeraeteubernahmeEvent::Storniert { .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "Storniert";
                }
            }
            GeraeteubernahmeEvent::Abgelehnt { .. }
            | GeraeteubernahmeEvent::DeadlineExpired { .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
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

    // Helper: build a minimal ReceiveAnfrage command with the given PID.
    // NOTE: WiM Strom has no ANFRAGE_PIDS (they moved to WiM Gas).
    // These test helpers use WiM Gas PIDs (17001) directly to exercise
    // the AnfrageReceived workflow path, which is still valid workflow logic.
    fn anfrage_cmd(pid: u32) -> GeraeteubernahmeCommand {
        GeraeteubernahmeCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::new(pid).unwrap(),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            melo_id: MeLo::new("DE00056789012"),
            device_id: DeviceId::new("EHZ-1234567890"),
            document_date: "20260101".to_owned(),
            message_ref: MessageRef::new("MSG-ORDERS-001"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    #[test]
    fn happy_path_phase1_to_phase2_to_abgeschlossen() {
        // Phase 1 (WiM Gas Anfrage path — PID 17001) still exercises the workflow
        // logic even though WiM Gas PIDs are routed by WimGasModule in mako-wim-gas.
        // WiM Strom ANFRAGE_PIDS is empty; the Anfrage path is only used from Gas.
        let state = GeraeteubernahmeState::default();

        // Phase 1: Anfrage received (WiM Gas PID 17001 — bypasses ANFRAGE_PIDS guard in tests)
        let events = WimGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(17001))
            .expect("Anfrage 17001 must succeed — bypassing PID guard for test");
        assert_eq!(events.len(), 2); // AnfrageReceived + ValidationPassed
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GeraeteubernahmeState::ValidationPassed(_)));

        // Phase 1: Dispatch positive ORDRSP
        let events = WimGeraeteubernahmeWorkflow::handle(
            &state,
            GeraeteubernahmeCommand::DispatchAnfrageOrdrsp {
                positive: true,
                response_ref: MessageRef::new("MSG-ORDRSP-001"),
                reason: None,
            },
        )
        .expect("DispatchAnfrageOrdrsp must succeed");
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GeraeteubernahmeState::AngebotGesendet(_)));

        // Phase 2: Bestellung received
        let events = WimGeraeteubernahmeWorkflow::handle(
            &state,
            GeraeteubernahmeCommand::ReceiveBestellung {
                pid: Pruefidentifikator::new(17005).unwrap(),
                message_ref: MessageRef::new("MSG-ORDERS-002"),
            },
        )
        .expect("ReceiveBestellung must succeed");
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        assert!(matches!(
            state,
            GeraeteubernahmeState::BestellungReceived(_)
        ));

        // Phase 2: Dispatch positive ORDRSP
        let events = WimGeraeteubernahmeWorkflow::handle(
            &state,
            GeraeteubernahmeCommand::DispatchBestellungOrdrsp {
                positive: true,
                response_ref: MessageRef::new("MSG-ORDRSP-002"),
                reason: None,
            },
        )
        .expect("DispatchBestellungOrdrsp must succeed");
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        // State remains BestellungReceived — ConfirmTransfer finalises it
        assert!(matches!(
            state,
            GeraeteubernahmeState::BestellungReceived(_)
        ));

        // Confirm physical transfer
        let events = WimGeraeteubernahmeWorkflow::handle(
            &state,
            GeraeteubernahmeCommand::ConfirmTransfer {
                device_id: DeviceId::new("NEW-EHZ-9999999"),
            },
        )
        .expect("ConfirmTransfer must succeed");
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GeraeteubernahmeState::Abgeschlossen(_)));
    }

    #[test]
    fn negative_anfrage_ordrsp_rejects() {
        let state = GeraeteubernahmeState::default();
        let events = WimGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(17001)).unwrap();
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        // ValidationPassed → dispatch negative
        let events = WimGeraeteubernahmeWorkflow::handle(
            &state,
            GeraeteubernahmeCommand::DispatchAnfrageOrdrsp {
                positive: false,
                response_ref: MessageRef::new("MSG-ORDRSP-NEG"),
                reason: Some("MeLo nicht bekannt".to_owned()),
            },
        )
        .unwrap();
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GeraeteubernahmeState::Abgelehnt { .. }));
    }

    #[test]
    fn validation_failure_rejects() {
        // WiM Gas PID 17001 directly — bypasses ANFRAGE_PIDS guard.
        let state = GeraeteubernahmeState::default();
        let events = WimGeraeteubernahmeWorkflow::handle(
            &state,
            GeraeteubernahmeCommand::ReceiveAnfrage {
                pid: Pruefidentifikator::new(17001).unwrap(),
                sender: MarktpartnerCode::new("9900123456789"),
                receiver: MarktpartnerCode::new("9900987654321"),
                melo_id: MeLo::new("DE00011111111"),
                device_id: DeviceId::new("EHZ-001"),
                document_date: "20260101".to_owned(),
                message_ref: MessageRef::new("MSG-001"),
                validation_passed: false,
                validation_errors: vec!["mandatory segment missing".to_owned()],
            },
        )
        .unwrap();
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GeraeteubernahmeState::Abgelehnt { .. }));
    }

    #[test]
    fn stornierung_from_active_transitions_to_storniert() {
        let state = GeraeteubernahmeState::default();
        let events = WimGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(17001)).unwrap();
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        // ValidationPassed → Storniert
        let events = WimGeraeteubernahmeWorkflow::handle(
            &state,
            GeraeteubernahmeCommand::ReceiveStornierung {
                pid: Pruefidentifikator::new(17011).unwrap(), // WiM Strom Teil 1 Stornierung
                message_ref: MessageRef::new("MSG-STORNO-001"),
            },
        )
        .unwrap();
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GeraeteubernahmeState::Storniert { .. }));
    }

    #[test]
    fn deadline_on_active_rejects() {
        let state = GeraeteubernahmeState::default();
        let events = WimGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(17001)).unwrap();
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        let events = WimGeraeteubernahmeWorkflow::handle(
            &state,
            GeraeteubernahmeCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: "wim-geraeteubernahme-ordrsp-deadline".into(),
            },
        )
        .unwrap();
        let state = events
            .iter()
            .fold(state, WimGeraeteubernahmeWorkflow::apply);
        assert!(matches!(state, GeraeteubernahmeState::Abgelehnt { .. }));
    }

    #[test]
    fn deadline_on_terminal_is_noop() {
        let terminal = GeraeteubernahmeState::Abgelehnt {
            reason: "test".to_owned(),
        };
        let events = WimGeraeteubernahmeWorkflow::handle(
            &terminal,
            GeraeteubernahmeCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: "late-deadline".into(),
            },
        )
        .unwrap();
        assert!(events.is_empty(), "deadline on terminal must be a no-op");
    }

    #[test]
    fn all_anfrage_pids_accepted() {
        // ANFRAGE_PIDS is empty (all moved to WiM Gas); this test is now a no-op.
        // WiM Gas PIDs (17001/17002) are accepted by the same workflow when routed
        // by WimGasModule — tested in integration/E2E tests.
        for &pid in ANFRAGE_PIDS {
            let state = GeraeteubernahmeState::default();
            assert!(
                WimGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(pid)).is_ok(),
                "PID {pid} must be accepted",
            );
        }
    }

    #[test]
    fn wrong_pid_family_rejected() {
        let state = GeraeteubernahmeState::default();
        let result = WimGeraeteubernahmeWorkflow::handle(&state, anfrage_cmd(55001));
        assert!(
            result.is_err(),
            "GPKE PID must be rejected by Geräteübernahme"
        );
    }
}
