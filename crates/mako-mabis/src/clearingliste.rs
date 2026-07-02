//! MaBiS Clearingliste workflows — clearing-list distribution in the
//! Bilanzkreisabrechnung settlement process (BNetzA BK6-24-174).
//!
//! Implements three related **receive-and-validate** workflows for UTILMD-based
//! clearing-list messages published by the BIKO or NB during the MaBiS
//! settlement cycle.
//!
//! # Process overview
//!
//! After the billing period closes, the BIKO distributes clearing lists that
//! document the final assignment of market locations to suppliers and balance
//! groups. These messages are **inbound-only** documents — no response is
//! expected from the receiving party beyond the implicit APERAK acknowledgement
//! at the AS4 transport level.
//!
//! ```text
//! BIKO ──┬──(UTILMD 55069 Clearingliste DZR)──→  NB / ÜNB (this workflow)
//!        └──(UTILMD 55070 Clearingliste BAS)──→  BKV       (this workflow)
//! NB   ─────(UTILMD 55065 Lieferantenclearingliste)──→  LF (this workflow)
//! ```
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom FV2025-10-01)
//!
//! | PID   | Process name (AHB)                        | Direction       |
//! |-------|-------------------------------------------|-----------------|
//! | 55065 | Lieferantenclearingliste                  | NB → LF         |
//! | 55069 | Clearingliste DZR                         | BIKO → NB / ÜNB |
//! | 55070 | Clearingliste BAS                         | BIKO → BKV      |
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 3 MaBiS** — Clearingverfahren
//! - **UTILMD S2.1 / S2.2** — EDI@Energy message format
//!
//! # State machine
//!
//! All three PIDs share the same state machine:
//!
//! ```text
//! New
//!  └─ ClearinglisteErhalten ──── (ValidationPassed) ──→ ValidationPassed (terminal)
//!                           └─── (ValidationFailed) ──→ ValidationFailed (terminal)
//! ```
//!
//! No outbound messages and no deadline are associated with these workflows.
//! The absence of a follow-up APERAK deadline distinguishes them from
//! request–response workflows such as `gpke-supplier-change` or `mabis-billing`.
//!
//! # Notes on PID 55065
//!
//! The Lieferantenclearingliste (PID 55065) is sent by the NB to the LF at the
//! end of each billing period to communicate the final clearing list of assigned
//! market locations. It is listed under MaBiS in BK6-24-174 Anlage 3 because it
//! is part of the billing coordination cycle even though the sender is the NB.

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    types::{BillingPeriod, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// All UTILMD Clearingliste Prüfidentifikatoren handled by
/// [`MabisClearinglisteWorkflow`].
///
/// | PID   | Process (AHB name)              | Direction       |
/// |-------|---------------------------------|-----------------|
/// | 55065 | Lieferantenclearingliste        | NB → LF         |
/// | 55069 | Clearingliste DZR               | BIKO → NB / ÜNB |
/// | 55070 | Clearingliste BAS               | BIKO → BKV      |
pub const CLEARINGLISTE_PIDS: &[u32] = &[55065, 55069, 55070];

/// Stable workflow name for process routing.
pub const WORKFLOW_NAME: &str = "mabis-clearingliste";

// ── ClearinglisteKind ─────────────────────────────────────────────────────────

/// Which variant of clearing list this workflow instance received.
///
/// Derived from the inbound PID and stored in every event for auditability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearinglisteKind {
    /// PID 55065 — Lieferantenclearingliste (NB → LF).
    Lieferantenclearingliste,
    /// PID 55069 — Clearingliste DZR (BIKO → NB / ÜNB).
    ClearinglisteDzr,
    /// PID 55070 — Clearingliste BAS (BIKO → BKV).
    ClearinglisteBas,
}

impl ClearinglisteKind {
    /// Derive the variant from a raw Prüfidentifikator value.
    ///
    /// Returns `None` for PIDs that are not handled by this workflow.
    #[must_use]
    pub fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            55065 => Some(Self::Lieferantenclearingliste),
            55069 => Some(Self::ClearinglisteDzr),
            55070 => Some(Self::ClearinglisteBas),
            _ => None,
        }
    }

    /// Return the canonical BDEW AHB process name.
    #[must_use]
    pub fn process_name(self) -> &'static str {
        match self {
            Self::Lieferantenclearingliste => "Lieferantenclearingliste",
            Self::ClearinglisteDzr => "Clearingliste DZR",
            Self::ClearinglisteBas => "Clearingliste BAS",
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a Clearingliste UTILMD is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClearinglisteData {
    /// BDEW Prüfidentifikator of the inbound UTILMD.
    pub pruefidentifikator: Pruefidentifikator,
    /// Which variant of clearing list this is.
    pub kind: ClearinglisteKind,
    /// GLN of the sending party (BIKO or NB).
    pub sender: MarktpartnerCode,
    /// GLN of the receiving party (NB, ÜNB, BKV, or LF).
    pub receiver: MarktpartnerCode,
    /// Billing period this clearing list covers (e.g. `"2025-09"`).
    ///
    /// Extracted from the DTM segment with date qualifier `137`
    /// (document date / Erstellungsdatum) or derived from the UNB header date.
    /// May be empty if the period cannot be extracted from the UTILMD payload.
    pub billing_period: BillingPeriod,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the MaBiS Clearingliste workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClearinglisteEvent {
    /// Inbound Clearingliste UTILMD received.
    ClearinglisteErhalten {
        /// BDEW Prüfidentifikator (55065, 55069, or 55070).
        pruefidentifikator: Pruefidentifikator,
        /// Clearing list variant derived from the PID.
        kind: ClearinglisteKind,
        /// GLN of the sending party.
        sender: MarktpartnerCode,
        /// GLN of the receiving party.
        receiver: MarktpartnerCode,
        /// Billing period this clearing list covers.
        billing_period: BillingPeriod,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// EDIFACT message passed profile validation (terminal).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// EDIFACT message failed profile validation (terminal).
    ValidationFailed {
        /// Human-readable summary of validation errors.
        reason: String,
    },
}

impl EventPayload for ClearinglisteEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::ClearinglisteErhalten { .. } => "MabisClearinglisteErhalten",
            Self::ValidationPassed { .. } => "MabisClearinglisteValidationPassed",
            Self::ValidationFailed { .. } => "MabisClearinglisteValidationFailed",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a MaBiS Clearingliste process stream.
///
/// # Lifecycle
///
/// ```text
/// New
///  └─ ClearinglisteErhalten → ValidationPassed (terminal)
///                           ↘ ValidationFailed (terminal)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum ClearinglisteState {
    /// No events yet.
    New,
    /// Clearingliste UTILMD received; awaiting validation result.
    Erhalten(ClearinglisteData),
    /// Validation passed; clearing list is available for downstream processing (terminal).
    ValidationPassed(ClearinglisteData),
    /// Validation failed (terminal).
    ValidationFailed {
        /// Validation error reason.
        reason: String,
    },
}

impl Default for ClearinglisteState {
    fn default() -> Self {
        Self::New
    }
}

impl ClearinglisteState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Erhalten(_) => "Erhalten",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the MaBiS Clearingliste workflow.
///
/// `Workflow::handle()` is pure — no I/O, no EDIFACT parsing, no store access.
#[derive(Clone)]
pub enum ClearinglisteCommand {
    /// Inbound Clearingliste UTILMD received (PIDs 55065, 55069, or 55070).
    ///
    /// Constructed by the EDIFACT adapter in `makod` when a UTILMD with one of
    /// the handled PIDs arrives on the AS4 inbound channel.
    ReceiveClearingliste {
        /// BDEW Prüfidentifikator of the inbound UTILMD.
        pid: Pruefidentifikator,
        /// GLN of the sending party (BIKO or NB).
        sender: MarktpartnerCode,
        /// GLN of the receiving party (NB, ÜNB, BKV, or LF).
        receiver: MarktpartnerCode,
        /// Billing period this clearing list covers.
        billing_period: BillingPeriod,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed.
        validation_passed: bool,
        /// Human-readable validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
}

impl CommandPayload for ClearinglisteCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// MaBiS Clearingliste workflow — handles inbound clearing-list UTILMD messages.
///
/// Handles PIDs 55065 (Lieferantenclearingliste), 55069 (Clearingliste DZR),
/// and 55070 (Clearingliste BAS) in the MaBiS settlement cycle (BK6-24-174
/// Anlage 3).
///
/// This workflow is purely receive-and-record: it validates the inbound UTILMD
/// and stores the clearing data for downstream billing-period projection,
/// read-model queries, and ERP webhook delivery.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<MabisClearinglisteWorkflow>(
///     tenant_id,
///     WorkflowId::new("mabis-clearingliste", "FV2025-10-01"),
/// );
/// process.execute(ClearinglisteCommand::ReceiveClearingliste { ... }).await?;
/// ```
pub struct MabisClearinglisteWorkflow;

impl Workflow for MabisClearinglisteWorkflow {
    type State = ClearinglisteState;
    type Event = ClearinglisteEvent;
    type Command = ClearinglisteCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            ClearinglisteEvent::ClearinglisteErhalten {
                pruefidentifikator,
                kind,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
            } => ClearinglisteState::Erhalten(ClearinglisteData {
                pruefidentifikator: *pruefidentifikator,
                kind: *kind,
                sender: sender.clone(),
                receiver: receiver.clone(),
                billing_period: billing_period.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            }),

            ClearinglisteEvent::ValidationPassed { .. } => match state {
                ClearinglisteState::Erhalten(data) => ClearinglisteState::ValidationPassed(data),
                other => other,
            },

            ClearinglisteEvent::ValidationFailed { reason } => {
                ClearinglisteState::ValidationFailed {
                    reason: reason.clone(),
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            ClearinglisteCommand::ReceiveClearingliste {
                pid,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, ClearinglisteState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let kind = ClearinglisteKind::from_pid(pid.as_u32()).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "PID {pid} is not a handled Clearingliste PID \
                         (expected 55065, 55069, or 55070)"
                    ))
                })?;

                let mut events = vec![ClearinglisteEvent::ClearinglisteErhalten {
                    pruefidentifikator: pid,
                    kind,
                    sender,
                    receiver,
                    billing_period,
                    document_date,
                    message_ref: message_ref.clone(),
                }];

                if validation_passed {
                    events.push(ClearinglisteEvent::ValidationPassed { message_ref });
                } else {
                    events.push(ClearinglisteEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    });
                }

                Ok(events.into())
            }
        }
    }
}
