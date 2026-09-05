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
| `PidRouter` | Routes inbound messages to the correct workflow by **Prüfidentifikator** (PID) — the five-digit BDEW code naming the Anwendungsfall, not the EDIFACT message type |
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
use mako_engine::error::WorkflowError;
use mako_engine::workflow::{Workflow, WorkflowOutput};

pub struct MyWorkflow;

impl Workflow for MyWorkflow {
    type Command = MyCommand;
    type Event   = MyEvent;
    type State   = MyState;

    /// Pure function — rebuild state from one event at a time.
    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // state transition here
    }

    /// Pure function — no I/O, no clock, no global state.
    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // business logic here
    }
}
```

A `WorkflowOutput` carries the events *and* the outbox messages they imply. The
outbox half is only persisted when the command is dispatched through
`Process::execute_and_enqueue`; plain `Process::execute` ignores it. An empty
output is how a workflow says "already processed" — a no-op, not an error.

> `handle()` and `apply()` **must be pure**. All parsing, validation, and
> I/O happens at the transport boundary before a command is constructed.

---

## Feature flags

| Flag | Enables |
|---|---|
| `slatedb` | Production `SlateDbStore` — a single durable `EventStore` + `OutboxStore`; enable in binary crates only |
| `testing` | `InMemoryEventStore`, `InMemoryOutboxStore`, `NoopDeadLetterSink` — never in production |
| `tracing` | Structured instrumentation spans on workflow execution |

---

## Regulatory deadlines

Deadlines are scheduled through `DeadlineStore`; the *numbers* never live beside
a call site. Every business answer window comes from one table,
`mako_fristen::antwort`, keyed on the inbound Prüfidentifikator:

| Family | Deadline shape |
|---|---|
| GPKE Strom | a clock time on the *n*-th Werktag after the ÜT (11:00 / 06:00 / 05:00 / 09:00 on the 1., 00:00 on the 61. for a Neuanlage) |
| GPKE Sperrung / Teil 4 | „spätester ÜT ist der *n*. WT nach dem ÜT" — 1 WT, 2 WT, 10 WT (BK6-22-024 Anlage 1d) |
| WiM, beide Sparten | 3 / 5 / 7 / 1 Werktage, per PID |
| GeLi Gas | „Ablauf des *n*. Werktags nach Eingang" — 4 / 3 / 2 WT |
| NZR-EMob / Modell 2 | day-granular Werktage between two Netzbetreiber (55238–55243) |

`antwortfrist` returns `None` for a PID the Festlegungen do not quantify. That is
**unknown**, never unbounded and never "no deadline".

Two clocks sit outside that table and are deliberately separate:

- the **APERAK** technical acknowledgement — 45 minutes on a Werktag
  (`mako_fristen::aperak_strom_due_at`), with its own Gas variants;
- an **INVOIC** answer, which counts back from the Zahlungsziel the invoice
  itself carries (`SG8 DTM+265`) rather than forward from receipt — that one is
  `mako_fristen::vorlauf`.

**Saturday is not a Werktag.** GPKE (BK6-24-174) Teil 1: *"alle Tage ..., die kein Samstag, Sonntag oder gesetzlicher Feiertag sind"*. A holiday observed in any single Bundesland counts nationwide, and 24.12. and 31.12. count as holidays.
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
| [`mako-engine`](https://docs.rs/mako-engine) ← **this crate** | The runtime — `Workflow`, `Process`, `EventStore`, outbox, deadline scheduler |
| [`mako-gpke`](https://docs.rs/mako-gpke) | GPKE Strom — Lieferantenwechsel, Zuordnung, Netznutzungsabrechnung |
| [`mako-wim`](https://docs.rs/mako-wim) | WiM Strom und Gas — Wechselprozesse im Messwesen |
| [`mako-geli-gas`](https://docs.rs/mako-geli-gas) | GeLi Gas — Lieferantenwechsel Gas |
| [`mako-mabis`](https://docs.rs/mako-mabis) | MaBiS — Bilanzkreisabrechnung Strom |
| [`mako-gabi-gas`](https://docs.rs/mako-gabi-gas) | GaBi Gas — Gasbilanzierung |
| [`mako-redispatch`](https://docs.rs/mako-redispatch) | Redispatch 2.0 |
| [`mako-emob`](https://docs.rs/mako-emob) | NZR-EMob / Modell 2 |
| [`edi-energy`](https://docs.rs/edi-energy) | EDI@Energy EDIFACT — parse · validate · build (UTILMD, MSCONS, ORDERS, INVOIC, APERAK, …) |
| [`mako-fristen`](https://docs.rs/mako-fristen) | *When* an answer is due — Werktage, the MaKo holiday calendar, the per-PID Antwortfristen |
| [`mako-events`](https://docs.rs/mako-events) | CloudEvents `type` catalog — the shared event vocabulary |
| [`makod`](https://hupe1980.github.io/mako/docs/services/makod/) | Production daemon — assembles every module |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>
