//! Stammdatenübermittlung workflow for Redispatch 2.0.
//!
//! **Direction:** ANB → VNB → ÜNB\
//! **Document:** `redispatch_xml::Stammdaten` (Z02 reduced, Z03 enriched,
//! Z04 NB aggregate, Z14 BKV)
//!
//! # Process description
//!
//! 1. ANB sends `Stammdaten` to VNB (initial + updates on change).
//! 2. Receiver sends `AcknowledgementDocument` within **6 wall-clock hours**
//!    (UTC — see note below).
//! 3. VNB optionally forwards enriched `Stammdaten` to ÜNB within **1 Werktag**
//!    of the master-data change (BK6-20-060 §3.2).
//!
//! # Clock semantics
//!
//! All Redispatch 2.0 fristen use **UTC wall-clock hours**, not German local
//! time (CET/CEST). The `UtcDateTime` fields in XSD carry explicit `Z` offsets.
//! This differs from GPKE/WiM deadlines, which use German local time.
//!
//! # Regulatory basis
//!
//! `BNetzA` BK6-20-059 §4.3 (6h ACK), BK6-20-060 §3.2 (Stammdaten update).

use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use serde::{Deserialize, Serialize};

// ── Workflow name ─────────────────────────────────────────────────────────────

/// Stable workflow name — used in `ProcessRegistry` lookups and log output.
pub const WORKFLOW_NAME: &str = "redispatch-stammdaten";

// ── Deadline labels ───────────────────────────────────────────────────────────

/// 6h UTC window for dispatching `AcknowledgementDocument` (BK6-20-059 §4.3).
///
/// Register immediately after [`StammdatenEvent::Received`] is applied.
pub const ACK_WINDOW_LABEL: &str = "redispatch-stammdaten-ack-window";

/// 1 Werktag forwarding window for VNB→ÜNB enrichment (BK6-20-060 §3.2).
///
/// Register after [`StammdatenEvent::Acknowledged`] is applied, when the
/// deployment role is VNB.
pub const FORWARD_WINDOW_LABEL: &str = "redispatch-stammdaten-forward-window";

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the Stammdaten workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StammdatenEvent {
    /// `Stammdaten` document received from ANB or VNB.
    Received {
        /// MRID (UUID) of the received `Stammdaten` document.
        mrid: String,
        /// GLN of the sender (ANB or VNB).
        sender: String,
        /// GLN of the receiver (VNB or ÜNB).
        receiver: String,
        /// Document type code (Z02/Z03/Z04/Z14).
        doc_type: String,
        /// Number of resource objects (`Anlagen`) included.
        anlagen_count: u32,
        /// UTC receipt timestamp in ISO-8601 format.
        received_at: String,
    },
    /// `AcknowledgementDocument` dispatched within the 6h window.
    Acknowledged {
        /// MRID of the outbound `AcknowledgementDocument`.
        ack_mrid: String,
    },
    /// Enriched `Stammdaten` forwarded upstream (VNB→ÜNB, role-conditional).
    Forwarded {
        /// MRID of the upstream `Stammdaten` sent to ÜNB.
        upstream_mrid: String,
    },
    /// The 6h acknowledgement window expired without a response.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for StammdatenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Received { .. } => "StammdatenReceived",
            Self::Acknowledged { .. } => "StammdatenAcknowledged",
            Self::Forwarded { .. } => "StammdatenForwarded",
            Self::DeadlineExpired { .. } => "StammdatenDeadlineExpired",
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Business data captured when the `Stammdaten` document is first received.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceivedData {
    /// MRID (UUID) of the received `Stammdaten` document.
    pub mrid: String,
    /// GLN of the sender.
    pub sender: String,
    /// GLN of the receiver.
    pub receiver: String,
    /// Document type code.
    pub doc_type: String,
    /// Number of resource objects.
    pub anlagen_count: u32,
    /// UTC receipt timestamp.
    pub received_at: String,
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Current state of a Stammdaten process stream.
///
/// # Lifecycle
///
/// ```text
/// New → Received → Acknowledged → [Forwarded →] Done
///                ↘ DeadlineExpired (6h window lapsed)
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum StammdatenState {
    /// No events yet.
    #[default]
    New,
    /// Document received; `AcknowledgementDocument` not yet sent.
    Received(ReceivedData),
    /// `AcknowledgementDocument` sent; forwarding to ÜNB not yet done.
    Acknowledged(ReceivedData),
    /// Enriched document forwarded to ÜNB (VNB role only).
    Forwarded(ReceivedData),
    /// Process terminated due to a missed deadline.
    DeadlineExpired {
        /// Human-readable description of the expired deadline.
        reason: String,
    },
}

impl StammdatenState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Received(_) => "Received",
            Self::Acknowledged(_) => "Acknowledged",
            Self::Forwarded(_) => "Forwarded",
            Self::DeadlineExpired { .. } => "DeadlineExpired",
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands for the Stammdaten workflow.
///
/// All domain values are pre-extracted by the transport layer before
/// construction. `Workflow::handle` is pure — no I/O.
#[derive(Clone)]
pub enum StammdatenCommand {
    /// Inbound `Stammdaten` document received and parsed by the transport layer.
    Receive {
        /// MRID (UUID) of the received document.
        mrid: String,
        /// GLN of the sender.
        sender: String,
        /// GLN of the receiver.
        receiver: String,
        /// Document type code (Z02/Z03/Z04/Z14).
        doc_type: String,
        /// Number of resource objects in the document.
        anlagen_count: u32,
        /// UTC receipt timestamp (ISO-8601 string).
        received_at: String,
    },
    /// `AcknowledgementDocument` dispatched to the sender.
    ///
    /// The caller is responsible for building and enqueuing the outbound XML
    /// via the outbox before issuing this command.
    SendAcknowledgement {
        /// MRID assigned to the outbound `AcknowledgementDocument`.
        ack_mrid: String,
    },
    /// Enriched `Stammdaten` forwarded to ÜNB (VNB role only).
    ///
    /// The caller is responsible for building and enqueuing the upstream XML.
    Forward {
        /// MRID assigned to the upstream `Stammdaten` document.
        upstream_mrid: String,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for StammdatenCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// Stammdatenübermittlung workflow for Redispatch 2.0.
///
/// Handles the reception, acknowledgement, and optional forwarding of
/// `Stammdaten` documents exchanged between ANB, VNB, and ÜNB.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<StammdatenWorkflow>(
///     tenant_id,
///     WorkflowId::new(WORKFLOW_NAME, "FV2025-10-01"),
/// );
/// ```
pub struct StammdatenWorkflow;

impl Workflow for StammdatenWorkflow {
    type State = StammdatenState;
    type Event = StammdatenEvent;
    type Command = StammdatenCommand;

    /// Fire deadline commands when the ACK or forward windows expire.
    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (ACK_WINDOW_LABEL, StammdatenState::Received(_)) => {
                Some(StammdatenCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            StammdatenEvent::Received {
                mrid,
                sender,
                receiver,
                doc_type,
                anlagen_count,
                received_at,
            } => StammdatenState::Received(ReceivedData {
                mrid: mrid.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                doc_type: doc_type.clone(),
                anlagen_count: *anlagen_count,
                received_at: received_at.clone(),
            }),

            StammdatenEvent::Acknowledged { .. } => match state {
                StammdatenState::Received(data) => StammdatenState::Acknowledged(data),
                other => other,
            },

            StammdatenEvent::Forwarded { .. } => match state {
                StammdatenState::Acknowledged(data) => StammdatenState::Forwarded(data),
                other => other,
            },

            StammdatenEvent::DeadlineExpired { label, .. } => StammdatenState::DeadlineExpired {
                reason: format!("deadline expired: {label}"),
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            StammdatenCommand::Receive {
                mrid,
                sender,
                receiver,
                doc_type,
                anlagen_count,
                received_at,
            } => {
                if !matches!(state, StammdatenState::New) {
                    // Idempotent: document already received — this is a retry.
                    return Ok(vec![].into());
                }
                Ok(vec![StammdatenEvent::Received {
                    mrid,
                    sender,
                    receiver,
                    doc_type,
                    anlagen_count,
                    received_at,
                }]
                .into())
            }

            StammdatenCommand::SendAcknowledgement { ack_mrid } => match state {
                StammdatenState::Received(_) => {
                    Ok(vec![StammdatenEvent::Acknowledged { ack_mrid }].into())
                }
                StammdatenState::Acknowledged(_) | StammdatenState::Forwarded(_) => {
                    // Idempotent — acknowledgement already sent.
                    Ok(vec![].into())
                }
                other => Err(WorkflowError::rejected(format!(
                    "SendAcknowledgement not valid in state {}",
                    other.label()
                ))),
            },

            StammdatenCommand::Forward { upstream_mrid } => match state {
                StammdatenState::Acknowledged(_) => {
                    Ok(vec![StammdatenEvent::Forwarded { upstream_mrid }].into())
                }
                StammdatenState::Forwarded(_) => {
                    // Idempotent.
                    Ok(vec![].into())
                }
                other => Err(WorkflowError::rejected(format!(
                    "Forward not valid in state {}",
                    other.label()
                ))),
            },

            StammdatenCommand::TimeoutExpired { deadline_id, label } => {
                match state {
                    // Terminal states — deadline is a no-op.
                    StammdatenState::Acknowledged(_)
                    | StammdatenState::Forwarded(_)
                    | StammdatenState::DeadlineExpired { .. } => Ok(vec![].into()),
                    _ => Ok(vec![StammdatenEvent::DeadlineExpired { deadline_id, label }].into()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::ids::DeadlineId;

    fn received_cmd() -> StammdatenCommand {
        StammdatenCommand::Receive {
            mrid: "mrid-001".into(),
            sender: "4012345000001".into(),
            receiver: "4012345000002".into(),
            doc_type: "Z02".into(),
            anlagen_count: 3,
            received_at: "2025-10-15T10:00:00Z".into(),
        }
    }

    #[test]
    fn receive_transitions_new_to_received() {
        let state = StammdatenState::New;
        let output = StammdatenWorkflow::handle(&state, received_cmd()).unwrap();
        assert_eq!(output.events.len(), 1);
        let new_state = StammdatenWorkflow::apply(state, &output.events[0]);
        assert!(matches!(new_state, StammdatenState::Received(_)));
    }

    #[test]
    fn acknowledge_transitions_received_to_acknowledged() {
        let state = StammdatenState::Received(ReceivedData {
            mrid: "m".into(),
            sender: "s".into(),
            receiver: "r".into(),
            doc_type: "Z02".into(),
            anlagen_count: 1,
            received_at: "2025-10-15T10:00:00Z".into(),
        });
        let output = StammdatenWorkflow::handle(
            &state,
            StammdatenCommand::SendAcknowledgement {
                ack_mrid: "ack-001".into(),
            },
        )
        .unwrap();
        assert_eq!(output.events.len(), 1);
        let new_state = StammdatenWorkflow::apply(state, &output.events[0]);
        assert!(matches!(new_state, StammdatenState::Acknowledged(_)));
    }

    #[test]
    fn forward_requires_acknowledged_state() {
        let state = StammdatenState::Received(ReceivedData {
            mrid: "m".into(),
            sender: "s".into(),
            receiver: "r".into(),
            doc_type: "Z03".into(),
            anlagen_count: 1,
            received_at: "2025-10-15T10:00:00Z".into(),
        });
        let result = StammdatenWorkflow::handle(
            &state,
            StammdatenCommand::Forward {
                upstream_mrid: "u".into(),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn timeout_in_received_state_emits_deadline_expired() {
        let state = StammdatenState::Received(ReceivedData {
            mrid: "m".into(),
            sender: "s".into(),
            receiver: "r".into(),
            doc_type: "Z02".into(),
            anlagen_count: 1,
            received_at: "2025-10-15T10:00:00Z".into(),
        });
        let output = StammdatenWorkflow::handle(
            &state,
            StammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ACK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(matches!(
            output.events.as_slice(),
            [StammdatenEvent::DeadlineExpired { .. }]
        ));
    }

    #[test]
    fn timeout_in_acknowledged_state_is_noop() {
        let state = StammdatenState::Acknowledged(ReceivedData {
            mrid: "m".into(),
            sender: "s".into(),
            receiver: "r".into(),
            doc_type: "Z02".into(),
            anlagen_count: 1,
            received_at: "2025-10-15T10:00:00Z".into(),
        });
        let output = StammdatenWorkflow::handle(
            &state,
            StammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ACK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(output.events.is_empty());
    }
}
