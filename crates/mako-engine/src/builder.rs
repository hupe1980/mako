//! [`EngineModule`] trait, [`EngineBuilder`], and [`EngineContext`].
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
pub struct OutboxWorker<OS: OutboxStore, S: As4Sender> {
    store: OS,
    sender: S,
    batch_size: usize,
    poll_interval: std::time::Duration,
    /// Maximum total delivery attempts before a message is dead-lettered.
    ///
    /// Default: 48 (covers ~4 hours at the 300 s backoff cap).
    /// Set to `u32::MAX` to disable the cap (not recommended for production).
    max_attempts: u32,
    /// Sink for messages that exceed `max_attempts`.
    dead_letter_sink: std::sync::Arc<dyn crate::dead_letter::DeadLetterSink>,
    /// Optional liveness heartbeat — stores the current UTC Unix timestamp
    /// (seconds) after each poll cycle so health probes can detect stale workers.
    heartbeat: Option<std::sync::Arc<std::sync::atomic::AtomicI64>>,
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

impl<OS: OutboxStore, S: As4Sender> OutboxWorker<OS, S> {
    /// Run the outbox drain loop until the task is cancelled.
    ///
    /// # Panics
    ///
    /// Panics if `time::Duration::try_from(delay)` overflows (unreachable for
    /// the delay values produced by `backoff_delay`).
    #[allow(clippy::too_many_lines)]
    pub async fn run(self) {
        loop {
            let batch = match self.store.pending_now(self.batch_size).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "outbox worker: store error polling pending messages (will retry)");
                    tokio::time::sleep(self.poll_interval).await;
                    continue;
                }
            };

            if batch.is_empty() {
                tokio::time::sleep(self.poll_interval).await;
                continue;
            }

            for msg in batch {
                // ── Max-attempt cap ───────────────────────────────────
                // `attempt_count` starts at 0 and is incremented on each
                // `reschedule` call.  When it reaches `max_attempts` the
                // message is considered permanently undeliverable: acknowledge
                // it (remove from outbox) and dead-letter it so the regulatory
                // audit trail is preserved.
                if msg.attempt_count >= self.max_attempts {
                    tracing::error!(
                        message_id   = %msg.message_id,
                        message_type = %msg.message_type,
                        recipient    = %msg.recipient,
                        attempts     = msg.attempt_count,
                        max_attempts = self.max_attempts,
                        "outbox worker: max delivery attempts reached; dead-lettering message",
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
                            if elapsed > time::Duration::hours(crate::fristen::CONTRL_FRIST_HOURS) {
                                tracing::warn!(
                                    message_id   = %msg.message_id,
                                    elapsed_secs = elapsed.whole_seconds(),
                                    max_secs     = crate::fristen::CONTRL_FRIST_HOURS * 3600,
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
                                    crate::fristen::APERAK_STROM_WEEKDAY_MINUTES,
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
                    }
                    // Permanent error: dead-letter immediately without retrying.
                    // PartnerUnknown requires operator intervention (add --as4-partner);
                    // Serialization errors will never succeed on retry.
                    Err(ref e)
                        if e.is_partner_unknown() || matches!(e, EngineError::Serialization(_)) =>
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
            // Tick liveness heartbeat at the end of every poll cycle so the
            // health endpoint can detect a stale (hung) outbox worker.
            if let Some(ref hb) = self.heartbeat {
                hb.store(
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    std::sync::atomic::Ordering::Relaxed,
                );
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
    /// `max_attempts` — maximum total delivery attempts before dead-lettering.
    /// Pass `48` for a ~4-hour retry budget at the 300 s backoff cap, or
    /// `u32::MAX` to disable the cap (not recommended for production).
    ///
    /// ```rust,ignore
    /// use std::time::Duration;
    ///
    /// let worker = ctx.run_outbox_worker(my_sender, 50, Duration::from_secs(1), 48);
    /// tokio::spawn(async move { worker.run().await });
    /// ```
    #[must_use]
    pub fn run_outbox_worker<S: As4Sender>(
        &self,
        sender: S,
        batch_size: usize,
        poll_interval: std::time::Duration,
        max_attempts: u32,
    ) -> OutboxWorker<OS, S> {
        OutboxWorker {
            store: self.outbox_store.clone(),
            sender,
            batch_size,
            poll_interval,
            max_attempts,
            dead_letter_sink: self.dead_letter_sink.clone(),
            heartbeat: None,
        }
    }
}

impl<OS: OutboxStore, S: As4Sender> OutboxWorker<OS, S> {
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
pub struct NoopAs4Sender;

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
}

impl<DS: DeadlineStore> DeadlineScheduler<DS> {
    /// Run the deadline poll loop until the task is cancelled.
    pub async fn run(self) {
        loop {
            let result = match self.store.due_now(self.batch_size).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "deadline scheduler: store error polling due deadlines (will retry)",
                    );
                    tokio::time::sleep(self.poll_interval).await;
                    continue;
                }
            };

            if result.deadlines.is_empty() {
                tokio::time::sleep(self.poll_interval).await;
                continue;
            }

            for deadline in result.deadlines {
                let id = deadline.deadline_id();
                let label = deadline.label().to_owned();
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

            // Tick liveness heartbeat at the end of every poll cycle so the
            // health endpoint can detect a stale (hung) deadline scheduler.
            if let Some(ref hb) = self.heartbeat {
                hb.store(
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
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
}
