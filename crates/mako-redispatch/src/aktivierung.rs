//! Redispatch-Aktivierung workflow for Redispatch 2.0.
//!
//! **Direction:** ÜNB → VNB → ANB\
//! **Document:** `redispatch_xml::ActivationDocument`
//!   - `A96` (`Ordered`) — ACO, Anweisung (ÜNB → VNB)
//!   - `A41` (`ActivationResponse`) — ACR, Antwort (ANB/VNB → ÜNB)
//!   - `A42` (`TenderReduction`) — AAR, Teilablehnung
//!
//! # Process description
//!
//! 1. ÜNB sends `ActivationDocument` (ACO, A96) with `status = Ordered` to VNB.
//! 2. VNB cascades the ACO to the relevant ANB(s) within the remaining window.
//! 3. ANB confirms with ACR (A41) or partially rejects with AAR (A42).
//! 4. VNB aggregates responses and sends upstream ACR/AAR to ÜNB.
//!
//! # Critical timing
//!
//! The ANB must confirm or reject within **5 minutes** of receiving the ACO
//! (BK6-20-060 §6.3). This is a **hard real-time constraint** — late
//! confirmation is a regulatory failure.
//!
//! The `makod` deadline scheduler for Redispatch must poll at ≤ 30 second
//! intervals (see `docs/redis.md §9.4`).
//!
//! In addition, each party must send an `AcknowledgementDocument` (transport
//! ACK) within **6 wall-clock hours** of receiving the ACO (BK6-20-059 §4.3).
//!
//! # Clock semantics
//!
//! All Redispatch 2.0 fristen use **UTC wall-clock hours** (not German local
//! time). The XSD `UtcDateTime` fields carry explicit `Z` offsets.
//!
//! # IFTSTA integration
//!
//! Redispatch 2.0 IFTSTA PIDs (confirmed from IFTSTA AHB 2.1 and PID 4.0):
//!
//! | PID   | Perspective | Description |
//! |-------|-------------|-------------|
//! | 21037 | NB (VNB)    | Kommunikationsprozesse Redispatch — Ansicht NB |
//! | 21038 | BTR         | Kommunikationsprozesse Redispatch — Ansicht BTR |
//!
//! These PIDs are registered by [`crate::RedispatchModule`] into the `PidRouter`
//! and route to this workflow via conversation-ID lookup.
//!
//! PIDs 21035 (GPKE Rückmeldung Lieferstelle → `gpke-supplier-change`),
//! 21036 (`WiM` Strom Teil 1, unassigned) and 21040 (`AWH` Sperrprozesse Gas,
//! unassigned) are **not** Redispatch PIDs and must not be registered here.
//! See `docs/pid-reference.md` for the authoritative PID ownership table.
//!
//! # Regulatory basis
//!
//! `BNetzA` BK6-20-059 §4.3 (6h transport ACK), BK6-20-060 §6.3 (5 min activation).

use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use serde::{Deserialize, Serialize};

// ── Workflow name ─────────────────────────────────────────────────────────────

/// Stable workflow name — used in `ProcessRegistry` lookups and log output.
pub const WORKFLOW_NAME: &str = "redispatch-aktivierung";

/// Redispatch IFTSTA PIDs (IFTSTA AHB 2.1, PID 4.0):
///
/// | PID   | Perspective | Description                                      |
/// |-------|-------------|--------------------------------------------------|
/// | 21037 | NB (VNB)    | Kommunikationsprozesse Redispatch — Ansicht NB   |
/// | 21038 | BTR         | Kommunikationsprozesse Redispatch — Ansicht BTR  |
///
/// Only these two PIDs belong to Redispatch 2.0.  PIDs 21035, 21036, and
/// 21040 are NOT Redispatch PIDs — they belong to GPKE and AWH Sperrprozesse
/// respectively (see `docs/pid-reference.md`).
pub const IFTSTA_PIDS: &[u32] = &[21_037, 21_038];

/// Redispatch MSCONS PIDs — time-series data delivery for Ausfallarbeit and EEG.
///
/// These MSCONS messages carry Redispatch 2.0 time-series data and are correlated
/// with the Aktivierung process via conversation-ID (CI tag).
///
/// | PID   | Beschreibung                                       |
/// |-------|----------------------------------------------------|
/// | 13020 | Ausfallarbeitsüberführungszeitreihe                |
/// | 13021 | Redispatch meteorologische Daten                   |
/// | 13022 | Redispatch Einzelzeitreihe Ausfallarbeit           |
/// | 13023 | Redispatch Ausfallarbeitssummen                    |
/// | 13026 | EEG-Überführungszeitreihe aufgrund Ausfallarbeit   |
///
/// Source: MSCONS AHB (Redispatch 2.0 Annex), IFTSTA AHB + PID 4.0.
///
/// ## Design note — intentional duplication
///
/// These five PID numbers are also defined in `mako-edm::REDISPATCH_MSCONS_PIDS`
/// so that the `edmd` ingest filter can accept them without depending on
/// `mako-redispatch`. The two definitions must always agree; they are validated
/// together by `cargo xtask validate-pruefids`.
///
/// A cross-crate dependency between this process-engine crate and the data-tier
/// `mako-edm` crate would be the wrong direction architecturally:
///   - `mako-redispatch` = process/workflow layer (stateful, event-sourced)
///   - `mako-edm` = data/repository layer (storage types, receipts, OLAP)
///
/// Coupling the workflow layer to the data layer just for 5 constants would be
/// over-engineering. Stable regulatory constants with cross-crate `xtask`
/// validation are an acceptable and common Rust workspace pattern.
pub const MSCONS_PIDS: &[u32] = &[13_020, 13_021, 13_022, 13_023, 13_026];

/// Redispatch ORDERS PIDs — inbound requests related to Ausfallarbeit.
///
/// | PID   | Beschreibung                                                          |
/// |-------|-----------------------------------------------------------------------|
/// | 17209 | Anforderung der Ausfallarbeit durch den anfNB                         |
/// | 17210 | Anforderung Lieferantenausfallarbeitsclearingliste / Beendigung Abo   |
/// | 17211 | Reklamation von Profilen bzw. Profilscharen                           |
///
/// Source: ORDERS AHB (Redispatch 2.0), PID 4.0.
pub const ORDERS_PIDS: &[u32] = &[17_209, 17_210, 17_211];

/// Redispatch ORDRSP PIDs — responses to Redispatch subscription/aggregation requests.
///
/// | PID   | Beschreibung                                       |
/// |-------|----------------------------------------------------|
/// | 19204 | Ablehnung Ab-/Bestellung der Aggregationsebene     |
/// | 19301 | Ablehnung Abo                                      |
/// | 19302 | Bestätigung Ende Abo                               |
///
/// Source: ORDRSP AHB (Redispatch 2.0), PID 4.0.
pub const ORDRSP_PIDS: &[u32] = &[19_204, 19_301, 19_302];

// ── Deadline labels ───────────────────────────────────────────────────────────

/// **5-minute hard constraint** for ANB activation response (BK6-20-060 §6.3).
///
/// Register immediately after [`AktivierungEvent::AcoReceived`] is applied.
/// The `makod` Redispatch scheduler must poll at ≤ 30 s intervals.
pub const ACTIVATION_RESPONSE_WINDOW_LABEL: &str = "redispatch-activation-response-window";

/// 6h UTC window for `AcknowledgementDocument` (BK6-20-059 §4.3).
pub const ACK_WINDOW_LABEL: &str = "redispatch-aktivierung-ack-window";

// ── Enumerations ──────────────────────────────────────────────────────────────

/// Response type from ANB (or upstream VNB → ÜNB).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseType {
    /// Full activation confirmed (ACR, A41).
    Confirmed,
    /// Partial rejection — only a fraction of ordered MW available (AAR, A42).
    PartialRejection,
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the Aktivierung workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AktivierungEvent {
    /// `ActivationDocument` (ACO, A96 Ordered) received.
    AcoReceived {
        /// MRID (UUID) of the ACO document.
        mrid: String,
        /// Ordered MW to redispatch.
        ordered_mw: f64,
        /// Resource object identifier (NDE coding scheme, BDEW resource code).
        resource_id: String,
        /// Time period of the activation (ISO-8601 interval).
        period: String,
        /// GLN of the sender (ÜNB or VNB).
        sender: String,
        /// GLN of the receiver (VNB or ANB).
        receiver: String,
        /// UTC receipt timestamp.
        received_at: String,
    },
    /// ACO cascaded to ANB (VNB role only).
    AcoCascaded {
        /// GLN of the ANB the ACO was dispatched to.
        recipient_anb: String,
        /// MRID of the outbound ACO.
        child_mrid: String,
    },
    /// ACR (A41) sent — full activation confirmed.
    AcrSent {
        /// MRID of the outbound ACR document.
        acr_mrid: String,
        /// MW actually activated.
        activated_mw: f64,
    },
    /// AAR (A42) sent — partial rejection.
    AarSent {
        /// MRID of the outbound AAR document.
        aar_mrid: String,
        /// MW available (less than ordered).
        available_mw: f64,
        /// Reason code for the rejection.
        reason_code: String,
    },
    /// Response from ANB received (VNB role — aggregating before forwarding).
    AnbResponseReceived {
        /// GLN of the responding ANB.
        anb_id: String,
        /// Whether the ANB confirmed or partially rejected.
        response_type: ResponseType,
        /// MRID of the ANB's response document.
        response_mrid: String,
    },
    /// Aggregated response forwarded upstream to ÜNB (VNB role only).
    ResponseForwarded {
        /// MRID of the upstream ACR or AAR.
        upstream_mrid: String,
        /// Final aggregate response type.
        response_type: ResponseType,
    },
    /// IFTSTA Vollzugsmeldung received (PID 21037 or 21038).
    IftstaReceived {
        /// IFTSTA Prüfidentifikator (21037 = NB view, 21038 = BTR view).
        pid: u32,
        /// GLN of the sender.
        sender: String,
        /// GLN of the receiver.
        receiver: String,
        /// EDIFACT message reference.
        message_ref: String,
    },
    /// A registered deadline expired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for AktivierungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AcoReceived { .. } => "AktivierungAcoReceived",
            Self::AcoCascaded { .. } => "AktivierungAcoCascaded",
            Self::AcrSent { .. } => "AktivierungAcrSent",
            Self::AarSent { .. } => "AktivierungAarSent",
            Self::AnbResponseReceived { .. } => "AktivierungAnbResponseReceived",
            Self::ResponseForwarded { .. } => "AktivierungResponseForwarded",
            Self::IftstaReceived { .. } => "AktivierungIftstaReceived",
            Self::DeadlineExpired { .. } => "AktivierungDeadlineExpired",
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Core activation data set at `AcoReceived` time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcoData {
    /// MRID of the initiating ACO.
    pub mrid: String,
    /// Ordered MW.
    pub ordered_mw: f64,
    /// Resource identifier.
    pub resource_id: String,
    /// Activation time period.
    pub period: String,
    /// Sender GLN.
    pub sender: String,
    /// Receiver GLN.
    pub receiver: String,
    /// Receipt timestamp.
    pub received_at: String,
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Current state of a Redispatch activation process.
///
/// # Lifecycle (ANB role)
///
/// ```text
/// New → ActivationOrdered → Confirmed | PartialRejection
/// ```
///
/// # Lifecycle (VNB role)
///
/// ```text
/// New → ActivationOrdered → DispatchedToAnb → ResponseAggregated → Done
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum AktivierungState {
    /// No events yet.
    #[default]
    New,
    /// ACO received; response not yet sent.
    ActivationOrdered(AcoData),
    /// ACO cascaded to ANB (VNB role); awaiting response.
    DispatchedToAnb(AcoData),
    /// ANB/VNB confirmed full activation (ACR).
    Confirmed {
        /// Core activation data.
        data: AcoData,
        /// MRID of the sent ACR.
        acr_mrid: String,
    },
    /// ANB/VNB partially rejected (AAR).
    PartialRejection {
        /// Core activation data.
        data: AcoData,
        /// MRID of the sent AAR.
        aar_mrid: String,
    },
    /// VNB aggregated ANB responses and forwarded upstream.
    Done(AcoData),
    /// Activation deadline expired without a response.
    DeadlineExpired {
        /// Human-readable reason.
        reason: String,
    },
}

impl AktivierungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::ActivationOrdered(_) => "ActivationOrdered",
            Self::DispatchedToAnb(_) => "DispatchedToAnb",
            Self::Confirmed { .. } => "Confirmed",
            Self::PartialRejection { .. } => "PartialRejection",
            Self::Done(_) => "Done",
            Self::DeadlineExpired { .. } => "DeadlineExpired",
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands for the Aktivierung workflow.
///
/// All domain values must be pre-extracted by the transport layer.
/// `Workflow::handle` is pure — no I/O.
#[derive(Clone)]
pub enum AktivierungCommand {
    /// Inbound ACO (`ActivationDocument` A96 Ordered) received and parsed.
    ReceiveAco {
        /// MRID of the ACO document.
        mrid: String,
        /// Ordered MW.
        ordered_mw: f64,
        /// Resource object identifier.
        resource_id: String,
        /// Activation time period.
        period: String,
        /// Sender GLN.
        sender: String,
        /// Receiver GLN.
        receiver: String,
        /// UTC receipt timestamp.
        received_at: String,
    },
    /// Cascade ACO to ANB (VNB role only).
    CascadeToAnb {
        /// GLN of the target ANB.
        recipient_anb: String,
        /// MRID assigned to the outbound ACO.
        child_mrid: String,
    },
    /// Send ACR — full activation confirmed (ANB or VNB→ÜNB role).
    ///
    /// The caller is responsible for enqueuing the outbound XML via the outbox.
    SendAcr {
        /// MRID assigned to the outbound ACR.
        acr_mrid: String,
        /// MW actually activated.
        activated_mw: f64,
    },
    /// Send AAR — partial rejection (ANB or VNB→ÜNB role).
    ///
    /// The caller is responsible for enqueuing the outbound XML via the outbox.
    SendAar {
        /// MRID assigned to the outbound AAR.
        aar_mrid: String,
        /// MW available.
        available_mw: f64,
        /// Rejection reason code.
        reason_code: String,
    },
    /// Record ANB response received (VNB role).
    RecordAnbResponse {
        /// GLN of the responding ANB.
        anb_id: String,
        /// Confirmed or partially rejected.
        response_type: ResponseType,
        /// MRID of the ANB's ACR or AAR.
        response_mrid: String,
    },
    /// Forward aggregated response upstream to ÜNB (VNB role).
    ForwardResponse {
        /// MRID of the upstream ACR or AAR.
        upstream_mrid: String,
        /// Final aggregate response type.
        response_type: ResponseType,
    },
    /// IFTSTA Vollzugsmeldung received (PID 21037 or 21038).
    ReceiveIftsta {
        /// IFTSTA PID (21037 = NB view, 21038 = BTR view).
        pid: u32,
        /// Sender GLN.
        sender: String,
        /// Receiver GLN.
        receiver: String,
        /// EDIFACT message reference.
        message_ref: String,
    },
    /// Deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for AktivierungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// Redispatch-Aktivierung workflow for Redispatch 2.0.
///
/// Handles ACO reception, response dispatch (ACR/AAR), VNB cascading,
/// and IFTSTA Vollzugsmeldung recording.
///
/// # Deadline requirements
///
/// The `makod` daemon must configure a **separate `DeadlineScheduler`** with a
/// ≤ 30 s tick interval for Redispatch workflows to honour the 5-minute ACO
/// response window (BK6-20-060 §6.3). The standard Werktage scheduler used
/// for GPKE/WiM is not sufficient.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<AktivierungWorkflow>(
///     tenant_id,
///     WorkflowId::new(WORKFLOW_NAME, "FV2025-10-01"),
/// );
/// ```
pub struct AktivierungWorkflow;

impl Workflow for AktivierungWorkflow {
    type State = AktivierungState;
    type Event = AktivierungEvent;
    type Command = AktivierungCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                ACTIVATION_RESPONSE_WINDOW_LABEL,
                AktivierungState::ActivationOrdered(_) | AktivierungState::DispatchedToAnb(_),
            ) => Some(AktivierungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            AktivierungEvent::AcoReceived {
                mrid,
                ordered_mw,
                resource_id,
                period,
                sender,
                receiver,
                received_at,
            } => AktivierungState::ActivationOrdered(AcoData {
                mrid: mrid.clone(),
                ordered_mw: *ordered_mw,
                resource_id: resource_id.clone(),
                period: period.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                received_at: received_at.clone(),
            }),

            AktivierungEvent::AcoCascaded { .. } => match state {
                AktivierungState::ActivationOrdered(data) => {
                    AktivierungState::DispatchedToAnb(data)
                }
                other => other,
            },

            AktivierungEvent::AcrSent { acr_mrid, .. } => match state {
                AktivierungState::ActivationOrdered(data)
                | AktivierungState::DispatchedToAnb(data) => AktivierungState::Confirmed {
                    data,
                    acr_mrid: acr_mrid.clone(),
                },
                other => other,
            },

            AktivierungEvent::AarSent { aar_mrid, .. } => match state {
                AktivierungState::ActivationOrdered(data)
                | AktivierungState::DispatchedToAnb(data) => AktivierungState::PartialRejection {
                    data,
                    aar_mrid: aar_mrid.clone(),
                },
                other => other,
            },

            AktivierungEvent::AnbResponseReceived { .. } => {
                // Does not change the VNB's own state variant — tracked via projection.
                state
            }

            AktivierungEvent::ResponseForwarded { .. } => match state {
                AktivierungState::DispatchedToAnb(data) => AktivierungState::Done(data),
                other => other,
            },

            // IFTSTA is audit-only — does not drive state transitions.
            AktivierungEvent::IftstaReceived { .. } => state,

            AktivierungEvent::DeadlineExpired { label, .. } => match state {
                AktivierungState::Confirmed { .. }
                | AktivierungState::PartialRejection { .. }
                | AktivierungState::Done(_)
                | AktivierungState::DeadlineExpired { .. } => state,
                _ => AktivierungState::DeadlineExpired {
                    reason: format!("deadline expired: {label}"),
                },
            },
        }
    }

    // Workflow state machines legitimately require large match expressions.
    // Each arm is a single documented state-transition rule.
    #[allow(clippy::too_many_lines)]
    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            AktivierungCommand::ReceiveAco {
                mrid,
                ordered_mw,
                resource_id,
                period,
                sender,
                receiver,
                received_at,
            } => {
                if !matches!(state, AktivierungState::New) {
                    return Ok(vec![].into());
                }
                Ok(vec![AktivierungEvent::AcoReceived {
                    mrid,
                    ordered_mw,
                    resource_id,
                    period,
                    sender,
                    receiver,
                    received_at,
                }]
                .into())
            }

            AktivierungCommand::CascadeToAnb {
                recipient_anb,
                child_mrid,
            } => match state {
                AktivierungState::ActivationOrdered(_) => Ok(vec![AktivierungEvent::AcoCascaded {
                    recipient_anb,
                    child_mrid,
                }]
                .into()),
                AktivierungState::DispatchedToAnb(_) => Ok(vec![].into()),
                other => Err(WorkflowError::rejected(format!(
                    "CascadeToAnb not valid in state {}",
                    other.label()
                ))),
            },

            AktivierungCommand::SendAcr {
                acr_mrid,
                activated_mw,
            } => match state {
                AktivierungState::ActivationOrdered(_) | AktivierungState::DispatchedToAnb(_) => {
                    Ok(vec![AktivierungEvent::AcrSent {
                        acr_mrid,
                        activated_mw,
                    }]
                    .into())
                }
                AktivierungState::Confirmed { .. } => Ok(vec![].into()),
                other => Err(WorkflowError::rejected(format!(
                    "SendAcr not valid in state {}",
                    other.label()
                ))),
            },

            AktivierungCommand::SendAar {
                aar_mrid,
                available_mw,
                reason_code,
            } => match state {
                AktivierungState::ActivationOrdered(_) | AktivierungState::DispatchedToAnb(_) => {
                    Ok(vec![AktivierungEvent::AarSent {
                        aar_mrid,
                        available_mw,
                        reason_code,
                    }]
                    .into())
                }
                AktivierungState::PartialRejection { .. } => Ok(vec![].into()),
                other => Err(WorkflowError::rejected(format!(
                    "SendAar not valid in state {}",
                    other.label()
                ))),
            },

            AktivierungCommand::RecordAnbResponse {
                anb_id,
                response_type,
                response_mrid,
            } => match state {
                AktivierungState::DispatchedToAnb(_) => {
                    Ok(vec![AktivierungEvent::AnbResponseReceived {
                        anb_id,
                        response_type,
                        response_mrid,
                    }]
                    .into())
                }
                other => Err(WorkflowError::rejected(format!(
                    "RecordAnbResponse not valid in state {}",
                    other.label()
                ))),
            },

            AktivierungCommand::ForwardResponse {
                upstream_mrid,
                response_type,
            } => match state {
                AktivierungState::DispatchedToAnb(_) => {
                    Ok(vec![AktivierungEvent::ResponseForwarded {
                        upstream_mrid,
                        response_type,
                    }]
                    .into())
                }
                AktivierungState::Done(_) => Ok(vec![].into()),
                other => Err(WorkflowError::rejected(format!(
                    "ForwardResponse not valid in state {}",
                    other.label()
                ))),
            },

            // IFTSTA is accepted in any non-terminal state — audit record only.
            AktivierungCommand::ReceiveIftsta {
                pid,
                sender,
                receiver,
                message_ref,
            } => Ok(vec![AktivierungEvent::IftstaReceived {
                pid,
                sender,
                receiver,
                message_ref,
            }]
            .into()),

            AktivierungCommand::TimeoutExpired { deadline_id, label } => match state {
                AktivierungState::Confirmed { .. }
                | AktivierungState::PartialRejection { .. }
                | AktivierungState::Done(_)
                | AktivierungState::DeadlineExpired { .. } => Ok(vec![].into()),
                _ => Ok(vec![AktivierungEvent::DeadlineExpired { deadline_id, label }].into()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::ids::DeadlineId;

    fn aco_data() -> AcoData {
        AcoData {
            mrid: "aco-001".into(),
            ordered_mw: 50.0,
            resource_id: "4012345678901".into(),
            period: "2025-10-15T10:00:00Z/2025-10-15T11:00:00Z".into(),
            sender: "4012345000001".into(),
            receiver: "4012345000002".into(),
            received_at: "2025-10-15T09:58:00Z".into(),
        }
    }

    fn receive_aco_cmd() -> AktivierungCommand {
        AktivierungCommand::ReceiveAco {
            mrid: "aco-001".into(),
            ordered_mw: 50.0,
            resource_id: "4012345678901".into(),
            period: "2025-10-15T10:00:00Z/2025-10-15T11:00:00Z".into(),
            sender: "4012345000001".into(),
            receiver: "4012345000002".into(),
            received_at: "2025-10-15T09:58:00Z".into(),
        }
    }

    #[test]
    fn receive_aco_transitions_new_to_activation_ordered() {
        let state = AktivierungState::New;
        let output = AktivierungWorkflow::handle(&state, receive_aco_cmd()).unwrap();
        assert_eq!(output.events.len(), 1);
        let new_state = AktivierungWorkflow::apply(state, &output.events[0]);
        assert!(matches!(new_state, AktivierungState::ActivationOrdered(_)));
    }

    #[test]
    fn send_acr_from_activation_ordered_produces_confirmed() {
        let state = AktivierungState::ActivationOrdered(aco_data());
        let output = AktivierungWorkflow::handle(
            &state,
            AktivierungCommand::SendAcr {
                acr_mrid: "acr-001".into(),
                activated_mw: 50.0,
            },
        )
        .unwrap();
        let new_state = AktivierungWorkflow::apply(state, &output.events[0]);
        assert!(matches!(new_state, AktivierungState::Confirmed { .. }));
    }

    #[test]
    fn send_aar_from_activation_ordered_produces_partial_rejection() {
        let state = AktivierungState::ActivationOrdered(aco_data());
        let output = AktivierungWorkflow::handle(
            &state,
            AktivierungCommand::SendAar {
                aar_mrid: "aar-001".into(),
                available_mw: 30.0,
                reason_code: "A96".into(),
            },
        )
        .unwrap();
        let new_state = AktivierungWorkflow::apply(state, &output.events[0]);
        assert!(matches!(
            new_state,
            AktivierungState::PartialRejection { .. }
        ));
    }

    #[test]
    fn timeout_in_confirmed_state_is_noop() {
        let state = AktivierungState::Confirmed {
            data: aco_data(),
            acr_mrid: "acr-001".into(),
        };
        let output = AktivierungWorkflow::handle(
            &state,
            AktivierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ACTIVATION_RESPONSE_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(output.events.is_empty());
    }

    #[test]
    fn iftsta_accepted_in_any_state() {
        for state in [
            AktivierungState::New,
            AktivierungState::ActivationOrdered(aco_data()),
            AktivierungState::Confirmed {
                data: aco_data(),
                acr_mrid: "x".into(),
            },
        ] {
            let output = AktivierungWorkflow::handle(
                &state,
                AktivierungCommand::ReceiveIftsta {
                    pid: 21_037,
                    sender: "s".into(),
                    receiver: "r".into(),
                    message_ref: "ref-1".into(),
                },
            )
            .unwrap();
            assert!(matches!(
                output.events.as_slice(),
                [AktivierungEvent::IftstaReceived { pid: 21037, .. }]
            ));
        }
    }
}
