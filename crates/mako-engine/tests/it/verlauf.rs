//! An event that is a market message carries its direction, PID and reference
//! onto the stored envelope — what the Desk's Verlauf reads — and an envelope
//! written before the field existed still loads.

use mako_engine::{
    envelope::{EventEnvelope, Richtung, Verlauf},
    event_store::{AppendBatch, AtomicAppend, EventStore, ExpectedVersion, InMemoryEventStore},
    ids::{ProcessId, StreamId, TenantId},
    version::WorkflowId,
    workflow::{CommandContext, EventPayload},
};

#[derive(serde::Serialize, serde::Deserialize)]
enum Ev {
    AnfrageGesendet,
    Aktiviert,
}

impl EventPayload for Ev {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet => "AnfrageGesendet",
            Self::Aktiviert => "Aktiviert",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnfrageGesendet => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(55001),
                message_ref: Some("REF-1".to_owned()),
            }),
            Self::Aktiviert => None,
        }
    }
}

#[tokio::test]
async fn a_message_event_stores_its_verlauf_and_a_state_change_stores_none() {
    let store = InMemoryEventStore::new();
    let process = ProcessId::new();
    let ctx = CommandContext::new(
        TenantId::new(),
        process,
        WorkflowId::new("verlauf", "FV2025-10-01"),
    );
    let stream = StreamId::for_process(ctx.tenant_id, &process);
    let events = [
        ctx.new_event(&Ev::AnfrageGesendet).expect("event"),
        ctx.new_event(&Ev::Aktiviert).expect("event"),
    ];
    store
        .append(AppendBatch::new(&stream, ExpectedVersion::Any, &events))
        .await
        .expect("append");

    let loaded = store.load(&stream).await.expect("load");
    assert_eq!(
        loaded[0].verlauf,
        Some(Verlauf {
            richtung: Richtung::Ausgang,
            pid: Some(55001),
            message_ref: Some("REF-1".to_owned()),
        })
    );
    assert_eq!(loaded[1].verlauf, None);
}

#[test]
fn an_envelope_stored_without_verlauf_still_reads() {
    let process = ProcessId::new();
    let tenant = TenantId::new();
    let ctx = CommandContext::new(tenant, process, WorkflowId::new("verlauf", "FV2025-10-01"));
    let envelope = EventEnvelope::from_new(
        ctx.new_event(&Ev::Aktiviert).expect("event"),
        StreamId::for_process(tenant, &process),
        1,
        time::OffsetDateTime::UNIX_EPOCH,
    );
    let mut json = serde_json::to_value(&envelope).expect("serialise");
    assert!(
        json.get("verlauf").is_none(),
        "absent, not null, when there is none"
    );
    json.as_object_mut().expect("object").remove("verlauf");
    let read: EventEnvelope = serde_json::from_value(json).expect("an old envelope loads");
    assert_eq!(read.verlauf, None);
}
