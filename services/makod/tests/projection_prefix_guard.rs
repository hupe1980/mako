//! Guard: a projection worker's stream prefix matches the streams that exist.
//!
//! `ProjectionWorker::new` takes an optional prefix and passes it to
//! `list_streams`, which scans the stream index under `si/{prefix}`. Every
//! process stream in this platform is named `process/{tenant}/{process}` —
//! `StreamId::for_process` builds no other shape — so a prefix that is not a
//! prefix *of that* selects nothing and the projection folds zero events for
//! the life of the deployment.
//!
//! That failure is completely silent. The worker ticks, the heartbeat updates,
//! `GET /health` reports it healthy, and `catch_up_persistent` returns an empty
//! checkpoint every time. Two workers shipped in exactly that state, wired to
//! `Some("gpke/")`, and the makod operations table described their failure mode
//! as „read models serve stale data" — they served nothing at all.

use mako_engine::{
    envelope::NewEvent,
    event_store::{EventStore as _, ExpectedVersion},
    ids::{ConversationId, CorrelationId, ProcessId, StreamId, TenantId},
    store_slatedb::SlateDbStore,
    version::WorkflowId,
};

/// Append one process event and return the store it lives in.
async fn store_with_one_process() -> (SlateDbStore, StreamId) {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store opens");
    let tenant = TenantId::from_uuid(uuid::Uuid::new_v4());
    let process = ProcessId::from_uuid(uuid::Uuid::new_v4());
    let stream = StreamId::for_process(tenant, &process);
    store
        .append(
            &stream,
            ExpectedVersion::Any,
            &[NewEvent::new(
                CorrelationId::new(),
                None,
                ConversationId::new(),
                process,
                tenant,
                WorkflowId::new("projection-prefix-guard", "FV2026-04-01"),
                "ProjectionPrefixGuardProbe",
                1,
                serde_json::json!({}),
            )],
        )
        .await
        .expect("append");
    (store, stream)
}

/// The canonical shape, spelled out so the guard below states what it assumes.
#[tokio::test]
async fn a_process_stream_is_named_process_slash_tenant_slash_process() {
    let (_store, stream) = store_with_one_process().await;
    let raw = stream.as_str();
    assert!(
        raw.starts_with("process/"),
        "process streams are `process/{{tenant}}/{{process}}`, got {raw}"
    );
    assert_eq!(
        raw.split('/').count(),
        3,
        "three segments, not two or four: {raw}"
    );
}

/// A prefix naming a domain rather than the stream shape selects nothing.
///
/// This is the defect itself, pinned so it cannot come back as a plausible
/// looking `Some("wim/")` or `Some("mabis/")`.
#[tokio::test]
async fn a_domain_flavoured_prefix_selects_no_stream() {
    let (store, _stream) = store_with_one_process().await;

    let all = store.list_streams(None).await.expect("list all");
    assert_eq!(all.len(), 1, "the probe stream exists");

    let by_shape = store
        .list_streams(Some("process/"))
        .await
        .expect("list by shape");
    assert_eq!(by_shape.len(), 1, "`process/` is the prefix that works");

    for wrong in ["gpke/", "mabis/", "wim/", "geli-gas/"] {
        let found = store.list_streams(Some(wrong)).await.expect("list");
        assert!(
            found.is_empty(),
            "`{wrong}` matched {} stream(s) — if this ever passes, the stream \
             naming changed and this guard's premise is stale",
            found.len()
        );
    }
}

/// Every prefix `startup` hands a projection worker must select real streams.
///
/// A source scan rather than a behavioural check: the wiring is what went
/// wrong, and it is wrong at the call site regardless of what the projection
/// then does with the events.
#[test]
fn every_wired_projection_prefix_can_match_a_process_stream() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/startup/mod.rs"))
        .expect("startup/mod.rs is a workspace file");

    let mut checked = 0usize;
    for (i, _) in src.match_indices("ProjectionWorker::new(") {
        let tail = &src[i..];
        let end = tail.find(')').map_or(tail.len(), |e| e + 1);
        let args = &tail[..end.max(200).min(tail.len())];
        checked += 1;
        // The prefix argument is `None` or `Some("…")`.
        if let Some(start) = args.find("Some(\"") {
            let rest = &args[start + 6..];
            let prefix = &rest[..rest.find('"').expect("closing quote")];
            assert!(
                "process/".starts_with(prefix) || prefix.starts_with("process/"),
                "projection worker #{checked} is wired to prefix `{prefix}`, which no \
                 `process/{{tenant}}/{{process}}` stream starts with — it would fold zero \
                 events and report healthy while doing it"
            );
        }
    }
    assert!(
        checked > 0,
        "no `ProjectionWorker::new(` in startup/mod.rs — the wiring moved and this \
         guard now checks nothing"
    );
}
