//! Startup phases, factored out of `async_main`.
//!
//! The phase table below links the functions it names. They are deliberately
//! crate-private — a startup phase is not API — so the links only resolve under
//! `--document-private-items`, which is the readership this module has.
#![allow(rustdoc::private_intra_doc_links)]
//!
//! `main` reads as the boot *order*; the work each step does lives here, so that
//! order stays visible rather than buried under state construction. Each phase
//! is independently callable, which is what lets the startup guards run as
//! ordinary tests.
//!
//! | Function | Phase |
//! |---|---|
//! | [`lock_data_dir`] | Refuse a second writer on the same SlateDB path |
//! | [`build_engine`] | Register the compiled-in domain modules; build the [`EngineContext`] |
//! | [`validate_adapter_coverage`] | Every adapter registry covers every active format version |
//! | [`validate_dispatch_completeness`] | Every routed workflow has an ingest arm |
//! | [`servers`] | Bind and spawn the HTTP, AS4 and API-Webdienste ports |
//! | [`spawn_workers`] | Outbox, ERP webhook, deadlines, projections, retention purge |
//!
//! Everything above `spawn_workers` runs in `--check` mode too, which is why the
//! validations panic or return errors rather than logging: `--check` promises
//! that exit 0 means the configuration will start.
//!
//! ## Shutdown
//!
//! [`spawn_workers`] returns [`WorkerHandles`]. `main` cancels the shared token
//! and then *joins* them before closing the event store — see
//! [`WorkerHandles::join_all`] for why the join, not the cancel, is the
//! load-bearing step.
//!
//! ## Type alias
//!
//! `MakodCtx` names the concrete `EngineContext` type used throughout the
//! production daemon. Tests that need an engine context can build one with
//! `EngineBuilder::with_stores(...)` and store it as `MakodCtx`.
//!
//! [`EngineContext`]: mako_engine::builder::EngineContext

use std::sync::Arc;
use std::time::Duration;

use edi_energy::Platform;
use mako_engine::{
    builder::{EngineBuilder, EngineContext},
    store_slatedb::{
        SlateDbDeadlineStore, SlateDbInboxStore, SlateDbProcessRegistry, SlateDbSnapshotStore,
        SlateDbStore,
    },
};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::{
    adapters, deadline_dispatch, erp_adapter, ingest_dispatcher, malo_cache,
    party_registry::MpIdRegistry,
};

pub(crate) mod servers;

// ── HTTP middleware ───────────────────────────────────────────────────────────

/// Scope the request's W3C `traceparent` header into the engine task-local.
///
/// Every `OutboxMessage` created while handling the request captures it into
/// its persisted `trace_context`, and the delivery workers re-inject it into
/// outbound HTTP — end-to-end tracing across the asynchronous outbox boundary.
///
/// Layered onto all three ports by `servers`, which is why it lives here rather
/// than in the binary: `main.rs` and the lib target compile these files
/// separately, and only one of them can own the symbol the routers reference.
pub(crate) async fn trace_ctx_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let tp = req
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    mako_engine::trace_ctx::TRACEPARENT
        .scope(tp, next.run(req))
        .await
}

/// The outbox retry duty: unacknowledged messages are retried for **72 hours**
/// (BDEW AS4 Kommunikationshandbuch, via
/// [`mako_as4::constants::MAX_RETRY_DURATION_SECS`]) before dead-lettering.
/// This window is the budget; the attempt count below is only a runaway belt —
/// the backoff is full-jitter, so a count cannot promise a duration.
const OUTBOX_RETRY_WINDOW: Duration =
    Duration::from_secs(mako_as4::constants::MAX_RETRY_DURATION_SECS);

/// Attempt belt for the outbox worker. At the 300 s backoff cap the expected
/// cadence is ~150 s, so 72 h needs ≈ 1 700 attempts; 10 000 leaves jitter
/// headroom while still bounding a pathological loop long before it matters.
const OUTBOX_MAX_ATTEMPTS: u32 = 10_000;

// ── Type aliases ──────────────────────────────────────────────────────────────

/// Concrete [`EngineContext`] type used by the `makod` production daemon.
///
/// Type alias for the five-parameter generic with the SlateDB-backed store
/// types.  Useful in tests and startup helpers to avoid repeating all five
/// type parameters.
pub(crate) type MakodCtx = EngineContext<
    SlateDbStore,
    SlateDbSnapshotStore,
    SlateDbStore,
    SlateDbDeadlineStore,
    SlateDbProcessRegistry,
>;

// ── lock_data_dir ─────────────────────────────────────────────────────────────

/// Take an exclusive lock on the data directory, creating it if needed.
///
/// Two concurrent writers against one SlateDB path corrupt the write-ahead log
/// and produce split-brain event sequences, so a second instance is refused
/// rather than allowed to interleave.
///
/// Returns `None` in volatile mode, where there is no directory to guard.
///
/// The returned guard borrows `'static` because the underlying `RwLock<File>`
/// is deliberately leaked: the lock must outlive every scope in `main`, and the
/// file descriptor is reclaimed by the OS at process exit either way. `Box::leak`
/// is safe std, so no `unsafe` is involved.
///
/// # Errors
///
/// Returns an error when the directory cannot be created or opened, or when
/// another `makod` already holds the lock.
pub(crate) fn lock_data_dir(
    data_dir: Option<&std::path::Path>,
) -> anyhow::Result<Option<fd_lock::RwLockWriteGuard<'static, std::fs::File>>> {
    let Some(data_dir) = data_dir else {
        return Ok(None);
    };
    std::fs::create_dir_all(data_dir)?;
    let lock_path = data_dir.join(".makod.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    let lock_ref: &'static mut fd_lock::RwLock<std::fs::File> =
        Box::leak(Box::new(fd_lock::RwLock::new(lock_file)));
    match lock_ref.try_write() {
        Ok(guard) => {
            info!(path = %lock_path.display(), "acquired exclusive data-dir lock");
            Ok(Some(guard))
        }
        // Returned, not `process::exit`: exiting here skips the OpenTelemetry
        // guard's flush, so the span explaining the refusal never reaches the
        // collector — on the one failure an operator most needs the trace for.
        Err(e) => anyhow::bail!(
            "another makod instance is already using the data directory {} ({e}). \
             Refusing to start: two writers on one SlateDB path corrupt the \
             write-ahead log and produce split-brain event sequences.",
            lock_path.display(),
        ),
    }
}

// ── build_engine ──────────────────────────────────────────────────────────────

/// Register the compiled-in domain modules and assemble the [`EngineContext`].
///
/// Which modules exist is a *build-time* choice: the `role-*` features gate each
/// `push` below, so a role-scoped binary carries neither the code nor the PID
/// registrations for roles it does not deploy. The binary SHA therefore
/// identifies the role scope for a BNetzA audit.
///
/// # Panics
///
/// Panics when the build selected no Marktrolle at all — such a binary compiles
/// but routes nothing, and would silently dead-letter every inbound message.
/// `EngineBuilder::build` additionally panics when a registered module declares
/// a message type with no active `edi-energy` profile.
/// Every domain module this build registers, in registration order.
///
/// **The single list.** Production calls it from [`build_engine`]; the
/// coverage guards in `tests/` call it instead of restating the stack. Four
/// test files used to keep their own copy and two of them fell behind when a
/// module was added — an omitted module registers workflows whose deadlines
/// nothing dispatches and silently shrinks every figure the guards pin, so the
/// list has to exist once.
///
/// Each entry is gated on the deployment roles that need it, so a role-scoped
/// build registers fewer. The `default` feature set names every role, which is
/// why a default build — what the guards run — yields the full stack.
///
/// Role → module mapping:
/// - `role-lf-strom` / `role-nb-strom` → `GpkeModule` (both sides; the
///   `PidRouter` separates them by Marktrolle)
/// - `role-msb-*` / `role-nb-*` / `role-esa-strom` → `WimModule` (WiM in
///   beiden Sparten, incl. the WiM Teil 2 ESA leg)
/// - `role-lf-gas` / `role-nb-gas` → `GeliGasModule`
/// - `role-nb-gas` → `GaBiGasModule`
/// - `role-nb-strom` → `MabisModule`, `RedispatchModule`
/// - `role-nb-strom` / `role-lf-strom` → `EmobModule` (all three parties to
///   Modell 2 answer one of its legs)
///
/// # Panics
///
/// When the build selected no Marktrolle at all, so every module is gated out
/// and the `PidRouter` would be empty.
#[must_use]
#[expect(
    clippy::vec_init_then_push,
    reason = "the pushes are #[cfg]-gated and cannot be merged into a vec![] literal"
)]
pub fn production_modules() -> Vec<Box<dyn mako_engine::builder::EngineModule>> {
    let mut m: Vec<Box<dyn mako_engine::builder::EngineModule>> = Vec::new();

    // GpkeModule — GPKE 55001–55018/55022–55024/55555/55600–55609 +
    //   INVOIC 31001/31002/31005/31006 + ORDERS Sperrung 17115–17117 +
    //   ORDERS/ORDRSP Konfiguration 17134/17135/19001/19002 + PARTIN
    //   37000–37006. Both sides of GPKE live here; the PidRouter separates
    //   them by Marktrolle.
    #[cfg(any(feature = "role-lf-strom", feature = "role-nb-strom"))]
    m.push(Box::new(mako_gpke::GpkeModule));

    // WimModule — Messstellenbetrieb in **beiden Sparten**: 55039/55042/
    //   55051/55168 and the Gas twins 44039/44042/44051/44168/44183, ORDERS
    //   Geräteübernahme 17001–17011, INSRPT 23001–23012, INVOIC
    //   31009/31003/31004, and the ESA-side Wertebestellung of WiM Teil 2
    //   Kap. 4. One crate for both, so every WiM role loads it.
    #[cfg(any(
        feature = "role-msb-strom",
        feature = "role-msb-gas",
        feature = "role-nb-strom",
        feature = "role-nb-gas",
        feature = "role-esa-strom",
    ))]
    m.push(Box::new(mako_wim::WimModule));

    // GeliGasModule — GeLi Gas 3.0 44001–44024 (incl. the Stornierung it
    //   shares with WiM Gas) + ORDERS Sperrung Gas 17115–17117 + PARTIN Gas
    //   37008–37014 + INVOIC 31011 (AWH Rechnung).
    #[cfg(any(feature = "role-lf-gas", feature = "role-nb-gas"))]
    m.push(Box::new(mako_geli_gas::GeliGasModule));

    // GaBiGasModule — INVOIC 31007/31008/31010 + REMADV 33001 +
    //   COMDIS 29001 + MSCONS 13013 Allokationsliste. BKV/MGV interactions
    //   are GNB-side (BK7-24-01-008).
    #[cfg(feature = "role-nb-gas")]
    m.push(Box::new(mako_gabi_gas::GaBiGasModule));

    // MabisModule — Bilanzkreisabrechnung Strom (BK6-24-174): MSCONS
    //   Summenzeitreihen, IFTSTA Datenstatus, the MaBiS-ZP lifecycle.
    #[cfg(feature = "role-nb-strom")]
    m.push(Box::new(mako_mabis::MabisModule));

    // EmobModule — NZR-EMob / Modell 2 (BK6-20-160 Anlage 6, BK6-24-267):
    //   UTILMD 55238–55243, the three legs that move a Marktlokation into
    //   and out of the LPB's Bilanzierungsgebiet.
    //
    //   Loaded for **both** Strom roles, because all three parties to the
    //   model answer one of the legs: the VNB answers the Anmeldung and the
    //   Abmeldung, the **LF** answers the Beendigung der Zuordnung (55240),
    //   and the LPB — whose wire role is NB — receives both its own
    //   answers. A `Marktrolle::Lpb` deployment is an NB deployment.
    //
    //   55235–55237 (Zuordnung des ZP der NGZ zur NZR) are deliberately
    //   **not** here: they are MaBiS — UTILMD AHB Strom 2.2 Kap. 13.16,
    //   answered from `E_0102`/`E_0103` — and `MabisModule` registers them
    //   as the `ZpSerie::NetzgangzeitreiheNzr` family of
    //   `mabis-zp-lifecycle`.
    #[cfg(any(feature = "role-nb-strom", feature = "role-lf-strom"))]
    m.push(Box::new(mako_emob::EmobModule));

    // RedispatchModule — Redispatch 2.0 (§§ 13/13a/14 EnWG); XML routing +
    //   IFTSTA 21037/21038. VNB, ANB and ÜNB share the NB Strom deployment
    //   role; LF, MSB and gas-only deployments have no §13a obligations
    //   (BK6-20-059/060/061).
    #[cfg(feature = "role-nb-strom")]
    m.push(Box::new(mako_redispatch::RedispatchModule));

    // A binary with no role compiles but can route nothing — every module
    // is gated out and the PidRouter is empty. Catch it here rather than
    // shipping a daemon that silently dead-letters every inbound message.
    assert!(
        !m.is_empty(),
        "this build selected no Marktrolle: every domain module was gated out. \
         Build with the default features, or name at least one role feature \
         (e.g. --no-default-features --features role-lf)."
    );
    m
}

pub(crate) fn build_engine(
    store: &SlateDbStore,
    dead_letter_sink: mako_engine::store_slatedb::SlateDbDeadLetterSink,
    deployment_roles: mako_engine::marktrolle::DeploymentRoles,
) -> MakodCtx {
    let modules = production_modules();

    EngineBuilder::with_stores(
        store.clone(),
        store.as_deadline_store(),
        store.as_process_registry(),
    )
    .with_event_store(store.clone())
    // Wire the durable snapshot store so replay cost is bounded to at most
    // 100 tail events per command dispatch instead of O(n) full replay.
    .with_snapshot_store(store.as_snapshot_store())
    // Wire the buffered dead-letter sink so every rejected EDIFACT message is
    // persisted to SlateDB for regulatory audit.
    .with_dead_letter_sink(dead_letter_sink)
    // Validate at startup that each domain module has an active edi-energy
    // profile for its declared message types.  The validator runs inside
    // EngineBuilder::build so a missing profile panics with an actionable message
    // rather than silently dead-lettering at first dispatch.
    .with_profile_validator({
        let today = mako_fristen::heute();
        move |msg_type| {
            // If the type code is unrecognised, treat as missing (fail-safe).
            let Some(mt) = edi_energy::MessageType::from_unh_code(msg_type) else {
                return false;
            };
            edi_energy::registry::ReleaseRegistry::global()
                .profiles_for(mt)
                .any(|p| match (p.valid_from(), p.valid_until()) {
                    (Some(from), Some(until)) => from <= today && today <= until,
                    (Some(from), None) => from <= today,
                    (None, _) => true, // legacy profile — always active
                })
        }
    })
    .register_many(modules)
    .with_deployment_roles(deployment_roles)
    .build()
}

// ── validate_adapter_coverage ─────────────────────────────────────────────────

/// Validate that every domain adapter registry covers every active BDEW format
/// version.
///
/// Called once during startup, before any worker is spawned and before the
/// `--check` early exit.
///
/// The registry list is derived from [`adapters::coverage`], not maintained
/// here. The previous version of this function was a hand-written table of
/// workflow names, and it had drifted: twenty of the seventy-two registries
/// were absent from it and were therefore never checked at all.
///
/// # Panics
///
/// Panics when a registry holds no adapter for one or more active format
/// versions, or when the compiled `edi-energy` profile registry declares no
/// format version at all. Both are hard fail-fast conditions: inbound messages
/// would otherwise be silently dead-lettered rather than dispatched.
pub(crate) fn validate_adapter_coverage() {
    let known = adapters::known_fvs();
    assert!(
        !known.is_empty(),
        "startup failure: the compiled edi-energy profile registry declares no \
         BDEW format version. Every adapter accepts only known format versions, \
         so nothing would parse. Check the edi-energy feature set."
    );
    let fv_names: Vec<&str> = known.iter().map(|fv| fv.as_str()).collect();

    let report = adapters::coverage();
    let gaps: Vec<String> = report
        .iter()
        .filter(|c| !c.uncovered.is_empty())
        .map(|c| {
            let missing: Vec<&str> = c.uncovered.iter().map(|fv| fv.as_str()).collect();
            format!("{} has no adapter for {missing:?}", c.registry)
        })
        .collect();
    assert!(
        gaps.is_empty(),
        "startup failure: adapter coverage is incomplete:\n  {}\n\
         Register the missing adapters in orchestrator/adapters/.",
        gaps.join("\n  ")
    );

    info!(
        registries = report.len(),
        adapters = report.iter().map(|c| c.adapters).sum::<usize>(),
        format_versions = ?fv_names,
        "adapter coverage validated"
    );
}

// ── validate_dispatch_completeness ───────────────────────────────────────────

/// Validate that every workflow name reachable via `PidRouter` has a
/// corresponding dispatch arm in [`EdifactIngestDispatcher`].
///
/// This is a startup guard against the gap where a domain crate registers a
/// new PID → workflow mapping, but the developer forgets to add a matching
/// `match` arm in `ingest_dispatcher.rs`.  Without this check, inbound
/// messages for the new PID would be silently dead-lettered at runtime.
///
/// # How it works
///
/// 1. Enumerate all unique workflow names from `router` (both unambiguous and
///    commodity-qualified entries).
/// 2. Compare against [`EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES`] — the
///    compile-time list of workflow names that have a dispatch arm.
/// 3. Panic with an actionable message listing every undispatched workflow.
///
/// # When to update `KNOWN_WORKFLOW_NAMES`
///
/// When adding a new PID in a domain crate's `register_pids`:
/// 1. Add a dispatch arm in `ingest_dispatcher.rs::dispatch`.
/// 2. Add the workflow name string to `EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES`.
///
/// # Panics
///
/// Panics when any PidRouter-registered workflow name is absent from
/// `EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES`.
pub(crate) fn validate_dispatch_completeness(router: &mako_engine::pid_router::PidRouter) {
    use std::collections::HashSet;
    let known: HashSet<&str> = ingest_dispatcher::EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES
        .iter()
        .copied()
        .collect();

    let mut missing: Vec<&str> = router
        .workflow_names()
        .into_iter()
        .filter(|name| !known.contains(name))
        .collect();
    missing.sort_unstable();

    if !missing.is_empty() {
        panic!(
            "startup failure: the following workflows are registered in the PidRouter \
             but have no dispatch arm in EdifactIngestDispatcher:\n  {}\n\
             Add a dispatch arm in ingest_dispatcher.rs AND add the workflow name to \
             EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES.",
            missing.join("\n  ")
        );
    }
    // Log what this binary actually registers, not only what the dispatcher
    // could handle. `KNOWN_WORKFLOW_NAMES` is a const array and identical in
    // every build, so on its own it reads as "all workflows active" even in a
    // role-scoped deployment that registers a fraction of them. The registered
    // counts are what identify the binary's role scope for a BNetzA audit.
    info!(
        registered_workflows = router.workflow_names().len(),
        registered_pids = router.registered_pids().count(),
        dispatch_arms = ingest_dispatcher::EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES.len(),
        "dispatch completeness validated"
    );
}

// ── spawn_workers ─────────────────────────────────────────────────────────────

/// Configuration for all background workers spawned after server bind.
///
/// Build from the parsed [`Cli`] and assembled engine context, then pass to
/// [`spawn_workers`].
///
/// [`Cli`]: crate::main::Cli
pub(crate) struct WorkersConfig {
    /// Assembled engine context (consumed by outbox and deadline workers).
    pub ctx: MakodCtx,
    /// Store shared across projection and ERP workers.
    pub store: SlateDbStore,
    /// Inbox store swept by the daily retention purge task.
    pub inbox_store_for_purge: SlateDbInboxStore,
    /// Shared Platform instance (used by the AS4 loopback path).
    pub platform: Arc<Platform>,
    /// Ingest dispatcher (used by the AS4 loopback path).
    pub ingest_dispatcher: Arc<ingest_dispatcher::EdifactIngestDispatcher>,
    /// Shared HTTP client (OIDC JWKS, MaLo-ID callbacks).
    pub http_client: reqwest::Client,
    /// MaLo cache (for MaloIdentSender and MCP server).
    pub malo_cache: Arc<malo_cache::SlateDbMaloCache>,
    /// Graceful-shutdown token. Every worker below observes it and returns at
    /// its next message/tick boundary, so `main` can await the returned
    /// [`WorkerHandles`] before closing the event store.
    pub shutdown_token: CancellationToken,
    // ── Outbound AS4 config ──────────────────────────────────────────────
    /// MP-ID registry — maps roles to MP-IDs and provides own-MP-ID detection.
    ///
    /// Built from `[[party]]` entries in `makod.toml`. The primary MP-ID is used
    /// as the storage partition key (`TenantId`) and AS4 `partyId` fallback.
    pub mp_id_registry: Arc<MpIdRegistry>,
    /// Configuration validated by `core::preflight` — the AS4 partner P-Mode
    /// registry, MaLo-ID callback endpoints, and the Verzeichnisdienst URL are
    /// already parsed and proven here, so this module never re-parses a flag.
    pub checked: crate::preflight::Preflight,
    pub as4_signing_key_pem: Option<SecretString>,
    pub as4_signing_cert_pem: Option<String>,
    pub as4_trust_anchor_pem: Option<String>,
    /// When `true`, a missing/mismatched synchronous `eb:Receipt` only warns
    /// instead of failing the delivery (interop debugging; strict by default).
    pub as4_lenient_receipts: bool,
    // ── ERP webhook config ───────────────────────────────────────────────
    pub erp_webhook_url: Option<String>,
    pub erp_webhook_secret: Option<SecretString>,
    // ── EDIFACT outbox webhook (dev/no-AS4 mode) ─────────────────────────
    pub edifact_outbox_webhook_url: Option<String>,
    // ── §20b EnWG Netzzugangsplattform adapter ───────────────────────────
    /// Platform endpoint URL — absent until a §20b interface exists; the
    /// sender then falls back to the ERP webhook (operator submits via the
    /// NB Webportal).
    pub netzzugang_endpoint_url: Option<String>,
    /// marktd client for the `netzzugang_antraege` projection.
    pub marktd_client: Option<Arc<mako_markt::marktd_client::MarktdClient>>,
    /// Shared durable dead-letter sink (backed by the same SlateDB store and
    /// drained by the dead-letter worker spawned in `main`).
    pub dead_letter_sink: mako_engine::store_slatedb::SlateDbDeadLetterSink,
    // ── Scheduler / timing config ────────────────────────────────────────
    pub snapshot_interval: u64,
    pub deadline_poll_interval_secs: u64,
    pub projection_checkpoint_interval: u64,
    /// Health state — worker heartbeats are registered here after spawning.
    pub health_state: crate::health::HealthState,
}

/// Handles for every spawned background worker and HTTP listener.
///
/// Returned so `main` can await them after cancelling the shutdown token.
/// Closing the event store while a worker is still writing to it risks a
/// half-applied outbox acknowledge — the counterparty has the message, the
/// outbox still shows it pending, and the next start delivers it again. An
/// in-flight HTTP request is the same hazard from the ingest side, which is why
/// the listeners are joined here too rather than left to their own drain.
pub(crate) struct WorkerHandles(Vec<tokio::task::JoinHandle<()>>);

impl WorkerHandles {
    /// Track one more task in this drain set.
    pub(crate) fn push(&mut self, handle: tokio::task::JoinHandle<()>) {
        self.0.push(handle);
    }

    /// Wait for every task to return, giving up after `timeout`.
    ///
    /// Returns `true` when all of them stopped on their own. `false` means at
    /// least one was still running — the caller should treat the shutdown as
    /// unclean, because the store is about to be closed underneath it.
    pub(crate) async fn join_all(self, timeout: Duration) -> bool {
        let count = self.0.len();
        if count == 0 {
            return true;
        }
        let joined = tokio::time::timeout(timeout, async {
            for h in self.0 {
                // A worker that panicked already logged; its handle resolving to
                // Err is not a reason to keep the rest of the drain waiting.
                if let Err(e) = h.await {
                    tracing::error!(error = %e, "background worker terminated abnormally");
                }
            }
        })
        .await;
        match joined {
            Ok(()) => {
                info!(workers = count, "all background workers stopped");
                true
            }
            Err(_) => {
                tracing::error!(
                    workers = count,
                    timeout_secs = timeout.as_secs(),
                    "background workers did not stop within the shutdown timeout; \
                     closing the store anyway — in-flight writes may be lost",
                );
                false
            }
        }
    }
}

/// Spawn all background workers and return their handles.
///
/// Workers run as Tokio tasks and stop when `cfg.shutdown_token` is cancelled.
/// Await [`WorkerHandles::join_all`] after cancelling, *before* closing the
/// event store.
///
/// # Errors
///
/// Returns an error if the outbound AS4 session cannot be built (invalid PEM)
/// or the AS4 event bus cannot be created. Everything else this function needs
/// was already parsed and proven by `core::preflight`.
pub(crate) async fn spawn_workers(cfg: WorkersConfig) -> anyhow::Result<WorkerHandles> {
    use crate::as4_sender::BdewAs4Sender;
    use crate::malo_ident_sender::MaloIdentSender;
    use crate::verzeichnisdienst_worker;
    use crate::worker_health::new_heartbeat;
    use secrecy::ExposeSecret as _;

    let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // ── MaLo-ID partner directory (parsed by the preflight) ──────────────
    let maloid_partners = cfg.checked.maloid_partners.clone();
    if !maloid_partners.is_empty() {
        let glns: Vec<&str> = maloid_partners.keys().map(String::as_str).collect();
        info!(partners = ?glns, "MaLo-ID partner directory loaded");
    }

    // ── Optional Verzeichnisdienst lookup ────────────────────────────────
    let verzeichnisdienst_lookup: Option<verzeichnisdienst_worker::VerzeichnisdienstLookup> =
        if let Some(base_url) = cfg.checked.verzeichnisdienst_url.clone() {
            let vz_client = energy_api::directory::DirectoryServiceClient::new(
                base_url.clone(),
                cfg.http_client.clone(),
            );
            let vz_partner_store = cfg.store.as_partner_store();
            let vz_tenant_id =
                mako_engine::ids::TenantId::from_party_id(cfg.mp_id_registry.primary_mp_id());
            info!(url = %base_url, "Verzeichnisdienst integration enabled");
            let lookup = verzeichnisdienst_worker::VerzeichnisdienstLookup::new(
                vz_client,
                vz_partner_store,
                vz_tenant_id,
            );
            let refresh_lookup = lookup.clone();
            handles.push(tokio::spawn(
                verzeichnisdienst_worker::verzeichnisdienst_refresh_task(
                    refresh_lookup,
                    Duration::from_secs(300),
                    cfg.shutdown_token.clone(),
                ),
            ));
            Some(lookup)
        } else {
            None
        };

    let malo_sender = MaloIdentSender::new(
        (*cfg.malo_cache).clone(),
        cfg.http_client.clone(),
        maloid_partners,
        verzeichnisdienst_lookup,
        cfg.store.clone(),
    );

    // ── AS4 partner P-Mode registry (built by the preflight) ─────────────
    let as4_profile = cfg.checked.as4_profile;
    if !as4_profile.registry().is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        for pm in as4_profile.all_pmodes() {
            seen.insert(pm.partner_id.as_str());
        }
        let glns: Vec<&str> = seen.into_iter().collect();
        info!(partners = ?glns, "AS4 partner P-Mode registry loaded");
    }

    // ── §20b Netzzugangsplattform sender (shared by both outbox senders) ──
    let netzzugang_sender = Arc::new(crate::netzzugang::NetzzugangSender::new(
        cfg.http_client.clone(),
        cfg.netzzugang_endpoint_url.clone(),
        cfg.erp_webhook_url.clone(),
        cfg.erp_webhook_secret.clone(),
        cfg.marktd_client.clone(),
    ));

    // ── Outbox delivery worker ────────────────────────────────────────────
    //
    // BdewAs4Sender when signing credentials are present; MaloIdentSender
    // (MaLo-ID callbacks only) otherwise.
    if let (Some(key_pem), Some(cert_pem)) = (
        cfg.as4_signing_key_pem.as_ref().map(|s| s.expose_secret()),
        cfg.as4_signing_cert_pem.as_deref(),
    ) {
        let party_id = cfg.checked.as4_party_id.clone();

        // NOTE: One outbound SessionContext / signing key is used for ALL mp_ids.
        // For a combined Strom+Gas deployment the Gas mp_id's outbound AS4 messages
        // are signed with the Strom (primary) cert.  Counterparties validate the cert
        // against the BDEW/SM-PKI CA trust anchor, not against <eb:From>, so this is
        // accepted in practice.  Full per-mp_id cert isolation would require separate
        // SessionContexts keyed by sender mp_id — tracked as a future enhancement.
        // See site/content/docs/reference/as4-bdew.md §"Signing cert and <eb:From>" for guidance.

        let outbound_session = {
            let session_id = format!("makod-outbound-{}", uuid::Uuid::new_v4());
            let trust_anchor = cfg
                .as4_trust_anchor_pem
                .clone()
                .unwrap_or_else(|| cert_pem.to_owned());
            asx_rs::core::SessionContextBuilder::new(&session_id, &party_id)
                .with_signing_material(cert_pem, key_pem)
                .with_trust_anchor_pem(trust_anchor)
                .build()
                .map_err(|e| anyhow::anyhow!("AS4 outbound session build failed: {e}"))?
        };
        let outbound_bus = Arc::new(
            asx_rs::observability::EventBus::new(256)
                .map_err(|e| anyhow::anyhow!("AS4 EventBus (outbound) init failed: {e}"))?,
        );

        let sender = BdewAs4Sender::new(
            Arc::new(outbound_session),
            outbound_bus,
            Arc::new(as4_profile),
            malo_sender,
            Arc::clone(&cfg.mp_id_registry),
            Some(Arc::new(crate::edifact_api::EdifactApiState {
                platform: Arc::clone(&cfg.platform),
                pid_router: cfg.ctx.pid_router().clone(),
                mp_id_registry: Arc::clone(&cfg.mp_id_registry),
                cedar: Arc::new(
                    crate::cedar_authz::CedarAuthorizer::unauthenticated()
                        .expect("CedarAuthorizer::unauthenticated is infallible"),
                ),
                max_body_bytes: 256 * 1024 * 1024, // 256 MiB — generous but finite (F-009)
                partner_store: None,
                tenant_id: mako_engine::ids::TenantId::from_party_id(
                    cfg.mp_id_registry.primary_mp_id(),
                ),
                // The durable sink, not a log-only one: a self-addressed
                // message that fails to route is as much a § 147 AO record as
                // one that arrived over the wire, and a combined-role
                // deployment delivers a large share of its traffic this way.
                dl_sink: std::sync::Arc::new(cfg.dead_letter_sink.clone()),
                dispatcher: Some(Arc::clone(&cfg.ingest_dispatcher)),
                // AS4 loopback (self-delivery) does not need a CONTRL ack:
                // we are both sender and receiver in this code path.
                contrl_ack: None,
            })),
            Arc::clone(&cfg.platform),
            cfg.as4_lenient_receipts,
        )?
        .with_netzzugang(Arc::clone(&netzzugang_sender))
        // `<eb:From>/<eb:PartyId>` must match the signing certificate's subject
        // (AS4-Profil §2.3.2), and this is the identity that cert was issued to.
        .with_party_id(party_id.as_str());

        info!(
            party_id        = %party_id,
            primary_mp_id     = %cfg.mp_id_registry.primary_mp_id(),
            own_mp_ids        = ?cfg.mp_id_registry.own_mp_ids().collect::<Vec<_>>(),
            "AS4 outbound sender active (BdewAs4Sender)",
        );
        let (outbox_hb, outbox_watch) = new_heartbeat("outbox-worker", 120);
        let worker = cfg
            .ctx
            .run_outbox_worker(
                sender,
                50,
                Duration::from_secs(5),
                OUTBOX_MAX_ATTEMPTS,
                OUTBOX_RETRY_WINDOW,
            )
            .with_heartbeat(outbox_hb.last_tick_raw())
            .with_shutdown(cfg.shutdown_token.clone());
        cfg.health_state.register_worker(outbox_watch);
        handles.push(tokio::spawn(async move { worker.run().await }));
    } else if let Some(ref edifact_webhook_url) = cfg.edifact_outbox_webhook_url {
        use crate::as4_sender::WebhookEdifactSender;
        let sender = WebhookEdifactSender::new(
            edifact_webhook_url.as_str(),
            Arc::clone(&cfg.mp_id_registry),
            cfg.http_client.clone(),
            malo_sender,
        )
        .with_netzzugang(Arc::clone(&netzzugang_sender));
        info!(
            url = %edifact_webhook_url,
            "EDIFACT outbox webhook sender active (WebhookEdifactSender) — \
             outbound EDIFACT will be POSTed as CloudEvents",
        );
        let (outbox_hb, outbox_watch) = new_heartbeat("outbox-worker", 120);
        let worker = cfg
            .ctx
            .run_outbox_worker(
                sender,
                50,
                Duration::from_secs(5),
                OUTBOX_MAX_ATTEMPTS,
                OUTBOX_RETRY_WINDOW,
            )
            .with_heartbeat(outbox_hb.last_tick_raw())
            .with_shutdown(cfg.shutdown_token.clone());
        cfg.health_state.register_worker(outbox_watch);
        handles.push(tokio::spawn(async move { worker.run().await }));
    } else {
        // The preflight already refused this combination unless
        // `--allow-no-as4-signing` was passed, so reaching here is deliberate.
        tracing::warn!(
            "AS4 signing credentials not configured \
             (--as4-signing-key-pem / --as4-signing-cert-pem not set). \
             Outbox delivery is running in MaloIdentCallback-only mode — \
             all EDIFACT messages will be logged and rescheduled without transmission. \
             Pass --allow-no-as4-signing to silence this warning.",
        );
        let (outbox_hb, outbox_watch) = new_heartbeat("outbox-worker", 120);
        let worker = cfg
            .ctx
            .run_outbox_worker(
                malo_sender,
                50,
                Duration::from_secs(5),
                OUTBOX_MAX_ATTEMPTS,
                OUTBOX_RETRY_WINDOW,
            )
            .with_heartbeat(outbox_hb.last_tick_raw())
            .with_shutdown(cfg.shutdown_token.clone());
        cfg.health_state.register_worker(outbox_watch);
        handles.push(tokio::spawn(async move { worker.run().await }));
    }

    info!("outbox delivery worker started");

    // ── ERP webhook outbound worker ───────────────────────────────────────
    //
    // Health-tracked like every other worker, and permanently failed
    // deliveries land in the durable dead-letter store — an ERP notification
    // that exhausted its retries must stay queryable, not vanish into a log.
    let erp_dl_sink: std::sync::Arc<dyn mako_engine::dead_letter::DeadLetterSink> =
        std::sync::Arc::new(cfg.dead_letter_sink.clone());
    if let Some(erp_url) = cfg.erp_webhook_url.clone() {
        let adapter = erp_adapter::WebhookErpAdapter::new(erp_url.clone(), cfg.erp_webhook_secret);
        let (erp_hb, erp_watch) = new_heartbeat("erp-webhook-worker", 120);
        let worker = erp_adapter::OutboxErpWorker::new(
            cfg.store.clone(),
            adapter,
            50,
            Duration::from_secs(5),
        )
        .with_dead_letter_sink(erp_dl_sink)
        .with_heartbeat(erp_hb.last_tick_raw())
        .with_shutdown(cfg.shutdown_token.clone());
        cfg.health_state.register_worker(erp_watch);
        info!(erp_webhook_url = %erp_url, "ERP webhook outbound worker started");
        handles.push(tokio::spawn(async move { worker.run().await }));
    } else {
        // Health-tracked like the webhook variant. The adapter only logs, but
        // the worker still drains the outbox — a stall leaves ERP-targeted
        // entries accumulating, and this was the one worker whose death `GET
        // /health/ready` could not see.
        let adapter = mako_engine::erp::LogErpAdapter;
        let (erp_hb, erp_watch) = new_heartbeat("erp-log-worker", 120);
        let worker = erp_adapter::OutboxErpWorker::new(
            cfg.store.clone(),
            adapter,
            50,
            Duration::from_secs(30),
        )
        .with_dead_letter_sink(erp_dl_sink)
        .with_heartbeat(erp_hb.last_tick_raw())
        .with_shutdown(cfg.shutdown_token.clone());
        cfg.health_state.register_worker(erp_watch);
        tracing::debug!(
            "ERP outbound notifications are logged only \
             (--erp-webhook-url not set; set to enable HTTP delivery)",
        );
        handles.push(tokio::spawn(async move { worker.run().await }));
    }

    // ── Deadline scheduler ────────────────────────────────────────────────
    let event_store_for_scheduler = Arc::clone(cfg.ctx.event_store());
    let (deadline_hb, deadline_watch) = new_heartbeat(
        "deadline-scheduler",
        (cfg.deadline_poll_interval_secs.max(1) * 3) as i64,
    );
    let scheduler = deadline_dispatch::build_scheduler(
        &cfg.ctx,
        event_store_for_scheduler,
        cfg.snapshot_interval,
        Duration::from_secs(cfg.deadline_poll_interval_secs.max(1)),
    )
    .with_heartbeat(deadline_hb.last_tick_raw())
    .with_shutdown(cfg.shutdown_token.clone());
    cfg.health_state.register_worker(deadline_watch);
    handles.push(tokio::spawn(async move { scheduler.run().await }));
    info!(
        poll_interval_secs = cfg.deadline_poll_interval_secs.max(1),
        "deadline scheduler started",
    );

    // ── Projection checkpoint workers ─────────────────────────────────────
    if cfg.projection_checkpoint_interval > 0 {
        let interval = Duration::from_secs(cfg.projection_checkpoint_interval);

        let (proj1_hb, proj1_watch) = new_heartbeat(
            "projection-worker:gpke-konfiguration",
            (cfg.projection_checkpoint_interval * 5).max(300) as i64,
        );
        let worker = crate::projection_worker::ProjectionWorker::new(
            cfg.store.clone(),
            mako_gpke::KonfigurationProjection::default(),
            Some("gpke/"),
            interval,
        )
        .with_heartbeat(proj1_hb.last_tick_raw())
        .with_shutdown(cfg.shutdown_token.clone());
        cfg.health_state.register_worker(proj1_watch);
        handles.push(tokio::spawn(async move { worker.run().await }));

        let (proj2_hb, proj2_watch) = new_heartbeat(
            "projection-worker:gpke-supplier-change",
            (cfg.projection_checkpoint_interval * 5).max(300) as i64,
        );
        let worker = crate::projection_worker::ProjectionWorker::new(
            cfg.store.clone(),
            mako_gpke::SupplierChangeProjection::default(),
            Some("gpke/"),
            interval,
        )
        .with_heartbeat(proj2_hb.last_tick_raw())
        .with_shutdown(cfg.shutdown_token.clone());
        cfg.health_state.register_worker(proj2_watch);
        handles.push(tokio::spawn(async move { worker.run().await }));

        info!(
            interval_secs = cfg.projection_checkpoint_interval,
            "projection checkpoint workers started",
        );
    } else {
        tracing::warn!(
            "--projection-checkpoint-interval=0: projection checkpoints disabled; \
             every restart will trigger a full event-store replay",
        );
    }

    // ── Retention purge worker ────────────────────────────────────────────
    //
    // Sweeps the two time-bounded key spaces: the AS4 inbox dedup index and the
    // `Idempotency-Key` records behind `POST /api/v1/commands`.
    //
    // The BDEW AS4 retry window is 72 hours; a dedup entry evicted at exactly
    // 72h could let a final retry at the window's edge re-process as a new
    // message. Retention adds a 24-hour safety margin on top of the window.
    const INBOX_DEDUP_RETENTION: time::Duration = time::Duration::hours(72 + 24);
    let inbox_store_for_purge = cfg.inbox_store_for_purge;
    let idempotency_store = cfg.store.clone();

    // Heartbeat like every other worker. A stalled purge is not a compliance
    // failure — dedup keeps working, entries simply accumulate — but it leaks
    // storage indefinitely, and this was the only worker whose death was
    // invisible to `GET /health`.
    //
    // The window is generous: the loop ticks daily, so anything under ~2 cycles
    // would flap on a slow purge over a large store.
    let (purge_hb, purge_watch) = new_heartbeat("retention-purge-worker", 26 * 3600);
    cfg.health_state.register_worker(purge_watch);

    let purge_token = cfg.shutdown_token.clone();
    handles.push(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 3600));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                () = purge_token.cancelled() => {
                    tracing::info!("retention purge worker: shutdown signalled; stopping");
                    return;
                }
            }
            // Tick before the work: the heartbeat proves the loop is alive, and
            // a purge that fails still logs its own error below.
            purge_hb.tick();
            let now = time::OffsetDateTime::now_utc();
            match inbox_store_for_purge
                .purge_expired(now - INBOX_DEDUP_RETENTION)
                .await
            {
                Ok(n) => tracing::info!(removed = n, "inbox purge complete"),
                Err(e) => tracing::error!(error = %e, "inbox purge failed"),
            }
            match crate::commands_api::idempotency::purge_expired(&idempotency_store, now).await {
                Ok(n) => tracing::info!(removed = n, "idempotency record purge complete"),
                Err(e) => tracing::error!(error = %e, "idempotency record purge failed"),
            }
        }
    }));
    info!(
        "retention purge worker started (daily; 96h AS4 inbox dedup for the 72h \
         retry window, 24h Idempotency-Key records)"
    );

    Ok(WorkerHandles(handles))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that every domain workflow has an adapter registered for all
    /// active BDEW format versions.
    ///
    /// This test is the primary guard against accidentally shipping a build
    /// where some workflows silently dead-letter cross-FV messages.  Any
    /// breakage here means `adapters.rs` needs a new adapter entry.
    #[test]
    fn all_workflows_have_adapter_coverage() {
        // validate_adapter_coverage panics on missing coverage — the panic
        // itself is the assertion.
        validate_adapter_coverage();
    }

    /// Every name `KNOWN_WORKFLOW_NAMES` certifies must actually have a
    /// dispatch arm.
    ///
    /// [`validate_dispatch_completeness`] compares the live `PidRouter` against
    /// that constant and reports "dispatch completeness validated" when they
    /// agree. That statement is only true if the constant itself is true — and
    /// it is hand-maintained, so it can name a workflow whose arm was never
    /// written. Such a message reaches `dispatch_inner`, falls through the
    /// family match to `unknown_workflow_skip`, and is dropped while the
    /// startup log claims full coverage. Four DVGW workflows were in exactly
    /// that state.
    ///
    /// The arms are read out of the per-family submodule sources rather than
    /// listed again here: a second hand-maintained list is the defect this
    /// test exists to prevent.
    #[test]
    fn every_certified_workflow_has_a_dispatch_arm() {
        use ingest_dispatcher::EdifactIngestDispatcher;

        const SOURCES: &[(&str, &str)] = &[
            (
                "emob.rs",
                include_str!("../orchestrator/ingest_dispatcher/emob.rs"),
            ),
            (
                "gabi_gas.rs",
                include_str!("../orchestrator/ingest_dispatcher/gabi_gas.rs"),
            ),
            (
                "geli_gas.rs",
                include_str!("../orchestrator/ingest_dispatcher/geli_gas.rs"),
            ),
            (
                "gpke.rs",
                include_str!("../orchestrator/ingest_dispatcher/gpke.rs"),
            ),
            (
                "mabis.rs",
                include_str!("../orchestrator/ingest_dispatcher/mabis.rs"),
            ),
            (
                "redispatch.rs",
                include_str!("../orchestrator/ingest_dispatcher/redispatch.rs"),
            ),
            (
                "wim.rs",
                include_str!("../orchestrator/ingest_dispatcher/wim.rs"),
            ),
        ];

        // A dispatch arm is either a string literal (`"gpke-sperrung" => {`) or
        // a path to a crate constant (`… mako_wim::x::WORKFLOW_NAME => {`). The
        // second form is resolved through this table, keyed by the path as it
        // appears in the source and valued by the constant itself — so a
        // renamed constant changes the value and the assertion below catches it.
        const CONSTANT_ARMS: &[(&str, &str)] = &[
            (
                "EmobAnmeldungWorkflow::WORKFLOW_NAME",
                mako_emob::EmobAnmeldungWorkflow::WORKFLOW_NAME,
            ),
            (
                "EmobZuordnungsendeWorkflow::WORKFLOW_NAME",
                mako_emob::EmobZuordnungsendeWorkflow::WORKFLOW_NAME,
            ),
            (
                "EmobAbmeldungWorkflow::WORKFLOW_NAME",
                mako_emob::EmobAbmeldungWorkflow::WORKFLOW_NAME,
            ),
            (
                "mako_wim::esa_wertebestellung::WORKFLOW_NAME",
                mako_wim::esa_wertebestellung::WORKFLOW_NAME,
            ),
            (
                "mako_wim::ersteinbau::WORKFLOW_NAME",
                mako_wim::ersteinbau::WORKFLOW_NAME,
            ),
            (
                "mako_wim::weiterverpflichtung::WORKFLOW_NAME",
                mako_wim::weiterverpflichtung::WORKFLOW_NAME,
            ),
            (
                "mako_wim::wertebestellung::WORKFLOW_NAME",
                mako_wim::wertebestellung::WORKFLOW_NAME,
            ),
        ];

        let mut arms: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (_, src) in SOURCES {
            for line in src.lines() {
                let trimmed = line.trim();
                if !trimmed.contains("=>") {
                    continue;
                }
                if let Some(rest) = trimmed.strip_prefix('"')
                    && let Some((name, tail)) = rest.split_once('"')
                    && tail.trim_start().starts_with("=>")
                {
                    arms.insert(name);
                }
                for (path, value) in CONSTANT_ARMS {
                    if trimmed.contains(path) {
                        arms.insert(value);
                    }
                }
            }
        }

        let missing: Vec<&str> = EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES
            .iter()
            .copied()
            .filter(|n| !arms.contains(n))
            .collect();
        assert!(
            missing.is_empty(),
            "these workflows are in KNOWN_WORKFLOW_NAMES — so \
             validate_dispatch_completeness certifies them — but no dispatch arm \
             exists in any ingest_dispatcher submodule. Inbound messages routed \
             to them are dropped:\n  {}",
            missing.join("\n  ")
        );

        let stale: Vec<&str> = arms
            .iter()
            .copied()
            .filter(|n| !EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES.contains(n))
            .collect();
        assert!(
            stale.is_empty(),
            "these dispatch arms exist but are absent from KNOWN_WORKFLOW_NAMES, \
             so validate_dispatch_completeness would panic at startup if their \
             PIDs were registered:\n  {stale:?}"
        );
    }

    /// Verify that `KNOWN_WORKFLOW_NAMES` is sorted and deduplicated.
    ///
    /// The list is maintained by hand; this test catches accidental duplicates
    /// or missorting that would make the coverage diff harder to read.
    #[test]
    fn known_workflow_names_sorted_and_unique() {
        use ingest_dispatcher::EdifactIngestDispatcher;
        let names = EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES;

        // Check sorted order.
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            names,
            sorted.as_slice(),
            "KNOWN_WORKFLOW_NAMES must be sorted alphabetically; \
             expected {sorted:?}",
        );

        // Check no duplicates.
        sorted.dedup();
        assert_eq!(
            names.len(),
            sorted.len(),
            "KNOWN_WORKFLOW_NAMES contains duplicates: {:?}",
            {
                let mut seen = std::collections::HashSet::new();
                names
                    .iter()
                    .filter(|&&n| !seen.insert(n))
                    .copied()
                    .collect::<Vec<_>>()
            }
        );
    }
}
