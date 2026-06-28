//! Generic background worker that periodically runs a [`Projection`] against
//! the event store using [`ProjectionRunner::catch_up_persistent`].
//!
//! The worker loads the last persisted [`GlobalProjectionCheckpoint`] from
//! SlateDB on each tick, feeds only the *new* events to the projection, and
//! writes back the updated cursors.  This bounds cold-start replay to O(events
//! since last checkpoint) instead of O(all events).
//!
//! ## Wiring
//!
//! ```rust,ignore
//! use std::time::Duration;
//! use mako_gpke::KonfigurationProjection;
//! use makod::projection_worker::ProjectionWorker;
//!
//! let worker = ProjectionWorker::new(
//!     store.clone(),
//!     KonfigurationProjection::default(),
//!     None,                       // stream prefix filter (None = all streams)
//!     Duration::from_secs(60),    // checkpoint interval
//! );
//! tokio::spawn(async move { worker.run().await });
//! ```
//!
//! The checkpoint interval is the maximum amount of work lost on an unclean
//! restart.  Shorter intervals reduce replay time but add more I/O load on the
//! checkpoint store.
//!
//! ## Projection name → checkpoint key
//!
//! The worker uses [`Projection::name`] as the `checkpoint_name` parameter to
//! `catch_up_persistent`.  Each distinct projection class therefore gets its
//! own key-space under `cp/<name>/` in SlateDB.  Two workers running the same
//! projection type share a single checkpoint (which is correct — they would
//! duplicate effort otherwise).

use std::time::Duration;

use mako_engine::{
    projection::{Projection, ProjectionRunner},
    store_slatedb::SlateDbStore,
};

// ── ProjectionWorker ──────────────────────────────────────────────────────────

/// Background task that drives a [`Projection`] with durable checkpoint
/// persistence.
///
/// Create with [`ProjectionWorker::new`] and spawn with [`ProjectionWorker::run`].
pub struct ProjectionWorker<P> {
    store: SlateDbStore,
    projection: P,
    prefix: Option<&'static str>,
    poll_interval: Duration,
}

impl<P: Projection + Send> ProjectionWorker<P> {
    /// Construct a new worker.
    ///
    /// - `store` — the SlateDB store (implements both `EventStore` and
    ///   `ProjectionCheckpointStore`)
    /// - `projection` — the projection instance; it must be `Default` for
    ///   a clean initial build
    /// - `prefix` — optional stream-key prefix filter (e.g. `"gpke/"` to
    ///   scan only GPKE streams); `None` scans all streams
    /// - `poll_interval` — how often to run the catch-up loop; also the
    ///   maximum event-loss window on unclean restart
    pub fn new(
        store: SlateDbStore,
        projection: P,
        prefix: Option<&'static str>,
        poll_interval: Duration,
    ) -> Self {
        Self {
            store,
            projection,
            prefix,
            poll_interval,
        }
    }

    /// Run the worker loop forever.
    ///
    /// Ticks at `poll_interval`, calling `catch_up_persistent` on each tick
    /// to feed new events to the projection and persist the updated checkpoint.
    /// Errors are logged but do not terminate the loop; transient storage
    /// failures self-heal on the next tick.
    pub async fn run(mut self) {
        let name = self.projection.name();
        let mut interval = tokio::time::interval(self.poll_interval);
        // Missed ticks are skipped (burst prevention).
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        tracing::info!(
            projection = name,
            prefix = self.prefix.unwrap_or("(all streams)"),
            poll_interval_secs = self.poll_interval.as_secs(),
            "projection worker started",
        );

        loop {
            interval.tick().await;
            match ProjectionRunner::catch_up_persistent(
                &mut self.projection,
                &self.store,
                self.prefix,
                name,
            )
            .await
            {
                Ok(_checkpoint) => {
                    tracing::debug!(projection = name, "projection checkpoint persisted",);
                }
                Err(e) => {
                    tracing::error!(
                        projection = name,
                        error = %e,
                        "projection catch-up failed; will retry on next tick",
                    );
                }
            }
        }
    }
}
