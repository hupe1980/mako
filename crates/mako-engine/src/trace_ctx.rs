//! W3C trace-context propagation across the outbox boundary.
//!
//! End-to-end tracing in `makod` crosses an asynchronous store boundary: an
//! inbound HTTP request (AS4 push, REST ingest, ERP command) produces outbox
//! messages that a worker delivers minutes later on a different task. A span
//! cannot survive that hop — but the W3C `traceparent` string can.
//!
//! The transport layer scopes the inbound request's `traceparent` header into
//! the [`TRACEPARENT`] task-local; every [`OutboxMessage`] created inside
//! that scope — by `materialise_outbox` or `OutboxMessage::new` — captures it
//! into its persisted `trace_context` field. Delivery workers then inject it
//! into outbound HTTP requests (ERP webhook `traceparent` header and the
//! CloudEvents `traceparent` extension), closing the chain:
//!
//! ```text
//! inbound traceparent ─▶ task-local ─▶ outbox row ─▶ outbound traceparent
//! ```
//!
//! [`OutboxMessage`]: crate::outbox::OutboxMessage

tokio::task_local! {
    /// The W3C `traceparent` value of the request currently being processed.
    pub static TRACEPARENT: Option<String>;
}

/// The `traceparent` of the current task scope, if one was propagated.
///
/// Returns `None` outside a [`TRACEPARENT`] scope (workers, tests, CLI).
#[must_use]
pub fn current() -> Option<String> {
    TRACEPARENT
        .try_with(std::clone::Clone::clone)
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    /// A message created inside a TRACEPARENT scope captures it; one created
    /// outside does not.
    #[tokio::test]
    async fn outbox_message_captures_scoped_traceparent() {
        use crate::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
        use crate::outbox::OutboxMessage;

        let mk = || {
            let tenant = TenantId::from_party_id("9900000000001");
            let process = ProcessId::new();
            OutboxMessage::new(
                StreamId::for_process(tenant, &process),
                process,
                tenant,
                CorrelationId::new(),
                ConversationId::new(),
                EventId::new(),
                "APERAK",
                "9900000000002",
                serde_json::json!({}),
            )
        };

        let tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let inside = super::TRACEPARENT
            .scope(Some(tp.to_owned()), async { mk() })
            .await;
        assert_eq!(inside.trace_context.as_deref(), Some(tp));

        let outside = mk();
        assert_eq!(outside.trace_context, None);
    }
}
