//! The Prüfvermerk: every proposal on a Freigabe is held against processd's
//! decision route and makod's rehearsal before a person reads it, the verdict
//! is stored on the run once, and a service that does not answer never reads
//! as a pass.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agentplane::model::{Completion, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::pruefung::{PruefUrteil, Stufe};
use agentd::plane::vorschlag::Art;
use agentd::plane::{Activation, Plane, PlaneConfig, Stores};

/// processd and makod as one fake: four open Freigaben, each answered by the
/// decision rehearsal differently, and a makod that rehearses one command.
#[derive(Debug, Default)]
struct Dienste {
    rehearsals: AtomicUsize,
}

const PROBENFAEHIG: &str = "gpke.lieferbeginn.anmelden";

#[async_trait::async_trait]
impl ToolClient for Dienste {
    fn destination(&self, _tool: &ToolId) -> agentplane::tools::Destination {
        agentplane::tools::Destination::Local
    }

    async fn call(
        &self,
        tool: &ToolId,
        arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        let besteht = |command: &str| {
            json!({
                "id": arguments["id"], "freigeben": false, "urteil": "besteht",
                "command": command, "marktrolle": "NB",
                "payload": { "antwort_code": "A01" }, "grund": null,
                "body_sha256": format!("sha-{}", arguments["id"].as_str().unwrap_or_default()),
            })
        };
        Ok(match (tool.server.as_str(), tool.tool.as_str()) {
            ("processd", "list_pending_approvals") => json!({
                "freigaben": [{ "id": "abgelehnt" }, { "id": "ungeprobt" }, { "id": "bestanden" }, { "id": "stumm" }],
                "count": 4,
                "saturated": false,
            }),
            ("processd", "rehearse_decision") => {
                self.rehearsals.fetch_add(1, Ordering::SeqCst);
                match arguments["id"].as_str() {
                    Some("abgelehnt") => json!({
                        "id": "abgelehnt", "freigeben": false, "urteil": "abgelehnt",
                        "command": "gpke.lieferbeginn.ablehnen", "marktrolle": "NB", "payload": null,
                        "grund": {
                            "code": "code_outside_tree",
                            "text": "Antwortcode \"A01\" is not in the Ablehnung-Cluster of E_0622",
                            "ebd": "E_0622",
                        },
                    }),
                    Some("ungeprobt") => besteht("gpke.lieferbeginn.ablehnen"),
                    Some("bestanden") => besteht(PROBENFAEHIG),
                    _ => {
                        return Err(ToolError::Unreachable {
                            tool: tool.clone(),
                            detail: "connection refused (os error 61)".to_owned(),
                        });
                    }
                }
            }
            ("makod", "rehearse_command") => {
                if arguments["command"] == PROBENFAEHIG {
                    json!({ "command": PROBENFAEHIG, "marktrolle": "NB", "urteil": "besteht",
                            "stufe": null, "grund": null, "nachrichten": [] })
                } else {
                    return Err(ToolError::Refused {
                        tool: tool.clone(),
                        detail: format!(
                            "not_rehearsable: {} cannot be rehearsed",
                            arguments["command"]
                        ),
                    });
                }
            }
            _ => json!({ "id": "abgelehnt", "pid": 55001 }),
        })
    }
}

fn proposal() -> Completion {
    Completion {
        text: String::new(),
        model: None,
        structured: Some(json!({
            "saetze": [{ "text": "Die Anmeldung ist eskaliert.", "quelle": "processd/get_queue_entry" }],
            "vorschlag": {
                "aktion": "ablehnen",
                "antwortcode": "A01",
                "befund": null,
                "buchungsdatum": null,
                "grund": null,
                "begruendung": "Die Anmeldung ist eskaliert.",
                "quelle": "processd/get_queue_entry"
            },
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
async fn every_freigabe_proposal_carries_the_gates_verdict_stored_once() {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let dienste = Arc::new(Dienste::default());
    let servers = agentd::plane::tools::servers_named_in_grants()
        .into_iter()
        .map(|name| (name, Arc::clone(&dienste) as Arc<dyn ToolClient>))
        .collect();
    let provider = FakeProvider::new();
    for _ in 0..4 {
        provider.will_answer(proposal());
    }
    let plane = Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-pruefvermerk",
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

    let client: &dyn ToolClient = dienste.as_ref();
    agentd::plane::vorschlag::tick(&plane, Art::Freigabe, client)
        .await
        .expect("the tick reads the queue");
    for id in ["abgelehnt", "ungeprobt", "bestanden", "stumm"] {
        concluded(&plane, id).await;
    }
    // The next tick finds every run concluded and stores each verdict.
    agentd::plane::vorschlag::tick(&plane, Art::Freigabe, client)
        .await
        .expect("the tick reads the queue");
    let after_tick = dienste.rehearsals.load(Ordering::SeqCst);

    let vermerk = |id: &'static str| {
        let plane = &plane;
        async move {
            plane
                .vorschlag_of(Art::Freigabe, id, "erika")
                .await
                .expect("readable")
                .and_then(|a| a.pruefvermerk)
                .unwrap_or_else(|| panic!("{id} carries a Prüfvermerk"))
        }
    };

    // A refusal comes back with its code, its tree and the route's words.
    let v = vermerk("abgelehnt").await;
    assert_eq!(v.urteil, PruefUrteil::Abgelehnt, "{v:?}");
    let refusal = v.ablehnung().expect("the refusing stage");
    assert_eq!(refusal.stufe, Stufe::Entscheidung);
    assert_eq!(refusal.code.as_deref(), Some("code_outside_tree"));
    assert_eq!(refusal.regel.as_deref(), Some("E_0622"));
    assert_eq!(
        v.stufen.len(),
        1,
        "a refused decision is not rehearsed further"
    );

    // processd passes it, makod does not rehearse that door: not checked,
    // with makod's own code — never passed.
    let v = vermerk("ungeprobt").await;
    assert_eq!(v.urteil, PruefUrteil::NichtGeprueft, "{v:?}");
    assert_eq!(v.stufen[1].code.as_deref(), Some("not_rehearsable"));

    // Both stages ran and passed.
    let v = vermerk("bestanden").await;
    assert_eq!(v.urteil, PruefUrteil::Bestanden, "{v:?}");
    assert_eq!(v.stufen.len(), 2);
    // The verdict names the body it was taken for, so an edited body is not
    // shown as checked.
    assert_eq!(v.body_sha256.as_deref(), Some("sha-bestanden"));
    assert!(!v.geprueft_um.is_empty());

    // processd unreachable: not checked, with the reason.
    let v = vermerk("stumm").await;
    assert_eq!(v.urteil, PruefUrteil::NichtGeprueft, "{v:?}");
    assert_eq!(v.stufen[0].code.as_deref(), Some("unreachable"));

    // Stored once — the unreachable one too, so every reader sees what the
    // tick saw: reading the proposals again rehearses nothing.
    assert_eq!(
        dienste.rehearsals.load(Ordering::SeqCst),
        after_tick,
        "a stored Prüfvermerk is read, not recomputed"
    );
}

async fn concluded(plane: &Plane, id: &str) -> agentd::plane::Answer {
    let mut answer = None;
    for _ in 0..200 {
        answer = plane
            .vorschlag_of(Art::Freigabe, id, "erika")
            .await
            .expect("readable");
        if answer.as_ref().is_some_and(|a| a.status != "running") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    answer.unwrap_or_else(|| panic!("{id} has its proposal run"))
}

/// The proposal keeps its trail: the reads the run made (the Spur the Desk
/// already streams, `schritte`) and the gate's verdict come back with it, and
/// the entry still resolves its run after it left processd's open list.
#[tokio::test]
async fn a_decided_entry_still_resolves_its_proposal_with_its_trail_and_verdict() {
    use agentplane::model::ToolCall;

    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let dienste = Arc::new(Dienste::default());
    let servers = agentd::plane::tools::servers_named_in_grants()
        .into_iter()
        .map(|name| (name, Arc::clone(&dienste) as Arc<dyn ToolClient>))
        .collect();
    let provider = FakeProvider::new();
    provider.will_answer(Completion {
        structured: None,
        tool_calls: vec![ToolCall {
            id: "call-1".to_owned(),
            name: "processd__get_queue_entry".to_owned(),
            arguments: json!({ "id": "abgelehnt" }),
        }],
        stop_reason: Some("tool_use".to_owned()),
        ..proposal()
    });
    provider.will_answer(proposal());
    let plane = Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-spur",
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

    plane
        .vorschlag_zulassen(Art::Freigabe, "abgelehnt")
        .await
        .expect("the desk assistant is activated");
    let first = concluded(&plane, "abgelehnt").await;
    assert_eq!(first.status, "succeeded", "{first:?}");

    // Decided and gone from the queue: nothing lists it, and the entry's id
    // still finds the same run, its reads and its verdict.
    let again = plane
        .vorschlag_of(Art::Freigabe, "abgelehnt", "erika")
        .await
        .expect("readable")
        .expect("the decided entry still resolves its run");
    assert_eq!(again.run, first.run);
    let quellen: Vec<&str> = again.schritte.iter().map(|s| s.quelle.as_str()).collect();
    assert_eq!(quellen, ["processd/get_queue_entry"]);
    let vermerk = again
        .pruefvermerk
        .expect("the verdict stays with the proposal");
    assert_eq!(vermerk.urteil, PruefUrteil::Abgelehnt);
}

/// processd after the person decided: every entry `Rejected`, sent with a body
/// other than the one the proposal was checked for.
#[derive(Debug, Default)]
struct Entschieden(Dienste);

#[async_trait::async_trait]
impl ToolClient for Entschieden {
    fn destination(&self, tool: &ToolId) -> agentplane::tools::Destination {
        self.0.destination(tool)
    }

    async fn call(
        &self,
        tool: &ToolId,
        arguments: &Value,
        provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        if (tool.server.as_str(), tool.tool.as_str()) == ("processd", "get_queue_entry") {
            return Ok(json!({
                "id": arguments["id"], "status": "Rejected",
                "gesendet_sha256": "sha-of-an-edited-body",
            }));
        }
        self.0.call(tool, arguments, provenance).await
    }
}

/// A proposal's fate is read off processd's decided entry. The client
/// reports „sent", but the decision went out with another body than the one
/// the proposal was checked for, so the fate recorded is „edited".
#[tokio::test]
async fn a_client_reported_sent_on_an_edited_proposal_records_edited() {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let dienste = Arc::new(Entschieden::default());
    let servers = agentd::plane::tools::servers_named_in_grants()
        .into_iter()
        .map(|name| (name, Arc::clone(&dienste) as Arc<dyn ToolClient>))
        .collect();
    let provider = FakeProvider::new();
    provider.will_answer(proposal());
    let plane = Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-schicksal",
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

    plane
        .vorschlag_zulassen(Art::Freigabe, "bestanden")
        .await
        .expect("the desk assistant is activated");
    let answer = concluded(&plane, "bestanden").await;
    let vermerk = answer.pruefvermerk.expect("checked");
    assert_eq!(vermerk.body_sha256.as_deref(), Some("sha-bestanden"));

    plane
        .bewerten(
            &answer.run,
            "erika",
            agentd::plane::Urteil::Vorschlag(agentd::plane::VorschlagSchicksal::Sent),
        )
        .await
        .expect("a decided Freigabe's proposal is assessable");
    let bilanz = plane
        .bilanz(
            agentd::plane::vorschlag::SKILL,
            time::OffsetDateTime::now_utc() + time::Duration::seconds(1),
        )
        .await
        .expect("an activated skill has a record");
    assert_eq!(
        bilanz.vorschlag.get("edited"),
        Some(&1),
        "{:?}",
        bilanz.vorschlag
    );
    assert_eq!(
        bilanz.vorschlag.get("sent"),
        None,
        "the client's report is not the fate"
    );
}
