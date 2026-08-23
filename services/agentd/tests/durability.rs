//! **A store fault must never duplicate an effect.**
//!
//! The missing half of the durability claim: the regulatory suite proves an
//! *in-doubt tool answer* is never blindly resent; this file proves the same
//! discipline against the **journal itself**. `Fault::CommittedThenLost` is
//! the write that "fails" while the record is durably present — unreachable by
//! truncating a store, because a truncation is a clean cut and this is not.
//! A runtime that reacts by retrying the append writes the record twice, and
//! the chain then carries two entries claiming the same position in history;
//! one that re-runs the effect performs a real call a second time.
//!
//! Deliberately, this file does **not** assert which terminal state the run
//! reaches. Failing, quarantining, or reconciling the ambiguous append and
//! completing are all defensible recoveries, and pinning one would turn an
//! upstream improvement into a red suite. What is *not* defensible under any
//! recovery — and what this file pins — is duplication: the model asked twice,
//! the tool called twice, or the journal recording a second attempt.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agentplane::model::{Completion, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::{FakeProvider, Fault, Faulty, Schedule};
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Stores};

/// One inbound event's identity, as `POST /webhook` would have read it off the
/// CloudEvent envelope.
///
/// `source` is the producer half of the admission key: an id is unique only
/// within one emitter, so the key `run_correlated_once` claims carries both.
fn ce<'a>(id: &'a str, event_type: &'a str) -> Envelope<'a> {
    Envelope {
        id,
        source: "urn:mako:test:tenant:9900357000004",
        event_type,
    }
}

const AGENT: &str = "gabi-gas-agent";
const EVENT_TYPE: &str = "de.gabi.imbalance.notified";

#[derive(Debug, Default)]
struct CountingTools {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ToolClient for CountingTools {
    async fn call(
        &self,
        _tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({ "items": [] }))
    }
}

/// The production wiring, with the journal wrapped in a fault schedule.
///
/// Everything else — cases, tasks, timers, events, push, memory — reads and
/// writes the store directly: the schedule covers `append`, which is where the
/// world and the caller can come to disagree.
fn faulty_plane(
    provider: &Arc<FakeProvider>,
    tools: &Arc<CountingTools>,
    schedule: Schedule,
) -> (Plane, Arc<Faulty>) {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let store = Arc::new(
        RedbStore::open_in_memory()
            .expect("store")
            .for_tenant(tenant.clone()),
    );
    let faulty = Arc::new(Faulty::new(
        Arc::clone(&store) as Arc<dyn agentplane::journal::JournalStore>,
        schedule,
    ));
    let stores = Stores {
        journal: Arc::clone(&faulty) as Arc<dyn agentplane::journal::JournalStore>,
        cases: Arc::clone(&store) as _,
        tasks: Arc::clone(&store) as _,
        timers: Arc::clone(&store) as _,
        events: Arc::clone(&store) as _,
        push: Arc::clone(&store) as _,
        quotas: Arc::clone(&store) as _,
        memory: store as _,
    };
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let servers = ["makod", "marktd", "netzbilanzd"]
        .into_iter()
        .map(|name| (name.to_owned(), Arc::clone(tools) as Arc<dyn ToolClient>))
        .collect();

    let plane = Plane::new(
        stores,
        PlaneConfig {
            outbox: None,
            // Unattested and unbounded, which is what a test wants: signing is
            // a deployment's key and a quota is a deployment's number, and
            // inventing either here would test this file's choices rather than
            // the plane's behaviour.
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-durability-test",
            tenant: &tenant,
            activated: &Activation::named(vec![AGENT.to_owned()]),
            providers: vec![(
                "anthropic".to_owned(),
                Arc::clone(provider) as Arc<dyn agentplane::model::ModelProvider>,
            )],
            tool_servers: servers,
            policy,
            keyring: None,
        },
    )
    .expect("the plane assembles");
    (plane, faulty)
}

/// A read-only plan: one granted read, one parse. No approval suspension, so
/// the whole run executes in one dispatch and the fault lands mid-flight.
fn readonly_plan() -> Completion {
    let read = ToolId::new("makod", "list_overdue_deadlines").wire_name();
    completion(json!({
        "steps": [
            { "tool": read, "args": {} },
            { "parse": { "from": "$step0", "schema": result_schema() } }
        ],
        "answer": "$step1"
    }))
}

fn result_schema() -> Value {
    json!({
        "type": "object",
        "required": [
            "gas_day", "imbalance_status", "allocation_version",
            "deadline_compliant", "action", "legal_basis"
        ],
        "properties": {
            "gas_day":            { "type": "string" },
            "imbalance_status":   { "type": "string" },
            "allocation_version": { "type": "string" },
            "deadline_compliant": { "type": "boolean" },
            "action":             { "type": "string" },
            "legal_basis":        { "type": "string" }
        }
    })
}

fn parsed_answer() -> Completion {
    completion(json!({
        "gas_day": "2026-08-06",
        "imbalance_status": "MINDER",
        "allocation_version": "Initial",
        "deadline_compliant": true,
        "action": "NONE",
        "legal_basis": "KoV §6.4",
        "have_enough_information": true
    }))
}

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

fn event() -> Value {
    json!({
        "malo_id": "51238696012",
        "pid": "13013",
        "mp_id": "9900357000004",
        "gas_day": "2026-08-06",
    })
}

/// The wrapper itself is not the experiment: a fault-free schedule behaves
/// exactly like the bare store, or every other assertion in this file is
/// about the harness.
#[tokio::test]
async fn a_quiet_schedule_changes_nothing() {
    let provider = FakeProvider::new();
    provider.will_answer(readonly_plan());
    provider.will_answer(parsed_answer());
    let tools = Arc::new(CountingTools::default());
    let (plane, faulty) = faulty_plane(&provider, &tools, Schedule::seeded(7));

    let decision = plane
        .dispatch_one(AGENT, ce("ce-quiet", EVENT_TYPE), event())
        .await
        .expect("activated");

    assert_eq!(decision.outcome, "completed", "{decision:?}");
    assert!(
        faulty.injected().is_empty(),
        "a quiet schedule injects nothing"
    );
    assert_eq!(tools.calls.load(Ordering::SeqCst), 1);
}

/// An append that commits while the caller sees an error duplicates nothing.
///
/// The fault lands on the first batch carrying an `EffectDone` — the record of
/// the plan model call having succeeded. The record is durably present; the
/// runtime was told it is not. Whatever the runtime concludes, three things
/// must hold, and each catches a different wrong reaction:
///
/// * the model was asked at most twice (plan + parse) — a third ask means the
///   landed call was re-executed;
/// * the tool ran at most once — re-running the plan re-dispatches it;
/// * the journal holds no second attempt of any effect — retrying the append
///   would write two records claiming the same position in history.
#[tokio::test]
async fn a_committed_then_lost_append_duplicates_no_effect() {
    let provider = FakeProvider::new();
    provider.will_answer(readonly_plan());
    provider.will_answer(parsed_answer());
    let tools = Arc::new(CountingTools::default());
    let (plane, faulty) = faulty_plane(
        &provider,
        &tools,
        Schedule::seeded(7).on_kind("EffectDone", Fault::CommittedThenLost),
    );

    let decision = plane
        .dispatch_one(AGENT, ce("ce-lost", EVENT_TYPE), event())
        .await
        .expect("activated");

    // The experiment happened. A fault-injection test that injected nothing
    // passes for the wrong reason, invisibly.
    assert!(
        faulty
            .injected()
            .iter()
            .any(|(_, f)| matches!(f, Fault::CommittedThenLost)),
        "the schedule never fired — the test proved nothing"
    );

    assert!(
        provider.calls() <= 2,
        "the landed model call was re-executed after its record was already \
         durable: {} calls",
        provider.calls()
    );
    assert!(
        tools.calls.load(Ordering::SeqCst) <= 1,
        "re-running the plan re-dispatched the tool call"
    );

    // The journal agrees: no effect has a second attempt.
    let runtime = plane.runtime();
    for run in faulty.runs() {
        let records = runtime.journal().read(run, 0).await.expect("records");
        let repeated: Vec<_> = records
            .iter()
            .filter_map(|r| match r.kind() {
                agentplane::journal::RecordKind::EffectStarted { attempt, .. } if *attempt > 1 => {
                    Some(*attempt)
                }
                _ => None,
            })
            .collect();
        assert!(
            repeated.is_empty(),
            "run {run}: the journal records a repeated effect attempt {repeated:?} — \
             an ambiguous append must never become a second execution"
        );
    }

    // Whatever the recovery, it is a *named* state — never a silent success
    // that hides an unaccounted append error.
    assert!(
        ["completed", "failed", "quarantined"].contains(&decision.outcome.as_str()),
        "unexpected terminal state: {decision:?}"
    );
}
