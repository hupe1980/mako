//! Integration tests for [`SlateDbStore`] and its companion stores.
//!
//! Covers the following correctness properties:
//! - `append` + `load` round-trip
//! - `VersionConflict` on concurrent append to the same stream
//! - `stream_version` consistency after append
//! - `list_streams` prefix filter correctness
//! - `pending` chronological ordering when `deliver_after` is set
//! - `acknowledge` removes both `om/` and `ot/` entries
//! - `append_with_outbox` atomicity (events + outbox in one transaction)
//! - `SlateDbDeadlineStore::due_now` returns only expired deadlines
//! - `SlateDbDeadlineStore::due_now` sets `has_more` correctly
//! - `SlateDbInboxStore::accept` deduplication across two callers
//! - Inbox key length guard (`MAX_INBOX_KEY_LEN`)
//! - Registry key validation (`RegistryKey::parse`)
//! - `SlateDbDeadLetterSink::reject` persists records to `dr/` key space
//! - `SlateDbDeadLetterSink::list_recent` returns records in reverse-chronological order
//! - Dead-letter `dr/` keys do not interfere with `ot/`, `dt/`, `it/` key spaces
//!
//! All tests use `SlateDbStore::open_in_memory()` for isolation.

#![cfg(feature = "slatedb")]

use mako_engine::{
    dead_letter::{DeadLetterReason, DeadLetterSink as _},
    deadline::{Deadline, DeadlineStore},
    envelope::NewEvent,
    error::EngineError,
    event_store::{AtomicAppend, EventStore, ExpectedVersion},
    ids::{ConversationId, CorrelationId, ProcessId, StreamId, TenantId},
    inbox::{InboxStore, MAX_INBOX_KEY_LEN},
    outbox::{OutboxStore, PendingOutbox},
    registry::{MAX_REGISTRY_KEY_LEN, ProcessRegistry, RegistryKey},
    store_slatedb::SlateDbStore,
    version::WorkflowId,
};
use time::OffsetDateTime;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn wid() -> WorkflowId {
    WorkflowId::new("TestWorkflow", "FV2025-10-01")
}

fn new_event() -> NewEvent {
    NewEvent::new(
        CorrelationId::new(),
        None,
        ConversationId::new(),
        ProcessId::new(),
        TenantId::new(),
        wid(),
        "TestEvent",
        1,
        serde_json::json!({"v": 1}),
    )
}

fn pending_outbox() -> PendingOutbox {
    PendingOutbox::new("APERAK", "9900000000001", serde_json::json!({"ok": true}))
}

async fn open() -> SlateDbStore {
    SlateDbStore::open_in_memory()
        .await
        .expect("open_in_memory must succeed")
}

// ── EventStore tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn append_and_load_roundtrip() {
    let store = open().await;
    let stream = StreamId::new("test/roundtrip");

    let result = store
        .append(&stream, ExpectedVersion::NoStream, &[new_event()])
        .await
        .expect("append must succeed");
    assert_eq!(result.last_sequence, 1);

    let events = store.load(&stream).await.expect("load must succeed");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].sequence_number, 1);
    assert_eq!(events[0].event_type.as_ref(), "TestEvent");
}

#[tokio::test]
async fn version_conflict_on_stale_expected_version() {
    let store = open().await;
    let stream = StreamId::new("test/conflict");

    store
        .append(&stream, ExpectedVersion::NoStream, &[new_event()])
        .await
        .expect("first append must succeed");

    let err = store
        .append(&stream, ExpectedVersion::Exact(0), &[new_event()])
        .await
        .expect_err("append at wrong version must fail");
    assert!(
        matches!(err, EngineError::VersionConflict { .. }),
        "expected VersionConflict, got {err:?}"
    );
}

#[tokio::test]
async fn stream_version_tracks_sequence() {
    let store = open().await;
    let stream = StreamId::new("test/version-track");

    assert_eq!(
        store.stream_version(&stream).await.unwrap(),
        0,
        "empty stream must have version 0"
    );

    store
        .append(&stream, ExpectedVersion::NoStream, &[new_event()])
        .await
        .unwrap();
    assert_eq!(store.stream_version(&stream).await.unwrap(), 1);

    store
        .append(
            &stream,
            ExpectedVersion::Exact(1),
            &[new_event(), new_event()],
        )
        .await
        .unwrap();
    assert_eq!(store.stream_version(&stream).await.unwrap(), 3);
}

#[tokio::test]
async fn list_streams_prefix_filter() {
    let store = open().await;

    for name in ["gpke/s1", "gpke/s2", "wim/s1"] {
        store
            .append(
                &StreamId::new(name),
                ExpectedVersion::NoStream,
                &[new_event()],
            )
            .await
            .unwrap();
    }

    let all = store.list_streams(None).await.unwrap();
    assert!(all.len() >= 3, "must list all streams");

    let gpke = store.list_streams(Some("gpke/")).await.unwrap();
    assert_eq!(gpke.len(), 2, "must return only gpke/ streams");
    assert!(gpke.iter().all(|s| s.as_str().starts_with("gpke/")));
}

#[tokio::test]
async fn fold_stream_accumulates_from_sequence() {
    let store = open().await;
    let stream = StreamId::new("test/fold");

    for _ in 0..4 {
        store
            .append(&stream, ExpectedVersion::Any, &[new_event()])
            .await
            .unwrap();
    }

    let count = store
        .fold_stream(&stream, 0, 0usize, |acc, _env| Ok(acc + 1))
        .await
        .unwrap();
    assert_eq!(count, 4, "fold from 0 must count all 4 events");

    let tail = store
        .fold_stream(&stream, 2, 0usize, |acc, _env| Ok(acc + 1))
        .await
        .unwrap();
    assert_eq!(tail, 2, "fold from seq 2 must count only 2 tail events");
}

// ── AtomicAppend / OutboxStore tests ─────────────────────────────────────────

#[tokio::test]
async fn append_with_outbox_is_atomic_and_causation_linked() {
    let store = open().await;
    let stream = StreamId::new("test/atomic");

    store
        .append_with_outbox(
            &stream,
            ExpectedVersion::NoStream,
            &[new_event()],
            &[pending_outbox()],
        )
        .await
        .expect("append_with_outbox must succeed");

    let events = store.load(&stream).await.unwrap();
    assert_eq!(events.len(), 1, "event must be persisted");

    let msgs = store.pending(10, OffsetDateTime::now_utc()).await.unwrap();
    assert_eq!(msgs.len(), 1, "outbox message must be pending");
    // Causation linkage: outbox entry must reference the persisted event.
    assert_eq!(
        msgs[0].causation_event_id, events[0].event_id,
        "causation_event_id must link to the appended event"
    );
}

#[tokio::test]
async fn acknowledge_removes_outbox_message() {
    let store = open().await;
    let stream = StreamId::new("test/ack");

    store
        .append_with_outbox(
            &stream,
            ExpectedVersion::NoStream,
            &[new_event()],
            &[pending_outbox()],
        )
        .await
        .unwrap();

    let msgs = store.pending(10, OffsetDateTime::now_utc()).await.unwrap();
    assert_eq!(msgs.len(), 1);

    store.acknowledge(msgs[0].message_id).await.unwrap();

    let after = store.pending(10, OffsetDateTime::now_utc()).await.unwrap();
    assert!(after.is_empty(), "acknowledged message must not be pending");
}

#[tokio::test]
async fn pending_excludes_future_dated_messages() {
    let store = open().await;
    let stream = StreamId::new("test/future");

    let now = OffsetDateTime::now_utc();
    let future = now + time::Duration::hours(1);

    let due_now = PendingOutbox::new("APERAK", "9900000000001", serde_json::json!({"now": true}));
    let deferred = PendingOutbox::new(
        "APERAK",
        "9900000000001",
        serde_json::json!({"future": true}),
    )
    .with_deliver_after(future);

    store
        .append_with_outbox(
            &stream,
            ExpectedVersion::NoStream,
            &[new_event()],
            &[due_now, deferred],
        )
        .await
        .unwrap();

    let due = store.pending(10, OffsetDateTime::now_utc()).await.unwrap();
    assert_eq!(due.len(), 1, "only the immediately-due message must appear");
}

// ── DeadlineStore tests ───────────────────────────────────────────────────────

fn make_deadline(due_at: OffsetDateTime) -> Deadline {
    Deadline::new(
        StreamId::new("test/deadline-stream"),
        ProcessId::new(),
        TenantId::new(),
        wid(),
        "test-deadline",
        due_at,
    )
}

#[tokio::test]
async fn due_now_returns_only_expired_deadlines() {
    let store = open().await;
    let ds = store.as_deadline_store();

    let now = OffsetDateTime::now_utc();
    let past = make_deadline(now - time::Duration::minutes(5));
    let future = make_deadline(now + time::Duration::hours(1));

    let past_id = past.deadline_id();
    ds.register(&past).await.unwrap();
    ds.register(&future).await.unwrap();

    let result = ds.due_now(10).await.unwrap();
    assert_eq!(
        result.deadlines.len(),
        1,
        "only the past deadline must be due"
    );
    assert_eq!(result.deadlines[0].deadline_id(), past_id);
    assert!(
        !result.has_more,
        "has_more must be false when only one overdue deadline"
    );
}

#[tokio::test]
async fn due_now_has_more_set_when_exactly_at_limit() {
    let store = open().await;
    let ds = store.as_deadline_store();

    let past = OffsetDateTime::now_utc() - time::Duration::seconds(10);
    for _ in 0..3 {
        ds.register(&make_deadline(past)).await.unwrap();
    }

    let result = ds.due_now(2).await.unwrap();
    assert_eq!(
        result.deadlines.len(),
        2,
        "must return exactly limit deadlines"
    );
    assert!(
        result.has_more,
        "has_more must be true when more overdue entries exist"
    );
}

// ── InboxStore tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn inbox_accept_deduplicates() {
    let store = open().await;
    let inbox = store.as_inbox_store();

    assert!(
        inbox.accept("msg-001").await.unwrap(),
        "first accept must return true"
    );
    assert!(
        !inbox.accept("msg-001").await.unwrap(),
        "duplicate must return false"
    );
    assert!(
        inbox.accept("msg-002").await.unwrap(),
        "different key must be accepted"
    );
}

#[tokio::test]
async fn inbox_key_too_long_returns_error() {
    let store = open().await;
    let inbox = store.as_inbox_store();

    let oversized = "x".repeat(MAX_INBOX_KEY_LEN + 1);
    let err = inbox
        .accept(&oversized)
        .await
        .expect_err("oversized key must be rejected");
    assert!(
        matches!(err, EngineError::Inbox { .. }),
        "expected Inbox error, got {err:?}"
    );
}

// ── ProcessRegistry tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn process_registry_register_lookup_remove() {
    use mako_engine::ids::{ProcessId, ProcessIdentity};

    let store = open().await;
    let registry = store.as_process_registry();
    let tenant = TenantId::new();

    let identity = ProcessIdentity::new(ProcessId::new(), tenant, wid());
    let key = RegistryKey::parse("conv:integration-test").expect("valid key");

    registry
        .register(tenant, &key, identity.clone())
        .await
        .unwrap();

    let found = registry.lookup(tenant, &key).await.unwrap();
    assert!(found.is_some(), "registered identity must be found");
    assert_eq!(found.unwrap().stream_id(), identity.stream_id());

    registry.remove(tenant, &key).await.unwrap();

    let gone = registry.lookup(tenant, &key).await.unwrap();
    assert!(
        gone.is_none(),
        "removed identity must not be found after remove"
    );
}

// ── RegistryKey validation tests ─────────────────────────────────────────────

#[test]
fn registry_key_try_from_str_rejects_nul_bytes() {
    let err = RegistryKey::parse("bad\0key").expect_err("NUL byte must be rejected");
    assert!(matches!(err, EngineError::Registry { .. }));
}

#[test]
fn registry_key_try_from_str_rejects_oversized() {
    let long = "k".repeat(MAX_REGISTRY_KEY_LEN + 1);
    let err = RegistryKey::parse(&long).expect_err("oversized key must be rejected");
    assert!(matches!(err, EngineError::Registry { .. }));
}

#[test]
fn registry_key_try_from_str_accepts_valid() {
    let key = RegistryKey::parse("conv:valid-key").expect("valid key must be accepted");
    assert_eq!(key.as_str(), "conv:valid-key");
}

// ── Concurrent accept (SSI CAS correctness) ───────────────────────────────────

/// Fire N concurrent tasks all trying to `accept` the same inbox key.
///
/// Exactly one must succeed (return `true`). All others must observe the
/// committed sentinel and return `false`. This validates the SSI transaction
/// CAS correctness — previously the in-process `DashMap<Mutex>` guard could
/// not provide this guarantee across multiple `makod` instances; the SSI
/// transaction makes it linearisable for all concurrent callers sharing the
/// same SlateDB storage.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbox_concurrent_accept_exactly_one_winner() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let store = Arc::new(open().await);
    let wins = Arc::new(AtomicUsize::new(0));
    const N: usize = 16;

    let handles: Vec<_> = (0..N)
        .map(|_| {
            let inbox = store.as_inbox_store();
            let wins = Arc::clone(&wins);
            tokio::spawn(async move {
                match inbox.accept("concurrent-key").await {
                    Ok(true) => {
                        wins.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(false) => {}
                    Err(e) => panic!("accept failed: {e}"),
                }
            })
        })
        .collect();

    for h in handles {
        h.await.expect("task must not panic");
    }

    assert_eq!(
        wins.load(Ordering::Relaxed),
        1,
        "exactly one concurrent accept must win; SSI CAS broken if this fails"
    );
}

// ── Dead-letter store tests ───────────────────────────────────

/// `reject()` persists a `DeadLetterRecord` to the `dr/` key space and
/// `list_dead_letters(1)` returns it (most-recent first).
///
/// Uses the mpsc-buffered sink: spawn the worker, reject, signal
/// shutdown, await worker, then read.
#[tokio::test]
async fn dead_letter_persists_and_lists() {
    let store = open().await;
    let (sink, worker) = store.as_dead_letter_sink();
    let handle = tokio::spawn(worker.run());

    sink.reject(&DeadLetterReason::UnknownPid(55001));

    // Close the channel; the worker drains remaining entries then exits.
    sink.signal_shutdown();
    let persisted = handle.await.expect("worker task must not panic");
    assert_eq!(persisted, 1, "expected exactly 1 entry persisted");

    let records = store
        .list_dead_letters(10)
        .await
        .expect("list_dead_letters must succeed");
    assert_eq!(records.len(), 1, "expected exactly 1 dead-letter record");
    assert_eq!(records[0].reason_label, "unknown_pid");
    assert!(
        records[0].reason_detail.contains("55001"),
        "reason_detail should mention the unknown PID"
    );
}

/// Multiple `reject()` calls produce multiple records; `list_dead_letters(limit)`
/// returns them in reverse-chronological order (most-recent first) and
/// respects the `limit` cap.
#[tokio::test]
async fn dead_letter_list_recent_order_and_limit() {
    let store = open().await;
    let (sink, worker) = store.as_dead_letter_sink();
    let handle = tokio::spawn(worker.run());

    let reasons = [
        DeadLetterReason::UnknownPid(55001),
        DeadLetterReason::UnknownConversation {
            conversation_id: "conv-A".into(),
        },
        DeadLetterReason::DuplicateMessage {
            inbox_key: "msg-id-XYZ".into(),
        },
    ];

    for reason in &reasons {
        sink.reject(reason);
        // Slight delay so each record gets a distinct timestamp key.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }

    // Close the channel; the worker drains and exits.
    sink.signal_shutdown();
    let persisted = handle.await.expect("worker must not panic");
    assert_eq!(persisted, 3, "all 3 entries must be persisted");

    // limit = 2: should return the 2 most-recent records.
    let recent = store
        .list_dead_letters(2)
        .await
        .expect("list_dead_letters must succeed");
    assert_eq!(recent.len(), 2, "limit should cap the result to 2");
    // Most-recent first: DuplicateMessage was last.
    assert_eq!(recent[0].reason_label, "duplicate_message");
    assert_eq!(recent[1].reason_label, "unknown_conversation");

    // list_dead_letters(0) returns empty.
    let empty = store
        .list_dead_letters(0)
        .await
        .expect("list_dead_letters(0) must succeed");
    assert!(empty.is_empty(), "limit=0 must return empty vec");
}

/// Dead-letter records written by one sink are visible via `store.list_dead_letters`.
///
/// `signal_shutdown()` on any clone closes the shared channel, causing the
/// worker to drain and exit cleanly.
#[tokio::test]
async fn dead_letter_visible_after_worker_drains() {
    let store = open().await;

    let (sink, worker) = store.as_dead_letter_sink();
    let handle = tokio::spawn(worker.run());

    sink.reject(&DeadLetterReason::ProcessingError {
        message: "simulated adapter crash".into(),
    });

    sink.signal_shutdown();
    handle.await.expect("worker must not panic");

    let records = store
        .list_dead_letters(10)
        .await
        .expect("list_dead_letters must succeed");

    assert_eq!(
        records.len(),
        1,
        "record must be visible after worker drain"
    );
    assert_eq!(records[0].reason_label, "processing_error");
    assert!(
        records[0].reason_detail.contains("simulated adapter crash"),
        "reason detail must be preserved",
    );
}

/// `dr/` key space does not interfere with `ot/`, `dt/`, or `it/` key spaces.
///
/// Writes events to the outbox, registers a deadline, accepts an inbox key,
/// and then verifies `list_dead_letters` still returns only the dead-letter records.
#[tokio::test]
async fn dead_letter_key_space_is_isolated() {
    let store = open().await;
    let (sink, worker) = store.as_dead_letter_sink();
    let handle = tokio::spawn(worker.run());

    // Populate other key spaces.
    let stream = StreamId::new("test/dl-isolation");
    store
        .append_with_outbox(
            &stream,
            ExpectedVersion::NoStream,
            &[new_event()],
            &[pending_outbox()],
        )
        .await
        .expect("append_with_outbox must succeed");
    store
        .as_deadline_store()
        .register(&make_deadline(
            time::OffsetDateTime::now_utc() + time::Duration::seconds(60),
        ))
        .await
        .expect("register deadline must succeed");
    store
        .as_inbox_store()
        .accept("isolation-test-key")
        .await
        .expect("accept must succeed");

    // Write a dead-letter record.
    sink.reject(&DeadLetterReason::VersionMismatch {
        expected: "FV2025-10-01".into(),
        received: "FV2024-04-01".into(),
    });

    // Drain and verify.
    sink.signal_shutdown();
    handle.await.expect("worker must not panic");

    let records = store
        .list_dead_letters(100)
        .await
        .expect("list_dead_letters must succeed");
    assert_eq!(
        records.len(),
        1,
        "list_dead_letters must return only dead-letter records"
    );
    assert_eq!(records[0].reason_label, "version_mismatch");
}

// ── SSI concurrent-write regression ───────────────────────────────────

/// Two concurrent `append()` calls to the **same stream** must never both
/// succeed with `ExpectedVersion::NoStream` — exactly one must observe a
/// `VersionConflict`.  This is the serialisability guarantee provided by
/// SlateDB's `IsolationLevel::SerializableSnapshot`.
///
/// A regression here would indicate that the isolation level was weakened
/// (e.g., accidentally defaulted to `ReadCommitted`), which would allow the
/// event log to diverge and break replay determinism.
#[tokio::test]
async fn concurrent_append_to_same_stream_ssi_one_wins() {
    let store = std::sync::Arc::new(open().await);
    let stream = StreamId::new("test/concurrent-ssi");

    let s1 = store.clone();
    let s2 = store.clone();
    let sid1 = stream.clone();
    let sid2 = stream.clone();

    let (r1, r2) = tokio::join!(
        tokio::spawn(async move {
            s1.append(&sid1, ExpectedVersion::NoStream, &[new_event()])
                .await
        }),
        tokio::spawn(async move {
            s2.append(&sid2, ExpectedVersion::NoStream, &[new_event()])
                .await
        }),
    );

    let r1 = r1.expect("task 1 must not panic");
    let r2 = r2.expect("task 2 must not panic");

    match (&r1, &r2) {
        (Ok(_), Ok(_)) => {
            panic!("both concurrent appends succeeded with NoStream — SSI is not enforced")
        }
        (Err(e), Ok(_)) | (Ok(_), Err(e)) => {
            assert!(
                matches!(e, EngineError::VersionConflict { .. }),
                "losing append must produce VersionConflict, got {e:?}"
            );
        }
        (Err(_), Err(_)) => {
            // Both failing is acceptable (aggressive SSI under high contention);
            // what is NOT acceptable is both succeeding (checked above).
        }
    }

    // The winning append must have produced exactly one event in the stream.
    let events = store.load(&stream).await.expect("load must succeed");
    assert_eq!(
        events.len(),
        1,
        "exactly one event must be in the stream after a concurrent-write race"
    );
}

/// After a `VersionConflict`, retrying the losing task with the correct
/// `ExpectedVersion::Exact` must succeed — the engine's retry loop is only
/// correct if SlateDB exposes the current version through the conflict error.
#[tokio::test]
async fn concurrent_append_conflict_retry_succeeds() {
    let store = std::sync::Arc::new(open().await);
    let stream = StreamId::new("test/concurrent-retry");

    let s1 = store.clone();
    let s2 = store.clone();
    let sid1 = stream.clone();
    let sid2 = stream.clone();

    let (r1, r2) = tokio::join!(
        tokio::spawn(async move {
            s1.append(&sid1, ExpectedVersion::NoStream, &[new_event()])
                .await
        }),
        tokio::spawn(async move {
            s2.append(&sid2, ExpectedVersion::NoStream, &[new_event()])
                .await
        }),
    );
    let r1 = r1.expect("task 1 must not panic");
    let r2 = r2.expect("task 2 must not panic");

    // Determine which one won and which one lost.
    let loser_retry = match (&r1, &r2) {
        (Ok(_), Err(_)) => {
            // r2 lost; retry at version 1.
            store
                .append(&stream, ExpectedVersion::Exact(1), &[new_event()])
                .await
        }
        (Err(_), Ok(_)) => {
            // r1 lost; retry at version 1.
            store
                .append(&stream, ExpectedVersion::Exact(1), &[new_event()])
                .await
        }
        (Ok(_), Ok(_)) => panic!("both appends succeeded — SSI not enforced"),
        (Err(_), Err(_)) => return, // both failed; skip retry assertion
    };

    loser_retry.expect("retry after VersionConflict must succeed");

    let events = store.load(&stream).await.expect("load must succeed");
    assert_eq!(
        events.len(),
        2,
        "both events must be present after successful retry"
    );
}
