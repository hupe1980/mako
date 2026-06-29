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
//! use mako_engine::dead_letter::{AuditContext, DeadLetterReason, DeadLetterSink, LogDeadLetterSink};
//!
//! let sink = LogDeadLetterSink;
//! sink.reject(&DeadLetterReason::UnknownPid { pid: 99999, context: AuditContext::now() });
//! ```
//!
//! [`EngineBuilder::with_dead_letter_sink`]: crate::builder::EngineBuilder::with_dead_letter_sink

use std::sync::Arc;

// ── AuditContext ──────────────────────────────────────────────────────────────

/// Structured audit context attached to every dead-letter event.
///
/// All fields are `Option` because they are only partially known at rejection
/// time (e.g. `pid` is not available for a parse failure before the PID is
/// decoded). Callers fill in as many fields as they have.
///
/// These fields map to §22 MessZV audit-log requirements for AS4 message
/// rejection events:
///
/// | Field | §22 MessZV requirement |
/// |---|---|
/// | `message_type` | Nachrichtentyp (UTILMD, MSCONS, APERAK, …) |
/// | `release_code` | Releasekennung (S2.1, G1.1, 2.4c, …) |
/// | `pid` | Prüfidentifikator |
/// | `sender_eic` | GLN des Absenders |
/// | `receiver_eic` | GLN des Empfängers |
/// | `message_ref` | UNH-Referenz |
/// | `process_id` | Geschäftsvorfallkennung |
/// | `tenant_id` | Mandant |
/// | `correlation_id` | AS4 `ConversationId` or similar |
/// | `timestamp` | Zeitstempel des Eingangs (German local time) |
#[derive(Debug, Clone)]
pub struct AuditContext {
    /// EDIFACT message type (e.g. `"UTILMD"`, `"MSCONS"`, `"APERAK"`).
    pub message_type: Option<String>,
    /// BDEW release code (e.g. `"S2.1"`, `"G1.1"`, `"2.4c"`).
    pub release_code: Option<String>,
    /// BDEW Prüfidentifikator numeric code.
    pub pid: Option<u32>,
    /// GLN of the AS4 sender.
    pub sender_eic: Option<String>,
    /// GLN of the AS4 receiver.
    pub receiver_eic: Option<String>,
    /// UNH message reference (interchange + message ref).
    pub message_ref: Option<String>,
    /// Internal process / workflow stream ID.
    pub process_id: Option<String>,
    /// Tenant identifier (Mandant).
    pub tenant_id: Option<String>,
    /// AS4 ConversationId or engine correlation key.
    pub correlation_id: Option<String>,
    /// Timestamp of message receipt, in German local time (CET/CEST).
    pub timestamp: time::OffsetDateTime,
}

impl AuditContext {
    /// Create an `AuditContext` with only a timestamp, all other fields `None`.
    ///
    /// Use builder-style setters to fill in known fields:
    /// ```rust
    /// use mako_engine::dead_letter::AuditContext;
    ///
    /// let ctx = AuditContext::now()
    ///     .with_message_type("UTILMD")
    ///     .with_pid(55001)
    ///     .with_sender_eic("4012345000023");
    /// ```
    #[must_use]
    pub fn now() -> Self {
        Self {
            message_type: None,
            release_code: None,
            pid: None,
            sender_eic: None,
            receiver_eic: None,
            message_ref: None,
            process_id: None,
            tenant_id: None,
            correlation_id: None,
            timestamp: time::OffsetDateTime::now_utc(),
        }
    }

    /// Set the message type.
    #[must_use]
    pub fn with_message_type(mut self, mt: impl Into<String>) -> Self {
        self.message_type = Some(mt.into());
        self
    }

    /// Set the BDEW release code.
    #[must_use]
    pub fn with_release_code(mut self, rc: impl Into<String>) -> Self {
        self.release_code = Some(rc.into());
        self
    }

    /// Set the Prüfidentifikator.
    #[must_use]
    pub fn with_pid(mut self, pid: u32) -> Self {
        self.pid = Some(pid);
        self
    }

    /// Set the sender GLN.
    #[must_use]
    pub fn with_sender_eic(mut self, eic: impl Into<String>) -> Self {
        self.sender_eic = Some(eic.into());
        self
    }

    /// Set the receiver GLN.
    #[must_use]
    pub fn with_receiver_eic(mut self, eic: impl Into<String>) -> Self {
        self.receiver_eic = Some(eic.into());
        self
    }

    /// Set the UNH message reference.
    #[must_use]
    pub fn with_message_ref(mut self, r: impl Into<String>) -> Self {
        self.message_ref = Some(r.into());
        self
    }

    /// Set the internal process / stream ID.
    #[must_use]
    pub fn with_process_id(mut self, id: impl Into<String>) -> Self {
        self.process_id = Some(id.into());
        self
    }

    /// Set the tenant identifier.
    #[must_use]
    pub fn with_tenant_id(mut self, id: impl Into<String>) -> Self {
        self.tenant_id = Some(id.into());
        self
    }

    /// Set the AS4 correlation / conversation ID.
    #[must_use]
    pub fn with_correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }
}

impl Default for AuditContext {
    fn default() -> Self {
        Self::now()
    }
}

// ── DeadLetterReason ──────────────────────────────────────────────────────────

/// Structured reason why an inbound message was rejected.
///
/// The variant gives the dispatch path enough information to emit an
/// actionable CONTRL or log entry. Each variant carries an [`AuditContext`]
/// with the §22 MessZV fields required for regulatory audit logging.
///
/// Adding new variants is a non-breaking change thanks to `#[non_exhaustive]`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DeadLetterReason {
    /// No workflow is registered for this PID in the [`PidRouter`].
    ///
    /// The PID is either from a future BDEW release not yet deployed or a
    /// malformed message. Respond with a CONTRL negative acknowledgement.
    ///
    /// [`PidRouter`]: crate::pid_router::PidRouter
    UnknownPid {
        /// The numeric Prüfidentifikator that had no registered workflow.
        pid: u32,
        /// §22 MessZV structured audit context.
        context: AuditContext,
    },

    /// No in-flight process matched the inbound `conversation_id`.
    ///
    /// This typically means the process completed, was never started, or
    /// the [`ProcessRegistry`] was lost on restart (see.
    ///
    /// [`ProcessRegistry`]: crate::registry::ProcessRegistry
    UnknownConversation {
        /// The `conversation_id` from the inbound EDIFACT interchange.
        conversation_id: String,
        /// §22 MessZV structured audit context.
        context: AuditContext,
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
        /// §22 MessZV structured audit context.
        context: AuditContext,
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
        /// §22 MessZV structured audit context.
        context: AuditContext,
    },

    /// A workflow or adapter returned a processing error.
    ///
    /// The message was routed correctly but could not be processed. Use
    /// this variant when the failure is definitive (not retriable).
    ProcessingError {
        /// Short, human-readable description of the failure.
        message: String,
        /// §22 MessZV structured audit context.
        context: AuditContext,
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
            Self::UnknownPid { .. } => "unknown_pid",
            Self::UnknownConversation { .. } => "unknown_conversation",
            Self::VersionMismatch { .. } => "version_mismatch",
            Self::DuplicateMessage { .. } => "duplicate_message",
            Self::ProcessingError { .. } => "processing_error",
            Self::OutboxExhausted { .. } => "outbox_exhausted",
        }
    }

    /// Return the [`AuditContext`] embedded in this reason, if present.
    ///
    /// `OutboxExhausted` does not carry an `AuditContext` because it refers
    /// to an outbound message (not an inbound AS4 message).
    #[must_use]
    pub fn audit_context(&self) -> Option<&AuditContext> {
        match self {
            Self::UnknownPid { context, .. }
            | Self::UnknownConversation { context, .. }
            | Self::VersionMismatch { context, .. }
            | Self::DuplicateMessage { context, .. }
            | Self::ProcessingError { context, .. } => Some(context),
            Self::OutboxExhausted { .. } => None,
        }
    }
}

impl std::fmt::Display for DeadLetterReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPid { pid, .. } => write!(f, "unknown PID {pid}"),
            Self::UnknownConversation {
                conversation_id, ..
            } => {
                write!(f, "unknown conversation {conversation_id}")
            }
            Self::VersionMismatch {
                expected, received, ..
            } => write!(
                f,
                "version mismatch: expected {expected}, received {received}"
            ),
            Self::DuplicateMessage { inbox_key, .. } => write!(f, "duplicate message {inbox_key}"),
            Self::ProcessingError { message, .. } => write!(f, "processing error: {message}"),
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
        // Emit all §22 MessZV structured audit fields when available.
        if let Some(ctx) = reason.audit_context() {
            tracing::warn!(
                reason = reason.label(),
                message_type = ctx.message_type.as_deref().unwrap_or(""),
                release_code = ctx.release_code.as_deref().unwrap_or(""),
                pid = ctx.pid.unwrap_or(0),
                sender_eic = ctx.sender_eic.as_deref().unwrap_or(""),
                receiver_eic = ctx.receiver_eic.as_deref().unwrap_or(""),
                message_ref = ctx.message_ref.as_deref().unwrap_or(""),
                process_id = ctx.process_id.as_deref().unwrap_or(""),
                tenant_id = ctx.tenant_id.as_deref().unwrap_or(""),
                correlation_id = ctx.correlation_id.as_deref().unwrap_or(""),
                %ctx.timestamp,
                "dead letter: {reason}",
            );
        } else {
            // OutboxExhausted has no inbound audit context; log its own fields.
            match reason {
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
                        "dead letter: outbox exhausted — message removed after max delivery \
                         attempts; manual intervention required to deliver this message",
                    );
                }
                _ => {
                    tracing::warn!(reason = reason.label(), "dead letter: {reason}");
                }
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
        assert_eq!(
            DeadLetterReason::UnknownPid {
                pid: 55001,
                context: AuditContext::now()
            }
            .label(),
            "unknown_pid"
        );
        assert_eq!(
            DeadLetterReason::UnknownConversation {
                conversation_id: "abc".into(),
                context: AuditContext::now(),
            }
            .label(),
            "unknown_conversation"
        );
        assert_eq!(
            DeadLetterReason::VersionMismatch {
                expected: "FV2025-10-01".into(),
                received: "FV2026-10-01".into(),
                context: AuditContext::now(),
            }
            .label(),
            "version_mismatch"
        );
        assert_eq!(
            DeadLetterReason::DuplicateMessage {
                inbox_key: "msg-1".into(),
                context: AuditContext::now(),
            }
            .label(),
            "duplicate_message"
        );
        assert_eq!(
            DeadLetterReason::ProcessingError {
                message: "invalid state".into(),
                context: AuditContext::now(),
            }
            .label(),
            "processing_error"
        );
    }

    #[test]
    fn log_sink_does_not_panic() {
        let sink = LogDeadLetterSink;
        sink.reject(&DeadLetterReason::UnknownPid {
            pid: 99999,
            context: AuditContext::now(),
        });
        sink.reject(&DeadLetterReason::UnknownConversation {
            conversation_id: "conv-123".into(),
            context: AuditContext::now(),
        });
        sink.reject(&DeadLetterReason::VersionMismatch {
            expected: "FV2025-10-01".into(),
            received: "FV2026-10-01".into(),
            context: AuditContext::now(),
        });
        sink.reject(&DeadLetterReason::DuplicateMessage {
            inbox_key: "msg-42".into(),
            context: AuditContext::now(),
        });
        sink.reject(&DeadLetterReason::ProcessingError {
            message: "workflow rejected command".into(),
            context: AuditContext::now(),
        });
    }

    #[test]
    fn noop_sink_does_not_panic() {
        let sink = NoopDeadLetterSink;
        sink.reject(&DeadLetterReason::UnknownPid {
            pid: 55001,
            context: AuditContext::now(),
        });
    }

    #[test]
    fn arc_blanket_impl_works() {
        let sink: Arc<LogDeadLetterSink> = Arc::new(LogDeadLetterSink);
        sink.reject(&DeadLetterReason::UnknownPid {
            pid: 1,
            context: AuditContext::now(),
        });
    }

    #[test]
    fn dead_letter_reason_display() {
        assert_eq!(
            DeadLetterReason::UnknownPid {
                pid: 55001,
                context: AuditContext::now()
            }
            .to_string(),
            "unknown PID 55001"
        );
        assert!(
            DeadLetterReason::VersionMismatch {
                expected: "FV2025-10-01".into(),
                received: "FV2026-10-01".into(),
                context: AuditContext::now(),
            }
            .to_string()
            .contains("version mismatch")
        );
    }

    #[test]
    fn audit_context_builder() {
        let ctx = AuditContext::now()
            .with_message_type("UTILMD")
            .with_pid(55001)
            .with_sender_eic("4012345000023")
            .with_receiver_eic("9900357000004")
            .with_message_ref("00001")
            .with_tenant_id("tenant-a")
            .with_correlation_id("conv-xyz");

        assert_eq!(ctx.message_type.as_deref(), Some("UTILMD"));
        assert_eq!(ctx.pid, Some(55001));
        assert_eq!(ctx.sender_eic.as_deref(), Some("4012345000023"));
        assert_eq!(ctx.correlation_id.as_deref(), Some("conv-xyz"));
    }

    #[test]
    fn audit_context_returned_for_inbound_reasons() {
        let r = DeadLetterReason::UnknownPid {
            pid: 99,
            context: AuditContext::now().with_pid(99),
        };
        assert!(r.audit_context().is_some());
    }
}
