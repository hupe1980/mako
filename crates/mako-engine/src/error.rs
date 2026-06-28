//! Engine-level error types.

/// Errors that can occur during engine operations (command dispatch, event
/// persistence, state reconstruction).
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The event store returned an error.
    #[error("store error: {message}")]
    Store {
        /// Human-readable description of the storage failure.
        message: String,
        /// `true` when the error is transient (retry may succeed after backoff).
        transient: bool,
    },

    /// Optimistic concurrency check failed: the stream was modified by a
    /// concurrent writer between the read and the append.
    #[error("version conflict: expected {expected}, found {actual}")]
    VersionConflict {
        /// The sequence number the caller expected the stream to be at.
        expected: u64,
        /// The actual current sequence number of the stream.
        actual: u64,
    },

    /// Could not deserialize a stored event payload into the typed event.
    ///
    /// This typically indicates a schema migration is required.
    #[error("event deserialization failed: {0}")]
    Deserialization(String),

    /// Could not serialize a domain event into the envelope payload.
    #[error("event serialization failed: {0}")]
    Serialization(String),

    /// A snapshot storage operation failed.
    #[error("snapshot store error: {message}")]
    Snapshot {
        /// Human-readable description of the storage failure.
        message: String,
        /// `true` when the error is transient (retry may succeed after backoff).
        transient: bool,
    },

    /// An outbox storage operation failed.
    #[error("outbox store error: {message}")]
    Outbox {
        /// Human-readable description of the storage failure.
        message: String,
        /// `true` when the error is transient (retry may succeed after backoff).
        transient: bool,
    },

    /// A deadline storage operation failed.
    #[error("deadline store error: {message}")]
    Deadline {
        /// Human-readable description of the storage failure.
        message: String,
        /// `true` when the error is transient (retry may succeed after backoff).
        transient: bool,
    },

    /// A process registry operation failed.
    #[error("process registry error: {message}")]
    Registry {
        /// Human-readable description of the storage failure.
        message: String,
        /// `true` when the error is transient (retry may succeed after backoff).
        transient: bool,
    },

    /// An inbox (AS4 dedup) operation failed.
    #[error("inbox store error: {message}")]
    Inbox {
        /// Human-readable description of the storage failure.
        message: String,
        /// `true` when the error is transient (retry may succeed after backoff).
        transient: bool,
    },

    /// A partner-store operation failed, or a required partner record is absent.
    #[error("partner store error: {message}")]
    Partner {
        /// Human-readable description of the storage failure.
        message: String,
        /// `true` when the error is transient (retry may succeed after backoff).
        transient: bool,
    },

    /// A dead-letter query operation failed.
    ///
    /// Covers `SlateDbDeadLetterSink::list_recent` and similar read-path
    /// operations.  Writes are fire-and-forget (logged on error) and do not
    /// produce this variant.
    #[error("dead-letter store error: {message}")]
    DeadLetter {
        /// Human-readable description of the storage failure.
        message: String,
        /// `true` when the error is transient (retry may succeed after backoff).
        transient: bool,
    },

    /// The workflow rejected the command or reached an invalid state.
    #[error("workflow error: {0}")]
    Workflow(#[from] WorkflowError),

    /// Appending the requested events would exceed the per-stream event count
    /// limit configured on the store.
    ///
    /// This is a hard safety guard against runaway streams. The caller should
    /// archive or compact the stream before retrying.
    ///
    /// The `stream_id`, `limit`, `new_events`, and `actual` fields are
    /// available for internal structured logging but are intentionally **not**
    /// included in the `Display` string returned to API callers to avoid
    /// leaking internal stream topology.
    #[error("event stream quota exceeded")]
    StreamQuotaExceeded {
        /// The stream that hit the limit (for internal logging only).
        stream_id: crate::ids::StreamId,
        /// The configured maximum number of events per stream.
        limit: u64,
        /// Number of events that would be written by this append.
        new_events: usize,
        /// Total event count after the append would complete.
        actual: u64,
    },

    /// An AS4 transport send operation failed.
    ///
    /// Distinct from [`Store`] so the outbox worker can decide retry strategy
    /// without string-matching: transport errors are potentially
    /// transient; serialization errors are permanent.
    ///
    /// [`Store`]: EngineError::Store
    #[error("AS4 transport error sending to {endpoint}: {message}")]
    Transport {
        /// The AS4 endpoint URL (or `"unknown"` when not available).
        endpoint: Box<str>,
        /// The underlying error description.
        message: String,
    },

    /// The outbound message cannot be delivered because no AS4 endpoint is
    /// registered for the recipient GLN.
    ///
    /// This is a **permanent** failure: the operator must add the missing
    /// `--as4-partner <GLN>=<URL>` entry before delivery can succeed.
    /// The outbox worker should dead-letter immediately rather than retrying.
    #[error("no AS4 endpoint configured for recipient {recipient}")]
    PartnerUnknown {
        /// The recipient GLN that has no registered endpoint.
        recipient: Box<str>,
    },

    /// The outbound message cannot be rendered to EDIFACT wire format because
    /// no renderer is implemented for its message type.
    ///
    /// This is a **permanent** failure: retrying will never succeed until a
    /// wire-format renderer is implemented for the message type. The outbox
    /// worker should dead-letter the message immediately and alert the operator.
    ///
    /// Use this instead of silently transmitting JSON blobs over AS4, which
    /// violates BDEW MaKo interoperability requirements.
    #[error(
        "no EDIFACT renderer implemented for message type '{message_type}' \
         (outbox entry {message_id}); implement a wire-format renderer before \
         enabling this workflow path in production"
    )]
    RendererNotImplemented {
        /// The EDIFACT message type string (e.g. `"MSCONS"`, `"INVOIC"`).
        message_type: Box<str>,
        /// The outbox message ID for correlation with the dead-letter store.
        message_id: Box<str>,
    },

    /// A string could not be converted into a valid [`StreamId`].
    ///
    /// Stream IDs must be non-empty and must not contain NUL bytes.
    /// This error is produced by [`StreamId::try_new`] and the
    /// [`TryFrom`] impls. Use the typed constructors
    /// ([`StreamId::for_process`], [`StreamId::for_partner`]) where possible
    /// to avoid constructing stream IDs from raw strings.
    ///
    /// [`StreamId`]: crate::ids::StreamId
    /// [`StreamId::try_new`]: crate::ids::StreamId::try_new
    /// [`StreamId::for_process`]: crate::ids::StreamId::for_process
    /// [`StreamId::for_partner`]: crate::ids::StreamId::for_partner
    #[error("invalid stream ID: {reason}")]
    InvalidStreamId {
        /// The rejected input (truncated to 200 chars for log safety).
        input: Box<str>,
        /// Human-readable explanation of why the ID was rejected.
        reason: &'static str,
    },
}

impl EngineError {
    // ── Storage error constructors ────────────────────────────────────────────

    /// Construct a **permanent** (non-retriable) event-store error.
    pub fn store(message: impl Into<String>) -> Self {
        Self::Store {
            message: message.into(),
            transient: false,
        }
    }

    /// Construct a **transient** (retriable) event-store error.
    pub fn transient_store(message: impl Into<String>) -> Self {
        Self::Store {
            message: message.into(),
            transient: true,
        }
    }

    /// Construct a **permanent** outbox-store error.
    pub fn outbox(message: impl Into<String>) -> Self {
        Self::Outbox {
            message: message.into(),
            transient: false,
        }
    }

    /// Construct a **transient** outbox-store error.
    pub fn transient_outbox(message: impl Into<String>) -> Self {
        Self::Outbox {
            message: message.into(),
            transient: true,
        }
    }

    /// Construct a **permanent** deadline-store error.
    pub fn deadline(message: impl Into<String>) -> Self {
        Self::Deadline {
            message: message.into(),
            transient: false,
        }
    }

    /// Construct a **transient** deadline-store error.
    pub fn transient_deadline(message: impl Into<String>) -> Self {
        Self::Deadline {
            message: message.into(),
            transient: true,
        }
    }

    /// Construct a **permanent** process-registry error.
    pub fn registry(message: impl Into<String>) -> Self {
        Self::Registry {
            message: message.into(),
            transient: false,
        }
    }

    /// Construct a **transient** process-registry error.
    pub fn transient_registry(message: impl Into<String>) -> Self {
        Self::Registry {
            message: message.into(),
            transient: true,
        }
    }

    /// Construct a **permanent** inbox-store error.
    pub fn inbox(message: impl Into<String>) -> Self {
        Self::Inbox {
            message: message.into(),
            transient: false,
        }
    }

    /// Construct a **transient** inbox-store error.
    pub fn transient_inbox(message: impl Into<String>) -> Self {
        Self::Inbox {
            message: message.into(),
            transient: true,
        }
    }

    /// Construct a **permanent** snapshot-store error.
    pub fn snapshot(message: impl Into<String>) -> Self {
        Self::Snapshot {
            message: message.into(),
            transient: false,
        }
    }

    /// Construct a **transient** snapshot-store error.
    pub fn transient_snapshot(message: impl Into<String>) -> Self {
        Self::Snapshot {
            message: message.into(),
            transient: true,
        }
    }

    /// Construct a **permanent** partner-store error.
    pub fn partner(message: impl Into<String>) -> Self {
        Self::Partner {
            message: message.into(),
            transient: false,
        }
    }

    /// Construct a **transient** partner-store error.
    pub fn transient_partner(message: impl Into<String>) -> Self {
        Self::Partner {
            message: message.into(),
            transient: true,
        }
    }

    /// Construct a **permanent** dead-letter-store error.
    pub fn dead_letter(message: impl Into<String>) -> Self {
        Self::DeadLetter {
            message: message.into(),
            transient: false,
        }
    }

    /// Construct a **transient** dead-letter-store error.
    pub fn transient_dead_letter(message: impl Into<String>) -> Self {
        Self::DeadLetter {
            message: message.into(),
            transient: true,
        }
    }

    // ── Predicate helpers ─────────────────────────────────────────────────────

    /// Return `true` when this is a [`EngineError::VersionConflict`].
    ///
    /// Useful for retry logic: on a version conflict the caller should reload
    /// state and re-issue the command.
    #[must_use]
    pub fn is_version_conflict(&self) -> bool {
        matches!(self, Self::VersionConflict { .. })
    }

    /// Return `true` when this is a [`EngineError::StreamQuotaExceeded`].
    #[must_use]
    pub fn is_stream_quota_exceeded(&self) -> bool {
        matches!(self, Self::StreamQuotaExceeded { .. })
    }

    /// Return `true` when this is a [`EngineError::Workflow`].
    ///
    /// Useful for distinguishing infrastructure failures (store errors) from
    /// domain rejections (the workflow refused the command).
    #[must_use]
    pub fn is_workflow_error(&self) -> bool {
        matches!(self, Self::Workflow(_))
    }

    /// Return `true` when the error is likely transient and the operation
    /// can be safely retried after a short backoff.
    ///
    /// Storage errors carry an explicit `transient` flag set at the point of
    /// construction by the storage layer, eliminating any reliance on
    /// string-matching heuristics.
    ///
    /// Transport errors (network timeouts, TLS failures) are always transient.
    /// All other errors (version conflicts, quota exceeded, workflow
    /// rejections, …) are permanent.
    ///
    /// # Usage
    ///
    /// ```rust,ignore
    /// for attempt in 0..MAX_RETRIES {
    ///     match process.execute(cmd.clone()).await {
    ///         Ok(result) => return Ok(result),
    ///         Err(e) if e.is_version_conflict() => { /* reload and retry */ }
    ///         Err(e) if e.is_transient() => {
    ///             tokio::time::sleep(backoff(attempt)).await;
    ///         }
    ///         Err(e) => return Err(e),
    ///     }
    /// }
    /// ```
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Store { transient, .. }
            | Self::Outbox { transient, .. }
            | Self::Deadline { transient, .. }
            | Self::Registry { transient, .. }
            | Self::Inbox { transient, .. }
            | Self::Snapshot { transient, .. }
            | Self::Partner { transient, .. }
            | Self::DeadLetter { transient, .. } => *transient,
            // Transport errors (network timeouts, TLS failures) are transient.
            Self::Transport { .. } => true,
            // Everything else (missing partner, version conflict, …) is permanent.
            _ => false,
        }
    }

    /// Return `true` when this is a [`EngineError::PartnerUnknown`].
    ///
    /// `PartnerUnknown` is a **permanent** failure: no retry will succeed until
    /// the operator registers the missing `--as4-partner` entry. The outbox
    /// worker should dead-letter the message immediately.
    #[must_use]
    pub fn is_partner_unknown(&self) -> bool {
        matches!(self, Self::PartnerUnknown { .. })
    }

    /// Return `true` when this is a [`EngineError::RendererNotImplemented`].
    ///
    /// `RendererNotImplemented` is a **permanent** failure: no retry will
    /// succeed until a wire-format renderer is implemented for the message type.
    /// The outbox worker should dead-letter the message immediately.
    #[must_use]
    pub fn is_renderer_not_implemented(&self) -> bool {
        matches!(self, Self::RendererNotImplemented { .. })
    }

    /// Return `true` when this is a [`EngineError::Transport`].
    #[must_use]
    pub fn is_transport_error(&self) -> bool {
        matches!(self, Self::Transport { .. })
    }

    /// Return the inner [`WorkflowError`] if this is a workflow rejection,
    /// or `None` otherwise.
    #[must_use]
    pub fn as_workflow_error(&self) -> Option<&WorkflowError> {
        if let Self::Workflow(e) = self {
            Some(e)
        } else {
            None
        }
    }

    /// Return `true` when this is a [`EngineError::Snapshot`].
    #[must_use]
    pub fn is_snapshot_error(&self) -> bool {
        matches!(self, Self::Snapshot { .. })
    }

    /// Return `true` when this is a [`EngineError::Outbox`].
    #[must_use]
    pub fn is_outbox_error(&self) -> bool {
        matches!(self, Self::Outbox { .. })
    }

    /// Return `true` when this is a [`EngineError::Deadline`].
    #[must_use]
    pub fn is_deadline_error(&self) -> bool {
        matches!(self, Self::Deadline { .. })
    }

    /// Return `true` when this is a [`EngineError::Registry`].
    #[must_use]
    pub fn is_registry_error(&self) -> bool {
        matches!(self, Self::Registry { .. })
    }

    /// Return `true` when this is a [`EngineError::Inbox`].
    #[must_use]
    pub fn is_inbox_error(&self) -> bool {
        matches!(self, Self::Inbox { .. })
    }
}

/// Reasons a workflow may refuse a command.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    /// The command is not valid for the process in its current state.
    #[error("command rejected: {reason}")]
    CommandRejected {
        /// Human-readable rejection reason.
        reason: String,
    },

    /// The command arrived when the process was in an unexpected state.
    #[error("invalid state: expected {expected}, found {actual}")]
    InvalidState {
        /// The state the workflow expected.
        expected: String,
        /// The actual state the process was in.
        actual: String,
    },

    /// Domain validation of the command payload failed.
    #[error("validation failed: {message}")]
    ValidationFailed {
        /// Description of what failed validation.
        message: String,
    },

    /// The process identified by `pid` is registered in the PID router but has
    /// no workflow implementation yet.
    ///
    /// Callers should respond to the sender with a CONTRL/APERAK rejection.
    /// This variant is **never** a transient error — it indicates missing
    /// implementation and must not be retried.
    #[error("workflow not implemented for PID {pid}")]
    NotImplemented {
        /// The Prüfidentifikator that has no implementation.
        pid: u32,
    },

    /// An unexpected error occurred inside the workflow handler.
    #[error("{message}")]
    Other {
        /// Error description.
        message: String,
    },
}

impl WorkflowError {
    /// Construct a [`WorkflowError::CommandRejected`] with a formatted reason.
    #[must_use]
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::CommandRejected {
            reason: reason.into(),
        }
    }

    /// Construct a [`WorkflowError::InvalidState`].
    #[must_use]
    pub fn invalid_state(expected: impl Into<String>, actual: impl Into<String>) -> Self {
        Self::InvalidState {
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    /// Construct a [`WorkflowError::ValidationFailed`].
    #[must_use]
    pub fn validation(message: impl Into<String>) -> Self {
        Self::ValidationFailed {
            message: message.into(),
        }
    }

    /// Construct a [`WorkflowError::NotImplemented`] for a given PID.
    ///
    /// Use this to signal that the PID is routed but has no workflow
    /// implementation. Callers must respond with a CONTRL/APERAK rejection
    /// to the sender — this variant must never be silently discarded.
    #[must_use]
    pub fn not_implemented(pid: u32) -> Self {
        Self::NotImplemented { pid }
    }

    /// Construct a [`WorkflowError::Other`].
    #[must_use]
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other {
            message: message.into(),
        }
    }

    /// Return `true` when this is a [`WorkflowError::CommandRejected`].
    #[must_use]
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::CommandRejected { .. })
    }

    /// Return `true` when this is a [`WorkflowError::InvalidState`].
    #[must_use]
    pub fn is_invalid_state(&self) -> bool {
        matches!(self, Self::InvalidState { .. })
    }

    /// Return `true` when this is a [`WorkflowError::ValidationFailed`].
    #[must_use]
    pub fn is_validation_failed(&self) -> bool {
        matches!(self, Self::ValidationFailed { .. })
    }

    /// Return `true` when this is a [`WorkflowError::NotImplemented`].
    ///
    /// This variant is never transient — callers must not retry.
    #[must_use]
    pub fn is_not_implemented(&self) -> bool {
        matches!(self, Self::NotImplemented { .. })
    }
}
