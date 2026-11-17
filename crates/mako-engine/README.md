# mako-engine

[![Crates.io](https://img.shields.io/crates/v/mako-engine?label=mako-engine&color=f59e0b&logo=rust)](https://crates.io/crates/mako-engine)

The event-sourced process runtime the MaKo domain crates build on. A `Workflow`
is a pair of pure functions; `Process::run` replays its events, handles one
command and persists the resulting events, outbox messages, deadlines and
correlation keys in one atomic write. The engine routes inbound messages by
Prüfidentifikator and schedules deadlines. It parses no EDIFACT (that is
`edi-energy`) and holds no process rule — those live in the domain crates.

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
    process.run(Receive { pid: 55001 }, RunOptions::system()).await?;
    process.run(Receive { pid: 55001 }, RunOptions::system()).await?; // a redelivery: no-op

    assert_eq!(process.state().await?, vec![55001]);
    Ok(())
}
```

## What it covers

| Item | Role |
|---|---|
| `Workflow` | pure `handle` and `apply`; `DEADLINE_LABELS` and `on_deadline` for the Fristen it owns |
| `Process`, `RunOptions` | `run`, `run_timeout`, `state`, `state_with_snapshot`; every option travels in `RunOptions` |
| `EngineBuilder`, `EngineContext` | wire stores and `EngineModule`s; `spawn` and `resume` processes |
| `AtomicAppend` | the store's one write, `append(AppendBatch)`, with no default body |
| `EventStore`, `SnapshotStore` | the append-only log and the optional snapshot layer |
| `OutboxStore`, `DeadlineStore` | outbound delivery and registered deadlines |
| `WorkflowDriver` | a workflow registered once, by type, in `EngineModule::drivers` |
| `PidRouter`, `ProcessRegistry` | route an inbound message by Prüfidentifikator; find its process |
| `MessageAdapter`, `AdapterRegistry` | the inbound message's own format version picks its adapter |
| `DeadLetterSink` | unroutable or duplicate messages, with a structured reason |
| `payload` | typed outbound payloads (`AperakPayload`, …) shared by workflow and renderer |

## Contracts

- **One write.** An outbox entry or deadline without an event is refused
  (`EngineError::UnrecordedEffect`).
- **Owned deadlines.** A label outside the workflow's `DEADLINE_LABELS` is
  refused unless it is a delivery window. Deadlines are never cancelled, so
  `on_deadline` checks that the window is still open.
- **Format versions coexist.** A process keeps the `WorkflowId` it was created
  with; `AdapterRegistry::uncovered_format_versions` names a format version no
  adapter covers.
- **The windows live in `mako-fristen`**, never beside a call site.

## Features

| Feature | Enables |
|---|---|
| `slatedb` | `SlateDbStore`, the durable SlateDB event and outbox store — binary crates only |
| `testing` | the `InMemory*` and `Noop*` stores and `EngineBuilder::new()` — never in production |
| `tracing` | `#[instrument]` spans on `Process::run`, `state` and `state_with_snapshot` |

Architecture: [Engine](https://hupe1980.github.io/mako/docs/architecture/engine/) ·
API: <https://docs.rs/mako-engine>

License: MIT OR Apache-2.0
