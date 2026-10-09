//! `mako-engine` — event-sourced process runtime for German energy market
//! communication (MaKo).
//!
//! # Architecture
//!
//! ```text
//! Raw EDIFACT bytes (AS4 transport)
//!         │
//!         ▼
//! [edi-energy] parse · validate
//!         │
//!         ▼  Command (typed, validated)
//! EngineContext::spawn / ::resume → Process::run(cmd, RunOptions)
//!         │
//!         ├─ load events → reconstruct state (Workflow::apply + upcast)
//!         ├─ handle command (Workflow::handle — pure, deterministic)
//!         └─ AtomicAppend::append — events, outbox, deadlines, keys in one batch
//!
//! EventStore ──► ProjectionRunner ──► Read models
//! SnapshotStore ──► Process::state_with_snapshot (O(k) replay)
//! OutboxStore ──► delivery worker ──► AS4 endpoint
//! DeadlineStore ──► scheduler ──► WorkflowDriver ──► Process::run_timeout
//! ProcessRegistry ──► inbound message routing ──► Process
//! ```
//!
//! # Quick start
//!
//! ```rust,ignore
//! use mako_engine::{
//!     builder::EngineBuilder,
//!     ids::TenantId,
//!     version::WorkflowId,
//!     event_store::InMemoryEventStore,
//! };
//!
//! let ctx = EngineBuilder::new()
//!     .with_event_store(InMemoryEventStore::new())
//!     .build();
//!
//! // Spawn a new process.
//! let process = ctx.spawn::<MyWorkflow>(TenantId::new(), WorkflowId::new("…", "FV2024-10-01"));
//! let ran = process.run(my_command, RunOptions::new()).await?;
//!
//! // Reconstruct typed state by replaying all events.
//! let state = process.state().await?;
//!
//! // Persist routing information and resume on the next message.
//! ctx.registry().register(tenant, &conv_id.to_string(), process.identity()).await?;
//! let identity = ctx.registry().lookup(tenant, &conv_id.to_string()).await?.unwrap();
//! let resumed  = ctx.resume::<MyWorkflow>(identity);
//! ```
//!
//! # Crate modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`ids`] | Typed identifier newtypes (`EventId`, `StreamId`, `ProcessId`, `ProcessIdentity`, `DeadlineId`, …) |
//! | [`types`] | Semantic domain identifiers (`MaLo`, `MeLo`, `MarktpartnerCode`, `MessageRef`, `DeviceId`, `BkvId`, `UenbId`, `BillingPeriod`) |
//! | [`version`] | `FormatVersion` and `WorkflowId` |
//! | [`envelope`] | `EventEnvelope` and `NewEvent` |
//! | [`error`] | `EngineError`, `WorkflowError` |
//! | [`event_store`] | `EventStore` trait (with `stream_version`) + `InMemoryEventStore` |
//! | [`workflow`] | `Workflow` trait, `EventPayload`, `CommandContext` |
//! | [`message_adapter`] | `MessageAdapter` trait, `AdapterRegistry`, `FnAdapter` — cross-FV command translation |
//! | [`process`] | `Process<W,S>` — ergonomic typed process handle |
//! | [`projection`] | `Projection` trait + `ProjectionRunner` (single-stream and multi-stream) + `GlobalProjectionCheckpoint` |
//! | [`snapshot`] | `Snapshot`, `SnapshotStore` + `InMemorySnapshotStore` / `NoopSnapshotStore` |
//! | [`outbox`] | `OutboxMessage`, `OutboxStore` + `InMemoryOutboxStore` / `NoopOutboxStore` |
//! | [`inbox`] | `InboxStore` trait + `InMemoryInboxStore` for AS4 retry deduplication |
//! | [`deadline`] | `Deadline`, `DeadlineStore` + `InMemoryDeadlineStore` / `NoopDeadlineStore` |
//! | [`registry`] | `ProcessRegistry` + `InMemoryProcessRegistry` / `NoopProcessRegistry` |
//! | [`pid_router`] | `PidRouter` — maps `Prüfidentifikator` values to workflow names |
//! | [`dead_letter`] | `DeadLetterSink` trait + `LogDeadLetterSink` / `NoopDeadLetterSink` |
//! | [`erp`] | `ErpAdapter`, `ErpEvent` — the outbound ERP/backend integration contract (BO4E) |
//! | [`builder`] | `EngineModule` trait, `EngineBuilder`, `EngineContext` |
//! | [`payload`] | Typed outbound payloads (`AperakPayload`, …) shared by producer and renderer |
//! | [`driver`] | `WorkflowDriver` — a workflow registered once, by type, under its name |

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic)]
#![expect(
    clippy::module_name_repetitions,
    reason = "a pedantic style lint this crate does not adopt"
)]
// BDEW domain terms (MaKo, GPKE, WiM, GeLi) and product names (PostgreSQL,
// SlateDB) are not code identifiers — suppress doc_markdown for the crate.
#![expect(
    clippy::doc_markdown,
    reason = "a pedantic style lint this crate does not adopt"
)]

pub mod builder;
pub mod dead_letter;
pub mod deadline;
pub mod driver;
pub mod envelope;
pub mod erc;
pub mod erp;
pub mod error;
pub mod event_store;
pub mod ids;
pub mod inbox;
pub mod lf_vorgang;
pub mod marktrolle;
pub mod message_adapter;
pub mod metrics;
pub mod migration;
pub mod outbox;
pub mod partner;
pub mod payload;
pub mod pid_router;
pub mod process;
pub mod profile;
pub mod projection;
pub mod registry;
pub mod snapshot;
#[cfg(feature = "slatedb")]
pub mod store_slatedb;
pub mod trace_ctx;
pub mod types;
pub mod version;
pub mod workflow;
