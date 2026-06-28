//! # Process Resumption, Snapshots, Retry, and Error Handling
//!
//! This example demonstrates engine capabilities that go beyond the happy-path
//! write→store→read flow shown in `gpke_supplier_change.rs`:
//!
//! 1. **`EngineContext::spawn` / `::resume`** — assemble infrastructure via
//!    `EngineBuilder`, spawn a new process, persist the `ProcessIdentity`,
//!    and re-attach after a simulated service restart.
//!
//! 2. **Snapshot-integrated replay** (`take_snapshot` + `state_with_snapshot`)
//!    — writes state to `InMemorySnapshotStore`, then reconstructs with O(k)
//!    tail replay instead of O(n) full replay.
//!
//! 3. **`execute_with_retry`** — demonstrates the automatic version-conflict
//!    retry loop for commands that may race with concurrent writers.
//!
//! 4. **Error classification via `EngineError::is_*`** — `is_workflow_error()`,
//!    `as_workflow_error()`, and `is_invalid_state()` for clean error routing
//!    without exhaustive `match` arms.
//!
//! 5. **`process.workflow_id()` / `process.tenant_id()` accessors** — used to
//!    build a `CommandContext` for `execute_with` without threading IDs.
//!
//! ## Run
//!
//! ```text
//! cargo run --example process_lifecycle -p mako-engine
//! ```

use mako_engine::{
    builder::EngineBuilder,
    error::{EngineError, WorkflowError},
    event_store::InMemoryEventStore,
    ids::TenantId,
    snapshot::InMemorySnapshotStore,
    version::WorkflowId,
    workflow::{CommandContext, EventPayload, Workflow, WorkflowOutput},
};

// ═══════════════════════════════════════════════════════════════════════════════
// Minimal domain: a two-step activation workflow
//
// This is intentionally tiny — the focus is on engine mechanics, not domain
// complexity.
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
enum ActivationEvent {
    Registered { name: String },
    Activated,
    Deactivated { reason: String },
}

impl EventPayload for ActivationEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Registered { .. } => "Registered",
            Self::Activated => "Activated",
            Self::Deactivated { .. } => "Deactivated",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum ActivationStatus {
    #[default]
    New,
    Registered,
    Active,
    Inactive,
}

impl ActivationStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Registered => "Registered",
            Self::Active => "Active",
            Self::Inactive => "Inactive",
        }
    }
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct ActivationState {
    status: ActivationStatus,
    name: Option<String>,
}

#[derive(Clone)]
enum ActivationCommand {
    Register { name: String },
    Activate,
    Deactivate { reason: String },
}

impl mako_engine::workflow::CommandPayload for ActivationCommand {}

struct ActivationWorkflow;

impl Workflow for ActivationWorkflow {
    type State = ActivationState;
    type Event = ActivationEvent;
    type Command = ActivationCommand;

    fn apply(mut state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            ActivationEvent::Registered { name } => {
                state.status = ActivationStatus::Registered;
                state.name = Some(name.clone());
            }
            ActivationEvent::Activated => {
                state.status = ActivationStatus::Active;
            }
            ActivationEvent::Deactivated { .. } => {
                state.status = ActivationStatus::Inactive;
            }
        }
        state
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            ActivationCommand::Register { name } => {
                if state.status != ActivationStatus::New {
                    return Err(WorkflowError::invalid_state("New", state.status.as_str()));
                }
                Ok(vec![ActivationEvent::Registered { name }].into())
            }
            ActivationCommand::Activate => {
                if state.status != ActivationStatus::Registered {
                    return Err(WorkflowError::invalid_state(
                        "Registered",
                        state.status.as_str(),
                    ));
                }
                Ok(vec![ActivationEvent::Activated].into())
            }
            ActivationCommand::Deactivate { reason } => {
                if state.status != ActivationStatus::Active {
                    return Err(WorkflowError::invalid_state(
                        "Active",
                        state.status.as_str(),
                    ));
                }
                Ok(vec![ActivationEvent::Deactivated { reason }].into())
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  mako-engine — Process lifecycle example                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .with_snapshot_store(InMemorySnapshotStore::new())
        .build();

    // Snapshot every 2 events — low threshold to trigger within this example.
    // Production value is typically 100–200.
    const SNAP_INTERVAL: u64 = 2;

    // ── Phase 1: create and run a process ─────────────────────────────────────
    println!("Phase 1: Create process and run two commands");
    println!("─────────────────────────────────────────────");

    let process = ctx.spawn::<ActivationWorkflow>(
        TenantId::new(),
        WorkflowId::new("activation", "FV2025-04-01"),
    );

    // Save the identity so we can resume after simulated restart.
    // ProcessIdentity bundles (stream_id, process_id, tenant_id, workflow_id)
    // into a single serializable value — store it in your routing table.
    let identity = process.identity();

    println!("  Stream  : {}", process.stream_id());
    println!("  Workflow: {}", process.workflow_id());
    println!();

    // Register
    let envs = process
        .execute(ActivationCommand::Register {
            name: "Messstelle 12345".to_owned(),
        })
        .await?;
    for env in &envs {
        println!("  ✓ {} (seq {})", env.event_type, env.sequence_number);
    }

    // Activate — after this we have 2 events, SNAP_INTERVAL = 2 → snapshot fires.
    let envs = process.execute(ActivationCommand::Activate).await?;
    for env in &envs {
        println!("  ✓ {} (seq {})", env.event_type, env.sequence_number);
    }

    // take_snapshot: serializes state, persists to InMemorySnapshotStore.
    let snapped = process
        .take_snapshot(ctx.snapshot_store(), SNAP_INTERVAL)
        .await?;
    println!(
        "  📸 Snapshot taken: {snapped} (seq {})",
        process.event_count().await?
    );

    // state_with_snapshot: loads snapshot first, then replays tail (0 new events
    // here because we just snapshotted — O(0) tail replay).
    let state = process.state_with_snapshot(ctx.snapshot_store()).await?;
    println!("  State after phase 1: {}", state.status.as_str());

    // ── Phase 2: simulate service restart — resume via ProcessIdentity ────────
    println!();
    println!("Phase 2: Simulate service restart — resume via ProcessIdentity");
    println!("────────────────────────────────────────────────────────────────");

    // In production the identity would be deserialized from a routing table or
    // database row. Here we just use the value we saved above.
    let identity_json = serde_json::to_string(&identity).unwrap();
    println!("  identity JSON: {identity_json}");

    let identity_back: mako_engine::ids::ProcessIdentity =
        serde_json::from_str(&identity_json).unwrap();
    println!("  Resuming stream: {}", identity_back.stream_id());

    let resumed = ctx.resume::<ActivationWorkflow>(identity_back);

    // Use snapshot-accelerated replay: loads snapshot seq=2, tail = empty.
    let resumed_state = resumed.state_with_snapshot(ctx.snapshot_store()).await?;
    println!(
        "  ✓ State after resume (snapshot-accelerated): {} (name: {})",
        resumed_state.status.as_str(),
        resumed_state.name.as_deref().unwrap_or("-")
    );
    println!("  ✓ Event count: {}", resumed.event_count().await?);

    // Deactivate using execute_with (propagates tenant / workflow from accessors).
    let cmd_ctx = CommandContext::new(
        resumed.tenant_id(),
        resumed.process_id(),
        resumed.workflow_id().clone(),
    );
    let envs = resumed
        .execute_with(
            ActivationCommand::Deactivate {
                reason: "Planned maintenance".to_owned(),
            },
            cmd_ctx,
        )
        .await?;
    for env in &envs {
        println!("  ✓ {} (seq {})", env.event_type, env.sequence_number);
    }

    // ── Phase 3: execute_with_retry ───────────────────────────────────────────
    println!();
    println!("Phase 3: execute_with_retry — automatic version-conflict retry");
    println!("─────────────────────────────────────────────────────────────");

    // Create a second process to demonstrate retry. After one Register it will
    // be at seq=1. Retrying an already-valid Activate command succeeds on the
    // first attempt (no actual conflict here — illustrates the API).
    let p2 = ctx.spawn::<ActivationWorkflow>(
        TenantId::new(),
        WorkflowId::new("activation", "FV2025-04-01"),
    );
    p2.execute(ActivationCommand::Register {
        name: "P2-Messstelle".to_owned(),
    })
    .await?;
    let envs = p2
        .execute_with_retry(ActivationCommand::Activate, 3)
        .await?;
    println!(
        "  ✓ execute_with_retry succeeded (max_attempts=3): {} at seq {}",
        envs[0].event_type, envs[0].sequence_number
    );

    // ── Phase 4: error classification ─────────────────────────────────────────
    println!();
    println!("Phase 4: Error classification with EngineError::is_* helpers");
    println!("──────────────────────────────────────────────────────────────");

    // Activate on an already-Inactive process → InvalidState workflow error.
    let err: EngineError = resumed
        .execute(ActivationCommand::Activate)
        .await
        .unwrap_err();

    println!("  Raw error              : {err}");
    println!("  is_workflow_error()    : {}", err.is_workflow_error());
    println!("  is_version_conflict()  : {}", err.is_version_conflict());

    if let Some(we) = err.as_workflow_error() {
        println!("  WorkflowError variant  : {we}");
        println!("  is_invalid_state()     : {}", we.is_invalid_state());
        println!("  is_rejected()          : {}", we.is_rejected());
    }

    // ── Summary ───────────────────────────────────────────────────────────────
    println!();
    let final_state = resumed.state_with_snapshot(ctx.snapshot_store()).await?;
    let total_events = resumed.event_count().await?;

    println!("══════════════════════════════════════════════════════════════════");
    println!("  Final state  : {}", final_state.status.as_str());
    println!("  Total events : {total_events}");
    println!("  Checks passed: process resumption, snapshot-accelerated replay,");
    println!("                 execute_with_retry, error classification.");
    println!("══════════════════════════════════════════════════════════════════");

    Ok(())
}
