//! **A `202` must mean *this message will be acted on*.**
//!
//! `POST /webhook` is the end of an at-least-once chain: `marktd`'s fan-out
//! retries until it sees a 2xx and advances its outbox cursor on that answer.
//! What the cursor advances *past* is therefore whatever agentd's 2xx promised.
//!
//! So the acknowledgement must not precede the commit. Any window between the
//! two is one in which a deploy, a SIGTERM or a crash loses the event outright,
//! with no record anywhere that it arrived. `Plane::accept` returns only once
//! admission has committed — the policy gate, the quota reservation, the case
//! binding and the claim on the key, inside the transaction that appends the
//! run's first record.
//!
//! ## How you test "it was durable before it returned"
//!
//! Not by crashing a process. The observable consequence is that **a second
//! delivery of the same event is answered with the first one's run** the instant
//! `accept` returns — which can only be true if the key was claimed before the
//! return. If admission were still in flight, the second call would either race
//! into a second run or find nothing. That is what these tests assert.

use std::sync::Arc;

use agentplane::model::{Completion, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Reception, Stores};

const AGENT: &str = "gabi-gas-agent";
const EVENT_TYPE: &str = "de.gabi.imbalance.notified";
const SOURCE: &str = "urn:mako:test:tenant:9900357000004";

fn ce(id: &str) -> Envelope<'_> {
    Envelope {
        id,
        source: SOURCE,
        event_type: EVENT_TYPE,
    }
}

#[derive(Debug, Default)]
struct Tools;

#[async_trait::async_trait]
impl ToolClient for Tools {
    async fn call(
        &self,
        _tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        Ok(json!({ "items": [] }))
    }
}

fn plane(provider: &Arc<FakeProvider>) -> Plane {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let servers = ["makod", "marktd", "netzbilanzd"]
        .into_iter()
        .map(|name| (name.to_owned(), Arc::new(Tools) as Arc<dyn ToolClient>))
        .collect();

    Plane::new(
        Stores::redb(
            RedbStore::open_in_memory()
                .expect("store")
                .origin(agentd::plane::attest::ORIGIN),
            &tenant,
        ),
        PlaneConfig {
            owner: "agentd-test",
            tenant: &tenant,
            activated: &Activation::named(vec![AGENT.to_owned()]),
            providers: vec![(
                "anthropic".to_owned(),
                Arc::clone(provider) as Arc<dyn agentplane::model::ModelProvider>,
            )],
            tool_servers: servers,
            policy,
            keyring: None,
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
        },
    )
    .expect("the plane assembles")
}

fn event() -> Value {
    json!({
        "malo_id": "51238696012",
        "bilanzkreis_id": "THE0BFH012345678",
        "gas_day": "2026-08-06",
    })
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

/// Enough answers for several runs: the fan-out here is one specialist, but a
/// redelivery test must not fail because the script ran out rather than because
/// a second run started.
fn script(provider: &FakeProvider) {
    let imbalance = ToolId::new("netzbilanzd", "get_gas_imbalance").wire_name();
    let plan = json!({
        "steps": [
            { "tool": imbalance, "args": {
                "bilanzkreis": "$input/bilanzkreis_id",
                "gas_day": "$input/gas_day"
            }},
            { "parse": { "from": "$step0", "schema": {
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
            }}}
        ],
        "answer": "$step1"
    });
    let parsed = json!({
        "gas_day": "2026-08-06",
        "imbalance_status": "MINDER",
        "allocation_version": "Initial",
        "deadline_compliant": true,
        "action": "REQUEST_CORRECTION",
        "legal_basis": "KoV §6.4 Abs. 3",
        "have_enough_information": true
    });
    for _ in 0..4 {
        provider.will_answer(completion(plan.clone()));
        provider.will_answer(completion(parsed.clone()));
    }
}

// ── The contract ──────────────────────────────────────────────────────────

/// Accepting an event yields a run id, and the run exists in the journal.
///
/// The id is the point of returning a body at all: a bare `202` tells the
/// caller the message was taken and gives them nothing to follow.
#[tokio::test]
async fn accepting_an_event_admits_a_run_that_is_already_in_the_journal() {
    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider);

    let accepted = plane.accept(ce("ce-1"), event()).await;
    assert_eq!(accepted.len(), 1, "one subscribing specialist");
    let entry = &accepted[0];
    assert_eq!(entry.agent_name, AGENT);
    assert!(
        entry.fresh,
        "the first delivery is the one that admitted it"
    );
    assert!(entry.refused.is_none(), "{:?}", entry.refused);

    // The run is addressable *now*, not eventually. Reading it back through the
    // same handle the oversight surface uses is what makes "admission committed
    // before the call returned" an observation rather than a claim.
    let run = agentplane::core::RunId::parse(&entry.run_id).expect("a run id");
    assert!(
        plane
            .runtime()
            .case_of(run)
            .await
            .expect("readable")
            .is_some(),
        "an admitted run belongs to a case the moment it is admitted"
    );
}

/// **A redelivery is answered with the original run, immediately.**
///
/// A retry arriving before the key is claimed starts a second fan-out — and, for
/// the one specialist that suspends on a four-eyes decision, puts a second
/// identical Freigabe in front of a reviewer, which is a four-eyes control
/// degrading into a guess.
#[tokio::test]
async fn a_redelivery_is_answered_with_the_run_the_first_one_started() {
    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider);

    let first = plane.accept(ce("ce-retry"), event()).await;
    let second = plane.accept(ce("ce-retry"), event()).await;

    assert!(first[0].fresh, "the first delivery admitted the run");
    assert!(
        !second[0].fresh,
        "the retry must not admit a second run — `fresh` is what tells an operator \
         \"the same event twice\" from \"a retry answered correctly\""
    );
    assert_eq!(
        first[0].run_id, second[0].run_id,
        "the retry is answered with the original run"
    );
}

/// A different event id is a different message, and starts its own run.
///
/// The other half of the check above: an admission key that swallowed distinct
/// events would deduplicate the whole plane down to its first message, and would
/// look identical to correct deduplication from the outside.
#[tokio::test]
async fn two_distinct_events_get_two_runs() {
    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider);

    let a = plane.accept(ce("ce-a"), event()).await;
    let b = plane.accept(ce("ce-b"), event()).await;
    assert!(a[0].fresh && b[0].fresh);
    assert_ne!(
        a[0].run_id, b[0].run_id,
        "distinct events are distinct runs"
    );
}

/// A payload a `planned` specialist cannot plan from is refused, not admitted.
///
/// And the refusal spends no admission key, so a corrected redelivery of the
/// same event id is admitted rather than answered with the refusal — which is
/// the difference between a bad payload and a poisoned message.
#[tokio::test]
async fn a_planned_specialist_with_no_trusted_input_is_refused_and_keeps_the_key_free() {
    let provider = FakeProvider::new();
    script(&provider);
    let plane = plane(&provider);

    let refused = plane
        .accept(ce("ce-untrusted"), json!({ "note": "no identifier here" }))
        .await;
    assert!(refused[0].refused.is_some(), "nothing to plan from");
    assert!(refused[0].run_id.is_empty(), "nothing was admitted");
    // **Permanent, not back-pressure.** The same bytes will carry no identifier
    // next time either, so the webhook answers `422` — which mako's emitter
    // dead-letters immediately, putting it where an operator sees it today
    // rather than after a retry schedule that could never have changed it.
    assert!(
        !refused[0].retryable,
        "resending identical bytes cannot help"
    );
    assert_eq!(
        Plane::reception(&refused),
        Reception::Unprocessable,
        "an all-refused fan-out with nothing retryable must not be answered 429 — that \
         burns the emitter's whole retry schedule to reach the same dead letter"
    );

    // Same event id, corrected payload.
    let corrected = plane.accept(ce("ce-untrusted"), event()).await;
    assert!(
        corrected[0].fresh,
        "a refusal must not claim the key — a corrected redelivery has to be admissible"
    );
}

/// An event nothing subscribes to accepts nothing, rather than everything.
#[tokio::test]
async fn an_unsubscribed_event_admits_nothing() {
    let provider = FakeProvider::new();
    let plane = plane(&provider);
    let accepted = plane
        .accept(
            Envelope {
                id: "ce-nobody",
                source: SOURCE,
                event_type: "de.nobody.listens.here",
            },
            event(),
        )
        .await;
    assert!(accepted.is_empty());
    assert_eq!(
        Plane::reception(&accepted),
        Reception::Admitted,
        "nothing subscribing is a 204, not a retry — a message nobody wants must never \
         come back as retryable, or every unsubscribed event is resent forever"
    );
}
