//! A workflow is registered once, and makod treats it by name from there.
//!
//! The fixture below is a whole domain module: one workflow, one PID, one
//! `WorkflowDriver::of` line. Nothing in makod names it. The engine built from
//! it spawns and resumes the process, the deadline dispatcher decides its fired
//! Frist through the driver, and the migration runs through the driver too — so
//! adding a workflow touches its crate and that one registration line.

use std::sync::Arc;

use mako_engine::{
    builder::{EngineBuilder, EngineModule},
    deadline::Deadline,
    driver::WorkflowDriver,
    error::WorkflowError,
    ids::TenantId,
    pid_router::PidRouter,
    process::RunOptions,
    store_slatedb::SlateDbStore,
    version::WorkflowId,
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use makod::orchestrator::deadline_dispatch::{Fired, dispatch_deadline};

const NAME: &str = "fixture-anfrage";
const ANTWORT: &str = "fixture-anfrage-antwort";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum FixtureEvent {
    Gesendet,
    Beantwortet,
    Abgelaufen,
}

impl EventPayload for FixtureEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Gesendet => "FixtureGesendet",
            Self::Beantwortet => "FixtureBeantwortet",
            Self::Abgelaufen => "FixtureAbgelaufen",
        }
    }
}

#[derive(Debug, Clone)]
enum FixtureCommand {
    Senden { frist: time::OffsetDateTime },
    Antworten,
    TimeoutExpired,
}

impl CommandPayload for FixtureCommand {}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum FixtureState {
    #[default]
    Neu,
    Wartet,
    Beantwortet,
    Abgelaufen,
}

struct FixtureWorkflow;

impl Workflow for FixtureWorkflow {
    type State = FixtureState;
    type Event = FixtureEvent;
    type Command = FixtureCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT];

    fn on_deadline(deadline: &Deadline, state: &FixtureState) -> Option<FixtureCommand> {
        (deadline.label() == ANTWORT && *state == FixtureState::Wartet)
            .then_some(FixtureCommand::TimeoutExpired)
    }

    fn apply(_state: FixtureState, event: &FixtureEvent) -> FixtureState {
        match event {
            FixtureEvent::Gesendet => FixtureState::Wartet,
            FixtureEvent::Beantwortet => FixtureState::Beantwortet,
            FixtureEvent::Abgelaufen => FixtureState::Abgelaufen,
        }
    }

    fn handle(
        _state: &FixtureState,
        command: FixtureCommand,
    ) -> Result<WorkflowOutput<FixtureEvent>, WorkflowError> {
        Ok(match command {
            FixtureCommand::Senden { frist } => WorkflowOutput::with_outbox_and_deadline(
                vec![FixtureEvent::Gesendet],
                Vec::new(),
                PendingDeadline::new(ANTWORT, frist),
            ),
            FixtureCommand::Antworten => vec![FixtureEvent::Beantwortet].into(),
            FixtureCommand::TimeoutExpired => vec![FixtureEvent::Abgelaufen].into(),
        })
    }
}

/// The whole registration: a PID routed to the name, and one driver line.
struct FixtureModule;

impl EngineModule for FixtureModule {
    fn name(&self) -> &'static str {
        "fixture"
    }

    fn register_pids(&self, router: &mut PidRouter) {
        router.register(17_999, NAME);
    }

    fn drivers(&self) -> Vec<WorkflowDriver> {
        vec![WorkflowDriver::of::<FixtureWorkflow>(NAME)]
    }
}

#[tokio::test]
async fn a_workflow_registered_once_is_spawned_resumed_and_timed_out() {
    let store = Arc::new(SlateDbStore::open_in_memory().await.expect("store"));
    let ctx = EngineBuilder::new()
        .with_event_store(Arc::clone(&store))
        .register(Box::new(FixtureModule))
        .build();
    assert_eq!(ctx.registered_workflows(), [NAME]);
    let drivers = ctx.drivers().iter().map(|d| (d.name(), *d)).collect();

    let tenant = TenantId::new();
    let workflow_id = WorkflowId::new(NAME, "FV2026-10-01");

    // Spawned, and its Frist registered in the same write.
    let frist = time::macros::datetime!(2026-10-09 22:00 UTC);
    let spawned = ctx.spawn::<FixtureWorkflow>(tenant, workflow_id.clone());
    let ran = spawned
        .run(FixtureCommand::Senden { frist }, RunOptions::new())
        .await
        .expect("spawned");
    assert_eq!(ran.deadlines.len(), 1);

    // The fired Frist reaches the workflow through its driver, and acts.
    let fired = dispatch_deadline(
        &drivers,
        ran.deadlines[0].clone(),
        Arc::clone(&store),
        store.as_snapshot_store(),
        0,
    )
    .await
    .expect("dispatched");
    assert_eq!(fired, Fired::Acted);

    // Resumed by identity, a second process answers before its Frist — and the
    // same firing then decides nothing.
    let second = ctx.spawn::<FixtureWorkflow>(tenant, workflow_id);
    let ran = second
        .run(FixtureCommand::Senden { frist }, RunOptions::new())
        .await
        .expect("spawned");
    ctx.resume::<FixtureWorkflow>(second.identity())
        .run(FixtureCommand::Antworten, RunOptions::new())
        .await
        .expect("resumed");
    let fired = dispatch_deadline(
        &drivers,
        ran.deadlines[0].clone(),
        Arc::clone(&store),
        store.as_snapshot_store(),
        0,
    )
    .await
    .expect("dispatched");
    assert_eq!(fired, Fired::Closed);
    assert_eq!(
        spawned.state().await.expect("replay"),
        FixtureState::Abgelaufen
    );

    // The migration runs through the same driver.
    let report = ctx.drivers()[0]
        .migrate_identity(&store, "FV2026-10-01", "FV2027-04-01")
        .await;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

/// A PID routed to a workflow that registers no driver is refused at build:
/// its deadlines would fire into nothing.
#[test]
#[should_panic(expected = "registers no driver for")]
fn a_routed_workflow_without_a_driver_does_not_build() {
    struct Undriven;
    impl EngineModule for Undriven {
        fn name(&self) -> &'static str {
            "undriven"
        }
        fn register_pids(&self, router: &mut PidRouter) {
            router.register(17_998, "undriven-workflow");
        }
    }
    let _ = EngineBuilder::new()
        .with_event_store(mako_engine::event_store::InMemoryEventStore::new())
        .register(Box::new(Undriven))
        .build();
}
