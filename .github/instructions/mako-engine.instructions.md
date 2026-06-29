---
description: "Use when working in crates/mako-engine: implementing Workflow or EventStore traits, writing commands/events, handling deadlines, projections, snapshots, outbox, or the SlateDB persistence layer."
applyTo: "crates/mako-engine/**"
---

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

```rust
// GPKE: 24 consecutive wall-clock hours (BK6-22-024 §5)
let deadline = fristen::add_hours(received_at, 24);

// WiM: 5 Werktage (BK6-24-174) — Saturday counts, Sunday/holidays do not
let deadline = fristen::add_werktage(received_date, 5, BdewMaKo);

// GeLi Gas: 10 Werktage (BK7-24-01-009)
let deadline = fristen::add_werktage(received_date, 10, BdewMaKo);
```

All deadline arithmetic in **German local time (CET/CEST)**. Use `time::OffsetDateTime`, never `chrono`.

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
