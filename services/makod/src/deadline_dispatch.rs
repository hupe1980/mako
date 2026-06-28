//! Deadline scheduler dispatch table.
//!
//! Maps workflow names to their `TimeoutExpired` command dispatch logic.
//! Adding a new workflow family requires adding one entry to the `match` in
//! [`dispatch_deadline`] — there is no other location to update.
//!
//! ## Compensation and atomicity
//!
//! All deadline commands are dispatched via
//! [`Process::execute_and_enqueue_with_retry`], which atomically persists
//! both events *and* any outbox entries (e.g. APERAK Ablehnung) produced by
//! the `TimeoutExpired` handler in a single `WriteBatch`.  This ensures
//! there is no window where the `DeadlineExpired` event is stored but the
//! outbound APERAK message is lost.
//!
//! Alternatively, [`Process::execute_timeout_with_retry`] may be used to
//! delegate the dispatch to `Workflow::on_deadline`, but this path is
//! equivalent when every workflow registers a `TimeoutExpired` command in its
//! `on_deadline` hook.
//!
//! ## Coverage check
//!
//! [`build_scheduler`] panics at startup if any workflow name registered by
//! an [`EngineModule`] via [`workflow_names`] is absent from the dispatch
//! table below.  This converts the silent regulatory miss (deadline fires,
//! drops into `unknown` branch, emits WARN) into an immediate startup failure
//! that blocks deployment.
//!
//! [`workflow_names`]: mako_engine::builder::EngineModule::workflow_names
//! [`EngineModule`]: mako_engine::builder::EngineModule

use std::sync::Arc;
use std::time::Duration;

use mako_engine::{
    builder::DeadlineScheduler,
    deadline::{Deadline, DeadlineStore},
    error::EngineError,
    ids::ProcessIdentity,
    process::Process,
};
use mako_geli_gas::{
    GasSperrungCommand, GasSupplierChangeCommand, GeliGasSperrungWorkflow,
    GeliGasSupplierChangeWorkflow,
};
use mako_gpke::{
    AbrechnungCommand, GpkeAbrechnungWorkflow, GpkeKonfigurationWorkflow, GpkeLfAbmeldungWorkflow,
    GpkeLfAnmeldungWorkflow, GpkeNeuanlageWorkflow, GpkeSperrungWorkflow,
    GpkeSupplierChangeWorkflow, KonfigurationCommand, LfAbmeldungCommand, LfAnmeldungCommand,
    NeuanlageCommand, SperrungCommand, SupplierChangeCommand,
    lf_anmeldung::WORKFLOW_NAME as LF_ANMELDUNG_WORKFLOW,
};
use mako_mabis::{BillingCommand, MabisBillingWorkflow};
use mako_wim::{
    DeviceChangeCommand, GeraeteubernahmeCommand, PreisanfrageCommand, PreislisteCommand,
    StammdatenCommand, SteuerungsauftragCommand, StornierungCommand, WimDeviceChangeWorkflow,
    WimGeraeteubernahmeWorkflow, WimPreisanfrageWorkflow, WimPreislisteWorkflow,
    WimRechnungCommand, WimRechnungWorkflow, WimStammdatenWorkflow, WimSteuerungsauftragWorkflow,
    WimStornierungWorkflow,
};
use mako_wim_gas::{
    WimGasAnmeldungCommand, WimGasAnmeldungWorkflow, WimGasKuendigungCommand,
    WimGasKuendigungWorkflow, WimGasVerpflichtungsanfrageCommand,
    WimGasVerpflichtungsanfrageWorkflow,
};

use mako_engine::metrics::EngineMetrics;
use mako_engine::store_slatedb::{SlateDbSnapshotStore, SlateDbStore};

/// Dispatch a fired `deadline` to the correct workflow's `TimeoutExpired` command.
///
/// After a successful execute, takes a snapshot if the stream has accumulated
/// a multiple of `snapshot_interval` events (auto-snapshot.
///
/// Returns `Ok(())` on success or non-conflict failure (the scheduler will
/// cancel the deadline after this call). Returns `Err(EngineError::VersionConflict)`
/// if the process was concurrently modified; the scheduler retries.
pub async fn dispatch_deadline(
    deadline: Deadline,
    event_store: Arc<SlateDbStore>,
    snap_store: SlateDbSnapshotStore,
    snapshot_interval: u64,
) -> Result<(), EngineError> {
    let wf_name = deadline.workflow_id().name.as_ref();
    let identity = ProcessIdentity::new(
        deadline.process_id(),
        deadline.tenant_id(),
        deadline.workflow_id().clone(),
    );
    let deadline_id = deadline.deadline_id();
    let label: Box<str> = deadline.label().into();

    // Derive the process family label from the workflow name prefix for metrics.
    // "gpke-supplier-change" → "gpke", "wim-device-change" → "wim", etc.
    let family = wf_name.split('-').next().unwrap_or(wf_name);

    let result = match wf_name {
        "gpke-supplier-change" => {
            let p = Process::<GpkeSupplierChangeWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                SupplierChangeCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-sperrung" => {
            let p = Process::<GpkeSperrungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                SperrungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        LF_ANMELDUNG_WORKFLOW => {
            let p = Process::<GpkeLfAnmeldungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                LfAnmeldungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-konfiguration" => {
            let p = Process::<GpkeKonfigurationWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                KonfigurationCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-abrechnung" => {
            let p = Process::<GpkeAbrechnungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                AbrechnungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-device-change" => {
            let p = Process::<WimDeviceChangeWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                DeviceChangeCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-geraeteubernahme" => {
            let p = Process::<WimGeraeteubernahmeWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GeraeteubernahmeCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-stammdaten" => {
            let p = Process::<WimStammdatenWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                StammdatenCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-stornierung" => {
            let p = Process::<WimStornierungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                StornierungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-steuerungsauftrag" => {
            let p = Process::<WimSteuerungsauftragWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                SteuerungsauftragCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "geli-gas-supplier-change" => {
            let p = Process::<GeliGasSupplierChangeWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GasSupplierChangeCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "geli-gas-sperrung" => {
            let p = Process::<GeliGasSperrungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                GasSperrungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "mabis-billing" => {
            let p = Process::<MabisBillingWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                BillingCommand::PruefmitteilungDeadlineExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-neuanlage" => {
            let p = Process::<GpkeNeuanlageWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                NeuanlageCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "gpke-lf-abmeldung" => {
            let p = Process::<GpkeLfAbmeldungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                LfAbmeldungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-preisanfrage" => {
            let p = Process::<WimPreisanfrageWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                PreisanfrageCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-preisliste" => {
            let p = Process::<WimPreislisteWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                PreislisteCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-rechnung" => {
            let p = Process::<WimRechnungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimRechnungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-gas-anmeldung" => {
            let p = Process::<WimGasAnmeldungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimGasAnmeldungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-gas-kuendigung" => {
            let p = Process::<WimGasKuendigungWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimGasKuendigungCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        "wim-gas-verpflichtungsanfrage" => {
            let p = Process::<WimGasVerpflichtungsanfrageWorkflow, _>::from_identity(
                Arc::clone(&event_store),
                identity,
            );
            p.execute_and_enqueue_with_retry(
                WimGasVerpflichtungsanfrageCommand::TimeoutExpired { deadline_id, label },
                3,
            )
            .await?;
            p.take_snapshot(&snap_store, snapshot_interval)
                .await
                .map(|_| ())
        }
        unknown => {
            tracing::error!(
                deadline_id  = %deadline_id,
                workflow     = %unknown,
                label        = %label,
                "deadline scheduler: dispatch table has no entry for this workflow — \
                 deadline dropped; add a match arm to deadline_dispatch::dispatch_deadline",
            );
            Ok(())
        }
    };

    // Increment the per-family deadline-fired counter on successful dispatch.
    if result.is_ok() {
        EngineMetrics::global().deadline_fired(family);
    }

    result
}

/// All workflow names known to the deadline dispatcher.
///
/// Every name returned by an `EngineModule::workflow_names` implementation
/// that is registered with the production engine **must** appear here.
/// [`assert_dispatch_coverage`] enforces this at startup.
pub const DISPATCH_TABLE: &[&str] = &[
    "gpke-supplier-change",
    LF_ANMELDUNG_WORKFLOW,
    "gpke-sperrung",
    "gpke-abrechnung",
    "gpke-konfiguration",
    "gpke-neuanlage",
    "gpke-lf-abmeldung",
    "wim-device-change",
    "wim-geraeteubernahme",
    "wim-stammdaten",
    "wim-stornierung",
    "wim-steuerungsauftrag",
    "wim-preisanfrage",
    "wim-preisliste",
    "wim-rechnung",
    "geli-gas-supplier-change",
    "geli-gas-sperrung",
    "mabis-billing",
    "wim-gas-anmeldung",
    "wim-gas-kuendigung",
    "wim-gas-verpflichtungsanfrage",
];

/// Assert that every workflow in `registered` has a dispatch-table entry.
///
/// # Panics
///
/// Panics with an actionable message when a workflow name declared by an
/// `EngineModule` is absent from [`DISPATCH_TABLE`].  Call this at startup
/// before spawning the scheduler so missing entries are caught immediately.
pub fn assert_dispatch_coverage(registered: &[&str]) {
    for &wf in registered {
        if !DISPATCH_TABLE.contains(&wf) {
            panic!(
                "deadline_dispatch: workflow '{wf}' is registered by an EngineModule but has \
                 no entry in the dispatch table (deadline_dispatch::DISPATCH_TABLE). \
                 Add a match arm to dispatch_deadline() and add the name to DISPATCH_TABLE.",
            );
        }
    }
}

/// Build the deadline scheduler and verify dispatch coverage at startup.
///
/// # Panics
///
/// Panics when a workflow name declared by a registered `EngineModule` via
/// [`workflow_names`] is not covered by [`dispatch_deadline`]. This converts
/// a silent regulatory miss into an immediate startup failure.
///
/// [`workflow_names`]: mako_engine::builder::EngineModule::workflow_names
pub fn build_scheduler<SS, OS, DS, PR>(
    ctx: &mako_engine::builder::EngineContext<SlateDbStore, SS, OS, DS, PR>,
    event_store: Arc<SlateDbStore>,
    snapshot_interval: u64,
) -> DeadlineScheduler<DS>
where
    DS: DeadlineStore + Clone,
    SS: mako_engine::snapshot::SnapshotStore,
    OS: mako_engine::outbox::OutboxStore,
    PR: mako_engine::registry::ProcessRegistry,
{
    assert_dispatch_coverage(ctx.registered_workflows());

    ctx.run_deadline_scheduler(
        move |deadline| {
            let es = Arc::clone(&event_store);
            let ss = event_store.as_snapshot_store();
            Box::pin(async move { dispatch_deadline(deadline, es, ss, snapshot_interval).await })
        },
        100,
        Duration::from_secs(30),
    )
}
