//! GPKE Netznutzungsabrechnung / Mehr-Mindermengen — INVOIC-based billing processes.
//!
//! Covers GPKE billing processes that use the INVOIC message format (INVOIC AHB 2.8e /
//! INVOIC MIG 2.8e, superseded by INVOIC AHB 1.0 from FV2025-10-01).
//!
//! # Covered Prüfidentifikatoren (INVOIC AHB 2.8e / AHB 1.0)
//!
//! | PID   | Process variant                                     | AHB profile |
//! |-------|-----------------------------------------------------|-------------|
//! | 31001 | Abschlagsrechnung (Netznutzung)                     | 2.8e ✅     |
//! | 31002 | NN-Rechnung (Netznutzungsabrechnung)                 | 2.8e ✅     |
//! | 31004 | Stornorechnung (Storno der NN- / MMM-Rechnung)       | 2.8e ✅     |
//! | 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)               | 2.8e ✅     |
//! | 31006 | MMM-Rechnung (selbst ausgestellt)                    | 2.8e ✅     |
//! | 31007 | Aggregierte Mehr-/Mindermenge Rechnung               | 2.8e ✅     |
//! | 31008 | Aggregierte Mehr-/Mindermenge Rechnung (SA)          | 2.8e ✅     |
//!
//! All 7 PIDs share the same INVOIC-receive → settle/dispute state machine.
//! The stored `pruefidentifikator` field in [`AbrechnungData`] lets read-models
//! distinguish process variants.
//!
//! ## Note on ex-MPES PIDs 56005–56010
//!
//! PIDs 56005–56010 were referenced in earlier implementations as ex-MPES
//! billing PIDs transferred to GPKE per BK6-22-024. These PIDs **do not appear
//! in any extracted INVOIC AHB profile** (neither 2.8e nor 1.0). The new
//! INVOIC AHB 1.0a/1.0b PDFs exist in `docs/pdfs/` but have not yet been
//! extracted. Run `cargo xtask extract-pdf` to generate updated profiles.
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE** — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität
//! - **BK6-22-024** — GPKE APERAK Frist: **24 wall-clock hours**
//! - **INVOIC AHB 2.8e / INVOIC AHB 1.0** — EDI@Energy invoice message format
//! - **CONTRL / APERAK** — Acknowledgement and error responses

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

/// All GPKE billing Prüfidentifikatoren handled by this workflow (INVOIC-based).
///
/// These are the GPKE-domain PIDs from INVOIC AHB 2.8e (FV2025-10-01 / FV2026-04-01).
/// PIDs 31003 (WiM-Rechnung) and 31009 (MSB-Rechnung) belong to the WiM domain and
/// are NOT included here. Storno (31004) applies to GPKE invoices of any variant.
///
/// | PID   | Name                                         |
/// |-------|----------------------------------------------|
/// | 31001 | Abschlagsrechnung (Netznutzung)              |
/// | 31002 | NN-Rechnung (Netznutzungsabrechnung)          |
/// | 31004 | Stornorechnung                               |
/// | 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)        |
/// | 31006 | MMM-Rechnung (selbst ausgestellt)            |
/// | 31007 | Aggregierte Mehr-/Mindermenge Rechnung       |
/// | 31008 | Aggregierte Mehr-/Mindermenge Rechnung (SA)  |
pub const INVOIC_PIDS: &[u32] = &[31001, 31002, 31004, 31005, 31006, 31007, 31008];

/// Deadline label for the INVOIC settlement response window.
///
/// The NB must settle or dispute an incoming INVOIC within the
/// contractual period (typically 5 Werktage from receipt).
/// Register a `Deadline` with this label immediately after `ValidationPassed`.
pub const ABRECHNUNG_WINDOW_LABEL: &str = "invoic-settlement-deadline";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AbrechnungEvent {
    /// INVOIC received and initial processing completed.
    InvoicReceived {
        /// EDIFACT message reference of the INVOIC.
        invoice_ref: MessageRef,
        /// GLN of the billing party (sender).
        sender: MarktpartnerCode,
        /// GLN of the receiving party.
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// BDEW Prüfidentifikator (31001–31008).
        pruefidentifikator: Pruefidentifikator,
    },
    /// INVOIC passed profile validation (no rule violations).
    ValidationPassed {
        /// Reference of the validated INVOIC message.
        invoice_ref: MessageRef,
    },
    /// Invoice settled — positive CONTRL or acknowledgement dispatched.
    InvoiceSettled,
    /// Invoice disputed — negative CONTRL or APERAK dispatched.
    InvoiceDisputed {
        /// Human-readable dispute reason.
        reason: String,
    },
    /// Invoice was rejected immediately due to validation failure.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline expired before the billing process completed.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for AbrechnungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::InvoicReceived { .. } => "AbrechnungInvoicReceived",
            Self::ValidationPassed { .. } => "AbrechnungValidationPassed",
            Self::InvoiceSettled => "AbrechnungInvoiceSettled",
            Self::InvoiceDisputed { .. } => "AbrechnungInvoiceDisputed",
            Self::Rejected { .. } => "AbrechnungRejected",
            Self::DeadlineExpired { .. } => "AbrechnungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data populated when the INVOIC is first received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbrechnungData {
    /// BDEW Prüfidentifikator (31001–31008, GPKE INVOIC range).
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the invoice sender (typically the DSO/TSO).
    pub sender: MarktpartnerCode,
    /// GLN of the invoice recipient (typically the supplier).
    pub recipient: MarktpartnerCode,
    /// EDIFACT document date string from BGM/DTM.
    pub document_date: String,
    /// Invoice reference number from UNH/BGM.
    pub invoice_ref: MessageRef,
}

/// Current state of a GPKE billing process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum AbrechnungState {
    /// No events yet.
    New,
    /// INVOIC received.
    InvoicReceived(AbrechnungData),
    /// INVOIC passed validation; response not yet dispatched.
    ValidationPassed(AbrechnungData),
    /// Invoice settled (positive CONTRL).
    Settled(AbrechnungData),
    /// Invoice disputed (negative CONTRL or APERAK).
    Disputed {
        /// Billing data captured at the time of dispute.
        data: AbrechnungData,
        /// Human-readable reason for the dispute.
        reason: String,
    },
    /// Process rejected due to validation failure or unrecoverable error.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl Default for AbrechnungState {
    fn default() -> Self {
        Self::New
    }
}

impl AbrechnungState {
    /// Stable string label for the current variant.
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

    /// Return `Some(&AbrechnungData)` when the invoice has been received.
    #[must_use]
    pub fn abrechnung_data(&self) -> Option<&AbrechnungData> {
        match self {
            Self::InvoicReceived(d) | Self::ValidationPassed(d) | Self::Settled(d) => Some(d),
            Self::Disputed { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE billing workflow.
#[derive(Clone)]
pub enum AbrechnungCommand {
    /// Inbound INVOIC received from the AS4 layer. Domain fields extracted and
    /// validation performed by the caller before constructing this command.
    ReceiveInvoic {
        /// Prüfidentifikator of the inbound INVOIC.
        pid: Pruefidentifikator,
        /// GLN of the sender (Messstellenbetreiber).
        sender: MarktpartnerCode,
        /// GLN of the recipient (Netzbetreiber or Bilanzkreisverantwortlicher).
        recipient: MarktpartnerCode,
        /// Message reference of the inbound INVOIC.
        invoice_ref: MessageRef,
        /// Document date extracted from the INVOIC.
        document_date: String,
        /// `true` if `msg.validate()` returned a report with no errors.
        validation_passed: bool,
        /// Human-readable validation issue strings for the `Rejected` event.
        validation_errors: Vec<String>,
    },
    /// Settle the invoice — dispatch a positive CONTRL to the sender.
    ///
    /// BDEW GPKE / BK6-22-024: the response must be sent within **24
    /// wall-clock hours** of receiving the INVOIC.
    SettleInvoice,
    /// Dispute the invoice — dispatch a negative CONTRL or APERAK.
    ///
    /// BDEW GPKE / BK6-22-024: the response must be sent within **24
    /// wall-clock hours** of receiving the INVOIC.
    DisputeInvoice {
        /// Human-readable reason for the dispute.
        reason: String,
    },
    /// A registered deadline fired and was dispatched by the scheduler.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for AbrechnungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE billing workflow for INVOIC-based processes.
///
/// Covers Netznutzungsabrechnung (31001/31002), Storno (31004) and
/// Mehr-/Mindermengen (31005–31008) processes. These are the GPKE-domain
/// billing PIDs in INVOIC AHB 2.8e / 1.0.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeAbrechnungWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-abrechnung", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeAbrechnungWorkflow;

impl Workflow for GpkeAbrechnungWorkflow {
    type State = AbrechnungState;
    type Event = AbrechnungEvent;
    type Command = AbrechnungCommand;

    /// Deadline compensation for the INVOIC settlement window.
    ///
    /// | Label | State guard | Command emitted | Rule |
    /// |---|---|---|---|
    /// | `"invoic-settlement-deadline"` | `InvoicReceived` or `ValidationPassed` | `TimeoutExpired` | BDEW GPKE — settlement period |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                ABRECHNUNG_WINDOW_LABEL,
                AbrechnungState::InvoicReceived(_) | AbrechnungState::ValidationPassed(_),
            ) => Some(AbrechnungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            AbrechnungEvent::InvoicReceived {
                invoice_ref,
                sender,
                recipient,
                document_date,
                pruefidentifikator,
            } => AbrechnungState::InvoicReceived(AbrechnungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                recipient: recipient.clone(),
                document_date: document_date.clone(),
                invoice_ref: invoice_ref.clone(),
            }),

            AbrechnungEvent::ValidationPassed { .. } => match state {
                AbrechnungState::InvoicReceived(data) => AbrechnungState::ValidationPassed(data),
                other => other,
            },

            AbrechnungEvent::InvoiceSettled => match state {
                AbrechnungState::ValidationPassed(data) => AbrechnungState::Settled(data),
                other => other,
            },

            AbrechnungEvent::InvoiceDisputed { reason } => match state {
                AbrechnungState::ValidationPassed(data) => AbrechnungState::Disputed {
                    data,
                    reason: reason.clone(),
                },
                other => other,
            },

            AbrechnungEvent::Rejected { reason } => AbrechnungState::Rejected {
                reason: reason.clone(),
            },

            AbrechnungEvent::DeadlineExpired { label, .. } => match state {
                AbrechnungState::Settled(_)
                | AbrechnungState::Disputed { .. }
                | AbrechnungState::Rejected { .. } => state,
                _ => AbrechnungState::Rejected {
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
            AbrechnungCommand::ReceiveInvoic {
                pid,
                sender,
                recipient,
                invoice_ref,
                document_date,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, AbrechnungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !INVOIC_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a GPKE INVOIC PID (31001/31002/31004–31008), got {pid}",
                    )));
                }
                let mut events = vec![AbrechnungEvent::InvoicReceived {
                    invoice_ref: invoice_ref.clone(),
                    sender,
                    recipient,
                    document_date,
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(AbrechnungEvent::ValidationPassed { invoice_ref });
                } else {
                    events.push(AbrechnungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            AbrechnungCommand::SettleInvoice => {
                if !matches!(state, AbrechnungState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![AbrechnungEvent::InvoiceSettled].into())
            }

            AbrechnungCommand::DisputeInvoice { reason } => {
                if !matches!(state, AbrechnungState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![AbrechnungEvent::InvoiceDisputed { reason }].into())
            }

            AbrechnungCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    AbrechnungState::Settled(_)
                        | AbrechnungState::Disputed { .. }
                        | AbrechnungState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![AbrechnungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single GPKE billing process stream.
#[derive(Debug)]
pub struct AbrechnungRecord {
    /// Current lifecycle status label.
    pub status: &'static str,
    /// BDEW Prüfidentifikator once the INVOIC is received.
    pub pruefidentifikator: Option<Pruefidentifikator>,
    /// Total events processed for this stream.
    pub event_count: usize,
}

impl Default for AbrechnungRecord {
    fn default() -> Self {
        Self {
            status: "New",
            pruefidentifikator: None,
            event_count: 0,
        }
    }
}

/// In-process read model tracking all GPKE billing process streams.
#[derive(Debug, Default)]
pub struct AbrechnungProjection {
    /// All known billing process records keyed by stream ID.
    pub records: HashMap<String, AbrechnungRecord>,
    /// Sequence number of the last event applied.
    pub last_seq: u64,
}

impl Projection for AbrechnungProjection {
    fn name(&self) -> &'static str {
        "AbrechnungProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        let Ok(event) = envelope.decode::<AbrechnungEvent>() else {
            return;
        };

        match event {
            AbrechnungEvent::InvoicReceived {
                pruefidentifikator, ..
            } => {
                record.status = "InvoicReceived";
                record.pruefidentifikator = Some(pruefidentifikator);
            }
            AbrechnungEvent::ValidationPassed { .. } => {
                record.status = "ValidationPassed";
            }
            AbrechnungEvent::InvoiceSettled => {
                record.status = "Settled";
            }
            AbrechnungEvent::InvoiceDisputed { .. } => {
                record.status = "Disputed";
            }
            AbrechnungEvent::Rejected { .. } => {
                record.status = "Rejected";
            }
            AbrechnungEvent::DeadlineExpired { .. } => {
                record.status = "Rejected";
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
