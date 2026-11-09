//! The Desk's question, end to end on the plane: a person's run is admitted,
//! the model calls a read tool and answers in cited sentences, and the answer
//! read marks each sentence by whether the cited call's output carries its
//! figures — a figure the model worked out itself is `belegt: false`.

use std::sync::Arc;

use agentplane::model::{Completion, ToolCall, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Stores};

/// makod's `get_process` for one MaLo, with one pending Frist.
#[derive(Debug, Default)]
struct Makod;

#[async_trait::async_trait]
impl ToolClient for Makod {
    fn destination(&self, _tool: &ToolId) -> agentplane::tools::Destination {
        agentplane::tools::Destination::Local
    }

    async fn call(
        &self,
        _tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        Ok(json!({
            "business_key": "51238696012",
            "processes": [{
                "workflow_name": "gpke-supplier-change",
                "deadlines": [{ "label": "gpke-antwortfrist", "due_at": "2026-11-05T10:00:00Z" }]
            }]
        }))
    }
}

fn hilfreich(hilfreich: bool, begruendung: Option<&str>) -> agentd::plane::Urteil {
    agentd::plane::Urteil::Hilfreich {
        hilfreich,
        begruendung: begruendung.map(str::to_owned),
    }
}

fn turn(text: &str, structured: Option<Value>, tool_calls: Vec<ToolCall>) -> Completion {
    Completion {
        text: text.to_owned(),
        model: None,
        structured,
        stop_reason: Some(
            if tool_calls.is_empty() {
                "end_turn"
            } else {
                "tool_use"
            }
            .to_owned(),
        ),
        tool_calls,
        usage: Usage::default(),
        truncated: false,
        continuation: None,
        ..Completion::default()
    }
}

#[tokio::test]
async fn a_question_is_answered_in_sentences_each_checked_against_its_call() {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let tools: Arc<dyn ToolClient> = Arc::new(Makod);
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
            owner: "agentd-desk-answer",
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

    provider.will_answer(turn(
        "",
        None,
        vec![ToolCall {
            id: "call-1".to_owned(),
            name: "makod__get_process".to_owned(),
            arguments: json!({ "business_key": "51238696012" }),
        }],
    ));
    provider.will_answer(turn(
        "",
        Some(json!({ "saetze": [
            {
                "text": "Für 51238696012 läuft eine Anmeldung; die Frist endet am 05.11.2026 um 11:00 Uhr.",
                "quelle": "makod/get_process"
            },
            { "text": "Das sind noch 3 Tage.", "quelle": "makod/get_process" },
            { "text": "Mehr liefern die Dienste nicht.", "quelle": null }
        ], "vorschlag": {
            "aktion": "ablehnen",
            "antwortcode": "A02",
            "befund": null,
            "buchungsdatum": null,
            "grund": null,
            "begruendung": "Die Frist endet am 05.11.2026 um 11:00 Uhr.",
            "quelle": "makod/get_process"
        }})),
        Vec::new(),
    ));

    let accepted = plane
        .accept_named(
            "desk-assistant",
            Envelope {
                id: "q-1",
                source: "urn:mako:test:desk",
                event_type: "manual",
            },
            json!({ "frage": "Wie steht die Anmeldung?", "malo": "51238696012" }),
        )
        .await
        .expect("activated");
    assert!(accepted.refused.is_none(), "{:?}", accepted.refused);

    // Admission returned at once; the run works in the background.
    let mut answer = plane
        .answer(&accepted.run_id, "erika")
        .await
        .expect("readable")
        .expect("known");
    for _ in 0..200 {
        if answer.status != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        answer = plane
            .answer(&accepted.run_id, "erika")
            .await
            .expect("readable")
            .expect("known");
    }
    assert_eq!(answer.status, "succeeded", "{answer:?}");
    assert_eq!(answer.disclosure, "AI-generated.");
    let marks: Vec<(bool, Option<&str>)> = answer
        .saetze
        .iter()
        .map(|s| (s.belegt, s.quelle.as_deref()))
        .collect();
    assert_eq!(
        marks,
        vec![
            (true, Some("makod/get_process")),
            (false, Some("makod/get_process")),
            (false, None),
        ],
        "{answer:#?}"
    );

    // The proposal arrives as the model stated it, its Begründung checked like
    // any sentence.
    let vorschlag = answer.vorschlag.as_ref().expect("a proposal");
    assert_eq!(
        (vorschlag.aktion.as_str(), vorschlag.antwortcode.as_deref()),
        ("ablehnen", Some("A02"))
    );
    assert!(vorschlag.belegt, "{vorschlag:?}");

    // The reads the run made, in order, each settled.
    let reads: Vec<(&str, bool)> = answer
        .schritte
        .iter()
        .map(|s| (s.quelle.as_str(), s.fertig))
        .collect();
    assert_eq!(reads, vec![("makod/get_process", true)], "{answer:#?}");

    // Each reader's verdict is their own, and a later one replaces it.
    assert_eq!(answer.hilfreich, None, "no verdict given yet");
    plane
        .bewerten(
            &accepted.run_id,
            "erika",
            hilfreich(false, Some("Frist fehlt")),
        )
        .await
        .expect("a concluded run is assessable");
    plane
        .bewerten(&accepted.run_id, "erika", hilfreich(true, None))
        .await
        .expect("a second verdict replaces the first");
    plane
        .bewerten(&accepted.run_id, "jan", hilfreich(false, None))
        .await
        .expect("another reader judges on their own");
    let read = |who: &'static str| {
        let plane = &plane;
        let run = accepted.run_id.clone();
        async move {
            plane
                .answer(&run, who)
                .await
                .expect("readable")
                .expect("known")
                .hilfreich
        }
    };
    assert_eq!(read("erika").await, Some(true));
    assert_eq!(read("jan").await, Some(false));
    assert_eq!(read("lena").await, None);

    // What became of a proposal is its own metric: recording it leaves the
    // reader's „hilfreich" standing.
    plane
        .bewerten(
            &accepted.run_id,
            "erika",
            agentd::plane::Urteil::Vorschlag(agentd::plane::VorschlagSchicksal::Edited),
        )
        .await
        .expect("a proposal's fate is assessable");
    assert_eq!(read("erika").await, Some(true));

    // The catalog's record is agentplane's, to the count: one run, each
    // reader's standing verdict, and the proposal's fate.
    let bilanz = plane
        .bilanz(
            "desk-assistant",
            time::OffsetDateTime::now_utc() + time::Duration::seconds(1),
        )
        .await
        .expect("an activated skill has a record");
    assert_eq!(bilanz.tage, agentd::plane::BILANZ_TAGE);
    assert_eq!(bilanz.laeufe, 1);
    assert_eq!(
        bilanz.hilfreich,
        agentd::plane::Urteile { ja: 1, nein: 1 },
        "erika's later verdict replaced her first"
    );
    assert_eq!(bilanz.vorschlag.get("edited"), Some(&1));
    assert!(
        plane
            .bilanz("no-such-skill", time::OffsetDateTime::now_utc())
            .await
            .is_none(),
        "a skill not activated here has no record"
    );

    assert!(
        plane
            .answer("not-a-run", "erika")
            .await
            .expect("readable")
            .is_none()
    );
    assert!(matches!(
        plane
            .bewerten("not-a-run", "erika", hilfreich(true, None))
            .await,
        Err(agentd::plane::BewertungError::NichtBewertbar(_))
    ));
}

/// A passage index with § 41f Abs. 1 and Abs. 5 EnWG, as `cargo xtask
/// index-sources` writes it.
fn quellen() -> agentd::plane::quellen::Quellen {
    let index = json!({ "dokumente": [{
        "doc": "EnWG", "edition": "Stand 2026-10-22", "datei": "gesetze/enwg_2005.xml",
        "sha256": "03b376d9ceb7", "stellen": [
            { "at": "§ 41f Abs. 1", "text": "(1) Bei der Nichterfüllung einer Zahlungsverpflichtung eines Haushaltskunden trotz Mahnung …" },
            { "at": "§ 41f Abs. 5", "text": "(5) Der Beginn der Unterbrechung der Energieversorgung ist dem Haushaltskunden acht Werktage im Voraus durch briefliche Mitteilung anzukündigen." }
        ] }] });
    agentd::plane::quellen::Quellen::new(
        agentd::plane::quellen::Quellenindex::from_slice(index.to_string().as_bytes())
            .expect("an index"),
    )
}

/// A sentence naming a § is belegt only beside the quote of that very
/// address: the model quoted Abs. 1, stated Abs. 5, and the sentence is
/// „nicht belegt"; the one stating Abs. 1 from its quote is belegt; and an
/// address the index lacks is refused, so no quote exists to carry it.
#[tokio::test]
async fn a_rule_is_belegt_only_beside_the_quote_of_its_own_address() {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let stub: Arc<dyn ToolClient> = Arc::new(Makod);
    let eigene: Arc<dyn ToolClient> = Arc::new(quellen());
    let servers = agentd::plane::tools::servers_named_in_grants()
        .into_iter()
        .map(|name| {
            let client = if name == agentd::plane::quellen::SERVER {
                Arc::clone(&eigene)
            } else {
                Arc::clone(&stub)
            };
            (name, client)
        })
        .collect();
    let provider = FakeProvider::new();
    let plane = Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-desk-quote",
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

    provider.will_answer(turn(
        "",
        None,
        vec![
            ToolCall {
                id: "call-1".to_owned(),
                name: "agentd__quote_source".to_owned(),
                arguments: json!({ "doc": "EnWG", "edition": null, "at": "§ 41f Abs. 1" }),
            },
            ToolCall {
                id: "call-2".to_owned(),
                name: "agentd__quote_source".to_owned(),
                arguments: json!({ "doc": "EnWG", "edition": null, "at": "§ 41f Abs. 6" }),
            },
        ],
    ));
    provider.will_answer(turn(
        "",
        Some(json!({ "saetze": [
            {
                "text": "Die Unterbrechung ist acht Werktage im Voraus anzukündigen (§ 41f Abs. 5 EnWG).",
                "quelle": "agentd/quote_source"
            },
            {
                "text": "Bei Nichterfüllung trotz Mahnung gilt § 41f Abs. 1 EnWG.",
                "quelle": "agentd/quote_source"
            },
            {
                "text": "§ 41f Abs. 6 EnWG regelt die Unterbrechungsandrohung.",
                "quelle": "agentd/quote_source"
            }
        ], "vorschlag": null })),
        Vec::new(),
    ));

    let accepted = plane
        .accept_named(
            "desk-assistant",
            Envelope {
                id: "q-quote",
                source: "urn:mako:test:desk",
                event_type: "manual",
            },
            json!({ "frage": "Wann ist eine Sperrung anzukündigen?", "malo": "51238696012" }),
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
    let answer = answer.expect("the run concludes");
    assert_eq!(answer.status, "succeeded", "{answer:?}");
    let marks: Vec<bool> = answer.saetze.iter().map(|s| s.belegt).collect();
    assert_eq!(
        marks,
        vec![false, true, false],
        "Abs. 5 against a quote of Abs. 1, Abs. 1 against its own, Abs. 6 refused: {answer:#?}"
    );
}
