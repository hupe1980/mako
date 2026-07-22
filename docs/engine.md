---
layout: default
title: Process Engine
nav_order: 14
parent: Architecture
mermaid: true
description: >
  mako-engine architecture: event-sourced Workflow FSMs, atomic dual-write,
  DeadlineStore, OutboxWorker, PidRouter, ProcessRegistry, SlateDB backend,
  format-version coexistence (FV2025/FV2026), and ForwardCompatible policy.
---

# Process Engine Guide (`mako-engine`)

`mako-engine` is an event-sourced process runtime for long-running German energy
market workflows. It handles the stateful side of MaKo: tracking in-flight
Lieferbeginn, Gerätewechsel, and billing processes as append-only event streams,
enforcing regulatory deadlines, and delivering outbound EDIFACT messages
atomically with domain events.

---

## Architecture Overview

```mermaid
graph TD
    subgraph Inbound["Inbound path"]
        AS4["AS4/ebMS3"]
        REST["HTTP REST"]
        AS4 & REST --> IB["InboxStore\n(dedup sentinel)"]
        IB --> PARSE["edi-energy\nparse + validate"]
        PARSE --> ROUTER["PidRouter\n(PID → handler)"]
    end

    subgraph Core["Process core (per workflow instance)"]
        ROUTER --> CMD["Command construction\n(at transport boundary)"]
        CMD --> PROC["Process&lt;W, S&gt;\nexecute / execute_and_enqueue"]
        PROC --> WH["Workflow::handle\n(pure)"]
        WH --> WA["Workflow::apply\n(pure)"]
        WA -->|"fold events"| STATE["State"]
    end

    subgraph Atomic["Atomic write (single WriteBatch)"]
        PROC -->|"append_with_outbox"| ES["EventStore\ne/ + sv/ + si/"]
        PROC -->|"append_with_outbox"| OMS["OutboxStore\nom/"]
    end

    subgraph Background["Background workers"]
        OW["OutboxWorker\n(continuous)"]
        DS["DeadlineScheduler\n(every 30 s)"]
        OMS -->|"pending"| OW
        OW -->|"AS4 SOAP"| PARTNER["Trading Partner MSH"]
        DS -->|"due_now"| DL["DeadlineStore\ndl/ + dt/"]
        DL -->|"TimeoutExpired"| PROC
    end

    subgraph Registry["Process Registry"]
        PR["ProcessRegistry\npr/ (routing)"]
        CI["ProcessRegistry\nci/ (correlated)"]
        PROC --> PR & CI
    end

    subgraph Partners["Partner Store"]
        PT["PartnerStore\npt/ — GLN → AS4 endpoint"]
        OW --> PT
    end
```

**Key invariants:**
- `Workflow::handle` and `Workflow::apply` are **pure functions** — no I/O, no clock, no global state mutation.
- All parsing, validation, and external lookups happen at the transport boundary, **before** the command is constructed.
- Events and outbox messages are always written in a **single `WriteBatch`** via `AtomicAppend::append_with_outbox`. Separate writes are not permitted on the production path.

---

## Core concepts

### `Workflow` trait

The central trait. Implementors define:

```rust
pub trait Workflow: Sized {
    type State:   Default + Clone;
    type Command: Send;
    type Event:   EventPayload;

    /// Produce events from the current state and a command.
    /// Must be pure — no I/O, no clock access.
    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError>;

    /// Fold one event into the current state.
    /// Must be pure — must never fail.
    fn apply(state: Self::State, event: &Self::Event) -> Self::State;
}
```

`WorkflowOutput<E>` carries both `events: Vec<E>` and `outbox: Vec<PendingOutbox>`. The `outbox` field holds lightweight descriptors for outbound EDIFACT messages; the engine stamps them with IDs, tenant, and timestamps during materialisation.

### `Process`

A handle to a specific workflow instance (one delivery point, one meter change, …):

```rust
// Spawn: creates a new empty stream
let process = ctx.spawn::<MyWorkflow>(tenant_id, workflow_id);

// Execute: replay → handle → atomic append
let envelopes = process.execute(command).await?;

// Execute with retry on VersionConflict (up to N attempts):
let envelopes = process.execute_with_retry(command, 3).await?;

// Execute and enqueue outbox atomically:
let envelopes = process.execute_and_enqueue(command).await?;
let envelopes = process.execute_and_enqueue_with_retry(command, 3).await?;

// Execute and get outbox messages back (for tests / rendering):
let (envelopes, outbox) = process.execute_and_collect(command).await?;

// Read the current state (full replay):
let state = process.state().await?;

// Read state with snapshot (O(k) replay from checkpoint):
let state = process.state_with_snapshot(&snapshot_store).await?;
```

`execute_with_retry` reloads the full event stream on each attempt — stale state is never carried forward into a retry.

`execute_and_collect` returns the fully-stamped [`OutboxMessage`] entries produced by `Workflow::handle`, with `causation_event_id` set to the `event_id` of the first persisted event — identical to what `execute_and_enqueue` writes into the `OutboxStore` atomically.  Use this in tests and render pipelines where you need the outbox messages after persisting without calling `handle()` a second time.

### `EngineBuilder`

Type-state builder that enforces a store is registered before `build()`:

```rust
use mako_engine::{builder::EngineBuilder, store_slatedb::SlateDbStore};

let store = SlateDbStore::open("/data/mako").await?;
let ctx   = EngineBuilder::new()
    .with_event_store(store)
    .with_dead_letter_sink(LogDeadLetterSink::new())
    .build();
```

`build()` is only callable when the `ES` type parameter implements `EventStore` — a missing store is a compile-time error.

---

## Event payload design

### Why `serde_json::Value` at the store boundary

`EventEnvelope::payload` is `serde_json::Value`. This is an intentional trade-off:

| Concern | Decision | Rationale |
|---|---|---|
| Schema evolution | `Value` — untyped at rest | Adding a field to `*Data` is backward-compatible: old events simply lack the key; `#[serde(default)]` fills in the default. Removing a key does not fail deserialization of new code against old events. |
| In-process type safety | Enforced by `apply()` | `apply()` deserializes the `Value` payload into the strongly-typed `*Event` enum via `serde_json::from_value`. Compile-time type checks apply within the domain crate. |
| Tamper detection | `deny_unknown_fields` on `*Data` | All domain `*Data` structs (carrier of business fields through state variants) are annotated with `#[serde(deny_unknown_fields)]`. This means `apply()` will return an error if a stored payload contains unrecognized keys, catching accidental field renames early. |
| Performance | Acceptable for current event depths | A benchmark for 10,000-event replay would quantify the allocation cost vs. a `Bytes`-based design. At current BDEW process depths (< 20 events per stream), the overhead is negligible. |

### Type safety guarantee

The guarantee is: **business correctness is enforced in `apply()`**, not at the store boundary.

```rust
// ✓ Type safety is enforced here — if the payload JSON is malformed
//   for the expected event type, apply() propagates the serde error.
fn apply(state: Self::State, event: &Self::Event) -> Self::State {
    // event was deserialized from Value by the engine before calling apply()
    match event {
        MyEvent::Initiated { location_id, .. } => { /* typed */ }
    }
}
```

Fields not present in a stored event payload (due to being added in a later schema version) are silently filled with their `#[serde(default)]` value. This is the supported upgrade path. Downgrade (removing a field from a newer schema and reading events written with the field) is handled by `deny_unknown_fields` failing loudly — a schema migration is required.

---

## Stores

All five store traits are implemented by `SlateDbStore`. In tests, swap in the `InMemoryEventStore` (and related noop/in-memory impls) via the `testing` feature flag.

### `EventStore`

Append-only stream of `EventEnvelope`s with optimistic concurrency via `ExpectedVersion`:

```rust
// Append at an exact version — returns VersionConflict if the stream moved:
store.append(&stream_id, ExpectedVersion::Exact(3), events).await?;

// Load from a sequence number:
store.load_from(&stream_id, from_sequence).await?;

// Constant-memory fold:
store.fold_stream(&stream_id, init, |acc, env| acc).await?;
```

Concurrent writers use SlateDB snapshot-isolation transactions: both writers stamp the `sv_key`, triggering a write-write conflict at commit time. Exactly one succeeds.

### `AtomicAppend`

Extends `EventStore` with a single method that writes events **and** outbox messages in one `WriteBatch`:

```rust
store.append_with_outbox(&stream_id, version, events, outbox).await?;
```

This is the only safe way to enqueue outbound EDIFACT — never write events first and outbox second.

### `OutboxStore`

FIFO delivery queue with exponential backoff and per-message jitter:

```rust
// Enqueue an outbound message:
store.enqueue(&outbox_msg).await?;

// Poll next batch for the delivery worker:
let batch = store.pending(tenant, limit).await?;

// Acknowledge delivery:
store.acknowledge(tenant, &message_id).await?;

// Reschedule after a transient failure (with backoff):
store.reschedule(tenant, &message_id, next_attempt_at).await?;
```

### `DeadlineStore`

Time-indexed store for regulatory Fristen:

```rust
// Register a deadline (GPKE 24h, WiM 5 Werktage, GeLi Gas 10 Werktage):
store.register(&deadline).await?;

// Poll deadlines due now:
let due = store.due_now(tenant, limit).await?;

// Cancel after successful dispatch or process termination:
store.cancel(tenant, &deadline_id).await?;
```

Deadlines that fire with a `VersionConflict` are **not cancelled** — the scheduler leaves them due for retry on the next poll cycle.

### `ProcessRegistry`

Reverse lookup from EDIFACT conversation ID → `ProcessIdentity`:

```rust
// Register when a new process is spawned:
registry.register(tenant, &conv_id, &process.identity()).await?;

// Look up on every subsequent inbound message for the same conversation:
let identity = registry.lookup(tenant, &conv_id).await?;
```

### `InboxStore`

Per-key deduplication for AS4 inbound messages. `accept` returns `true` only the first time a message ID is seen:

```rust
let is_new = inbox.accept(tenant, &message_id, ttl).await?;
```

Within a single process the store uses a per-key `DashMap<_, Arc<Mutex<()>>>` to serialise concurrent `accept` calls. Across multiple `makod` instances a distributed lock is required (not yet implemented).

---

## Regulatory Fristen (`fristen` module)

| Process family | Unit | Function |
|---|---|---|
| GPKE | 24 wall-clock hours | `fristen::add_hours(t, 24)` |
| WiM Strom | 5 Werktage | `fristen::add_werktage(d, 5, BdewMaKo)` |
| GeLi Gas | 10 Werktage | `fristen::add_werktage(d, 10, BdewMaKo)` |
| WiM Gas | 10 Werktage | `fristen::add_werktage(d, 10, BdewMaKo)` |
| MABIS | 1 Werktag (Prüfmitteilung) | `fristen::add_werktage(d, 1, BdewMaKo)` |

**Saturday is not a Werktag.** GPKE (BK6-24-174) Teil 1: *"alle Tage ..., die kein Samstag, Sonntag oder gesetzlicher Feiertag sind"*. A holiday observed in any single Bundesland counts nationwide, and 24.12. and 31.12. count as holidays.

Deadlines are always expressed as `17:00 Europe/Berlin` on the due date (not UTC). The `fristen` module uses `time_tz::assume_timezone(Europe/Berlin)` and the Anonymous Gregorian Easter algorithm for public holiday detection (valid for all years).

---

## Format-version coexistence

Every `WorkflowId` carries a BDEW format version (`FV2025-10-01`). A process started under one format version continues executing under those rules for its entire lifetime, even after the annual cutover.

```rust
// FV2025-10-01 process started in September 2025:
let wf_id = WorkflowId::new("supplier-change", "FV2025-10-01");
let process = ctx.spawn::<SupplierChangeWorkflow>(tenant, wf_id);

// Still runs under FV2025-10-01 rules in November 2025,
// even though FV2026-10-01 is now active for new processes.
```

`WorkflowVersionPolicy::ForwardCompatible` is the default for all MaKo workflows — **do not use `Pinned`** except for tests.

---

## Dead-letter sink

Unroutable or permanently failing messages are sent to a `DeadLetterSink`:

```rust
pub enum DeadLetterReason {
    UnknownPid { pid: u32, context: AuditContext },
    UnknownConversation { conversation_id: String, context: AuditContext },
    VersionMismatch { expected: String, received: String, context: AuditContext },
    DuplicateMessage { inbox_key: String, context: AuditContext },
    ProcessingError { message: String, context: AuditContext },
    OutboxExhausted { message_id: OutboxMessageId, message_type: String, recipient: String, last_error: String, attempts: u32 },
}
```

`AuditContext` carries § 147 AO / GoBD structured audit fields (`message_type`, `pid`, `sender_eic`, `receiver_eic`, `message_ref`, `process_id`, `tenant_id`, `correlation_id`, `timestamp`). Build it with the builder methods on `AuditContext::now()`.

`LogDeadLetterSink` (default) emits `tracing::warn!` with all available audit fields. `NoopDeadLetterSink` is available behind the `testing` feature. Implement `DeadLetterSink` to write to Prometheus, a database, or a page channel.

The `#[non_exhaustive]` attribute on `DeadLetterReason` allows new variants to be added without breaking existing sink implementations.

---

## Deadline-triggered commands (`Workflow::on_deadline`)

Workflows that own regulatory deadlines implement the `on_deadline` hook to turn
a fired deadline into a command:

```rust
impl Workflow for SupplierChangeWorkflow {
    // …

    fn on_deadline(label: &str, state: &Self::State) -> Option<Self::Command> {
        match label {
            "aperak-window" if !state.aperak_sent => {
                Some(SupplierChangeCommand::SendAperakTimeout)
            }
            _ => None,
        }
    }
}
```

The `DeadlineScheduler` calls `Process::execute_timeout` every 30 seconds on
each overdue deadline. The workflow produces an APERAK reject and appends it to
the outbox atomically — no manual timer management required.

```rust
// DeadlineScheduler internally calls:
let result = process.execute_timeout(&deadline).await?;
// or, with automatic retry on VersionConflict:
let result = process.execute_timeout_with_retry(&deadline, 3).await?;
```

---

## Correlated process lookup

Some inbound messages (e.g. CONTRL referencing multiple previous conversations)
need to locate more than one process at once. The **correlated registry** indexes
processes by an arbitrary tag, enabling one-to-many lookups:

```rust
// Register a process under a Bilanzkreis tag when it is spawned:
ctx.registry()
    .register_correlated(tenant, "bilanzkreis:DE-ABC-123", process_id, identity)
    .await?;

// Look up all processes for a given Bilanzkreis (e.g. on incoming MSCONS):
let identities = ctx.registry()
    .lookup_correlated(tenant, "bilanzkreis:DE-ABC-123")
    .await?;

// Remove when the process terminates:
ctx.registry()
    .remove_correlated(tenant, "bilanzkreis:DE-ABC-123", process_id)
    .await?;
```

The correlated index is stored under `ci/{tenant_id}/{tag}/{process_id}` in
SlateDB — a per-tenant, per-tag prefix scan returns all matches in a single
pass.

---

## Inbound correlation via `CorrelationId`

When an inbound EDIFACT message references a previous interchange (e.g. APERAK
referencing the original UTILMD), use `context_for_inbound` to stamp the command
context with a deterministic `CorrelationId` derived from the interchange
reference number. This ties the new command back to the originating interchange
in any tracing system without requiring a round-trip to the database:

```rust
// Derive a stable CorrelationId from the UNB interchange reference field:
let ctx_with_correlation = process.context_for_inbound(&interchange_ref);
let envelopes = process.execute_with_context(command, ctx_with_correlation).await?;
```

`CorrelationId::from_interchange_ref(ref)` uses UUID v5 with a fixed namespace,
so the same interchange reference always produces the same `CorrelationId` —
idempotent and deterministic.

---

## Format-version coexistence

`makod` runs **two BDEW format versions simultaneously** in production:

| Format version | Valid period | Status |
|---|---|---|
| `FV2025-10-01` | 2025-10-01 – 2026-09-30 | Current production |
| `FV2026-10-01` | from 2026-10-01 | Next release — profiles already deployed |

The `PidRouter` dispatches each inbound EDIFACT message to the correct profile
version based on the UNB `DE0031` date field (date of preparation) and the
transition window. A process that **starts** under `FV2025-10-01` continues under
those rules until it completes — even after the `FV2026-10-01` cutover date.

```mermaid
timeline
    title Format-version lifecycle (annual BDEW release cycle)
    section FV2025-10-01
        Oct 2025 : Deployed, active
        Oct 2026 : Superseded — new processes use FV2026
    section FV2026-10-01
        Oct 2026 : Goes live; FV2025 still handles in-flight processes
        Sep 2027 : All FV2025 processes complete; FV2025 retired
    section FV2027-10-01
        Oct 2027 : Next annual release
```

### `WorkflowVersionPolicy`

Every workflow must declare how it handles messages from a different format
version than the one it was started with:

```rust
impl Workflow for MyWorkflow {
    fn version_policy() -> WorkflowVersionPolicy {
        WorkflowVersionPolicy::ForwardCompatible   // ← required default for all MaKo
    }
}
```

| Policy | Meaning | When to use |
|---|---|---|
| `ForwardCompatible` | Accept newer-version messages during the old version's lifetime | **Default for all MaKo workflows** — e.g. a FV2025 process can receive a FV2026-encoded APERAK |
| `Pinned` | Reject messages from a different format version | Special-purpose only; never the default |
| `Strict` | Enforce exact version match | Testing only |

> **Do not default to `Pinned`.** A `Pinned` policy on any GPKE/WiM/GeLi Gas
> workflow will cause the process to reject APERAKs sent by counterparties that
> have already migrated to the next format version, silently breaking the workflow.

---

## Format-version migration (`migration` module)

When a new BDEW annual release (e.g. `FV2026-10-01`) changes a workflow's state
schema, in-flight processes that started under `FV2025-10-01` must be migrated
before the new rules can apply. The `MigrationRunner` handles this:

```rust
use mako_engine::migration::{MigrationRunner, StateMigration, MigrationReport};

struct UpgradeSupplierChange2025to2026;

impl StateMigration for UpgradeSupplierChange2025to2026 {
    type FromWorkflow = SupplierChangeWorkflowFV2025;
    type ToWorkflow   = SupplierChangeWorkflowFV2026;

    fn from_workflow_id() -> &'static str { "supplier-change/FV2025-10-01" }
    fn to_workflow_id()   -> &'static str { "supplier-change/FV2026-10-01" }

    fn migrate(state: OldState) -> Result<NewState, String> {
        Ok(NewState {
            // Map old fields to new schema
            status: state.status,
            new_field: Default::default(),
        })
    }
}

let runner = MigrationRunner::new(
    UpgradeSupplierChange2025to2026,
    event_store.clone(),
    snapshot_store.clone(),
);
let report: MigrationReport = runner.run().await;
println!(
    "Migrated {} processes, skipped {}, errors: {}",
    report.migrated, report.skipped, report.errors.len()
);
```

Migrations are intentionally **not fatal** — errors are collected in
`MigrationReport::errors` and the runner continues with the next stream.
Run migrations as an `xtask` or startup check before enabling the new workflow
version in production.

---

## Trading-partner master data (`PartnerStore`)

The `PartnerStore` trait provides a durable, PARTIN-aware registry of trading
partners. It replaces the static `HashMap<GLN, URL>` that config-only solutions
provide and survives restarts, runtime updates, and inbound PARTIN messages.

```rust
use mako_engine::partner::{PartnerRecord, CommunicationChannel, PartnerStore};
use mako_engine::store_slatedb::SlateDbStore;

let store = SlateDbStore::open("/data/mako").await?;
let partners = store.as_partner_store();

// Bootstrap from config (called by makod at startup):
for record in PartnerRecord::from_cli_pairs(&config.as4.partners)? {
    partners.upsert(tenant_id, &record).await?;
}

// Update from inbound PARTIN:
let incoming = parse_partin_message(edifact_bytes)?;
partners.upsert(tenant_id, &incoming).await?;

// Look up an AS4 endpoint before delivering:
let record = partners.get(tenant_id, &recipient_gln).await?
    .ok_or_else(|| EngineError::Partner(format!("no endpoint for {recipient_gln}")))?;
let endpoint = record.as4_endpoint()
    .ok_or_else(|| EngineError::Partner(format!("{recipient_gln} has no AS4 endpoint")))?;
```

### Partner record data model

```
PartnerRecord {
    gln:          MarktpartnerId,       // 13-digit Marktpartner-ID (partition key)
                                        // May be BDEW-Codenummer (99…), DVGW-Codenummer (98…),
                                        // or GS1 GLN — use nad_agency_code() to derive NAD DE3055
    display_name: Option<Box<str>>,     // NAD company name
    channels:     Vec<CommunicationChannel>,  // COM segments
      ├── qualifier "AK" → AS4 endpoint URL  (PARTIN AHB 1.0f DE 3155)
      ├── qualifier "EM" → email address
      ├── qualifier "TE" → telephone
      └── qualifier "FX" → fax
    roles:        Vec<MarketRole>,      // LfStrom, NbStrom, MsbStrom, …
    valid_from:   Option<OffsetDateTime>, // DTM+137
    contacts:     Vec<ContactPerson>,   // CTA/NAD/COM groups
    country_code: Option<Box<str>>,     // NAD country (e.g. "DE")
    updated_at:   OffsetDateTime,       // last write timestamp
}
```

`merge_from_partin` merges an incoming PARTIN record into the existing one. A
newer `valid_from` wins; config-bootstrapped records (no `valid_from`) are
always overwritten by PARTIN data.

Partners are managed at runtime via the REST admin API — see
[`makod` Operator Guide](./makod.md#partner-management-adminpartners).

---

## Domain crates

| Crate | Process family | Key inbound PIDs | APERAK Frist |
|---|---|---|---|
| `mako-gpke` | GPKE — Lieferbeginn/-ende Strom, ORDERS Sperrung (NB role), INVOIC billing, Konfiguration | 55001–55002, 55016–55018, 55555, 17115–17117 (NB inbound), 31001–31008, 17134/17135, 19001/19002 | **24 wall-clock hours** |
| `mako-wim` | WiM Strom — Messstellenwechsel, INSRPT Strom, WiM-Rechnung | 55039, 55042, 55051, 55168, 19001/19002, 23001/23003/23004/23008 | **5 Werktage** |
| `mako-geli-gas` | GeLi Gas — Lieferbeginn/-ende Gas, Gas Sperrung (LF role), Gas Datenabruf, INVOIC 31011 (AWH Sperrprozesse Gas) | 44001–44021, 17103, 17104, 19103, 19104, 19116, 19117, 19128, 19129, 31011 | **10 Werktage** |
| `mako-wim-gas` | WiM Gas — Messstellenwechsel Gas, INSRPT Gas, WiM-Rechnung Gas | 44039–44053, 44168–44170, 23005, 23009, 31003, 31004 | **10 Werktage** |
| `mako-mabis` | MABIS — Bilanzkreisabrechnung | 13003 (MSCONS Summenzeitreihe, IFTSTA 21000–21005) | n/a (batch, not saga) |
| `mako-gabi-gas` | GaBi Gas 2.1 — allocation, nomination, schedules, imbalance, transport (ALOCAT/NOMINT/NOMRES/SCHEDL/IMBNOT/TRANOT/DELORD/DELRES) + Kapazitäts-/Mehr-Mindermengen-INVOIC | INVOIC 31007/31008/31010, ORDERS 17110, ORDRSP 19110, MSCONS 13013, synthetic 90001–90062 | KoV deadlines (GasDay D-1 14:00 etc.) |

### MABIS architecture note

MABIS workflows are fundamentally different from supplier-switch workflows: instead of one stream per delivery point, MABIS aggregates meter data across thousands of MaLo streams for a billing period. This is driven by `ProjectionRunner::catch_up_persistent` rather than a per-MaLo `Process`. See [Projections and Read Models](#projections-and-read-models) below.

---

## Projections and Read Models

Projections build read models from the event stream. They are asynchronous, disposable (rebuildable), and eventually consistent. Projection failures must **never** affect event persistence.

### `catch_up_persistent` — the mandatory production API

`ProjectionRunner::catch_up_persistent` is the **only** projection entry point suitable for production background workers. It:

1. Loads the named checkpoint from `store` (cursor per stream from a previous run).
2. Performs incremental catch-up — feeds only events **newer** than the saved cursor.
3. Saves the updated checkpoint back atomically, writing only streams whose cursors advanced (O(changed_streams) writes).

On restart, only events appended since the last run are processed — **no full replay**. Over a live deployment with millions of events, full replay would be prohibitively expensive.

```rust,ignore
use mako_engine::{
    projection::{Projection, ProjectionRunner},
    store_slatedb::SlateDbStore,
};

// Implement the read-model builder:
struct BillingProjection { /* ... */ }
impl Projection for BillingProjection {
    fn name(&self) -> &'static str { "mabis-billing" }
    fn handle_event(&mut self, env: &EventEnvelope) { /* update read model */ }
    fn last_sequence(&self) -> Option<u64> { /* return cursor or None */ }
}

// Run the worker loop:
let store: Arc<SlateDbStore> = /* ... */;
let mut proj = BillingProjection::default();
loop {
    let _checkpoint = ProjectionRunner::catch_up_persistent(
        &mut proj,
        &store,
        Some("process/"),      // stream prefix — all process streams
        "mabis-billing",       // checkpoint name (unique per projection)
    ).await?;
    tokio::time::sleep(Duration::from_secs(30)).await;
}
```

### API surface

| Function | When to use |
|---|---|
| `catch_up_persistent(proj, store, prefix, name)` | **Production workers** — incremental, checkpoint-backed, restart-safe |
| `catch_up_all_streams(proj, store, streams, cp)` | In-process catch-up when you manage the checkpoint yourself |
| `run_all_streams(proj, store, streams)` | One-shot full replay (tests, diagnostic tools) |
| `run_from_store(proj, store, stream)` | Single-stream full replay (unit tests) |
| `catch_up_from_store(proj, store, stream)` | Single-stream incremental catch-up using `Projection::last_sequence` |

> **Warning**: `run_all_streams`, `run_matching_streams`, and `run_from_store` perform **full replays** from sequence 0. Never use them in a long-running production worker — use `catch_up_persistent` instead.

### Checkpoint store

`SlateDbStore` implements `ProjectionCheckpointStore`. The key space is:

```
cp/{checkpoint_name}/{stream_id}  →  u64 LE (8 bytes)
```

Each call to `catch_up_persistent` only writes streams whose cursors advanced, keeping write amplification at O(active_streams), not O(total_streams).

### `GlobalProjectionCheckpoint`

`GlobalProjectionCheckpoint` is a `BTreeMap<StreamId, u64>` — a per-stream sequence-number cursor. A cursor value of `0` means "never seen; replay from the beginning". The checkpoint is automatically serialised and deserialised by `catch_up_persistent`.

---

## `makod` production daemon

`makod` assembles all modules into a production-ready process. For the complete
configuration reference — all CLI flags, environment variables, TOML config,
Docker/Kubernetes deployment, secrets management, and health checks — see the
dedicated operator guide:

**[`makod` Operator Guide →](./makod.md)**

Quick start (development, volatile in-memory):

```bash
cargo run -p makod -- --config makod.toml --allow-volatile --http-addr 127.0.0.1:8080
```

`makod.toml` needs at least one `[[party]]` entry (`mp_id` + `roles`) — the
operator identity that scopes all process streams.

Omitting `--data-dir` starts in volatile in-memory mode — all process state is
lost on restart. A `WARN` is emitted at startup.

---

## Testing

Use the `testing` feature to swap in in-memory stores:

```rust
use mako_engine::{
    builder::EngineBuilder,
    event_store::InMemoryEventStore,
    dead_letter::NoopDeadLetterSink,
};

let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .with_dead_letter_sink(NoopDeadLetterSink)
    .build();
```

Each test should create its own `EngineBuilder` instance for isolation — in-memory stores share no global state.

### Bilateral E2E tests

For end-to-end pipeline tests that exercise the full
render → wire bytes → parse → adapt → execute chain, model each market
participant as a **mock ERP backend** struct that owns its own `Process` and
exposes protocol-level methods.

The key primitive is `Process::execute_and_collect` — it persists the command **and**
returns the stamped `OutboxMessage` entries in one call, ready to pass to
`render_to_wire_bytes` without any manual ID stitching:

```rust
// One call: persist + get outbox (no double handle() invocation).
let (_, outbox) = self.process
    .execute_and_collect(LfAnmeldungCommand::InitiateAnmeldung { .. })
    .await?;

// outbox[0] is already a fully-stamped OutboxMessage:
let wire = render_to_wire_bytes(&outbox[0], LFN_ID)?;
```

The returned `OutboxMessage` has `causation_event_id` set to the real
`event_id` of the persisted `Initiated` event — identical to what the
production `OutboxWorker` reads from the store when it delivers the message
over AS4.

#### Protocol sequence (Lieferbeginn Strom, PID 55001)

```mermaid
sequenceDiagram
    participant LFN as LFN ERP (MockLfn)
    participant NB  as NB ERP (MockNb)

    LFN->>LFN: execute_and_collect(InitiateAnmeldung)
    Note right of LFN: Initiated event persisted
    LFN->>LFN: assert outbox payload invariants
    LFN->>LFN: render_to_wire_bytes → UTILMD 55001
    LFN-->>NB: wire bytes (UTILMD 55001)

    NB->>NB: Platform::parse(wire)
    NB->>NB: assert UNH ref ≠ "1" (causation_event_id derivation)
    NB->>NB: gpke_registry().dispatch → ReceiveUtilmd
    NB->>NB: assert adapter preserved UNH ref (for APERAK)
    NB->>NB: execute(ReceiveUtilmd[validation_passed=true])
    Note right of NB: ValidationPassed state

    NB->>NB: execute_and_collect(SendAntwort { accepted: true })
    Note right of NB: AntwortGesendet event persisted
    NB->>NB: assert UTILMD 55003 + MSCONS 13015 in outbox
    NB->>NB: render_to_wire_bytes → UTILMD 55003
    NB-->>LFN: wire bytes (UTILMD 55003)

    LFN->>LFN: Platform::parse(wire)
    LFN->>LFN: gpke_lf_anmeldung_registry().dispatch → HandleAntwort
    LFN->>LFN: execute(HandleAntwort { accepted: true })
    Note left of LFN: Active state
    LFN->>LFN: assert Active data fields
```

See `services/makod/tests/e2e_lieferbeginn.rs` for the complete bilateral
implementation covering both the acceptance and rejection paths.

---

## See Also

- [Getting Started](./getting-started.md) — first steps for both layers
- [Platform Guide](./platform.md) — multi-tenant `edi-energy` usage
- [Parsing Guide](./parsing.md) — EDIFACT parsing
- [API-Webdienste Strom](./api-webdienste.md) — REST/JSON channel for iMS processes
- [Release Lifecycle](./release-lifecycle.md) — annual BDEW profile updates
