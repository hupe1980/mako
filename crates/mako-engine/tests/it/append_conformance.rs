//! One conformance suite for every [`AtomicAppend`] store.
//!
//! [`AtomicAppend::append`] takes one [`AppendBatch`] — events, outbox,
//! deadlines and correlation entries — and either lands all four or none. The
//! same assertions run against the in-memory store and the SlateDB store, so a
//! store that persists only part of a batch fails here rather than in
//! production, where the missing part is a Frist that never fires or a reply
//! that finds no process.

#![cfg(all(feature = "testing", feature = "slatedb"))]

use mako_engine::{
    deadline::{Deadline, DeadlineStore},
    envelope::NewEvent,
    error::EngineError,
    event_store::{
        AppendBatch, AtomicAppend, CorrelationEntry, ExpectedVersion, InMemoryEventStore,
    },
    ids::{ConversationId, CorrelationId, ProcessId, ProcessIdentity, StreamId, TenantId},
    outbox::{OutboxMessage, OutboxStore, PendingOutbox},
    registry::ProcessRegistry,
    store_slatedb::SlateDbStore,
    version::WorkflowId,
};
use time::OffsetDateTime;

/// What the suite reads back from a store after a batch.
trait Inspect: AtomicAppend {
    async fn outbox(&self) -> Vec<OutboxMessage>;
    async fn deadlines(&self, stream_id: &StreamId) -> Vec<Deadline>;
    async fn resolves(&self, tenant: TenantId, tag: &str) -> Vec<ProcessId>;
}

impl Inspect for InMemoryEventStore {
    async fn outbox(&self) -> Vec<OutboxMessage> {
        self.outbox_messages().await
    }
    async fn deadlines(&self, stream_id: &StreamId) -> Vec<Deadline> {
        self.deadlines_for(stream_id).await
    }
    async fn resolves(&self, tenant: TenantId, tag: &str) -> Vec<ProcessId> {
        self.correlations()
            .await
            .into_iter()
            .filter(|c| c.tenant_id == tenant && c.tag == tag)
            .map(|c| c.process_id)
            .collect()
    }
}

impl Inspect for SlateDbStore {
    async fn outbox(&self) -> Vec<OutboxMessage> {
        self.pending(100, OffsetDateTime::now_utc() + time::Duration::days(1))
            .await
            .expect("pending")
    }
    async fn deadlines(&self, stream_id: &StreamId) -> Vec<Deadline> {
        self.as_deadline_store()
            .for_stream(stream_id)
            .await
            .expect("for_stream")
    }
    async fn resolves(&self, tenant: TenantId, tag: &str) -> Vec<ProcessId> {
        self.as_process_registry()
            .lookup_correlated(tenant, tag)
            .await
            .expect("lookup_correlated")
            .into_iter()
            .map(|identity| identity.process_id)
            .collect()
    }
}

fn wid() -> WorkflowId {
    WorkflowId::new("conformance", "FV2025-10-01")
}

fn event(tenant: TenantId, process: ProcessId) -> NewEvent {
    NewEvent::new(
        CorrelationId::new(),
        None,
        ConversationId::new(),
        process,
        tenant,
        wid(),
        "Spawned",
        1,
        serde_json::json!({"v": 1}),
    )
}

/// A batch carrying all four parts lands all four, linked to the event.
async fn a_full_batch_lands_whole<S: Inspect>(store: &S) {
    let tenant = TenantId::new();
    let process = ProcessId::new();
    let stream = StreamId::for_process(tenant, &process);
    let deadline = Deadline::new(
        stream.clone(),
        process,
        tenant,
        wid(),
        "antwort",
        OffsetDateTime::UNIX_EPOCH,
    );
    let correlation = CorrelationEntry {
        tenant_id: tenant,
        tag: "51238696012".into(),
        process_id: process,
        identity: ProcessIdentity::new(process, tenant, wid()),
    };
    let outbox = PendingOutbox::new("APERAK", "9900000000001", serde_json::json!({"ok": true}));

    let result = store
        .append(
            AppendBatch::new(
                &stream,
                ExpectedVersion::NoStream,
                &[event(tenant, process)],
            )
            .outbox(std::slice::from_ref(&outbox))
            .deadlines(std::slice::from_ref(&deadline))
            .correlations(std::slice::from_ref(&correlation)),
        )
        .await
        .expect("a full batch appends");

    let events = store.load(&stream).await.unwrap();
    assert_eq!(events.len(), 1, "the event is persisted");
    assert_eq!(
        result.events.iter().map(|e| e.event_id).collect::<Vec<_>>(),
        events.iter().map(|e| e.event_id).collect::<Vec<_>>(),
        "the result carries the stamped events"
    );

    let sent = store.outbox().await;
    assert_eq!(sent.len(), 1, "the outbox entry is enqueued");
    assert_eq!(sent[0].causation_event_id, events[0].event_id);
    assert_eq!(
        result.outbox.len(),
        1,
        "the result carries the enqueued entry"
    );
    assert_eq!(result.outbox[0].message_id, sent[0].message_id);

    let due = store.deadlines(&stream).await;
    assert_eq!(due.len(), 1, "the deadline is registered");
    assert_eq!(due[0].label(), "antwort");

    assert_eq!(
        store.resolves(tenant, "51238696012").await,
        vec![process],
        "the business key resolves to the process"
    );
}

/// A refused batch leaves nothing behind — not the deadline, not the key.
async fn a_refused_batch_lands_nothing<S: Inspect>(store: &S) {
    let tenant = TenantId::new();
    let process = ProcessId::new();
    let stream = StreamId::for_process(tenant, &process);
    store
        .append(AppendBatch::new(
            &stream,
            ExpectedVersion::NoStream,
            &[event(tenant, process)],
        ))
        .await
        .unwrap();

    let deadline = Deadline::new(
        stream.clone(),
        process,
        tenant,
        wid(),
        "antwort",
        OffsetDateTime::UNIX_EPOCH,
    );
    let correlation = CorrelationEntry {
        tenant_id: tenant,
        tag: "51238696012".into(),
        process_id: process,
        identity: ProcessIdentity::new(process, tenant, wid()),
    };
    let outbox = PendingOutbox::new("APERAK", "9900000000001", serde_json::json!({}));
    let err = store
        .append(
            AppendBatch::new(
                &stream,
                ExpectedVersion::NoStream,
                &[event(tenant, process)],
            )
            .outbox(std::slice::from_ref(&outbox))
            .deadlines(std::slice::from_ref(&deadline))
            .correlations(std::slice::from_ref(&correlation)),
        )
        .await
        .expect_err("a stale expected version is refused");
    assert!(
        matches!(err, EngineError::VersionConflict { .. }),
        "{err:?}"
    );

    assert_eq!(store.load(&stream).await.unwrap().len(), 1);
    assert!(store.outbox().await.is_empty(), "no outbox entry");
    assert!(store.deadlines(&stream).await.is_empty(), "no deadline");
    assert!(store.resolves(tenant, "51238696012").await.is_empty());
}

/// An outbox entry needs an event to name as its cause; a batch without one
/// is refused rather than enqueuing a message no event accounts for.
async fn an_outbox_entry_without_an_event_is_refused<S: Inspect>(store: &S) {
    let stream = StreamId::new("process/conformance/no-event");
    let outbox = PendingOutbox::new("APERAK", "9900000000001", serde_json::json!({}));
    store
        .append(
            AppendBatch::new(&stream, ExpectedVersion::NoStream, &[])
                .outbox(std::slice::from_ref(&outbox)),
        )
        .await
        .expect_err("an outbox entry with no event is refused");
    assert!(store.outbox().await.is_empty());
}

#[tokio::test]
async fn the_in_memory_store_conforms() {
    a_full_batch_lands_whole(&InMemoryEventStore::new()).await;
    a_refused_batch_lands_nothing(&InMemoryEventStore::new()).await;
    an_outbox_entry_without_an_event_is_refused(&InMemoryEventStore::new()).await;
}

async fn slatedb() -> SlateDbStore {
    SlateDbStore::open_in_memory()
        .await
        .expect("in-memory SlateDB")
}

#[tokio::test]
async fn the_slatedb_store_conforms() {
    a_full_batch_lands_whole(&slatedb().await).await;
    a_refused_batch_lands_nothing(&slatedb().await).await;
    an_outbox_entry_without_an_event_is_refused(&slatedb().await).await;
}
