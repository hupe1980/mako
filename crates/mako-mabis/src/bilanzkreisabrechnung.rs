//! MABIS Bilanzkreisabrechnung — balance group settlement workflow (PID 13003).
//!
//! Models the Bilanzkreisverantwortlicher (BKV) side of the MaBiS
//! Bilanzkreisabrechnung Strom process (BNetzA BK6-24-174).
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174** — *Marktregeln für die Durchführung der Bilanzkreis-
//!   abrechnung Strom (MaBiS)*, Anlage 3
//! - **MSCONS 2.4c / 2.5** — Metered Services Consumption Report (Summenzeitreihen)
//!
//! # Process overview
//!
//! The **BIKO** (Bilanzkoordinator) calculates and sends an
//! `Abrechnungssummenzeitreihe` (billing summary time series) to the BKV.
//! The BKV must respond with a `Prüfmitteilung` (positive or negative) within
//! **1 Werktag** (BK6-24-174 §13.8). This workflow models the BKV's process
//! stream.
//!
//! ```text
//! BIKO                               BKV (this workflow)
//! ────                               ───────────────────
//! Abrechnungssummenzeitreihe ──────→ ReceiveSummenzeitreihe
//!                                        └─ SendPruefmitteilung (≤ 1 WT)
//!                           ←──────── Prüfmitteilung (positive / negative)
//! Datenstatus                ──────→ ReceiveDatastatus  →  Settled / Disputed
//! ```
//!
//! # Fristen (MaBiS BK6-24-174)
//!
//! | Milestone | Deadline |
//! |---|---|
//! | Preliminary billing dispatch (BIKO → BKV) | 18th Werktag after billing month |
//! | Final billing dispatch (BIKO → BKV) | 42nd Werktag after billing month |
//! | Prüfmitteilung (BKV → BIKO) | **1 Werktag** after receiving Abrechnungssummenzeitreihe |
//!
//! # Key difference from supplier-switch workflows
//!
//! Supplier-switch workflows (GPKE, WiM, GeLi Gas) are triggered by a single
//! inbound EDIFACT message and involve one delivery point (MeLo/MaLo).
//! MaBiS Bilanzkreisabrechnung aggregates time series across many MaLo
//! streams per billing period. The BIKO, not the BKV, performs the aggregation.

use std::collections::HashMap;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    projection::Projection,
    types::{BikoId, BillingPeriod, BkvId, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the MABIS Bilanzkreisabrechnung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum BillingEvent {
    /// BIKO sent the Abrechnungssummenzeitreihe to the BKV; billing period
    /// opened from the BKV's perspective.
    SummenzeitreiheReceived {
        /// Billing period (start and end date).
        billing_period: BillingPeriod,
        /// BK-Verantwortlicher (BKV) identifier.
        bkv_id: BkvId,
        /// Bilanzkoordinator (BIKO) identifier.
        biko_id: BikoId,
        /// BDEW Prüfidentifikator for the MSCONS message.
        pruefidentifikator: Pruefidentifikator,
        /// Billing version: `"vorlaeufig"` (preliminary, ≤ 18. WT) or
        /// `"endgueltig"` (final, ≤ 42. WT).
        version: BillingVersion,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// BKV sent a positive Prüfmitteilung to BIKO (accepts the billing).
    PruefmitteilungPositivSent {
        /// Message reference of the dispatched Prüfmitteilung.
        message_ref: MessageRef,
    },
    /// BKV sent a negative Prüfmitteilung to BIKO (disputes the billing).
    PruefmitteilungNegativSent {
        /// Message reference of the dispatched Prüfmitteilung.
        message_ref: MessageRef,
        /// Dispute reason sent to BIKO.
        reason: String,
    },
    /// BIKO sent the Datenstatus confirming settlement.
    DatenstatusReceived {
        /// Datenstatus value received from BIKO.
        data_status: DataStatus,
    },
    /// A registered Prüfmitteilung deadline expired before the BKV responded.
    PruefmitteilungDeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// An IFTSTA MaBiS status message was received (informational).
    ///
    /// Emitted for all inbound MaBiS IFTSTA PIDs **except** PID 21004 which
    /// drives the `DatenstatusReceived` event instead. Recorded for audit
    /// and read-model purposes; does not change the billing state.
    IftstaStatusReceived {
        /// IFTSTA Prüfidentifikator (21000–21005, ≠ 21004).
        pid: Pruefidentifikator,
        /// Sender party code (GLN).
        sender: MarktpartnerCode,
        /// Receiver party code (GLN).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
}

/// Billing version from the BIKO's Abrechnungssummenzeitreihe.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingVersion {
    /// Preliminary billing (vorläufig) — sent by ≤ 18th Werktag.
    Vorlaeufig,
    /// Final billing (endgültig) — sent by ≤ 42nd Werktag.
    Endgueltig,
}

/// Datenstatus values as defined in MaBiS BK6-24-174.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataStatus {
    /// `"Abrechnungsdaten"` — data used for billing.
    Abrechnungsdaten,
    /// `"abgerechnete Daten"` — settled final data.
    AbgerechtneteDaten,
    /// `"abgerechnete Daten KBKA"` — settled final data (KBKA variant).
    AbgerechtneteDatenKbka,
}

impl EventPayload for BillingEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::SummenzeitreiheReceived { .. } => "MabisSummenzeitreiheReceived",
            Self::PruefmitteilungPositivSent { .. } => "MabisPruefmitteilungPositivSent",
            Self::PruefmitteilungNegativSent { .. } => "MabisPruefmitteilungNegativSent",
            Self::DatenstatusReceived { .. } => "MabisDatenstatusReceived",
            Self::PruefmitteilungDeadlineExpired { .. } => "MabisPruefmitteilungDeadlineExpired",
            Self::IftstaStatusReceived { .. } => "MabisIftstaStatusReceived",
        }
    }
    // schema_version defaults to 1; increment and add an upcast arm on the
    // next backward-incompatible payload layout change.
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Data present from the moment the Abrechnungssummenzeitreihe arrives.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingData {
    /// Billing period (e.g. `"2025-09"`).
    pub billing_period: BillingPeriod,
    /// Bilanzkreisverantwortlicher (this BKV's ID).
    pub bkv_id: BkvId,
    /// Bilanzkoordinator who sent the billing data.
    pub biko_id: BikoId,
    /// BDEW Prüfidentifikator (13003 for Bilanzkreisabrechnung Strom —
    /// "Summenzeitreihen und Ausfallarbeitssummen", MSCONS AHB 2.4c/2.5).
    pub pruefidentifikator: Pruefidentifikator,
    /// Whether this is a preliminary or final billing.
    pub version: BillingVersion,
    /// Message reference of the inbound Abrechnungssummenzeitreihe.
    pub message_ref: MessageRef,
}

/// Current state of a MaBiS Bilanzkreisabrechnung process stream.
///
/// # Lifecycle
///
/// ```text
/// New
///  └─ SummenzeitreiheReceived
///       ├─ PruefmitteilungPositivSent → Settled (after DatenstatusReceived)
///       └─ PruefmitteilungNegativSent → Disputed
///  └─ PruefmitteilungDeadlineExpired → DeadlineExpired
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum BillingState {
    /// No events yet.
    New,
    /// Abrechnungssummenzeitreihe received from BIKO; Prüfmitteilung not yet sent.
    SummenzeitreiheReceived(BillingData),
    /// BKV sent positive Prüfmitteilung; awaiting Datenstatus from BIKO.
    PruefmitteilungSent(BillingData),
    /// BIKO confirmed settlement via Datenstatus.
    Settled(BillingData),
    /// BKV sent negative Prüfmitteilung; billing disputed.
    Disputed {
        /// Captured billing data.
        billing: BillingData,
        /// Dispute reason sent to BIKO.
        reason: String,
    },
    /// Prüfmitteilung deadline expired before the BKV responded.
    DeadlineExpired(BillingData),
}

impl Default for BillingState {
    fn default() -> Self {
        Self::New
    }
}

impl BillingState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::SummenzeitreiheReceived(_) => "SummenzeitreiheReceived",
            Self::PruefmitteilungSent(_) => "PruefmitteilungSent",
            Self::Settled(_) => "Settled",
            Self::Disputed { .. } => "Disputed",
            Self::DeadlineExpired(_) => "DeadlineExpired",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the MaBiS Bilanzkreisabrechnung workflow.
///
/// `Workflow::handle()` is pure — no I/O, no EDIFACT parsing, no store access.
#[derive(Clone)]
pub enum BillingCommand {
    /// BIKO sent the Abrechnungssummenzeitreihe; open the billing period from
    /// the BKV's perspective.
    ///
    /// `pid` must be 13003 (MSCONS Summenzeitreihe — "Summenzeitreihen und
    /// Ausfallarbeitssummen", MSCONS AHB 2.4c/2.5 §5).
    ReceiveSummenzeitreihe {
        /// MSCONS PID (13003).
        pid: Pruefidentifikator,
        /// Billing period (start and end date).
        billing_period: BillingPeriod,
        /// BK-Verantwortlicher (BKV) identifier.
        bkv_id: BkvId,
        /// Bilanzkoordinator (BIKO) identifier.
        biko_id: BikoId,
        /// Billing version (preliminary or final).
        version: BillingVersion,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// BKV accepts the billing — send positive Prüfmitteilung to BIKO.
    ///
    /// Must be issued within 1 Werktag of receiving the
    /// Abrechnungssummenzeitreihe (MaBiS BK6-24-174 §13.8).
    SendPruefmitteilungPositiv {
        /// Message reference assigned to the outbound Prüfmitteilung.
        message_ref: MessageRef,
    },
    /// BKV disputes the billing — send negative Prüfmitteilung to BIKO.
    ///
    /// Must be issued within 1 Werktag of receiving the
    /// Abrechnungssummenzeitreihe.
    SendPruefmitteilungNegativ {
        /// Message reference assigned to the outbound Prüfmitteilung.
        message_ref: MessageRef,
        /// Dispute reason sent to BIKO.
        reason: String,
    },
    /// BIKO confirmed settlement by sending Datenstatus.
    ReceiveDatastatus {
        /// Datenstatus value received from BIKO.
        data_status: DataStatus,
    },
    /// The 1-Werktag Prüfmitteilung deadline fired without a response.
    PruefmitteilungDeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// Received an IFTSTA MaBiS status message (PIDs 21000–21005).
    ///
    /// PID 21004 ("Statusmeldung vom BIKO an BKV/NB") drives the `Settled`
    /// state transition — the `data_status` field must be `Some` in that case.
    /// All other MaBiS IFTSTA PIDs are informational: they are accepted and
    /// recorded in the event log but do not change the billing state.
    ///
    /// **Note:** PID 21006 does not exist. PID 21007 is WiM Strom Teil 1 /
    /// WiM Gas and is NOT a MaBiS PID — it is registered in `mako-wim`.
    ///
    /// This command is constructed by the IFTSTA adapter in `makod` when an
    /// inbound AS4 IFTSTA message with a MaBiS PID arrives, or via the
    /// `"mabis.datenstatus.empfangen"` / `"mabis.iftsta.empfangen"` REST command.
    ReceiveIftsta {
        /// IFTSTA Prüfidentifikator (21000–21005).
        pid: Pruefidentifikator,
        /// Sender party code (GLN).
        sender: MarktpartnerCode,
        /// Receiver party code (GLN).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// Whether the IFTSTA passed AHB validation.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator, if any.
        validation_errors: Vec<String>,
        /// Datenstatus code extracted from the STS segment.
        ///
        /// Required for PID 21004 (Datenstatus from BIKO); `None` for all
        /// other MaBiS IFTSTA PIDs.
        data_status: Option<DataStatus>,
    },
}

impl CommandPayload for BillingCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// MaBiS Bilanzkreisabrechnung Strom (PID 13003) workflow.
///
/// Models the BKV's side of the MaBiS billing process (BK6-24-174 §13).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<MabisBillingWorkflow>(
///     tenant_id,
///     WorkflowId::new("mabis-billing", "FV2025-10-01"),
/// );
/// ```
pub struct MabisBillingWorkflow;

/// Deadline label for the 1-Werktag Prüfmitteilung response window.
///
/// Register a `Deadline` with this label immediately after
/// `ReceiveSummenzeitreihe` so the engine fires
/// `PruefmitteilungDeadlineExpired` if the BKV does not respond in time:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     received_at,
///     1,
///     mako_engine::fristen::HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(
///     process.stream_id().clone(), ..., PRUEFMITTEILUNG_DEADLINE_LABEL, due,
/// );
/// deadline_store.register(&deadline).await?;
/// ```
pub const PRUEFMITTEILUNG_DEADLINE_LABEL: &str = "mabis-pruefmitteilung-1-werktag";

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "mabis-billing";

/// All MaBiS IFTSTA Prüfidentifikatoren (21000–21005).
///
/// These are exchanged between LF, NB/ÜNB, BIKO, and BKV in the MaBiS process.
/// All are routed to `"mabis-billing"` so inbound messages can be correlated
/// with their billing stream via conversation ID.
///
/// PID [`IFTSTA_DATENSTATUS_PID`] (21004) drives the `PruefmitteilungSent` →
/// `Settled` transition. All others are informational.
///
/// **Note:** PID 21006 does not exist in any BDEW AHB. PID 21007 belongs to
/// WiM Strom Teil 1 / WiM Gas (NB→LF / NB→MSBA) and is registered in
/// `mako-wim` (`wim-device-change`), not here.
pub const IFTSTA_PIDS: &[u32] = &[21_000, 21_001, 21_002, 21_003, 21_004, 21_005];

/// IFTSTA PID 21004: "MaBiS / Statusmeldung vom BIKO an BKV/NB".
///
/// The only MaBiS IFTSTA PID that carries a `DataStatus` code and drives the
/// billing stream to `Settled`. All other MaBiS IFTSTA PIDs are informational.
pub const IFTSTA_DATENSTATUS_PID: u32 = 21_004;

impl Workflow for MabisBillingWorkflow {
    type State = BillingState;
    type Event = BillingEvent;
    type Command = BillingCommand;

    /// Deadline compensation: fire `PruefmitteilungDeadlineExpired` if the
    /// BKV has not responded within 1 Werktag.
    ///
    /// | Label | State guard | Command emitted |
    /// |---|---|---|
    /// | `"mabis-pruefmitteilung-1-werktag"` | `SummenzeitreiheReceived` | `PruefmitteilungDeadlineExpired` |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (PRUEFMITTEILUNG_DEADLINE_LABEL, BillingState::SummenzeitreiheReceived(_)) => {
                Some(BillingCommand::PruefmitteilungDeadlineExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            BillingEvent::SummenzeitreiheReceived {
                billing_period,
                bkv_id,
                biko_id,
                pruefidentifikator,
                version,
                message_ref,
            } => BillingState::SummenzeitreiheReceived(BillingData {
                billing_period: billing_period.clone(),
                bkv_id: bkv_id.clone(),
                biko_id: biko_id.clone(),
                pruefidentifikator: *pruefidentifikator,
                version: version.clone(),
                message_ref: message_ref.clone(),
            }),

            BillingEvent::PruefmitteilungPositivSent { .. } => {
                if let BillingState::SummenzeitreiheReceived(d) = state {
                    BillingState::PruefmitteilungSent(d)
                } else {
                    state
                }
            }

            BillingEvent::PruefmitteilungNegativSent { reason, .. } => match state {
                BillingState::SummenzeitreiheReceived(billing) => BillingState::Disputed {
                    billing,
                    reason: reason.clone(),
                },
                _ => state,
            },

            BillingEvent::DatenstatusReceived { .. } => {
                if let BillingState::PruefmitteilungSent(d) = state {
                    BillingState::Settled(d)
                } else {
                    state
                }
            }

            BillingEvent::PruefmitteilungDeadlineExpired { .. } => {
                if let BillingState::SummenzeitreiheReceived(d) = state {
                    BillingState::DeadlineExpired(d)
                } else {
                    state
                }
            }

            // Informational IFTSTA status messages do not change billing state.
            BillingEvent::IftstaStatusReceived { .. } => state,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            BillingCommand::ReceiveSummenzeitreihe {
                pid,
                billing_period,
                bkv_id,
                biko_id,
                version,
                message_ref,
            } => {
                if !matches!(state, BillingState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if pid.as_u32() != 13_003 {
                    return Err(WorkflowError::not_implemented(pid.as_u32()));
                }
                Ok(vec![BillingEvent::SummenzeitreiheReceived {
                    billing_period,
                    bkv_id,
                    biko_id,
                    pruefidentifikator: pid,
                    version,
                    message_ref,
                }]
                .into())
            }

            BillingCommand::SendPruefmitteilungPositiv { message_ref } => {
                if !matches!(state, BillingState::SummenzeitreiheReceived(_)) {
                    return Err(WorkflowError::invalid_state(
                        "SummenzeitreiheReceived",
                        state.status_str(),
                    ));
                }
                Ok(vec![BillingEvent::PruefmitteilungPositivSent { message_ref }].into())
            }

            BillingCommand::SendPruefmitteilungNegativ {
                message_ref,
                reason,
            } => {
                if !matches!(state, BillingState::SummenzeitreiheReceived(_)) {
                    return Err(WorkflowError::invalid_state(
                        "SummenzeitreiheReceived",
                        state.status_str(),
                    ));
                }
                Ok(vec![BillingEvent::PruefmitteilungNegativSent {
                    message_ref,
                    reason,
                }]
                .into())
            }

            BillingCommand::ReceiveDatastatus { data_status } => {
                if !matches!(state, BillingState::PruefmitteilungSent(_)) {
                    return Err(WorkflowError::invalid_state(
                        "PruefmitteilungSent",
                        state.status_str(),
                    ));
                }
                Ok(vec![BillingEvent::DatenstatusReceived { data_status }].into())
            }

            BillingCommand::PruefmitteilungDeadlineExpired { deadline_id, label } => {
                if !matches!(state, BillingState::SummenzeitreiheReceived(_)) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(
                    vec![BillingEvent::PruefmitteilungDeadlineExpired { deadline_id, label }]
                        .into(),
                )
            }

            BillingCommand::ReceiveIftsta {
                pid,
                sender,
                receiver,
                message_ref,
                data_status,
                ..
            } => {
                if pid.as_u32() == IFTSTA_DATENSTATUS_PID {
                    // PID 21004: Statusmeldung vom BIKO an BKV/NB.
                    // This is the Datenstatus confirmation that transitions
                    // the billing stream from PruefmitteilungSent → Settled.
                    if !matches!(state, BillingState::PruefmitteilungSent(_)) {
                        return Err(WorkflowError::invalid_state(
                            "PruefmitteilungSent",
                            state.status_str(),
                        ));
                    }
                    let ds = data_status.ok_or_else(|| {
                        WorkflowError::validation(
                            "IFTSTA PID 21004 (Datenstatus): \
                             STS segment DataStatus code is required",
                        )
                    })?;
                    Ok(vec![BillingEvent::DatenstatusReceived { data_status: ds }].into())
                } else {
                    // All other MaBiS IFTSTA PIDs are informational.
                    // Record in the event log; no state transition.
                    Ok(vec![BillingEvent::IftstaStatusReceived {
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
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single MaBiS billing process stream.
///
/// Uses a type-state design: the `Active` variant carries all domain fields
/// that are structurally guaranteed once the process moves past `New`,
/// eliminating `Option::unwrap()` at every field access.
#[derive(Debug)]
pub enum BillingRecord {
    /// No `SummenzeitreiheReceived` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `SummenzeitreiheReceived` event applied; billing data available.
    Active {
        /// Current lifecycle stage.
        status: &'static str,
        /// Billing period (start and end date).
        billing_period: BillingPeriod,
        /// BK-Verantwortlicher (BKV) identifier.
        bkv_id: BkvId,
        /// Bilanzkoordinator (BIKO) identifier.
        biko_id: BikoId,
        /// Billing version (preliminary or final).
        version: BillingVersion,
        /// Total events applied.
        event_count: usize,
    },
}

impl BillingRecord {
    /// Current lifecycle status label.
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

    /// Domain data if billing has been initiated, or `None` if still `New`.
    #[must_use]
    pub fn active_data(&self) -> Option<BillingRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                billing_period,
                bkv_id,
                biko_id,
                version,
                ..
            } => Some(BillingRecordData {
                billing_period,
                bkv_id,
                biko_id,
                version,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` `BillingRecord`.
#[derive(Debug, Clone, Copy)]
pub struct BillingRecordData<'a> {
    /// Billing period (start and end date).
    pub billing_period: &'a BillingPeriod,
    /// BK-Verantwortlicher (BKV) identifier.
    pub bkv_id: &'a BkvId,
    /// Bilanzkoordinator (BIKO) identifier.
    pub biko_id: &'a BikoId,
    /// Billing version (preliminary or final).
    pub version: &'a BillingVersion,
}

impl Default for BillingRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model that tracks status across MaBiS billing streams.
///
/// Feed via [`mako_engine::projection::ProjectionRunner`].
#[derive(Debug, Default)]
pub struct BillingProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, BillingRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for BillingProjection {
    fn name(&self) -> &'static str {
        "BillingProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<BillingEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            BillingRecord::New { event_count } => *event_count += 1,
            BillingRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            BillingEvent::SummenzeitreiheReceived {
                billing_period,
                bkv_id,
                biko_id,
                version,
                ..
            } => {
                let count = record.event_count();
                *record = BillingRecord::Active {
                    status: "SummenzeitreiheReceived",
                    billing_period,
                    bkv_id,
                    biko_id,
                    version,
                    event_count: count,
                };
            }
            BillingEvent::PruefmitteilungPositivSent { .. } => {
                if let BillingRecord::Active { status, .. } = record {
                    *status = "PruefmitteilungSent";
                }
            }
            BillingEvent::PruefmitteilungNegativSent { .. } => {
                if let BillingRecord::Active { status, .. } = record {
                    *status = "Disputed";
                }
            }
            BillingEvent::DatenstatusReceived { .. } => {
                if let BillingRecord::Active { status, .. } = record {
                    *status = "Settled";
                }
            }
            BillingEvent::PruefmitteilungDeadlineExpired { .. } => {
                if let BillingRecord::Active { status, .. } = record {
                    *status = "DeadlineExpired";
                }
            }
            BillingEvent::IftstaStatusReceived { .. } => {
                // Informational — does not change the status label.
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn receive_cmd(version: BillingVersion) -> BillingCommand {
        BillingCommand::ReceiveSummenzeitreihe {
            pid: Pruefidentifikator::new(13_003).expect("13003 is valid"),
            billing_period: BillingPeriod::new("2025-09"),
            bkv_id: BkvId::new("BKV-DE-001"),
            biko_id: BikoId::new("BIKO-DE-001"),
            version,
            message_ref: MessageRef::new("MSCONS-BKA-2025-09-001"),
        }
    }

    #[test]
    fn happy_path_positive_pruefmitteilung_to_settled() {
        let state = BillingState::default();

        // Step 1: receive Abrechnungssummenzeitreihe
        let events = MabisBillingWorkflow::handle(&state, receive_cmd(BillingVersion::Vorlaeufig))
            .expect("should accept PID 13003");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            BillingEvent::SummenzeitreiheReceived { .. }
        ));

        let state = events.iter().fold(state, MabisBillingWorkflow::apply);
        assert_eq!(state.status_str(), "SummenzeitreiheReceived");

        // Step 2: BKV sends positive Prüfmitteilung
        let events = MabisBillingWorkflow::handle(
            &state,
            BillingCommand::SendPruefmitteilungPositiv {
                message_ref: MessageRef::new("PRUEF-POS-001"),
            },
        )
        .expect("positive Prüfmitteilung must be accepted");
        let state = events.iter().fold(state, MabisBillingWorkflow::apply);
        assert_eq!(state.status_str(), "PruefmitteilungSent");

        // Step 3: BIKO sends Datenstatus → Settled
        let events = MabisBillingWorkflow::handle(
            &state,
            BillingCommand::ReceiveDatastatus {
                data_status: DataStatus::AbgerechtneteDaten,
            },
        )
        .expect("ReceiveDatastatus from PruefmitteilungSent must succeed");
        let state = events.iter().fold(state, MabisBillingWorkflow::apply);
        assert_eq!(state.status_str(), "Settled");
    }

    #[test]
    fn negative_pruefmitteilung_transitions_to_disputed() {
        let state = BillingState::default();
        let events =
            MabisBillingWorkflow::handle(&state, receive_cmd(BillingVersion::Endgueltig)).unwrap();
        let state = events.iter().fold(state, MabisBillingWorkflow::apply);

        let events = MabisBillingWorkflow::handle(
            &state,
            BillingCommand::SendPruefmitteilungNegativ {
                message_ref: MessageRef::new("PRUEF-NEG-001"),
                reason: "Zählpunkt DE000... fehlt in der Summenzeitreihe".to_owned(),
            },
        )
        .expect("negative Prüfmitteilung must be accepted");
        let state = events.iter().fold(state, MabisBillingWorkflow::apply);
        assert_eq!(state.status_str(), "Disputed");
    }

    #[test]
    fn wrong_pid_returns_not_implemented() {
        let state = BillingState::default();
        let err = MabisBillingWorkflow::handle(
            &state,
            BillingCommand::ReceiveSummenzeitreihe {
                pid: Pruefidentifikator::new(55_001).expect("valid pid"),
                billing_period: BillingPeriod::new("2025-09"),
                bkv_id: BkvId::new("BKV-001"),
                biko_id: BikoId::new("BIKO-001"),
                version: BillingVersion::Vorlaeufig,
                message_ref: MessageRef::new("REF-001"),
            },
        )
        .expect_err("PID 55001 must be rejected");
        assert!(err.is_not_implemented(), "{err}");
    }

    #[test]
    fn pruefmitteilung_in_wrong_state_is_rejected() {
        let state = BillingState::New;
        let err = MabisBillingWorkflow::handle(
            &state,
            BillingCommand::SendPruefmitteilungPositiv {
                message_ref: MessageRef::new("REF"),
            },
        )
        .expect_err("must fail on New state");
        assert!(err.to_string().contains("SummenzeitreiheReceived"), "{err}");
    }

    #[test]
    fn deadline_expired_in_summenzeitreihe_received_state() {
        let state = BillingState::default();
        let events =
            MabisBillingWorkflow::handle(&state, receive_cmd(BillingVersion::Vorlaeufig)).unwrap();
        let state = events.iter().fold(state, MabisBillingWorkflow::apply);

        let events = MabisBillingWorkflow::handle(
            &state,
            BillingCommand::PruefmitteilungDeadlineExpired {
                deadline_id: DeadlineId::new(),
                label: PRUEFMITTEILUNG_DEADLINE_LABEL.into(),
            },
        )
        .expect("deadline in SummenzeitreiheReceived must be accepted");
        let state = events.iter().fold(state, MabisBillingWorkflow::apply);
        assert_eq!(state.status_str(), "DeadlineExpired");
    }

    #[test]
    fn deadline_expired_in_terminal_state_is_noop() {
        let state = BillingState::default();
        let events =
            MabisBillingWorkflow::handle(&state, receive_cmd(BillingVersion::Vorlaeufig)).unwrap();
        let state = events.iter().fold(state, MabisBillingWorkflow::apply);
        let events = MabisBillingWorkflow::handle(
            &state,
            BillingCommand::SendPruefmitteilungNegativ {
                message_ref: MessageRef::new("REF"),
                reason: "disputed".to_owned(),
            },
        )
        .unwrap();
        let state = events.iter().fold(state, MabisBillingWorkflow::apply);
        assert_eq!(state.status_str(), "Disputed");

        // Deadline firing after Disputed is a no-op
        let events = MabisBillingWorkflow::handle(
            &state,
            BillingCommand::PruefmitteilungDeadlineExpired {
                deadline_id: DeadlineId::new(),
                label: PRUEFMITTEILUNG_DEADLINE_LABEL.into(),
            },
        )
        .expect("deadline in terminal state must produce empty events");
        assert!(events.is_empty(), "no events expected in terminal state");
    }
}
