//! The proposal that is waiting: every open Freigabe, Rechnungsdisput and
//! parked booking gets exactly one desk-assistant run, however often the tick
//! reads its list, and its checked proposal is read back by its kind and id
//! without starting another.

use std::sync::Arc;

use agentplane::model::{Completion, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::vorschlag::Art;
use agentd::plane::{Activation, Plane, PlaneConfig, Stores};

/// processd with two open Freigaben and invoicd with one open dispute, and the
/// entry the run reads.
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
            "list_disputes" => json!({
                "disputes": [{ "id": "d1" }],
                "count": 1,
                "saturated": false,
            }),
            "list_parked_bookings" => json!({
                "buchungen": [{ "key": "rechnung:RE-1" }],
                "count": 1,
                "saturated": false,
            }),
            "get_parked_booking" => json!({
                "key": "rechnung:RE-1",
                "buchungsdatum": "2026-01-20",
                "vorschlag_datum": "2026-02-01",
            }),
            _ => json!({ "id": "e1", "malo_id": "51238696012", "pid": 55001 }),
        })
    }
}

fn proposal(vorschlag: Value) -> Completion {
    Completion {
        text: String::new(),
        model: None,
        structured: Some(json!({
            "saetze": [{ "text": "Die Anmeldung für 51238696012 ist gehalten.", "quelle": "processd/get_queue_entry" }],
            "vorschlag": vorschlag,
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
    let freigeben = json!({
        "aktion": "freigeben",
        "antwortcode": null,
        "befund": null,
        "buchungsdatum": null,
        "grund": null,
        "begruendung": "Die Anmeldung für 51238696012 ist gehalten.",
        "quelle": "processd/get_queue_entry"
    });
    let provider = FakeProvider::new();
    provider.will_answer(proposal(freigeben.clone()));
    provider.will_answer(proposal(freigeben));
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
            .vorschlag_of(Art::Freigabe, "e1", "erika")
            .await
            .expect("readable")
            .is_none()
    );

    let first = agentd::plane::vorschlag::tick(&plane, Art::Freigabe, processd.as_ref())
        .await
        .expect("the tick reads the queue");
    let second = agentd::plane::vorschlag::tick(&plane, Art::Freigabe, processd.as_ref())
        .await
        .expect("the tick reads the queue");
    assert_eq!(
        (first, second),
        (2, 0),
        "two entries are two runs, not four"
    );

    let answer = concluded(&plane, Art::Freigabe, "e1").await;
    assert_eq!(answer.status, "succeeded", "{answer:?}");
    let vorschlag = answer.vorschlag.expect("a proposal");
    assert_eq!(vorschlag.aktion, "freigeben");
    // The fake model cites a read the run never made, so the Begründung is
    // checked like any sentence and comes back unbelegt.
    assert!(!vorschlag.belegt, "{vorschlag:?}");

    // A Freigabe no tick saw has none, and asking admits nothing.
    assert!(
        plane
            .vorschlag_of(Art::Freigabe, "e3", "erika")
            .await
            .expect("readable")
            .is_none()
    );
    let third = agentd::plane::vorschlag::tick(&plane, Art::Freigabe, processd.as_ref())
        .await
        .expect("the tick reads the queue");
    assert_eq!(third, 0);

    // The Freigaben have concluded; the dispute's run is the next one asked.
    concluded(&plane, Art::Freigabe, "e2").await;
    provider.will_answer(proposal(json!({
        "aktion": "aufrechterhalten",
        "antwortcode": null,
        "befund": "TariffDeviation",
        "buchungsdatum": null,
        "grund": null,
        "begruendung": "Der Preis weicht vom Preisblatt ab.",
        "quelle": "invoicd/get_check_result"
    })));
    let disputes = agentd::plane::vorschlag::tick(&plane, Art::Disput, processd.as_ref())
        .await
        .expect("the tick reads the disputes");
    let again = agentd::plane::vorschlag::tick(&plane, Art::Disput, processd.as_ref())
        .await
        .expect("the tick reads the disputes");
    assert_eq!((disputes, again), (1, 0), "one dispute is one run");
    let answer = concluded(&plane, Art::Disput, "d1").await;
    let vorschlag = answer.vorschlag.expect("a proposal for the dispute");
    assert_eq!(vorschlag.aktion, "aufrechterhalten");
    assert_eq!(vorschlag.befund.as_deref(), Some("TariffDeviation"));
    // The run read no findings, so the finding is not one a read names.
    assert!(!vorschlag.belegt, "{vorschlag:?}");

    // A Freigabe's run is not a dispute's, though they share the store.
    assert!(
        plane
            .vorschlag_of(Art::Disput, "e1", "erika")
            .await
            .expect("readable")
            .is_none()
    );

    // A parked booking is the third kind: one run per key, read back by it.
    provider.will_answer(Completion {
        structured: Some(json!({
            "saetze": [{ "text": "Der Januar ist festgeschrieben.", "quelle": "accountingd/get_parked_booking" }],
            "vorschlag": {
                "aktion": "neu_buchen",
                "antwortcode": null,
                "befund": null,
                "buchungsdatum": "2026-02-01",
                "grund": null,
                "begruendung": "Der erste offene Tag ist der 2026-02-01.",
                "quelle": "accountingd/get_parked_booking"
            },
        })),
        ..proposal(Value::Null)
    });
    let buchungen = agentd::plane::vorschlag::tick(&plane, Art::Buchung, processd.as_ref())
        .await
        .expect("the tick reads the parked bookings");
    let again = agentd::plane::vorschlag::tick(&plane, Art::Buchung, processd.as_ref())
        .await
        .expect("the tick reads the parked bookings");
    assert_eq!((buchungen, again), (1, 0), "one parked booking is one run");
    let answer = concluded(&plane, Art::Buchung, "rechnung:RE-1").await;
    let vorschlag = answer.vorschlag.expect("a proposal for the parked booking");
    assert_eq!(vorschlag.aktion, "neu_buchen");
    assert_eq!(vorschlag.buchungsdatum.as_deref(), Some("2026-02-01"));
}

/// The proposal run of one entry, once it has concluded.
async fn concluded(plane: &Plane, art: Art, id: &str) -> agentd::plane::Answer {
    let mut answer = None;
    for _ in 0..200 {
        answer = plane
            .vorschlag_of(art, id, "erika")
            .await
            .expect("readable");
        if answer.as_ref().is_some_and(|a| a.status != "running") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    answer.unwrap_or_else(|| panic!("{id} has its proposal run"))
}
