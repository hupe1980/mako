//! [`EngineModule`] trait, [`EngineBuilder`], and [`EngineContext`].
//!
// Allow using deprecated Noop stores as *type-level defaults* in EngineBuilder / EngineContext
// generic parameters.  The types are deprecated to prevent instantiation in production code,
// but using them as default type parameters in struct definitions (not instantiating them) is
// the intended pattern for the type-state builder API.
#![allow(deprecated)]
//!
//! # Summary
//!
//! `EngineBuilder` assembles all engine infrastructure into a single
//! [`EngineContext`] value. Domain modules (GPKE, WiM, GeLi Gas, …) register
//! themselves at startup via the [`EngineModule`] trait, making their names
//! visible in diagnostics and health checks.
//!
//! # Type-state guarantee
//!
//! [`EngineBuilder::build`] is only available when the event store type
//! parameter `ES` implements [`EventStore`]. Forgetting to call
//! [`with_event_store`] is a **compile-time error**, not a runtime panic.
//!
//! All other stores default to their respective `Noop` implementations:
//!
//! | Store | Default |
//! |-------|---------|
//! | Snapshot store | [`NoopSnapshotStore`] |
//! | Outbox store | [`NoopOutboxStore`] |
//! | Deadline store | [`NoopDeadlineStore`] |
//! | Process registry | [`NoopProcessRegistry`] |
//!
//! # Assembly example
//!
//! ```rust,ignore
//! use mako_engine::builder::{EngineBuilder, EngineModule};
//! use mako_engine::event_store::InMemoryEventStore;
//! use mako_engine::outbox::InMemoryOutboxStore;
//! use mako_engine::deadline::InMemoryDeadlineStore;
//! use mako_engine::registry::InMemoryProcessRegistry;
//! use mako_engine::snapshot::InMemorySnapshotStore;
//!
//! struct GpkeModule;
//! impl EngineModule for GpkeModule { fn name(&self) -> &'static str { "gpke" } }
//!
//! let ctx = EngineBuilder::new()
//!     .with_event_store(InMemoryEventStore::new())
//!     .with_snapshot_store(InMemorySnapshotStore::new())
//!     .with_outbox_store(InMemoryOutboxStore::new())
//!     .with_deadline_store(InMemoryDeadlineStore::new())
//!     .with_registry(InMemoryProcessRegistry::new())
//!     .register(Box::new(GpkeModule))
//!     .build();
//!
//! // Spawn a fresh process:
//! let p = ctx.spawn::<SupplierChangeWorkflow>(tenant_id, workflow_id);
//! p.execute(ReceiveUtilmd { .. }).await?;
//!
//! // Resume an existing process from a persisted identity:
//! let identity = ctx.registry.lookup(&conv_id.to_string()).await?.unwrap();
//! let p = ctx.resume::<SupplierChangeWorkflow>(identity);
//!
//! // Access stores for delivery workers / schedulers:
//! let pending = ctx.outbox_store.pending_now(50).await?;
//! let overdue = ctx.deadline_store.due_now(50).await?;
//! ```
//!
//! [`with_event_store`]: EngineBuilder::with_event_store

// Type-state generics can produce long signatures that trip up the
// `type_complexity` lint; suppress it for this module only.
#![allow(clippy::type_complexity)]

// The Noop* types are marked #[deprecated] to guard against accidental
// production use.  The builder is the only place they're instantiated as
// defaults; suppress the lint here explicitly.
#[allow(deprecated)]
use crate::{
    dead_letter::{DeadLetterSink, LogDeadLetterSink},
    deadline::{Deadline, DeadlineStore, NoopDeadlineStore},
    error::EngineError,
    event_store::EventStore,
    ids::{ProcessIdentity, TenantId},
    marktrolle::DeploymentRoles,
    outbox::{NoopOutboxStore, OutboxMessage, OutboxStore},
    pid_router::PidRouter,
    process::Process,
    registry::{NoopProcessRegistry, ProcessRegistry},
    snapshot::{NoopSnapshotStore, SnapshotStore},
    version::WorkflowId,
    workflow::Workflow,
};

use std::sync::Arc;

// ── EngineModule ──────────────────────────────────────────────────────────────

/// A self-contained domain module that registers with the engine at startup.
///
/// Domain crates implement this trait to declare their presence in the engine.
/// The module name is surfaced in [`EngineContext::registered_modules`] for
/// diagnostics, health checks, and log output.
///
/// ## Startup validation
///
/// Override [`configure`] to perform adapter coverage checks at engine startup
/// time. The engine calls [`configure`] for every registered module during
/// [`EngineBuilder::build`] and panics with an actionable message if any
/// module returns `Err`. This surfaces missing adapter registrations as a
/// startup failure rather than a silent runtime error.
///
/// ## Example
///
/// ```rust,ignore
/// pub struct GpkeModule;
///
/// impl EngineModule for GpkeModule {
///     fn name(&self) -> &'static str { "gpke" }
///
///     fn configure(&self) -> Result<(), String> {
///         // Validate that every known BDEW format version has an adapter:
///         GPKE_ADAPTER_REGISTRY
///             .validate_policy(&GpkeWorkflow::version_policy(), &KNOWN_FVS)
///             .map_err(|uncovered| format!(
///                 "gpke: missing adapters for format versions: {:?}",
///                 uncovered
///             ))
///     }
/// }
///
/// let ctx = EngineBuilder::new()
///     .with_event_store(my_store)
///     .register(Box::new(GpkeModule))
///     .build(); // panics if GpkeModule::configure returns Err
///
/// assert_eq!(ctx.registered_modules(), &["gpke"]);
/// ```
///
/// [`configure`]: EngineModule::configure
pub trait EngineModule: Send + 'static {
    /// Stable, unique name for this domain module.
    ///
    /// Used in diagnostics, health checks, and structured log output.
    /// Choose a short lowercase identifier (e.g. `"gpke"`, `"wim"`,
    /// `"geli"`).
    fn name(&self) -> &'static str;

    /// Register all PIDs this module handles into the shared [`PidRouter`].
    ///
    /// # Mutability contract
    ///
    /// This method is called **exactly once** by [`EngineBuilder::build`],
    /// before the resulting [`EngineContext`] is handed to the caller. The
    /// `&mut PidRouter` reference is only available here, at build time.
    /// After `build` returns the router is **sealed** — the engine provides
    /// only a shared `&PidRouter` reference, with no mutation path at runtime.
    ///
    /// Consequence: **all PIDs a module will ever need must be registered
    /// here**. Do not attempt to register PIDs lazily from async handlers or
    /// after the engine has started — there is no API for that by design.
    ///
    /// Duplicate registrations (same PID from two modules) silently overwrite
    /// the previous mapping; the last module to register wins. Use
    /// `cargo xtask validate-pruefids` to catch accidental PID conflicts
    /// between modules before they reach production.
    ///
    /// For role-conditional registration (PIDs that should only be active for
    /// specific BDEW Marktrollen), override [`register_pids_with_roles`] instead.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// fn register_pids(&self, router: &mut PidRouter) {
    ///     // GPKE Lieferantenwechsel / Lieferbeginn (BK6-22-024, PIDs 55001, 55002, 55017)
    ///     for &pid in &[55001_u32, 55002, 55017] {
    ///         router.register(pid, "gpke-supplier-change");
    ///     }
    /// }
    /// ```
    ///
    /// [`register_pids_with_roles`]: EngineModule::register_pids_with_roles
    fn register_pids(&self, _router: &mut PidRouter) {}

    /// Register PIDs with role-context awareness.
    ///
    /// This is the **preferred override** for modules that have role-conditional
    /// PID registrations — PIDs that should only be active when this `makod`
    /// instance holds a specific [`Marktrolle`].
    ///
    /// The default implementation calls [`register_pids`] (role-agnostic) so
    /// existing modules that override `register_pids` continue to work without
    /// changes.
    ///
    /// Override this method instead of `register_pids` when any PID registration
    /// should be conditional on the deployment role:
    ///
    /// ```rust,ignore
    /// use mako_engine::marktrolle::Marktrolle;
    ///
    /// fn register_pids_with_roles(&self, router: &mut PidRouter, roles: &DeploymentRoles) {
    ///     // Always register: 55001, 55002 (not role-specific)
    ///     for pid in [55001_u32, 55002] { router.register_with_module(pid, "gpke-supplier-change", self.name()); }
    ///
    ///     // Only when NB role: 19001/19002 inbound ORDRSP from MSB
    ///     if roles.contains(Marktrolle::Nb) {
    ///         for pid in [19001_u32, 19002] { router.register_with_module(pid, "gpke-konfiguration", self.name()); }
    ///     }
    /// }
    /// ```
    ///
    /// # Conflict guard
    ///
    /// Use [`PidRouter::register_with_module`] (not `register`) inside this
    /// method. The conflict guard panics at build time if two modules register
    /// the same PID to different workflows — this makes role misconfigurations
    /// visible at startup rather than silently misrouting messages.
    ///
    /// [`Marktrolle`]: crate::marktrolle::Marktrolle
    /// [`register_pids`]: EngineModule::register_pids
    fn register_pids_with_roles(&self, router: &mut PidRouter, _roles: &DeploymentRoles) {
        self.register_pids(router);
    }

    /// Workflow names this module handles for deadline dispatch.
    ///
    /// Return the same name strings that [`register_pids`] maps PIDs to.
    /// These names are stored in [`EngineContext::registered_workflows`] and
    /// used to validate that every workflow that has deadlines scheduled is
    /// covered by the deadline scheduler dispatch function at runtime.
    ///
    /// The default implementation returns an empty slice. Override it to
    /// declare all workflow names that may fire deadlines:
    ///
    /// ```rust,ignore
    /// fn workflow_names(&self) -> &'static [&'static str] {
    ///     &["gpke-supplier-change", "gpke-abrechnung"]
    /// }
    /// ```
    ///
    /// [`register_pids`]: EngineModule::register_pids
    /// [`EngineContext::registered_workflows`]: crate::builder::EngineContext::registered_workflows
    fn workflow_names(&self) -> &'static [&'static str] {
        &[]
    }

    /// Declare the EDIFACT profile types this module requires at runtime.
    ///
    /// Returning a non-empty slice causes [`EngineBuilder::build`] to call the
    /// registered profile validator for each requirement.  If no active profile
    /// exists for a required message type, `build` panics with an actionable
    /// error so deployment fails fast rather than silently.
    ///
    /// **This replaces the previous pattern** of calling
    /// `edi_energy::registry::ReleaseRegistry::global()` inside `configure()`.
    /// Domain crates no longer need `edi-energy` in their production
    /// `[dependencies]` — they just declare their requirements here.
    ///
    /// ```rust,ignore
    /// fn profile_requirements(&self) -> &'static [ProfileRequirement] {
    ///     &[
    ///         ProfileRequirement { message_type: "UTILMD", label: "UTILMD Strom (GPKE)" },
    ///         ProfileRequirement { message_type: "INVOIC", label: "INVOIC Abrechnung (GPKE)" },
    ///     ]
    /// }
    /// ```
    ///
    /// [`ProfileRequirement`]: crate::profile::ProfileRequirement
    fn profile_requirements(&self) -> &'static [crate::profile::ProfileRequirement] {
        &[]
    }

    /// Validate adapter coverage and configuration at engine startup.
    ///
    /// Called by [`EngineBuilder::build`] after all modules are registered.
    /// Return `Ok(())` when the module is fully configured. Return `Err(msg)`
    /// with an actionable description when an adapter or configuration is
    /// missing — the engine will panic with that message so the deployment
    /// fails early rather than silently.
    ///
    /// The default implementation is a no-op (always returns `Ok(())`).
    /// Override it in domain crates to call
    /// [`AdapterRegistry::validate_policy`] and emit structured errors.
    ///
    /// Note: if your validation needs access to the edi-energy profile
    /// registry, use [`profile_requirements`] instead — it does not require
    /// importing `edi-energy` in domain crates.
    ///
    /// [`AdapterRegistry::validate_policy`]: crate::message_adapter::AdapterRegistry::validate_policy
    /// [`profile_requirements`]: EngineModule::profile_requirements
    ///
    /// # Errors
    ///
    /// Returns a descriptive error string when the module's configuration is invalid.
    fn configure(&self) -> Result<(), String> {
        Ok(())
    }
}

// ── EngineContext ─────────────────────────────────────────────────────────────

/// Assembled engine infrastructure returned by [`EngineBuilder::build`].
///
/// `EngineContext` bundles all stores and the process registry into a single
/// value. It is the root dependency for:
///
/// - Spawning new processes ([`spawn`])
/// - Resuming existing processes ([`resume`])
/// - Running outbox delivery workers (`outbox_store.pending_now(…)`)
/// - Driving the deadline scheduler (`deadline_store.due_now(…)`)
///
/// ## Generic parameters
///
/// | Param | Role | Default |
/// |-------|------|---------|
/// | `ES`  | [`EventStore`] backend | — (required) |
/// | `SS`  | [`SnapshotStore`] backend | [`NoopSnapshotStore`] |
/// | `OS`  | [`OutboxStore`] backend  | [`NoopOutboxStore`]   |
/// | `DS`  | [`DeadlineStore`] backend | [`NoopDeadlineStore`] |
/// | `PR`  | [`ProcessRegistry`] backend | [`NoopProcessRegistry`] |
///
/// In most codebases all type parameters are inferred from the builder calls.
///
/// [`spawn`]: EngineContext::spawn
/// [`resume`]: EngineContext::resume
pub struct EngineContext<
    ES,
    SS = NoopSnapshotStore,
    OS = NoopOutboxStore,
    DS = NoopDeadlineStore,
    PR = NoopProcessRegistry,
> {
    event_store: Arc<ES>,
    snapshot_store: SS,
    outbox_store: OS,
    deadline_store: DS,
    registry: PR,
    /// Dead-letter sink for unroutable or unprocessable inbound messages.
    ///
    /// Stored as `Arc<dyn DeadLetterSink>` so callers can share it across
    /// tasks without an extra type parameter on `EngineContext`.
    pub dead_letter_sink: Arc<dyn DeadLetterSink>,
    /// PID-to-workflow routing table, populated from all registered modules.
    pid_router: PidRouter,
    registered_modules: Vec<&'static str>,
    /// Workflow names declared by all registered modules via
    /// [`EngineModule::workflow_names`]. Used to validate deadline scheduler
    /// coverage at runtime (see [`EngineContext::registered_workflows`]).
    registered_workflows: Vec<&'static str>,
}

// ── Type aliases ──────────────────────────────────────────────────────────────

/// An [`EngineContext`] with all optional subsystems disabled.
///
/// Uses `NoopSnapshotStore` and, in `testing`-enabled builds, Noop
/// implementations for outbox, deadline, and process registry. Suitable for
/// tests and minimal deployments where only a durable event store is required.
///
/// All five type parameters are inferred from context when used with
/// [`EngineBuilder`]:
///
/// ```rust,ignore
/// // Only available in test / testing-feature builds:
/// use mako_engine::builder::{EngineBuilder, MinimalEngine};
/// use mako_engine::event_store::InMemoryEventStore;
///
/// let ctx: MinimalEngine<InMemoryEventStore> = EngineBuilder::new()
///     .with_event_store(InMemoryEventStore::new())
///     .build();
/// ```
pub type MinimalEngine<ES> = EngineContext<ES>;

impl<ES, SS, OS, DS, PR> EngineContext<ES, SS, OS, DS, PR>
where
    ES: EventStore,
{
    /// Spawn a new process and return a typed `Process<W, Arc<ES>>` handle.
    ///
    /// No `ES: Clone` bound is required — the engine stores the event store
    /// behind an `Arc` so spawning is always a cheap pointer clone.
    ///
    /// ```rust,ignore
    /// let p = ctx.spawn::<SupplierChangeWorkflow>(tenant_id, workflow_id);
    /// p.execute(ReceiveUtilmd { .. }).await?;
    /// ```
    #[must_use]
    pub fn spawn<W: Workflow>(
        &self,
        tenant_id: TenantId,
        workflow_id: WorkflowId,
    ) -> Process<W, Arc<ES>> {
        Process::new(Arc::clone(&self.event_store), tenant_id, workflow_id)
    }

    /// Resume an existing process from a [`ProcessIdentity`].
    ///
    /// ```rust,ignore
    /// let identity = ctx.registry()
    ///     .lookup(tenant_id, &conv_id.to_string())
    ///     .await?
    ///     .ok_or(EngineError::Registry("unknown conversation".into()))?;
    /// let p = ctx.resume::<SupplierChangeWorkflow>(identity);
    /// p.execute(HandleAperak { .. }).await?;
    /// ```
    #[must_use]
    pub fn resume<W: Workflow>(&self, identity: ProcessIdentity) -> Process<W, Arc<ES>> {
        Process::from_identity(Arc::clone(&self.event_store), identity)
    }

    /// Names of all domain modules registered with the builder, in
    /// registration order.
    #[must_use]
    pub fn registered_modules(&self) -> &[&'static str] {
        &self.registered_modules
    }

    /// Workflow names declared by all registered modules, in registration order.
    ///
    /// Use this in the deadline scheduler dispatch function to detect unknown
    /// workflow names at startup. If a deadline fires for a workflow name that
    /// is not in this list, the scheduler's dispatch function should emit an
    /// error rather than silently dropping the deadline:
    ///
    /// ```rust,ignore
    /// let known = ctx.registered_workflows().iter().copied().collect::<HashSet<_>>();
    /// let scheduler = ctx.run_deadline_scheduler(
    ///     move |deadline| {
    ///         let wf = deadline.workflow_id().name.as_ref();
    ///         if !known.contains(wf) {
    ///             tracing::error!(workflow = %wf, "deadline fired for unregistered workflow");
    ///             return Box::pin(async { Ok(()) });
    ///         }
    ///         // dispatch by workflow name …
    ///         Box::pin(async { Ok(()) })
    ///     },
    ///     100,
    ///     Duration::from_secs(30),
    /// );
    /// ```
    #[must_use]
    pub fn registered_workflows(&self) -> &[&'static str] {
        &self.registered_workflows
    }

    /// The event store backend (behind an `Arc`).
    #[must_use]
    pub fn event_store(&self) -> &Arc<ES> {
        &self.event_store
    }

    /// The snapshot store backend.
    #[must_use]
    pub fn snapshot_store(&self) -> &SS {
        &self.snapshot_store
    }

    /// The outbox store backend.
    ///
    /// Poll `outbox_store().pending_now(limit)` in a background task to drain
    /// the delivery queue.
    #[must_use]
    pub fn outbox_store(&self) -> &OS {
        &self.outbox_store
    }

    /// The deadline store backend.
    ///
    /// Poll `deadline_store().due_now(limit)` in a background scheduler to
    /// fire overdue process timers.
    #[must_use]
    pub fn deadline_store(&self) -> &DS {
        &self.deadline_store
    }

    /// The process routing registry.
    ///
    /// Register a [`ProcessIdentity`] under a `(tenant_id, key)` pair at
    /// process creation, then `lookup` it when routing inbound messages.
    #[must_use]
    pub fn registry(&self) -> &PR {
        &self.registry
    }

    /// The dead-letter sink for unroutable or unprocessable messages.
    ///
    /// Call [`DeadLetterSink::reject`] when an inbound message cannot be
    /// dispatched to any workflow. The default sink emits `tracing::warn!`
    /// so rejections are always visible in the log output.
    #[must_use]
    pub fn dead_letter_sink(&self) -> &Arc<dyn DeadLetterSink> {
        &self.dead_letter_sink
    }

    /// Assert that no Noop store is active — call this during production startup.
    ///
    /// Checks the type names of `OS`, `DS`, and `PR` against the string `"Noop"`.
    /// Panics with a human-readable message if any match, directing the operator
    /// to configure a persistent backend.
    ///
    /// # When to call
    ///
    /// Call this early in `makod`'s startup path (and `--check` mode) to catch
    /// deployments where a Noop store was accidentally wired — e.g. the
    /// `[outbox]`, `[deadline]`, or `[registry]` configuration section was
    /// omitted from `makod.toml`.  The check is defence-in-depth: in release
    /// builds without the `testing` feature, Noop stores cannot implement the
    /// required traits at all and the compiler would have already rejected them.
    ///
    /// # Panics
    ///
    /// Panics when any of `OS`, `DS`, or `PR` is a Noop implementation.
    pub fn assert_production_stores(&self) {
        let checks: &[(&str, &str)] = &[
            ("OutboxStore", std::any::type_name::<OS>()),
            ("DeadlineStore", std::any::type_name::<DS>()),
            ("ProcessRegistry", std::any::type_name::<PR>()),
        ];
        for (trait_name, type_name) in checks {
            assert!(
                !type_name.contains("Noop"),
                "makod: Noop{trait_name} is active — \
                 configure a persistent {trait_name} backend in makod.toml. \
                 Type resolved to: {type_name}"
            );
        }
    }

    /// The PID-to-workflow routing table.
    ///
    /// Populated **once** during [`EngineBuilder::build`] by calling
    /// [`EngineModule::register_pids`] on every registered module in
    /// registration order. After `build` returns the table is **sealed** —
    /// it is read-only for the lifetime of the `EngineContext` and may be
    /// freely shared across async tasks without synchronisation.
    ///
    /// # Mutability contract
    ///
    /// There is intentionally no `pid_router_mut()` accessor. Adding PIDs
    /// after the engine is built would create a TOCTOU race between the
    /// dispatch path (which calls `route(pid)`) and any hypothetical
    /// concurrent mutator. Instead, register all PIDs during the build phase
    /// via `EngineModule::register_pids`.
    ///
    /// If a new process family needs to be added without restarting the
    /// binary, rebuild and restart `makod` — hot-swap of PID routing is not
    /// supported.
    ///
    /// # Example — dispatch at the AS4 reception boundary
    ///
    /// ```rust,ignore
    /// let workflow_name = ctx.pid_router().route(pid)
    ///     .ok_or_else(|| EngineError::Workflow(WorkflowError::InvalidCommand(
    ///         format!("no workflow registered for PID {pid}").into()
    ///     )))?;
    ///
    /// match workflow_name {
    ///     "gpke-supplier-change" => dispatch::<GpkeSupplierChangeWorkflow>(&ctx, pid, payload).await,
    ///     "wim-device-change"    => dispatch::<WimDeviceChangeWorkflow>(&ctx, pid, payload).await,
    ///     other => Err(EngineError::Workflow(WorkflowError::InvalidCommand(
    ///         format!("unhandled workflow name: {other}").into()
    ///     ))),
    /// }
    /// ```
    #[must_use]
    pub fn pid_router(&self) -> &PidRouter {
        &self.pid_router
    }
}

// ── As4Sender ─────────────────────────────────────────────────────────────────

/// Sends a single AS4 / EDIINT-over-HTTP outbound message.
///
/// Implement this trait for your AS4 gateway client and pass it to
/// [`EngineContext::run_outbox_worker`].
///
/// # Contract
///
/// Return `Ok(())` only after the message has been **durably accepted** by the
/// receiving MSH.  Return `Err(…)` on transient or permanent failure — the
/// outbox worker calls [`OutboxStore::reschedule`] so the message is retried.
pub trait As4Sender: Send + Sync + 'static {
    /// Transmit `msg` and return when the remote MSH has accepted it.
    fn send(
        &self,
        msg: &OutboxMessage,
    ) -> impl std::future::Future<Output = Result<(), EngineError>> + Send;
}

// ── OutboxWorker ──────────────────────────────────────────────────────────────

/// A background worker that drains the outbox by polling pending
/// [`OutboxMessage`]s and dispatching them via an [`As4Sender`].
///
/// Obtain via [`EngineContext::run_outbox_worker`] and drive by spawning
/// [`OutboxWorker::run`] in a Tokio task.
///
/// # Polling behaviour
///
/// When the poll returns an empty batch the worker sleeps for `poll_interval`
/// before polling again.  Non-empty batches are processed immediately.
///
/// # Error handling
///
/// Successful sends are acknowledged via [`OutboxStore::acknowledge`].
/// Failed sends are rescheduled via [`OutboxStore::reschedule`] using
/// **full-jitter exponential backoff**: `delay = rand(0, min(MAX, BASE * 2^n))`
/// where `n = attempt_count`. This avoids thundering-herd when multiple
/// `makod` instances restart simultaneously after a receiver outage.
///
/// When `attempt_count >= max_attempts`, the message is **acknowledged** (removed
/// from the outbox) and a [`DeadLetterReason::OutboxExhausted`] record is written
/// to the dead-letter sink. This prevents permanently-undeliverable messages
/// from clogging the outbox forever.
///
/// All errors are emitted as structured `tracing` events at `warn` / `error`
/// level rather than `eprintln!`, so they appear in the application's log
/// pipeline with full context (message_id, error).
///
/// # Example
///
/// ```rust,ignore
/// use std::time::Duration;
///
/// let worker = ctx.run_outbox_worker(my_sender, 50, Duration::from_secs(1));
/// tokio::spawn(async move { worker.run().await });
/// ```
///
/// [`DeadLetterReason::OutboxExhausted`]: crate::dead_letter::DeadLetterReason::OutboxExhausted
pub struct OutboxWorker<OS: OutboxStore, S: As4Sender, DS: DeadlineStore> {
    store: OS,
    sender: S,
    /// Used to discharge a delivery-window deadline once the message it was
    /// watching has actually been sent — see [`OutboxWorker::run`].
    deadline_store: DS,
    batch_size: usize,
    poll_interval: std::time::Duration,
    /// Maximum total delivery attempts before a message is dead-lettered — a
    /// runaway belt, not the budget. The budget is [`Self::max_retry_window`]:
    /// the backoff is full-jitter, so an attempt *count* cannot promise a
    /// retry *duration*, and the BDEW retry duty is stated in hours.
    max_attempts: u32,
    /// Maximum age (from `created_at`) a message is retried for before it is
    /// dead-lettered. This is what honours a time-stated retry duty (BDEW AS4
    /// Kommunikationshandbuch: 72 h for unacknowledged messages) — see
    /// `mako_as4::constants::MAX_RETRY_DURATION_SECS`.
    ///
    /// Checked only after at least one attempt: a message that aged in a
    /// stopped worker still gets its first try rather than being buried
    /// unsent.
    max_retry_window: std::time::Duration,
    /// Sink for messages that exceed `max_attempts` or `max_retry_window`.
    dead_letter_sink: std::sync::Arc<dyn crate::dead_letter::DeadLetterSink>,
    /// Optional liveness heartbeat — stores the current UTC Unix timestamp
    /// (seconds) after each poll cycle so health probes can detect stale workers.
    heartbeat: Option<std::sync::Arc<std::sync::atomic::AtomicI64>>,
    /// Graceful-shutdown signal. When cancelled the worker finishes the message
    /// it is delivering, then returns from [`OutboxWorker::run`] — see
    /// [`OutboxWorker::with_shutdown`].
    shutdown: Option<tokio_util::sync::CancellationToken>,
}

/// Sleep for `dur`, returning early if `token` is cancelled.
///
/// Returns `true` when the sleep completed and the caller should keep looping,
/// `false` when the token was cancelled and the caller must return.
///
/// A worker that sleeps on a bare `tokio::time::sleep` cannot observe a
/// shutdown until its poll interval elapses. For the deadline scheduler that is
/// 30 seconds by default — longer than a typical container termination grace
/// period, which turns a graceful drain into a SIGKILL.
///
/// Public so that binaries running their own poll-loop workers alongside the
/// engine's (projection catch-up, webhook delivery, retention purges) can honour
/// the same token and stop before the store is closed.
pub async fn sleep_or_cancel(
    dur: std::time::Duration,
    token: Option<&tokio_util::sync::CancellationToken>,
) -> bool {
    let Some(t) = token else {
        tokio::time::sleep(dur).await;
        return true;
    };
    tokio::select! {
        () = tokio::time::sleep(dur) => true,
        () = t.cancelled() => false,
    }
}

/// Compute a full-jitter exponential backoff delay.
///
/// `attempt` is the number of prior attempts (0 = first retry).
/// `entropy` provides randomness; derive from a stable message identifier
/// (e.g. hash of `message_id`) rather than the current timestamp — a
/// timestamp-derived value is deterministic within a single batch, which
/// defeats jitter when multiple messages fail simultaneously.
///
/// | attempt | window (s) | expected delay (s) |
/// |---------|------------|-------------------|
/// | 0       | 5          | 2.5               |
/// | 1       | 10         | 5                 |
/// | 2       | 20         | 10                |
/// | 3       | 40         | 20                |
/// | 4       | 80         | 40                |
/// | 5+      | 300 (cap)  | 150               |
fn backoff_delay(attempt: u32, entropy: u64) -> std::time::Duration {
    const BASE_SECS: u64 = 5;
    const MAX_SECS: u64 = 300;
    // Exponential window: BASE * 2^attempt, capped at MAX.
    let window = BASE_SECS
        .saturating_mul(1u64.wrapping_shl(attempt.min(5)))
        .min(MAX_SECS);
    // Full jitter: uniform random in [0, window).
    let jitter_secs = if window == 0 { 0 } else { entropy % window };
    std::time::Duration::from_secs(jitter_secs)
}

impl<OS: OutboxStore, S: As4Sender, DS: DeadlineStore> OutboxWorker<OS, S, DS> {
    /// Run the outbox drain loop until the shutdown token is cancelled.
    ///
    /// Without a token (see [`OutboxWorker::with_shutdown`]) the loop runs until
    /// the task is aborted or the process exits. With one, cancellation is
    /// observed between messages and during the idle sleep, so an in-flight
    /// delivery is always finished and acknowledged before the worker returns —
    /// dropping it mid-`send` would risk a duplicate AS4 delivery on restart.
    ///
    /// # Panics
    ///
    /// Panics if `time::Duration::try_from(delay)` overflows (unreachable for
    /// the delay values produced by `backoff_delay`).
    #[allow(clippy::too_many_lines)]
    pub async fn run(self) {
        loop {
            if self
                .shutdown
                .as_ref()
                .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
            {
                tracing::info!("outbox worker: shutdown signalled; stopping");
                return;
            }
            // Tick liveness at the *start* of every poll cycle, ahead of the
            // early-`continue` paths below.  An idle worker (empty outbox) and
            // one retrying after a store error are both alive and must keep
            // ticking; only a worker genuinely hung inside an `.await` stops.
            if let Some(ref hb) = self.heartbeat {
                hb.store(
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }

            let batch = match self.store.pending_now(self.batch_size).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "outbox worker: store error polling pending messages (will retry)");
                    if !sleep_or_cancel(self.poll_interval, self.shutdown.as_ref()).await {
                        return;
                    }
                    continue;
                }
            };

            if batch.is_empty() {
                if !sleep_or_cancel(self.poll_interval, self.shutdown.as_ref()).await {
                    return;
                }
                continue;
            }

            for msg in batch {
                // Between messages, not inside one: a `send` that is already in
                // flight must run to its `acknowledge`, or the counterparty
                // receives a message the outbox still believes is pending and
                // redelivers it after the restart.
                if self
                    .shutdown
                    .as_ref()
                    .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
                {
                    tracing::info!(
                        "outbox worker: shutdown signalled mid-batch; \
                         remaining messages stay queued for the next start"
                    );
                    return;
                }
                // ── Retry budget ──────────────────────────────────────
                // `attempt_count` starts at 0 and is incremented on each
                // `reschedule` call. The message is permanently undeliverable
                // when the retry *window* has elapsed (the BDEW duty is stated
                // in hours, and full-jitter backoff makes a count no proxy for
                // a duration) or the attempt belt is exhausted: acknowledge it
                // (remove from outbox) and dead-letter it so the regulatory
                // audit trail is preserved. The window is only consulted after
                // a first attempt, so a message that aged while the worker was
                // down still gets tried once.
                let age = time::OffsetDateTime::now_utc() - msg.created_at;
                let window_elapsed = msg.attempt_count > 0
                    && age
                        >= time::Duration::try_from(self.max_retry_window)
                            .unwrap_or(time::Duration::hours(72));
                if msg.attempt_count >= self.max_attempts || window_elapsed {
                    tracing::error!(
                        message_id   = %msg.message_id,
                        message_type = %msg.message_type,
                        recipient    = %msg.recipient,
                        attempts     = msg.attempt_count,
                        max_attempts = self.max_attempts,
                        age_secs     = age.whole_seconds(),
                        window_secs  = self.max_retry_window.as_secs(),
                        "outbox worker: retry budget exhausted; dead-lettering message",
                    );
                    self.dead_letter_sink.reject(
                        &crate::dead_letter::DeadLetterReason::OutboxExhausted {
                            message_id: msg.message_id,
                            message_type: msg.message_type.to_string(),
                            recipient: msg.recipient.to_string(),
                            last_error: format!(
                                "delivery exhausted after {} attempts",
                                msg.attempt_count
                            ),
                            attempts: msg.attempt_count,
                        },
                    );
                    if let Err(e) = self.store.acknowledge(msg.message_id).await {
                        tracing::error!(
                            message_id = %msg.message_id,
                            error = %e,
                            "outbox worker: acknowledge after exhaust failed; message may reappear",
                        );
                    }
                    continue;
                }

                match self.sender.send(&msg).await {
                    Ok(()) => {
                        if let Err(e) = self.store.acknowledge(msg.message_id).await {
                            tracing::warn!(
                                message_id = %msg.message_id,
                                error = %e,
                                "outbox worker: acknowledge failed",
                            );
                        }
                        // CONTRL AHB 1.0 §1.2: the CONTRL must be delivered
                        // within 6 wall-clock hours of interchange receipt.
                        // `msg.created_at` is when the PendingOutbox was
                        // materialised (which should equal the ingest timestamp
                        // for transport-layer CONTRL obligations).
                        if msg.message_type.as_ref() == "CONTRL" {
                            let elapsed = time::OffsetDateTime::now_utc() - msg.created_at;
                            if elapsed > time::Duration::hours(mako_fristen::CONTRL_FRIST_HOURS) {
                                tracing::warn!(
                                    message_id   = %msg.message_id,
                                    elapsed_secs = elapsed.whole_seconds(),
                                    max_secs     = mako_fristen::CONTRL_FRIST_HOURS * 3600,
                                    "outbox worker: CONTRL delivered OUTSIDE the 6h Übertragungsfrist \
                                     (CONTRL AHB 1.0 §1.2) — this is a BNetzA compliance violation"
                                );
                            }
                        }
                        // APERAK AHB 1.0 §2.4.1: Strom UTILMD/ORDERS APERAK must be
                        // delivered within 45 minutes on weekdays, or by Sunday 12:00
                        // if received on Saturday.  Log a compliance warning if the
                        // delivery window was missed so operators can investigate.
                        if msg.message_type.as_ref() == "APERAK" {
                            let elapsed = time::OffsetDateTime::now_utc() - msg.created_at;
                            if elapsed
                                > time::Duration::minutes(
                                    mako_fristen::APERAK_STROM_WEEKDAY_MINUTES,
                                )
                            {
                                tracing::warn!(
                                    message_id   = %msg.message_id,
                                    elapsed_mins = elapsed.whole_minutes(),
                                    "outbox worker: APERAK delivered after the 45-minute Strom \
                                     sending window (APERAK AHB 1.0 §2.4.1) — \
                                     check OutboxWorker and AS4 transport health"
                                );
                            }
                        }
                        // The message is out, so any delivery window that was
                        // watching for it has been answered — retire it.
                        //
                        // Nothing else cancels these. A monitoring deadline that
                        // outlives the obligation it monitors fires for every
                        // process, including every one that answered on time,
                        // and the scheduler cannot tell those apart because a
                        // deadline reaching `due_now` is late by construction.
                        // Leaving them registered turns the miss counters into
                        // counts of *processes started*.
                        self.discharge_delivery_window(&msg).await;
                    }
                    // Permanent error: dead-letter immediately without retrying.
                    // PartnerUnknown requires operator intervention (add --as4-partner);
                    // Serialization errors will never succeed on retry; a missing
                    // wire-format renderer cannot appear between attempts — its own
                    // documentation promises immediate dead-lettering, and until this
                    // arm matched it, that promise was broken and the message burned
                    // the whole retry budget first.
                    Err(ref e)
                        if e.is_partner_unknown()
                            || e.is_renderer_not_implemented()
                            || matches!(e, EngineError::Serialization(_)) =>
                    {
                        tracing::error!(
                            message_id   = %msg.message_id,
                            message_type = %msg.message_type,
                            recipient    = %msg.recipient,
                            error        = %e,
                            "outbox worker: permanent send failure; dead-lettering without retry",
                        );
                        self.dead_letter_sink.reject(
                            &crate::dead_letter::DeadLetterReason::OutboxExhausted {
                                message_id: msg.message_id,
                                message_type: msg.message_type.to_string(),
                                recipient: msg.recipient.to_string(),
                                last_error: e.to_string(),
                                attempts: msg.attempt_count,
                            },
                        );
                        if let Err(re) = self.store.acknowledge(msg.message_id).await {
                            tracing::error!(
                                message_id = %msg.message_id,
                                error = %re,
                                "outbox worker: acknowledge after permanent failure failed",
                            );
                        }
                    }
                    Err(e) => {
                        // Stable jitter entropy derived from the UUID bytes of
                        // `message_id`.  Using the last 8 bytes as a `u64` gives
                        // uniform entropy across message IDs (UUIDs are random in
                        // all 128 bits for v4) and is stable across Rust versions —
                        // unlike `DefaultHasher`, whose algorithm is explicitly
                        // documented as unstable.
                        let entropy = {
                            let uuid = msg.message_id.as_uuid();
                            let bytes = uuid.as_bytes();
                            u64::from_le_bytes(bytes[8..16].try_into().unwrap())
                        };
                        let delay = backoff_delay(msg.attempt_count, entropy);
                        let retry_at = time::OffsetDateTime::now_utc()
                            + time::Duration::try_from(delay).unwrap_or(time::Duration::minutes(5));
                        tracing::warn!(
                            message_id   = %msg.message_id,
                            attempt      = msg.attempt_count,
                            max_attempts = self.max_attempts,
                            retry_in     = ?delay,
                            error        = %e,
                            "outbox worker: send failed; rescheduling with backoff",
                        );
                        if let Err(re) = self.store.reschedule(msg.message_id, retry_at).await {
                            tracing::error!(
                                message_id = %msg.message_id,
                                error      = %re,
                                "outbox worker: reschedule failed; message may be stuck",
                            );
                        }
                    }
                }
            }
        }
    }
}

impl<ES, SS, OS, DS, PR> EngineContext<ES, SS, OS, DS, PR>
where
    ES: EventStore,
    OS: OutboxStore + Clone,
{
    /// Construct an [`OutboxWorker`] that drains the outbox via `sender`.
    ///
    /// `batch_size` — messages fetched per poll cycle.
    /// `poll_interval` — sleep duration when the batch is empty.
    ///
    /// `max_attempts` — attempt belt against runaway loops; the real budget is
    /// `max_retry_window`, the message age after which delivery is abandoned.
    /// The BDEW AS4 retry duty is stated in *hours* (72 h for unacknowledged
    /// messages — `mako_as4::constants::MAX_RETRY_DURATION_SECS`), and the
    /// full-jitter backoff makes an attempt count no proxy for a duration, so
    /// both are taken and either exhausts the message.
    ///
    /// ```rust,ignore
    /// use std::time::Duration;
    ///
    /// let worker = ctx.run_outbox_worker(
    ///     my_sender, 50, Duration::from_secs(1),
    ///     10_000, Duration::from_secs(72 * 3600),
    /// );
    /// tokio::spawn(async move { worker.run().await });
    /// ```
    #[must_use]
    pub fn run_outbox_worker<S: As4Sender>(
        &self,
        sender: S,
        batch_size: usize,
        poll_interval: std::time::Duration,
        max_attempts: u32,
        max_retry_window: std::time::Duration,
    ) -> OutboxWorker<OS, S, DS>
    where
        DS: DeadlineStore + Clone,
    {
        OutboxWorker {
            store: self.outbox_store.clone(),
            sender,
            deadline_store: self.deadline_store.clone(),
            batch_size,
            poll_interval,
            max_attempts,
            max_retry_window,
            dead_letter_sink: self.dead_letter_sink.clone(),
            heartbeat: None,
            shutdown: None,
        }
    }
}

impl<OS: OutboxStore, S: As4Sender, DS: DeadlineStore> OutboxWorker<OS, S, DS> {
    /// Attach a liveness heartbeat to this worker.
    ///
    /// The worker will store the current UTC Unix timestamp (seconds) into
    /// `heartbeat` at the end of every poll cycle.  Pass the same
    /// `Arc<AtomicI64>` to the health endpoint so it can detect stale workers.
    #[must_use]
    pub fn with_heartbeat(
        mut self,
        heartbeat: std::sync::Arc<std::sync::atomic::AtomicI64>,
    ) -> Self {
        self.heartbeat = Some(heartbeat);
        self
    }

    /// Attach a graceful-shutdown token.
    ///
    /// Cancelling it makes [`OutboxWorker::run`] return at the next message
    /// boundary or immediately out of its idle sleep. Await the worker's
    /// `JoinHandle` afterwards: the point of the token is that the caller can
    /// close the event store *after* the worker has stopped writing to it.
    #[must_use]
    pub fn with_shutdown(mut self, shutdown: tokio_util::sync::CancellationToken) -> Self {
        self.shutdown = Some(shutdown);
        self
    }

    /// Retire the delivery window `msg` was being watched by, if one is open.
    ///
    /// A delivery-window deadline exists to answer one question: *did this
    /// message go out in time?* Once it has gone out the question is settled,
    /// and leaving the deadline registered only guarantees a false alarm later.
    /// [`fristen::discharges_delivery_window`] decides which labels a given
    /// message type answers for; deadlines that merely share the stream (a
    /// process-response window, say) are left alone.
    ///
    /// Best-effort: a failure here costs a spurious alert at the window's close,
    /// never a lost or duplicated message, so it is logged rather than
    /// propagated — the delivery itself has already been acknowledged.
    ///
    /// [`fristen::discharges_delivery_window`]: mako_fristen::discharges_delivery_window
    async fn discharge_delivery_window(&self, msg: &crate::outbox::OutboxMessage) {
        let open = match self.deadline_store.for_stream(&msg.stream_id).await {
            Ok(deadlines) => deadlines,
            Err(e) => {
                tracing::warn!(
                    message_id   = %msg.message_id,
                    message_type = %msg.message_type,
                    error        = %e,
                    "outbox worker: could not read deadlines to discharge the delivery \
                     window; it may fire a spurious regulatory alert",
                );
                return;
            }
        };

        for deadline in open
            .iter()
            .filter(|d| mako_fristen::discharges_delivery_window(&msg.message_type, d.label()))
        {
            if let Err(e) = self.deadline_store.cancel(deadline.deadline_id()).await {
                tracing::warn!(
                    message_id  = %msg.message_id,
                    deadline_id = %deadline.deadline_id(),
                    label       = %deadline.label(),
                    error       = %e,
                    "outbox worker: could not discharge the delivery window; \
                     it may fire a spurious regulatory alert",
                );
            } else {
                tracing::debug!(
                    message_id   = %msg.message_id,
                    message_type = %msg.message_type,
                    deadline_id  = %deadline.deadline_id(),
                    label        = %deadline.label(),
                    "outbox worker: message delivered — delivery window discharged",
                );
            }
        }
    }
}

impl<ES, SS, OS, DS, PR> std::fmt::Debug for EngineContext<ES, SS, OS, DS, PR>
where
    ES: std::fmt::Debug,
    SS: std::fmt::Debug,
    OS: std::fmt::Debug,
    DS: std::fmt::Debug,
    PR: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineContext")
            .field("registered_modules", &self.registered_modules)
            .field("registered_workflows", &self.registered_workflows)
            .field("pid_router_len", &self.pid_router.len())
            .finish_non_exhaustive()
    }
}

// ── NoopAs4Sender / LogAs4Sender ──────────────────────────────────────────────

/// An [`As4Sender`] that succeeds immediately without sending anything.
///
/// Use in tests and environments where outbound AS4 delivery is not yet
/// wired. All outbox messages are acknowledged (removed from the queue)
/// without being transmitted.
///
/// # ⚠️ Data loss warning
///
/// Every outbox message is **silently discarded** — no EDIFACT message is
/// sent to any counterparty. Do not use in production.
#[derive(Debug, Clone, Copy, Default)]
#[must_use = "NoopAs4Sender discards all outbound messages silently — use a real AS4 gateway in production"]
#[cfg_attr(
    not(any(test, feature = "testing")),
    deprecated = "NoopAs4Sender must not be wired in production builds; every \
                  outbound EDIFACT message would be silently discarded. Use \
                  a real As4Sender implementation instead."
)]
pub struct NoopAs4Sender;

// The trait impl is test/testing-only: a release build without the `testing`
// feature cannot wire NoopAs4Sender into an outbox worker at all.
#[cfg(any(test, feature = "testing"))]
impl As4Sender for NoopAs4Sender {
    async fn send(&self, _msg: &OutboxMessage) -> Result<(), EngineError> {
        Ok(())
    }
}

/// An [`As4Sender`] that logs every outbound message at `warn` level and
/// succeeds without transmitting.
///
/// Useful for development and integration-testing environments where the
/// full AS4 stack is not yet available but message visibility is desired.
/// All outbox messages are acknowledged (removed from the queue) after logging.
///
/// # ⚠️ Data loss warning
///
/// No EDIFACT message is sent to any counterparty. Do not use in production.
#[derive(Debug, Clone, Copy, Default)]
#[must_use = "LogAs4Sender discards all outbound messages — use a real AS4 gateway in production"]
pub struct LogAs4Sender;

impl As4Sender for LogAs4Sender {
    async fn send(&self, msg: &OutboxMessage) -> Result<(), EngineError> {
        tracing::warn!(
            message_id   = %msg.message_id,
            message_type = %msg.message_type,
            recipient    = %msg.recipient,
            "LogAs4Sender: outbox message dropped — configure a real AS4 gateway for production",
        );
        Ok(())
    }
}

// ── DeadlineScheduler ─────────────────────────────────────────────────────────

/// A background task that polls [`DeadlineStore::due_now`] and dispatches
/// deadline commands to the owning processes via a caller-supplied function.
///
/// Obtain via [`EngineContext::run_deadline_scheduler`] and drive by spawning
/// [`DeadlineScheduler::run`] in a Tokio task.
///
/// # Dispatch function
///
/// The `dispatch` function receives a fired [`Deadline`] and returns a future
/// that dispatches the appropriate timeout command to the process. The function
/// is responsible for resuming the correct workflow and calling `execute`.
/// After the future completes, the scheduler cancels the deadline from the
/// store regardless of the dispatch outcome (to prevent re-firing).
///
/// ```rust,ignore
/// use std::time::Duration;
///
/// let scheduler = ctx.run_deadline_scheduler(
///     |deadline| async move {
///         tracing::warn!(
///             deadline_id = %deadline.deadline_id(),
///             label = %deadline.label(),
///             "deadline fired",
///         );
///         Ok(())
///     },
///     100,
///     Duration::from_secs(30),
/// );
/// tokio::spawn(async move { scheduler.run().await });
/// ```
pub struct DeadlineScheduler<DS: DeadlineStore> {
    store: DS,
    dispatch: Box<
        dyn Fn(
                Deadline,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<(), EngineError>> + Send>,
            > + Send
            + Sync,
    >,
    batch_size: usize,
    poll_interval: std::time::Duration,
    /// Optional liveness heartbeat — stores the current UTC Unix timestamp
    /// (seconds) after each poll cycle.
    heartbeat: Option<std::sync::Arc<std::sync::atomic::AtomicI64>>,
    /// Graceful-shutdown signal — see [`DeadlineScheduler::with_shutdown`].
    shutdown: Option<tokio_util::sync::CancellationToken>,
}

impl<DS: DeadlineStore> DeadlineScheduler<DS> {
    /// Run the deadline poll loop until the shutdown token is cancelled.
    ///
    /// Cancellation is observed between deadlines and during the idle sleep, so
    /// a deadline already being dispatched runs to completion. A deadline left
    /// undispatched stays registered and fires on the next start — it is due, so
    /// the next `due_now` returns it again.
    pub async fn run(self) {
        loop {
            if self
                .shutdown
                .as_ref()
                .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
            {
                tracing::info!("deadline scheduler: shutdown signalled; stopping");
                return;
            }
            // Tick liveness at the *start* of every poll cycle, ahead of the
            // early-`continue` paths below.  An idle scheduler (no due
            // deadlines) is alive and must keep ticking; only one genuinely
            // hung inside an `.await` stops.
            if let Some(ref hb) = self.heartbeat {
                hb.store(
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }

            let result = match self.store.due_now(self.batch_size).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "deadline scheduler: store error polling due deadlines (will retry)",
                    );
                    if !sleep_or_cancel(self.poll_interval, self.shutdown.as_ref()).await {
                        return;
                    }
                    continue;
                }
            };

            if result.deadlines.is_empty() {
                if !sleep_or_cancel(self.poll_interval, self.shutdown.as_ref()).await {
                    return;
                }
                continue;
            }

            for deadline in result.deadlines {
                // Between deadlines, not inside one: a dispatch already running
                // must finish so its events and outbox entries commit together.
                if self
                    .shutdown
                    .as_ref()
                    .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
                {
                    tracing::info!(
                        "deadline scheduler: shutdown signalled mid-batch; \
                         undispatched deadlines remain due and fire on the next start"
                    );
                    return;
                }
                let id = deadline.deadline_id();
                let label = deadline.label().to_owned();

                // An APERAK delivery window that reaches this point is a
                // regulatory violation under APERAK AHB 1.0 §2.4.1 (Strom
                // 45 min) / §2.3.1 (Gas 1 Werktag): the outbox worker discharges
                // the window the moment the APERAK goes out, so one that is
                // still registered when it comes due was never answered.
                //
                // Do NOT re-test `now > due_at` here. `due_now` selects on
                // `due_at <= now`, so that comparison is true by construction and
                // says nothing about compliance — it was the reason this counter
                // once tracked "Strom processes started" rather than "APERAKs
                // missed". The discharge is what carries the meaning.
                if label.starts_with(mako_fristen::APERAK_WINDOW_LABEL_PREFIX) {
                    let now = time::OffsetDateTime::now_utc();
                    crate::metrics::EngineMetrics::global().aperak_missed(&label);
                    tracing::error!(
                        deadline_id = %id,
                        label       = %label,
                        due_at      = %deadline.due_at(),
                        fired_at    = %now,
                        overdue_secs = (now - deadline.due_at()).whole_seconds(),
                        "APERAK delivery window closed with no delivery — regulatory \
                         violation (APERAK AHB 1.0 §2.4.1 Strom / §2.3.1 Gas). \
                         Counter: makod_aperak_missed_total",
                    );
                }

                let should_cancel = match (self.dispatch)(deadline).await {
                    Ok(()) => true,
                    Err(ref e) if e.is_version_conflict() => {
                        // The process was modified concurrently; the timeout
                        // command will be retried on the next poll cycle.
                        // Do NOT cancel — let the deadline remain due so it
                        // fires again until a non-conflict dispatch succeeds.
                        tracing::warn!(
                            deadline_id = %id,
                            label       = %label,
                            "deadline scheduler: VersionConflict; will retry on next poll",
                        );
                        false
                    }
                    Err(e) => {
                        tracing::warn!(
                            deadline_id = %id,
                            label       = %label,
                            error       = %e,
                            "deadline scheduler: dispatch failed (permanent); cancelling",
                        );
                        true
                    }
                };
                if should_cancel && let Err(e) = self.store.cancel(id).await {
                    tracing::error!(
                        deadline_id = %id,
                        error       = %e,
                        "deadline scheduler: cancel failed; deadline may fire again",
                    );
                }
            }

            // If has_more, loop immediately to drain the batch.
        }
    }
}

impl<DS: DeadlineStore> DeadlineScheduler<DS> {
    /// Attach a liveness heartbeat to this scheduler.
    ///
    /// The scheduler will store the current UTC Unix timestamp (seconds) into
    /// `heartbeat` at the end of every poll cycle.
    #[must_use]
    pub fn with_heartbeat(
        mut self,
        heartbeat: std::sync::Arc<std::sync::atomic::AtomicI64>,
    ) -> Self {
        self.heartbeat = Some(heartbeat);
        self
    }

    /// Attach a graceful-shutdown token.
    ///
    /// Cancelling it makes [`DeadlineScheduler::run`] return at the next
    /// deadline boundary or immediately out of its idle sleep, so the caller can
    /// close the event store once the scheduler has stopped writing to it.
    #[must_use]
    pub fn with_shutdown(mut self, shutdown: tokio_util::sync::CancellationToken) -> Self {
        self.shutdown = Some(shutdown);
        self
    }
}

impl<ES, SS, OS, DS, PR> EngineContext<ES, SS, OS, DS, PR>
where
    ES: EventStore,
    DS: DeadlineStore + Clone,
{
    /// Construct a [`DeadlineScheduler`] that polls the deadline store and
    /// dispatches fired deadlines via `dispatch`.
    ///
    /// The `dispatch` function is called for every fired deadline. It should
    /// resume the owning process and execute the appropriate timeout command.
    ///
    /// `batch_size` — deadlines fetched per poll cycle.
    /// `poll_interval` — sleep duration when no deadlines are due.
    ///
    /// ```rust,ignore
    /// use std::time::Duration;
    ///
    /// let scheduler = ctx.run_deadline_scheduler(
    ///     |d| async move {
    ///         tracing::info!(label = %d.label(), "firing deadline");
    ///         Ok(())
    ///     },
    ///     100,
    ///     Duration::from_secs(30),
    /// );
    /// tokio::spawn(async move { scheduler.run().await });
    /// ```
    #[must_use]
    pub fn run_deadline_scheduler<F, Fut>(
        &self,
        dispatch: F,
        batch_size: usize,
        poll_interval: std::time::Duration,
    ) -> DeadlineScheduler<DS>
    where
        F: Fn(Deadline) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), EngineError>> + Send + 'static,
    {
        DeadlineScheduler {
            store: self.deadline_store.clone(),
            dispatch: Box::new(move |d| Box::pin(dispatch(d))),
            batch_size,
            poll_interval,
            heartbeat: None,
            shutdown: None,
        }
    }
}

// ── EngineBuilder ─────────────────────────────────────────────────────────────

/// Assembles engine infrastructure and produces an [`EngineContext`].
///
/// Uses type-state to enforce that an event store is provided before
/// [`build`] can be called. All other stores default to `Noop`
/// implementations.
///
/// ## Quick start
///
/// ```rust,ignore
/// // Minimal — event store only, all others are Noop:
/// let ctx = EngineBuilder::new()
///     .with_event_store(InMemoryEventStore::new())
///     .build();
///
/// // Full infrastructure:
/// let ctx = EngineBuilder::new()
///     .with_event_store(InMemoryEventStore::new())
///     .with_snapshot_store(InMemorySnapshotStore::new())
///     .with_outbox_store(InMemoryOutboxStore::new())
///     .with_deadline_store(InMemoryDeadlineStore::new())
///     .with_registry(InMemoryProcessRegistry::new())
///     .register(Box::new(GpkeModule))
///     .build();
/// ```
///
/// [`build`]: EngineBuilder::build
pub struct EngineBuilder<
    ES = (),
    SS = NoopSnapshotStore,
    OS = NoopOutboxStore,
    DS = NoopDeadlineStore,
    PR = NoopProcessRegistry,
> {
    event_store: ES,
    snapshot_store: SS,
    outbox_store: OS,
    deadline_store: DS,
    registry: PR,
    dead_letter_sink: Arc<dyn DeadLetterSink>,
    modules: Vec<Box<dyn EngineModule>>,
    /// Active [`DeploymentRoles`] for this engine instance.
    ///
    /// Controls role-conditional PID registration via
    /// [`EngineModule::register_pids_with_roles`]. Defaults to
    /// [`DeploymentRoles::all()`] for backward compatibility.
    deployment_roles: DeploymentRoles,
    /// Optional profile validator injected by `makod` or callers that have
    /// access to `edi-energy`.  When `Some`, called for each
    /// [`ProfileRequirement`] declared by registered modules.  When `None`,
    /// profile requirements are not validated (safe in unit tests).
    ///
    /// Signature: `fn(message_type: &str) -> bool`
    ///
    /// [`ProfileRequirement`]: crate::profile::ProfileRequirement
    profile_validator: Option<Box<dyn Fn(&str) -> bool + Send + Sync>>,
}
#[cfg(any(test, feature = "testing"))]
impl Default
    for EngineBuilder<
        (),
        NoopSnapshotStore,
        NoopOutboxStore,
        NoopDeadlineStore,
        NoopProcessRegistry,
    >
{
    fn default() -> Self {
        Self {
            event_store: (),
            snapshot_store: NoopSnapshotStore,
            outbox_store: NoopOutboxStore,
            deadline_store: NoopDeadlineStore,
            registry: NoopProcessRegistry,
            dead_letter_sink: Arc::new(LogDeadLetterSink),
            modules: Vec::new(),
            deployment_roles: DeploymentRoles::all(),
            profile_validator: None,
        }
    }
}

#[cfg(any(test, feature = "testing"))]
impl EngineBuilder {
    /// Create a new builder with all `Noop` defaults.
    ///
    /// Only available in `#[cfg(test)]` or with the `testing` feature enabled,
    /// because the Noop defaults silently discard outbox messages, deadlines,
    /// and process registry entries. Production binaries must wire real stores
    /// via the `with_*` builder methods.
    ///
    /// Call [`with_event_store`] before [`build`] — the event store is
    /// **required**.
    ///
    /// [`with_event_store`]: EngineBuilder::with_event_store
    /// [`build`]: EngineBuilder::build
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<OS, DS, PR> EngineBuilder<(), NoopSnapshotStore, OS, DS, PR>
where
    OS: OutboxStore,
    DS: DeadlineStore,
    PR: ProcessRegistry,
{
    /// Create a production-ready builder with explicit stores for outbox,
    /// deadline, and process registry.
    ///
    /// This constructor is available in all build configurations including
    /// production binaries. It enforces that the three stores that can cause
    /// silent data loss (`OutboxStore`, `DeadlineStore`, `ProcessRegistry`)
    /// are provided explicitly — there is no Noop fallback.
    ///
    /// `NoopSnapshotStore` is used as the snapshot default because it is safe
    /// for production: skipping snapshots means full replay, but no data loss.
    /// Override with [`with_snapshot_store`] to enable snapshot-accelerated
    /// replay.
    ///
    /// Call [`with_event_store`] before [`build`] — the event store is
    /// **required**.
    ///
    /// ```rust,ignore
    /// let ctx = EngineBuilder::with_stores(outbox, deadline, registry)
    ///     .with_event_store(store.clone())
    ///     .with_snapshot_store(InMemorySnapshotStore::new())
    ///     .build();
    /// ```
    ///
    /// [`with_snapshot_store`]: EngineBuilder::with_snapshot_store
    /// [`with_event_store`]: EngineBuilder::with_event_store
    /// [`build`]: EngineBuilder::build
    #[must_use]
    pub fn with_stores(outbox_store: OS, deadline_store: DS, registry: PR) -> Self {
        Self {
            event_store: (),
            snapshot_store: NoopSnapshotStore,
            outbox_store,
            deadline_store,
            registry,
            dead_letter_sink: Arc::new(LogDeadLetterSink),
            modules: Vec::new(),
            deployment_roles: DeploymentRoles::all(),
            profile_validator: None,
        }
    }
}

impl<ES, SS, OS, DS, PR> EngineBuilder<ES, SS, OS, DS, PR> {
    /// Set the event store. **Required** — `build()` is only available once
    /// this has been called with a type that implements [`EventStore`].
    ///
    /// Replaces any previously set event store (type-state transition).
    #[must_use]
    pub fn with_event_store<ES2: EventStore>(
        self,
        store: ES2,
    ) -> EngineBuilder<ES2, SS, OS, DS, PR> {
        EngineBuilder {
            event_store: store,
            snapshot_store: self.snapshot_store,
            outbox_store: self.outbox_store,
            deadline_store: self.deadline_store,
            registry: self.registry,
            dead_letter_sink: self.dead_letter_sink,
            modules: self.modules,
            deployment_roles: self.deployment_roles,
            profile_validator: self.profile_validator,
        }
    }

    /// Set the snapshot store (default: [`NoopSnapshotStore`]).
    ///
    /// ## Default: `NoopSnapshotStore`
    ///
    /// Without calling this method the builder uses [`NoopSnapshotStore`],
    /// which silently discards all snapshot writes and returns `None` for
    /// every snapshot read.  The engine still functions correctly — every
    /// command handling call replays the full event log from the beginning
    /// instead of starting from a stored snapshot.  For low-volume processes
    /// this is fine; for long-lived processes with many events the replay cost
    /// can become significant.
    ///
    /// Enable snapshotting in production by providing a real [`SnapshotStore`]
    /// implementation (e.g. the SlateDB-backed store in `makod`).  In tests,
    /// `InMemorySnapshotStore` is available behind the `testing` feature flag.
    ///
    /// Note: [`Process::state_with_snapshot`][crate::process::Process::state_with_snapshot]
    /// is a compile-time no-op when the snapshot store is `NoopSnapshotStore`
    /// — it never calls the store and always returns `None`, so no snapshot is
    /// ever saved or loaded.
    #[must_use]
    pub fn with_snapshot_store<SS2: SnapshotStore>(
        self,
        store: SS2,
    ) -> EngineBuilder<ES, SS2, OS, DS, PR> {
        EngineBuilder {
            event_store: self.event_store,
            snapshot_store: store,
            outbox_store: self.outbox_store,
            deadline_store: self.deadline_store,
            registry: self.registry,
            dead_letter_sink: self.dead_letter_sink,
            modules: self.modules,
            deployment_roles: self.deployment_roles,
            profile_validator: self.profile_validator,
        }
    }

    /// Set the outbox store (default: [`NoopOutboxStore`]).
    #[must_use]
    pub fn with_outbox_store<OS2: OutboxStore>(
        self,
        store: OS2,
    ) -> EngineBuilder<ES, SS, OS2, DS, PR> {
        EngineBuilder {
            event_store: self.event_store,
            snapshot_store: self.snapshot_store,
            outbox_store: store,
            deadline_store: self.deadline_store,
            registry: self.registry,
            dead_letter_sink: self.dead_letter_sink,
            modules: self.modules,
            deployment_roles: self.deployment_roles,
            profile_validator: self.profile_validator,
        }
    }

    /// Set the deadline store (default: [`NoopDeadlineStore`]).
    #[must_use]
    pub fn with_deadline_store<DS2: DeadlineStore>(
        self,
        store: DS2,
    ) -> EngineBuilder<ES, SS, OS, DS2, PR> {
        EngineBuilder {
            event_store: self.event_store,
            snapshot_store: self.snapshot_store,
            outbox_store: self.outbox_store,
            deadline_store: store,
            registry: self.registry,
            dead_letter_sink: self.dead_letter_sink,
            modules: self.modules,
            deployment_roles: self.deployment_roles,
            profile_validator: self.profile_validator,
        }
    }

    /// Set the process registry (default: [`NoopProcessRegistry`]).
    #[must_use]
    pub fn with_registry<PR2: ProcessRegistry>(
        self,
        registry: PR2,
    ) -> EngineBuilder<ES, SS, OS, DS, PR2> {
        EngineBuilder {
            event_store: self.event_store,
            snapshot_store: self.snapshot_store,
            outbox_store: self.outbox_store,
            deadline_store: self.deadline_store,
            registry,
            dead_letter_sink: self.dead_letter_sink,
            modules: self.modules,
            deployment_roles: self.deployment_roles,
            profile_validator: self.profile_validator,
        }
    }

    /// Set the dead-letter sink (default: [`LogDeadLetterSink`]).
    ///
    /// The dead-letter sink receives every message that cannot be routed to a
    /// workflow. The default [`LogDeadLetterSink`] emits `tracing::warn!`
    /// events, making rejections visible in log output without configuration.
    ///
    /// Override with a persistent DLQ implementation in production:
    ///
    /// ```rust,ignore
    /// use mako_engine::dead_letter::LogDeadLetterSink;
    ///
    /// let ctx = EngineBuilder::new()
    ///     .with_event_store(my_store)
    ///     .with_dead_letter_sink(MyPersistentDlq::new())
    ///     .build();
    /// ```
    ///
    /// [`LogDeadLetterSink`]: crate::dead_letter::LogDeadLetterSink
    #[must_use]
    pub fn with_dead_letter_sink(mut self, sink: impl DeadLetterSink) -> Self {
        self.dead_letter_sink = Arc::new(sink);
        self
    }

    /// Register an `edi-energy` profile validator for startup profile checks.
    ///
    /// The closure receives a message-type string (e.g. `"UTILMD"`) and must
    /// return `true` if at least one active profile for that message type is
    /// registered for today's date.
    ///
    /// Wire this in `makod` using the `edi-energy` global registry:
    ///
    /// ```rust,ignore
    /// use edi_energy::registry::ReleaseRegistry;
    ///
    /// let today = time::OffsetDateTime::now_utc().date();
    /// builder.with_profile_validator(move |msg_type| {
    ///     ReleaseRegistry::global()
    ///         .profiles_for_str(msg_type)
    ///         .any(|p| match (p.valid_from(), p.valid_until()) {
    ///             (Some(f), Some(u)) => f <= today && today <= u,
    ///             (Some(f), None)    => f <= today,
    ///             (None, _)          => true,
    ///         })
    /// })
    /// ```
    ///
    /// Domain crates do **not** need to call this — they only declare
    /// [`profile_requirements`].
    ///
    /// [`profile_requirements`]: EngineModule::profile_requirements
    #[must_use]
    pub fn with_profile_validator(
        mut self,
        validator: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.profile_validator = Some(Box::new(validator));
        self
    }

    /// Register a domain module.
    ///
    /// The module name becomes visible in
    /// [`EngineContext::registered_modules`] after [`build`] is called.
    ///
    /// [`build`]: EngineBuilder::build
    #[must_use]
    pub fn register(mut self, module: Box<dyn EngineModule>) -> Self {
        self.modules.push(module);
        self
    }

    /// Register multiple [`EngineModule`]s at once from a pre-built `Vec`.
    ///
    /// Equivalent to calling [`register`] in a loop. Useful when the set of
    /// modules is assembled conditionally (e.g. via `#[cfg]`-gated pushes to a
    /// `Vec<Box<dyn EngineModule>>`) before the builder chain starts.
    ///
    /// [`register`]: EngineBuilder::register
    #[must_use]
    pub fn register_many(mut self, modules: Vec<Box<dyn EngineModule>>) -> Self {
        self.modules.extend(modules);
        self
    }

    /// Set the active [`DeploymentRoles`] for this engine instance.
    ///
    /// Controls role-conditional PID registration in [`EngineModule::register_pids_with_roles`].
    ///
    /// The default is [`DeploymentRoles::all()`], which registers every PID unconditionally
    /// — identical to the pre-role-aware behavior. Providing an explicit role set
    /// restricts role-conditional blocks to only the declared roles:
    ///
    /// - **NB-only** (`DeploymentRoles::nb()`): 19001/19002 route to `gpke-konfiguration`;
    ///   WiM nMSB blocks are skipped.
    /// - **nMSB-only** (`DeploymentRoles::nmsb()`): 19001/19002 route to `wim-geraeteubernahme`;
    ///   GPKE NB blocks are skipped.
    /// - **NB + gMSB** (`DeploymentRoles::nb_msb()`): most common Stadtwerke combination.
    ///
    /// # Conflict guard
    ///
    /// When two modules would register the same PID to **different** workflows, the
    /// engine panics during [`build`]. Set explicit roles to prevent both modules from
    /// activating the same PID simultaneously:
    ///
    /// ```rust,ignore
    /// use mako_engine::marktrolle::DeploymentRoles;
    ///
    /// let ctx = EngineBuilder::with_stores(outbox, deadline, registry)
    ///     .with_event_store(store)
    ///     .with_deployment_roles(DeploymentRoles::nb())  // only NB: GPKE gets 19001/19002
    ///     .register(Box::new(GpkeModule))
    ///     .register(Box::new(WimModule))  // nMSB block skipped — no conflict
    ///     .build();
    /// ```
    ///
    /// [`build`]: EngineBuilder::build
    #[must_use]
    pub fn with_deployment_roles(mut self, roles: DeploymentRoles) -> Self {
        self.deployment_roles = roles;
        self
    }
}

impl<ES, SS, OS, DS, PR> EngineBuilder<ES, SS, OS, DS, PR>
where
    ES: EventStore,
    SS: SnapshotStore,
    OS: OutboxStore,
    DS: DeadlineStore,
    PR: ProcessRegistry,
{
    /// Build the [`EngineContext`].
    ///
    /// Consumes the builder. All registered modules and configured stores are
    /// moved into the returned [`EngineContext`].
    ///
    /// This method is only available when `ES` implements [`EventStore`].
    /// If you have not called [`with_event_store`], this will not compile.
    ///
    /// # Panics
    ///
    /// Panics when any registered module returns `Err` from
    /// [`EngineModule::configure`]. The panic message includes the module
    /// name and the error string so the deployment failure is actionable.
    ///
    /// [`with_event_store`]: EngineBuilder::with_event_store
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn build(self) -> EngineContext<ES, SS, OS, DS, PR> {
        // ── Noop store safety checks ──────────────────────────────────────────
        //
        // Noop stores lose data silently: NoopDeadlineStore drops every APERAK
        // deadline (BNetzA violation), NoopOutboxStore discards all outbound
        // messages, NoopProcessRegistry loses conversation routing on restart.
        //
        // In production builds (no `testing` feature, not running under
        // `#[test]`), the Noop constructors are cfg-gated out so this branch
        // is dead code and compiles away. In test/testing/tracing builds we
        // emit warnings so test harnesses see the configuration in log output.
        //
        // IMPORTANT: if you are reading this because a panic fired in production,
        // it means the `testing` feature was accidentally enabled in the binary.
        // Remove it from the production Cargo.toml feature list immediately.
        {
            let os_name = std::any::type_name::<OS>();
            let ds_name = std::any::type_name::<DS>();
            let pr_name = std::any::type_name::<PR>();

            // Regulatory-critical stores: panic in any build context if these
            // are noop. OutboxStore and DeadlineStore must be durable in
            // production; ProcessRegistry must survive restarts.
            #[cfg(not(any(test, feature = "testing")))]
            {
                assert!(
                    !ds_name.contains("NoopDeadlineStore"),
                    "EngineBuilder::build: NoopDeadlineStore is active in a \
                     non-testing build. This silently discards all APERAK deadlines, \
                     which is an immediately reportable BNetzA violation \
                     (BK6-22-024 §5, BK7-24-01-009). \
                     Call .with_deadline_store(SlateDbStore::as_deadline_store()) \
                     in your production engine assembly. \
                     If this is a test, enable the 'testing' feature."
                );
                assert!(
                    !os_name.contains("NoopOutboxStore"),
                    "EngineBuilder::build: NoopOutboxStore is active in a \
                     non-testing build. This silently discards all outbound \
                     APERAK, CONTRL, and UTILMD messages. \
                     Call .with_outbox_store(SlateDbStore::as_outbox_store()) \
                     in your production engine assembly. \
                     If this is a test, enable the 'testing' feature."
                );
                assert!(
                    !pr_name.contains("NoopProcessRegistry"),
                    "EngineBuilder::build: NoopProcessRegistry is active in a \
                     non-testing build. This means conversation routing \
                     (PID → stream_id lookup) is lost on every restart, \
                     breaking all WiM, GeLi Gas, and GPKE in-flight processes. \
                     Call .with_registry(SlateDbStore::as_process_registry()) \
                     in your production engine assembly. \
                     If this is a test, enable the 'testing' feature."
                );
            }

            // In test/testing/tracing builds: emit warnings instead of panicking.
            #[cfg(any(test, feature = "testing", feature = "tracing"))]
            {
                let ss_name = std::any::type_name::<SS>();
                if ss_name.contains("NoopSnapshotStore") {
                    tracing::warn!(
                        store = ss_name,
                        "EngineBuilder: NoopSnapshotStore is active — snapshots will not be \
                         persisted. Use SlateDbStore::as_snapshot_store() in production."
                    );
                }
                if os_name.contains("NoopOutboxStore") {
                    tracing::warn!(
                        store = os_name,
                        "EngineBuilder: NoopOutboxStore is active — outbound messages will be \
                         silently discarded. Use SlateDbStore::as_outbox_store() in production."
                    );
                }
                if ds_name.contains("NoopDeadlineStore") {
                    tracing::warn!(
                        store = ds_name,
                        "EngineBuilder: NoopDeadlineStore is active — scheduled deadlines will \
                         not fire after restart. Use SlateDbStore::as_deadline_store() in production."
                    );
                }
                if pr_name.contains("NoopProcessRegistry") {
                    tracing::warn!(
                        store = pr_name,
                        "EngineBuilder: NoopProcessRegistry is active — process routing will be \
                         lost on restart. Use SlateDbStore::as_process_registry() in production."
                    );
                }
            }
        }
        // Validate every module before assembling the context.
        // A missing adapter or misconfigured module fails at startup (not at
        // first inbound message), making deployment failures observable immediately.
        for module in &self.modules {
            if let Err(msg) = module.configure() {
                panic!(
                    "EngineBuilder::build: module '{}' failed configuration validation: {}",
                    module.name(),
                    msg
                );
            }
            // Validate profile requirements via the injected validator.
            // Domain crates declare requirements; only the binary crate (makod)
            // injects the edi-energy registry — domain crates need no edi-energy
            // import for this check.
            if let Some(ref validator) = self.profile_validator {
                for req in module.profile_requirements() {
                    assert!(
                        validator(req.message_type),
                        "EngineBuilder::build: module '{}' requires an active edi-energy \
                             profile for '{}' ({}) but none is registered for today's date. \
                             Run `cargo xtask codegen` to add the missing profile.",
                        module.name(),
                        req.message_type,
                        req.label,
                    );
                }
            }
        }
        // Build the PID router from all registered modules.
        // Also assert that no two modules claim the same PID — a PID overlap
        // is always a configuration error: one module's messages would be
        // silently swallowed by another's workflow, producing missing-process
        // errors or incorrect audit trails.
        let mut pid_router = PidRouter::new();
        let mut pid_owners: std::collections::HashMap<u32, &str> = std::collections::HashMap::new();
        // Keep each module's scratch router so we can build `pid_router` from
        // them in a second pass with the resolved ownership table.
        let mut module_scratches: Vec<PidRouter> = Vec::with_capacity(self.modules.len());

        // Pass 1 — detect conflicts, determine PID ownership (first-wins for
        // explicit roles, last-wins for DeploymentRoles::all()).
        for module in &self.modules {
            // Temporarily build a scratch router to read this module's PIDs
            // for cross-module overlap detection (module-ownership level).
            let mut scratch = PidRouter::new();
            module.register_pids_with_roles(&mut scratch, &self.deployment_roles);
            for pid in scratch.registered_pids() {
                if let Some(prev) = pid_owners.insert(pid, module.name()) {
                    if self.deployment_roles.is_all() {
                        // With DeploymentRoles::all() (the default), role-conditional PIDs
                        // are registered by all modules that claim them, producing last-wins
                        // semantics. This is acceptable for single-role and dev/test deployments.
                        //
                        // In production multi-role deployments where both an NB and nMSB role
                        // are served by the same instance, set explicit roles via
                        // `EngineBuilder::with_deployment_roles` to prevent silent misrouting.
                        //
                        // We emit a debug-level log here (not warn) because the vast majority
                        // of deployments are single-role and this overlap is expected/harmless.
                        #[cfg(feature = "tracing")]
                        tracing::debug!(
                            pid,
                            previous_module = prev,
                            current_module = module.name(),
                            "PID registered by multiple modules with DeploymentRoles::all(); \
                             last module wins (use with_deployment_roles for strict routing)",
                        );
                        let _ = prev; // suppress unused-variable warning when tracing is off
                    } else {
                        // Explicit roles: the FIRST module to register a PID retains ownership.
                        // Restore the previous (first) owner and emit a warning so the operator
                        // can investigate.  A panic would be too strict: some shared PIDs
                        // (e.g. REMADV 33001/33002) are legitimately claimed by both GPKE and
                        // WiM billing; conversation-ID routing is the long-term solution, but
                        // first-wins gives correct behaviour for all current deployments.
                        pid_owners.insert(pid, prev); // restore first owner
                        #[cfg(feature = "tracing")]
                        tracing::warn!(
                            pid,
                            first_module = prev,
                            second_module = module.name(),
                            "PID {pid} claimed by both '{prev}' and '{}' with explicit \
                             DeploymentRoles; first module ('{prev}') retains ownership. \
                             Verify PID registration is correct for this deployment.",
                            module.name(),
                        );
                        #[cfg(not(feature = "tracing"))]
                        let _ = prev; // suppress unused-variable warning when tracing is off
                    }
                }
            }
            module_scratches.push(scratch);
        }

        // Pass 2 — build the real `pid_router` from the scratch pads, respecting
        // the ownership table built in pass 1.
        for (module, scratch) in self.modules.iter().zip(module_scratches.iter()) {
            // Unambiguous (Sparte-agnostic) entries: only register if this module
            // owns the PID in the resolved ownership table.
            for pid in scratch.registered_pids() {
                if pid_owners.get(&pid).copied() == Some(module.name())
                    && let Some(wf) = scratch.route(pid)
                {
                    pid_router.register(pid, wf);
                }
            }
            // Commodity (Sparte-qualified) entries use distinct (pid, Sparte) keys
            // and never conflict across modules; register them all unconditionally.
            for (pid, sparte, wf) in scratch.registered_commodity_entries() {
                pid_router.register_with_sparte(pid, sparte, wf);
            }
        }
        let registered_modules = self.modules.iter().map(|m| m.name()).collect();
        let registered_workflows = self
            .modules
            .iter()
            .flat_map(|m| m.workflow_names().iter().copied())
            .collect();
        EngineContext {
            event_store: Arc::new(self.event_store),
            snapshot_store: self.snapshot_store,
            outbox_store: self.outbox_store,
            deadline_store: self.deadline_store,
            registry: self.registry,
            dead_letter_sink: self.dead_letter_sink,
            pid_router,
            registered_modules,
            registered_workflows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        deadline::InMemoryDeadlineStore,
        error::WorkflowError,
        event_store::InMemoryEventStore,
        ids::TenantId,
        outbox::InMemoryOutboxStore,
        pid_router::PidRouter,
        registry::InMemoryProcessRegistry,
        snapshot::InMemorySnapshotStore,
        version::WorkflowId,
        workflow::{CommandPayload, EventPayload, Workflow},
    };

    // ── Minimal workflow for spawn/resume tests ───────────────────────────────

    #[derive(serde::Serialize, serde::Deserialize)]
    struct PingEvent;

    impl EventPayload for PingEvent {
        fn event_type(&self) -> &'static str {
            "Ping"
        }
    }

    struct PingCommand;

    impl CommandPayload for PingCommand {}

    #[derive(Default, Clone)]
    struct PingState;

    struct PingWorkflow;

    impl Workflow for PingWorkflow {
        type State = PingState;
        type Event = PingEvent;
        type Command = PingCommand;

        fn apply(state: PingState, _: &PingEvent) -> PingState {
            state
        }

        fn handle(
            _: &PingState,
            _: PingCommand,
        ) -> Result<crate::workflow::WorkflowOutput<PingEvent>, WorkflowError> {
            Ok(vec![PingEvent].into())
        }
    }

    struct TestModule;

    impl EngineModule for TestModule {
        fn name(&self) -> &'static str {
            "test-module"
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn build_with_event_store_only() {
        let ctx = EngineBuilder::new()
            .with_event_store(InMemoryEventStore::new())
            .build();
        assert!(ctx.registered_modules().is_empty());
    }

    #[test]
    fn build_with_all_stores_and_module() {
        let ctx = EngineBuilder::new()
            .with_event_store(InMemoryEventStore::new())
            .with_snapshot_store(InMemorySnapshotStore::new())
            .with_outbox_store(InMemoryOutboxStore::new())
            .with_deadline_store(InMemoryDeadlineStore::new())
            .with_registry(InMemoryProcessRegistry::new())
            .register(Box::new(TestModule))
            .build();
        assert_eq!(ctx.registered_modules(), &["test-module"]);
    }

    #[test]
    fn multiple_modules_ordered() {
        struct ModA;
        impl EngineModule for ModA {
            fn name(&self) -> &'static str {
                "mod-a"
            }
        }
        struct ModB;
        impl EngineModule for ModB {
            fn name(&self) -> &'static str {
                "mod-b"
            }
        }

        let ctx = EngineBuilder::new()
            .with_event_store(InMemoryEventStore::new())
            .register(Box::new(ModA))
            .register(Box::new(ModB))
            .build();
        assert_eq!(ctx.registered_modules(), &["mod-a", "mod-b"]);
    }

    #[tokio::test]
    async fn spawn_creates_independent_processes() {
        let ctx = EngineBuilder::new()
            .with_event_store(InMemoryEventStore::new())
            .build();
        let wf_id = WorkflowId::new("ping", "FV2024-10-01");

        let p1 = ctx.spawn::<PingWorkflow>(TenantId::new(), wf_id.clone());
        let p2 = ctx.spawn::<PingWorkflow>(TenantId::new(), wf_id);

        assert_ne!(p1.process_id(), p2.process_id());
    }

    #[tokio::test]
    async fn resume_sees_previously_appended_events() {
        let store = InMemoryEventStore::new();
        let ctx = EngineBuilder::new().with_event_store(store).build();

        let p = ctx.spawn::<PingWorkflow>(TenantId::new(), WorkflowId::new("ping", "FV2024-10-01"));
        p.execute(PingCommand).await.unwrap();

        let identity = p.identity();
        let resumed = ctx.resume::<PingWorkflow>(identity);
        assert_eq!(resumed.event_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn registry_routes_process_via_conversation_key() {
        use crate::registry::RegistryKey;
        let ctx = EngineBuilder::new()
            .with_event_store(InMemoryEventStore::new())
            .with_registry(InMemoryProcessRegistry::new())
            .build();

        let p = ctx.spawn::<PingWorkflow>(TenantId::new(), WorkflowId::new("ping", "FV2024-10-01"));
        let tenant = p.tenant_id();
        let conv_key = RegistryKey::parse("conv:test-conversation-123").expect("valid key");
        ctx.registry()
            .register(tenant, &conv_key, p.identity())
            .await
            .unwrap();

        let found = ctx
            .registry()
            .lookup(tenant, &conv_key)
            .await
            .unwrap()
            .expect("must be registered");
        let resumed = ctx.resume::<PingWorkflow>(found);
        assert_eq!(resumed.process_id(), p.process_id());
    }

    #[test]
    fn pid_router_populated_by_module_register_pids() {
        struct PidModule;
        impl EngineModule for PidModule {
            fn name(&self) -> &'static str {
                "pid-module"
            }
            fn register_pids(&self, router: &mut PidRouter) {
                router.register(55001, "gpke-supplier-change");
                router.register(55002, "gpke-supplier-change");
            }
        }

        let ctx = EngineBuilder::new()
            .with_event_store(InMemoryEventStore::new())
            .register(Box::new(PidModule))
            .build();

        assert_eq!(ctx.pid_router().route(55001), Some("gpke-supplier-change"));
        assert_eq!(ctx.pid_router().route(55002), Some("gpke-supplier-change"));
        assert!(ctx.pid_router().route(99999).is_none());
        assert_eq!(ctx.pid_router().len(), 2);
    }

    /// Verify that `register_pids_with_roles` gates PIDs behind role checks.
    ///
    /// Scenario: two modules share PID 19001.
    /// - ModuleA registers 19001 → "workflow-a" when role `Nb` is present.
    /// - ModuleB registers 19001 → "workflow-b" when role `Nmsb` is explicitly set
    ///   (not on `all()`).
    ///
    /// - `all()`: ModuleA fires (Nb ∈ all), ModuleB does NOT (is_all → skip).
    ///   → 19001 routes to "workflow-a".
    /// - `from_roles([Nb])`: ModuleA fires, ModuleB skips.
    ///   → 19001 routes to "workflow-a".
    /// - `from_roles([Nmsb])`: ModuleA skips, ModuleB fires.
    ///   → 19001 routes to "workflow-b".
    #[test]
    fn register_pids_with_roles_gates_pids_correctly() {
        use crate::marktrolle::{DeploymentRoles, Marktrolle};

        struct ModuleA;
        impl EngineModule for ModuleA {
            fn name(&self) -> &'static str {
                "module-a"
            }
            fn register_pids_with_roles(&self, router: &mut PidRouter, roles: &DeploymentRoles) {
                if roles.contains(Marktrolle::Nb) {
                    router.register(19_001, "workflow-a");
                }
            }
        }

        struct ModuleB;
        impl EngineModule for ModuleB {
            fn name(&self) -> &'static str {
                "module-b"
            }
            fn register_pids_with_roles(&self, router: &mut PidRouter, roles: &DeploymentRoles) {
                // Only fires on explicit Nmsb, not on all() (backward-compat sentinel).
                if !roles.is_all() && roles.contains(Marktrolle::Nmsb) {
                    router.register(19_001, "workflow-b");
                    router.register(19_015, "workflow-b");
                }
            }
        }

        let build = |roles: DeploymentRoles| {
            EngineBuilder::new()
                .with_event_store(InMemoryEventStore::new())
                .with_deployment_roles(roles)
                .register(Box::new(ModuleA))
                .register(Box::new(ModuleB))
                .build()
        };

        // all() → backward compat: ModuleA registers 19001 (Nb ∈ all), ModuleB skips.
        let ctx = build(DeploymentRoles::all());
        assert_eq!(ctx.pid_router().route(19_001), Some("workflow-a"));
        assert!(ctx.pid_router().route(19_015).is_none());

        // Explicit Nb → same result: ModuleA registers, ModuleB (nMSB) skips.
        let ctx = build(DeploymentRoles::nb());
        assert_eq!(ctx.pid_router().route(19_001), Some("workflow-a"));
        assert!(ctx.pid_router().route(19_015).is_none());

        // Explicit Nmsb → ModuleA skips (Nb ∉ roles), ModuleB registers.
        let ctx = build(DeploymentRoles::nmsb());
        assert_eq!(ctx.pid_router().route(19_001), Some("workflow-b"));
        assert_eq!(ctx.pid_router().route(19_015), Some("workflow-b"));
    }

    /// Verify that explicit roles with two conflicting modules use first-wins semantics
    /// (the first module to register a PID retains ownership; the second is silently skipped).
    #[test]
    fn register_pids_with_roles_conflict_uses_first_wins_with_explicit_roles() {
        use crate::marktrolle::{DeploymentRoles, Marktrolle};

        struct ConflictA;
        impl EngineModule for ConflictA {
            fn name(&self) -> &'static str {
                "conflict-a"
            }
            fn register_pids_with_roles(&self, router: &mut PidRouter, roles: &DeploymentRoles) {
                if roles.contains(Marktrolle::Nb) {
                    router.register(19_001, "workflow-a");
                }
            }
        }

        struct ConflictB;
        impl EngineModule for ConflictB {
            fn name(&self) -> &'static str {
                "conflict-b"
            }
            fn register_pids_with_roles(&self, router: &mut PidRouter, roles: &DeploymentRoles) {
                if !roles.is_all() && roles.contains(Marktrolle::Nmsb) {
                    router.register(19_001, "workflow-b"); // same PID, different workflow
                }
            }
        }

        // from_roles([Nb, Nmsb]): both modules fire for PID 19_001.
        // First-wins: ConflictA (registered first) retains ownership → "workflow-a".
        let ctx = EngineBuilder::new()
            .with_event_store(InMemoryEventStore::new())
            .with_deployment_roles(DeploymentRoles::from_roles([
                Marktrolle::Nb,
                Marktrolle::Nmsb,
            ]))
            .register(Box::new(ConflictA))
            .register(Box::new(ConflictB))
            .build();
        assert_eq!(
            ctx.pid_router().route(19_001),
            Some("workflow-a"),
            "first module should win on PID conflict with explicit roles"
        );
    }

    // ── Graceful shutdown ─────────────────────────────────────────────────────

    /// Cancelling the token must make `run` return.
    ///
    /// The workers used to loop until the process exited: the shutdown path
    /// cancelled a token nobody read, dropped their `JoinHandle`s — which does
    /// not abort a Tokio task — and then closed the event store underneath
    /// them. An outbox `acknowledge` losing that race leaves the counterparty
    /// holding a message the outbox still shows as pending, and the next start
    /// delivers it again.
    #[tokio::test]
    async fn a_cancelled_outbox_worker_returns() {
        let worker = OutboxWorker {
            store: InMemoryOutboxStore::new(),
            sender: AlwaysDelivers,
            deadline_store: InMemoryDeadlineStore::new(),
            batch_size: 10,
            // Far longer than the timeout below: the point is that cancellation
            // interrupts the idle sleep rather than being noticed after it.
            poll_interval: std::time::Duration::from_secs(300),
            max_attempts: 48,
            max_retry_window: std::time::Duration::from_secs(72 * 3600),
            dead_letter_sink: std::sync::Arc::new(crate::dead_letter::LogDeadLetterSink),
            heartbeat: None,
            shutdown: None,
        };
        let token = tokio_util::sync::CancellationToken::new();
        let worker = worker.with_shutdown(token.clone());

        let handle = tokio::spawn(worker.run());
        // Let it reach the sleep, then signal.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        token.cancel();

        tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("outbox worker must return promptly after cancellation")
            .expect("outbox worker must not panic");
    }

    #[tokio::test]
    async fn a_cancelled_deadline_scheduler_returns() {
        let scheduler = DeadlineScheduler {
            store: InMemoryDeadlineStore::new(),
            dispatch: Box::new(|_| Box::pin(async { Ok(()) })),
            batch_size: 100,
            poll_interval: std::time::Duration::from_secs(300),
            heartbeat: None,
            shutdown: None,
        };
        let token = tokio_util::sync::CancellationToken::new();
        let scheduler = scheduler.with_shutdown(token.clone());

        let handle = tokio::spawn(scheduler.run());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        token.cancel();

        tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("deadline scheduler must return promptly after cancellation")
            .expect("deadline scheduler must not panic");
    }

    /// A token cancelled before the first poll must stop the worker without it
    /// touching the store at all — the case where shutdown arrives during boot.
    #[tokio::test]
    async fn a_worker_cancelled_before_it_starts_does_no_work() {
        let outbox = InMemoryOutboxStore::new();
        let stream_id = crate::ids::StreamId::new("gpke/shutdown-test");
        let msg = outbox_message(&stream_id, "UTILMD");
        outbox.enqueue(std::slice::from_ref(&msg)).await.unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();

        let worker = OutboxWorker {
            store: outbox.clone(),
            sender: AlwaysDelivers,
            deadline_store: InMemoryDeadlineStore::new(),
            batch_size: 10,
            poll_interval: std::time::Duration::from_millis(5),
            max_attempts: 48,
            max_retry_window: std::time::Duration::from_secs(72 * 3600),
            dead_letter_sink: std::sync::Arc::new(crate::dead_letter::LogDeadLetterSink),
            heartbeat: None,
            shutdown: None,
        }
        .with_shutdown(token);

        tokio::time::timeout(std::time::Duration::from_secs(5), worker.run())
            .await
            .expect("an already-cancelled worker must return immediately");

        assert_eq!(
            outbox.pending_now(10).await.unwrap().len(),
            1,
            "the message must stay queued for the next start, not be delivered \
             by a worker that was told to stop",
        );
    }

    // ── APERAK delivery-window discharge ──────────────────────────────────────

    /// A sender that always succeeds, so the worker takes the delivery path.
    struct AlwaysDelivers;
    impl As4Sender for AlwaysDelivers {
        async fn send(&self, _msg: &crate::outbox::OutboxMessage) -> Result<(), EngineError> {
            Ok(())
        }
    }

    fn outbox_message(
        stream_id: &crate::ids::StreamId,
        message_type: &str,
    ) -> crate::outbox::OutboxMessage {
        crate::outbox::OutboxMessage::new(
            stream_id.clone(),
            crate::ids::ProcessId::new(),
            TenantId::new(),
            crate::ids::CorrelationId::new(),
            crate::ids::ConversationId::new(),
            crate::ids::EventId::new(),
            message_type,
            "9900357000004",
            serde_json::json!({}),
        )
    }

    fn deadline_on(
        stream_id: &crate::ids::StreamId,
        msg: &crate::outbox::OutboxMessage,
        label: &str,
    ) -> Deadline {
        Deadline::new(
            stream_id.clone(),
            msg.process_id,
            msg.tenant_id,
            WorkflowId::new("gpke-supplier-change", "FV2025-10-01"),
            label,
            time::OffsetDateTime::now_utc() + time::Duration::hours(6),
        )
    }

    /// Deliver `msg` through the worker's real loop and return the labels that
    /// survive on its stream.
    ///
    /// Drives `run` rather than calling the discharge directly — the wiring is
    /// the thing under test, and calling the method straight passes even when
    /// `run` never invokes it.
    async fn labels_surviving_delivery(
        msg: &crate::outbox::OutboxMessage,
        stream_id: &crate::ids::StreamId,
        registered: &[&str],
    ) -> Vec<String> {
        let deadlines = InMemoryDeadlineStore::new();
        for label in registered {
            deadlines
                .register(&deadline_on(stream_id, msg, label))
                .await
                .unwrap();
        }
        let outbox = InMemoryOutboxStore::new();
        outbox.enqueue(std::slice::from_ref(msg)).await.unwrap();

        let worker = OutboxWorker {
            store: outbox.clone(),
            sender: AlwaysDelivers,
            deadline_store: deadlines.clone(),
            batch_size: 10,
            poll_interval: std::time::Duration::from_millis(5),
            max_attempts: 48,
            max_retry_window: std::time::Duration::from_secs(72 * 3600),
            dead_letter_sink: std::sync::Arc::new(crate::dead_letter::LogDeadLetterSink),
            heartbeat: None,
            shutdown: None,
        };
        // `run` never returns; give it enough cycles to drain the one message.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(300), worker.run()).await;
        assert!(
            outbox.pending_now(10).await.unwrap().is_empty(),
            "the message must have been delivered and acknowledged",
        );

        let mut left: Vec<String> = deadlines
            .for_stream(stream_id)
            .await
            .unwrap()
            .iter()
            .map(|d| d.label().to_owned())
            .collect();
        left.sort();
        left
    }

    /// Delivering a message must retire the window that was watching for it.
    ///
    /// These windows are registered when the message is enqueued and nothing
    /// else ever cancels them, so without the discharge they fire for **every**
    /// process — including every one that answered on time. The scheduler cannot
    /// tell those apart (a deadline reaching `due_now` is late by construction),
    /// so the miss counters would track processes started, not obligations
    /// missed.
    #[tokio::test]
    async fn delivering_a_message_discharges_its_delivery_window() {
        // (message type, the window it answers for)
        for (message_type, window) in [
            ("APERAK", mako_fristen::APERAK_STROM_WINDOW_LABEL),
            ("APERAK", mako_fristen::APERAK_GAS_FOLGEPROZESS_LABEL),
            ("APERAK", mako_fristen::APERAK_GAS_INITIALPROZESS_LABEL),
            ("CONTRL", mako_fristen::CONTRL_FRIST_LABEL),
        ] {
            let stream_id = crate::ids::StreamId::new("gpke-supplier-change-1");
            let msg = outbox_message(&stream_id, message_type);
            // A process-response deadline shares the stream and must survive:
            // it is waiting on the counterparty, not on our delivery.
            let left =
                labels_surviving_delivery(&msg, &stream_id, &[window, "gpke-response-window"])
                    .await;
            assert_eq!(
                left,
                vec!["gpke-response-window"],
                "delivering {message_type} must discharge `{window}` and leave \
                 every other deadline alone",
            );
        }
    }

    /// A delivery must not discharge a *different* message's window.
    ///
    /// The CONTRL and APERAK obligations run concurrently on the same
    /// interchange. Acknowledging syntax (CONTRL) says nothing about whether the
    /// application-level APERAK went out, so discharging both on one delivery
    /// would silence a real violation.
    #[tokio::test]
    async fn a_delivery_does_not_discharge_another_messages_window() {
        let stream_id = crate::ids::StreamId::new("gpke-supplier-change-1");
        let contrl = outbox_message(&stream_id, "CONTRL");

        let left = labels_surviving_delivery(
            &contrl,
            &stream_id,
            &[
                mako_fristen::CONTRL_FRIST_LABEL,
                mako_fristen::APERAK_STROM_WINDOW_LABEL,
            ],
        )
        .await;

        assert_eq!(
            left,
            vec![mako_fristen::APERAK_STROM_WINDOW_LABEL.to_owned()],
            "a delivered CONTRL discharges only the CONTRL window; the APERAK \
             obligation is still outstanding",
        );
    }

    /// Every delivery-window label must be discharged by the message it watches.
    ///
    /// This is the invariant the miss counters rest on. A window label that
    /// `discharges_delivery_window` does not recognise is never retired, so it
    /// fires on every process and is counted as a regulatory violation each
    /// time — which is precisely how `makod_aperak_missed_total` once came to
    /// count Strom processes rather than missed APERAKs.
    ///
    /// Adding a delivery window means adding a row here.
    #[test]
    fn every_delivery_window_label_is_discharged_by_its_message() {
        for (message_type, label) in [
            ("APERAK", mako_fristen::APERAK_STROM_WINDOW_LABEL),
            ("APERAK", mako_fristen::APERAK_GAS_FOLGEPROZESS_LABEL),
            ("APERAK", mako_fristen::APERAK_GAS_INITIALPROZESS_LABEL),
            ("CONTRL", mako_fristen::CONTRL_FRIST_LABEL),
        ] {
            assert!(
                mako_fristen::discharges_delivery_window(message_type, label),
                "delivering {message_type} must discharge `{label}`, or the window \
                 outlives its obligation and alerts on every process",
            );
        }
    }

    // ── Retry-budget classification ───────────────────────────────────────────

    /// Sink double that records every rejection's attempt count.
    #[derive(Default)]
    struct RecordingSink(std::sync::Mutex<Vec<u32>>);
    impl crate::dead_letter::DeadLetterSink for std::sync::Arc<RecordingSink> {
        fn reject(&self, reason: &crate::dead_letter::DeadLetterReason) {
            if let crate::dead_letter::DeadLetterReason::OutboxExhausted { attempts, .. } = reason {
                self.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(*attempts);
            }
        }
    }

    struct NoRenderer;
    impl As4Sender for NoRenderer {
        async fn send(&self, msg: &crate::outbox::OutboxMessage) -> Result<(), EngineError> {
            Err(EngineError::RendererNotImplemented {
                message_type: msg.message_type.as_ref().into(),
                message_id: msg.message_id.to_string().into(),
            })
        }
    }

    async fn drive_worker<S: As4Sender>(
        sender: S,
        msg: crate::outbox::OutboxMessage,
    ) -> (InMemoryOutboxStore, std::sync::Arc<RecordingSink>) {
        let outbox = InMemoryOutboxStore::new();
        outbox.enqueue(std::slice::from_ref(&msg)).await.unwrap();
        let sink = std::sync::Arc::new(RecordingSink::default());
        let worker = OutboxWorker {
            store: outbox.clone(),
            sender,
            deadline_store: InMemoryDeadlineStore::new(),
            batch_size: 10,
            poll_interval: std::time::Duration::from_millis(5),
            max_attempts: 48,
            max_retry_window: std::time::Duration::from_secs(72 * 3600),
            dead_letter_sink: std::sync::Arc::new(std::sync::Arc::clone(&sink)),
            heartbeat: None,
            shutdown: None,
        };
        let _ = tokio::time::timeout(std::time::Duration::from_millis(300), worker.run()).await;
        (outbox, sink)
    }

    /// `RendererNotImplemented` is documented as permanent — the worker must
    /// dead-letter it on the *first* attempt, not burn the retry budget on a
    /// failure that cannot heal between attempts. Until the permanent arm
    /// matched it, this promise was broken.
    #[tokio::test(start_paused = true)]
    async fn a_missing_renderer_dead_letters_without_retrying() {
        let stream_id = crate::ids::StreamId::new("test-renderer-missing");
        let msg = outbox_message(&stream_id, "MSCONS");
        let (outbox, sink) = drive_worker(NoRenderer, msg).await;

        assert!(
            outbox.pending_now(10).await.unwrap().is_empty(),
            "the message must be acknowledged, not left for another attempt",
        );
        let rejections = sink
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            rejections,
            vec![0],
            "exactly one dead-letter, on the first attempt (attempt_count 0)",
        );
    }

    /// The retry budget is a *window*, not a count: a message whose age has
    /// exceeded it after at least one attempt is dead-lettered even though the
    /// attempt belt is nowhere near exhausted — full-jitter backoff makes a
    /// count no proxy for the 72 h duty.
    #[tokio::test(start_paused = true)]
    async fn an_aged_message_with_a_prior_attempt_is_dead_lettered() {
        let stream_id = crate::ids::StreamId::new("test-window-exhausted");
        let mut msg = outbox_message(&stream_id, "UTILMD");
        msg.created_at = time::OffsetDateTime::now_utc() - time::Duration::hours(73);
        msg.attempt_count = 1;
        let (outbox, sink) = drive_worker(AlwaysDelivers, msg).await;

        assert!(
            outbox.pending_now(10).await.unwrap().is_empty(),
            "the exhausted message must leave the outbox",
        );
        let rejections = sink
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            rejections,
            vec![1],
            "the window, not the attempt belt, must have dead-lettered it",
        );
    }

    /// A message that aged past the window while the worker was down still
    /// gets its first try — the window is only consulted after an attempt, so
    /// downtime never buries a message unsent.
    #[tokio::test(start_paused = true)]
    async fn an_aged_message_with_no_attempts_is_still_tried_once() {
        let stream_id = crate::ids::StreamId::new("test-aged-first-try");
        let mut msg = outbox_message(&stream_id, "UTILMD");
        msg.created_at = time::OffsetDateTime::now_utc() - time::Duration::hours(200);
        let (outbox, sink) = drive_worker(AlwaysDelivers, msg).await;

        assert!(
            outbox.pending_now(10).await.unwrap().is_empty(),
            "the message must have been delivered and acknowledged",
        );
        assert!(
            sink.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "delivery, not dead-lettering: age alone must never bury a message",
        );
    }
}
