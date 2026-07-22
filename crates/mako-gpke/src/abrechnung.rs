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
//! | 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)               | 2.8e ✅     |
//! | 31006 | MMM-Rechnung (selbst ausgestellt)                    | 2.8e ✅     |
//!
//! All 4 PIDs share the same INVOIC-receive → settle/dispute state machine.
//! PID 31003 (WiM-Rechnung) belongs to `mako-wim-gas`; not listed here.
//! PID 31009 (MSB-Rechnung, multi-domain: GPKE Teil 3 / WiM Strom Teil 1) belongs to
//! `mako-wim` (`wim-rechnung` workflow) per `crates/mako-wim/src/rechnung.rs`. It must
//! not be registered here to avoid double-registration with `WIM_INVOIC_PIDS`.
//! PIDs 31007/31008 (Aggreg. MMM-Rechnung Gas, NB → MGV, Gas-only) belong to
//! `mako-gabi-gas` `gabi-gas-invoic` — MGV is a Gas-only role; not registered here.
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
    outbox::PendingOutbox,
    projection::Projection,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use rubo4e::current::Rechnung;

// ── PID set ───────────────────────────────────────────────────────────────────

/// All GPKE billing Prüfidentifikatoren handled by this workflow (INVOIC-based).
///
/// These are the GPKE-domain PIDs from INVOIC AHB (FV2025-10-01 / FV2026-04-01).
/// PID 31003 (WiM-Rechnung) belongs to WiM Gas (`mako-wim-gas`).
/// PID 31004 (Stornorechnung WiM Gas) belongs to `mako-wim-gas` per `docs/pid-reference.md`.
/// PID 31009 (MSB-Rechnung, multi-domain: GPKE Teil 3 / WiM Strom Teil 1) belongs to
/// `mako-wim` to avoid double-registration; see `crates/mako-wim/src/rechnung.rs`.
/// PIDs 31007/31008 (Aggreg. MMM-Rechnung NB → MGV, Gas-only) belong to
/// `mako-gabi-gas` `gabi-gas-invoic` — MGV is a Gas-only role.
///
/// | PID   | Name                                         |
/// |-------|----------------------------------------------|
/// | 31001 | Abschlagsrechnung (Netznutzung)              |
/// | 31002 | NN-Rechnung (Netznutzungsabrechnung)          |
/// | 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)        |
/// | 31006 | MMM-Rechnung (selbst ausgestellt)            |
pub const INVOIC_PIDS: &[u32] = &[31001, 31002, 31005, 31006];

/// REMADV Prüfidentifikatoren handled by this workflow (inbound payment advice).
///
/// These are received by the **invoicer** (NB/MSB) after sending an INVOIC.
/// The PAYER (LF/NB) sends one of these as payment confirmation or partial rejection.
///
/// | PID   | Name                                                                |
/// |-------|---------------------------------------------------------------------|
/// | 33001 | Zahlungsavis (Bestätigung vollständige Zahlung)                     |
/// | 33002 | Zahlungsavis (Ablehnung Zahlung)                                    |
/// | 33003 | Zahlungsavis (Bestätigung Teilzahlung Netznutzungsentgelt)          |
/// | 33004 | Zahlungsavis (Bestätigung Teilzahlung Mehr-/Mindermengen)           |
///
/// Source: REMADV AHB 1.0, GPKE Teil 2/Teil 3, BK6-24-174.
pub const REMADV_PIDS: &[u32] = &[33001, 33002, 33003, 33004];

/// COMDIS Prüfidentifikator for inbound Ablehnung REMADV (payer side).
///
/// Received by the **payer** (LF/NB) after the invoicer rejects the payer's
/// REMADV (e.g. incorrect payment amount). The invoicer sends COMDIS 29001
/// outbound; the payer receives it inbound.
///
/// Source: COMDIS AHB 1.0, GPKE Teil 2/Teil 3, BK6-24-174.
pub const COMDIS_ABLEHNUNG_REMADV_PID: u32 = 29001;

/// Deadline label for the INVOIC settlement response window.
///
/// The NB must settle or dispute an incoming INVOIC within the
/// contractual period (typically 5 Werktage from receipt).
/// Register a `Deadline` with this label immediately after `ValidationPassed`.
pub const ABRECHNUNG_WINDOW_LABEL: &str = "invoic-settlement-deadline";

/// Canonical workflow name registered in the process engine.
///
/// Used as the `workflow_name` parameter in `spawn_or_resume` /
/// `dispatch_to_process` calls.  Must match the string returned by
/// `GpkeAbrechnungWorkflow::name()`.
pub const WORKFLOW_NAME: &str = "gpke-abrechnung";

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
        /// BO4E invoice domain object produced by the `makod` anti-corruption
        /// layer (EDIFACT → `Rechnung` translation).  Stored in events so that
        /// `invoicd` can run [`invoic_checker::InvoicCheckEngine::check`]
        /// directly from the event store — without re-fetching the original
        /// EDIFACT message.
        ///
        /// `None` for events produced before this field was introduced (for
        /// graceful deserialization of older event store entries).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rechnung: Option<Box<Rechnung>>,
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
    /// INVOIC was sent by this party (invoicer role — NB/MSB sent INVOIC, waiting for REMADV).
    InvoicSent {
        /// Prüfidentifikator of the outbound INVOIC.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the sender (invoicer: NB/MSB).
        sender: MarktpartnerCode,
        /// GLN of the recipient (payer: LF/NB).
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference of the outbound INVOIC.
        invoice_ref: MessageRef,
    },
    /// Inbound REMADV received (invoicer role — payer confirms or partially disputes).
    ///
    /// PID 33001 = payment confirmed; 33002/33003/33004 = disputed / partial.
    RemadvReceived {
        /// REMADV Prüfidentifikator (33001–33004).
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// GLN of the REMADV sender (payer).
        sender: MarktpartnerCode,
        /// `true` when PID 33001 (full payment confirmed), `false` for 33002–33004.
        is_confirmed: bool,
    },
    /// Inbound COMDIS 29001 received (payer role — invoicer rejected our sent REMADV).
    ComdisAbLehnungReceived {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
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
            Self::InvoicSent { .. } => "AbrechnungInvoicSent",
            Self::RemadvReceived { .. } => "AbrechnungRemadvReceived",
            Self::ComdisAbLehnungReceived { .. } => "AbrechnungComdisAbLehnungReceived",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data populated when the INVOIC is first received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// BO4E invoice domain object (EDIFACT → BO4E translation by `makod` adapter).
    ///
    /// Downstream services (`invoicd`) read this field from the event store
    /// to run [`invoic_checker::InvoicCheckEngine::check`] without accessing
    /// the original EDIFACT archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rechnung: Option<Box<Rechnung>>,
}

/// Current state of a GPKE billing process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum AbrechnungState {
    /// No events yet.
    #[default]
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
    /// INVOIC was sent (invoicer role); awaiting REMADV from payer.
    InvoicSent(AbrechnungData),
    /// REMADV received confirming full payment (PID 33001).
    PaymentConfirmed(AbrechnungData),
    /// REMADV received with partial or rejected payment (PIDs 33002/33003/33004).
    PaymentDisputed {
        /// Billing data.
        data: AbrechnungData,
        /// REMADV Prüfidentifikator indicating the dispute type.
        remadv_pid: Pruefidentifikator,
    },
    /// COMDIS 29001 received — invoicer rejected our REMADV (payer role).
    ComdisRejected(AbrechnungData),
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
            Self::InvoicSent(_) => "InvoicSent",
            Self::PaymentConfirmed(_) => "PaymentConfirmed",
            Self::PaymentDisputed { .. } => "PaymentDisputed",
            Self::ComdisRejected(_) => "ComdisRejected",
        }
    }

    /// Return `Some(&AbrechnungData)` when the invoice has been received.
    #[must_use]
    pub fn abrechnung_data(&self) -> Option<&AbrechnungData> {
        match self {
            Self::InvoicReceived(d)
            | Self::ValidationPassed(d)
            | Self::Settled(d)
            | Self::InvoicSent(d)
            | Self::PaymentConfirmed(d)
            | Self::ComdisRejected(d) => Some(d),
            Self::Disputed { data, .. } | Self::PaymentDisputed { data, .. } => Some(data),
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
        /// BO4E invoice domain object (EDIFACT → BO4E by `makod` adapter).
        /// `invoicd` uses this to run automated plausibility checks.
        rechnung: Option<Box<Rechnung>>,
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
    /// Invoicer role: record an outbound INVOIC that was sent to the payer.
    ///
    /// After recording this command the process waits in `InvoicSent` state
    /// until a REMADV (33001–33004) arrives from the payer.
    SendInvoic {
        /// Prüfidentifikator of the outbound INVOIC (31001–31008).
        pid: Pruefidentifikator,
        /// GLN of the sender (invoicer: NB/MSB).
        sender: MarktpartnerCode,
        /// GLN of the recipient (payer: LF/NB).
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference of the outbound INVOIC.
        invoice_ref: MessageRef,
    },
    /// Invoicer role: inbound REMADV received from the payer.
    ///
    /// PIDs 33001–33004 (REMADV AHB 1.0, GPKE Teil 2/3, BK6-24-174).
    /// - 33001 = full payment confirmed
    /// - 33002/33003/33004 = partial payment / rejection
    ReceiveRemadv {
        /// REMADV Prüfidentifikator (33001–33004).
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// GLN of the REMADV sender (payer).
        sender: MarktpartnerCode,
    },
    /// Payer role: inbound COMDIS 29001 received — invoicer rejected our REMADV.
    ///
    /// COMDIS PID 29001 (Ablehnung REMADV, COMDIS AHB 1.0, BK6-24-174).
    ReceiveComdis {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
}

impl CommandPayload for AbrechnungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE billing workflow for INVOIC-based processes.
///
/// Covers Netznutzungsabrechnung (31001/31002) and Mehr-/Mindermengen
/// (31005/31006). These are the GPKE-domain billing PIDs in INVOIC
/// AHB 2.8e / 1.0. PID 31004 belongs to `mako-wim-gas`; 31009 to `mako-wim`.
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
                rechnung,
            } => AbrechnungState::InvoicReceived(AbrechnungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                recipient: recipient.clone(),
                document_date: document_date.clone(),
                invoice_ref: invoice_ref.clone(),
                rechnung: rechnung.clone(),
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
                | AbrechnungState::Rejected { .. }
                | AbrechnungState::PaymentConfirmed(_)
                | AbrechnungState::PaymentDisputed { .. }
                | AbrechnungState::ComdisRejected(_) => state,
                _ => AbrechnungState::Rejected {
                    reason: format!("deadline expired: {label}"),
                },
            },

            AbrechnungEvent::InvoicSent {
                pruefidentifikator,
                sender,
                recipient,
                document_date,
                invoice_ref,
            } => AbrechnungState::InvoicSent(AbrechnungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                recipient: recipient.clone(),
                document_date: document_date.clone(),
                invoice_ref: invoice_ref.clone(),
                rechnung: None,
            }),

            AbrechnungEvent::RemadvReceived {
                pid, is_confirmed, ..
            } => match state {
                AbrechnungState::InvoicSent(data) => {
                    if *is_confirmed {
                        AbrechnungState::PaymentConfirmed(data)
                    } else {
                        AbrechnungState::PaymentDisputed {
                            remadv_pid: *pid,
                            data,
                        }
                    }
                }
                other => other,
            },

            AbrechnungEvent::ComdisAbLehnungReceived { .. } => match state {
                AbrechnungState::ValidationPassed(data)
                | AbrechnungState::InvoicSent(data)
                | AbrechnungState::Settled(data) => AbrechnungState::ComdisRejected(data),
                other => other,
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
                rechnung,
            } => {
                if !matches!(state, AbrechnungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !INVOIC_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a GPKE INVOIC PID (31001/31002/31005/31006), got {pid}",
                    )));
                }
                let mut events = vec![AbrechnungEvent::InvoicReceived {
                    invoice_ref: invoice_ref.clone(),
                    sender: sender.clone(),
                    recipient: recipient.clone(),
                    document_date: document_date.clone(),
                    pruefidentifikator: pid,
                    rechnung: rechnung.clone(),
                }];
                let mut outbox: Vec<PendingOutbox> = Vec::new();
                if validation_passed {
                    events.push(AbrechnungEvent::ValidationPassed {
                        invoice_ref: invoice_ref.clone(),
                    });
                    // Notify invoicd that a validated INVOIC is ready for plausibility
                    // checking.  The `Rechnung` BO4E object is embedded so that `invoicd`
                    // can run `InvoicCheckEngine::check` directly from the webhook payload
                    // without re-fetching the original EDIFACT message.
                    outbox.push(
                        PendingOutbox::new(
                            "ProcessInitiated",
                            recipient.as_str(),
                            serde_json::json!({
                                "pid":         pid.as_u32(),
                                "invoice_ref": invoice_ref.as_str(),
                                "sender_mp_id":  sender.as_str(),
                                "rechnung":    serde_json::to_value(rechnung.as_deref())
                                    .unwrap_or(serde_json::Value::Null),
                            }),
                        )
                        // Caused by ValidationPassed (index 1).
                        .caused_by(1),
                    );
                } else {
                    events.push(AbrechnungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(WorkflowOutput::with_outbox(events, outbox))
            }

            AbrechnungCommand::SettleInvoice => {
                if !matches!(state, AbrechnungState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                let pid = state
                    .abrechnung_data()
                    .map(|d| d.pruefidentifikator.as_u32())
                    .unwrap_or(0);
                let invoice_ref = state
                    .abrechnung_data()
                    .map(|d| d.invoice_ref.to_string())
                    .unwrap_or_default();
                let outbox = vec![PendingOutbox::new(
                    "ProcessCompleted",
                    "",
                    serde_json::json!({
                        "pid":         pid,
                        "invoice_ref": invoice_ref,
                        "outcome":     "settled",
                    }),
                )];
                Ok(WorkflowOutput::with_outbox(
                    vec![AbrechnungEvent::InvoiceSettled],
                    outbox,
                ))
            }

            AbrechnungCommand::DisputeInvoice { reason } => {
                if !matches!(state, AbrechnungState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                let pid = state
                    .abrechnung_data()
                    .map(|d| d.pruefidentifikator.as_u32())
                    .unwrap_or(0);
                let invoice_ref = state
                    .abrechnung_data()
                    .map(|d| d.invoice_ref.to_string())
                    .unwrap_or_default();
                let outbox = vec![PendingOutbox::new(
                    "ProcessCompleted",
                    "",
                    serde_json::json!({
                        "pid":         pid,
                        "invoice_ref": invoice_ref,
                        "outcome":     "disputed",
                        "reason":      &reason,
                    }),
                )];
                Ok(WorkflowOutput::with_outbox(
                    vec![AbrechnungEvent::InvoiceDisputed { reason }],
                    outbox,
                ))
            }

            AbrechnungCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    AbrechnungState::Settled(_)
                        | AbrechnungState::Disputed { .. }
                        | AbrechnungState::Rejected { .. }
                        | AbrechnungState::PaymentConfirmed(_)
                        | AbrechnungState::PaymentDisputed { .. }
                        | AbrechnungState::ComdisRejected(_)
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![AbrechnungEvent::DeadlineExpired { deadline_id, label }].into())
            }

            AbrechnungCommand::SendInvoic {
                pid,
                sender,
                recipient,
                document_date,
                invoice_ref,
            } => {
                if !matches!(state, AbrechnungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !INVOIC_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a GPKE INVOIC PID (31001/31002/31005/31006), got {pid}",
                    )));
                }
                Ok(vec![AbrechnungEvent::InvoicSent {
                    pruefidentifikator: pid,
                    sender,
                    recipient,
                    document_date,
                    invoice_ref,
                }]
                .into())
            }

            AbrechnungCommand::ReceiveRemadv {
                pid,
                remadv_ref,
                sender,
            } => {
                if !matches!(state, AbrechnungState::InvoicSent(_)) {
                    return Err(WorkflowError::invalid_state("InvoicSent", state.label()));
                }
                if !REMADV_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a GPKE REMADV PID (33001–33004), got {pid}",
                    )));
                }
                let is_confirmed = pid.as_u32() == 33001;
                Ok(vec![AbrechnungEvent::RemadvReceived {
                    pid,
                    remadv_ref,
                    sender,
                    is_confirmed,
                }]
                .into())
            }

            AbrechnungCommand::ReceiveComdis { comdis_ref } => {
                // COMDIS 29001 can arrive after validation (payer role) or after
                // settlement; guard only against terminal states.
                if matches!(
                    state,
                    AbrechnungState::New
                        | AbrechnungState::InvoicReceived(_)
                        | AbrechnungState::Rejected { .. }
                        | AbrechnungState::ComdisRejected(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed|Settled",
                        state.label(),
                    ));
                }
                Ok(vec![AbrechnungEvent::ComdisAbLehnungReceived { comdis_ref }].into())
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
            AbrechnungEvent::InvoicSent {
                pruefidentifikator, ..
            } => {
                record.status = "InvoicSent";
                record.pruefidentifikator = Some(pruefidentifikator);
            }
            AbrechnungEvent::RemadvReceived { is_confirmed, .. } => {
                record.status = if is_confirmed {
                    "PaymentConfirmed"
                } else {
                    "PaymentDisputed"
                };
            }
            AbrechnungEvent::ComdisAbLehnungReceived { .. } => {
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
