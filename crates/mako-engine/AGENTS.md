<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# mako-engine Crate Instructions

## Core Contracts

### Workflow trait
`Workflow::handle` and `Workflow::apply` are **pure functions** — zero I/O, no clock access, no global state mutation. All parsing, external calls, and validation happen at the transport boundary **before** constructing the command.

```rust
impl Workflow for MyWorkflow {
    type State   = MyState;   // Default + Clone + Send + Sync + 'static
    type Event   = MyEvent;   // impl EventPayload
    type Command = MyCommand; // impl CommandPayload

    fn handle(state: &Self::State, command: Self::Command)
        -> Result<WorkflowOutput<Self::Event>, WorkflowError> { … } // pure

    fn apply(state: Self::State, event: &Self::Event)
        -> Self::State { … } // pure

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_LABEL];

    fn on_deadline(deadline: &Deadline, state: &Self::State)
        -> Option<Self::Command> { … } // pure; asks window_open
}
```

`cargo xtask check-workflow-purity` refuses a clock, RNG, environment or
filesystem read inside a `Workflow` impl — an instant is a command field.

### Deadlines are owned
A workflow lists every label it registers in `Workflow::DEADLINE_LABELS`. The
engine refuses to persist any other label (`EngineError::UnownedDeadlineLabel`)
unless it is a delivery window (`mako_fristen::is_delivery_window` — the APERAK
and CONTRL windows, retired by the outbox worker on delivery).
`Process::run_timeout` calls `on_deadline` only for an owned label.

Deadlines are never cancelled, so every one fires — on the healthy path too.
`on_deadline` and the `TimeoutExpired` arm of `handle` must ask the same
`window_open(label, state)` guard; a lapse with no sourced default outcome
records `FristAbgelaufen` rather than inventing an answer.

### Modules register drivers
An `EngineModule` registers each workflow once, by type, in `drivers()`:
`WorkflowDriver::of::<MyWorkflow>(WORKFLOW_NAME)`. The driver carries the
labels and — with `slatedb` — the monomorphised `run_timeout` and migration
entry points, so a host dispatches by name without a name→type table.

### Dual-write atomicity — critical
Events, outbox entries, deadlines and correlation keys **must** be written together
in a single `WriteBatch`. `AtomicAppend::append(AppendBatch)` is the store's only
write and has no default body:
```rust
store.append(AppendBatch::new(&stream_id, version, &events).outbox(&outbox).deadlines(&deadlines)).await?;
// NEVER: write events first, then the rest — a crash between the two loses the APERAK permanently
```
`tests/it/append_conformance.rs` runs one suite over every store.

## Typed IDs

All domain identifiers are UUID v4 newtypes created via the `define_id!` macro in `ids.rs`:
```rust
// Correct — typed ID
let pid: ProcessId = ProcessId::new();

// Wrong — never use plain Uuid or String where a typed ID belongs
let pid: Uuid = Uuid::new_v4(); // ❌
```

Key ID types: `ProcessId`, `StreamId`, `EventId`, `DeadlineId`, `TenantId`.

## Deadline arithmetic — the `mako-fristen` crate

`mako-engine` schedules and dispatches deadlines; it does not compute them and
re-exports nothing. The arithmetic below is `mako_fristen`, a separate leaf
crate a caller depends on directly (these samples read as `fristen::` because
`use mako_fristen as fristen;` is the house spelling).

**There is no 24-hour GPKE window**, under BK6-24-174 or anything else — see
`mako_fristen::antwort::GPKE_IS_NOT_TWENTY_FOUR_HOURS`. A flat 24 h is neither
the technical acknowledgement nor the business answer, and it is wrong in the
direction that does not announce itself: it reports a lapsed Frist as still
running. Three different clocks apply, and they are not interchangeable:

```rust
// Wall-clock windows the Festlegungen state as durations — weekends and
// holidays do not extend them. CONTRL 6 h, Strom APERAK 45 min.
let due = fristen::contrl_due_at(received_at, ContrlAnlass::Regelfall);
let due = fristen::aperak_strom_due_at(received_at);

// The business answer is a clock time on the n-th Werktag after the
// Übertragungstag — never a duration. Ask the per-PID table, do not count.
// It yields the **published obligation**, not an instant: `None` means no
// Festlegung this codebase has read quantifies the window.
let obligation: Option<&'static AntwortObligation> =
    fristen::antwort::antwort_obligation(55_001);

// Werktage arithmetic, where a Festlegung really does count days.
// A Werktag is any day that is not a Samstag, a Sonntag or a gesetzlicher
// Feiertag (GPKE Teil 1 Kap. 2 and Kap. 7 „Fristenberechnung“) — Saturday does NOT count.
let deadline = fristen::add_werktage(received_date, 5, HolidayCalendar::BdewMaKo);
```

Deadline arithmetic is in **Europe/Berlin**; use `time::OffsetDateTime`, never
`chrono`. The governing Festlegung differs per process family — GPKE Teil 1–3 is
BK6-24-174, WiM Strom too, GPKE Teil 4 is BK6-22-024, GeLi Gas is
BK7-24-01-009 — so take the citation from the domain crate's own `AGENTS.md`
rather than from an example.

## Format versions

A workflow declares no acceptance policy. The inbound message's own FV picks the
adapter; the process keeps the `WorkflowId` it was created with. Every adapter
registry must cover every known FV — check with
`AdapterRegistry::uncovered_format_versions(&known_fvs())`, which `makod` calls
at startup and refuses to boot on.

Use `FormatVersion::parse(s)?` for user-supplied strings. `FormatVersion::new(...)` is unchecked — only for compile-time literals.

## Feature Flags

- `testing` — enables `InMemoryEventStore`, `InMemoryOutboxStore`, `NoopSnapshotStore`, etc. Gate behind `#[cfg(feature = "testing")]`. Must not appear in production builds.
- `slatedb` — persistent store. Enable only at the binary level (`services/makod`), never in library defaults.
- `tracing` — optional OTLP instrumentation; off by default.

## Process Lifecycle

```rust
// Spawn a new process
let process = ctx.spawn::<MyWorkflow>(tenant_id, workflow_id);
let ran = process.run(command, RunOptions::new()).await?;   // the one way to run a command

// A fired deadline: the workflow's own on_deadline decides the command
let acted = process.run_timeout(&deadline, RunOptions::new()).await?;

// Resume an existing process by identity (looked up via ProcessRegistry)
let identity = ctx.registry().lookup(tenant, &conv_id).await?.unwrap();
let process   = ctx.resume::<MyWorkflow>(identity);

// Reconstruct state by full replay
let state = process.state().await?;

// With snapshot optimization (O(k) replay from last snapshot)
let state = process.state_with_snapshot().await?;
```

## Testing

- Use `InMemoryEventStore` + `testing` feature for unit tests — never hit SlateDB in unit tests.
- Integration tests are modules of the one binary `tests/it/` (`main.rs` declares them).
- Run: `cargo test -p mako-engine --all-features` (or `--test it` for the integration binary).
