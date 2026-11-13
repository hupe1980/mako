//! The Einrichtung's pre-fill on the plane: a person's sentence starts an
//! `einrichtung-assistent` run, the model reads it into interview answers, and
//! the answer read keeps an identifier only where the sentence contains it —
//! a code the model completed or made up arrives as an unanswered question.

use std::sync::Arc;

use agentplane::model::{Completion, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Stores};

/// Every other specialist's servers, which this run never reaches.
#[derive(Debug, Default)]
struct Niemand;

#[async_trait::async_trait]
impl ToolClient for Niemand {
    fn destination(&self, _tool: &ToolId) -> agentplane::tools::Destination {
        agentplane::tools::Destination::Local
    }

    async fn call(
        &self,
        tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        panic!("the pre-fill reads no service, and called {tool:?}")
    }
}

#[tokio::test]
async fn a_sentence_is_read_into_answers_and_an_invented_code_is_dropped() {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let tools: Arc<dyn ToolClient> = Arc::new(Niemand);
    let servers = agentd::plane::tools::servers_named_in_grants()
        .into_iter()
        .map(|name| (name, Arc::clone(&tools)))
        .collect();
    let provider = FakeProvider::new();
    let plane = Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-einrichtung",
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

    provider.will_answer(Completion {
        text: String::new(),
        model: None,
        structured: Some(json!({ "angaben": {
            "rollen": [
                { "rolle": "lf-strom", "mp_id": "9900357000004" },
                { "rolle": "lf-gas", "mp_id": "9800357000005" }
            ],
            "primaer": null,
            "bilanzkreise": [{ "sparte": "strom", "standard": ["11XBK-STD------Y"] }],
            "verbundene_lieferanten": []
        }})),
        stop_reason: Some("end_turn".to_owned()),
        tool_calls: Vec::new(),
        usage: Usage::default(),
        truncated: false,
        continuation: None,
        ..Completion::default()
    });

    let satz = "Wir sind Lieferant Strom und Gas; Strom unter 9900357000004, \
                Bilanzkreis 11XBK-STD------Y, keine verbundenen Lieferanten.";
    let accepted = plane
        .accept_named(
            "einrichtung-assistent",
            Envelope {
                id: "e-1",
                source: "urn:mako:test:desk",
                event_type: "manual",
            },
            json!({ "satz": satz }),
        )
        .await
        .expect("activated");
    assert!(accepted.refused.is_none(), "{:?}", accepted.refused);

    let mut answer = None;
    for _ in 0..200 {
        let a = plane
            .answer(&accepted.run_id, "erika")
            .await
            .expect("readable")
            .expect("known");
        if a.status != "running" {
            answer = Some(a);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let answer = answer.expect("the run concluded");
    assert_eq!(answer.status, "succeeded", "{answer:?}");
    assert_eq!(answer.disclosure, "AI-generated.");
    assert!(answer.schritte.is_empty(), "the pre-fill reads no service");
    assert_eq!(
        answer.angaben,
        Some(json!({
            "rollen": [
                { "rolle": "lf-strom", "mp_id": "9900357000004" },
                { "rolle": "lf-gas", "mp_id": null }
            ],
            "primaer": null,
            "bilanzkreise": [{ "sparte": "strom", "standard": ["11XBK-STD------Y"] }],
            "verbundene_lieferanten": []
        }))
    );

    // The Desk's pre-fill fixture is this answer, the run id fixed: a change to
    // the answer's shape fails here until the fixture is blessed
    // (`BLESS_DESK_FIXTURE=1`), so the Desk never renders an answer agentd
    // would not give.
    let mut json = serde_json::to_value(&answer).expect("serializes");
    json["run"] = json!("vorbelegen");
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../desk/fixtures/vorbelegen.json"
    );
    if std::env::var_os("BLESS_DESK_FIXTURE").is_some() {
        let mut text = serde_json::to_string_pretty(&json).expect("serializes");
        text.push('\n');
        std::fs::write(path, text).expect("writes the fixture");
    }
    // Compared as JSON, so the Desk's formatter may lay the file out its way.
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_default())
            .unwrap_or_default();
    assert_eq!(
        fixture, json,
        "desk/fixtures/vorbelegen.json is not agentd's answer — rerun with BLESS_DESK_FIXTURE=1"
    );
}
