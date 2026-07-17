//! WiM Steuerungsauftrag — grid control command workflow.
//!
//! Covers the lifecycle of a single **remote load control command**
//! (Steuerbefehl) exchanged between a network operator (NB) or supplier (LF)
//! and a metering-point operator (MSB / iMSB) via the BDEW
//! **API-Webdienste Strom** `controlMeasuresV1` REST channel.
//!
//! This workflow is the event-sourced counterpart of the UTILMD-based
//! Gerätewechsel for the REST channel: instead of AS4 + EDIFACT,
//! NB/LF and MSB exchange JSON over HTTPS.
//!
//! ## Role context
//!
//! When `makod` acts as **MSB** (Messstellenbetreiber):
//! - Receives `konfiguration` or `initialZustand` from NB/LF.
//! - Acknowledges immediately with `202 Accepted`.
//! - Sends `vorlaeufigePositiveAntwort` once MSB back-end confirms feasibility.
//! - Sends final `positiveAntwort` / `negativeAntwort` after execution.
//!
//! ## Regulatory basis
//!
//! - **BDEW API-Guideline 1.0a** — API-Webdienste Strom, `controlMeasuresV1.yaml`
//! - **BK6-18-032** — WiM timeline obligations (5 Werktage for responses)
//!
//! ## Frist
//!
//! | Frist | Value | Calculation |
//! |-------|-------|-------------|
//! | Response window | **5 Werktage** | `fristen::add_werktage(5, BdewMaKo)` |
//!
//! ## State machine
//!
//! ```text
//! New ──ReceiveKonfiguration/ReceiveInitialZustand──► Received
//!                                                       │
//!                             SendEndantwort(positive) ─┤─► Completed
//!                             SendEndantwort(negative) ─┤─► Rejected
//!                             TimeoutExpired           ─┘─► Rejected
//! ```
//!
//! ## ERP commands (via `POST /api/v1/commands`)
//!
//! | Command | Marktrolle | Description |
//! |---------|-----------|-------------|
//! | `wim.steuerungsauftrag.bestaetigen` | `MSB` | Send final positive response |
//! | `wim.steuerungsauftrag.ablehnen`    | `MSB` | Send final negative response |

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::MarktpartnerCode,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use rubo4e::identifiers::{NeloId, SrId};

/// Stable workflow name used in `WorkflowId` and `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "wim-steuerungsauftrag";

/// Deadline label for the 5-Werktage response confirmation window (WiM BK6-18-032).
///
/// Register a `Deadline` with this label immediately after `KonfigurationReceived`
/// or `InitialZustandReceived`:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     received_at, 5, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., STEUERUNGSAUFTRAG_DEADLINE_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const STEUERUNGSAUFTRAG_DEADLINE_LABEL: &str = "wim-steuerungsauftrag-deadline";

// ── Control target location ───────────────────────────────────────────────────

/// Typed target of a WiM Steuerungsauftrag command.
///
/// A control command addresses either a NeLo (network element location) or
/// a SteuerbareRessource (iMS controllable resource, BK6-24-174 §6).
/// Both identifiers are validated at construction time via rubo4e.
///
/// | Prefix | Type | Example |
/// |--------|------|---------|
/// | `E…` / `10Y…` | [`NeloId`] — EIC or BDEW Codenummer | `10XDE-EON-NETZ--G` |
/// | `C…` | [`SrId`] — Steuerbare-Ressource-ID | `C0001234567890` |
///
/// [`NeloId`]: rubo4e::identifiers::NeloId
/// [`SrId`]: rubo4e::identifiers::SrId
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum LocationId {
    /// Netz-Element-Lokation (EIC code or BDEW Codenummer).
    Nelo(NeloId),
    /// Steuerbare Ressource — iMS controllable resource (BK6-24-174 §6).
    Sr(SrId),
}

impl LocationId {
    /// Returns the raw string representation of this location identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Nelo(id) => id.as_ref(),
            Self::Sr(id) => id.as_ref(),
        }
    }
}

impl std::fmt::Display for LocationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Type of control command ───────────────────────────────────────────────────

/// Variant of the control command received from NB/LF.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SteuerungsCommandType {
    /// Power regulation command — limit smart meter to a specific maximum kW.
    Konfiguration,
    /// Reset command — restore to normal (unlimited) state.
    InitialZustand,
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Immutable data captured when the MSB receives a control command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteuerungsauftragData {
    /// Transaction ID from the NB/LF (unique per command).
    pub tx_id: String,
    /// GLN of the NB or LF that sent the command.
    pub sender_mp_id: MarktpartnerCode,
    /// Typed target: either a NeLo-ID or SR-ID (iMS SteuerbareRessource).
    pub location_id: LocationId,
    /// Whether this is a power-regulation or reset command.
    pub command_type: SteuerungsCommandType,
    /// ISO-8601 UTC timestamp from which the control effect begins.
    pub execution_time_from: String,
    /// Maximum power value in kW (only set for `Konfiguration`).
    pub max_power_kw: Option<String>,
    /// ISO-8601 UTC timestamp at which the control effect ends (if bounded).
    pub execution_time_until: Option<String>,
    /// Contracted Konfigurationsprodukt code (e.g. `"TX-MODUL2-HT"`).
    ///
    /// Extracted from the inbound ORDERS `LIN+1` segment at ingestion time.
    /// Used by `makod` to verify that the requested product is in the SR's
    /// `konfigurationsprodukte` list before dispatching the ORDRSP
    /// `bestaetigen` response (M1 guard).
    ///
    /// `None` for legacy processes created before this field was introduced
    /// or for `InitialZustand` (reset) commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produkt_code: Option<String>,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Steuerungsauftrag workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SteuerungsauftragEvent {
    /// MSB received a `konfiguration` command from NB/LF.
    KonfigurationReceived {
        /// Transaction ID from NB/LF (idempotency key).
        tx_id: String,
        /// GLN of the NB/LF sending the command.
        sender_mp_id: MarktpartnerCode,
        /// Typed target: NeLo or SteuerbareRessource.
        location_id: LocationId,
        /// ISO-8601 UTC timestamp from which the limit takes effect.
        execution_time_from: String,
        /// Maximum power in kW.
        max_power_kw: String,
        /// ISO-8601 UTC timestamp at which the limit ends (if bounded).
        execution_time_until: Option<String>,
        /// Contracted Konfigurationsprodukt code from ORDERS `LIN+1` segment.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        produkt_code: Option<String>,
    },
    /// MSB received an `initialZustand` (reset) command from NB/LF.
    InitialZustandReceived {
        /// Transaction ID from NB/LF (idempotency key).
        tx_id: String,
        /// GLN of the NB/LF sending the command.
        sender_mp_id: MarktpartnerCode,
        /// Typed target: NeLo or SteuerbareRessource.
        location_id: LocationId,
        /// ISO-8601 UTC timestamp from which the reset takes effect.
        execution_time_from: String,
    },
    /// MSB sent a final positive response — command executed successfully.
    EndantwortPositiv {
        /// Reference ID echoed back to NB/LF.
        reference_id: String,
    },
    /// MSB sent a final negative response — command could not be executed.
    EndantwortNegativ {
        /// Optional failure reason.
        reason: Option<String>,
    },
    /// A registered deadline expired before a final response was sent.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for SteuerungsauftragEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::KonfigurationReceived { .. } => "WimSteuerungsauftragKonfigurationReceived",
            Self::InitialZustandReceived { .. } => "WimSteuerungsauftragInitialZustandReceived",
            Self::EndantwortPositiv { .. } => "WimSteuerungsauftragEndantwortPositiv",
            Self::EndantwortNegativ { .. } => "WimSteuerungsauftragEndantwortNegativ",
            Self::DeadlineExpired { .. } => "WimSteuerungsauftragDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a WiM Steuerungsauftrag process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum SteuerungsauftragState {
    /// No events yet.
    #[default]
    New,
    /// Control command received; waiting for MSB back-end confirmation.
    Received(SteuerungsauftragData),
    /// Final positive response sent — command executed.
    Completed(SteuerungsauftragData),
    /// Process closed with a negative or timeout outcome.
    Rejected {
        /// Transaction ID, if known at rejection time.
        tx_id: Option<String>,
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl SteuerungsauftragState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Received(_) => "Received",
            Self::Completed(_) => "Completed",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Steuerungsauftrag workflow.
#[derive(Clone)]
pub enum SteuerungsauftragCommand {
    /// Inbound `konfiguration` command from NB/LF via REST.
    ///
    /// Spawns a new process and records the command for 5-Werktage tracking.
    ReceiveKonfiguration {
        /// Transaction ID from NB/LF (idempotency key).
        tx_id: String,
        /// GLN of the NB/LF sending the command.
        sender_mp_id: MarktpartnerCode,
        /// Typed target: NeLo or SteuerbareRessource (iMS).
        location_id: LocationId,
        /// ISO-8601 UTC timestamp from which the limit takes effect.
        execution_time_from: String,
        /// Maximum power in kW.
        max_power_kw: String,
        /// ISO-8601 UTC timestamp at which the limit ends (if bounded).
        execution_time_until: Option<String>,
        /// Contracted Konfigurationsprodukt code from the inbound ORDERS `LIN+1` segment.
        ///
        /// Stored in process state so that `makod` can verify the product is
        /// in the SR's `konfigurationsprodukte` list before dispatching ORDRSP
        /// `bestaetigen`.  `None` when the ORDERS message does not specify a
        /// product (legacy / non-§14a flows).
        produkt_code: Option<String>,
    },
    /// Inbound `initialZustand` (reset) command from NB/LF via REST.
    ReceiveInitialZustand {
        /// Transaction ID from NB/LF (idempotency key).
        tx_id: String,
        /// GLN of the NB/LF sending the command.
        sender_mp_id: MarktpartnerCode,
        /// Typed target: NeLo or SteuerbareRessource (iMS).
        location_id: LocationId,
        /// ISO-8601 UTC timestamp from which the reset takes effect.
        execution_time_from: String,
    },
    /// ERP instructs MSB to send the final positive answer to NB/LF.
    ///
    /// Called via `wim.steuerungsauftrag.bestaetigen`.
    SendEndantwortPositiv {
        /// Reference ID echoed back to NB/LF.
        reference_id: String,
    },
    /// ERP instructs MSB to send the final negative answer to NB/LF.
    ///
    /// Called via `wim.steuerungsauftrag.ablehnen`.
    SendEndantwortNegativ {
        /// Optional failure reason.
        reason: Option<String>,
    },
    /// A registered 5-Werktage deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for SteuerungsauftragCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Steuerungsauftrag workflow — MSB-side control command tracking.
///
/// Spawned from `MakodApiHandler::on_konfiguration` / `on_initial_zustand` when
/// the API-Webdienste Strom server receives a control command from an NB or LF.
pub struct WimSteuerungsauftragWorkflow;

impl Workflow for WimSteuerungsauftragWorkflow {
    type State = SteuerungsauftragState;
    type Event = SteuerungsauftragEvent;
    type Command = SteuerungsauftragCommand;

    /// Deadline compensation for the WiM Steuerungsauftrag 5-Werktage confirmation window.
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"wim-steuerungsauftrag-deadline"` | `Received` | `TimeoutExpired` | BK6-18-032 — 5 Werktage Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (STEUERUNGSAUFTRAG_DEADLINE_LABEL, SteuerungsauftragState::Received(_)) => {
                Some(SteuerungsauftragCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            SteuerungsauftragEvent::KonfigurationReceived {
                tx_id,
                sender_mp_id,
                location_id,
                execution_time_from,
                max_power_kw,
                execution_time_until,
                produkt_code,
            } => SteuerungsauftragState::Received(SteuerungsauftragData {
                tx_id: tx_id.clone(),
                sender_mp_id: sender_mp_id.clone(),
                location_id: location_id.clone(),
                command_type: SteuerungsCommandType::Konfiguration,
                execution_time_from: execution_time_from.clone(),
                max_power_kw: Some(max_power_kw.clone()),
                execution_time_until: execution_time_until.clone(),
                produkt_code: produkt_code.clone(),
            }),
            SteuerungsauftragEvent::InitialZustandReceived {
                tx_id,
                sender_mp_id,
                location_id,
                execution_time_from,
            } => SteuerungsauftragState::Received(SteuerungsauftragData {
                tx_id: tx_id.clone(),
                sender_mp_id: sender_mp_id.clone(),
                location_id: location_id.clone(),
                command_type: SteuerungsCommandType::InitialZustand,
                execution_time_from: execution_time_from.clone(),
                max_power_kw: None,
                execution_time_until: None,
                produkt_code: None,
            }),
            SteuerungsauftragEvent::EndantwortPositiv { .. } => {
                if let SteuerungsauftragState::Received(data) = state {
                    SteuerungsauftragState::Completed(data)
                } else {
                    state
                }
            }
            SteuerungsauftragEvent::EndantwortNegativ { reason } => {
                let tx_id = match &state {
                    SteuerungsauftragState::Received(d) => Some(d.tx_id.clone()),
                    _ => None,
                };
                SteuerungsauftragState::Rejected {
                    tx_id,
                    reason: reason
                        .clone()
                        .unwrap_or_else(|| "negative response".to_owned()),
                }
            }
            SteuerungsauftragEvent::DeadlineExpired { label, .. } => match state {
                SteuerungsauftragState::Completed(_) | SteuerungsauftragState::Rejected { .. } => {
                    state
                }
                _ => {
                    let tx_id = if let SteuerungsauftragState::Received(ref d) = state {
                        Some(d.tx_id.clone())
                    } else {
                        None
                    };
                    SteuerungsauftragState::Rejected {
                        tx_id,
                        reason: format!("5-Werktage deadline expired: {label}"),
                    }
                }
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            SteuerungsauftragCommand::ReceiveKonfiguration {
                tx_id,
                sender_mp_id,
                location_id,
                execution_time_from,
                max_power_kw,
                execution_time_until,
                produkt_code,
            } => {
                if !matches!(state, SteuerungsauftragState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                Ok(vec![SteuerungsauftragEvent::KonfigurationReceived {
                    tx_id,
                    sender_mp_id,
                    location_id,
                    execution_time_from,
                    max_power_kw,
                    execution_time_until,
                    produkt_code,
                }]
                .into())
            }

            SteuerungsauftragCommand::ReceiveInitialZustand {
                tx_id,
                sender_mp_id,
                location_id,
                execution_time_from,
            } => {
                if !matches!(state, SteuerungsauftragState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                Ok(vec![SteuerungsauftragEvent::InitialZustandReceived {
                    tx_id,
                    sender_mp_id,
                    location_id,
                    execution_time_from,
                }]
                .into())
            }

            SteuerungsauftragCommand::SendEndantwortPositiv { reference_id } => {
                let SteuerungsauftragState::Received(data) = &state else {
                    return Err(WorkflowError::invalid_state("Received", state.status_str()));
                };
                let event = SteuerungsauftragEvent::EndantwortPositiv { reference_id };

                // Emit a `DispatchConfirmed` outbox entry for `Konfiguration`
                // (load-reduction) commands so that `billingd` can auto-trigger
                // VPP settlement billing without operator intervention.
                //
                // `InitialZustand` (reset) commands restore normal operation —
                // no dispatch billing event is emitted for resets.
                //
                // The payload carries all dispatch metadata needed by
                // `billingd POST /api/v1/webhooks/vpp-dispatch`.
                if matches!(data.command_type, SteuerungsCommandType::Konfiguration) {
                    let payload = serde_json::json!({
                        "tx_id":               data.tx_id,
                        "location_id":         data.location_id.as_str(),
                        "location_type":       match &data.location_id {
                                                   LocationId::Sr(_) => "sr",
                                                   LocationId::Nelo(_) => "nelo",
                                               },
                        "execution_time_from": data.execution_time_from,
                        "execution_time_until": data.execution_time_until,
                        "max_power_kw":        data.max_power_kw,
                        "command_type":        "Konfiguration",
                        "sender_mp_id":        data.sender_mp_id.as_str(),
                        "produkt_code":        data.produkt_code,
                    });
                    let outbox = mako_engine::outbox::PendingOutbox::new(
                        "DispatchConfirmed",
                        // recipient = the NB/LF that sent the Steuerungsauftrag
                        data.sender_mp_id.as_str(),
                        payload,
                    );
                    Ok(mako_engine::workflow::WorkflowOutput::with_outbox(
                        vec![event],
                        vec![outbox],
                    ))
                } else {
                    // InitialZustand reset — no billing event.
                    Ok(vec![event].into())
                }
            }

            SteuerungsauftragCommand::SendEndantwortNegativ { reason } => {
                if !matches!(state, SteuerungsauftragState::Received(_)) {
                    return Err(WorkflowError::invalid_state("Received", state.status_str()));
                }
                Ok(vec![SteuerungsauftragEvent::EndantwortNegativ { reason }].into())
            }

            SteuerungsauftragCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    SteuerungsauftragState::Completed(_) | SteuerungsauftragState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![SteuerungsauftragEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}
