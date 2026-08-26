//! **The coded specialist's breach reaches a person.**
//!
//! `deadline-alert-agent` runs no model, so it declares no `oversight` block —
//! agentplane refuses one on a manifest with no `execution`. The
//! terminal-finding check in `plane::` therefore exempted coded specialists on
//! the stated assumption that they open a worklist row in Rust instead. Nothing
//! verified the assumption, and this specialist did not: a `BREACH` went into
//! the journal, into the decision delivery, and in front of nobody. The one
//! specialist whose whole job is *a regulatory window has closed* was the one
//! that told no one it had.
//!
//! This suite runs it on the production wiring with an overdue row from obsd and
//! asserts the row lands: on the right worklist, at the right priority, with the
//! window bounded by an obligation the calendar resolved, and widening rather
//! than expiring when nobody answers.
//!
//! It also pins the negative: a run whose worst severity is not `BREACH` files
//! nothing. A worklist that gains a row per warning is a worklist people close
//! without reading, which costs more than the control buys.

use std::sync::Arc;

use agentplane::core::{OnExpiry, Priority, TaskState};
use agentplane::store::RedbStore;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Stores};

/// The specialist under test: the one with no model at all.
const AGENT: &str = "deadline-alert-agent";
const EVENT_TYPE: &str = "de.obs.deadline.approaching";
/// The desk its Rust names, and the one it widens to.
const AUDIENCE: &str = "marktkommunikation";
const ESCALATION: &str = "mako-operations";

fn ce<'a>(id: &'a str) -> Envelope<'a> {
    Envelope {
        id,
        source: "urn:mako:test:tenant:9900357000004",
        event_type: EVENT_TYPE,
    }
}

/// obsd's overdue list, with a deadline placed relative to now.
///
/// A fixture with a literal timestamp would classify differently depending on
/// the day the suite runs — which is the one thing a severity test must not do.
#[derive(Debug)]
struct ObsdWith {
    offset_secs: i64,
}

#[async_trait::async_trait]
impl ToolClient for ObsdWith {
    async fn call(
        &self,
        _tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        let at = OffsetDateTime::now_utc() + time::Duration::seconds(self.offset_secs);
        Ok(json!({
            "overdue": [{
                "process_id": "9f1d2c3b-4a5e-6f70-8192-a3b4c5d6e7f8",
                "pid": 55001,
                "state": "AWAITING_ANSWER",
                "partner_mp_id": "9900357000004",
                "deadline_at": at.format(&Rfc3339).expect("rfc3339"),
                "deadline_source": "GPKE Teil 2 § 3.5",
            }],
            "saturated": false,
        }))
    }
}

/// The production wiring, with obsd substituted and no model at all.
fn plane(obsd: Arc<ObsdWith>) -> Plane {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");

    Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-deadline-test",
            tenant: &tenant,
            activated: &Activation::named(vec![AGENT.to_owned()]),
            // A specialist that asks no model needs no provider, and wiring one
            // would hide a regression where it started asking.
            providers: Vec::new(),
            tool_servers: vec![("obsd".to_owned(), obsd as Arc<dyn ToolClient>)],
            policy,
            keyring: None,
        },
    )
    .expect("the plane assembles")
}

/// The event obsd's warning carries: a process and the instant it is due.
fn event(offset_secs: i64) -> Value {
    let at = OffsetDateTime::now_utc() + time::Duration::seconds(offset_secs);
    json!({
        "malo_id": "51238696012",
        "process_id": "9f1d2c3b-4a5e-6f70-8192-a3b4c5d6e7f8",
        "pid": "55001",
        "partner_mp_id": "9900357000004",
        "due_at": at.format(&Rfc3339).expect("rfc3339"),
        "deadline_source": "GPKE Teil 2 § 3.5",
    })
}

async fn worklist(plane: &Plane, roles: &[&str]) -> Vec<agentplane::core::Task> {
    let runtime = plane.runtime();
    let tasks = runtime.tasks().expect("the plane has a task store").clone();
    let roles: Vec<String> = roles.iter().map(|r| (*r).to_owned()).collect();
    tasks.queue(&roles, 10).await.expect("queue readable")
}

/// A passed Frist opens a row on the MaKo desk, and the run still completes.
///
/// Opened rather than awaited: the window has already closed, so there is no
/// decision left to gate and suspending a run per breach would buy nothing.
#[tokio::test]
async fn a_breach_opens_a_worklist_row_and_does_not_suspend_the_run() {
    // Ninety minutes past due: BREACH on both the event and obsd's row.
    let plane = plane(Arc::new(ObsdWith {
        offset_secs: -90 * 60,
    }));

    let decision = plane
        .dispatch_one(AGENT, ce("ce-breach-1"), event(-90 * 60))
        .await
        .expect("the specialist is activated");

    assert_eq!(
        decision.outcome, "completed",
        "the row is opened, not awaited — the run concludes: {}",
        decision.summary
    );
    assert!(
        decision.summary.contains("BREACH"),
        "the answer states the worst severity it found: {}",
        decision.summary
    );

    let queue = worklist(&plane, &[AUDIENCE]).await;
    assert_eq!(
        queue.len(),
        1,
        "a breached Frist must reach the desk that answers for it, got {queue:#?}"
    );
    let task = &queue[0];
    assert_eq!(task.kind, "deadline.breach");
    assert_eq!(task.state, TaskState::Open);
    assert_eq!(
        task.priority,
        Priority::Urgent,
        "a window that has already closed is not normal-priority work"
    );
    assert_eq!(
        task.on_expiry,
        OnExpiry::Escalate,
        "the row *is* the finding — expiring it deletes the delivery of something \
         correctly detected"
    );
    assert_eq!(task.escalate_to, vec![ESCALATION.to_owned()]);
    assert!(
        task.due_at.is_some(),
        "the row is bounded by an obligation the calendar resolved, so a reviewer has a \
         window rather than an open-ended item"
    );
    assert!(
        task.justification.summary.contains("past their Frist"),
        "a reviewer must be able to act on the row without opening the journal: {}",
        task.justification.summary
    );
}

/// A Frist still in the future files nothing.
///
/// The negative half, and the reason the trigger row is classified at all: an
/// approaching deadline is a warning, and a worklist that gains a row per
/// warning is one people close without reading.
#[tokio::test]
async fn an_approaching_deadline_opens_no_row() {
    // Six hours out: COMPLIANT on both rows.
    let plane = plane(Arc::new(ObsdWith {
        offset_secs: 6 * 60 * 60,
    }));

    let decision = plane
        .dispatch_one(AGENT, ce("ce-warn-1"), event(6 * 60 * 60))
        .await
        .expect("the specialist is activated");

    assert_eq!(decision.outcome, "completed");
    assert!(
        worklist(&plane, &[AUDIENCE]).await.is_empty(),
        "nothing has breached, so nothing belongs on a worklist"
    );
}

/// The escalation audience can see the row too — after the escalation, and not
/// instead of the original desk.
#[tokio::test]
async fn the_escalation_audience_is_a_widening_not_a_reassignment() {
    let plane = plane(Arc::new(ObsdWith {
        offset_secs: -90 * 60,
    }));
    plane
        .dispatch_one(AGENT, ce("ce-breach-2"), event(-90 * 60))
        .await
        .expect("the specialist is activated");

    let task = worklist(&plane, &[AUDIENCE])
        .await
        .into_iter()
        .next()
        .expect("one row");
    assert!(
        task.may_decide("someone", &[AUDIENCE.to_owned()]),
        "the desk the row was filed for must be able to answer it"
    );
    assert!(
        !task.may_decide("someone", &["metering".to_owned()]),
        "a desk with no part in MaKo traffic is not an audience for it"
    );
}
