//! Dead-letter sink for unroutable or unprocessable inbound messages.
//!
//! BDEW AS4 requires that every received message is either processed or
//! explicitly refused (CONTRL negative acknowledgement). Messages that
//! cannot be routed to a workflow — because the PID is unknown, the
//! conversation is not in-flight, or the format version has no adapter —
//! must not be silently dropped.
//!
//! # Design
//!
//! [`DeadLetterSink`] is a synchronous trait that receives a structured
//! [`DeadLetterReason`] for every rejected message. The synchronous contract
//! keeps dispatch-path hot code fast; implementations that need async work
//! (e.g. persisting to a durable DLQ or sending a CONTRL) can use
//! `tokio::spawn` internally.
//!
//! # Implementations
//!
//! | Type | Behaviour |
//! |------|-----------|
//! | [`LogDeadLetterSink`] | Emits structured `tracing::warn!`; suitable for all deployments |
//! | [`NoopDeadLetterSink`] | Silently discards; **only for testing** |
//!
//! # Wiring
//!
//! Pass an implementation to [`EngineBuilder::with_dead_letter_sink`].
//! The default is [`LogDeadLetterSink`] so unroutable messages are always
//! visible in the log output without any configuration.
//!
//! ```rust
//! use mako_engine::dead_letter::{DeadLetterReason, DeadLetterSink, LogDeadLetterSink};
//!
//! let sink = LogDeadLetterSink;
//! sink.reject(&DeadLetterReason::UnknownPid(99999));
//! ```
//!
//! [`EngineBuilder::with_dead_letter_sink`]: crate::builder::EngineBuilder::with_dead_letter_sink

use std::sync::Arc;

// ── DeadLetterReason ──────────────────────────────────────────────────────────

/// Structured reason why an inbound message was rejected.
///
/// The variant gives the dispatch path enough information to emit an
/// actionable CONTRL or log entry. Adding new variants is a non-breaking
/// change thanks to `#[non_exhaustive]`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DeadLetterReason {
    /// No workflow is registered for this PID in the [`PidRouter`].
    ///
    /// The PID is either from a future BDEW release not yet deployed or a
    /// malformed message. Respond with a CONTRL negative acknowledgement.
    ///
    /// [`PidRouter`]: crate::pid_router::PidRouter
    UnknownPid(u32),

    /// No in-flight process matched the inbound `conversation_id`.
    ///
    /// This typically means the process completed, was never started, or
    /// the [`ProcessRegistry`] was lost on restart (see.
    ///
    /// [`ProcessRegistry`]: crate::registry::ProcessRegistry
    UnknownConversation {
        /// The `conversation_id` from the inbound EDIFACT interchange.
        conversation_id: String,
    },

    /// The message's format version has no registered [`MessageAdapter`].
    ///
    /// Either the adapter registry is incomplete (see or the sender
    /// is using a deprecated / future format version.
    ///
    /// [`MessageAdapter`]: crate::message_adapter::MessageAdapter
    VersionMismatch {
        /// The format version string the adapter registry expected.
        expected: String,
        /// The format version string carried in the inbound message.
        received: String,
    },

    /// A message with this inbox key was already accepted (AS4 duplicate).
    ///
    /// The AS4 sender retries for up to 72 hours. The [`InboxStore`]
    /// detected the duplicate and the message must not be processed again.
    ///
    /// [`InboxStore`]: crate::inbox::InboxStore
    DuplicateMessage {
        /// The inbox deduplication key (typically the AS4 `MessageId`).
        inbox_key: String,
    },

    /// A workflow or adapter returned a processing error.
    ///
    /// The message was routed correctly but could not be processed. Use
    /// this variant when the failure is definitive (not retriable).
    ProcessingError {
        /// Short, human-readable description of the failure.
        message: String,
    },

    /// The outbox delivery worker gave up after exhausting all retry attempts.
    ///
    /// The message was re-queued `max_attempts` times and never successfully
    /// delivered to the AS4 endpoint (or ERP webhook). The message is removed
    /// from the outbox and recorded here for regulatory audit.
    OutboxExhausted {
        /// The outbox message ID of the undeliverable message.
        message_id: crate::ids::OutboxMessageId,
        /// The message type (e.g. `"APERAK"`, `"CONTRL"`).
        message_type: String,
        /// The intended recipient GLN.
        recipient: String,
        /// The last error returned by the AS4 sender.
        last_error: String,
        /// How many delivery attempts were made.
        attempts: u32,
    },
}

impl DeadLetterReason {
    /// Short label identifying the rejection category.
    ///
    /// Suitable for structured log fields and metric labels.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::UnknownPid(_) => "unknown_pid",
            Self::UnknownConversation { .. } => "unknown_conversation",
            Self::VersionMismatch { .. } => "version_mismatch",
            Self::DuplicateMessage { .. } => "duplicate_message",
            Self::ProcessingError { .. } => "processing_error",
            Self::OutboxExhausted { .. } => "outbox_exhausted",
        }
    }
}

impl std::fmt::Display for DeadLetterReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPid(pid) => write!(f, "unknown PID {pid}"),
            Self::UnknownConversation { conversation_id } => {
                write!(f, "unknown conversation {conversation_id}")
            }
            Self::VersionMismatch { expected, received } => write!(
                f,
                "version mismatch: expected {expected}, received {received}"
            ),
            Self::DuplicateMessage { inbox_key } => write!(f, "duplicate message {inbox_key}"),
            Self::ProcessingError { message } => write!(f, "processing error: {message}"),
            Self::OutboxExhausted {
                message_id,
                message_type,
                recipient,
                attempts,
                ..
            } => write!(
                f,
                "outbox exhausted after {attempts} attempts: {message_type} → {recipient} (id={message_id})"
            ),
        }
    }
}

// ── DeadLetterSink trait ──────────────────────────────────────────────────────

/// Receives messages that cannot be routed or processed.
///
/// Implement this trait to:
/// - Emit CONTRL negative acknowledgements for unroutable messages
/// - Persist rejections to a durable dead-letter queue for manual review
/// - Trigger alerts when duplicate-message counts exceed a threshold
///
/// The method is **synchronous**. Implementations that require async work
/// (network calls, database writes) must use `tokio::spawn` internally.
///
/// # Default
///
/// The default dead-letter sink is [`LogDeadLetterSink`], which emits
/// `tracing::warn!` events. Override with
/// [`EngineBuilder::with_dead_letter_sink`] to add CONTRL dispatch or
/// persistent DLQ storage.
///
/// [`EngineBuilder::with_dead_letter_sink`]: crate::builder::EngineBuilder::with_dead_letter_sink
pub trait DeadLetterSink: Send + Sync + 'static {
    /// Record a rejected message.
    ///
    /// Called by the dispatch path synchronously, before the inbound
    /// message is acknowledged at the AS4 transport layer. Must not block.
    fn reject(&self, reason: &DeadLetterReason);
}

// ── LogDeadLetterSink ─────────────────────────────────────────────────────────

/// A [`DeadLetterSink`] that emits a structured `tracing::warn!` event for
/// every rejected message.
///
/// Suitable for all deployment tiers. In production, combine with a
/// `tracing` subscriber that forwards `warn`-level events to your alert
/// pipeline (Loki, CloudWatch, etc.).
///
/// This is the **default** dead-letter sink in [`EngineBuilder`].
///
/// [`EngineBuilder`]: crate::builder::EngineBuilder
#[derive(Debug, Clone, Default)]
pub struct LogDeadLetterSink;

impl DeadLetterSink for LogDeadLetterSink {
    fn reject(&self, reason: &DeadLetterReason) {
        match reason {
            DeadLetterReason::UnknownPid(pid) => {
                tracing::warn!(
                    pid,
                    reason = reason.label(),
                    "dead letter: unknown PID — no workflow registered; \
                     send CONTRL negative acknowledgement",
                );
            }
            DeadLetterReason::UnknownConversation { conversation_id } => {
                tracing::warn!(
                    conversation_id,
                    reason = reason.label(),
                    "dead letter: unknown conversation — no in-flight process found; \
                     process may have completed or registry was lost on restart",
                );
            }
            DeadLetterReason::VersionMismatch { expected, received } => {
                tracing::warn!(
                    expected,
                    received,
                    reason = reason.label(),
                    "dead letter: format version mismatch — no adapter registered for received version",
                );
            }
            DeadLetterReason::DuplicateMessage { inbox_key } => {
                tracing::warn!(
                    inbox_key,
                    reason = reason.label(),
                    "dead letter: duplicate message — AS4 retry already processed; ignoring",
                );
            }
            DeadLetterReason::ProcessingError { message } => {
                tracing::warn!(
                    message,
                    reason = reason.label(),
                    "dead letter: processing error — message routed but could not be processed",
                );
            }
            DeadLetterReason::OutboxExhausted {
                message_id,
                message_type,
                recipient,
                last_error,
                attempts,
            } => {
                tracing::error!(
                    %message_id,
                    message_type,
                    recipient,
                    last_error,
                    attempts,
                    reason = reason.label(),
                    "dead letter: outbox exhausted — message removed after max delivery attempts; \
                     manual intervention required to deliver this message",
                );
            }
        }
    }
}

// ── NoopDeadLetterSink ────────────────────────────────────────────────────────

/// A [`DeadLetterSink`] that silently discards all rejected messages.
///
/// **Use only in unit tests** where dead-letter events are not the subject
/// under test. Using this in production means unroutable messages are lost
/// without any diagnostic output, violating BDEW AS4 requirements.
#[derive(Debug, Clone, Default)]
#[must_use = "NoopDeadLetterSink discards all rejections; use LogDeadLetterSink in production"]
pub struct NoopDeadLetterSink;

#[cfg(any(test, feature = "testing"))]
impl DeadLetterSink for NoopDeadLetterSink {
    fn reject(&self, _reason: &DeadLetterReason) {}
}

// ── ArcDeadLetterSink ─────────────────────────────────────────────────────────

/// Blanket implementation so `Arc<T>` is a `DeadLetterSink` whenever `T` is.
///
/// This allows passing `Arc<LogDeadLetterSink>` or `Arc<dyn DeadLetterSink>`
/// wherever a `DeadLetterSink` is expected without an extra wrapper.
impl<T: DeadLetterSink> DeadLetterSink for Arc<T> {
    fn reject(&self, reason: &DeadLetterReason) {
        self.as_ref().reject(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_letter_reason_labels() {
        assert_eq!(DeadLetterReason::UnknownPid(55001).label(), "unknown_pid");
        assert_eq!(
            DeadLetterReason::UnknownConversation {
                conversation_id: "abc".into()
            }
            .label(),
            "unknown_conversation"
        );
        assert_eq!(
            DeadLetterReason::VersionMismatch {
                expected: "FV2025-10-01".into(),
                received: "FV2026-10-01".into(),
            }
            .label(),
            "version_mismatch"
        );
        assert_eq!(
            DeadLetterReason::DuplicateMessage {
                inbox_key: "msg-1".into(),
            }
            .label(),
            "duplicate_message"
        );
        assert_eq!(
            DeadLetterReason::ProcessingError {
                message: "invalid state".into(),
            }
            .label(),
            "processing_error"
        );
    }

    #[test]
    fn log_sink_does_not_panic() {
        let sink = LogDeadLetterSink;
        sink.reject(&DeadLetterReason::UnknownPid(99999));
        sink.reject(&DeadLetterReason::UnknownConversation {
            conversation_id: "conv-123".into(),
        });
        sink.reject(&DeadLetterReason::VersionMismatch {
            expected: "FV2025-10-01".into(),
            received: "FV2026-10-01".into(),
        });
        sink.reject(&DeadLetterReason::DuplicateMessage {
            inbox_key: "msg-42".into(),
        });
        sink.reject(&DeadLetterReason::ProcessingError {
            message: "workflow rejected command".into(),
        });
    }

    #[test]
    fn noop_sink_does_not_panic() {
        let sink = NoopDeadLetterSink;
        sink.reject(&DeadLetterReason::UnknownPid(55001));
    }

    #[test]
    fn arc_blanket_impl_works() {
        let sink: Arc<LogDeadLetterSink> = Arc::new(LogDeadLetterSink);
        sink.reject(&DeadLetterReason::UnknownPid(1));
    }

    #[test]
    fn dead_letter_reason_display() {
        assert_eq!(
            DeadLetterReason::UnknownPid(55001).to_string(),
            "unknown PID 55001"
        );
        assert!(
            DeadLetterReason::VersionMismatch {
                expected: "FV2025-10-01".into(),
                received: "FV2026-10-01".into(),
            }
            .to_string()
            .contains("version mismatch")
        );
    }
}
