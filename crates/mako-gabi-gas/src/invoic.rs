//! GaBi Gas INVOIC billing — PIDs 31010, 31007, 31008.
//!
//! Handles INVOIC-based billing in the GaBi Gas domain:
//! - **Kapazitätsrechnung** (PID 31010): FNB/VNB invoices BKV for transmission capacity
//! - **Aggreg. MMM-Rechnung Gas** (PID 31007): NB invoices MGV for aggregated Mehr-/Mindermengen
//! - **Aggreg. MMM-Rechnung Gas selbst ausgestellt** (PID 31008): NB invoices MGV (self-billed)
//!
//! # Covered Prüfidentifikatoren (INVOIC AHB / FV2025-10-01, BK7-14-020)
//!
//! | PID   | Process                                           | Direction   |
//! |-------|---------------------------------------------------|-------------|
//! | 31010 | Kapazitätsrechnung (capacity billing)             | FNB/VNB → BKV |
//! | 31007 | Aggreg. MMM-Rechnung Gas                          | NB → MGV    |
//! | 31008 | Aggreg. MMM-Rechnung Gas selbst ausgestellt       | NB → MGV    |
//!
//! # Not covered here — see `mako-geli-gas`
//!
//! PID 31011 (Rechnung sonstige Leistung / AWH Sperrprozesse Gas, VNB → LFN/LFA)
//! belongs to the GeLi Gas domain (BK7-24-01-009) and is handled by the
//! `geli-gas-sperrprozesse-invoic` workflow in `mako-geli-gas`.
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
//! - **BK7-14-020** — GaBi Gas 2.0 ruling (current; Bundesnetzagentur)
//! - **INVOIC AHB** — EDI@Energy invoice message format
//! - **GasNZV** — Gasnetzzugangsverordnung (statutory basis)
//!
//! # Notes
//!
//! These PIDs use INVOIC, a standard EDIFACT format handled by the `edi-energy`
//! crate's INVOIC profile. They are independent of the DVGW formats (ALOCAT,
//! NOMINT, NOMRES) parsed by `dvgw-edi`.

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

/// GaBi Gas billing Prüfidentifikatoren handled by this workflow (INVOIC AHB).
///
/// | PID   | Name                                                              |
/// |-------|-------------------------------------------------------------------|
/// | 31010 | Kapazitätsrechnung (capacity invoice, FNB/VNB → BKV)            |
/// | 31007 | Aggreg. MMM-Rechnung Gas (NB → MGV) — Gas-only, MGV is Gas role  |
/// | 31008 | Aggreg. MMM-Rechnung Gas selbst ausgestellt (NB → MGV) — Gas-only |
///
/// PIDs 31007/31008 were previously misassigned to `mako-gpke`; MGV
/// (Marktgebietsverantwortlicher) is a Gas-only market role that does not exist
/// in the Strom domain. Regulatory basis: BK7-14-020 GaBi Gas 2.0.
pub const GABI_GAS_INVOIC_PIDS: &[u32] = &[
    31010, // Kapazitätsrechnung (capacity billing, FNB/VNB → BKV)
    31007, // Aggreg. MMM-Rechnung Gas (NB → MGV)
    31008, // Aggreg. MMM-selbst ausgest. Rechnung Gas (NB → MGV)
];

/// Workflow key used for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-invoic";

/// Resume-path workflow name for REMADV (payment confirmation) messages.
///
/// Used in startup adapter validation. The REMADV resume path reuses the same
/// `GaBiGasInvoicWorkflow` but is validated separately because it accepts
/// a different adapter registry (`gabi_gas_remadv_registry`).
pub const REMADV_RESUME_PATH: &str = "gabi-gas-invoic/remadv";

/// Resume-path workflow name for COMDIS (payment rejection) messages.
///
/// Used in startup adapter validation. The COMDIS resume path reuses the same
/// `GaBiGasInvoicWorkflow` but is validated separately because it accepts
/// a different adapter registry (`gabi_gas_comdis_registry`).
pub const COMDIS_RESUME_PATH: &str = "gabi-gas-invoic/comdis";

/// Deadline label for the GaBi Gas INVOIC settlement response window.
///
/// Per standard GaBi Gas billing terms, the receiving party must settle or
/// dispute an inbound INVOIC within the contractual response window.
/// Register a [`mako_engine::deadline::Deadline`] with this label immediately
/// after the `ValidationPassed` event.
pub const SETTLEMENT_WINDOW_LABEL: &str = "gabi-gas-invoic-settlement-deadline";

/// REMADV PID for GaBi Gas billing (inbound Zahlungsavis, invoicer role).
///
/// After a GaBi Gas INVOIC is issued, the recipient sends REMADV 33001 to
/// confirm payment:
/// - PID 31010 (Kapazitätsrechnung): BKV sends REMADV to FNB/VNB
/// - PIDs 31007/31008 (Aggreg. MMM-Rechnung Gas): MGV sends REMADV to NB
///
/// Source: REMADV AHB 1.0, GaBi Gas, BK7.
pub const GABI_GAS_REMADV_PID: u32 = 33001;

/// COMDIS PID for GaBi Gas billing (inbound Ablehnung REMADV, invoicer role).
///
/// The invoicer can reject the recipient's REMADV via COMDIS 29001:
/// - PID 31010: FNB/VNB rejects a BKV REMADV
/// - PIDs 31007/31008: NB rejects an MGV REMADV
///
/// Source: COMDIS AHB 1.0, GaBi Gas, BK7.
pub const GABI_GAS_COMDIS_ABLEHNUNG_PID: u32 = 29001;

// ── Data ──────────────────────────────────────────────────────────────────────

/// Business data captured when the GaBi Gas INVOIC is first received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GaBiGasInvoicData {
    /// BDEW Prüfidentifikator (31010 = Kapazitätsrechnung; 31007/31008 = Aggreg. MMM-Rechnung Gas).
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the invoice issuer (FNB/VNB).
    pub sender: MarktpartnerCode,
    /// GLN of the invoice recipient (BKV).
    pub recipient: MarktpartnerCode,
    /// EDIFACT document date string from BGM/DTM (YYYYMMDD).
    pub document_date: String,
    /// Invoice reference number from UNH/BGM.
    pub invoice_ref: MessageRef,
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Current state of a GaBi Gas INVOIC billing process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GaBiGasInvoicState {
    /// No INVOIC received yet.
    #[default]
    New,
    /// INVOIC received; AHB validation pending or in progress.
    InvoicReceived(GaBiGasInvoicData),
    /// INVOIC passed AHB validation; awaiting BKV settlement or dispute.
    ValidationPassed(GaBiGasInvoicData),
    /// Invoice settled — positive CONTRL dispatched to the issuer.
    Settled(GaBiGasInvoicData),
    /// Invoice disputed — negative CONTRL or APERAK dispatched to the issuer.
    Disputed {
        /// Billing data captured at the time of the dispute.
        data: GaBiGasInvoicData,
        /// Human-readable reason for the dispute.
        reason: String,
    },
    /// Process rejected due to AHB validation failure, duplicate, or deadline.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// Payment confirmed by BKV (REMADV 33001 received, invoicer role).
    PaymentConfirmed(GaBiGasInvoicData),
    /// COMDIS 29001 received — invoicer rejected the BKV's REMADV (payer role).
    ComdisRejected(GaBiGasInvoicData),
}

impl GaBiGasInvoicState {
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
            Self::ComdisRejected(_) => "ComdisRejected",
        }
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GaBi Gas INVOIC billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GaBiGasInvoicEvent {
    /// INVOIC received from the gas network operator (FNB/VNB).
    InvoicReceived {
        /// EDIFACT message reference of the inbound INVOIC.
        invoice_ref: MessageRef,
        /// GLN of the invoice issuer (FNB/VNB).
        sender: MarktpartnerCode,
        /// GLN of the invoice recipient (BKV).
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// BDEW Prüfidentifikator (always 31010 for this workflow).
        pruefidentifikator: Pruefidentifikator,
    },
    /// INVOIC passed AHB profile validation — no rule violations found.
    ///
    /// The settlement deadline (`gabi-gas-invoic-settlement-deadline`) should
    /// be registered immediately after this event is persisted.
    ValidationPassed {
        /// Reference of the validated INVOIC message.
        invoice_ref: MessageRef,
    },
    /// Invoice settled — positive CONTRL dispatched to the issuer.
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
    /// Settlement deadline expired before the BKV issued a response.
    ///
    /// A deadline expiry triggers a late-notice APERAK via the outbox.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// REMADV 33001 received — BKV confirms payment (invoicer role).
    RemadvReceived {
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// GLN of the REMADV sender (BKV/payer).
        sender: MarktpartnerCode,
    },
    /// COMDIS 29001 received — invoicer rejects BKV's REMADV (payer role).
    ComdisAbLehnungReceived {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
}

impl EventPayload for GaBiGasInvoicEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::InvoicReceived { .. } => "GaBiGasInvoicReceived",
            Self::ValidationPassed { .. } => "GaBiGasInvoicValidationPassed",
            Self::InvoiceSettled => "GaBiGasInvoicSettled",
            Self::InvoiceDisputed { .. } => "GaBiGasInvoicDisputed",
            Self::Rejected { .. } => "GaBiGasInvoicRejected",
            Self::DeadlineExpired { .. } => "GaBiGasInvoicDeadlineExpired",
            Self::RemadvReceived { .. } => "GaBiGasInvoicRemadvReceived",
            Self::ComdisAbLehnungReceived { .. } => "GaBiGasInvoicComdisAbLehnungReceived",
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands accepted by the GaBi Gas INVOIC billing workflow.
#[derive(Debug, Clone)]
pub enum GaBiGasInvoicCommand {
    /// Receive an inbound INVOIC from a GaBi Gas market participant (FNB/VNB).
    ///
    /// The transport/adapter layer is responsible for parsing the EDIFACT
    /// message and running AHB validation **before** constructing this command.
    /// Pass `validation_passed: false` and populate `validation_errors` if the
    /// AHB check found rule violations; the workflow will emit `Rejected` and
    /// enqueue a negative CONTRL.
    ReceiveInvoic {
        /// BDEW Prüfidentifikator — must be one of [`GABI_GAS_INVOIC_PIDS`] (31007, 31008, or 31010).
        pid: Pruefidentifikator,
        /// GLN of the sender (FNB/VNB).
        sender: MarktpartnerCode,
        /// GLN of the recipient (BKV).
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
    /// Settle the invoice — dispatch a positive CONTRL to the issuer.
    SettleInvoice,
    /// Dispute the invoice — dispatch a negative CONTRL or APERAK to the issuer.
    DisputeInvoice {
        /// Human-readable reason for the dispute.
        reason: String,
    },
    /// The settlement deadline fired before the BKV issued a response.
    ///
    /// Fired by the `DeadlineScheduler` when the `gabi-gas-invoic-settlement-deadline`
    /// deadline expires. The workflow emits `DeadlineExpired` and the outbox worker
    /// sends a late-notice APERAK to the issuer.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
    /// Invoicer role: inbound REMADV 33001 received from the BKV (payment confirmed).
    ///
    /// Source: REMADV AHB 1.0, GaBi Gas, BK7.
    ReceiveRemadv {
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// GLN of the REMADV sender (BKV/payer).
        sender: MarktpartnerCode,
    },
    /// Payer role: inbound COMDIS 29001 received (invoicer rejects BKV's REMADV).
    ///
    /// Source: COMDIS AHB 1.0, GaBi Gas, BK7.
    ReceiveComdis {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
}

impl CommandPayload for GaBiGasInvoicCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GaBi Gas INVOIC billing workflow (PIDs 31010, 31007, 31008).
///
/// Implements the receive → validate → settle/dispute state machine for GaBi
/// Gas billing (Kapazitätsrechnung and Aggreg. MMM-Rechnung Gas) under
/// BK7-14-020 (GaBi Gas 2.0).
///
/// # Deadline
///
/// Register a deadline with label [`SETTLEMENT_WINDOW_LABEL`] after the
/// `ValidationPassed` event fires.
pub struct GaBiGasInvoicWorkflow;

impl Workflow for GaBiGasInvoicWorkflow {
    type State = GaBiGasInvoicState;
    type Event = GaBiGasInvoicEvent;
    type Command = GaBiGasInvoicCommand;

    /// Deadline compensation for the GaBi Gas INVOIC settlement window.
    ///
    /// | Label | State guard | Command emitted |
    /// |---|---|---|
    /// | `"gabi-gas-invoic-settlement-deadline"` | `InvoicReceived` or `ValidationPassed` | `TimeoutExpired` |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                SETTLEMENT_WINDOW_LABEL,
                GaBiGasInvoicState::InvoicReceived(_) | GaBiGasInvoicState::ValidationPassed(_),
            ) => Some(GaBiGasInvoicCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GaBiGasInvoicEvent::InvoicReceived {
                invoice_ref,
                sender,
                recipient,
                document_date,
                pruefidentifikator,
            } => GaBiGasInvoicState::InvoicReceived(GaBiGasInvoicData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                recipient: recipient.clone(),
                document_date: document_date.clone(),
                invoice_ref: invoice_ref.clone(),
            }),

            GaBiGasInvoicEvent::ValidationPassed { .. } => match state {
                GaBiGasInvoicState::InvoicReceived(data) => {
                    GaBiGasInvoicState::ValidationPassed(data)
                }
                other => other,
            },

            GaBiGasInvoicEvent::InvoiceSettled => match state {
                GaBiGasInvoicState::ValidationPassed(data) => GaBiGasInvoicState::Settled(data),
                other => other,
            },

            GaBiGasInvoicEvent::InvoiceDisputed { reason } => match state {
                GaBiGasInvoicState::ValidationPassed(data) => GaBiGasInvoicState::Disputed {
                    data,
                    reason: reason.clone(),
                },
                other => other,
            },

            GaBiGasInvoicEvent::Rejected { reason } => GaBiGasInvoicState::Rejected {
                reason: reason.clone(),
            },

            GaBiGasInvoicEvent::DeadlineExpired { label, .. } => match state {
                // Terminal states — deadline expiry does not overwrite a completed process.
                GaBiGasInvoicState::Settled(_)
                | GaBiGasInvoicState::Disputed { .. }
                | GaBiGasInvoicState::Rejected { .. }
                | GaBiGasInvoicState::PaymentConfirmed(_)
                | GaBiGasInvoicState::ComdisRejected(_) => state,
                _ => GaBiGasInvoicState::Rejected {
                    reason: format!("settlement deadline expired: {label}"),
                },
            },

            GaBiGasInvoicEvent::RemadvReceived { remadv_ref, sender } => match state {
                GaBiGasInvoicState::Settled(data) | GaBiGasInvoicState::ValidationPassed(data) => {
                    let _ = (remadv_ref, sender);
                    GaBiGasInvoicState::PaymentConfirmed(data)
                }
                other => other,
            },

            GaBiGasInvoicEvent::ComdisAbLehnungReceived { .. } => match state {
                GaBiGasInvoicState::ValidationPassed(data)
                | GaBiGasInvoicState::Settled(data)
                | GaBiGasInvoicState::PaymentConfirmed(data) => {
                    GaBiGasInvoicState::ComdisRejected(data)
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
            GaBiGasInvoicCommand::ReceiveInvoic {
                pid,
                sender,
                recipient,
                invoice_ref,
                document_date,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GaBiGasInvoicState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !GABI_GAS_INVOIC_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a GaBi Gas INVOIC PID (31007/31008/31010), got {pid}",
                    )));
                }
                let mut events = vec![GaBiGasInvoicEvent::InvoicReceived {
                    invoice_ref: invoice_ref.clone(),
                    sender,
                    recipient,
                    document_date,
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(GaBiGasInvoicEvent::ValidationPassed { invoice_ref });
                } else {
                    events.push(GaBiGasInvoicEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            GaBiGasInvoicCommand::SettleInvoice => {
                if !matches!(state, GaBiGasInvoicState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![GaBiGasInvoicEvent::InvoiceSettled].into())
            }

            GaBiGasInvoicCommand::DisputeInvoice { reason } => {
                if !matches!(state, GaBiGasInvoicState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![GaBiGasInvoicEvent::InvoiceDisputed { reason }].into())
            }

            GaBiGasInvoicCommand::TimeoutExpired { deadline_id, label } => {
                // Idempotent in terminal states — the deadline may fire just
                // after the BKV dispatched a response.
                if matches!(
                    state,
                    GaBiGasInvoicState::Settled(_)
                        | GaBiGasInvoicState::Disputed { .. }
                        | GaBiGasInvoicState::Rejected { .. }
                        | GaBiGasInvoicState::PaymentConfirmed(_)
                        | GaBiGasInvoicState::ComdisRejected(_)
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GaBiGasInvoicEvent::DeadlineExpired { deadline_id, label }].into())
            }

            GaBiGasInvoicCommand::ReceiveRemadv { remadv_ref, sender } => {
                if !matches!(
                    state,
                    GaBiGasInvoicState::Settled(_) | GaBiGasInvoicState::ValidationPassed(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "Settled|ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![GaBiGasInvoicEvent::RemadvReceived { remadv_ref, sender }].into())
            }

            GaBiGasInvoicCommand::ReceiveComdis { comdis_ref } => {
                if matches!(
                    state,
                    GaBiGasInvoicState::New
                        | GaBiGasInvoicState::InvoicReceived(_)
                        | GaBiGasInvoicState::Rejected { .. }
                        | GaBiGasInvoicState::ComdisRejected(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed|Settled|PaymentConfirmed",
                        state.label(),
                    ));
                }
                Ok(vec![GaBiGasInvoicEvent::ComdisAbLehnungReceived { comdis_ref }].into())
            }
        }
    }
}

// ── Read-model projection ──────────────────────────────────────────────────────

/// Read-model record for a single GaBi Gas INVOIC billing process stream.
#[derive(Debug)]
pub struct GaBiGasInvoicRecord {
    /// Current lifecycle status label.
    pub status: &'static str,
    /// BDEW Prüfidentifikator once the INVOIC is received.
    pub pruefidentifikator: Option<Pruefidentifikator>,
    /// Total events processed for this stream.
    pub event_count: usize,
}

impl Default for GaBiGasInvoicRecord {
    fn default() -> Self {
        Self {
            status: "New",
            pruefidentifikator: None,
            event_count: 0,
        }
    }
}

/// In-process read model tracking all GaBi Gas INVOIC billing process streams.
#[derive(Debug, Default)]
pub struct GaBiGasInvoicProjection {
    /// All known billing process records keyed by stream ID.
    pub records: HashMap<String, GaBiGasInvoicRecord>,
    /// Sequence number of the last event applied.
    pub last_seq: u64,
}

impl Projection for GaBiGasInvoicProjection {
    fn name(&self) -> &'static str {
        "GaBiGasInvoicProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        let Ok(event) = envelope.decode::<GaBiGasInvoicEvent>() else {
            return;
        };

        match event {
            GaBiGasInvoicEvent::InvoicReceived {
                pruefidentifikator, ..
            } => {
                record.status = "InvoicReceived";
                record.pruefidentifikator = Some(pruefidentifikator);
            }
            GaBiGasInvoicEvent::ValidationPassed { .. } => {
                record.status = "ValidationPassed";
            }
            GaBiGasInvoicEvent::InvoiceSettled => {
                record.status = "Settled";
            }
            GaBiGasInvoicEvent::InvoiceDisputed { .. } => {
                record.status = "Disputed";
            }
            GaBiGasInvoicEvent::Rejected { .. } => {
                record.status = "Rejected";
            }
            GaBiGasInvoicEvent::DeadlineExpired { .. } => {
                record.status = "Rejected";
            }
            GaBiGasInvoicEvent::RemadvReceived { .. } => {
                record.status = "PaymentConfirmed";
            }
            GaBiGasInvoicEvent::ComdisAbLehnungReceived { .. } => {
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

    fn receive_cmd(p: u32, valid: bool) -> GaBiGasInvoicCommand {
        GaBiGasInvoicCommand::ReceiveInvoic {
            pid: pid(p),
            sender: MarktpartnerCode::new("9910123456789"),
            recipient: MarktpartnerCode::new("9910987654321"),
            invoice_ref: MessageRef::new("KAPREF001"),
            document_date: "20260630".into(),
            validation_passed: valid,
            validation_errors: if valid {
                vec![]
            } else {
                vec!["AHB rule 17 violated".into()]
            },
        }
    }

    #[test]
    fn receive_31010_valid_emits_received_and_validation_passed() {
        let state = GaBiGasInvoicState::default();
        let out = GaBiGasInvoicWorkflow::handle(&state, receive_cmd(31010, true))
            .expect("valid 31010 must succeed");
        assert_eq!(out.events.len(), 2);
        assert!(matches!(
            out.events[0],
            GaBiGasInvoicEvent::InvoicReceived { .. }
        ));
        assert!(matches!(
            out.events[1],
            GaBiGasInvoicEvent::ValidationPassed { .. }
        ));
    }

    #[test]
    fn receive_invalid_emits_received_and_rejected() {
        let state = GaBiGasInvoicState::default();
        let out = GaBiGasInvoicWorkflow::handle(&state, receive_cmd(31010, false))
            .expect("invalid INVOIC must return Ok (Rejected event)");
        assert_eq!(out.events.len(), 2);
        assert!(matches!(
            out.events[0],
            GaBiGasInvoicEvent::InvoicReceived { .. }
        ));
        assert!(matches!(out.events[1], GaBiGasInvoicEvent::Rejected { .. }));
    }

    #[test]
    fn duplicate_receive_is_error() {
        let state = GaBiGasInvoicState::InvoicReceived(GaBiGasInvoicData {
            pruefidentifikator: pid(31010),
            sender: MarktpartnerCode::new("9910123456789"),
            recipient: MarktpartnerCode::new("9910987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("KAPREF001"),
        });
        let err = GaBiGasInvoicWorkflow::handle(&state, receive_cmd(31010, true))
            .expect_err("second receive must be rejected");
        assert!(format!("{err}").contains("New"));
    }

    #[test]
    fn unknown_pid_is_error() {
        let state = GaBiGasInvoicState::default();
        let err = GaBiGasInvoicWorkflow::handle(&state, receive_cmd(31003, true))
            .expect_err("WiM Gas PID 31003 must not be accepted by GaBi Gas workflow");
        assert!(format!("{err}").contains("31003"));
    }

    #[test]
    fn settle_from_validation_passed_emits_settled() {
        let data = GaBiGasInvoicData {
            pruefidentifikator: pid(31010),
            sender: MarktpartnerCode::new("9910123456789"),
            recipient: MarktpartnerCode::new("9910987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("KAPREF001"),
        };
        let state = GaBiGasInvoicState::ValidationPassed(data);
        let out = GaBiGasInvoicWorkflow::handle(&state, GaBiGasInvoicCommand::SettleInvoice)
            .expect("settle must succeed from ValidationPassed");
        assert_eq!(out.events.len(), 1);
        assert!(matches!(out.events[0], GaBiGasInvoicEvent::InvoiceSettled));
    }

    #[test]
    fn dispute_from_validation_passed_emits_disputed() {
        let data = GaBiGasInvoicData {
            pruefidentifikator: pid(31010),
            sender: MarktpartnerCode::new("9910123456789"),
            recipient: MarktpartnerCode::new("9910987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("KAPREF001"),
        };
        let state = GaBiGasInvoicState::ValidationPassed(data);
        let out = GaBiGasInvoicWorkflow::handle(
            &state,
            GaBiGasInvoicCommand::DisputeInvoice {
                reason: "Kapazität abweichend".into(),
            },
        )
        .expect("dispute must succeed from ValidationPassed");
        assert_eq!(out.events.len(), 1);
        assert!(matches!(
            out.events[0],
            GaBiGasInvoicEvent::InvoiceDisputed { .. }
        ));
    }

    #[test]
    fn timeout_from_pending_emits_deadline_expired() {
        let data = GaBiGasInvoicData {
            pruefidentifikator: pid(31010),
            sender: MarktpartnerCode::new("9910123456789"),
            recipient: MarktpartnerCode::new("9910987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("KAPREF001"),
        };
        let state = GaBiGasInvoicState::ValidationPassed(data);
        let out = GaBiGasInvoicWorkflow::handle(
            &state,
            GaBiGasInvoicCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: SETTLEMENT_WINDOW_LABEL.into(),
            },
        )
        .expect("timeout must succeed from ValidationPassed");
        assert_eq!(out.events.len(), 1);
        assert!(matches!(
            out.events[0],
            GaBiGasInvoicEvent::DeadlineExpired { .. }
        ));
    }

    #[test]
    fn timeout_from_settled_is_noop() {
        let data = GaBiGasInvoicData {
            pruefidentifikator: pid(31010),
            sender: MarktpartnerCode::new("9910123456789"),
            recipient: MarktpartnerCode::new("9910987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("KAPREF001"),
        };
        let state = GaBiGasInvoicState::Settled(data);
        let out = GaBiGasInvoicWorkflow::handle(
            &state,
            GaBiGasInvoicCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: SETTLEMENT_WINDOW_LABEL.into(),
            },
        )
        .expect("timeout on Settled must return empty Ok");
        assert!(out.events.is_empty());
    }

    #[test]
    fn deadline_expiry_does_not_overwrite_settled() {
        let data = GaBiGasInvoicData {
            pruefidentifikator: pid(31010),
            sender: MarktpartnerCode::new("9910123456789"),
            recipient: MarktpartnerCode::new("9910987654321"),
            document_date: "20260630".into(),
            invoice_ref: MessageRef::new("KAPREF001"),
        };
        let state = GaBiGasInvoicState::Settled(data);
        let new_state = GaBiGasInvoicWorkflow::apply(
            state,
            &GaBiGasInvoicEvent::DeadlineExpired {
                deadline_id: DeadlineId::new(),
                label: SETTLEMENT_WINDOW_LABEL.into(),
            },
        );
        assert!(matches!(new_state, GaBiGasInvoicState::Settled(_)));
    }

    #[test]
    fn projection_defaults_to_empty() {
        let proj = GaBiGasInvoicProjection::default();
        assert!(proj.records.is_empty());
        assert_eq!(proj.last_sequence(), None);
    }
}
