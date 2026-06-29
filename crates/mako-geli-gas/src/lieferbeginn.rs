//! GeLi Gas Lieferbeginn / Lieferende — gas supplier switch workflow.
//!
//! Covers the process by which a new gas supplier (Gaslieferant) initiates a
//! supply start (Lieferbeginn Gas, PID 44001) by sending a UTILMD G message
//! to the gas grid operator (Gasnetzbetreiber, GNB). The GNB validates the
//! message and dispatches an APERAK within **10 Werktage** (business days).
//!
//! # Regulatory basis
//!
//! - **BDEW GeLi Gas** — Geschäftsprozesse Lieferantenwechsel Gas
//! - **BNetzA BK7** — ruling governing GeLi Gas timeline obligations
//! - **UTILMD G** — EDI@Energy UTILMD message format for gas processes
//! - **APERAK 2.x** — Application error acknowledgement (**10 Werktage** Frist)
//!
//! # Key differences from electricity processes
//!
//! | Aspect | GPKE (Strom) | WiM (Strom) | GeLi Gas |
//! |---|---|---|---|
//! | Market | Electricity | Electricity | **Gas** |
//! | Object | Messlokation (MeLo) | Messlokation (MeLo) | **Marktlokation (MaLo)** |
//! | Grid operator | Netzbetreiber (NB) | Netzbetreiber (NB) | **Gasnetzbetreiber (GNB)** |
//! | APERAK Frist | 24 h wall-clock | 5 Werktage | **10 Werktage** |
//! | Frist helper | `add_hours(24)` | `add_werktage(5, BdewMaKo)` | **`add_werktage(10, BdewMaKo)`** |
//!
//! # PID range
//!
//! | PID   | Process                                              | Profile              |
//! |-------|------------------------------------------------------|----------------------|
//! | 44001 | Lieferbeginn Gas (Anfrage LFN an NB)                 | ✅ fv20251001_gas+  |
//! | 44002 | Lieferende Gas (Anfrage LFN an NB)                   | ✅ fv20251001_gas+  |
//! | 44003 | Bestätigung Lieferbeginn Gas (NB an LFN)             | ✅ fv20251001_gas+  |
//! | 44004 | Ablehnung Lieferbeginn Gas (NB an LFN)               | ✅ fv20251001_gas+  |
//! | 44005 | Bestätigung Lieferende Gas (NB an LFN)               | ✅ fv20251001_gas+  |
//! | 44006 | Ablehnung Lieferende Gas (NB an LFN)                 | ✅ fv20251001_gas+  |
//! | 44017 | Kündigung Lieferbeginn Gas (LFN an LFA)              | ✅ fv20251001_gas+  |

use std::collections::HashMap;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Stable workflow name used as the `WorkflowId.name` and in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "geli-gas-supplier-change";

/// Deadline label for the 10-Werktage APERAK response window (GeLi Gas BNetzA ruling).
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     received_at, 10, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., APERAK_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const APERAK_WINDOW_LABEL: &str = "geli-gas-aperak-10-werktage";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas supplier-change workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasSupplierChangeEvent {
    /// Process initiated by a valid UTILMD G Lieferbeginn message.
    Initiated {
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// GLN of the new gas supplier (nLFN).
        new_supplier: MarktpartnerCode,
        /// GLN of the gas network operator (GNB).
        gas_operator: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation (no rule violations).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// A positive or negative APERAK was dispatched within 10 Werktage.
    AperakDispatched {
        /// `true` for positive APERAK, `false` for negative.
        positive: bool,
        /// Rejection reason (only set when `positive = false`).
        reason: Option<String>,
    },
    /// Gas supply relationship became active.
    Activated,
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
}

impl EventPayload for GasSupplierChangeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "GasSupplierChangeInitiated",
            Self::ValidationPassed { .. } => "GasSupplierChangeValidationPassed",
            Self::AperakDispatched { .. } => "GasSupplierChangeAperakDispatched",
            Self::Activated => "GasSupplierChangeActivated",
            Self::Rejected { .. } => "GasSupplierChangeRejected",
            Self::DeadlineExpired { .. } => "GasSupplierChangeDeadlineExpired",
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
pub struct GasSupplierChangeData {
    /// EIC/MaLo code for the gas supply location.
    pub malo_id: MaLo,
    /// Market partner code (GLN) of the new gas supplier.
    pub new_supplier: MarktpartnerCode,
    /// Market partner code (GLN) of the gas network operator.
    pub gas_operator: MarktpartnerCode,
    /// EDIFACT document date string from the UTILMD G.
    pub document_date: String,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: Pruefidentifikator,
    /// Original UTILMD G message reference, preserved for APERAK construction.
    /// `None` only for processes initiated before this field was added (old snapshots).
    #[serde(default)]
    pub message_ref: Option<MessageRef>,
}

/// Current state of a GeLi Gas supplier-change process stream.
///
/// Modelled as an enum-per-variant to eliminate all `Option`-unwraps.
///
/// # Lifecycle
///
/// ```text
/// New → Initiated → ValidationPassed → AperakSent → Active
///                                    ↘ Rejected
///     ↘ Rejected (failed validation at Initiated step)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GasSupplierChangeState {
    /// No events yet; stream exists but process has not started.
    New,
    /// UTILMD G received and `Initiated` event applied.
    Initiated(GasSupplierChangeData),
    /// EDIFACT validation passed; APERAK not yet dispatched.
    ValidationPassed(GasSupplierChangeData),
    /// Positive APERAK dispatched; awaiting supply activation.
    AperakSent(GasSupplierChangeData),
    /// Gas supply relationship is active.
    Active(GasSupplierChangeData),
    /// Process rejected (validation failure or negative APERAK).
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl Default for GasSupplierChangeState {
    fn default() -> Self {
        Self::New
    }
}

impl GasSupplierChangeState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Initiated(_) => "Initiated",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AperakSent(_) => "AperakSent",
            Self::Active(_) => "Active",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas supplier-change workflow.
///
/// **All domain values must be pre-extracted by the transport layer** before
/// constructing a command. `Workflow::handle()` is pure — no I/O, no EDIFACT
/// parsing, no external calls. See the crate-level doc for a construction
/// example.
#[derive(Clone)]
pub enum GasSupplierChangeCommand {
    /// Inbound UTILMD G accepted from the AS4 layer. Domain fields extracted
    /// and validation performed by the caller before constructing this command.
    ReceiveUtilmd {
        /// BDEW Prüfidentifikator.
        pid: Pruefidentifikator,
        /// GLN of the message sender (nLFN).
        sender: MarktpartnerCode,
        /// GLN of the message receiver (GNB).
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if `msg.validate()` returned a report with no errors.
        validation_passed: bool,
        /// Human-readable validation issue strings for the `Rejected` event.
        validation_errors: Vec<String>,
    },
    /// Dispatch a positive or negative APERAK.
    ///
    /// **BDEW GeLi Gas / BNetzA BK7**: APERAK must be sent within
    /// **10 Werktage** of receiving the UTILMD G (not wall-clock hours).
    /// Use `fristen::add_werktage(10, HolidayCalendar::BdewMaKo)` to compute
    /// the deadline.
    DispatchAperak {
        /// `true` for positive APERAK, `false` for negative.
        positive: bool,
        /// Rejection reason (only set when `positive = false`).
        reason: Option<String>,
    },
    /// Mark the gas supply relationship as active after all checks pass.
    Activate,
    /// A registered deadline fired and was dispatched by the scheduler.
    ///
    /// Transitions the process to `Rejected` unless it has already reached
    /// a terminal state (`Active` or `Rejected`), in which case this is a no-op.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for GasSupplierChangeCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// PIDs handled by this module (UTILMD G — gas Lieferantenwechsel, GeLi Gas 2.0/3.0).
///
/// These are the BDEW EDI@Energy UTILMD Gas `Prüfidentifikator` values
/// as defined in the GeLi Gas AHB profiles per `docs/pid-reference.md`.
///
/// **PIDs 44022–44024** (WiM Gas Stornierung) are NOT registered here \u2014
/// they belong to `mako-wim-gas` per `docs/pid-reference.md`.
pub const UTILMD_PIDS: &[u32] = &[
    44001, 44002, 44003, 44004, 44005, 44006, // Lieferbeginn / Lieferende (LFN ↔ NB)
    44007, 44008, 44009, // Abmeldung NN vom NB (NB → LFN)
    44010, 44011, 44012, // Abmeldungsanfrage des NB (NB → LFN)
    44013, 44014, 44015, // Anmeldung/Abmeldung EoG
    44016, // Kündigung beim alten Lieferanten
    44017, 44018, // Kündigung Lieferbeginn (LFN ↔ LFA)
    44019, 44020, 44021, // Bestandsliste / Änderungsmeldung
];

/// GeLi Gas supplier-change workflow (PIDs 44001–44006, 44017–44018).
///
/// Implements the BDEW GeLi Gas process for supplier change at a
/// Marktlokation (MaLo). The gas grid operator (GNB) receives a UTILMD G
/// Anmeldung from the new supplier and must respond with an APERAK within
/// **10 Werktage**.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GeliGasSupplierChangeWorkflow>(
///     tenant_id,
///     WorkflowId::new("geli-gas-supplier-change", "FV2025-10-01"),
/// );
/// ```
pub struct GeliGasSupplierChangeWorkflow;

impl Workflow for GeliGasSupplierChangeWorkflow {
    type State = GasSupplierChangeState;
    type Event = GasSupplierChangeEvent;
    type Command = GasSupplierChangeCommand;

    /// Deadline compensation for the GeLi Gas 10-Werktage APERAK window.
    ///
    /// | Label | State guard | Command emitted | Regulatory basis |
    /// |---|---|---|---|
    /// | `"geli-gas-aperak-10-werktage"` | `Initiated`, `ValidationPassed`, or `AperakSent` | `TimeoutExpired` | GeLi Gas — 10 Werktage APERAK Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                APERAK_WINDOW_LABEL,
                GasSupplierChangeState::Initiated(_)
                | GasSupplierChangeState::ValidationPassed(_)
                | GasSupplierChangeState::AperakSent(_),
            ) => Some(GasSupplierChangeCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GasSupplierChangeEvent::Initiated {
                malo_id,
                new_supplier,
                gas_operator,
                document_date,
                message_ref,
                pruefidentifikator,
            } => GasSupplierChangeState::Initiated(GasSupplierChangeData {
                malo_id: malo_id.clone(),
                new_supplier: new_supplier.clone(),
                gas_operator: gas_operator.clone(),
                document_date: document_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: Some(message_ref.clone()),
            }),
            GasSupplierChangeEvent::ValidationPassed { .. } => {
                if let GasSupplierChangeState::Initiated(data) = state {
                    GasSupplierChangeState::ValidationPassed(data)
                } else {
                    state
                }
            }
            GasSupplierChangeEvent::AperakDispatched { positive, .. } => match state {
                GasSupplierChangeState::ValidationPassed(data) => {
                    if *positive {
                        GasSupplierChangeState::AperakSent(data)
                    } else {
                        GasSupplierChangeState::Rejected {
                            reason: "negative APERAK".to_owned(),
                        }
                    }
                }
                _ => state,
            },
            GasSupplierChangeEvent::Activated => {
                if let GasSupplierChangeState::AperakSent(data) = state {
                    GasSupplierChangeState::Active(data)
                } else {
                    state
                }
            }
            GasSupplierChangeEvent::Rejected { reason } => GasSupplierChangeState::Rejected {
                reason: reason.clone(),
            },
            GasSupplierChangeEvent::DeadlineExpired { label, .. } => match state {
                GasSupplierChangeState::Active(_) | GasSupplierChangeState::Rejected { .. } => {
                    state
                }
                _ => GasSupplierChangeState::Rejected {
                    reason: format!("deadline expired: {label}"),
                },
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            GasSupplierChangeCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                malo_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GasSupplierChangeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                // PID guard — domain rule, no I/O required.
                if !UTILMD_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "unsupported GeLi Gas PID {pid} (expected one of: {UTILMD_PIDS:?})",
                    )));
                }
                let mut events = vec![GasSupplierChangeEvent::Initiated {
                    malo_id,
                    new_supplier: sender,
                    gas_operator: receiver,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(GasSupplierChangeEvent::ValidationPassed { message_ref });
                } else {
                    events.push(GasSupplierChangeEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            GasSupplierChangeCommand::DispatchAperak { positive, reason } => {
                let data = match state {
                    GasSupplierChangeState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.status_str(),
                        ));
                    }
                };
                let mut payload = serde_json::json!({
                    "pid":          data.pruefidentifikator.as_u32(),
                    "malo":         data.malo_id.as_str(),
                    "new_supplier": data.new_supplier.as_str(),
                    "gas_operator": data.gas_operator.as_str(),
                    "positive":     positive,
                });
                if let Some(ref mr) = data.message_ref {
                    payload["orig_message_ref"] = serde_json::Value::String(mr.as_str().to_owned());
                }
                if let Some(ref r) = reason {
                    payload["reason"] = serde_json::Value::String(r.clone());
                }
                let outbox_entry =
                    PendingOutbox::new("Aperak", data.new_supplier.as_str(), payload);
                Ok(WorkflowOutput::with_outbox(
                    vec![GasSupplierChangeEvent::AperakDispatched { positive, reason }],
                    vec![outbox_entry],
                ))
            }

            GasSupplierChangeCommand::Activate => {
                if !matches!(state, GasSupplierChangeState::AperakSent(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AperakSent",
                        state.status_str(),
                    ));
                }
                Ok(vec![GasSupplierChangeEvent::Activated].into())
            }

            GasSupplierChangeCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    GasSupplierChangeState::Active(_) | GasSupplierChangeState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }

                // Compensation: enqueue an AperakTimeout outbox entry so the
                // OutboxErpWorker notifies the ERP that no APERAK was received
                // within the 10-Werktage regulatory window (BK7 GeLi Gas).
                // Persisted atomically with DeadlineExpired via WriteBatch.
                let mut outbox: Vec<PendingOutbox> = vec![];
                let data_opt = match &state {
                    GasSupplierChangeState::Initiated(d)
                    | GasSupplierChangeState::ValidationPassed(d)
                    | GasSupplierChangeState::AperakSent(d) => Some(d),
                    _ => None,
                };
                if let Some(data) = data_opt {
                    outbox.push(PendingOutbox::new(
                        "AperakTimeout",
                        data.new_supplier.as_str(),
                        serde_json::json!({
                            "pid":          data.pruefidentifikator.as_u32(),
                            "malo":         data.malo_id.as_str(),
                            "new_supplier": data.new_supplier.as_str(),
                            "gas_operator": data.gas_operator.as_str(),
                            "deadline_label": label.as_ref(),
                            "deadline_id":  deadline_id,
                        }),
                    ));
                }

                let event = GasSupplierChangeEvent::DeadlineExpired { deadline_id, label };
                if outbox.is_empty() {
                    Ok(vec![event].into())
                } else {
                    Ok(WorkflowOutput::with_outbox(vec![event], outbox))
                }
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single GeLi Gas supplier-change process stream.
///
/// Uses a type-state design so field access never requires `Option::unwrap`:
/// the `Active` variant carries all domain fields that are structurally
/// guaranteed to exist once the process moves past `New`.
#[derive(Debug)]
pub enum GasSupplierChangeRecord {
    /// No `Initiated` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `Initiated` event applied; process fields now available.
    Active {
        /// Current lifecycle stage.
        status: &'static str,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// GLN of the new gas supplier.
        new_supplier: MarktpartnerCode,
        /// GLN of the gas network operator.
        gas_operator: MarktpartnerCode,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// Total events applied.
        event_count: usize,
    },
}

impl GasSupplierChangeRecord {
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
    pub fn active_data(&self) -> Option<GasSupplierChangeRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                malo_id,
                new_supplier,
                gas_operator,
                pruefidentifikator,
                ..
            } => Some(GasSupplierChangeRecordData {
                malo_id,
                new_supplier,
                gas_operator,
                pruefidentifikator,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` [`GasSupplierChangeRecord`].
#[derive(Debug, Clone, Copy)]
pub struct GasSupplierChangeRecordData<'a> {
    /// Marktlokation EIC code.
    pub malo_id: &'a MaLo,
    /// GLN of the new gas supplier.
    pub new_supplier: &'a MarktpartnerCode,
    /// GLN of the gas network operator.
    pub gas_operator: &'a MarktpartnerCode,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: &'a Pruefidentifikator,
}

impl Default for GasSupplierChangeRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model that tracks status across all GeLi Gas
/// supplier-change streams. Feed via [`mako_engine::projection::ProjectionRunner`].
#[derive(Debug, Default)]
pub struct GasSupplierChangeProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, GasSupplierChangeRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for GasSupplierChangeProjection {
    fn name(&self) -> &'static str {
        "GasSupplierChangeProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<GasSupplierChangeEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            GasSupplierChangeRecord::New { event_count }
            | GasSupplierChangeRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            GasSupplierChangeEvent::Initiated {
                malo_id,
                new_supplier,
                gas_operator,
                pruefidentifikator,
                ..
            } => {
                let count = record.event_count();
                *record = GasSupplierChangeRecord::Active {
                    status: "Initiated",
                    malo_id,
                    new_supplier,
                    gas_operator,
                    pruefidentifikator,
                    event_count: count,
                };
            }
            GasSupplierChangeEvent::ValidationPassed { .. } => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            GasSupplierChangeEvent::AperakDispatched { positive, .. } => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = if positive { "AperakSent" } else { "Rejected" };
                }
            }
            GasSupplierChangeEvent::Activated => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = "Active";
                }
            }
            GasSupplierChangeEvent::Rejected { .. } => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
            GasSupplierChangeEvent::DeadlineExpired { .. } => {
                if let GasSupplierChangeRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_receive_cmd(pid: u32, validation_passed: bool) -> GasSupplierChangeCommand {
        GasSupplierChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(pid).expect("test pid must be in range"),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            malo_id: MaLo::new("DE0000000001234567890000000000001"),
            document_date: "20250115".to_owned(),
            message_ref: MessageRef::new("MSG-GELI-001"),
            validation_passed,
            validation_errors: if validation_passed {
                vec![]
            } else {
                vec!["AHB rule violation".to_owned()]
            },
        }
    }

    #[test]
    fn happy_path_new_to_active() {
        let state = GasSupplierChangeState::default();

        let events = GeliGasSupplierChangeWorkflow::handle(&state, make_receive_cmd(44001, true))
            .expect("should accept valid PID 44001");
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], GasSupplierChangeEvent::Initiated { pruefidentifikator, .. } if pruefidentifikator.as_u32() == 44001)
        );
        assert!(matches!(
            &events[1],
            GasSupplierChangeEvent::ValidationPassed { .. }
        ));

        let state = events
            .iter()
            .fold(state, GeliGasSupplierChangeWorkflow::apply);
        assert!(
            matches!(&state, GasSupplierChangeState::ValidationPassed(_)),
            "expected ValidationPassed, got {}",
            state.status_str()
        );

        let events = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .expect("dispatch APERAK");
        let state = events
            .iter()
            .fold(state, GeliGasSupplierChangeWorkflow::apply);
        assert!(
            matches!(&state, GasSupplierChangeState::AperakSent(_)),
            "expected AperakSent"
        );

        let events =
            GeliGasSupplierChangeWorkflow::handle(&state, GasSupplierChangeCommand::Activate)
                .expect("activate");
        let state = events
            .iter()
            .fold(state, GeliGasSupplierChangeWorkflow::apply);
        assert!(
            matches!(&state, GasSupplierChangeState::Active(d) if d.malo_id == MaLo::new("DE0000000001234567890000000000001")),
            "expected Active with malo_id",
        );
    }

    #[test]
    fn wrong_pid_is_rejected() {
        let state = GasSupplierChangeState::default();
        let err = GeliGasSupplierChangeWorkflow::handle(&state, make_receive_cmd(55001, true))
            .expect_err("should reject wrong PID");
        let msg = err.to_string();
        assert!(
            msg.contains("55001"),
            "error should mention the unexpected PID: {msg}"
        );
    }

    #[test]
    fn validation_failure_rejects_process() {
        let state = GasSupplierChangeState::default();
        let events = GeliGasSupplierChangeWorkflow::handle(&state, make_receive_cmd(44001, false))
            .expect("should still produce events");
        assert!(matches!(
            &events[1],
            GasSupplierChangeEvent::Rejected { .. }
        ));
        let state = events
            .iter()
            .fold(state, GeliGasSupplierChangeWorkflow::apply);
        assert!(
            matches!(&state, GasSupplierChangeState::Rejected { .. }),
            "expected Rejected"
        );
    }

    #[test]
    fn dispatch_aperak_in_wrong_state_is_rejected() {
        let state = GasSupplierChangeState::default();
        let err = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .expect_err("should reject dispatch in wrong state");
        assert!(err.to_string().contains("ValidationPassed"), "{err}");
    }

    #[test]
    fn negative_aperak_sets_rejected_status() {
        let state = GasSupplierChangeState::default();
        let events =
            GeliGasSupplierChangeWorkflow::handle(&state, make_receive_cmd(44001, true)).unwrap();
        let state = events
            .iter()
            .fold(state, GeliGasSupplierChangeWorkflow::apply);

        let events = GeliGasSupplierChangeWorkflow::handle(
            &state,
            GasSupplierChangeCommand::DispatchAperak {
                positive: false,
                reason: Some("MaLo not found".to_owned()),
            },
        )
        .expect("dispatch negative APERAK");
        let state = events
            .iter()
            .fold(state, GeliGasSupplierChangeWorkflow::apply);
        assert!(
            matches!(&state, GasSupplierChangeState::Rejected { .. }),
            "expected Rejected after negative APERAK"
        );
    }
}
