//! GPKE Konfigurationseinrichtung — ORDERS 17134/17135 workflow (GPKE Teil 4).
//!
//! After a successful Lieferbeginn (UTILMD 55001 → UTILMD 55003 accepted), the
//! Netzbetreiber (NB) is required by BDEW GPKE Teil 4 to notify the
//! Messstellenbetreiber (MSB) to set up the metering configuration for the new
//! supplier assignment. This is done via ORDERS 17134.
//!
//! The MSB responds with ORDRSP 19001 (Bestätigung) or ORDRSP 19002 (Ablehnung).
//!
//! # Process overview
//!
//! ```text
//! NB sends ORDERS 17134 → MSB
//!                          ↓
//!                MSB responds ORDRSP 19001 / 19002
//!                          ↓
//!       KonfigurationsWorkflow → Bestätigt | Abgelehnt
//! ```
//!
//! # Prüfidentifikatoren (LFW24 ORDERS/ORDRSP AHB)
//!
//! ## Outbound ORDERS (NB → MSB) — NOT routed as inbound
//!
//! | PID   | Process name (AHB)                                         |
//! |-------|------------------------------------------------------------|
//! | 17134 | Einrichtung Konfiguration aufgrund Zuordnung LF (NB an MSB)|
//! | 17135 | Einrichtung Konfiguration aufgrund Zuordnung LF (MSB an MSB)|
//!
//! ## Inbound ORDRSP responses — routed to this workflow
//!
//! | PID   | Process name (AHB)         | Caused by |
//! |-------|---------------------------|-----------|
//! | 19001 | Bestellbestätigung         | 17134/17135 accept |
//! | 19002 | Ablehnung der Bestellung   | 17134/17135 reject |
//!
//! **Note:** PIDs 19001/19002 are shared across all ORDERS processes. The
//! router correlates inbound ORDRSP messages back to their parent process via
//! the `reference_message_id` field (BGM reference in ORDRSP → original
//! ORDERS message reference).
//!
//! # Triggering this workflow
//!
//! `GpkeKonfigurationWorkflow` is triggered by the outbox entry of type
//! `KonfigurationseinrichtungRequired` (emitted by
//! [`GpkeSupplierChangeWorkflow`] when UTILMD 55001 is accepted). The makod
//! adapter picks up the outbox entry and spawns a new process via
//! `KonfigurationCommand::NbSendsBeauftragung`.
//!
//! [`GpkeSupplierChangeWorkflow`]: crate::wechselprozesse::GpkeSupplierChangeWorkflow

use std::collections::HashMap;

use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-konfiguration";

/// Outbound ORDERS PIDs dispatched by this workflow (NB → MSB).
///
/// These are NOT inbound routing PIDs; they appear in outbox entries only.
/// 17134 = NB an MSB, 17135 = MSB an MSB (forwarded by NB outbox).
pub const ORDERS_PIDS: &[u32] = &[17134, 17135];

/// Inbound ORDRSP PIDs routed back to this workflow (MSB → NB).
///
/// 19001 = Bestellbestätigung (accept), 19002 = Ablehnung (reject).
pub const ORDRSP_PIDS: &[u32] = &[19001, 19002];

/// Deadline label for the ORDRSP response window.
///
/// BDEW GPKE Teil 4: MSB must respond with ORDRSP within **5 Werktage**.
/// Register a `Deadline` with this label immediately after `BeauftragungGesendet`:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     sent_at, 5, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., KONFIGURATION_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const KONFIGURATION_WINDOW_LABEL: &str = "konfiguration-deadline";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Konfigurationseinrichtung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KonfigurationEvent {
    /// NB sent ORDERS 17134 to the MSB.
    BeauftragungGesendet {
        /// Prüfidentifikator of the sent ORDERS (17134 = NB an MSB).
        orders_pid: Pruefidentifikator,
        /// GLN of the MSB that received the ORDERS.
        msb_mp_id: MarktpartnerCode,
        /// EIC/MaLo of the supply location.
        malo: MaLo,
        /// GLN of the new supplier (LFN).
        new_supplier: MarktpartnerCode,
        /// Reference ID of the sent ORDERS message.
        message_ref: MessageRef,
    },
    /// MSB accepted the configuration order (ORDRSP 19001).
    BestaetigungErhalten {
        /// ORDRSP Prüfidentifikator (19001).
        response_pid: Pruefidentifikator,
        /// Reference ID of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// MSB rejected the configuration order (ORDRSP 19002).
    AblehungErhalten {
        /// ORDRSP Prüfidentifikator (19002).
        response_pid: Pruefidentifikator,
        /// Human-readable rejection reason from the ORDRSP.
        reason: String,
        /// Reference ID of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// A registered deadline expired before the process completed.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for KonfigurationEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::BeauftragungGesendet { .. } => "KonfigurationBeauftragungGesendet",
            Self::BestaetigungErhalten { .. } => "KonfigurationBestaetigungErhalten",
            Self::AblehungErhalten { .. } => "KonfigurationAblehungErhalten",
            Self::DeadlineExpired { .. } => "KonfigurationDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data set when `BeauftragungGesendet` is applied.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeauftragungData {
    /// Prüfidentifikator of the sent ORDERS (17134).
    pub orders_pid: Pruefidentifikator,
    /// GLN of the addressed MSB.
    pub msb_mp_id: MarktpartnerCode,
    /// EIC/MaLo of the supply location.
    pub malo: MaLo,
    /// GLN of the new supplier (LFN).
    pub new_supplier: MarktpartnerCode,
    /// Message reference of the sent ORDERS (for ORDRSP correlation).
    pub message_ref: MessageRef,
}

/// Current state of a GPKE Konfigurationseinrichtung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → Beauftragt → Bestätigt
///                 ↘ Abgelehnt
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum KonfigurationState {
    /// No ORDERS sent yet.
    New,
    /// ORDERS 17134 was sent; waiting for ORDRSP.
    Beauftragt(BeauftragungData),
    /// MSB accepted (ORDRSP 19001).
    Bestaetigt {
        /// Beauftragung data from the original ORDERS.
        data: BeauftragungData,
        /// Message reference of the inbound ORDRSP.
        response_ref: MessageRef,
    },
    /// MSB rejected (ORDRSP 19002).
    Abgelehnt {
        /// Beauftragung data from the original ORDERS.
        data: BeauftragungData,
        /// Human-readable rejection reason from the MSB.
        reason: String,
    },
}

impl Default for KonfigurationState {
    fn default() -> Self {
        Self::New
    }
}

impl KonfigurationState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Beauftragt(_) => "Beauftragt",
            Self::Bestaetigt { .. } => "Bestaetigt",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Konfigurationseinrichtung workflow.
#[derive(Clone)]
pub enum KonfigurationCommand {
    /// NB has sent ORDERS 17134 to the MSB.
    ///
    /// Produced by the makod adapter when it processes a
    /// `KonfigurationseinrichtungRequired` outbox entry from
    /// [`GpkeSupplierChangeWorkflow`].
    ///
    /// [`GpkeSupplierChangeWorkflow`]: crate::wechselprozesse::GpkeSupplierChangeWorkflow
    NbSendsBeauftragung {
        /// Prüfidentifikator of the ORDERS sent (17134 or 17135).
        orders_pid: Pruefidentifikator,
        /// GLN of the MSB receiving the ORDERS.
        msb_mp_id: MarktpartnerCode,
        /// EIC/MaLo of the affected supply location.
        malo: MaLo,
        /// GLN of the new supplier (LFN).
        new_supplier: MarktpartnerCode,
        /// Message reference of the sent ORDERS (used for ORDRSP correlation).
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP received from MSB.
    ///
    /// Contains the MSB's acceptance or rejection of the ORDERS 17134.
    ReceiveOrdrsp {
        /// ORDRSP Prüfidentifikator (19001 = accept, 19002 = reject).
        pid: Pruefidentifikator,
        /// `true` if the MSB accepted (PID 19001); `false` if rejected (19002).
        accepted: bool,
        /// Optional rejection reason from the ORDRSP text segment.
        reason: Option<String>,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for KonfigurationCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Konfigurationseinrichtung workflow — tracks the NB's obligation to
/// send ORDERS 17134 to the MSB after confirming a supplier change (PID 55001).
pub struct GpkeKonfigurationWorkflow;

impl Workflow for GpkeKonfigurationWorkflow {
    type State = KonfigurationState;
    type Event = KonfigurationEvent;
    type Command = KonfigurationCommand;

    /// Deadline compensation for the GPKE Konfiguration ORDRSP window.
    ///
    /// | Label | State guard | Command emitted | Rule |
    /// |---|---|---|---|
    /// | `"konfiguration-deadline"` | `Beauftragt` | `TimeoutExpired` | BDEW GPKE Teil 4 — 5 Werktage |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (KONFIGURATION_WINDOW_LABEL, KonfigurationState::Beauftragt(_)) => {
                Some(KonfigurationCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            KonfigurationEvent::BeauftragungGesendet {
                orders_pid,
                msb_mp_id,
                malo,
                new_supplier,
                message_ref,
            } => KonfigurationState::Beauftragt(BeauftragungData {
                orders_pid: *orders_pid,
                msb_mp_id: msb_mp_id.clone(),
                malo: malo.clone(),
                new_supplier: new_supplier.clone(),
                message_ref: message_ref.clone(),
            }),
            KonfigurationEvent::BestaetigungErhalten { message_ref, .. } => {
                match state {
                    KonfigurationState::Beauftragt(data) => KonfigurationState::Bestaetigt {
                        response_ref: message_ref.clone(),
                        data,
                    },
                    other => other, // defensive no-op
                }
            }
            KonfigurationEvent::AblehungErhalten { reason, .. } => match state {
                KonfigurationState::Beauftragt(data) => KonfigurationState::Abgelehnt {
                    reason: reason.clone(),
                    data,
                },
                other => other,
            },
            KonfigurationEvent::DeadlineExpired { label, .. } => {
                // Terminal states absorb late-firing deadlines without error.
                match state {
                    KonfigurationState::Bestaetigt { .. }
                    | KonfigurationState::Abgelehnt { .. } => state,
                    KonfigurationState::Beauftragt(data) => KonfigurationState::Abgelehnt {
                        data,
                        reason: format!("deadline expired: {label}"),
                    },
                    KonfigurationState::New => state, // should not happen; no-op
                }
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            KonfigurationCommand::NbSendsBeauftragung {
                orders_pid,
                msb_mp_id,
                malo,
                new_supplier,
                message_ref,
            } => {
                if !matches!(state, KonfigurationState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_PIDS.contains(&orders_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected ORDERS PID 17134 or 17135, got {orders_pid}",
                    )));
                }
                // Emit BeauftragungGesendet and enqueue the outbound ORDERS
                // message to the MSB in the same atomic write.
                let event = KonfigurationEvent::BeauftragungGesendet {
                    orders_pid,
                    msb_mp_id: msb_mp_id.clone(),
                    malo: malo.clone(),
                    new_supplier: new_supplier.clone(),
                    message_ref: message_ref.clone(),
                };
                let outbox = vec![PendingOutbox::new(
                    "ORDERS",
                    msb_mp_id.as_str(),
                    serde_json::json!({
                        "type":         "Beauftragung",
                        "pid":          orders_pid.as_u32(),
                        "malo":         malo.as_str(),
                        "new_supplier": new_supplier.as_str(),
                        "orders_ref":   message_ref.as_str(),
                    }),
                )];
                Ok(WorkflowOutput::with_outbox(vec![event], outbox))
            }

            KonfigurationCommand::ReceiveOrdrsp {
                pid,
                accepted,
                reason,
                message_ref,
            } => {
                let _data = match state {
                    KonfigurationState::Beauftragt(d) => d,
                    _ => return Err(WorkflowError::invalid_state("Beauftragt", state.label())),
                };
                if !ORDRSP_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected ORDRSP PID 19001 or 19002, got {pid}",
                    )));
                }
                let event = if accepted {
                    KonfigurationEvent::BestaetigungErhalten {
                        response_pid: pid,
                        message_ref,
                    }
                } else {
                    KonfigurationEvent::AblehungErhalten {
                        response_pid: pid,
                        reason: reason.unwrap_or_else(|| "no reason provided".to_owned()),
                        message_ref,
                    }
                };
                Ok(vec![event].into())
            }

            KonfigurationCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    KonfigurationState::Bestaetigt { .. } | KonfigurationState::Abgelehnt { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![KonfigurationEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single Konfigurationseinrichtung process stream.
#[derive(Debug)]
pub struct KonfigurationRecord {
    /// Current lifecycle status label.
    pub status: &'static str,
    /// MSB GLN once `BeauftragungGesendet` is applied.
    pub msb_mp_id: Option<MarktpartnerCode>,
    /// MaLo once `BeauftragungGesendet` is applied.
    pub malo: Option<MaLo>,
    /// Total events processed.
    pub event_count: usize,
}

impl Default for KonfigurationRecord {
    fn default() -> Self {
        Self {
            status: "New",
            msb_mp_id: None,
            malo: None,
            event_count: 0,
        }
    }
}

/// In-process read model for all GPKE Konfigurationseinrichtung streams.
#[derive(Debug, Default)]
pub struct KonfigurationProjection {
    /// All known Konfiguration process records keyed by stream ID.
    pub records: HashMap<String, KonfigurationRecord>,
    /// Sequence number of the last event applied.
    pub last_seq: u64,
}

impl Projection for KonfigurationProjection {
    fn name(&self) -> &'static str {
        "KonfigurationProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        let Ok(event) = envelope.decode::<KonfigurationEvent>() else {
            return;
        };

        match event {
            KonfigurationEvent::BeauftragungGesendet {
                msb_mp_id, malo, ..
            } => {
                record.status = "Beauftragt";
                record.msb_mp_id = Some(msb_mp_id);
                record.malo = Some(malo);
            }
            KonfigurationEvent::BestaetigungErhalten { .. } => {
                record.status = "Bestaetigt";
            }
            KonfigurationEvent::AblehungErhalten { .. } => {
                record.status = "Abgelehnt";
            }
            KonfigurationEvent::DeadlineExpired { .. } => {
                record.status = "Abgelehnt";
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
