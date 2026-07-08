//! Benchmark: event-stream replay latency vs. stream length.
//!
//! Measures `EventStore::fold_stream` — the inner loop that `Process::state()`
//! executes on every command dispatch — over streams of varying length using
//! [`InMemoryEventStore`].
//!
//! The parametric sweep `[10, 50, 100, 200, 500, 2000, 10_000]` provides
//! fine-grained evidence for:
//!
//! - **Linearity guard**: latency / n should remain approximately constant
//!   across the sweep.  A super-linear growth (e.g. O(n²) from accidental
//!   Vec cloning) is immediately visible as a diverging ratio.
//!
//! - **Snapshot break-even point**: after snapshot integration, the benchmark
//!   should show the latency plateau at ≤ `snapshot_interval` events once
//!   snapshots are loaded, regardless of raw stream length.
//!
//! Keeping the benchmark on `InMemoryEventStore` eliminates I/O noise so the
//! numbers reflect deserialization cost (`serde_json::from_slice`) and state
//! accumulation overhead in isolation.
//!
//! # `serde_json::Value` allocation baseline
//!
//! The `payload_allocation` benchmark group measures the per-event allocation
//! cost of parsing a representative GPKE event payload into a
//! `serde_json::Value` tree (current design) vs. cloning raw `Bytes`
//! (hypothetical deferred-parse design). This provides the evidence needed to
//! decide whether a `Bytes`-based migration is worth the API-breaking cost.
//!
//! Expected interpretation:
//! - If `parse_value` ≈ `clone_bytes` — migration is not cost-effective.
//! - If `parse_value` >> `clone_bytes` at 10k events — consider migration
//!   and re-evaluate at that point.
//!
//! # Running
//!
//! ```bash
//! cargo bench -p mako-engine --bench replay_latency --features testing
//! ```
//!
//! Results are written to `target/criterion/replay_latency/`.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mako_engine::{
    envelope::NewEvent,
    error::EngineError,
    event_store::{EventStore, ExpectedVersion, InMemoryEventStore},
    ids::{ConversationId, CorrelationId, ProcessId, StreamId, TenantId},
    version::WorkflowId,
};
use tokio::runtime::Runtime;

/// Stream lengths used for the parametric sweep.
///
/// The fine-grained spacing (10 → 50 → 100 → 200 → 500 → 2 000 → 10 000) lets
/// the Criterion regression harness detect a slope change early: an O(n²) bug
/// becomes detectable between the 200 and 500 data points rather than only at
/// the 10 000 endpoint.
const STREAM_LENGTHS: &[usize] = &[10, 50, 100, 200, 500, 2_000, 10_000];

fn make_rt() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn wid() -> WorkflowId {
    WorkflowId::new("bench-workflow", "FV2025-10-01")
}

fn make_event() -> NewEvent {
    // Payload mirrors the smallest real GPKE SupplierChangeInitiated event so
    // the serde cost is representative of production workloads.
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
            "sender":             "4012345000023",
            "receiver":           "9900357000004",
            "location_id":        "51238696781",
            "document_date":      "20250115",
            "message_ref":        "REF-0000",
            "validation_passed":  true,
            "validation_errors":  []
        }),
    )
}

/// Create a seeded store with `n` events on a single stream.
fn seeded_store(rt: &Runtime, n: usize) -> (InMemoryEventStore, StreamId) {
    let store = InMemoryEventStore::new();
    let stream = StreamId::new("bench/replay-stream");
    let events: Vec<NewEvent> = (0..n).map(|_| make_event()).collect();

    rt.block_on(async {
        store
            .append(&stream, ExpectedVersion::NoStream, &events)
            .await
            .expect("seeded append must succeed");
    });

    (store, stream)
}

fn bench_replay_latency(c: &mut Criterion) {
    let rt = make_rt();
    let mut group = c.benchmark_group("replay_latency");

    for &n in STREAM_LENGTHS {
        let (store, stream) = seeded_store(&rt, n);

        // Throughput expressed as "events per second" — lets Criterion
        // compute MB/s only if we were measuring bytes; here it normalises
        // the Y-axis to ns/event which makes the O(n) slope immediately
        // visible in the HTML report.
        group.throughput(Throughput::Elements(n as u64));

        group.bench_with_input(BenchmarkId::new("fold_stream", n), &n, |b, _| {
            b.to_async(&rt).iter(|| async {
                let count: usize = store
                    .fold_stream(
                        &stream,
                        0,
                        0usize,
                        |acc, env| -> Result<usize, EngineError> {
                            black_box(&env);
                            Ok(acc + 1)
                        },
                    )
                    .await
                    .expect("fold_stream must succeed");
                assert_eq!(count, n, "all events must be replayed");
            });
        });
    }

    group.finish();
}

// ── — Value vs Bytes payload allocation baseline ────────────────────────

/// Representative GPKE event payload JSON (small-to-medium; ~200 bytes compact).
const SAMPLE_PAYLOAD_JSON: &str = r#"{
  "pruefidentifikator": 55001,
  "sender":             "4012345000023",
  "receiver":           "9900357000004",
  "location_id":        "51238696781",
  "document_date":      "20250115",
  "message_ref":        "REF-0000",
  "validation_passed":  true,
  "validation_errors":  []
}"#;

/// Measure the cost of parsing `n` copies of the sample payload into
/// `serde_json::Value` trees (current design: allocates a full JSON tree per
/// event on every replay).
fn bench_payload_parse_value(c: &mut Criterion) {
    // Use Arc<[u8]> as a cheap clone proxy; cloning increments a single refcount
    // and represents the lower bound of a hypothetical deferred-parse design.
    let raw: std::sync::Arc<[u8]> = SAMPLE_PAYLOAD_JSON.as_bytes().into();
    let mut group = c.benchmark_group("payload_allocation");

    for &n in STREAM_LENGTHS {
        group.throughput(Throughput::Elements(n as u64));

        group.bench_with_input(BenchmarkId::new("parse_value", n), &n, |b, &n| {
            b.iter(|| {
                let mut total: usize = 0;
                for _ in 0..n {
                    let v: serde_json::Value =
                        serde_json::from_slice(black_box(raw.as_ref())).unwrap();
                    // Simulate what apply() does: access one field.
                    total += v
                        .get("pruefidentifikator")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as usize;
                }
                black_box(total)
            });
        });

        // Baseline: O(1) arc clone — simulates carrying raw bytes through replay
        // without parsing. Represents the lower bound of a hypothetical
        // deferred-parse design where `EventEnvelope::payload` is `Arc<[u8]>`
        // and parsing happens lazily in `apply()` only.
        group.bench_with_input(BenchmarkId::new("clone_arc_bytes", n), &n, |b, &n| {
            b.iter(|| {
                let mut total: usize = 0;
                for _ in 0..n {
                    let cloned = black_box(raw.clone());
                    total += cloned.len();
                }
                black_box(total)
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_replay_latency, bench_payload_parse_value);
criterion_main!(benches);
