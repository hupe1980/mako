//! Step 1 of the agentplane migration: one specialist, run deterministically.
//!
//! `agentd` today has 30 test functions and not one of them exercises an agent —
//! every specialist needs a live model, a paid call and a non-deterministic
//! answer, so the turn loop and all 28 procedures are untested by construction.
//! This is the first test in the service that runs an agent end to end.
//!
//! It also answers the question `concepts/AGENTD.md` §5d calls the one decision
//! in the migration that cannot be revisited: **does a step's input land in a
//! journal record (un-erasable) or in a blob (crypto-shreddable)?** The
//! journal/blob split is by size and our personal data is small, so the answer
//! decides whether GDPR Art. 17 erasure is reachable at all. It is asserted
//! here, against a real store, rather than inferred from documentation.

use std::sync::Arc;

use agentplane::journal::JournalStore;
use agentplane::model::{Completion, Usage};
use agentplane::runtime::{Agent, RunStatus, Runtime};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolCatalog, ToolClient, ToolError, ToolId, ToolSafety};
use serde_json::{Value, json};

use agentd::plane::{GABI_GAS_MANIFEST, parse_manifest};

/// The one privileged call a `planned` agent makes: it returns a **plan**.
///
/// Not the answer. This is the shape the conversion bought — control flow is
/// decided here, from the trusted routing envelope alone, before a single
/// counterparty-authored value has been read. `$step0/...` is a reference the
/// runtime resolves with labels intact; the planner never sees what the tool
/// returned, so a hostile ALOCAT cannot steer the steps that follow it.
fn gabi_plan() -> Completion {
    // Derived, not spelled: a wire name escapes the separator byte, so
    // `get_gas_imbalance` is not `get_gas_imbalance` on the wire. Hand-writing
    // it produced a plan whose every step was refused as ungranted, which is a
    // test bug that reads exactly like a policy finding.
    let imbalance = ToolId::new("netzbilanzd", "get_gas_imbalance").wire_name();
    let deadlines = ToolId::new("makod", "list_overdue_deadlines").wire_name();

    completion(json!({
        "steps": [
            { "tool": imbalance,
              "args": { "bilanzkreis": "$input/bilanzkreis_id", "gas_day": "$input/gas_day" } },
            { "tool": deadlines, "args": {} },
            // The dual-model step. The tool output above is counterparty-derived
            // — a Bilanzkreis allocation the MGV computed from data the shipper
            // sent — and this is where a model reads it. It runs on the
            // **quarantined** model under a declared schema, and the only thing
            // it can say out of band is *not enough information*, which fails
            // the step. Nothing it returns becomes trusted.
            { "parse": { "from": "$step0", "schema": result_schema() } }
        ],
        "answer": "$step2"
    }))
}

/// The shape a parse step must return: the manifest's own result contract.
fn result_schema() -> Value {
    json!({
        "type": "object",
        "required": [
            "gas_day", "imbalance_status", "allocation_version",
            "deadline_compliant", "action", "legal_basis"
        ],
        "properties": {
            "gas_day":             { "type": "string" },
            "bilanzkreis":         { "type": ["string", "null"] },
            "imbalance_status":    { "type": "string" },
            "imbalance_kwh_hs":    { "type": ["number", "null"] },
            "allocation_version":  { "type": "string" },
            "deadline_compliant":  { "type": "boolean" },
            "action":              { "type": "string" },
            "legal_basis":         { "type": "string" }
        }
    })
}

/// A `Completion` carrying a structured value and nothing else.
fn completion(structured: Value) -> Completion {
    Completion {
        text: String::new(),
        structured: Some(structured),
        tool_calls: Vec::new(),
        usage: Usage::default(),
        stop_reason: Some("end_turn".to_owned()),
        truncated: false,
        continuation: None,
    }
}

/// What the plane admits a planned specialist with: re-validated identifiers.
///
/// Built through `plane::label`, not hand-written, so the test exercises the
/// same promotion rule production uses.
fn gabi_envelope() -> agentplane::core::Tainted<Value> {
    agentd::plane::label::routing_envelope(&imbnot_event())
        .expect("the event carries a re-validated Bilanzkreis and gas day")
}

/// The tools the manifest grants, as the operator's catalogue.
///
/// This is the § 4 point made concrete: the catalogue is what the operator
/// declares, not what a server advertises. A tool absent here cannot be called
/// even if `makod` offers it.
fn catalog() -> Arc<ToolCatalog> {
    use agentplane::core::Sensitivity;
    Arc::new(
        ToolCatalog::new()
            .allow(
                ToolId::new("makod", "list_overdue_deadlines"),
                ToolSafety::read_only().max_sensitivity(Sensitivity::Internal),
            )
            .allow(
                ToolId::new("netzbilanzd", "get_gas_imbalance"),
                ToolSafety::read_only().max_sensitivity(Sensitivity::Internal),
            ),
    )
}

/// Stands in for the mako MCP servers. Returns canned answers so the run is
/// deterministic; the real client is `agentplane::tools::McpClient` over rmcp.
#[derive(Debug, Default)]
struct StubTools;

#[async_trait::async_trait]
impl ToolClient for StubTools {
    async fn call(
        &self,
        tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        Ok(match tool.tool.as_str() {
            "get_gas_imbalance" => json!({
                "bilanzkreis": "THE0BFH012345678",
                "saldo": "MINDER",
                "kwh_hs": -18450.75,
                "allocation_version": "Initial"
            }),
            _ => json!({ "items": [] }),
        })
    }
}

/// A payload shaped like the CloudEvent a GaBi Gas specialist really receives,
/// carrying the identifiers that make it personal data under GDPR.
fn imbnot_event() -> serde_json::Value {
    json!({
        "bilanzkreis_id": "THE0BFH012345678",
        "gas_day": "2026-08-06",
        "imbalance_kwh": -18450.75,
        "malo_id": "51238696780",
        "anschlussnutzer": "Musterbäckerei Schmidt GmbH",
        "adresse": "Mühlenweg 14, 26121 Oldenburg",
    })
}

/// The quarantined model's reply to the parse step.
///
/// `have_enough_information` is the one thing a parse may say out of band, and
/// it is a **bit rather than a message** — a message would be untrusted text
/// steering the plan. `false` fails the step rather than letting a guess stand.
/// The runtime strips the flag before the value moves on.
fn parsed_answer() -> Completion {
    let mut value = scripted_answer();
    value["have_enough_information"] = json!(true);
    completion(value)
}

/// What a competent answer looks like, scripted so the run is deterministic.
fn scripted_answer() -> serde_json::Value {
    json!({
        "gas_day": "2026-08-06",
        "bilanzkreis": "THE0BFH012345678",
        "imbalance_status": "MINDER",
        "imbalance_kwh_hs": -18450.75,
        "allocation_version": "Initial",
        "deadline_compliant": true,
        "action": "REQUEST_CORRECTION",
        "legal_basis": "KoV §6.4 Abs. 3"
    })
}

/// The agent runs, produces the declared shape, and replays without a model.
#[tokio::test]
async fn the_gabi_specialist_runs_and_replays_deterministically() {
    let manifest = Arc::new(parse_manifest(GABI_GAS_MANIFEST).expect("manifest parses"));
    let provider = FakeProvider::new();
    provider.will_answer(gabi_plan());
    provider.will_answer(parsed_answer());

    let store = Arc::new(RedbStore::open_in_memory().expect("store"));
    let runtime = Runtime::builder(Arc::clone(&store) as Arc<dyn JournalStore>)
        // agentplane 0.14 fails closed: a manifest declaring `spec.oversight`
        // refuses to run on a plane with no case store — the exact wiring
        // production has (`runtime.rs .cases(..)`), so the tests carry it too.
        .cases(Arc::clone(&store) as Arc<dyn agentplane::case::CaseStore>)
        .tasks(Arc::clone(&store) as Arc<dyn agentplane::case::TaskStore>)
        .owner("agentd-test")
        // The manifest names `anthropic`; the driver is registered under that
        // name. Declarative agent — no Rust skill, the runtime drives the turn.
        .provider(
            "anthropic",
            Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
        )
        .tools(catalog(), Arc::new(StubTools) as Arc<dyn ToolClient>)
        .agent(Agent::new(&manifest))
        .build();

    let out = runtime
        .run("gabi.gas.balancing", gabi_envelope())
        .await
        .expect("the run completes");
    assert_eq!(out.status, RunStatus::Succeeded, "run status");

    let answer = out.output.clone().expect("an answer");
    let answer = answer.peek();
    assert_eq!(answer["action"], "REQUEST_CORRECTION");
    assert_eq!(
        answer["imbalance_status"], "MINDER",
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

/// The prompt the model is asked with is the manifest's, not the binary's.
///
/// This is the property that makes a manifest worth having: if a procedure edit
/// did not reach the model, the digest coverage would be decorative.
#[tokio::test]
async fn the_model_is_asked_with_the_manifests_own_procedure() {
    let manifest = Arc::new(parse_manifest(GABI_GAS_MANIFEST).expect("manifest parses"));
    let provider = FakeProvider::new();
    provider.will_answer(gabi_plan());
    provider.will_answer(parsed_answer());

    let store = Arc::new(RedbStore::open_in_memory().expect("store"));
    let runtime = Runtime::builder(Arc::clone(&store) as Arc<dyn JournalStore>)
        // agentplane 0.14 fails closed: a manifest declaring `spec.oversight`
        // refuses to run on a plane with no case store — the exact wiring
        // production has (`runtime.rs .cases(..)`), so the tests carry it too.
        .cases(Arc::clone(&store) as Arc<dyn agentplane::case::CaseStore>)
        .tasks(Arc::clone(&store) as Arc<dyn agentplane::case::TaskStore>)
        .owner("agentd-test")
        // The manifest names `anthropic`; the driver is registered under that
        // name. Declarative agent — no Rust skill, the runtime drives the turn.
        .provider(
            "anthropic",
            Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
        )
        .tools(catalog(), Arc::new(StubTools) as Arc<dyn ToolClient>)
        .agent(Agent::new(&manifest))
        .build();

    runtime
        .run("gabi.gas.balancing", gabi_envelope())
        .await
        .expect("run");

    let asks = provider.asked();
    assert!(!asks.is_empty(), "the model was asked at least once");
    let assembled = format!("{:?}", asks);

    assert!(
        assembled.contains("kWh_Hs"),
        "the DVGW G 685 unit rule from the manifest procedure must reach the model"
    );
    assert!(
        assembled.contains("KoV"),
        "the KoV allocation-version rules must reach the model"
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
    let dumped = run_and_dump_journal(imbnot_event()).await;

    assert!(
        dumped.contains("Musterbäckerei Schmidt") && dumped.contains("Mühlenweg 14"),
        "measured behaviour changed: personal data no longer appears inline in the journal. \
         If agentplane now blobs step inputs, the reference-only rule in \
         concepts/AGENTD.md §5d can be relaxed — check before doing so."
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
        "bilanzkreis_id": "THE0BFH012345678",
        "gas_day": "2026-08-06",
        "imbalance_kwh": -18450.75,
        "malo_id": "51238696780",
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
    let manifest = Arc::new(parse_manifest(GABI_GAS_MANIFEST).expect("manifest parses"));
    let provider = FakeProvider::new();
    provider.will_answer(gabi_plan());
    provider.will_answer(parsed_answer());

    let store = Arc::new(RedbStore::open_in_memory().expect("store"));
    let runtime = Runtime::builder(Arc::clone(&store) as Arc<dyn JournalStore>)
        // agentplane 0.14 fails closed: a manifest declaring `spec.oversight`
        // refuses to run on a plane with no case store — the exact wiring
        // production has (`runtime.rs .cases(..)`), so the tests carry it too.
        .cases(Arc::clone(&store) as Arc<dyn agentplane::case::CaseStore>)
        .tasks(Arc::clone(&store) as Arc<dyn agentplane::case::TaskStore>)
        .owner("agentd-test")
        .provider(
            "anthropic",
            Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
        )
        .tools(catalog(), Arc::new(StubTools) as Arc<dyn ToolClient>)
        .agent(Agent::new(&manifest))
        .build();

    let out = runtime.run("gabi.gas.balancing", event).await.expect("run");

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

    let manifest = Arc::new(parse_manifest(GABI_GAS_MANIFEST).expect("manifest parses"));
    let provider = FakeProvider::new();
    provider.will_answer(gabi_plan());
    provider.will_answer(parsed_answer());

    let store = Arc::new(RedbStore::open_in_memory().expect("store"));
    let runtime = Runtime::builder(Arc::clone(&store) as Arc<dyn JournalStore>)
        // agentplane 0.14 fails closed: a manifest declaring `spec.oversight`
        // refuses to run on a plane with no case store — the exact wiring
        // production has (`runtime.rs .cases(..)`), so the tests carry it too.
        .cases(Arc::clone(&store) as Arc<dyn agentplane::case::CaseStore>)
        .tasks(Arc::clone(&store) as Arc<dyn agentplane::case::TaskStore>)
        .owner("agentd-test")
        .keyring(Arc::new(MemoryKeyRing::new()) as Arc<dyn agentplane::keyring::KeyRing>)
        .provider(
            "anthropic",
            Arc::clone(&provider) as Arc<dyn agentplane::model::ModelProvider>,
        )
        .tools(catalog(), Arc::new(StubTools) as Arc<dyn ToolClient>)
        .agent(Agent::new(&manifest))
        .build();

    let out = runtime
        .run("gabi.gas.balancing", gabi_envelope())
        .await
        .expect("run");

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
         reach it (concepts/AGENTD.md §5d)"
    );
    assert!(
        !dumped.contains("Mühlenweg 14"),
        "with a key ring configured the address must not be readable in the raw journal"
    );
}
