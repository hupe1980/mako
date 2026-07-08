//! Benchmarks for outbox polling, deadline polling, and projection catch-up.
//!
//! These measure the core hot-paths that are directly on the regulatory
//! critical path:
//!
//! - **`bench_outbox_pending_now`** — `OutboxStore::pending_now()` at varying
//!   queue depths.  This is the innermost loop of the outbox delivery worker;
//!   it must stay well under the GPKE 24-hour and APERAK 45-minute deadlines.
//!
//! - **`bench_deadline_due_now`** — `DeadlineStore::due_now()` at varying
//!   deadline counts.  Called on every poll cycle of the deadline scheduler.
//!
//! - **`bench_projection_catchup`** — `ProjectionRunner::catch_up_persistent`
//!   replaying a growing stream.  Validates that projection checkpointing keeps
//!   the cold-start replay time bounded.
//!
//! # Running
//!
//! ```bash
//! cargo bench -p mako-engine --bench workers --features testing
//! ```
//!
//! Results are written to `target/criterion/workers/`.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use mako_engine::{
    deadline::{Deadline, DeadlineStore as _, InMemoryDeadlineStore},
    envelope::{EventEnvelope, NewEvent},
    event_store::{EventStore, ExpectedVersion, InMemoryEventStore},
    ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId},
    outbox::{InMemoryOutboxStore, OutboxMessage, OutboxStore as _},
    projection::{GlobalProjectionCheckpoint, Projection, ProjectionRunner},
    version::WorkflowId,
};
use tokio::runtime::Runtime;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_rt() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn wid() -> WorkflowId {
    WorkflowId::new("gpke-lf-anmeldung", "FV2025-10-01")
}

fn make_outbox_message() -> OutboxMessage {
    OutboxMessage::new(
        StreamId::new("bench/outbox-stream"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        "UTILMD",
        "9900357000004",
        serde_json::json!({
            "message_type": "UTILMD",
            "pid": 55001,
            "sender": "4012345000023",
            "receiver": "9900357000004"
        }),
    )
}

fn make_deadline(past: time::OffsetDateTime) -> Deadline {
    Deadline::new(
        StreamId::new("bench/dl-stream"),
        ProcessId::new(),
        TenantId::new(),
        wid(),
        "gpke-response-window",
        past,
    )
}

fn make_event() -> NewEvent {
    NewEvent::new(
        CorrelationId::new(),
        None,
        ConversationId::new(),
        ProcessId::new(),
        TenantId::new(),
        wid(),
        "SupplierChangeInitiated",
        1,
        serde_json::json!({
            "pruefidentifikator": 55001,
            "sender": "4012345000023",
            "receiver": "9900357000004"
        }),
    )
}

/// Seed an outbox store with `depth` pending messages; return the store.
fn seeded_outbox(rt: &Runtime, depth: usize) -> InMemoryOutboxStore {
    let store = InMemoryOutboxStore::new();
    let msgs: Vec<OutboxMessage> = (0..depth).map(|_| make_outbox_message()).collect();
    rt.block_on(async {
        store.enqueue(&msgs).await.expect("seeded enqueue");
    });
    store
}

/// Seed a deadline store with `count` deadlines that are all overdue.
fn seeded_deadlines_overdue(rt: &Runtime, count: usize) -> InMemoryDeadlineStore {
    let store = InMemoryDeadlineStore::new();
    let past = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    rt.block_on(async {
        for _ in 0..count {
            store
                .register(&make_deadline(past))
                .await
                .expect("seeded register");
        }
    });
    store
}

/// Seed an event store with `n` events on a single stream; return the pair.
fn seeded_events(rt: &Runtime, n: usize) -> (InMemoryEventStore, StreamId) {
    let store = InMemoryEventStore::new();
    let stream = StreamId::new("bench/projection-stream");
    let events: Vec<NewEvent> = (0..n).map(|_| make_event()).collect();
    rt.block_on(async {
        store
            .append(&stream, ExpectedVersion::NoStream, &events)
            .await
            .expect("seeded append");
    });
    (store, stream)
}

// ── NoopProjection ────────────────────────────────────────────────────────────

/// Minimal no-op projection for benchmarking catch-up overhead in isolation.
#[derive(Default)]
struct NopProjection {
    event_count: u64,
    last_seq: Option<u64>,
}

impl Projection for NopProjection {
    fn name(&self) -> &'static str {
        "bench-nop"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.event_count += 1;
        self.last_seq = Some(envelope.sequence_number);
    }

    fn last_sequence(&self) -> Option<u64> {
        self.last_seq
    }
}

// ── Benchmarks ────────────────────────────────────────────────────────────────

/// Benchmark `OutboxStore::pending_now(batch_size)` at varying queue depths.
///
/// Simulates the hot path of the outbox delivery worker: pick up a batch of
/// pending messages from the store.
fn bench_outbox_pending_now(c: &mut Criterion) {
    let rt = make_rt();
    let batch_size = 50usize;
    let mut group = c.benchmark_group("outbox/pending_now");

    for depth in [10, 100, 500, 1_000] {
        let store = seeded_outbox(&rt, depth);
        group.bench_with_input(BenchmarkId::new("depth", depth), &depth, |b, _depth| {
            b.to_async(&rt).iter(|| async {
                let batch = store
                    .pending_now(black_box(batch_size))
                    .await
                    .expect("pending_now");
                black_box(batch)
            });
        });
    }
    group.finish();
}

/// Benchmark `DeadlineStore::due_now(batch_size)` at varying overdue deadline counts.
///
/// Simulates the hot path of the deadline scheduler: poll for fired deadlines.
fn bench_deadline_due_now(c: &mut Criterion) {
    let rt = make_rt();
    let batch_size = 100usize;
    let mut group = c.benchmark_group("deadline/due_now");

    for count in [10, 100, 500, 1_000] {
        let store = seeded_deadlines_overdue(&rt, count);
        group.bench_with_input(BenchmarkId::new("overdue", count), &count, |b, _count| {
            b.to_async(&rt).iter(|| async {
                let result = store.due_now(black_box(batch_size)).await.expect("due_now");
                black_box(result)
            });
        });
    }
    group.finish();
}

/// Benchmark `ProjectionRunner::catch_up_matching_streams` replaying streams
/// with `n` events (cold start — empty checkpoint).
///
/// Uses `catch_up_matching_streams` which only requires `EventStore` (no
/// `ProjectionCheckpointStore`), making it suitable for `InMemoryEventStore`.
/// This validates that projection catch-up is O(new events) and that the
/// per-event cost stays bounded even at 1 000 events.
fn bench_projection_catchup(c: &mut Criterion) {
    let rt = make_rt();
    let mut group = c.benchmark_group("projection/catch_up_matching_streams");

    for n_events in [10, 100, 500, 1_000] {
        let (store, _stream) = seeded_events(&rt, n_events);
        group.bench_with_input(BenchmarkId::new("events", n_events), &n_events, |b, _n| {
            b.to_async(&rt).iter(|| async {
                // Cold-start: fresh projection + empty checkpoint each iteration.
                let mut proj = NopProjection::default();
                let checkpoint = GlobalProjectionCheckpoint::new();
                let result = ProjectionRunner::catch_up_matching_streams(
                    &mut proj,
                    &store,
                    None,        // no prefix filter — scan all streams
                    &checkpoint, // empty checkpoint → full replay
                )
                .await
                .expect("catch_up_matching_streams");
                black_box(result)
            });
        });
    }
    group.finish();
}

criterion_group!(
    workers,
    bench_outbox_pending_now,
    bench_deadline_due_now,
    bench_projection_catchup,
);
criterion_main!(workers);
