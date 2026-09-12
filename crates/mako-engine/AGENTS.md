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

    fn handle(state: &Self::State, cmd: Self::Command)
        -> Result<Vec<NewEvent<Self::Event>>, WorkflowError> { … } // pure

    fn apply(state: Self::State, event: &EventEnvelope<Self::Event>)
        -> Self::State { … } // pure
}
```

### Dual-write atomicity — critical
Events and outbox entries **must** be written together in a single `WriteBatch`:
```rust
store.append_with_outbox(&stream_id, events, outbox_entries).await?;
// NEVER: write events first, then outbox — a crash between the two loses the APERAK permanently
```

## Typed IDs

All domain identifiers are UUID v4 newtypes created via the `define_id!` macro in `ids.rs`:
```rust
// Correct — typed ID
let pid: ProcessId = ProcessId::new();

// Wrong — never use plain Uuid or String where a typed ID belongs
let pid: Uuid = Uuid::new_v4(); // ❌
```

Key ID types: `ProcessId`, `StreamId`, `EventId`, `DeadlineId`, `TenantId`.

## Deadline Arithmetic (fristen module)

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
let due = fristen::antwort::obligation(55_001);

// Werktage arithmetic, where a Festlegung really does count days:
// Saturday counts, Sunday and a BDEW MaKo holiday do not.
let deadline = fristen::add_werktage(received_date, 5, HolidayCalendar::BdewMaKo);
```

Deadline arithmetic is in **Europe/Berlin**; use `time::OffsetDateTime`, never
`chrono`. The governing Festlegung differs per process family — GPKE Teil 1–3 is
BK6-24-174, GPKE Teil 4 and WiM Strom are BK6-22-024, GeLi Gas is
BK7-24-01-009 — so take the citation from the domain crate's own `AGENTS.md`
rather than from an example.

## Version Policy

```rust
// Correct default for ALL MaKo workflows:
WorkflowVersionPolicy::ForwardCompatible

// Pinned is only for special upgrade scenarios — never the default
WorkflowVersionPolicy::Pinned // ❌ as default
```

Use `FormatVersion::parse(s)?` for user-supplied strings. `FormatVersion::new(...)` is unchecked — only for compile-time literals.

## Feature Flags

- `testing` — enables `InMemoryEventStore`, `InMemoryOutboxStore`, `NoopSnapshotStore`, etc. Gate behind `#[cfg(feature = "testing")]`. Must not appear in production builds.
- `slatedb` — persistent store. Enable only at the binary level (`services/makod`), never in library defaults.
- `tracing` — optional OTLP instrumentation; off by default.

## Process Lifecycle

```rust
// Spawn a new process
let process = ctx.spawn::<MyWorkflow>(tenant_id, workflow_id);
let envelopes = process.execute(command).await?;

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
- Run: `cargo test -p mako-engine --all-features`
