# mako-engine

**Event-sourced process runtime for German energy market communication (MaKo).**

The core runtime that all domain crates (`mako-gpke`, `mako-wim`,
`mako-geli-gas`, …) build on. Provides event sourcing, optimistic-concurrency
event storage, regulatory-deadline scheduling, outbox-based AS4 delivery, and
process-state projections.

---

## Architecture

```
Raw EDIFACT bytes (AS4 transport)
        │
        ▼
[edi-energy] parse · validate
        │
        ▼  Command (typed, validated)
EngineContext::spawn / ::resume → Process::execute
        │
        ├─ load events → reconstruct state (Workflow::apply)
        ├─ handle command (Workflow::handle — pure, deterministic)
        └─ append EventEnvelope batch (optimistic concurrency)

EventStore   ──► ProjectionRunner  ──► Read models
SnapshotStore ──► Process::state_with_snapshot (O(k) replay)
OutboxStore  ──► delivery worker   ──► AS4 endpoint
DeadlineStore ──► scheduler        ──► TimeoutDeadline command
PidRouter    ──► inbound routing   ──► Process
```

---

## Key traits and types

| Item | Description |
|---|---|
| `Workflow` | Core trait — implement `handle()` and `apply()` (both must be pure / no I/O) |
| `Process` | Runtime handle — `execute()`, `state()`, `state_with_snapshot()` |
| `EngineContext` | Entry point — `spawn()` and `resume()` processes |
| `EngineBuilder` | Fluent builder for wiring stores and modules |
| `EventStore` | Append-only, optimistic-concurrency event log |
| `OutboxStore` | Transactional outbox for AS4 message delivery |
| `DeadlineStore` | Regulatory deadline scheduling (APERAK Fristen) |
| `SnapshotStore` | Optional snapshot layer for O(k) state reconstruction |
| `PidRouter` | Routes inbound messages to the correct workflow by Prüfidentifikator |
| `ProcessRegistry` | Maps conversation IDs to `ProcessIdentity` |
| `DeadLetterSink` | Receives unroutable or duplicate messages with structured reasons |

---

## Quick start

```rust,ignore
use mako_engine::{
    builder::EngineBuilder,
    ids::TenantId,
    version::WorkflowId,
    event_store::InMemoryEventStore,
};

let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .build();

// Spawn a new process for one conversation.
let process = ctx.spawn::<MyWorkflow>(TenantId::new(), WorkflowId::new("wf-id", "FV2025-10-01"));
let envelopes = process.execute(my_command).await?;

// Reconstruct typed state by replaying all events.
let state = process.state().await?;

// Resume on the next inbound message.
let identity = ctx.registry().lookup(tenant, &conv_id).await?.unwrap();
let resumed  = ctx.resume::<MyWorkflow>(identity);
```

---

## Implementing a workflow

```rust,ignore
use mako_engine::workflow::{Workflow, WorkflowResult};

pub struct MyWorkflow;

impl Workflow for MyWorkflow {
    type Command = MyCommand;
    type Event   = MyEvent;
    type State   = MyState;

    /// Pure function — no I/O, no clock, no global state.
    fn handle(state: &Self::State, cmd: Self::Command) -> WorkflowResult<Self::Event> {
        // business logic here
    }

    /// Pure function — rebuild state from one event at a time.
    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // state transition here
    }
}
```

> `handle()` and `apply()` **must be pure**. All parsing, validation, and
> I/O happens at the transport boundary before a command is constructed.

---

## Feature flags

| Flag | Enables |
|---|---|
| `slatedb` | Production `SlateDbEventStore` / `SlateDbOutboxStore` — enable in binary crates only |
| `testing` | `InMemoryEventStore`, `InMemoryOutboxStore`, `NoopDeadLetterSink` — never in production |
| `tracing` | Structured instrumentation spans on workflow execution |

---

## Regulatory deadlines

APERAK response deadlines are encoded via `DeadlineStore`. Each domain crate
uses the correct helper from `fristen`:

| Process family | Deadline | Helper |
|---|---|---|
| GPKE | 24 wall-clock hours | `fristen::add_hours(t, 24)` |
| WiM | 5 Werktage | `fristen::add_werktage(d, 5, BdewMaKo)` |
| GeLi Gas | 10 Werktage | `fristen::add_werktage(d, 10, BdewMaKo)` |
| WiM Gas | 10 Werktage | `fristen::add_werktage(d, 10, BdewMaKo)` |

**Saturday counts as a Werktag.** Sunday and public holidays do not.
Deadline arithmetic uses **German local time (CET/CEST)** via the `time` crate.

---

## Dual-write atomicity

Events and outbox entries are written in a single `WriteBatch` via
`AtomicAppend::append_with_outbox`. Never write events first and outbox
second — a crash between the two produces a lost APERAK with no recovery path.

---

## Format-version coexistence

`WorkflowVersionPolicy::ForwardCompatible` (the default for all MaKo workflows)
allows a process started under `FV2025-10-01` to continue under those rules
after the `FV2026-10-01` cutover. Do not use `Pinned` as default.

---

## Related crates

| Crate | Role |
|---|---|
| `mako-engine` ← **this crate** | Runtime |
| `mako-gpke` | GPKE domain workflows |
| `mako-wim` | WiM Strom domain workflows |
| `mako-geli-gas` | GeLi Gas 3.0 domain workflows |
| `mako-mabis` | MABIS billing workflows |
| `edi-energy` | EDIFACT parse / validate (transport boundary) |
| `makod` | Production daemon — assembles all modules |
