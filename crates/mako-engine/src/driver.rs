//! [`WorkflowDriver`] — a workflow registered once, by type.
//!
//! An [`EngineModule`] names each workflow it owns together with its Rust type:
//!
//! ```rust,ignore
//! fn drivers(&self) -> Vec<WorkflowDriver> {
//!     vec![
//!         WorkflowDriver::of::<GpkeSupplierChangeWorkflow>(wechselprozesse::WORKFLOW_NAME),
//!         WorkflowDriver::of::<GpkeSperrungWorkflow>(sperrung::WORKFLOW_NAME),
//!     ]
//! }
//! ```
//!
//! That one line is everything a host needs to treat the workflow by name
//! without matching on the name itself: the labels it owns
//! ([`Workflow::DEADLINE_LABELS`]), and — with the `slatedb` feature — the
//! monomorphised entry points that run a fired deadline through
//! [`Process::run_timeout`] and repoint its snapshots across a format-version
//! cutover. A host dispatches a fired deadline by looking the driver up under
//! the deadline's workflow name; there is no table to keep beside the modules.
//!
//! [`EngineModule`]: crate::builder::EngineModule
//! [`Workflow::DEADLINE_LABELS`]: crate::workflow::Workflow::DEADLINE_LABELS
//! [`Process::run_timeout`]: crate::process::Process::run_timeout

use crate::workflow::Workflow;

#[cfg(feature = "slatedb")]
use std::{future::Future, pin::Pin, sync::Arc};

#[cfg(feature = "slatedb")]
use crate::{
    deadline::Deadline,
    error::EngineError,
    ids::ProcessIdentity,
    migration::{IdentityMigration, MigrationReport, MigrationRunner},
    process::{Process, RunOptions, RunOutcome},
    store_slatedb::{SlateDbSnapshotStore, SlateDbStore},
    version::WorkflowId,
};

/// A boxed, `Send` future borrowing for `'a`.
#[cfg(feature = "slatedb")]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Runs a fired deadline through [`Process::run_timeout`] for one workflow type.
///
/// [`Process::run_timeout`]: crate::process::Process::run_timeout
#[cfg(feature = "slatedb")]
type TimeoutFn = for<'a> fn(
    &'a Arc<SlateDbStore>,
    &'a SlateDbSnapshotStore,
    u64,
    &'a Deadline,
) -> BoxFuture<'a, Result<Option<RunOutcome>, EngineError>>;

/// Repoints every snapshot of one workflow type from one format version to
/// another.
#[cfg(feature = "slatedb")]
type MigrateFn =
    for<'a> fn(&'a SlateDbStore, &'static str, &'a str, &'a str) -> BoxFuture<'a, MigrationReport>;

/// One workflow, registered by type under its name.
///
/// Built with [`WorkflowDriver::of`]; see the module docs.
#[derive(Clone, Copy)]
pub struct WorkflowDriver {
    name: &'static str,
    deadline_labels: &'static [&'static str],
    #[cfg(feature = "slatedb")]
    timeout: TimeoutFn,
    #[cfg(feature = "slatedb")]
    migrate: MigrateFn,
}

impl std::fmt::Debug for WorkflowDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowDriver")
            .field("name", &self.name)
            .field("deadline_labels", &self.deadline_labels)
            .finish_non_exhaustive()
    }
}

impl WorkflowDriver {
    /// Register `W` under `name` — the name its streams are written with.
    #[must_use]
    pub fn of<W>(name: &'static str) -> Self
    where
        W: Workflow,
        W::State: serde::Serialize + serde::de::DeserializeOwned,
    {
        Self {
            name,
            deadline_labels: W::DEADLINE_LABELS,
            #[cfg(feature = "slatedb")]
            timeout: timeout::<W>,
            #[cfg(feature = "slatedb")]
            migrate: migrate::<W>,
        }
    }

    /// A name the module routes Prüfidentifikatoren to whose messages resume
    /// another workflow's process — no stream is ever written under it, so it
    /// owns no deadline and has nothing to migrate.
    ///
    /// `gabi-gas-mmma` is the case: its MSCONS 13013 is the answer to an
    /// Anforderung the `gpke-allokationsliste` process sent, and resumes that
    /// process.
    #[must_use]
    pub fn routing_only(name: &'static str) -> Self {
        Self {
            name,
            deadline_labels: &[],
            #[cfg(feature = "slatedb")]
            timeout: no_timeout,
            #[cfg(feature = "slatedb")]
            migrate: no_migration,
        }
    }

    /// The workflow name its streams are written with.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The business-deadline labels the workflow owns and decides — its
    /// [`Workflow::DEADLINE_LABELS`]. Empty for a workflow that registers no
    /// Frist.
    #[must_use]
    pub fn deadline_labels(&self) -> &'static [&'static str] {
        self.deadline_labels
    }

    /// Hand a fired `deadline` to the workflow's own `on_deadline` through
    /// [`Process::run_timeout`], snapshotting every `snapshot_interval` events.
    ///
    /// # Errors
    ///
    /// As [`Process::run_timeout`].
    ///
    /// [`Process::run_timeout`]: crate::process::Process::run_timeout
    #[cfg(feature = "slatedb")]
    #[must_use = "the future does nothing until awaited"]
    pub fn run_timeout<'a>(
        &self,
        store: &'a Arc<SlateDbStore>,
        snapshots: &'a SlateDbSnapshotStore,
        snapshot_interval: u64,
        deadline: &'a Deadline,
    ) -> BoxFuture<'a, Result<Option<RunOutcome>, EngineError>> {
        (self.timeout)(store, snapshots, snapshot_interval, deadline)
    }

    /// Repoint every snapshot of this workflow from format version `from` to
    /// `to`, unchanged ([`IdentityMigration`]), and its registry entries with
    /// them.
    #[cfg(feature = "slatedb")]
    #[must_use = "the future does nothing until awaited"]
    pub fn migrate_identity<'a>(
        &self,
        store: &'a SlateDbStore,
        from: &'a str,
        to: &'a str,
    ) -> BoxFuture<'a, MigrationReport> {
        (self.migrate)(store, self.name, from, to)
    }
}

#[cfg(feature = "slatedb")]
fn timeout<'a, W>(
    store: &'a Arc<SlateDbStore>,
    snapshots: &'a SlateDbSnapshotStore,
    snapshot_interval: u64,
    deadline: &'a Deadline,
) -> BoxFuture<'a, Result<Option<RunOutcome>, EngineError>>
where
    W: Workflow,
    W::State: serde::Serialize,
{
    Box::pin(async move {
        let identity = ProcessIdentity::new(
            deadline.process_id(),
            deadline.tenant_id(),
            deadline.workflow_id().clone(),
        );
        Process::<W, _>::from_identity(Arc::clone(store), identity)
            .run_timeout(
                deadline,
                RunOptions::new().snapshot(snapshots, snapshot_interval),
            )
            .await
    })
}

#[cfg(feature = "slatedb")]
fn migrate<'a, W>(
    store: &'a SlateDbStore,
    name: &'static str,
    from: &'a str,
    to: &'a str,
) -> BoxFuture<'a, MigrationReport>
where
    W: Workflow,
    W::State: serde::Serialize + serde::de::DeserializeOwned,
{
    Box::pin(async move {
        let runner = MigrationRunner::new(
            IdentityMigration::<W>::new(WorkflowId::new(name, from), WorkflowId::new(name, to)),
            store.clone(),
            store.as_snapshot_store(),
        );
        runner
            .run_and_update_registry(&store.as_process_registry())
            .await
    })
}

#[cfg(feature = "slatedb")]
fn no_timeout<'a>(
    _store: &'a Arc<SlateDbStore>,
    _snapshots: &'a SlateDbSnapshotStore,
    _snapshot_interval: u64,
    _deadline: &'a Deadline,
) -> BoxFuture<'a, Result<Option<RunOutcome>, EngineError>> {
    Box::pin(async { Ok(None) })
}

#[cfg(feature = "slatedb")]
fn no_migration<'a>(
    _store: &'a SlateDbStore,
    _name: &'static str,
    _from: &'a str,
    _to: &'a str,
) -> BoxFuture<'a, MigrationReport> {
    Box::pin(async { MigrationReport::default() })
}
