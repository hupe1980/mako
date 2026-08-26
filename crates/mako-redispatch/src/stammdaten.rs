//! Stammdatenübermittlung workflow for Redispatch 2.0.
//!
//! **Direction:** ANB → VNB → ÜNB\
//! **Document:** `redispatch_xml::Stammdaten` (Z02 reduced, Z03 enriched,
//! Z04 NB aggregate, Z14 BKV)
//!
//! # Process description
//!
//! 1. ANB sends `Stammdaten` to VNB (initial + updates on change).
//! 2. Receiver sends `AcknowledgementDocument` within **3 minutes**
//!    (UTC — see note below).
//! 3. VNB optionally forwards enriched `Stammdaten` to ÜNB. `BilAReM`
//!    Kap. 6.2.1.1 obliges the responsible Marktpartner to send a changed value
//!    „unverzüglich nach Bekanntwerden" but names no countable window, so the
//!    length is operator-configured
//!    ([`crate::fristen::`Betreiberfristen`::stammdaten_weiterleitung_werktage`]).
//!
//! # Who owns which Stammdatum
//!
//! `BilAReM` Kap. 6.2.1.1: „Für jedes ausgetauschte Stammdatum gibt es genau
//! **einen** Verantwortlichen und mindestens einen Berechtigten." The
//! Verantwortliche is the final authority on the value and must push a change
//! unverzüglich; the Berechtigte may request one (Kap. 6.2.1.4) and dispute it
//! through a Clearingprozess (Kap. 6.2.1.5). Kap. 6.1.3 fixes what a
//! Clearingprozess has to achieve: agreement **or a formally established
//! Dissens** — and „bis zu einer Änderung … durch den Verantwortlichen sind die
//! vom Verantwortlichen verteilten Informationen weiter gültig", so a disputed
//! Stammdatum keeps its current value rather than becoming unknown.
//!
//! Kap. 6.2.1.2 assigns the TR-, SR- and SG-bezogene `Stammdaten` to the **ANB**
//! and Kap. 6.2.1.7 the clusterbezogene to the **clusternder NB**.
//!
//! # `gueltig_ab` is bounded on both sides
//!
//! The `Stammdaten` AWT 1.4b constrains the effective date of a change:
//!
//! | Rule | Value | Source |
//! |---|---|---|
//! | at least this far ahead of receipt | 5 Werktage | Fußnote 27 |
//! | …or, for the `Stammdaten` marked with Fußnote 33 | 10 Werktage | Fußnote 33 |
//! | at most this far after the Erstellungszeitpunkt | 2 years | Fußnoten 31, 32 |
//!
//! See [`crate::fristen`] for the constants.
//!
//! # Clock semantics
//!
//! The acknowledgement window is wall-clock **UTC** — the XSD `UtcDateTime`
//! fields carry explicit `Z` offsets. The `gueltig_ab` Werktag rules follow
//! German local time and the GPKE Werktag definition, like GPKE/WiM.
//!
//! # Regulatory basis
//!
//! `BNetzA` **BK6-23-241** Anlage `BilAReM` Kap. 6.2.1 (Austausch von
//! `Stammdaten`); EDI@Energy *`Stammdaten`* FB/AWT 1.4b for the wire format, the
//! `gueltig_ab` Werktag rules and the two-year ceiling; *`AcknowledgementDocument`*
//! FB 1.0g for the 3-minute Frist.

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

/// Deadline label for the 3-minute `AcknowledgementDocument` window
/// ([`crate::fristen::ACK_FRIST`]).
///
/// Register immediately after [`StammdatenEvent::Received`] is applied.
pub const ACK_WINDOW_LABEL: &str = "redispatch-stammdaten-ack-window";

/// Deadline label for the VNB→ÜNB forwarding window.
///
/// The length is operator-configured: BK6-23-241 Tenorziffer 4 repealed
/// BK6-20-060, and `BilAReM` Kap. 6.2.1.1 keeps the obligation („unverzüglich
/// nach Bekanntwerden") without a countable window. See
/// [`crate::fristen::`Betreiberfristen`::stammdaten_weiterleitung_werktage`].
///
/// Register after [`StammdatenEvent::Acknowledged`] is applied, when the
/// deployment role is VNB.
pub const FORWARD_WINDOW_LABEL: &str = "redispatch-stammdaten-forward-window";

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the `Stammdaten` workflow.
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
    /// `AcknowledgementDocument` dispatched within the 3-minute window.
    Acknowledged {
        /// MRID of the outbound `AcknowledgementDocument`.
        ack_mrid: String,
    },
    /// Enriched `Stammdaten` forwarded upstream (VNB→ÜNB, role-conditional).
    Forwarded {
        /// MRID of the upstream `Stammdaten` sent to ÜNB.
        upstream_mrid: String,
    },
    /// The acknowledgement window expired without a response.
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

/// Current state of a `Stammdaten` process stream.
///
/// # Lifecycle
///
/// ```text
/// New → Received → Acknowledged → [Forwarded →] Done
///                ↘ DeadlineExpired (ACK window lapsed)
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

/// Commands for the `Stammdaten` workflow.
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
            // ACK window while Received; forwarding window (VNB → ÜNB)
            // while Acknowledged, so an acknowledged
            // Stammdaten document that is never forwarded expires visibly.
            (ACK_WINDOW_LABEL, StammdatenState::Received(_))
            | (FORWARD_WINDOW_LABEL, StammdatenState::Acknowledged { .. }) => {
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
                let is_forward_window = &*label == FORWARD_WINDOW_LABEL;
                match state {
                    // 1-Werktag forward window: an acknowledged document the
                    // VNB never forwarded to the ÜNB expires visibly.
                    StammdatenState::Acknowledged(_) if is_forward_window => {
                        Ok(vec![StammdatenEvent::DeadlineExpired { deadline_id, label }].into())
                    }
                    // Terminal / already-progressed states — no-op.
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

    #[test]
    fn unforwarded_stammdaten_expires_after_the_forward_window() {
        let data = ReceivedData {
            mrid: "sd-001".into(),
            sender: "4012345000001".into(),
            receiver: "4012345000002".into(),
            doc_type: "Z01".into(),
            anlagen_count: 1,
            received_at: "2025-10-15T09:00:00Z".into(),
        };
        let state = StammdatenState::Acknowledged(data);
        let out = StammdatenWorkflow::handle(
            &state,
            StammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: FORWARD_WINDOW_LABEL.into(),
            },
        )
        .expect("forward-window timeout handled");
        assert_eq!(out.events.len(), 1, "1-Werktag forward window must fire");
        // The ACK label stays a no-op in Acknowledged.
        let noop = StammdatenWorkflow::handle(
            &state,
            StammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ACK_WINDOW_LABEL.into(),
            },
        )
        .expect("ack timeout in acknowledged is noop");
        assert!(noop.events.is_empty());
    }
}
