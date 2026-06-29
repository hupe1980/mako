//! WiM Messstellenbetrieb — MSB change workflow (PIDs 55039, 55042, 55051, 55168).
//!
//! Covers the process by which an incoming metering point operator
//! (neuer Messstellenbetreiber, nMSB) initiates a change of the MSB at a
//! delivery point (Messlokation, MeLo) by sending a UTILMD message to the
//! grid operator (Netzbetreiber, NB). The NB validates the message and
//! dispatches an APERAK within **5 Werktage** (business days).
//!
//! # Regulatory basis
//!
//! - **MsbG** — Messstellenbetriebsgesetz (governing smart meter rollout)
//! - **BDEW WiM** — Wechselprozesse im Messwesen Strom
//! - **BNetzA BK6-18-032** — ruling governing WiM timeline obligations
//! - **UTILMD S2.x** — EDI@Energy message format for metering processes
//! - **APERAK 2.x** — Application error acknowledgement (**5 Werktage** Frist)
//!
//! # Frist comparison
//!
//! | Process family | APERAK Frist | Calculation |
//! |---|---|---|
//! | GPKE Lieferbeginn | 24 h wall-clock | `fristen::add_hours(24)` |
//! | WiM Gerätewechsel | **5 Werktage** | `fristen::add_werktage(5, BdewMaKo)` |
//! | GeLi Gas Anmeldung | **10 Werktage** | `fristen::add_werktage(10, BdewMaKo)` |

use std::collections::HashMap;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Stable workflow name used as the `WorkflowId.name` and in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "wim-device-change";

/// Deadline label for the 5-Werktage APERAK response window (WiM BK6-18-032).
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     received_at, 5, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., APERAK_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const APERAK_WINDOW_LABEL: &str = "wim-aperak-5-werktage";

/// WiM Strom IFTSTA Prüfidentifikatoren (PIDs 21029–21032).
///
/// These status messages are part of the WiM MSB-Wechsel (WiM Strom Teil 1)
/// process. All are routed to `"wim-device-change"` for correlation.
///
/// **PIDs 21009/21010/21011/21012/21013/21015/21018** are "WiM Gas" per
/// `docs/pid-reference.md`. They must be registered in `mako-wim-gas`,
/// NOT here.
///
/// | PID   | Beschreibung |
/// |-------|----------|
/// | 21029 | Vorabinformation (WiM Strom Teil 1) |
/// | 21030 | iMS-Ersteinbauzustand (WiM Strom Teil 1) |
/// | 21031 | Bestandssituation / Eigenausbau iMS (WiM Strom Teil 1) |
/// | 21032 | Antwort auf das Angebot (WiM Strom Teil 1) |
pub const IFTSTA_PIDS: &[u32] = &[21_029, 21_030, 21_031, 21_032];

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Gerätewechsel workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DeviceChangeEvent {
    /// Process initiated by a valid UTILMD Anmeldung Messstellenbetrieb.
    Initiated {
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// GLN of the incoming Messstellenbetreiber.
        incoming_msb: MarktpartnerCode,
        /// GLN of the grid operator (Netzbetreiber).
        grid_operator: MarktpartnerCode,
        /// Physical device identifier.
        device_id: DeviceId,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference (UNH/BGM).
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation (no rule violations).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// A positive or negative APERAK was dispatched within 5 Werktage.
    AperakDispatched {
        /// `true` for positive (accepted), `false` for negative (rejected).
        positive: bool,
        /// Rejection reason (only set when `positive = false`).
        reason: Option<String>,
    },
    /// Meter device physically changed; new MSB is active.
    Completed {
        /// Physical device identifier confirmed at completion.
        device_id: DeviceId,
    },
    /// Process was rejected and closed.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline expired before the process completed.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// Received an IFTSTA WiM status message (PIDs 21009–21018).
    ///
    /// WiM IFTSTA messages are informational: they notify the parties of
    /// process-status updates and Vollzugsmeldungen without driving a state
    /// transition. Recorded in the event log for audit purposes.
    IftstaStatusReceived {
        /// IFTSTA Prüfidentifikator (21009–21018).
        pid: Pruefidentifikator,
        /// Sender party code (GLN).
        sender: MarktpartnerCode,
        /// Receiver party code (GLN).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
}

impl EventPayload for DeviceChangeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "WimDeviceChangeInitiated",
            Self::ValidationPassed { .. } => "WimDeviceChangeValidationPassed",
            Self::AperakDispatched { .. } => "WimDeviceChangeAperakDispatched",
            Self::Completed { .. } => "WimDeviceChangeCompleted",
            Self::Rejected { .. } => "WimDeviceChangeRejected",
            Self::DeadlineExpired { .. } => "WimDeviceChangeDeadlineExpired",
            Self::IftstaStatusReceived { .. } => "WimDeviceChangeIftstaStatusReceived",
        }
    }
    // schema_version defaults to 1; increment and add an upcast arm on next
    // backward-incompatible payload layout change.
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data set at `Initiated` time and carried through every later state.
///
/// All fields are structurally guaranteed to be present once the process moves
/// past `New` — no `unwrap()` required downstream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceChangeData {
    /// EIC/MeLo code for the metering location.
    pub melo_id: MeLo,
    /// Market partner code (GLN) of the incoming MSB.
    pub incoming_msb: MarktpartnerCode,
    /// Market partner code (GLN) of the grid operator.
    pub grid_operator: MarktpartnerCode,
    /// Device identifier.
    pub device_id: DeviceId,
    /// EDIFACT document date string from the UTILMD.
    pub document_date: String,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: Pruefidentifikator,
    /// Original UTILMD message reference, preserved for APERAK construction.
    /// `None` only for processes initiated before this field was added (old snapshots).
    #[serde(default)]
    pub message_ref: Option<MessageRef>,
}

/// Current state of a WiM Gerätewechsel process stream.
///
/// Modelled as an enum-per-variant to eliminate all `Option`-unwraps:
/// each variant carries exactly the data that is structurally available at
/// that stage. Invalid states are unrepresentable.
///
/// # Lifecycle
///
/// ```text
/// New → Initiated → ValidationPassed → AperakSent → Completed
///                                    ↘ Rejected
///     ↘ Rejected (failed validation at Initiated step)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum DeviceChangeState {
    /// No events yet; stream exists but process has not started.
    New,
    /// UTILMD received and `Initiated` event applied.
    Initiated(DeviceChangeData),
    /// EDIFACT validation passed; APERAK not yet dispatched.
    ValidationPassed(DeviceChangeData),
    /// Positive APERAK dispatched; awaiting physical device swap.
    AperakSent(DeviceChangeData),
    /// Device physically changed; new MSB is active.
    Completed(DeviceChangeData),
    /// Process rejected (validation failure or negative APERAK).
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl Default for DeviceChangeState {
    fn default() -> Self {
        Self::New
    }
}

impl DeviceChangeState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Initiated(_) => "Initiated",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AperakSent(_) => "AperakSent",
            Self::Completed(_) => "Completed",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Gerätewechsel workflow.
///
/// **All domain values must be pre-extracted by the transport layer** before
/// constructing a command. `Workflow::handle()` is pure — no I/O, no EDIFACT
/// parsing, no external calls. See the crate-level doc for a construction
/// example.
#[derive(Clone)]
pub enum DeviceChangeCommand {
    /// Inbound UTILMD accepted from the AS4 layer. Domain fields extracted and
    /// validation performed by the caller before constructing this command.
    ReceiveUtilmd {
        /// BDEW Prüfidentifikator.
        pid: Pruefidentifikator,
        /// GLN of the message sender (nMSB).
        sender: MarktpartnerCode,
        /// GLN of the message receiver (NB).
        receiver: MarktpartnerCode,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// Physical device identifier.
        device_id: DeviceId,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if `msg.validate()` returned a report with no errors.
        validation_passed: bool,
        /// Human-readable validation issue strings for the `Rejected` event.
        validation_errors: Vec<String>,
    },
    /// Inbound iMS Universalbestellprozess order received via REST
    /// (BDEW API-Webdienste Strom, valid 2026-01-29+, PIDs 11021–11023).
    ///
    /// Used when the Netzbetreiber orders an iMS installation from the MSB
    /// through the REST channel rather than via EDIFACT/AS4. The caller is
    /// responsible for validating the request before constructing this command.
    ReceiveRestOrder {
        /// REST transaction UUID (idempotency key; carried through to events).
        tx_id: String,
        /// 13-digit GLN of the Netzbetreiber (order sender).
        sender_gln: MarktpartnerCode,
        /// EIC of the Messlokation at which the device should be installed.
        melo_id: MeLo,
        /// Requested device category (e.g. `"iMSys"`, `"mME"`, `"mME+KME"`).
        device_category: String,
        /// Requested installation / process date (ISO 8601 date string).
        process_date: String,
    },
    /// Dispatch a positive or negative APERAK.
    ///
    /// **BDEW WiM / BNetzA BK6-18-032**: APERAK must be sent within
    /// **5 Werktage** of receiving the UTILMD (not wall-clock hours).
    /// Use `fristen::add_werktage(5, HolidayCalendar::BdewMaKo)` to compute
    /// the deadline.
    DispatchAperak {
        /// `true` for positive APERAK, `false` for negative.
        positive: bool,
        /// Rejection reason (required when `positive = false`).
        reason: Option<String>,
    },
    /// Mark the device change as completed once the physical swap is confirmed.
    Complete {
        /// Physical device identifier confirmed at completion.
        device_id: DeviceId,
    },
    /// A registered deadline fired and was dispatched by the scheduler.
    ///
    /// Transitions the process to `Rejected` unless it has already reached
    /// a terminal state (`Completed` or `Rejected`), in which case this is a no-op.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// Received an IFTSTA WiM status message (PIDs 21009–21018).
    ///
    /// Constructed by the IFTSTA adapter in `makod` when an inbound AS4
    /// IFTSTA message with a WiM PID arrives, or via the
    /// `"wim.iftsta.empfangen"` REST command.
    ReceiveIftsta {
        /// IFTSTA Prüfidentifikator (21009–21018).
        pid: Pruefidentifikator,
        /// Sender party code (GLN).
        sender: MarktpartnerCode,
        /// Receiver party code (GLN).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// Whether the IFTSTA message passed AHB validation.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
}

impl CommandPayload for DeviceChangeCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Messstellenbetrieb (PIDs 55039, 55042, 55051, 55168) workflow.
///
/// Implements the BDEW WiM process for change and management of meter operators
/// (MSB) at a Messlokation. The grid operator receives inbound UTILMD messages
/// from the MSBN and must respond with an APERAK within **5 Werktage**.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<WimDeviceChangeWorkflow>(
///     tenant_id,
///     WorkflowId::new("wim-device-change", "FV2025-10-01"),
/// );
/// ```
pub struct WimDeviceChangeWorkflow;

impl Workflow for WimDeviceChangeWorkflow {
    type State = DeviceChangeState;
    type Event = DeviceChangeEvent;
    type Command = DeviceChangeCommand;

    /// Deadline compensation for the WiM Gerätewechsel 5-Werktage APERAK window.
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"wim-aperak-5-werktage"` | `Initiated` or `ValidationPassed` | `TimeoutExpired` | BK6-18-032 — 5 Werktage APERAK Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                APERAK_WINDOW_LABEL,
                DeviceChangeState::Initiated(_) | DeviceChangeState::ValidationPassed(_),
            ) => Some(DeviceChangeCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            DeviceChangeEvent::Initiated {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                document_date,
                message_ref,
                pruefidentifikator,
            } => DeviceChangeState::Initiated(DeviceChangeData {
                melo_id: melo_id.clone(),
                incoming_msb: incoming_msb.clone(),
                grid_operator: grid_operator.clone(),
                device_id: device_id.clone(),
                document_date: document_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: Some(message_ref.clone()),
            }),
            DeviceChangeEvent::ValidationPassed { .. } => {
                if let DeviceChangeState::Initiated(data) = state {
                    DeviceChangeState::ValidationPassed(data)
                } else {
                    state
                }
            }
            DeviceChangeEvent::AperakDispatched { positive, .. } => match state {
                DeviceChangeState::ValidationPassed(data) => {
                    if *positive {
                        DeviceChangeState::AperakSent(data)
                    } else {
                        DeviceChangeState::Rejected {
                            reason: "negative APERAK".to_owned(),
                        }
                    }
                }
                _ => state,
            },
            DeviceChangeEvent::Completed { device_id } => {
                if let DeviceChangeState::AperakSent(mut data) = state {
                    data.device_id = device_id.clone();
                    DeviceChangeState::Completed(data)
                } else {
                    state
                }
            }
            DeviceChangeEvent::Rejected { reason } => DeviceChangeState::Rejected {
                reason: reason.clone(),
            },
            DeviceChangeEvent::DeadlineExpired { label, .. } => match state {
                DeviceChangeState::Completed(_) | DeviceChangeState::Rejected { .. } => state,
                _ => DeviceChangeState::Rejected {
                    reason: format!("deadline expired: {label}"),
                },
            },

            // Informational WiM IFTSTA status messages do not change state.
            DeviceChangeEvent::IftstaStatusReceived { .. } => state,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            DeviceChangeCommand::ReceiveRestOrder {
                tx_id,
                sender_gln,
                melo_id,
                device_category,
                process_date,
            } => {
                if !matches!(state, DeviceChangeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                // PID 11021 — iMSys Anmeldung (Universalbestellprozess via REST).
                let pid = Pruefidentifikator::new(11_021).map_err(|e| {
                    WorkflowError::rejected(format!("constant PID 11021 invalid: {e}"))
                })?;
                // REST orders carry no EDIFACT device ID; use the tx_id as a
                // provisional placeholder until the MSB assigns a device EIC.
                let device_id = DeviceId::new(&*tx_id);
                let message_ref = MessageRef::new(&*tx_id);
                Ok(vec![
                    DeviceChangeEvent::Initiated {
                        melo_id,
                        incoming_msb: sender_gln,
                        // REST orders target the MSB (self); grid_operator is
                        // not known at this point — carry device_category in
                        // document_date for now (process_date holds the date).
                        grid_operator: MarktpartnerCode::new(""),
                        device_id,
                        document_date: format!("{process_date}|category={device_category}"),
                        message_ref: message_ref.clone(),
                        pruefidentifikator: pid,
                    },
                    // REST-sourced orders are structurally valid by definition
                    // (the HTTP layer validated the JSON payload); emit
                    // ValidationPassed immediately.
                    DeviceChangeEvent::ValidationPassed { message_ref },
                ]
                .into())
            }

            DeviceChangeCommand::ReceiveUtilmd {
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
                if !matches!(state, DeviceChangeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                // PID guard: reject any PID not in the WiM MSB-Wechsel family.
                // Only PIDs 55039, 55042, 55051, 55168 are registered by WimModule;
                // this guard is defence-in-depth for direct callers.
                let valid_pids = [55_039_u32, 55_042, 55_051, 55_168];
                if !valid_pids.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a WiM Messstellenbetrieb PID (expected 55039, 55042, 55051, or 55168)",
                        pid.as_u32()
                    )));
                }
                let mut events = vec![DeviceChangeEvent::Initiated {
                    melo_id,
                    incoming_msb: sender,
                    grid_operator: receiver,
                    device_id,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(DeviceChangeEvent::ValidationPassed { message_ref });
                } else {
                    events.push(DeviceChangeEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            DeviceChangeCommand::DispatchAperak { positive, reason } => {
                let data = match state {
                    DeviceChangeState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.status_str(),
                        ));
                    }
                };
                let mut payload = serde_json::json!({
                    "pid":           data.pruefidentifikator.as_u32(),
                    "melo":          data.melo_id.as_str(),
                    "incoming_msb":  data.incoming_msb.as_str(),
                    "grid_operator": data.grid_operator.as_str(),
                    "positive":      positive,
                });
                if let Some(ref mr) = data.message_ref {
                    payload["orig_message_ref"] = serde_json::Value::String(mr.as_str().to_owned());
                }
                if let Some(ref r) = reason {
                    payload["reason"] = serde_json::Value::String(r.clone());
                }
                let outbox_entry =
                    PendingOutbox::new("Aperak", data.incoming_msb.as_str(), payload);
                Ok(WorkflowOutput::with_outbox(
                    vec![DeviceChangeEvent::AperakDispatched { positive, reason }],
                    vec![outbox_entry],
                ))
            }

            DeviceChangeCommand::Complete { device_id } => {
                if !matches!(state, DeviceChangeState::AperakSent(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AperakSent",
                        state.status_str(),
                    ));
                }
                Ok(vec![DeviceChangeEvent::Completed { device_id }].into())
            }

            DeviceChangeCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    DeviceChangeState::Completed(_) | DeviceChangeState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![DeviceChangeEvent::DeadlineExpired { deadline_id, label }].into())
            }

            DeviceChangeCommand::ReceiveIftsta {
                pid,
                sender,
                receiver,
                message_ref,
                ..
            } => {
                // WiM IFTSTA messages are informational. Accept them in any
                // state (the process may already be completed when a late
                // Vollzugsmeldung arrives) and record for audit purposes.
                Ok(vec![DeviceChangeEvent::IftstaStatusReceived {
                    pid,
                    sender,
                    receiver,
                    message_ref,
                }]
                .into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single WiM Gerätewechsel process stream.
///
/// Uses a type-state design so field access never requires `Option::unwrap`:
/// the `Active` variant carries all domain fields that are structurally
/// guaranteed to exist once the process moves past `New`.
#[derive(Debug)]
pub enum DeviceChangeRecord {
    /// No `Initiated` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `Initiated` event applied; process fields now available.
    Active {
        /// Current lifecycle stage.
        status: &'static str,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// GLN of the incoming Messstellenbetreiber.
        incoming_msb: MarktpartnerCode,
        /// GLN of the grid operator.
        grid_operator: MarktpartnerCode,
        /// Physical device identifier (updated on `Completed`).
        device_id: DeviceId,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// Total events applied.
        event_count: usize,
    },
}

impl DeviceChangeRecord {
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
    pub fn active_data(&self) -> Option<DeviceChangeRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                pruefidentifikator,
                ..
            } => Some(DeviceChangeRecordData {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                pruefidentifikator,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` `DeviceChangeRecord`.
#[derive(Debug, Clone, Copy)]
pub struct DeviceChangeRecordData<'a> {
    /// Messlokation EIC code.
    pub melo_id: &'a MeLo,
    /// GLN of the incoming Messstellenbetreiber.
    pub incoming_msb: &'a MarktpartnerCode,
    /// GLN of the grid operator.
    pub grid_operator: &'a MarktpartnerCode,
    /// Physical device identifier.
    pub device_id: &'a DeviceId,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: &'a Pruefidentifikator,
}

impl Default for DeviceChangeRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model that tracks status across all WiM Gerätewechsel
/// streams. Feed via [`mako_engine::projection::ProjectionRunner`].
#[derive(Debug, Default)]
pub struct DeviceChangeProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, DeviceChangeRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for DeviceChangeProjection {
    fn name(&self) -> &'static str {
        "DeviceChangeProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<DeviceChangeEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            DeviceChangeRecord::New { event_count } => *event_count += 1,
            DeviceChangeRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            DeviceChangeEvent::Initiated {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                pruefidentifikator,
                ..
            } => {
                let count = record.event_count();
                *record = DeviceChangeRecord::Active {
                    status: "Initiated",
                    melo_id,
                    incoming_msb,
                    grid_operator,
                    device_id,
                    pruefidentifikator,
                    event_count: count,
                };
            }
            DeviceChangeEvent::ValidationPassed { .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            DeviceChangeEvent::AperakDispatched { positive, .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = if positive { "AperakSent" } else { "Rejected" };
                }
            }
            DeviceChangeEvent::Completed { device_id } => {
                if let DeviceChangeRecord::Active {
                    status,
                    device_id: d,
                    ..
                } = record
                {
                    *status = "Completed";
                    *d = device_id;
                }
            }
            DeviceChangeEvent::Rejected { .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
            DeviceChangeEvent::DeadlineExpired { .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
            DeviceChangeEvent::IftstaStatusReceived { .. } => {
                // Informational — does not change the status label.
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_receive_cmd(pid: u32, validation_passed: bool) -> DeviceChangeCommand {
        DeviceChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(pid).expect("test pid must be in range"),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            melo_id: MeLo::new("DE0000000001234567890000000000001"),
            device_id: DeviceId::new("ZHR-12345678"),
            document_date: "20250115".to_owned(),
            message_ref: MessageRef::new("MSG-WIM-001"),
            validation_passed,
            validation_errors: if validation_passed {
                vec![]
            } else {
                vec!["AHB rule violation".to_owned()]
            },
        }
    }

    #[test]
    fn happy_path_new_to_completed() {
        let state = DeviceChangeState::default();

        let events = WimDeviceChangeWorkflow::handle(&state, make_receive_cmd(55042, true))
            .expect("should accept valid PID 55042");
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], DeviceChangeEvent::Initiated { pruefidentifikator, .. } if pruefidentifikator.as_u32() == 55042)
        );
        assert!(matches!(
            &events[1],
            DeviceChangeEvent::ValidationPassed { .. }
        ));

        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::ValidationPassed(_)),
            "expected ValidationPassed, got {}",
            state.status_str()
        );

        let events = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .expect("dispatch APERAK");
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::AperakSent(_)),
            "expected AperakSent"
        );

        let events = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::Complete {
                device_id: DeviceId::new("ZHR-99999999"),
            },
        )
        .expect("complete");
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::Completed(d) if d.device_id == DeviceId::new("ZHR-99999999")),
            "expected Completed with new device_id",
        );
    }

    #[test]
    fn wrong_pid_is_rejected() {
        let state = DeviceChangeState::default();
        let err = WimDeviceChangeWorkflow::handle(&state, make_receive_cmd(55001, true))
            .expect_err("should reject wrong PID");
        let msg = err.to_string();
        assert!(
            msg.contains("55001"),
            "error should mention the supplied PID: {msg}"
        );
    }

    #[test]
    fn validation_failure_rejects_process() {
        let state = DeviceChangeState::default();
        let events = WimDeviceChangeWorkflow::handle(&state, make_receive_cmd(55042, false))
            .expect("should still produce events");
        assert!(matches!(&events[1], DeviceChangeEvent::Rejected { .. }));
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::Rejected { .. }),
            "expected Rejected"
        );
    }

    #[test]
    fn dispatch_aperak_in_wrong_state_is_rejected() {
        // Status is New (not ValidationPassed)
        let state = DeviceChangeState::default();
        let err = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .expect_err("should reject dispatch in wrong state");
        assert!(err.to_string().contains("ValidationPassed"), "{err}");
    }
}
