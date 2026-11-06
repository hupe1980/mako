//! The proposal that is waiting: every open Freigabe gets exactly one
//! desk-assistant run, however often the tick reads processd's queue, and its
//! checked proposal is read back by the Freigabe's id without starting another.

use std::sync::Arc;

use agentplane::model::{Completion, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::{Activation, Plane, PlaneConfig, Stores};

/// processd with two open Freigaben, and the entry the run reads.
#[derive(Debug, Default)]
struct Processd;

#[async_trait::async_trait]
impl ToolClient for Processd {
    fn destination(&self, _tool: &ToolId) -> agentplane::tools::Destination {
        agentplane::tools::Destination::Local
    }

    async fn call(
        &self,
        tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        Ok(match tool.tool.as_str() {
            "list_pending_approvals" => json!({
                "freigaben": [{ "id": "e1" }, { "id": "e2" }],
                "count": 2,
                "saturated": false,
            }),
            _ => json!({ "id": "e1", "malo_id": "51238696012", "pid": 55001 }),
        })
    }
}

fn proposal() -> Completion {
    Completion {
        text: String::new(),
        model: None,
        structured: Some(json!({
            "saetze": [{ "text": "Die Anmeldung für 51238696012 ist gehalten.", "quelle": "processd/get_queue_entry" }],
            "vorschlag": {
                "aktion": "freigeben",
                "antwortcode": null,
                "begruendung": "Die Anmeldung für 51238696012 ist gehalten.",
                "quelle": "processd/get_queue_entry"
            }
        })),
        stop_reason: Some("end_turn".to_owned()),
        tool_calls: Vec::new(),
        usage: Usage::default(),
        truncated: false,
        continuation: None,
        ..Completion::default()
    }
}

#[tokio::test]
async fn every_open_freigabe_gets_one_proposal_run_however_often_the_tick_reads() {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let processd: Arc<dyn ToolClient> = Arc::new(Processd);
    let servers = agentd::plane::tools::servers_named_in_grants()
        .into_iter()
        .map(|name| (name, Arc::clone(&processd)))
        .collect();
    let provider = FakeProvider::new();
    provider.will_answer(proposal());
    provider.will_answer(proposal());
    let plane = Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-vorschlag",
            tenant: &tenant,
            activated: &Activation::all(),
            providers: vec![(
                "anthropic".to_owned(),
                Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
            )],
            tool_servers: servers,
            policy,
            keyring: None,
        },
    )
    .expect("the plane assembles");

    // Nothing is read back before a tick admitted anything, and the read
    // starts nothing.
    assert!(
        plane
            .vorschlag_of("e1", "erika")
            .await
            .expect("readable")
            .is_none()
    );

    let first = agentd::plane::vorschlag::tick(&plane, processd.as_ref())
        .await
        .expect("the tick reads the queue");
    let second = agentd::plane::vorschlag::tick(&plane, processd.as_ref())
        .await
        .expect("the tick reads the queue");
    assert_eq!(
        (first, second),
        (2, 0),
        "two entries are two runs, not four"
    );

    let mut answer = None;
    for _ in 0..200 {
        answer = plane.vorschlag_of("e1", "erika").await.expect("readable");
        if answer.as_ref().is_some_and(|a| a.status != "running") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let answer = answer.expect("e1 has its proposal run");
    assert_eq!(answer.status, "succeeded", "{answer:?}");
    let vorschlag = answer.vorschlag.expect("a proposal");
    assert_eq!(vorschlag.aktion, "freigeben");
    // The fake model cites a read the run never made, so the Begründung is
    // checked like any sentence and comes back unbelegt.
    assert!(!vorschlag.belegt, "{vorschlag:?}");

    // A Freigabe no tick saw has none, and asking admits nothing.
    assert!(
        plane
            .vorschlag_of("e3", "erika")
            .await
            .expect("readable")
            .is_none()
    );
    let third = agentd::plane::vorschlag::tick(&plane, processd.as_ref())
        .await
        .expect("the tick reads the queue");
    assert_eq!(third, 0);
}
