//! WiM Gas INVOIC billing — PIDs 31003 (WiM-Rechnung) and 31004 (Stornorechnung).
//!
//! Handles INVOIC-based billing processes in the WiM Gas market domain where the
//! new metering service provider (gMSB) submits invoices to the grid operator (NB).
//!
//! # Covered Prüfidentifikatoren (INVOIC AHB / FV2025-10-01, BK7-24-01-009)
//!
//! | PID   | Process                           | Direction  |
//! |-------|-----------------------------------|------------|
//! | 31003 | WiM-Rechnung (MSB-Gerätewechsel)  | gMSB → NB  |
//! | 31004 | Stornorechnung WiM Gas            | gMSB → NB  |
//!
//! # State machine
//!
//! ```text
//! New ──ReceiveInvoic──► InvoicReceived ──[valid]──► ValidationPassed
//!                                      ╰──[invalid]──► Rejected
//! ValidationPassed ──SettleInvoice──► Settled
//!                  ╰─DisputeInvoice──► Disputed
//! Any active state ──TimeoutExpired──► Rejected
//! ```
//!
//! # Regulatory basis
//!
//! - **BK7-24-01-009** — WiM Gas process framework (Beschluss 12.09.2025)
//! - **INVOIC AHB 1.0** — EDI@Energy invoice message format (valid FV2025-10-01)
//! - **CONTRL deadline** — 10 Werktage per BK7-24-01-009 §5
//! - **APERAK deadline** — 10 Werktage per BK7-24-01-009 §5

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

// ── PID set ───────────────────────────────────────────────────────────────────

/// WiM Gas billing Prüfidentifikatoren handled by this workflow (INVOIC AHB 1.0).
///
/// | PID   | Name                                      |
/// |-------|-------------------------------------------|
/// | 31003 | WiM-Rechnung (gMSB → NB, Gerätewechsel)   |
/// | 31004 | Stornorechnung WiM Gas (gMSB → NB)        |
pub const WIM_GAS_INVOIC_PIDS: &[u32] = &[
    31003, // WiM-Rechnung (gMSB → NB)
    31004, // Stornorechnung WiM Gas (gMSB → NB)
];

/// Workflow key used for PID router registration.
pub const WORKFLOW_NAME: &str = "wim-gas-invoic";

/// Deadline label for the WiM Gas INVOIC settlement response window.
///
/// Per BK7-24-01-009 §5, the NB must settle or dispute an inbound INVOIC within
/// **10 Werktage** of receipt. Register a [`mako_engine::deadline::Deadline`]
/// with this label immediately after the `ValidationPassed` event.
pub const SETTLEMENT_WINDOW_LABEL: &str = "wim-gas-invoic-settlement-deadline";

/// REMADV PIDs for WiM Gas billing (inbound Zahlungsavis, gMSB invoicer role).
///
/// After the gMSB sends INVOIC 31003/31004, the NB (payer) sends REMADV back.
/// The gMSB receives these inbound. Without registration, REMADV is silently
/// dropped by the AS4 ingest layer, breaking the WiM Gas billing cycle.
///
/// Source: REMADV AHB 1.0, WiM Gas, BK7-24-01-009.
pub const WIM_GAS_REMADV_PIDS: &[u32] = &[33001, 33002];

/// COMDIS PID for WiM Gas billing (inbound Ablehnung REMADV, NB payer role).
///
/// After the NB sends REMADV, the gMSB (invoicer) can reject it via COMDIS 29001.
///
/// Source: COMDIS AHB 1.0, WiM Gas, BK7-24-01-009.
pub const WIM_GAS_COMDIS_ABLEHNUNG_PID: u32 = 29001;

// ── Data ──────────────────────────────────────────────────────────────────────

/// Business data captured when the INVOIC is first received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WimGasInvoicData {
    /// BDEW Prüfidentifikator (31003 = WiM-Rechnung, 31004 = Stornorechnung).
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the billing party (gMSB / sender).
    pub sender: MarktpartnerCode,
    /// GLN of the receiving party (NB / recipient).
    pub recipient: MarktpartnerCode,
    /// EDIFACT document date string from BGM/DTM (YYYYMMDD).
    pub document_date: String,
    /// Invoice reference number from UNH/BGM.
    pub invoice_ref: MessageRef,
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Current state of a WiM Gas INVOIC billing process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum WimGasInvoicState {
    /// No INVOIC received yet.
    New,
    /// INVOIC received; AHB validation pending or in progress.
    InvoicReceived(WimGasInvoicData),
    /// INVOIC passed AHB validation; awaiting NB settlement or dispute.
    ValidationPassed(WimGasInvoicData),
    /// Invoice settled — positive CONTRL dispatched to gMSB.
    Settled(WimGasInvoicData),
    /// Invoice disputed — negative CONTRL or APERAK dispatched to gMSB.
    Disputed {
        /// Billing data captured at the time of the dispute.
        data: WimGasInvoicData,
        /// Human-readable reason for the dispute (e.g. price discrepancy).
        reason: String,
    },
    /// Process rejected due to AHB validation failure, duplicate, or deadline.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// REMADV received — NB confirms payment (gMSB invoicer role).
    ///
    /// 33001 = full payment confirmed; 33002 = disputed.
    PaymentConfirmed(WimGasInvoicData),
    /// REMADV 33002 received — NB disputes the invoice amount.
    PaymentDisputed {
        /// Billing data.
        data: WimGasInvoicData,
        /// REMADV PID (33002).
        remadv_pid: Pruefidentifikator,
    },
    /// COMDIS 29001 received — gMSB rejects NB's REMADV (NB payer role).
    ComdisRejected(WimGasInvoicData),
}

impl Default for WimGasInvoicState {
    fn default() -> Self {
        Self::New
    }
}

impl WimGasInvoicState {
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
            Self::PaymentConfirmed(_) => "PaymentConfirmed",
            Self::PaymentDisputed { .. } => "PaymentDisputed",
            Self::ComdisRejected(_) => "ComdisRejected",
        }
    }
}

// New state variants are appended to the enum below via the Deserialize path;
// the enum itself is extended in the definition block above.

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the WiM Gas INVOIC billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimGasInvoicEvent {
    /// INVOIC received from gMSB.
    InvoicReceived {
        /// EDIFACT message reference of the inbound INVOIC.
        invoice_ref: MessageRef,
        /// GLN of the billing party (gMSB / sender).
        sender: MarktpartnerCode,
        /// GLN of the receiving party (NB / recipient).
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// BDEW Prüfidentifikator (31003 or 31004).
        pruefidentifikator: Pruefidentifikator,
    },
    /// INVOIC passed AHB profile validation — no rule violations found.
    ///
    /// The settlement deadline (`wim-gas-invoic-settlement-deadline`) should
    /// be registered immediately after this event is persisted.
    ValidationPassed {
        /// Reference of the validated INVOIC message.
        invoice_ref: MessageRef,
    },
    /// Invoice settled — positive CONTRL dispatched to gMSB.
    InvoiceSettled,
    /// Invoice disputed — negative CONTRL or APERAK dispatched to gMSB.
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
    /// Settlement deadline expired before the NB issued a response.
    ///
    /// Per BK7-24-01-009 §5, the NB is obligated to respond within 10
    /// Werktage. A deadline expiry triggers a late-notice APERAK via the outbox.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// REMADV received (gMSB invoicer role — NB confirms or disputes payment).
    ///
    /// PID 33001 = full payment confirmed; 33002 = NB disputes the amount.
    RemadvReceived {
        /// REMADV Prüfidentifikator (33001 or 33002).
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// GLN of the REMADV sender (NB / payer).
        sender: MarktpartnerCode,
        /// `true` for 33001 (full payment confirmed).
        is_confirmed: bool,
    },
    /// COMDIS 29001 received — gMSB rejects NB's REMADV (NB payer role).
    ComdisAbLehnungReceived {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
}

impl EventPayload for WimGasInvoicEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::InvoicReceived { .. } => "WimGasInvoicReceived",
            Self::ValidationPassed { .. } => "WimGasInvoicValidationPassed",
            Self::InvoiceSettled => "WimGasInvoicSettled",
            Self::InvoiceDisputed { .. } => "WimGasInvoicDisputed",
            Self::Rejected { .. } => "WimGasInvoicRejected",
            Self::DeadlineExpired { .. } => "WimGasInvoicDeadlineExpired",
            Self::RemadvReceived { .. } => "WimGasInvoicRemadvReceived",
            Self::ComdisAbLehnungReceived { .. } => "WimGasInvoicComdisAbLehnungReceived",
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands accepted by the WiM Gas INVOIC billing workflow.
#[derive(Debug, Clone)]
pub enum WimGasInvoicCommand {
    /// Receive an inbound INVOIC from a WiM Gas market participant (gMSB).
    ///
    /// The transport/adapter layer is responsible for parsing the EDIFACT
    /// message and running AHB validation **before** constructing this command.
    /// Pass `validation_passed: false` and populate `validation_errors` if the
    /// AHB check found rule violations; the workflow will emit `Rejected` and
    /// enqueue a negative CONTRL.
    ReceiveInvoic {
        /// BDEW Prüfidentifikator (31003 = WiM-Rechnung, 31004 = Stornorechnung).
        pid: Pruefidentifikator,
        /// GLN of the sender (gMSB).
        sender: MarktpartnerCode,
        /// GLN of the recipient (NB).
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
    /// Settle the invoice — dispatch a positive CONTRL acknowledgement to gMSB.
    ///
    /// BK7-24-01-009 §5: the NB must respond within **10 Werktage** of receipt.
    SettleInvoice,
    /// Dispute the invoice — dispatch a negative CONTRL or APERAK to gMSB.
    ///
    /// BK7-24-01-009 §5: the NB must respond within **10 Werktage** of receipt.
    DisputeInvoice {
        /// Human-readable reason for the dispute (e.g. amount discrepancy).
        reason: String,
    },
    /// The settlement deadline fired before the NB issued a response.
    ///
    /// Fired by the `DeadlineScheduler` when the `wim-gas-invoic-settlement-deadline`
    /// deadline expires. The workflow emits `DeadlineExpired` and the outbox worker
    /// sends a late-notice APERAK to gMSB.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
    /// gMSB invoicer role: inbound REMADV received from the NB (payer).
    ///
    /// PIDs 33001–33002 (REMADV AHB 1.0, WiM Gas, BK7-24-01-009).
    ReceiveRemadv {
        /// REMADV Prüfidentifikator (33001 or 33002).
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// GLN of the REMADV sender (NB / payer).
        sender: MarktpartnerCode,
    },
    /// NB payer role: inbound COMDIS 29001 (gMSB rejects NB's REMADV).
    ///
    /// Source: COMDIS AHB 1.0, WiM Gas, BK7-24-01-009.
    ReceiveComdis {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
}

impl CommandPayload for WimGasInvoicCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Gas INVOIC billing workflow (PIDs 31003 and 31004).
///
/// Implements the complete NB-side receive → validate → settle/dispute state
/// machine for WiM Gas billing under BK7-24-01-009.
///
/// # Deadline
///
/// Register a deadline with label [`SETTLEMENT_WINDOW_LABEL`] after the
/// `ValidationPassed` event fires. Compute the due date with:
///
/// ```rust,ignore
/// use mako_engine::fristen::{self, HolidayCalendar};
/// let due = fristen::add_werktage(received_date, 10, HolidayCalendar::BdewMaKo);
/// ```
pub struct WimGasInvoicWorkflow;

impl Workflow for WimGasInvoicWorkflow {
    type State = WimGasInvoicState;
    type Event = WimGasInvoicEvent;
    type Command = WimGasInvoicCommand;

    /// Deadline compensation for the WiM Gas INVOIC settlement window.
    ///
    /// | Label | State guard | Command emitted | Rule |
    /// |---|---|---|---|
    /// | `"wim-gas-invoic-settlement-deadline"` | `InvoicReceived` or `ValidationPassed` | `TimeoutExpired` | BK7-24-01-009 §5 — 10 Werktage |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                SETTLEMENT_WINDOW_LABEL,
                WimGasInvoicState::InvoicReceived(_) | WimGasInvoicState::ValidationPassed(_),
            ) => Some(WimGasInvoicCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            WimGasInvoicEvent::InvoicReceived {
                invoice_ref,
                sender,
                recipient,
                document_date,
                pruefidentifikator,
            } => WimGasInvoicState::InvoicReceived(WimGasInvoicData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                recipient: recipient.clone(),
                document_date: document_date.clone(),
                invoice_ref: invoice_ref.clone(),
            }),

            WimGasInvoicEvent::ValidationPassed { .. } => match state {
                WimGasInvoicState::InvoicReceived(data) => {
                    WimGasInvoicState::ValidationPassed(data)
                }
                other => other,
            },

            WimGasInvoicEvent::InvoiceSettled => match state {
                WimGasInvoicState::ValidationPassed(data) => WimGasInvoicState::Settled(data),
                other => other,
            },

            WimGasInvoicEvent::InvoiceDisputed { reason } => match state {
                WimGasInvoicState::ValidationPassed(data) => WimGasInvoicState::Disputed {
                    data,
                    reason: reason.clone(),
                },
                other => other,
            },

            WimGasInvoicEvent::Rejected { reason } => WimGasInvoicState::Rejected {
                reason: reason.clone(),
            },

            WimGasInvoicEvent::DeadlineExpired { label, .. } => match state {
                // Terminal states — deadline expiry does not overwrite a completed process.
                WimGasInvoicState::Settled(_)
                | WimGasInvoicState::Disputed { .. }
                | WimGasInvoicState::Rejected { .. }
                | WimGasInvoicState::PaymentConfirmed(_)
                | WimGasInvoicState::PaymentDisputed { .. }
                | WimGasInvoicState::ComdisRejected(_) => state,
                _ => WimGasInvoicState::Rejected {
                    reason: format!("settlement deadline expired: {label}"),
                },
            },

            WimGasInvoicEvent::RemadvReceived {
                pid, is_confirmed, ..
            } => match state {
                WimGasInvoicState::Settled(data) | WimGasInvoicState::ValidationPassed(data) => {
                    if *is_confirmed {
                        WimGasInvoicState::PaymentConfirmed(data)
                    } else {
                        WimGasInvoicState::PaymentDisputed {
                            remadv_pid: *pid,
                            data,
                        }
                    }
                }
                other => other,
            },

            WimGasInvoicEvent::ComdisAbLehnungReceived { .. } => match state {
                WimGasInvoicState::ValidationPassed(data)
                | WimGasInvoicState::Settled(data)
                | WimGasInvoicState::PaymentConfirmed(data) => {
                    WimGasInvoicState::ComdisRejected(data)
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
            WimGasInvoicCommand::ReceiveInvoic {
                pid,
                sender,
                recipient,
                invoice_ref,
                document_date,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, WimGasInvoicState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !WIM_GAS_INVOIC_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a WiM Gas INVOIC PID (31003 or 31004), got {pid}",
                    )));
                }
                let mut events = vec![WimGasInvoicEvent::InvoicReceived {
                    invoice_ref: invoice_ref.clone(),
                    sender,
                    recipient,
                    document_date,
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(WimGasInvoicEvent::ValidationPassed { invoice_ref });
                } else {
                    events.push(WimGasInvoicEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            WimGasInvoicCommand::SettleInvoice => {
                if !matches!(state, WimGasInvoicState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![WimGasInvoicEvent::InvoiceSettled].into())
            }

            WimGasInvoicCommand::DisputeInvoice { reason } => {
                if !matches!(state, WimGasInvoicState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![WimGasInvoicEvent::InvoiceDisputed { reason }].into())
            }

            WimGasInvoicCommand::TimeoutExpired { deadline_id, label } => {
                // Idempotent in terminal states — the deadline may fire just
                // after the NB dispatched a response.
                if matches!(
                    state,
                    WimGasInvoicState::Settled(_)
                        | WimGasInvoicState::Disputed { .. }
                        | WimGasInvoicState::Rejected { .. }
                        | WimGasInvoicState::PaymentConfirmed(_)
                        | WimGasInvoicState::PaymentDisputed { .. }
                        | WimGasInvoicState::ComdisRejected(_)
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![WimGasInvoicEvent::DeadlineExpired { deadline_id, label }].into())
            }

            WimGasInvoicCommand::ReceiveRemadv {
                pid,
                remadv_ref,
                sender,
            } => {
                if !matches!(
                    state,
                    WimGasInvoicState::Settled(_) | WimGasInvoicState::ValidationPassed(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "Settled|ValidationPassed",
                        state.label(),
                    ));
                }
                if !WIM_GAS_REMADV_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a WiM Gas REMADV PID (33001 or 33002), got {pid}",
                    )));
                }
                let is_confirmed = pid.as_u32() == 33001;
                Ok(vec![WimGasInvoicEvent::RemadvReceived {
                    pid,
                    remadv_ref,
                    sender,
                    is_confirmed,
                }]
                .into())
            }

            WimGasInvoicCommand::ReceiveComdis { comdis_ref } => {
                if matches!(
                    state,
                    WimGasInvoicState::New
                        | WimGasInvoicState::InvoicReceived(_)
                        | WimGasInvoicState::Rejected { .. }
                        | WimGasInvoicState::ComdisRejected(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed|Settled|PaymentConfirmed",
                        state.label(),
                    ));
                }
                Ok(vec![WimGasInvoicEvent::ComdisAbLehnungReceived { comdis_ref }].into())
            }
        }
    }
}

// ── Read-model projection ──────────────────────────────────────────────────────

/// Read-model record for a single WiM Gas INVOIC billing process stream.
#[derive(Debug)]
pub struct WimGasInvoicRecord {
    /// Current lifecycle status label.
    pub status: &'static str,
    /// BDEW Prüfidentifikator once the INVOIC is received.
    pub pruefidentifikator: Option<Pruefidentifikator>,
    /// Total events processed for this stream.
    pub event_count: usize,
}

impl Default for WimGasInvoicRecord {
    fn default() -> Self {
        Self {
            status: "New",
            pruefidentifikator: None,
            event_count: 0,
        }
    }
}

/// In-process read model tracking all WiM Gas INVOIC billing process streams.
#[derive(Debug, Default)]
pub struct WimGasInvoicProjection {
    /// All known billing process records keyed by stream ID.
    pub records: HashMap<String, WimGasInvoicRecord>,
    /// Sequence number of the last event applied.
    pub last_seq: u64,
}

impl Projection for WimGasInvoicProjection {
    fn name(&self) -> &'static str {
        "WimGasInvoicProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        let Ok(event) = envelope.decode::<WimGasInvoicEvent>() else {
            return;
        };

        match event {
            WimGasInvoicEvent::InvoicReceived {
                pruefidentifikator, ..
            } => {
                record.status = "InvoicReceived";
                record.pruefidentifikator = Some(pruefidentifikator);
            }
            WimGasInvoicEvent::ValidationPassed { .. } => {
                record.status = "ValidationPassed";
            }
            WimGasInvoicEvent::InvoiceSettled => {
                record.status = "Settled";
            }
            WimGasInvoicEvent::InvoiceDisputed { .. } => {
                record.status = "Disputed";
            }
            WimGasInvoicEvent::Rejected { .. } => {
                record.status = "Rejected";
            }
            WimGasInvoicEvent::DeadlineExpired { .. } => {
                record.status = "Rejected";
            }
            WimGasInvoicEvent::RemadvReceived { is_confirmed, .. } => {
                record.status = if is_confirmed {
                    "PaymentConfirmed"
                } else {
                    "PaymentDisputed"
                };
            }
            WimGasInvoicEvent::ComdisAbLehnungReceived { .. } => {
                record.status = "ComdisRejected";
            }
        }
    }

    fn last_sequence(&self) -> Option<u64> {
        if self.last_seq == 0 {
            None
        } else {
            Some(self.last_seq)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).expect("valid PID")
    }

    fn receive_cmd(p: u32, valid: bool) -> WimGasInvoicCommand {
        WimGasInvoicCommand::ReceiveInvoic {
            pid: pid(p),
            sender: MarktpartnerCode::new("9900123456789"),
            recipient: MarktpartnerCode::new("9900987654321"),
            invoice_ref: MessageRef::new("REF001"),
            document_date: "20260630".into(),
            validation_passed: valid,
            validation_errors: if valid {
                vec![]
            } else {
                vec!["AHB rule 42 violated".into()]
            },
        }
    }

    // ── ReceiveInvoic ──────────────────────────────────────────────────────────

    #[test]
    fn receive_31003_valid_emits_received_and_validation_passed() {
        let state = WimGasInvoicState::default();
        let out = WimGasInvoicWorkflow::handle(&state, receive_cmd(31003, true))
            .expect("valid 31003 must succeed");
        assert_eq!(out.events.len(), 2);
        assert!(matches!(
            out.events[0],
            WimGasInvoicEvent::InvoicReceived { .. }
        ));
        assert!(matches!(
            out.events[1],
            WimGasInvoicEvent::ValidationPassed { .. }
        ));
    }

    #[test]
    fn receive_31004_valid_emits_received_and_validation_passed() {
        let state = WimGasInvoicState::default();
        let out = WimGasInvoicWorkflow::handle(&state, receive_cmd(31004, true))
            .expect("valid 31004 must succeed");
        assert_eq!(out.events.len(), 2);
        assert!(matches!(
            out.events[0],
            WimGasInvoicEvent::InvoicReceived { .. }
        ));
        assert!(matches!(
            out.events[1],
            WimGasInvoicEvent::ValidationPassed { .. }
        ));
    }

    #[test]
    fn receive_invalid_emits_received_and_rejected() {
        let state = WimGasInvoicState::default();
        let out = WimGasInvoicWorkflow::handle(&state, receive_cmd(31003, false))
            .expect("invalid 31003 must still return Ok (Rejected event)");
        assert_eq!(out.events.len(), 2);
        assert!(matches!(
            out.events[0],
            WimGasInvoicEvent::InvoicReceived { .. }
        ));
        assert!(matches!(out.events[1], WimGasInvoicEvent::Rejected { .. }));
    }

    #[test]
    fn duplicate_receive_is_error() {
        let state = WimGasInvoicState::InvoicReceived(WimGasInvoicData {
            pruefidentifikator: pid(31003),
            sender: MarktpartnerCode::new("9900123456789"),
            recipient: MarktpartnerCode::new("9900987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("REF001"),
        });
        let err = WimGasInvoicWorkflow::handle(&state, receive_cmd(31003, true))
            .expect_err("second receive must be rejected");
        assert!(format!("{err}").contains("New"));
    }

    #[test]
    fn unknown_pid_is_error() {
        let state = WimGasInvoicState::default();
        let err = WimGasInvoicWorkflow::handle(&state, receive_cmd(31001, true))
            .expect_err("GPKE PID 31001 must not be accepted by WiM Gas workflow");
        assert!(format!("{err}").contains("31001"));
    }

    // ── SettleInvoice ──────────────────────────────────────────────────────────

    #[test]
    fn settle_from_validation_passed_emits_settled() {
        let data = WimGasInvoicData {
            pruefidentifikator: pid(31003),
            sender: MarktpartnerCode::new("9900123456789"),
            recipient: MarktpartnerCode::new("9900987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("REF001"),
        };
        let state = WimGasInvoicState::ValidationPassed(data);
        let out = WimGasInvoicWorkflow::handle(&state, WimGasInvoicCommand::SettleInvoice)
            .expect("settle must succeed from ValidationPassed");
        assert_eq!(out.events.len(), 1);
        assert!(matches!(out.events[0], WimGasInvoicEvent::InvoiceSettled));
    }

    #[test]
    fn settle_from_wrong_state_is_error() {
        let state = WimGasInvoicState::New;
        let err = WimGasInvoicWorkflow::handle(&state, WimGasInvoicCommand::SettleInvoice)
            .expect_err("settle from New must be rejected");
        assert!(format!("{err}").contains("ValidationPassed"));
    }

    // ── DisputeInvoice ──────────────────────────────────────────────────────────

    #[test]
    fn dispute_from_validation_passed_emits_disputed() {
        let data = WimGasInvoicData {
            pruefidentifikator: pid(31003),
            sender: MarktpartnerCode::new("9900123456789"),
            recipient: MarktpartnerCode::new("9900987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("REF001"),
        };
        let state = WimGasInvoicState::ValidationPassed(data);
        let out = WimGasInvoicWorkflow::handle(
            &state,
            WimGasInvoicCommand::DisputeInvoice {
                reason: "Betrag stimmt nicht".into(),
            },
        )
        .expect("dispute must succeed from ValidationPassed");
        assert_eq!(out.events.len(), 1);
        assert!(matches!(
            out.events[0],
            WimGasInvoicEvent::InvoiceDisputed { .. }
        ));
    }

    // ── TimeoutExpired ─────────────────────────────────────────────────────────

    #[test]
    fn timeout_from_pending_emits_deadline_expired() {
        let data = WimGasInvoicData {
            pruefidentifikator: pid(31003),
            sender: MarktpartnerCode::new("9900123456789"),
            recipient: MarktpartnerCode::new("9900987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("REF001"),
        };
        let state = WimGasInvoicState::ValidationPassed(data);
        let out = WimGasInvoicWorkflow::handle(
            &state,
            WimGasInvoicCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: SETTLEMENT_WINDOW_LABEL.into(),
            },
        )
        .expect("timeout must succeed from ValidationPassed");
        assert_eq!(out.events.len(), 1);
        assert!(matches!(
            out.events[0],
            WimGasInvoicEvent::DeadlineExpired { .. }
        ));
    }

    #[test]
    fn timeout_from_settled_is_noop() {
        let data = WimGasInvoicData {
            pruefidentifikator: pid(31003),
            sender: MarktpartnerCode::new("9900123456789"),
            recipient: MarktpartnerCode::new("9900987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("REF001"),
        };
        let state = WimGasInvoicState::Settled(data);
        let out = WimGasInvoicWorkflow::handle(
            &state,
            WimGasInvoicCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: SETTLEMENT_WINDOW_LABEL.into(),
            },
        )
        .expect("timeout on Settled must return empty Ok");
        assert!(out.events.is_empty());
    }

    // ── apply ──────────────────────────────────────────────────────────────────

    #[test]
    fn apply_settled_event_transitions_state() {
        let data = WimGasInvoicData {
            pruefidentifikator: pid(31003),
            sender: MarktpartnerCode::new("9900123456789"),
            recipient: MarktpartnerCode::new("9900987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("REF001"),
        };
        let state = WimGasInvoicState::ValidationPassed(data);
        let new_state = WimGasInvoicWorkflow::apply(state, &WimGasInvoicEvent::InvoiceSettled);
        assert!(matches!(new_state, WimGasInvoicState::Settled(_)));
    }

    #[test]
    fn deadline_expiry_does_not_overwrite_settled() {
        let data = WimGasInvoicData {
            pruefidentifikator: pid(31003),
            sender: MarktpartnerCode::new("9900123456789"),
            recipient: MarktpartnerCode::new("9900987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("REF001"),
        };
        let state = WimGasInvoicState::Settled(data);
        let new_state = WimGasInvoicWorkflow::apply(
            state,
            &WimGasInvoicEvent::DeadlineExpired {
                deadline_id: DeadlineId::new(),
                label: SETTLEMENT_WINDOW_LABEL.into(),
            },
        );
        assert!(matches!(new_state, WimGasInvoicState::Settled(_)));
    }

    // ── Projection ─────────────────────────────────────────────────────────────

    #[test]
    fn projection_defaults_to_new() {
        let proj = WimGasInvoicProjection::default();
        assert!(proj.records.is_empty());
        assert_eq!(proj.last_sequence(), None);
    }
}
