//! GeLi Gas INVOIC billing for AWH Sperrprozesse Gas — PID 31011
//! (Rechnung sonstige Leistung, VNB → LFN/LFA).
//!
//! The gas network operator (GNB/VNB) sends an INVOIC to the supplier (LFN/LFA)
//! for services rendered during the gas disconnection/reconnection process
//! (AWH = Abrechnungswürdige Handlungen from Sperrprozesse Gas).
//!
//! This is a GeLi Gas (BK7-24-01-009) billing process — **not** GaBi Gas.
//! PID 31010 (Kapazitätsrechnung, VNB → BKV) is the GaBi Gas capacity invoice
//! and belongs to `mako-gabi-gas`.
//!
//! # Covered Prüfidentifikatoren (INVOIC AHB / FV2025-10-01, BK7-24-01-009)
//!
//! | PID   | Process                                            | Direction     |
//! |-------|----------------------------------------------------|---------------|
//! | 31011 | Rechnung sonstige Leistung (AWH Sperrprozesse Gas) | VNB → LFN/LFA |
//!
//! # State machine
//!
//! ```text
//! New ──ReceiveInvoic──► InvoicReceived ──[valid]──► ValidationPassed
//!                                        ╰──[invalid]──► Rejected
//! ValidationPassed ──SettleInvoice──► Settled
//!                  ╰─DisputeInvoice──► Disputed
//! Any active state ──TimeoutExpired──► Rejected
//! ```
//!
//! # Regulatory basis
//!
//! - **BK7-24-01-009** — GeLi Gas 3.0 ruling (Beschluss 12.09.2025)
//! - **INVOIC AHB** — EDI@Energy invoice message format
//! - **GasNZV** — Gasnetzzugangsverordnung (statutory basis for gas network access)

use std::collections::HashMap;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    projection::Projection,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constant ──────────────────────────────────────────────────────────────

/// GeLi Gas AWH billing PID handled by this workflow (INVOIC AHB).
///
/// | PID   | Name                                                               |
/// |-------|--------------------------------------------------------------------|
/// | 31011 | Rechnung sonstige Leistung (AWH Sperrprozesse Gas, VNB → LFN/LFA) |
pub const SPERRPROZESSE_INVOIC_PID: u32 = 31011;

/// Workflow key used for PID router registration.
pub const WORKFLOW_NAME: &str = "geli-gas-sperrprozesse-invoic";

/// Deadline label for the GeLi Gas AWH INVOIC settlement response window.
///
/// Register a [`mako_engine::deadline::Deadline`] with this label immediately
/// after the `ValidationPassed` event fires so the workflow can enforce a
/// contractual settlement deadline.
pub const SETTLEMENT_WINDOW_LABEL: &str = "geli-gas-sperrprozesse-invoic-settlement";

// ── Data ──────────────────────────────────────────────────────────────────────

/// Business data captured when the GeLi Gas AWH INVOIC is first received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliGasSperrprozesseInvoicData {
    /// BDEW Prüfidentifikator (always 31011).
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the invoice issuer (GNB/VNB, the gas network operator).
    pub sender: MarktpartnerCode,
    /// GLN of the invoice recipient (LFN/LFA, the gas supplier).
    pub recipient: MarktpartnerCode,
    /// EDIFACT document date string from BGM/DTM (YYYYMMDD).
    pub document_date: String,
    /// Invoice reference number from UNH/BGM.
    pub invoice_ref: MessageRef,
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Current state of a GeLi Gas AWH Sperrprozesse INVOIC billing process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GeliGasSperrprozesseInvoicState {
    /// No INVOIC received yet.
    New,
    /// INVOIC received from the GNB/VNB; AHB validation pending or in progress.
    InvoicReceived(GeliGasSperrprozesseInvoicData),
    /// INVOIC passed AHB validation; awaiting LFN/LFA settlement or dispute.
    ValidationPassed(GeliGasSperrprozesseInvoicData),
    /// Invoice settled — positive CONTRL dispatched to the issuer (GNB/VNB).
    Settled(GeliGasSperrprozesseInvoicData),
    /// Invoice disputed — negative CONTRL or APERAK dispatched to the issuer.
    Disputed {
        /// Billing data captured at the time of the dispute.
        data: GeliGasSperrprozesseInvoicData,
        /// Human-readable reason for the dispute.
        reason: String,
    },
    /// Process rejected due to AHB validation failure, duplicate, or deadline.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl Default for GeliGasSperrprozesseInvoicState {
    fn default() -> Self {
        Self::New
    }
}

impl GeliGasSperrprozesseInvoicState {
    /// Stable string label for the current variant (used in error messages).
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::InvoicReceived(_) => "InvoicReceived",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Settled(_) => "Settled",
            Self::Disputed { .. } => "Disputed",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas AWH Sperrprozesse INVOIC billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeliGasSperrprozesseInvoicEvent {
    /// INVOIC received from the gas network operator (GNB/VNB).
    InvoicReceived {
        /// EDIFACT message reference of the inbound INVOIC.
        invoice_ref: MessageRef,
        /// GLN of the invoice issuer (GNB/VNB).
        sender: MarktpartnerCode,
        /// GLN of the invoice recipient (LFN/LFA).
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// BDEW Prüfidentifikator (always 31011 for this workflow).
        pruefidentifikator: Pruefidentifikator,
    },
    /// INVOIC passed AHB profile validation — no rule violations found.
    ///
    /// The settlement deadline ([`SETTLEMENT_WINDOW_LABEL`]) should be
    /// registered immediately after this event is persisted.
    ValidationPassed {
        /// Reference of the validated INVOIC message.
        invoice_ref: MessageRef,
    },
    /// Invoice settled — positive CONTRL dispatched to the GNB/VNB issuer.
    InvoiceSettled,
    /// Invoice disputed — negative CONTRL or APERAK dispatched to the issuer.
    InvoiceDisputed {
        /// Human-readable dispute reason.
        reason: String,
    },
    /// INVOIC rejected immediately due to AHB validation failure.
    ///
    /// A negative CONTRL with the relevant error code is enqueued.
    Rejected {
        /// Human-readable rejection reason (from AHB validation issues).
        reason: String,
    },
    /// Settlement deadline expired before the LFN/LFA issued a response.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GeliGasSperrprozesseInvoicEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::InvoicReceived { .. } => "GeliGasSperrprozesseInvoicReceived",
            Self::ValidationPassed { .. } => "GeliGasSperrprozesseInvoicValidationPassed",
            Self::InvoiceSettled => "GeliGasSperrprozesseInvoicSettled",
            Self::InvoiceDisputed { .. } => "GeliGasSperrprozesseInvoicDisputed",
            Self::Rejected { .. } => "GeliGasSperrprozesseInvoicRejected",
            Self::DeadlineExpired { .. } => "GeliGasSperrprozesseInvoicDeadlineExpired",
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands accepted by the GeLi Gas AWH Sperrprozesse INVOIC billing workflow.
#[derive(Debug, Clone)]
pub enum GeliGasSperrprozesseInvoicCommand {
    /// Receive an inbound INVOIC 31011 from a gas network operator (GNB/VNB).
    ///
    /// The transport/adapter layer is responsible for parsing the EDIFACT
    /// message and running AHB validation **before** constructing this command.
    /// Pass `validation_passed: false` and populate `validation_errors` if the
    /// AHB check found rule violations; the workflow will emit `Rejected` and
    /// enqueue a negative CONTRL.
    ReceiveInvoic {
        /// BDEW Prüfidentifikator (must be 31011 for this workflow).
        pid: Pruefidentifikator,
        /// GLN of the sender (GNB/VNB, the gas network operator).
        sender: MarktpartnerCode,
        /// GLN of the recipient (LFN/LFA, the gas supplier).
        recipient: MarktpartnerCode,
        /// EDIFACT message reference from the UNH segment.
        invoice_ref: MessageRef,
        /// EDIFACT document date extracted from BGM/DTM (YYYYMMDD).
        document_date: String,
        /// `true` if AHB profile validation found no errors.
        validation_passed: bool,
        /// Human-readable validation issue strings (empty when `validation_passed`).
        validation_errors: Vec<String>,
    },
    /// Settle the invoice — dispatch a positive CONTRL to the GNB/VNB issuer.
    SettleInvoice,
    /// Dispute the invoice — dispatch a negative CONTRL or APERAK to the issuer.
    DisputeInvoice {
        /// Human-readable reason for the dispute.
        reason: String,
    },
    /// The settlement deadline fired before the LFN/LFA issued a response.
    ///
    /// Fired by the `DeadlineScheduler` when the
    /// `geli-gas-sperrprozesse-invoic-settlement` deadline expires.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl CommandPayload for GeliGasSperrprozesseInvoicCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas AWH Sperrprozesse INVOIC billing workflow (PID 31011).
///
/// Implements the LFN/LFA-side receive → validate → settle/dispute state
/// machine for billing of AWH (Abrechnungswürdige Handlungen) arising from
/// gas disconnection/reconnection processes under BK7-24-01-009.
///
/// # Deadline
///
/// Register a deadline with label [`SETTLEMENT_WINDOW_LABEL`] after the
/// `ValidationPassed` event fires.
pub struct GeliGasSperrprozesseInvoicWorkflow;

impl Workflow for GeliGasSperrprozesseInvoicWorkflow {
    type State = GeliGasSperrprozesseInvoicState;
    type Event = GeliGasSperrprozesseInvoicEvent;
    type Command = GeliGasSperrprozesseInvoicCommand;

    /// Deadline compensation for the GeLi Gas AWH INVOIC settlement window.
    ///
    /// | Label | State guard | Command emitted |
    /// |---|---|---|
    /// | `"geli-gas-sperrprozesse-invoic-settlement"` | `InvoicReceived` or `ValidationPassed` | `TimeoutExpired` |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                SETTLEMENT_WINDOW_LABEL,
                GeliGasSperrprozesseInvoicState::InvoicReceived(_)
                | GeliGasSperrprozesseInvoicState::ValidationPassed(_),
            ) => Some(GeliGasSperrprozesseInvoicCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GeliGasSperrprozesseInvoicEvent::InvoicReceived {
                invoice_ref,
                sender,
                recipient,
                document_date,
                pruefidentifikator,
            } => GeliGasSperrprozesseInvoicState::InvoicReceived(GeliGasSperrprozesseInvoicData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                recipient: recipient.clone(),
                document_date: document_date.clone(),
                invoice_ref: invoice_ref.clone(),
            }),

            GeliGasSperrprozesseInvoicEvent::ValidationPassed { .. } => match state {
                GeliGasSperrprozesseInvoicState::InvoicReceived(data) => {
                    GeliGasSperrprozesseInvoicState::ValidationPassed(data)
                }
                other => other,
            },

            GeliGasSperrprozesseInvoicEvent::InvoiceSettled => match state {
                GeliGasSperrprozesseInvoicState::ValidationPassed(data) => {
                    GeliGasSperrprozesseInvoicState::Settled(data)
                }
                other => other,
            },

            GeliGasSperrprozesseInvoicEvent::InvoiceDisputed { reason } => match state {
                GeliGasSperrprozesseInvoicState::ValidationPassed(data) => {
                    GeliGasSperrprozesseInvoicState::Disputed {
                        data,
                        reason: reason.clone(),
                    }
                }
                other => other,
            },

            GeliGasSperrprozesseInvoicEvent::Rejected { reason } => {
                GeliGasSperrprozesseInvoicState::Rejected {
                    reason: reason.clone(),
                }
            }

            GeliGasSperrprozesseInvoicEvent::DeadlineExpired { label, .. } => match state {
                // Terminal states — deadline expiry does not overwrite a completed process.
                GeliGasSperrprozesseInvoicState::Settled(_)
                | GeliGasSperrprozesseInvoicState::Disputed { .. }
                | GeliGasSperrprozesseInvoicState::Rejected { .. } => state,
                _ => GeliGasSperrprozesseInvoicState::Rejected {
                    reason: format!("settlement deadline expired: {label}"),
                },
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            GeliGasSperrprozesseInvoicCommand::ReceiveInvoic {
                pid,
                sender,
                recipient,
                invoice_ref,
                document_date,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GeliGasSperrprozesseInvoicState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if pid.as_u32() != SPERRPROZESSE_INVOIC_PID {
                    return Err(WorkflowError::rejected(format!(
                        "expected GeLi Gas AWH INVOIC PID {SPERRPROZESSE_INVOIC_PID}, got {pid}",
                    )));
                }
                let mut events = vec![GeliGasSperrprozesseInvoicEvent::InvoicReceived {
                    invoice_ref: invoice_ref.clone(),
                    sender,
                    recipient,
                    document_date,
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(GeliGasSperrprozesseInvoicEvent::ValidationPassed { invoice_ref });
                } else {
                    events.push(GeliGasSperrprozesseInvoicEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            GeliGasSperrprozesseInvoicCommand::SettleInvoice => {
                if !matches!(state, GeliGasSperrprozesseInvoicState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![GeliGasSperrprozesseInvoicEvent::InvoiceSettled].into())
            }

            GeliGasSperrprozesseInvoicCommand::DisputeInvoice { reason } => {
                if !matches!(state, GeliGasSperrprozesseInvoicState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![GeliGasSperrprozesseInvoicEvent::InvoiceDisputed { reason }].into())
            }

            GeliGasSperrprozesseInvoicCommand::TimeoutExpired { deadline_id, label } => {
                // Idempotent in terminal states — the deadline may fire just
                // after the LFN/LFA dispatched a response.
                if matches!(
                    state,
                    GeliGasSperrprozesseInvoicState::Settled(_)
                        | GeliGasSperrprozesseInvoicState::Disputed { .. }
                        | GeliGasSperrprozesseInvoicState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(
                    vec![GeliGasSperrprozesseInvoicEvent::DeadlineExpired { deadline_id, label }]
                        .into(),
                )
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single GeLi Gas AWH INVOIC billing process stream.
#[derive(Debug)]
pub struct GeliGasSperrprozesseInvoicRecord {
    /// Current lifecycle status label.
    pub status: &'static str,
    /// BDEW Prüfidentifikator once the INVOIC is received.
    pub pruefidentifikator: Option<Pruefidentifikator>,
    /// Total events processed for this stream.
    pub event_count: usize,
}

impl Default for GeliGasSperrprozesseInvoicRecord {
    fn default() -> Self {
        Self {
            status: "New",
            pruefidentifikator: None,
            event_count: 0,
        }
    }
}

/// In-process read model tracking all GeLi Gas AWH INVOIC billing process streams.
#[derive(Debug, Default)]
pub struct GeliGasSperrprozesseInvoicProjection {
    /// All known billing process records keyed by stream ID.
    pub records: HashMap<String, GeliGasSperrprozesseInvoicRecord>,
    /// Sequence number of the last event applied.
    pub last_seq: u64,
}

impl Projection for GeliGasSperrprozesseInvoicProjection {
    fn name(&self) -> &'static str {
        "GeliGasSperrprozesseInvoicProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        let Ok(event) = envelope.decode::<GeliGasSperrprozesseInvoicEvent>() else {
            return;
        };
        record.status = event.event_type();
        if let GeliGasSperrprozesseInvoicEvent::InvoicReceived {
            pruefidentifikator, ..
        } = event
        {
            record.pruefidentifikator = Some(pruefidentifikator);
        }
    }
}
