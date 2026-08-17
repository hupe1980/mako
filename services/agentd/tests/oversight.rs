//! **A mutating tool call waits for a named human, and a human can answer it.**
//!
//! Every one of the 28 manifests declares `oversight` — an approval mode, the
//! roles eligible to give it, a deadline and what happens when it passes. Until
//! the case layer was wired, none of it could happen: asking a human needs a
//! case to hold the task, a calendar to resolve the deadline and a timer to
//! expire it, and a plane without them failed the call instead of asking.
//!
//! ## What this file also pins, because finding it cost an afternoon
//!
//! A model completion is labelled **untrusted always** — its source is
//! `model:<id>`, and no prompt makes it otherwise. agentplane's taint gate
//! refuses a mutating sink whose arguments are untrusted. Put together: a
//! `tool-calling` agent can never dispatch a mutating call, because the model
//! wrote the arguments. Not even after an approval: the reviewer approves, and
//! the runtime then refuses.
//!
//! A `planned` agent can, and that is the whole reason the distinction exists.
//! Plan arguments are `$input/…` references the runtime resolves itself, so they
//! arrive carrying the run input's own labels and never pass through a model's
//! context. `gabi-gas-agent` is that shape, and it is what this file exercises
//! end to end.
//!
//! `cargo xtask check-tool-grants` enforces the rule on the manifests, so a
//! mutating grant cannot be re-added to a tool-calling specialist and read as a
//! capability it does not have.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agentplane::core::Decision;
use agentplane::model::{Completion, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};

use agentd::plane::{Activation, Plane, PlaneConfig, Stores};

/// The specialist under test: `planned`, and the one that dispatches.
const AGENT: &str = "gabi-gas-agent";
const EVENT_TYPE: &str = "de.gabi.imbalance.notified";
/// The role its manifest names in `oversight.approvers`.
const APPROVER: &str = "gas-operations";

/// Stands in for mako's MCP servers, and counts what actually reached them.
///
/// The count is the point: "the run suspended" and "the run suspended *before*
/// the tool ran" are different claims, and only the second one is a control.
#[derive(Debug, Default)]
struct CountingTools {
    submitted: AtomicUsize,
}

#[async_trait::async_trait]
impl ToolClient for CountingTools {
    async fn call(
        &self,
        tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        if tool.tool == "submit_command" {
            self.submitted.fetch_add(1, Ordering::SeqCst);
            return Ok(json!({ "accepted": true, "message_id": "MSG-1" }));
        }
        Ok(json!({ "items": [] }))
    }
}

/// A plane on in-memory stores, wired exactly as production wires it.
///
/// Same `Plane::new`, same Cedar policy set, same calendar — only the model, the
/// tool transport and the storage are substituted. A test that assembled the
/// runtime by hand would prove that agentplane works, not that agentd wires it.
fn plane(provider: &Arc<FakeProvider>, tools: &Arc<CountingTools>) -> Plane {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let store = RedbStore::open_in_memory().expect("store");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");

    let servers = ["makod", "marktd", "netzbilanzd"]
        .into_iter()
        .map(|name| (name.to_owned(), Arc::clone(tools) as Arc<dyn ToolClient>))
        .collect();

    Plane::new(
        Stores::redb(store, &tenant),
        PlaneConfig {
            // No outbox: this suite asserts on the plane's own state, not on
            // what a receiver was told.
            outbox: None,
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
        },
    )
    .expect("the plane assembles")
}

/// The event: identifiers only, which is the rule for agent inputs — a
/// specialist is handed keys and reaches the rest through its granted tools.
fn event() -> Value {
    json!({
        "malo_id": "51238696012",
        "pid": "13013",
        "mp_id": "9900357000004",
        "bilanzkreis_id": "THE0BFH012345678",
        "gas_day": "2026-08-06",
    })
}

/// The plan the privileged model returns: dispatch, then say what happened.
///
/// The arguments are `$input/…` **references**, not values. That is what makes
/// the call dispatchable at all: the runtime resolves them itself, so they
/// arrive carrying the labels `plane::label` gave them — a re-validated MaLo is
/// trusted — instead of the untrusted label every model completion carries.
fn plan_that_dispatches() -> Completion {
    let submit = ToolId::new("makod", "submit_command").wire_name();
    completion(json!({
        "steps": [
            { "tool": submit, "args": {
                "malo_id": "$input/malo_id",
                "pid":     "$input/pid",
                "mp_id":   "$input/mp_id"
            }},
            { "parse": { "from": "$step0", "schema": result_schema() } }
        ],
        "answer": "$step1"
    }))
}

/// The manifest's own result contract.
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

/// What the quarantined model returns for the `parse` step.
fn parsed_answer() -> Completion {
    completion(json!({
        "gas_day": "2026-08-06",
        "imbalance_status": "MINDER",
        "allocation_version": "Initial",
        "deadline_compliant": true,
        "action": "REQUEST_CORRECTION",
        "legal_basis": "KoV §6.4 Abs. 3",
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

fn script(provider: &FakeProvider) {
    provider.will_answer(plan_that_dispatches());
    provider.will_answer(parsed_answer());
}

/// The single open task on a role's worklist.
async fn only_open_task(plane: &Plane, roles: &[String]) -> agentplane::core::Task {
    let runtime = plane.runtime();
    let tasks = runtime.tasks().expect("the plane has a task store").clone();
    let queue = tasks.queue(roles, 10).await.expect("queue readable");
    assert_eq!(
        queue.len(),
        1,
        "exactly one approval should be waiting, got {queue:#?}"
    );
    queue.into_iter().next().expect("one task")
}

/// A mutating call suspends the run and opens a task for the declared role.
#[tokio::test]
async fn a_mutating_tool_call_waits_for_a_named_approver() {
    let provider = FakeProvider::new();
    script(&provider);
    let tools = Arc::new(CountingTools::default());
    let plane = plane(&provider, &tools);

    let decision = plane
        .dispatch_one(AGENT, "ce-1", EVENT_TYPE, event())
        .await
        .expect("the specialist is activated");

    assert_eq!(
        decision.outcome, "suspended",
        "a call needing approval must suspend, not proceed: {decision:?}"
    );
    assert_eq!(
        tools.submitted.load(Ordering::SeqCst),
        0,
        "the tool ran before anybody approved it"
    );
    assert!(
        decision
            .waiting_for
            .as_deref()
            .is_some_and(|w| !w.is_empty()),
        "a suspended run must say what it waits for: {decision:?}"
    );

    // The declared approver sees it…
    let task = only_open_task(&plane, &[APPROVER.to_owned()]).await;
    assert!(
        task.candidate_roles.iter().any(|r| r == APPROVER),
        "the manifest's approvers must reach the task: {task:?}"
    );
    // …and it carries the exact call, not a description of one.
    let justification = format!("{:?}", task.justification);
    assert!(
        justification.contains("submit_command"),
        "the proposal must name the tool about to run: {justification}"
    );

    // …while somebody else's queue is empty. A worklist that shows every task
    // to everybody is not an eligibility control.
    let runtime = plane.runtime();
    assert!(
        runtime
            .tasks()
            .expect("task store")
            .queue(&["LF".to_owned()], 10)
            .await
            .expect("queue")
            .is_empty(),
        "a market role is not an approver for this agent"
    );
}

/// An approval resumes the run, and only then does the call land.
#[tokio::test]
async fn an_approval_resumes_the_run_and_the_call_lands() {
    let provider = FakeProvider::new();
    script(&provider);
    let tools = Arc::new(CountingTools::default());
    let plane = plane(&provider, &tools);

    plane
        .dispatch_one(AGENT, "ce-2", EVENT_TYPE, event())
        .await
        .expect("dispatched");
    let task = only_open_task(&plane, &[APPROVER.to_owned()]).await;

    let runtime = plane.runtime();
    let delivery = runtime
        .decide_task(
            task.id,
            &Decision::approve("user:anna", "the correction is due today"),
            &[APPROVER.to_owned()],
        )
        .await
        .expect("the decision is delivered and the run resumes");

    assert!(
        delivery.resumed_run().is_some(),
        "a decision must resume the run that waited for it, got {delivery:?}"
    );
    assert_eq!(
        tools.submitted.load(Ordering::SeqCst),
        1,
        "the approved call must reach the server exactly once"
    );
    assert_eq!(
        runtime
            .tasks()
            .expect("task store")
            .open_count()
            .await
            .expect("count"),
        0,
        "a decided task must leave the worklist"
    );
}

/// A rejection ends the run and the call never happens.
///
/// The reviewer's words stay out of the model's next turn on purpose — a
/// human's free text steering an agent is untrusted content in the one slot the
/// design keeps clean — so this asserts the effect, not the wording.
#[tokio::test]
async fn a_rejection_stops_the_call() {
    let provider = FakeProvider::new();
    script(&provider);
    let tools = Arc::new(CountingTools::default());
    let plane = plane(&provider, &tools);

    plane
        .dispatch_one(AGENT, "ce-3", EVENT_TYPE, event())
        .await
        .expect("dispatched");
    let task = only_open_task(&plane, &[APPROVER.to_owned()]).await;

    plane
        .runtime()
        .decide_task(
            task.id,
            &Decision::reject("user:anna", "the MGV already corrected it"),
            &[APPROVER.to_owned()],
        )
        .await
        .expect("the rejection is delivered");

    assert_eq!(
        tools.submitted.load(Ordering::SeqCst),
        0,
        "a rejected call must never reach the server"
    );
}

/// A caller without the declared role cannot decide.
///
/// Eligibility is enforced by the store rather than by the surface, so this
/// holds for every path into it — the HTTP worklist, a script, a future worker.
#[tokio::test]
async fn an_ineligible_actor_cannot_decide() {
    let provider = FakeProvider::new();
    script(&provider);
    let tools = Arc::new(CountingTools::default());
    let plane = plane(&provider, &tools);

    plane
        .dispatch_one(AGENT, "ce-4", EVENT_TYPE, event())
        .await
        .expect("dispatched");
    let task = only_open_task(&plane, &[APPROVER.to_owned()]).await;

    let err = plane
        .runtime()
        .decide_task(
            task.id,
            &Decision::approve("user:mallory", "looks fine"),
            &["LF".to_owned()],
        )
        .await
        .expect_err("a market role must not be able to approve a dispatch");

    let msg = err.to_string();
    assert!(
        msg.contains("mallory") || msg.to_lowercase().contains("role"),
        "the refusal should say why: {msg}"
    );
    assert_eq!(
        tools.submitted.load(Ordering::SeqCst),
        0,
        "an ineligible approval must not release the call"
    );
}

/// The surface itself builds over a plane, and refuses an ungoverned one.
///
/// `Api::new` checks that every registered plane has a policy engine, because
/// an HTTP surface with no authorization layer hands every authenticated caller
/// the whole plane. This asserts we satisfy that check rather than trip it at
/// startup — and, by construction, that `Plane` always carries a policy.
#[tokio::test]
async fn the_oversight_surface_mounts_over_the_plane() {
    let provider = FakeProvider::new();
    let tools = Arc::new(CountingTools::default());
    let plane = plane(&provider, &tools);

    let verifier = Arc::new(mako_service::oidc::OidcVerifier::disabled("9900357000004"));
    let _router = agentd::plane::oversight::router(plane.runtime(), verifier)
        .expect("the worklist mounts over a governed plane");
}

/// Runs about one MaLo share a case; a different MaLo is a different matter.
///
/// The case is the erasure unit: "everything we processed about this
/// Marktlokation" has to be one key to destroy, and one matter to read.
#[tokio::test]
async fn runs_about_one_malo_share_a_case() {
    let provider = FakeProvider::new();
    // Three runs that decide nothing: a plan whose only step is a parse, so no
    // approval is involved and each run ends immediately.
    for _ in 0..3 {
        provider.will_answer(completion(json!({
            "steps": [{ "parse": { "from": "$input", "schema": result_schema() } }],
            "answer": "$step0"
        })));
        provider.will_answer(parsed_answer());
    }
    let tools = Arc::new(CountingTools::default());
    let plane = plane(&provider, &tools);

    let malo = event();
    let mut other = event();
    other["malo_id"] = json!("51238696781");

    let first = plane
        .dispatch_one(AGENT, "ce-5", EVENT_TYPE, malo.clone())
        .await
        .expect("dispatched");
    let second = plane
        .dispatch_one(AGENT, "ce-6", EVENT_TYPE, malo)
        .await
        .expect("dispatched");
    let third = plane
        .dispatch_one(AGENT, "ce-7", EVENT_TYPE, other)
        .await
        .expect("dispatched");

    // Read the case off the run's own records rather than asking the case
    // store: every record of a case-bound run carries its case, which is what
    // makes "show me everything about this matter" one range scan — and what an
    // erasure has to reach.
    let runtime = plane.runtime();
    let journal = runtime.journal().clone();
    let case_of = async |run: &str| {
        let run = agentplane::core::RunId::parse(run).expect("a run id");
        journal
            .read(run, 0)
            .await
            .expect("records")
            .into_iter()
            .find_map(|r| r.body.case)
    };

    let a = case_of(&first.run_id).await;
    let b = case_of(&second.run_id).await;
    let c = case_of(&third.run_id).await;

    assert!(a.is_some(), "a dispatched run must belong to a case");
    assert_eq!(a, b, "two events about one MaLo belong to one matter");
    assert_ne!(a, c, "a different MaLo is a different matter");
}
