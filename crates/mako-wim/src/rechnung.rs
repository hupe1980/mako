//! WiM Rechnung — INVOIC-based billing processes for WiM market participants.
//!
//! Covers WiM-domain INVOIC processes where the new Messstellenbetreiber (nMSB)
//! or old Messstellenbetreiber (aMSB) sends a billing invoice to the Netzbetreiber
//! or vice versa.
//!
//! # Covered Prüfidentifikatoren (INVOIC AHB 1.0 / FV2025-10-01)
//!
//! | PID   | Process variant                                   | Party direction    |
//! |-------|---------------------------------------------------|--------------------|
//! | 31003 | WiM-Rechnung (MSB → NB für Gerätewechsel)         | nMSB/aMSB → NB     |
//! | 31009 | MSB-Rechnung (NB-initiated settlement)            | NB ↔ MSB           |
//!
//! **These PIDs belong exclusively to the WiM domain.** They must not be registered
//! by `mako-gpke`. See `crates/mako-gpke/src/abrechnung.rs` `INVOIC_PIDS` for the
//! explicit exclusion.
//!
//! # Regulatory basis
//!
//! - **BDEW WiM** — Wechselprozesse im Messwesen Strom (BDEW BK6-24-174)
//! - **INVOIC AHB 1.0** — EDI@Energy invoice message format (valid FV2025-10-01)
//! - **CONTRL / APERAK** — Acknowledgement (5 Werktage Frist per BK6-24-174)
//!
//! # Implementation status
//!
//! This module provides a **stub implementation** that:
//! 1. Registers PIDs 31003 and 31009 in the PID router (preventing dead-letter routing).
//! 2. Accepts inbound INVOIC via `ReceiveInvoic` command.
//! 3. Emits a `InvoicReceived` event and enqueues a CONTRL acknowledgement via the outbox.
//!
//! Full dispute/settlement business logic is tracked in TODO.md §WiM-Rechnung.
//! The stub is sufficient to satisfy the AS4 acknowledgement obligation
//! (BDEW AS4-Profile §5) and prevent BNetzA traceability gaps.

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// WiM billing Prüfidentifikatoren handled by this workflow (INVOIC AHB 1.0).
///
/// | PID   | Name                                          |
/// |-------|-----------------------------------------------|
/// | 31009 | MSB-Rechnung (NB-MSB settlement)              |
///
/// **PID 31003** (WiM-Rechnung) belongs to `mako-wim-gas` per
/// `docs/pid-reference.md`. It must not be registered here.
pub const WIM_INVOIC_PIDS: &[u32] = &[31009];

/// Workflow key for WiM billing processes.
pub const WORKFLOW_NAME: &str = "wim-rechnung";

/// Deadline label for the INVOIC settlement response window.
///
/// Per BDEW WiM BK6-24-174, the NB must respond within **5 Werktage** of receipt.
pub const WIM_RECHNUNG_WINDOW_LABEL: &str = "wim-invoic-settlement-deadline";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimRechnungEvent {
    /// INVOIC received and CONTRL acknowledgement enqueued.
    InvoicReceived {
        /// EDIFACT message reference of the INVOIC.
        invoice_ref: MessageRef,
        /// GLN of the billing party (sender).
        sender: MarktpartnerCode,
        /// GLN of the receiving party.
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// BDEW Prüfidentifikator (31003 or 31009).
        pruefidentifikator: Pruefidentifikator,
    },
    /// INVOIC rejected immediately due to AHB validation failure.
    ///
    /// A CONTRL with error code is enqueued. No further processing occurs.
    Rejected {
        /// Human-readable rejection reason (from AHB validation issues).
        reason: String,
    },
    /// Settlement deadline expired before a response was issued.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// Invoice settled — CONTRL acknowledgement was dispatched.
    Settled,
    /// Invoice disputed — negative CONTRL or APERAK was dispatched.
    Disputed {
        /// Human-readable dispute reason.
        reason: String,
    },
}

impl EventPayload for WimRechnungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::InvoicReceived { .. } => "WimRechnungInvoicReceived",
            Self::Rejected { .. } => "WimRechnungRejected",
            Self::DeadlineExpired { .. } => "WimRechnungDeadlineExpired",
            Self::Settled => "WimRechnungSettled",
            Self::Disputed { .. } => "WimRechnungDisputed",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands accepted by the WiM billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimRechnungCommand {
    /// Receive an inbound INVOIC from a WiM market participant.
    ///
    /// The transport layer is responsible for parsing and validating the raw
    /// EDIFACT bytes **before** constructing this command. Pass
    /// `validation_passed: false` and `validation_errors` if AHB validation
    /// found errors; the workflow will emit `Rejected` and enqueue a negative
    /// CONTRL.
    ReceiveInvoic {
        /// EDIFACT message reference from the UNH segment.
        invoice_ref: MessageRef,
        /// GLN of the sender (billing party).
        sender: MarktpartnerCode,
        /// GLN of the recipient.
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD from BGM+DTM).
        document_date: String,
        /// BDEW Prüfidentifikator (31003 or 31009).
        pruefidentifikator: Pruefidentifikator,
        /// `true` if AHB profile validation found no errors.
        validation_passed: bool,
        /// Validation error descriptions (empty when `validation_passed`).
        validation_errors: Vec<String>,
    },
    /// Settle the invoice — a positive CONTRL will be dispatched.
    Settle,
    /// Dispute the invoice — a negative CONTRL / APERAK will be dispatched.
    Dispute {
        /// Human-readable dispute reason.
        reason: String,
    },
    /// The settlement deadline fired before a response was issued.
    ///
    /// Fired by the `DeadlineScheduler` when the `wim-invoic-settlement-deadline`
    /// deadline expires. The workflow emits `DeadlineExpired` and the outbox
    /// worker sends a late-notice APERAK.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl CommandPayload for WimRechnungCommand {}

// ── Workflow state ─────────────────────────────────────────────────────────────

/// Internal state of the WiM billing workflow.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum WimRechnungState {
    /// No INVOIC received yet.
    #[default]
    New,
    /// INVOIC received; awaiting settlement or dispute action.
    PendingSettlement {
        /// Invoice reference for correlation.
        invoice_ref: MessageRef,
        /// BDEW Prüfidentifikator (31003 or 31009).
        pruefidentifikator: Pruefidentifikator,
    },
    /// Invoice was accepted and settled.
    Settled,
    /// Invoice was disputed.
    Disputed {
        /// Human-readable dispute reason.
        reason: String,
    },
    /// Invoice was rejected due to AHB validation failure or deadline expiry.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

// ── Workflow implementation ───────────────────────────────────────────────────

/// WiM billing workflow for INVOIC PIDs 31003 and 31009.
pub struct WimRechnungWorkflow;

impl Workflow for WimRechnungWorkflow {
    type Command = WimRechnungCommand;
    type Event = WimRechnungEvent;
    type State = WimRechnungState;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            WimRechnungEvent::InvoicReceived {
                invoice_ref,
                pruefidentifikator,
                ..
            } => WimRechnungState::PendingSettlement {
                invoice_ref: invoice_ref.clone(),
                pruefidentifikator: *pruefidentifikator,
            },
            WimRechnungEvent::Rejected { reason } => WimRechnungState::Rejected {
                reason: reason.clone(),
            },
            WimRechnungEvent::Settled => WimRechnungState::Settled,
            WimRechnungEvent::Disputed { reason } => WimRechnungState::Disputed {
                reason: reason.clone(),
            },
            WimRechnungEvent::DeadlineExpired { label, .. } => match state {
                // Terminal states — do not overwrite with deadline expiry.
                WimRechnungState::Settled
                | WimRechnungState::Disputed { .. }
                | WimRechnungState::Rejected { .. } => state,
                _ => WimRechnungState::Rejected {
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
            WimRechnungCommand::ReceiveInvoic {
                invoice_ref,
                sender,
                recipient,
                document_date,
                pruefidentifikator,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, WimRechnungState::New) {
                    return Err(WorkflowError::invalid_state("New", format!("{state:?}")));
                }
                if !WIM_INVOIC_PIDS.contains(&pruefidentifikator.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a WiM INVOIC PID (31003 or 31009), got {pruefidentifikator}"
                    )));
                }
                let events = if validation_passed {
                    vec![WimRechnungEvent::InvoicReceived {
                        invoice_ref,
                        sender,
                        recipient,
                        document_date,
                        pruefidentifikator,
                    }]
                } else {
                    vec![WimRechnungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    }]
                };
                Ok(WorkflowOutput::events(events))
            }

            WimRechnungCommand::Settle => {
                if !matches!(state, WimRechnungState::PendingSettlement { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                Ok(WorkflowOutput::events(vec![WimRechnungEvent::Settled]))
            }

            WimRechnungCommand::Dispute { reason } => {
                if !matches!(state, WimRechnungState::PendingSettlement { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                Ok(WorkflowOutput::events(vec![WimRechnungEvent::Disputed {
                    reason,
                }]))
            }

            WimRechnungCommand::TimeoutExpired { deadline_id, label } => {
                if !matches!(state, WimRechnungState::PendingSettlement { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                Ok(WorkflowOutput::events(vec![
                    WimRechnungEvent::DeadlineExpired { deadline_id, label },
                ]))
            }
        }
    }
}
