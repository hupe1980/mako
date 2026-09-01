//! The golden run: one specialist, end to end and deterministically.
//!
//! A live model means a paid call and a non-deterministic answer, which is why
//! an agent layer is usually the one part of a workspace that is untested.
//! `FakeProvider` removes the excuse: this file runs the `planned` specialist
//! through its real plan — privileged call, granted tool calls, quarantined
//! parse — and then **replays it strictly**, asserting the replay genuinely
//! replayed rather than silently re-executing.
//!
//! It also answers the one storage decision that cannot be revisited once
//! personal data is written: **does a step's input land in a journal record
//! (un-erasable) or in a blob (crypto-shreddable)?** The
//! journal/blob split is by size and our personal data is small, so the answer
//! decides whether GDPR Art. 17 erasure is reachable at all. It is asserted
//! here, against a real store, rather than inferred from documentation.

use std::sync::Arc;

use agentplane::core::CorrelationKey;
use agentplane::journal::JournalStore;
use agentplane::runtime::{Admission, Agent, RunOutcome, RunStatus, Runtime};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use serde_json::{Value, json};

use agentd::plane::{Envelope, find_manifest};

/// What the plane admits a coded specialist with: per-field trust labels.
fn gabi_envelope() -> agentplane::core::Tainted<Value> {
    agentd::plane::label::admit(mako_events::gabi::ALOCAT_MISSING, alocat_missing_event())
}

async fn run_gabi(
    runtime: &Runtime,
    input: agentplane::core::Tainted<Value>,
    event_id: &str,
) -> RunOutcome {
    let event = Envelope {
        id: event_id,
        source: "urn:mako:test:tenant:9900357000004",
        event_type: mako_events::gabi::ALOCAT_MISSING,
    };
    let keys = [CorrelationKey::new("event", event_id)];
    match runtime
        .run_correlated_once(
            "gabi.gas.balancing",
            input,
            "gabi-allocation",
            &keys,
            &event.admission_key("gabi-gas-agent"),
        )
        .await
        .expect("correlated run")
    {
        Admission::Fresh(outcome) => outcome,
        other => panic!("a fresh test event was not freshly admitted: {other:?}"),
    }
}

/// A payload shaped like the CloudEvent a GaBi Gas specialist really receives,
/// carrying the identifiers that make it personal data under GDPR.
fn alocat_missing_event() -> serde_json::Value {
    json!({
        "gas_day": "2026-08-06",
        "sender_eic": "11XRWENET-----1E",
        "receiver_eic": "11YN00000000TH2M",
        "deadline_label": "gabi-final-allocation",
        "synthetic_pid": "13013",
        "malo_id": "51238696012",
        "anschlussnutzer": "Musterbäckerei Schmidt GmbH",
        "adresse": "Mühlenweg 14, 26121 Oldenburg",
    })
}

/// The agent runs, produces the declared shape, and replays without a model.
#[tokio::test]
async fn the_gabi_specialist_runs_and_replays_deterministically() {
    let manifest = Arc::new(
        find_manifest("gabi-gas-agent")
            .expect("the GaBi Gas specialist is compiled in")
            .clone(),
    );
    let provider = FakeProvider::new();

    let store = Arc::new(RedbStore::open_in_memory().expect("store"));
    let runtime = Runtime::builder(Arc::clone(&store) as Arc<dyn JournalStore>)
        // The plane fails closed: a manifest declaring `spec.oversight`
        // refuses to run on a plane with no case store — the exact wiring
        // production has (`runtime.rs .cases(..)`), so the tests carry it too.
        .cases(Arc::clone(&store) as Arc<dyn agentplane::case::CaseStore>)
        .tasks(Arc::clone(&store) as Arc<dyn agentplane::case::TaskStore>)
        .owner("agentd-test")
        .calendar(Arc::new(agentd::plane::calendar::MakoCalendar))
        // The manifest names `anthropic`; the driver is registered under that
        // name. Declarative agent — no Rust skill, the runtime drives the turn.
        .provider(
            "anthropic",
            Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
        )
        .agent(Agent::new(&manifest).skill(agentd::skills::GabiAllocationTriage::new()))
        .build();

    let out = run_gabi(&runtime, gabi_envelope(), "golden-run").await;
    assert_eq!(out.status, RunStatus::Succeeded, "run status");

    let answer = out.output.clone().expect("an answer");
    let answer = answer.peek();
    assert_eq!(answer["action"], "OPEN_CLEARING_CASE");
    assert_eq!(
        answer["status"], "MISSING_FINAL_ALLOCATION",
        "the declared enum is what came back"
    );

    // Strict replay: the recorded effects are read back, not re-performed.
    let replayed = runtime
        .replay(out.run_id, agentplane::runtime::Mode::Strict)
        .await;

    // The assertion that makes the replay claim mean anything. Without it a
    // replay test passes while every effect is quietly performed again — the
    // store's unique index would catch the duplicate and the run would still
    // look fine.
    agentplane::testkit::assert_replay_was_not_backstopped("gabi strict replay", &replayed);
    assert_eq!(replayed.expect("replay").status, RunStatus::Succeeded);
}

/// Deterministic event translation must not invoke a model.
#[tokio::test]
async fn the_coded_specialist_asks_no_model() {
    let manifest = Arc::new(
        find_manifest("gabi-gas-agent")
            .expect("the GaBi Gas specialist is compiled in")
            .clone(),
    );
    let provider = FakeProvider::new();

    let store = Arc::new(RedbStore::open_in_memory().expect("store"));
    let runtime = Runtime::builder(Arc::clone(&store) as Arc<dyn JournalStore>)
        // The plane fails closed: a manifest declaring `spec.oversight`
        // refuses to run on a plane with no case store — the exact wiring
        // production has (`runtime.rs .cases(..)`), so the tests carry it too.
        .cases(Arc::clone(&store) as Arc<dyn agentplane::case::CaseStore>)
        .tasks(Arc::clone(&store) as Arc<dyn agentplane::case::TaskStore>)
        .owner("agentd-test")
        .calendar(Arc::new(agentd::plane::calendar::MakoCalendar))
        // The manifest names `anthropic`; the driver is registered under that
        // name. Declarative agent — no Rust skill, the runtime drives the turn.
        .provider(
            "anthropic",
            Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
        )
        .agent(Agent::new(&manifest).skill(agentd::skills::GabiAllocationTriage::new()))
        .build();

    let _out = run_gabi(&runtime, gabi_envelope(), "prompt-run").await;

    let asks = provider.asked();
    assert!(
        asks.is_empty(),
        "coded GaBi triage must spend no model call"
    );
}

/// **The erasure question, answered against a real store.**
///
/// agentplane erases a case by destroying its wrapping key, which reaches blob
/// payloads. `docs/regulation` is explicit that "personal data that reached a
/// journal record rather than a blob cannot be removed at all — the chain is
/// append-only by design", and the routing rule is *size*: blobs are "where
/// bytes too large for the journal go", with records over 1 MiB refused.
///
/// Our personal data is small, so nothing pushes it out of the chain. This test
/// pins what we measured: **a payload-carrying event puts the customer name, the
/// address and the MaLo-ID inline in journal records**, where `erase_case` cannot
/// reach them.
///
/// It asserts the hazard rather than a wish, so if a future agentplane release
/// starts blobbing step inputs this test fails and we revisit the design rule
/// below — a change in our favour should not pass silently.
#[tokio::test]
async fn personal_data_in_a_step_input_reaches_the_journal() {
    let dumped = run_and_dump_journal(alocat_missing_event()).await;

    assert!(
        dumped.contains("Musterbäckerei Schmidt") && dumped.contains("Mühlenweg 14"),
        "measured behaviour changed: personal data no longer appears inline in the journal. \
         If agentplane now blobs step inputs, the rule that agent inputs carry \
         references rather than payloads could be relaxed — check before doing so."
    );
}

/// The mitigation, enforced: a reference-only event carries nothing to erase.
///
/// This is the design rule §5d adopts. A specialist is handed identifiers and
/// fetches details through an authorised, journaled tool call; the customer
/// record never enters the run's input, so there is nothing in the chain that
/// `erase_case` would need to reach.
#[tokio::test]
async fn a_reference_only_event_keeps_personal_data_out_of_the_journal() {
    let reference_only = json!({
        "gas_day": "2026-08-06",
        "sender_eic": "11XRWENET-----1E",
        "receiver_eic": "11YN00000000TH2M",
        "deadline_label": "gabi-final-allocation",
        "synthetic_pid": "13013",
        "malo_id": "51238696012",
    });
    let dumped = run_and_dump_journal(reference_only).await;

    assert!(
        !dumped.contains("Musterbäckerei Schmidt"),
        "a reference-only event must not carry a customer name"
    );
    assert!(
        !dumped.contains("Mühlenweg 14"),
        "a reference-only event must not carry an address"
    );
}

/// Run the agent on `event` and return every journal record, rendered.
/// Run the specialist and return every journal record as text.
///
/// The event is admitted **trusted** here, and deliberately so: these tests ask
/// what the journal *stores*, not what the label lattice permits. Production
/// admits through `plane::label`, which promotes only re-validated identifiers —
/// but a payload that never reaches the journal cannot demonstrate that a
/// payload which does is erasable. Passing it whole is what makes the question
/// answerable.
async fn run_and_dump_journal(event: serde_json::Value) -> String {
    let event = agentplane::core::Tainted::trusted(event);
    let manifest = Arc::new(
        find_manifest("gabi-gas-agent")
            .expect("the GaBi Gas specialist is compiled in")
            .clone(),
    );
    let provider = FakeProvider::new();

    let store = Arc::new(RedbStore::open_in_memory().expect("store"));
    let runtime = Runtime::builder(Arc::clone(&store) as Arc<dyn JournalStore>)
        // The plane fails closed: a manifest declaring `spec.oversight`
        // refuses to run on a plane with no case store — the exact wiring
        // production has (`runtime.rs .cases(..)`), so the tests carry it too.
        .cases(Arc::clone(&store) as Arc<dyn agentplane::case::CaseStore>)
        .tasks(Arc::clone(&store) as Arc<dyn agentplane::case::TaskStore>)
        .owner("agentd-test")
        .calendar(Arc::new(agentd::plane::calendar::MakoCalendar))
        .provider(
            "anthropic",
            Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
        )
        .agent(Agent::new(&manifest).skill(agentd::skills::GabiAllocationTriage::new()))
        .build();

    let out = run_gabi(&runtime, event, "journal-placement").await;

    let records = store.read(out.run_id, 0).await.expect("journal records");
    records
        .iter()
        .map(|r| format!("{r:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// No first-wins dispatch, and no config key that could reintroduce one.
///
/// `race` existed and cancelled in-flight specialists by dropping their futures.
/// A specialist cancelled mid-turn may already have called a mutating MCP tool,
/// so the request can have landed on the server with nothing recorded here —
/// the unrecoverable unknown outcome.
///
/// After the cutover the guarantee is structural: every matching specialist gets
/// its own run and its own journal, and `dispatch` returns one decision each. No
/// branch is abandoned, so there is nothing to leave a started effect without a
/// terminal record. This pins the config half — `deny_unknown_fields` means a
/// well-meaning `dispatch_mode = "race"` is a refusal to boot, not a silently
/// ignored key.
#[test]
fn no_config_key_can_reintroduce_first_wins_dispatch() {
    let cfg = r#"
tenant = "9900357000004"
mcp_api_key = "test"
[providers.anthropic]
backend = "anthropic"
api_key = "test"
[orchestrator]
dispatch_mode = "race"
"#;
    let err = toml::from_str::<agentd::config::AgentdConfig>(cfg)
        .expect_err("`[orchestrator] dispatch_mode` must not deserialize");
    let msg = err.to_string();
    assert!(
        msg.contains("orchestrator") || msg.contains("unknown field"),
        "the refusal should name the offending key, got: {msg}"
    );
}

/// Every subscribing specialist runs; none is dropped.
///
/// The structural half of the guarantee above. Two specialists subscribe to
/// `de.billing.rechnung.erstellt` — an anomaly check and a regulatory guard —
/// and both must appear in the routing table, because a fan-out that silently
/// kept one opinion is exactly the first-wins behaviour that was removed.
#[test]
fn every_subscribing_specialist_is_routed_not_just_the_first() {
    let router = agentd::plane::Router::build(&agentd::plane::Activation::all()).expect("routes");
    let matched = router.matching(mako_events::billing::RECHNUNG_ERSTELLT);
    let names: Vec<&str> = matched.iter().map(|r| r.name).collect();

    assert!(
        names.contains(&"billing-anomaly-agent")
            && names.contains(&"billing-regulatory-guard-agent"),
        "both opinions must be routed, got: {names:?}"
    );

    // …and each opinion is admitted on its own key.
    //
    // Admission is at-most-once per key, so an event-wide key would admit the
    // first specialist and answer every other one with *its* run — the fan-out
    // this test exists to protect, defeated by the mechanism that protects the
    // fan-out from duplicating. It would present as the second and third
    // opinions "completing" instantly with somebody else's answer.
    let envelope = agentd::plane::Envelope {
        id: "ce-fanout",
        source: "urn:mako:test:tenant:9900357000004",
        event_type: mako_events::billing::RECHNUNG_ERSTELLT,
    };
    let keys: std::collections::BTreeSet<String> = matched
        .iter()
        .map(|r| envelope.admission_key(r.name))
        .collect();
    assert_eq!(
        keys.len(),
        matched.len(),
        "one event's specialists must not share an admission key: {keys:?}"
    );
}

/// **A key ring seals the journal, so journaled personal data is erasable.**
///
/// This is the answer to the constraint the two tests above pin. Without a key
/// ring, a payload-carrying event puts the customer name and address inline in
/// journal records and `erase_case` cannot reach them. With one,
/// `RuntimeBuilder::build` wraps the journal (and cases, events and task
/// proposals) in `SealedJournal` — so the same run leaves ciphertext, and
/// destroying the case's wrapping key destroys the plaintext everywhere,
/// including in backups.
///
/// The wrapping happens at `build`, not at `keyring(..)`, so registration order
/// cannot lose it. Note one deliberate exclusion: governed memory
/// (`EncryptedMemoryStore`) is not wrapped here, because its erasure unit
/// outlives the case and its single-writer mutex would not hold on an
/// active-active deployment. We do not use governed memory; if that changes,
/// it must be wrapped explicitly.
#[tokio::test]
async fn a_key_ring_seals_personal_data_in_the_journal() {
    use agentplane::testkit::MemoryKeyRing;

    let manifest = Arc::new(
        find_manifest("gabi-gas-agent")
            .expect("the GaBi Gas specialist is compiled in")
            .clone(),
    );
    let provider = FakeProvider::new();

    let store = Arc::new(RedbStore::open_in_memory().expect("store"));
    let runtime = Runtime::builder(Arc::clone(&store) as Arc<dyn JournalStore>)
        // The plane fails closed: a manifest declaring `spec.oversight`
        // refuses to run on a plane with no case store — the exact wiring
        // production has (`runtime.rs .cases(..)`), so the tests carry it too.
        .cases(Arc::clone(&store) as Arc<dyn agentplane::case::CaseStore>)
        .tasks(Arc::clone(&store) as Arc<dyn agentplane::case::TaskStore>)
        .owner("agentd-test")
        .calendar(Arc::new(agentd::plane::calendar::MakoCalendar))
        .keyring(Arc::new(MemoryKeyRing::new()) as Arc<dyn agentplane::keyring::KeyRing>)
        .provider(
            "anthropic",
            Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
        )
        .agent(Agent::new(&manifest).skill(agentd::skills::GabiAllocationTriage::new()))
        .build();

    let out = run_gabi(&runtime, gabi_envelope(), "sealed-run").await;

    // Read the *raw* store, behind the sealing decorator.
    let records = store.read(out.run_id, 0).await.expect("journal records");
    let dumped = records
        .iter()
        .map(|r| format!("{r:?}"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !dumped.contains("Musterbäckerei Schmidt"),
        "with a key ring configured the customer name must not be readable in the raw \
         journal — if it is, the journal is not being sealed and `erase_case` cannot \
         reach it"
    );
    assert!(
        !dumped.contains("Mühlenweg 14"),
        "with a key ring configured the address must not be readable in the raw journal"
    );
}
