# mako-engine

[![Crates.io](https://img.shields.io/crates/v/mako-engine?label=mako-engine&color=f59e0b&logo=rust)](https://crates.io/crates/mako-engine)

The event-sourced process runtime the MaKo domain crates build on. A
`Workflow` is a pair of pure functions; `Process::run` replays its events,
handles one command and persists the resulting events, outbox messages,
deadlines and correlation keys in one atomic write. The runtime also routes
inbound messages by Prüfidentifikator, schedules deadlines and drives the
outbox delivery worker.

```bash
cargo add mako-engine            # add --features testing for the in-memory stores
```

## Example

```rust
use mako_engine::builder::EngineBuilder;
use mako_engine::error::WorkflowError;
use mako_engine::event_store::InMemoryEventStore;
use mako_engine::ids::TenantId;
use mako_engine::process::RunOptions;
use mako_engine::version::WorkflowId;
use mako_engine::workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
struct Receive { pid: u32 }
impl CommandPayload for Receive {}

#[derive(Serialize, Deserialize)]
struct Received { pid: u32 }
impl EventPayload for Received {
    fn event_type(&self) -> &'static str { "Received" }
}

struct Inbox;
impl Workflow for Inbox {
    type State = Vec<u32>;
    type Event = Received;
    type Command = Receive;

    // Pure: fold one event into the state.
    fn apply(mut state: Vec<u32>, event: &Received) -> Vec<u32> {
        state.push(event.pid);
        state
    }

    // Pure: no I/O, no clock. An empty output means "already processed".
    fn handle(state: &Vec<u32>, cmd: Receive) -> Result<WorkflowOutput<Received>, WorkflowError> {
        if state.contains(&cmd.pid) {
            return Ok(vec![].into());
        }
        Ok(vec![Received { pid: cmd.pid }].into())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // EngineBuilder::new() and InMemoryEventStore need the `testing` feature.
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .build();

    let process = ctx.spawn::<Inbox>(TenantId::new(), WorkflowId::new("inbox", "FV2026-10-01"));
    process.run(Receive { pid: 55001 }, RunOptions::new()).await?;
    process.run(Receive { pid: 55001 }, RunOptions::new()).await?; // a redelivery: no-op

    assert_eq!(process.state().await?, vec![55001]);
    Ok(())
}
```

## Key types

| Item | Description |
|---|---|
| `Workflow` | Core trait — pure `handle()` and `apply()`; `DEADLINE_LABELS` and `on_deadline()` for the Fristen it owns |
| `Process` | Runtime handle — `run(cmd, RunOptions)`, `run_timeout(&deadline, RunOptions)`, `state()`, `state_with_snapshot()` |
| `RunOptions` | Context, version-conflict attempts, extra deadlines, correlation keys, snapshot — options, never another method |
| `EngineContext` | Entry point — `spawn()` and `resume()` processes |
| `EngineBuilder` | Fluent builder for wiring stores and modules |
| `EventStore` | Read side of the append-only event log |
| `AtomicAppend` | The one write: `append(AppendBatch)`, no default body |
| `WorkflowDriver` | A workflow registered once, by type, in `EngineModule::drivers` — what a host dispatches deadlines and migrations through |
| `payload::*` | Typed outbound payloads (`AperakPayload`, `UtilmdPayload`, …) shared by the producing workflow and the renderer |
| `OutboxStore` | Transactional outbox for AS4 message delivery |
| `DeadlineStore` | Registered deadlines; a fired one runs through `Process::run_timeout` → `Workflow::on_deadline` |
| `SnapshotStore` | Optional snapshot layer for O(k) state reconstruction |
| `PidRouter` | Routes inbound messages to the correct workflow by **Prüfidentifikator** (PID) — the five-digit BDEW code naming the Anwendungsfall, not the EDIFACT message type |
| `ProcessRegistry` | Maps conversation IDs to `ProcessIdentity` |
| `DeadLetterSink` | Receives unroutable or duplicate messages with structured reasons |

## Contracts

- **One write.** `AtomicAppend::append(AppendBatch)` is the store's only write
  and has no default body; an outbox entry or deadline without an event is
  refused (`EngineError::UnrecordedEffect`).
- **Owned deadlines.** The engine refuses to persist a label outside the
  workflow's `DEADLINE_LABELS` unless it is a delivery window. Deadlines are
  never cancelled, so `on_deadline` must check that the window is still open.
- **Format versions coexist.** A process keeps the `WorkflowId` (and FV) it was
  created with; the inbound message's FV picks the `MessageAdapter`.
  `AdapterRegistry::uncovered_format_versions` names what a registry misses.
- **The numbers live in `mako-fristen`**, never beside a call site.

| Feature | Enables |
|---|---|
| `slatedb` | the durable `SlateDbStore` — binary crates only |
| `testing` | `InMemoryEventStore`, `InMemoryOutboxStore`, `EngineBuilder::new()` — never in production |
| `tracing` | instrumentation spans on workflow execution |

API documentation: <https://docs.rs/mako-engine>. Architecture:
[engine](https://hupe1980.github.io/mako/docs/architecture/engine/).
