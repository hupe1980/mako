//! Generic acknowledge-and-forward workflow for Redispatch 2.0.
//!
//! Shared state machine used by:
//! - Verfügbarkeitsmeldung (`redispatch-verfuegbarkeit`)
//! - Netzengpassinformation (`redispatch-netzengpass`)
//! - Kaskade §13 Abs. 2 (`redispatch-kaskade`)
//! - Planungsdaten Abruffahrplan (`redispatch-planungsdaten`)
//! - Statusanfrage (`redispatch-statusanfrage`)
//! - Kostenblatt (`redispatch-kostenblatt`)
//!
//! Each of these processes follows the same pattern:
//! 1. Receive an XML document.
//! 2. Send an `AcknowledgementDocument` within **6 wall-clock hours** (UTC).
//! 3. Optionally forward to an upstream party.
//!
//! A separate workflow struct per process is defined below so that workflow
//! names, BDEW references, and deadline labels remain distinct.

use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use serde::{Deserialize, Serialize};

// ── Generic events ─────────────────────────────────────────────────────────────

/// Events shared by all acknowledge-and-forward workflows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AckForwardEvent {
    /// XML document received.
    Received {
        /// MRID (UUID) of the received document.
        mrid: String,
        /// Document type string (e.g. `"Unavailability"`, `"Kaskade"`).
        doc_type: String,
        /// GLN of the sender.
        sender: String,
        /// GLN of the receiver.
        receiver: String,
        /// UTC receipt timestamp (ISO-8601).
        received_at: String,
    },
    /// `AcknowledgementDocument` dispatched within the 6h window.
    Acknowledged {
        /// MRID of the outbound `AcknowledgementDocument`.
        ack_mrid: String,
    },
    /// Document forwarded upstream (role-conditional).
    Forwarded {
        /// MRID of the forwarded document.
        upstream_mrid: String,
    },
    /// A registered deadline expired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline.
        label: Box<str>,
    },
}

/// Commands shared by all acknowledge-and-forward workflows.
#[derive(Clone)]
pub enum AckForwardCommand {
    /// Inbound document received.
    Receive {
        /// MRID of the received document.
        mrid: String,
        /// Document type string.
        doc_type: String,
        /// Sender GLN.
        sender: String,
        /// Receiver GLN.
        receiver: String,
        /// UTC receipt timestamp.
        received_at: String,
    },
    /// `AcknowledgementDocument` dispatched.
    Acknowledge {
        /// MRID of the outbound `AcknowledgementDocument`.
        ack_mrid: String,
    },
    /// Document forwarded upstream.
    Forward {
        /// MRID of the forwarded document.
        upstream_mrid: String,
    },
    /// Deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline.
        label: Box<str>,
    },
}

impl CommandPayload for AckForwardCommand {}

/// Core data captured on receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceivedData {
    /// MRID of the received document.
    pub mrid: String,
    /// Document type string.
    pub doc_type: String,
    /// Sender GLN.
    pub sender: String,
    /// Receiver GLN.
    pub receiver: String,
    /// Receipt timestamp.
    pub received_at: String,
}

/// Generic state for acknowledge-and-forward workflows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum AckForwardState {
    /// No events yet.
    #[default]
    New,
    /// Document received; acknowledgement not yet sent.
    Received(ReceivedData),
    /// `AcknowledgementDocument` dispatched.
    Acknowledged(ReceivedData),
    /// Document forwarded upstream.
    Forwarded(ReceivedData),
    /// A registered deadline expired without acknowledgement.
    DeadlineExpired {
        /// Human-readable reason.
        reason: String,
    },
}

impl AckForwardState {
    /// Stable string label.
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

impl EventPayload for AckForwardEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Received { .. } => "AckForwardReceived",
            Self::Acknowledged { .. } => "AckForwardAcknowledged",
            Self::Forwarded { .. } => "AckForwardForwarded",
            Self::DeadlineExpired { .. } => "AckForwardDeadlineExpired",
        }
    }
}

// ── Per-workflow event newtypes ────────────────────────────────────────────────
//
// Each workflow needs distinct event_type() strings so that event logs,
// projections, and observability tools can identify events unambiguously
// across all six ack-forward process families.
//
// The macro below generates a thin newtype `FooEvent(AckForwardEvent)` for each
// workflow, with `EventPayload::event_type()` returning prefixed names such as
// `"VerfuegbarkeitReceived"`.  All apply/handle logic delegates to the shared
// `AckForwardEvent` via `From<FooEvent> for AckForwardEvent`.

macro_rules! define_workflow_event {
    ($event_type:ident, $prefix:expr) => {
        /// Workflow-specific event newtype for one of the six ack-forward process
        /// families.
        ///
        /// Wraps [`AckForwardEvent`] and returns a workflow-specific prefix from
        /// [`EventPayload::event_type`] so events from different ack-forward
        /// workflows are distinguishable in projections and the event log.
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $event_type(pub AckForwardEvent);

        impl From<AckForwardEvent> for $event_type {
            fn from(e: AckForwardEvent) -> Self {
                Self(e)
            }
        }

        impl From<$event_type> for AckForwardEvent {
            fn from(e: $event_type) -> AckForwardEvent {
                e.0
            }
        }

        impl EventPayload for $event_type {
            fn event_type(&self) -> &'static str {
                match &self.0 {
                    AckForwardEvent::Received { .. } => concat!($prefix, "Received"),
                    AckForwardEvent::Acknowledged { .. } => concat!($prefix, "Acknowledged"),
                    AckForwardEvent::Forwarded { .. } => concat!($prefix, "Forwarded"),
                    AckForwardEvent::DeadlineExpired { .. } => {
                        concat!($prefix, "DeadlineExpired")
                    }
                }
            }
        }
    };
}

define_workflow_event!(VerfuegbarkeitEvent, "Verfuegbarkeit");
define_workflow_event!(NetzengpassEvent, "Netzengpass");
define_workflow_event!(KaskadeEvent, "Kaskade");
define_workflow_event!(PlanungsdatenEvent, "Planungsdaten");
define_workflow_event!(StatusanfrageEvent, "Statusanfrage");
define_workflow_event!(KostenblattEvent, "Kostenblatt");

// ── Shared apply / handle logic ───────────────────────────────────────────────

/// Apply an `AckForwardEvent` to `AckForwardState`.
pub(crate) fn apply(state: AckForwardState, event: &AckForwardEvent) -> AckForwardState {
    match event {
        AckForwardEvent::Received {
            mrid,
            doc_type,
            sender,
            receiver,
            received_at,
        } => AckForwardState::Received(ReceivedData {
            mrid: mrid.clone(),
            doc_type: doc_type.clone(),
            sender: sender.clone(),
            receiver: receiver.clone(),
            received_at: received_at.clone(),
        }),

        AckForwardEvent::Acknowledged { .. } => match state {
            AckForwardState::Received(data) => AckForwardState::Acknowledged(data),
            other => other,
        },

        AckForwardEvent::Forwarded { .. } => match state {
            AckForwardState::Acknowledged(data) => AckForwardState::Forwarded(data),
            other => other,
        },

        AckForwardEvent::DeadlineExpired { label, .. } => AckForwardState::DeadlineExpired {
            reason: format!("deadline expired: {label}"),
        },
    }
}

/// Handle an `AckForwardCommand` against `AckForwardState`.
pub(crate) fn handle(
    state: &AckForwardState,
    command: AckForwardCommand,
    ack_window_label: &str,
) -> Result<WorkflowOutput<AckForwardEvent>, WorkflowError> {
    match command {
        AckForwardCommand::Receive {
            mrid,
            doc_type,
            sender,
            receiver,
            received_at,
        } => {
            if !matches!(state, AckForwardState::New) {
                return Ok(vec![].into());
            }
            Ok(vec![AckForwardEvent::Received {
                mrid,
                doc_type,
                sender,
                receiver,
                received_at,
            }]
            .into())
        }

        AckForwardCommand::Acknowledge { ack_mrid } => match state {
            AckForwardState::Received(_) => {
                Ok(vec![AckForwardEvent::Acknowledged { ack_mrid }].into())
            }
            AckForwardState::Acknowledged(_) | AckForwardState::Forwarded(_) => Ok(vec![].into()),
            other => Err(WorkflowError::rejected(format!(
                "Acknowledge not valid in state {}",
                other.label()
            ))),
        },

        AckForwardCommand::Forward { upstream_mrid } => match state {
            AckForwardState::Acknowledged(_) => {
                Ok(vec![AckForwardEvent::Forwarded { upstream_mrid }].into())
            }
            AckForwardState::Forwarded(_) => Ok(vec![].into()),
            other => Err(WorkflowError::rejected(format!(
                "Forward not valid in state {}",
                other.label()
            ))),
        },

        AckForwardCommand::TimeoutExpired { deadline_id, label } => match state {
            AckForwardState::Acknowledged(_)
            | AckForwardState::Forwarded(_)
            | AckForwardState::DeadlineExpired { .. } => Ok(vec![].into()),
            _ => {
                let _ = ack_window_label; // used by caller for label registration
                Ok(vec![AckForwardEvent::DeadlineExpired { deadline_id, label }].into())
            }
        },
    }
}

// ── Per-process workflow structs ───────────────────────────────────────────────

macro_rules! ack_forward_workflow {
    (
        $(#[$meta:meta])*
        $name:ident,
        $event_newtype:ident,
        $workflow_name:expr,
        $ack_label:expr,
        $event_prefix:expr $(,)?
    ) => {
        $(#[$meta])*
        pub struct $name;

        impl Workflow for $name {
            type State   = AckForwardState;
            type Event   = $event_newtype;
            type Command = AckForwardCommand;

            fn on_deadline(
                deadline: &Deadline,
                state: &Self::State,
            ) -> Option<Self::Command> {
                if deadline.label() == $ack_label {
                    if matches!(state, AckForwardState::Received(_)) {
                        return Some(AckForwardCommand::TimeoutExpired {
                            deadline_id: deadline.deadline_id(),
                            label: deadline.label().into(),
                        });
                    }
                }
                None
            }

            fn apply(state: Self::State, event: &Self::Event) -> Self::State {
                crate::ack_forward::apply(state, &event.0)
            }

            fn handle(
                state: &Self::State,
                command: Self::Command,
            ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
                let output = crate::ack_forward::handle(state, command, $ack_label)?;
                Ok(WorkflowOutput::with_outbox(
                    output.events.into_iter().map($event_newtype).collect(),
                    output.outbox,
                ))
            }
        }

        impl $name {
            /// Return the event-type prefix for this workflow's events.
            #[must_use]
            pub fn event_prefix() -> &'static str {
                $event_prefix
            }
        }
    };
}

ack_forward_workflow!(
    /// Verfügbarkeitsmeldung workflow — `UnavailabilityMarketDocument`.
    ///
    /// ANB → VNB. Receiver acknowledges within 6h (BK6-20-059 §4.3).
    VerfuegbarkeitWorkflow,
    VerfuegbarkeitEvent,
    "redispatch-verfuegbarkeit",
    "redispatch-verfuegbarkeit-ack-window",
    "Verfuegbarkeit",
);

ack_forward_workflow!(
    /// Netzengpassinformation workflow — `NetworkConstraintDocument`.
    ///
    /// ÜNB ↔ VNB. Receiver acknowledges within 6h (BK6-20-059 §4.3).
    NetzengpassWorkflow,
    NetzengpassEvent,
    "redispatch-netzengpass",
    "redispatch-netzengpass-ack-window",
    "Netzengpass",
);

ack_forward_workflow!(
    /// Kaskade workflow — emergency measures per § 13 Abs. 2 EnWG.
    ///
    /// ÜNB → VNB → ANB. Receiver acknowledges within 6h (BK6-20-059 §4.3).
    /// Only active for `Marktrolle::Nb` and `Marktrolle::Unb` deployments.
    KaskadeWorkflow,
    KaskadeEvent,
    "redispatch-kaskade",
    "redispatch-kaskade-ack-window",
    "Kaskade",
);

ack_forward_workflow!(
    /// Planungsdaten (Abruffahrplan) workflow — `PlannedResourceScheduleDocument`.
    ///
    /// ÜNB → VNB → ANB. Receiver acknowledges within 6h (BK6-20-059 §4.3).
    PlanungsdatenWorkflow,
    PlanungsdatenEvent,
    "redispatch-planungsdaten",
    "redispatch-planungsdaten-ack-window",
    "Planungsdaten",
);

ack_forward_workflow!(
    /// Statusanfrage workflow — `StatusRequest_MarketDocument`.
    ///
    /// Addressed party responds within 24h (BK6-20-059 §4.4).
    StatusanfrageWorkflow,
    StatusanfrageEvent,
    "redispatch-statusanfrage",
    "redispatch-statusanfrage-response-window",
    "Statusanfrage",
);

ack_forward_workflow!(
    /// Kostenblatt workflow — monthly cost reconciliation.
    ///
    /// VNB → ÜNB. Receiver acknowledges within 6h (BK6-20-059 §4.3).
    /// VNB submits by the 15th of the following month (BK6-20-061 §7).
    KostenblattWorkflow,
    KostenblattEvent,
    "redispatch-kostenblatt",
    "redispatch-kostenblatt-ack-window",
    "Kostenblatt",
);

/// Workflow name constants for each process.
/// Workflow name constants for each process in the acknowledge-and-forward family.
pub mod names {
    /// Workflow name for `VerfuegbarkeitWorkflow`.
    pub const VERFUEGBARKEIT: &str = "redispatch-verfuegbarkeit";
    /// Workflow name for `NetzengpassWorkflow`.
    pub const NETZENGPASS: &str = "redispatch-netzengpass";
    /// Workflow name for `KaskadeWorkflow`.
    pub const KASKADE: &str = "redispatch-kaskade";
    /// Workflow name for `PlanungsdatenWorkflow`.
    pub const PLANUNGSDATEN: &str = "redispatch-planungsdaten";
    /// Workflow name for `StatusanfrageWorkflow`.
    pub const STATUSANFRAGE: &str = "redispatch-statusanfrage";
    /// Workflow name for `KostenblattWorkflow`.
    pub const KOSTENBLATT: &str = "redispatch-kostenblatt";
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::workflow::EventPayload;

    #[test]
    fn verfuegbarkeit_receive_to_acknowledged() {
        let state = AckForwardState::New;
        let output = VerfuegbarkeitWorkflow::handle(
            &state,
            AckForwardCommand::Receive {
                mrid: "m1".into(),
                doc_type: "Unavailability".into(),
                sender: "s".into(),
                receiver: "r".into(),
                received_at: "2025-10-15T10:00:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(output.events.len(), 1);

        let state2 = VerfuegbarkeitWorkflow::apply(state, &output.events[0]);
        assert!(matches!(state2, AckForwardState::Received(_)));

        let output2 = VerfuegbarkeitWorkflow::handle(
            &state2,
            AckForwardCommand::Acknowledge {
                ack_mrid: "ack-1".into(),
            },
        )
        .unwrap();
        let state3 = VerfuegbarkeitWorkflow::apply(state2, &output2.events[0]);
        assert!(matches!(state3, AckForwardState::Acknowledged(_)));
    }

    #[test]
    fn kaskade_forward_requires_acknowledged_state() {
        let state = AckForwardState::Received(ReceivedData {
            mrid: "m".into(),
            doc_type: "Kaskade".into(),
            sender: "s".into(),
            receiver: "r".into(),
            received_at: "2025-10-15T10:00:00Z".into(),
        });
        let result = KaskadeWorkflow::handle(
            &state,
            AckForwardCommand::Forward {
                upstream_mrid: "u".into(),
            },
        );
        assert!(result.is_err());
    }

    /// Verify that each workflow's event types are unique and correctly prefixed.
    #[test]
    fn event_types_are_unique_per_workflow() {
        let inner = AckForwardEvent::Received {
            mrid: "m".into(),
            doc_type: "X".into(),
            sender: "s".into(),
            receiver: "r".into(),
            received_at: "t".into(),
        };

        let types: Vec<&'static str> = vec![
            VerfuegbarkeitEvent(inner.clone()).event_type(),
            NetzengpassEvent(inner.clone()).event_type(),
            KaskadeEvent(inner.clone()).event_type(),
            PlanungsdatenEvent(inner.clone()).event_type(),
            StatusanfrageEvent(inner.clone()).event_type(),
            KostenblattEvent(inner.clone()).event_type(),
        ];

        // All event types must be distinct.
        let unique: std::collections::HashSet<_> = types.iter().collect();
        assert_eq!(
            unique.len(),
            types.len(),
            "event_type() strings must be unique across all ack-forward workflows: {types:?}"
        );

        // All event types must be prefixed (not generic "AckForward…" names).
        for t in &types {
            assert!(
                !t.starts_with("AckForward"),
                "event_type '{t}' must not use the generic AckForward prefix"
            );
        }
    }
}
