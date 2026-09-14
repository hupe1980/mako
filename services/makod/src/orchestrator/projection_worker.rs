//! Generic background worker that periodically runs a [`Projection`] against
//! the event store using [`ProjectionRunner::catch_up_persistent`].
//!
//! The worker loads the last persisted `GlobalProjectionCheckpoint` from
//! SlateDB on each tick, feeds only the *new* events to the projection, and
//! writes back the updated cursors.  This bounds cold-start replay to O(events
//! since last checkpoint) instead of O(all events).
//!
//! ## Wiring
//!
//! ```rust,ignore
//! use std::time::Duration;
//! use mako_mabis::zp_register::ZpRegister;
//! use makod::projection_worker::ProjectionWorker;
//!
//! let worker = ProjectionWorker::new(
//!     store.clone(),
//!     ZpRegister::default(),
//!     Some("process/"),           // every process stream; see below
//!     Duration::from_secs(60),    // checkpoint interval
//! )
//! .with_observer(|r| tracing::info!(aktiv = r.aktive_anzahl(), "MaBiS-ZP"));
//! tokio::spawn(async move { worker.run().await });
//! ```
//!
//! The checkpoint interval is the maximum amount of work lost on an unclean
//! restart.  Shorter intervals reduce replay time but add more I/O load on the
//! checkpoint store.
//!
//! ## The prefix is `process/`, never a domain name
//!
//! Every stream in this platform is `process/{tenant}/{process}`, so a prefix
//! like `"gpke/"` selects nothing: the projection folds zero events for the
//! life of the deployment while its heartbeat reports it healthy. Two workers
//! shipped that way. Each projection sees every workflow's events and filters
//! on `event_type` itself.
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

/// Callback run against the read model after each successful catch-up.
type Observer<P> = Box<dyn Fn(&P) + Send>;

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
    /// Optional liveness heartbeat — updated after each tick.
    heartbeat: Option<std::sync::Arc<std::sync::atomic::AtomicI64>>,
    /// Graceful-shutdown signal — see [`ProjectionWorker::with_shutdown`].
    shutdown: Option<tokio_util::sync::CancellationToken>,
    /// Read-model observer — see [`ProjectionWorker::with_observer`].
    observer: Option<Observer<P>>,
}

impl<P: Projection + Send> ProjectionWorker<P> {
    /// Construct a new worker.
    ///
    /// - `store` — the SlateDB store (implements both `EventStore` and
    ///   `ProjectionCheckpointStore`)
    /// - `projection` — the projection instance; it must be `Default` for
    ///   a clean initial build
    /// - `prefix` — optional stream-key prefix filter; `None` scans all
    ///   streams. The only prefix that selects anything is `"process/"` or a
    ///   prefix of it: every stream is `process/{tenant}/{process}`, so a
    ///   domain name like `"gpke/"` folds zero events while the heartbeat
    ///   reports the worker healthy. `makod`'s `projection_prefix_guard`
    ///   refuses one. A projection therefore filters on `event_type` itself.
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
            heartbeat: None,
            shutdown: None,
            observer: None,
        }
    }

    /// Attach a liveness heartbeat to this worker.
    ///
    /// Updated with the current UTC Unix timestamp after each tick.
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
    /// Cancelling it makes [`ProjectionWorker::run`] return out of its idle
    /// tick, so the caller can close the store once the worker has stopped
    /// writing checkpoints to it.
    #[must_use]
    pub fn with_shutdown(mut self, shutdown: tokio_util::sync::CancellationToken) -> Self {
        self.shutdown = Some(shutdown);
        self
    }

    /// Observe the read model after every successful catch-up.
    ///
    /// The worker owns its projection, so without this the fold is
    /// write-only: events go in and nothing can look at what they produced.
    /// Two projections shipped in exactly that shape, and a read model nothing
    /// reads is indistinguishable from one that is never fed.
    ///
    /// The callback runs on the worker task between ticks, so it must not
    /// block: read what it needs, log or publish, return.
    #[must_use]
    pub fn with_observer(mut self, observer: impl Fn(&P) + Send + 'static) -> Self {
        self.observer = Some(Box::new(observer));
        self
    }

    /// Run the worker loop until the shutdown token is cancelled.
    ///
    /// Ticks at `poll_interval`, calling `catch_up_persistent` on each tick
    /// to feed new events to the projection and persist the updated checkpoint.
    /// Errors are logged but do not terminate the loop; transient storage
    /// failures self-heal on the next tick.
    ///
    /// A catch-up already in flight runs to completion: an interrupted one would
    /// leave the checkpoint behind the events it already folded, and the next
    /// start would replay them into a projection that had counted them.
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
            match &self.shutdown {
                Some(token) => {
                    tokio::select! {
                        _ = interval.tick() => {}
                        () = token.cancelled() => {
                            tracing::info!(
                                projection = name,
                                "projection worker: shutdown signalled; stopping",
                            );
                            return;
                        }
                    }
                }
                None => {
                    interval.tick().await;
                }
            }
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
                    // Only after a successful catch-up: an observer reading a
                    // fold that failed mid-way would report a read model
                    // missing whatever the failure skipped.
                    if let Some(ref observe) = self.observer {
                        observe(&self.projection);
                    }
                }
                Err(e) => {
                    tracing::error!(
                        projection = name,
                        error = %e,
                        "projection catch-up failed; will retry on next tick",
                    );
                }
            }
            // Tick heartbeat after every cycle so health probes can detect stale workers.
            if let Some(ref hb) = self.heartbeat {
                hb.store(
                    ::time::OffsetDateTime::now_utc().unix_timestamp(),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
        }
    }
}
