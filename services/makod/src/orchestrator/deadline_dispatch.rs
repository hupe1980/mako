//! Deadline scheduler dispatch.
//!
//! A fired deadline names its workflow; the workflow's [`WorkflowDriver`] —
//! registered once, by type, in its domain crate's `EngineModule::drivers` —
//! decides it. There is no table here to keep beside the modules.
//!
//! ## One guard
//!
//! Deadlines are registered when a message arrives and are never cancelled when
//! the process moves on, so every one of them fires — on the healthy path as
//! much as on the failing one. What a firing does is decided in exactly one
//! place:
//!
//! 1. A **delivery window** ([`mako_fristen::is_delivery_window`]) is a
//!    regulatory alert about a message that was not delivered in time. The
//!    outbox worker retires it on delivery, so one that fires is a real miss —
//!    and it is never a step of the business process it rode beside, so no
//!    workflow sees it.
//! 2. A label the workflow does not own ([`Workflow::DEADLINE_LABELS`]) is
//!    logged and dropped: nothing decides it. The engine refuses to register
//!    such a deadline in the first place.
//! 3. Every other label goes through [`Process::run_timeout`], which asks the
//!    workflow's own `on_deadline` whether the window the label names is still
//!    open in the current state, and runs the command it returns — atomically
//!    with its outbox entries — or nothing.
//!
//! [`WorkflowDriver`]: mako_engine::driver::WorkflowDriver
//! [`Workflow::DEADLINE_LABELS`]: mako_engine::workflow::Workflow::DEADLINE_LABELS
//! [`Process::run_timeout`]: mako_engine::process::Process::run_timeout

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mako_engine::{
    builder::DeadlineScheduler,
    deadline::{Deadline, DeadlineStore},
    driver::WorkflowDriver,
    error::EngineError,
};

use mako_engine::metrics::EngineMetrics;
use mako_engine::store_slatedb::{SlateDbSnapshotStore, SlateDbStore};

/// What a fired deadline came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fired {
    /// A delivery window: an alert, no workflow consulted.
    DeliveryWindow,
    /// The workflow does not own the label; nothing decides it.
    Unowned,
    /// The workflow's `on_deadline` found the window already closed.
    Closed,
    /// The workflow's `on_deadline` acted and its events were persisted.
    Acted,
}

/// The driver of every workflow this binary registers, by name.
///
/// Built once from [`crate::startup::production_modules`] — the same list the
/// engine is built from, so a role build sees exactly its own workflows.
#[must_use]
pub fn production_drivers() -> &'static HashMap<&'static str, WorkflowDriver> {
    static DRIVERS: OnceLock<HashMap<&'static str, WorkflowDriver>> = OnceLock::new();
    DRIVERS.get_or_init(|| {
        crate::startup::production_modules()
            .iter()
            .flat_map(|m| m.drivers())
            .map(|d| (d.name(), d))
            .collect()
    })
}

/// The labels each registered workflow owns, for every workflow that owns one.
///
/// Read by the `deadline_labels` guard, which holds every label a production
/// site registers against the table of the workflow it registers it for.
#[must_use]
pub fn owned_labels() -> Vec<(&'static str, &'static [&'static str])> {
    let mut out: Vec<(&'static str, &'static [&'static str])> = production_drivers()
        .values()
        .filter(|d| !d.deadline_labels().is_empty())
        .map(|d| (d.name(), d.deadline_labels()))
        .collect();
    out.sort_unstable_by_key(|(name, _)| *name);
    out
}

/// Raise the regulatory alert for a delivery window that fired.
///
/// Reaching it means the window was still registered when it came due:
/// `OutboxWorker::discharge_delivery_window` retires it the moment its message
/// is delivered, so an undischarged window is an undelivered message.
fn alert_delivery_window(deadline: &Deadline) {
    let label = deadline.label();
    let what = if label == mako_fristen::CONTRL_FRIST_LABEL {
        "the CONTRL was not delivered within its window (CONTRL AHB 1.0 §2.3.1)"
    } else if label == mako_fristen::APERAK_STROM_WINDOW_LABEL {
        "the Strom APERAK was not delivered within 45 minutes of receipt, or by \
         Sunday 12:00 on a Saturday (APERAK AHB 1.1 §2.4.1)"
    } else {
        "the Gas Verarbeitbarkeitsfehlermeldung was not delivered within its \
         window (APERAK AHB 1.1 §2.3.1)"
    };
    tracing::error!(
        deadline_id = %deadline.deadline_id(),
        workflow    = %deadline.workflow_id().name,
        label       = %label,
        "REGULATORY ALERT: delivery window expired — {what}. Inspect the outbox \
         for stuck messages and the AS4 transport.",
    );
}

/// Ask the workflow behind `driver` about one fired deadline — the one path a
/// timeout takes.
async fn decide(
    driver: &WorkflowDriver,
    deadline: &Deadline,
    event_store: &Arc<SlateDbStore>,
    snap_store: &SlateDbSnapshotStore,
    snapshot_interval: u64,
) -> Result<Fired, EngineError> {
    if !driver.deadline_labels().contains(&deadline.label()) {
        tracing::error!(
            deadline_id = %deadline.deadline_id(),
            workflow    = %deadline.workflow_id().name,
            label       = %deadline.label(),
            "deadline fired under a label its workflow does not own — nothing decides it. \
             List the label in the workflow's DEADLINE_LABELS and decide it in on_deadline.",
        );
        return Ok(Fired::Unowned);
    }
    let acted = driver
        .run_timeout(event_store, snap_store, snapshot_interval, deadline)
        .await?;
    Ok(if acted.is_some_and(|ran| !ran.is_empty()) {
        Fired::Acted
    } else {
        Fired::Closed
    })
}

/// Dispatch a fired `deadline` to the workflow that decides it, looked up by
/// name in `drivers` — the engine's own, as [`build_scheduler`] passes them,
/// or [`production_drivers`].
///
/// After a successful decision, takes a snapshot if the stream has accumulated
/// a multiple of `snapshot_interval` events.
///
/// Returns what the firing came to on success or non-conflict failure (the
/// scheduler cancels the deadline after this call).
///
/// # Errors
///
/// `EngineError::VersionConflict` if the process was concurrently modified on
/// every attempt — the scheduler retries — and storage errors.
pub async fn dispatch_deadline(
    drivers: &HashMap<&'static str, WorkflowDriver>,
    deadline: Deadline,
    event_store: Arc<SlateDbStore>,
    snap_store: SlateDbSnapshotStore,
    snapshot_interval: u64,
) -> Result<Fired, EngineError> {
    let wf_name = deadline.workflow_id().name.as_ref();

    // The `family` metric label, from the one table that owns it — a local
    // `split('-')` here reads `geli-gas-…` as `geli` and stops joining the
    // command counters.
    let family = crate::orchestrator::process_family::from_workflow(wf_name);

    let result = if mako_fristen::is_delivery_window(deadline.label()) {
        alert_delivery_window(&deadline);
        Ok(Fired::DeliveryWindow)
    } else if let Some(driver) = drivers.get(wf_name) {
        decide(
            driver,
            &deadline,
            &event_store,
            &snap_store,
            snapshot_interval,
        )
        .await
    } else {
        tracing::error!(
            deadline_id  = %deadline.deadline_id(),
            workflow     = %wf_name,
            label        = %deadline.label(),
            "deadline fired for a workflow this binary registers no driver for — \
             nothing decides it",
        );
        Ok(Fired::Unowned)
    };

    if let Ok(Fired::Acted) = result {
        tracing::warn!(
            deadline_id = %deadline.deadline_id(),
            workflow    = %wf_name,
            label       = %deadline.label(),
            "Frist lapsed with its window still open — the workflow recorded the lapse",
        );
    }
    if result.is_ok() {
        EngineMetrics::global().deadline_fired(family);
    }

    result
}

/// Build the deadline scheduler.
pub fn build_scheduler<SS, OS, DS, PR>(
    ctx: &mako_engine::builder::EngineContext<SlateDbStore, SS, OS, DS, PR>,
    event_store: Arc<SlateDbStore>,
    snapshot_interval: u64,
    poll_interval: Duration,
) -> DeadlineScheduler<DS>
where
    DS: DeadlineStore + Clone,
    SS: mako_engine::snapshot::SnapshotStore,
    OS: mako_engine::outbox::OutboxStore,
    PR: mako_engine::registry::ProcessRegistry,
{
    tracing::info!(
        workflows = ctx.drivers().len(),
        workflows_with_frist = owned_labels().len(),
        "deadline scheduler: every registered workflow decides its own deadlines",
    );
    // The engine's own drivers: the workflows this context was built with.
    let drivers: Arc<HashMap<&'static str, WorkflowDriver>> =
        Arc::new(ctx.drivers().iter().map(|d| (d.name(), *d)).collect());
    ctx.run_deadline_scheduler(
        move |deadline| {
            let es = Arc::clone(&event_store);
            let ss = event_store.as_snapshot_store();
            let drivers = Arc::clone(&drivers);
            Box::pin(async move {
                dispatch_deadline(&drivers, deadline, es, ss, snapshot_interval)
                    .await
                    .map(|_| ())
            })
        },
        100,
        poll_interval,
    )
}

#[cfg(test)]
mod driver_tests {
    /// A workflow that publishes an answer window owns its label, so a fired
    /// deadline reaches its `on_deadline` rather than being dropped as unowned.
    #[test]
    fn a_workflow_with_an_answer_window_owns_its_label() {
        let owned = super::owned_labels();
        for name in [
            mako_geli_gas::LF_ANMELDUNG_WORKFLOW_NAME,
            mako_gpke::lf_anmeldung::WORKFLOW_NAME,
            mako_wim::ersteinbau::WORKFLOW_NAME,
            "gpke-beendigung-zuordnung",
            "gpke-kuendigung",
        ] {
            assert!(
                owned
                    .iter()
                    .any(|(n, labels)| *n == name && !labels.is_empty()),
                "{name} registers a published answer window and must own its label",
            );
        }
    }

    /// No workflow lists a delivery window: it is the outbox's, not the
    /// workflow's.
    #[test]
    fn no_workflow_owns_a_delivery_window() {
        for (name, labels) in super::owned_labels() {
            for label in labels {
                assert!(
                    !mako_fristen::is_delivery_window(label),
                    "{name} lists the delivery window {label}; it is the outbox's, not the workflow's"
                );
            }
        }
    }
}
