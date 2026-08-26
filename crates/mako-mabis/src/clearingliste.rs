//! MaBiS Clearingliste workflows — clearing-list distribution in the
//! Bilanzkreisabrechnung settlement process (BNetzA BK6-24-174).
//!
//! Four **receive-and-record** UTILMD lists. Nothing is owed back on any of
//! them: no Antwort PID, no Korrekturliste, no deadline.
//!
//! # What makes a list record-only
//!
//! Not "it looks like a list". A list is record-only when the BDEW
//! *Anwendungsübersicht Prüfidentifikatoren* gives it **no answering
//! Prozessschritt**. Three of the four here are the *delivery* leg of a request
//! the counterparty already made (ORDERS 17203/17204/17205/17208 →
//! [`crate::anforderung`]); the fourth is a broadcast of profile definitions.
//!
//! The list that is **not** here is the Lieferantenclearingliste **55065**. It
//! carries a Prozessschritt-3 answer — 55066 „Korrekturliste zu
//! Lieferantenclearingliste" — and therefore lives in
//! [`crate::listenabgleich`]. Filing it as record-only drops the LF's
//! correction obligation, and the drop is invisible: the list still arrives,
//! still validates, still gets stored.
//!
//! ```text
//! NB / ÜNB ──(55067 Bilanzkreiszuordnungsliste)──→  BKV
//! BIKO ────┬─(55069 Clearingliste DZR)──────────→  NB / ÜNB
//!          └─(55070 Clearingliste BAS)──────────→  BKV
//! NB ──────┬─(55073 Liste der Profildefinitionen)→  LF
//!          └─(55073 Liste der Profildefinitionen)→  MSB
//! ```
//!
//! # Prüfidentifikatoren
//!
//! Verified against the BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0*
//! (01.04.2026), sheet *Prüf-ID Prozessschritt*.
//!
//! | PID   | Liste                          | Von → An        | Prozessschritt | Anfordernde ORDERS |
//! |-------|--------------------------------|-----------------|---------------:|--------------------|
//! | 55067 | Bilanzkreiszuordnungsliste     | NB/ÜNB → BKV    | 2              | 17203              |
//! | 55069 | Clearingliste DZR              | BIKO → NB/ÜNB   | 3              | 17205 / 17208      |
//! | 55070 | Clearingliste BAS              | BIKO → BKV      | 3              | 17204              |
//! | 55073 | Liste der Profildefinitionen   | NB → LF/MSB     | 1              | —                  |
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 3 MaBiS** — Kap. 6.3/6.7 (Profildefinitionen),
//!   Kap. 10.6/11.4 (Bilanzkreiszuordnungsliste), Kap. 13.11–13.13
//!   (Clearinglisten BAS / DZR / ÜNB-DZR)
//! - **UTILMD AHB Strom S2.1 / S2.2** — EDI@Energy message format
//!
//! # State machine
//!
//! All four PIDs share the same state machine:
//!
//! ```text
//! New
//!  └─ ClearinglisteErhalten ──── (ValidationPassed) ──→ ValidationPassed (terminal)
//!                           └─── (ValidationFailed) ──→ ValidationFailed (terminal)
//! ```

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
/// | PID   | Liste                        | Von → An      |
/// |-------|------------------------------|---------------|
/// | 55067 | Bilanzkreiszuordnungsliste   | NB/ÜNB → BKV  |
/// | 55069 | Clearingliste DZR            | BIKO → NB/ÜNB |
/// | 55070 | Clearingliste BAS            | BIKO → BKV    |
/// | 55073 | Liste der Profildefinitionen | NB → LF/MSB   |
///
/// **55065 is deliberately absent**: it owes a 55066 Korrekturliste and is
/// handled by [`crate::listenabgleich`].
pub const CLEARINGLISTE_PIDS: &[u32] = &[55067, 55069, 55070, 55073];

/// Stable workflow name for process routing.
pub const WORKFLOW_NAME: &str = "mabis-clearingliste";

// ── ClearinglisteKind ─────────────────────────────────────────────────────────

/// Which variant of clearing list this workflow instance received.
///
/// Derived from the inbound PID and stored in every event for auditability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearinglisteKind {
    /// PID 55067 — Bilanzkreiszuordnungsliste (NB / ÜNB → BKV), the delivery
    /// leg of ORDERS 17203.
    Bilanzkreiszuordnungsliste,
    /// PID 55069 — Clearingliste DZR (BIKO → NB / ÜNB), the delivery leg of
    /// ORDERS 17205 (NB) and 17208 (ÜNB).
    ClearinglisteDzr,
    /// PID 55070 — Clearingliste BAS (BIKO → BKV), the delivery leg of
    /// ORDERS 17204.
    ClearinglisteBas,
    /// PID 55073 — Liste der Profildefinitionen (NB → LF / MSB).
    Profildefinitionen,
}

impl ClearinglisteKind {
    /// Derive the variant from a raw Prüfidentifikator value.
    ///
    /// Returns `None` for PIDs that are not handled by this workflow.
    #[must_use]
    pub fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            55067 => Some(Self::Bilanzkreiszuordnungsliste),
            55069 => Some(Self::ClearinglisteDzr),
            55070 => Some(Self::ClearinglisteBas),
            55073 => Some(Self::Profildefinitionen),
            _ => None,
        }
    }

    /// Return the canonical BDEW AHB process name.
    #[must_use]
    pub fn process_name(self) -> &'static str {
        match self {
            Self::Bilanzkreiszuordnungsliste => "Bilanzkreiszuordnungsliste",
            Self::ClearinglisteDzr => "Clearingliste DZR",
            Self::ClearinglisteBas => "Clearingliste BAS",
            Self::Profildefinitionen => "Liste der Profildefinitionen",
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
        /// BDEW Prüfidentifikator (55067, 55069, 55070 or 55073).
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
#[derive(Default)]
pub enum ClearinglisteState {
    /// No events yet.
    #[default]
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
    /// Inbound Clearingliste UTILMD received (PIDs 55067, 55069, 55070, 55073).
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
/// Handles PIDs 55067 (Bilanzkreiszuordnungsliste), 55069 (Clearingliste DZR),
/// 55070 (Clearingliste BAS) and 55073 (Liste der Profildefinitionen) in the
/// MaBiS settlement cycle (BK6-24-174 Anlage 3).
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
                         (erwartet 55067, 55069, 55070 oder 55073; \
                         55065 gehört zu mabis-listenabgleich)"
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
