//! Worker liveness heartbeat for the `GET /health` endpoint.
//!
//! Each background worker holds a [`WorkerHeartbeat`] and calls
//! [`WorkerHeartbeat::tick`] at the end of every poll cycle.  The
//! `HealthState` (see `health` module) holds the corresponding [`WorkerWatch`]
//! values and reports `503 degraded` when any watch is stale.
//!
//! ## Design
//!
//! - A heartbeat is a shared `Arc<AtomicI64>` storing the last tick as a Unix
//!   timestamp (seconds). Workers can update it without holding any lock.
//! - A watch is the read side: it checks whether the timestamp is within the
//!   allowed staleness window.
//! - Workers are spawned inside `startup::spawn_workers` and register their
//!   watches into `HealthState` via
//!   [`HealthState::register_worker`](crate::health::HealthState::register_worker).
//!
//! ## Staleness threshold
//!
//! | Worker | Default max-stale |
//! |---|---|
//! | Outbox delivery | 60 s (2× the 30 s poll interval cap) |
//! | Deadline scheduler | 120 s (2× the default 60 s poll interval) |
//! | Projection worker | 300 s (5× the default 60 s checkpoint interval) |
//!
//! These thresholds are conservative: a single slow tick (e.g. a large batch)
//! should not flip the health check to degraded.

use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

// ── WorkerHeartbeat ───────────────────────────────────────────────────────────

/// The write side of a worker liveness heartbeat.
///
/// Call [`tick`] at the end of every poll cycle to signal that the worker is
/// alive.  Dropping `WorkerHeartbeat` marks the worker as permanently
/// stopped — the timestamp is never updated again, so the corresponding
/// [`WorkerWatch`] will report stale after `max_stale_secs`.
///
/// [`tick`]: WorkerHeartbeat::tick
#[derive(Clone)]
pub struct WorkerHeartbeat {
    last_tick: Arc<AtomicI64>,
}

impl WorkerHeartbeat {
    /// Update the heartbeat to the current UTC time.
    ///
    /// Called manually at the end of a poll loop iteration when the worker
    /// owns the tick (rather than delegating to `with_heartbeat` on the engine
    /// worker types).
    #[allow(dead_code)]
    pub fn tick(&self) {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        self.last_tick.store(now, Ordering::Relaxed);
    }

    /// Return a clone of the underlying `Arc<AtomicI64>` for passing to
    /// `OutboxWorker::with_heartbeat` / `DeadlineScheduler::with_heartbeat`.
    ///
    /// The worker and the `WorkerWatch` share the same `Arc` so the tick from
    /// inside the worker is visible to the health endpoint without any locking.
    #[must_use]
    pub fn last_tick_raw(&self) -> Arc<AtomicI64> {
        Arc::clone(&self.last_tick)
    }
}

// ── WorkerWatch ───────────────────────────────────────────────────────────────

/// The read side of a worker liveness heartbeat.
///
/// Created by [`new_heartbeat`] alongside the corresponding
/// [`WorkerHeartbeat`].  Placed in [`HealthState`] to be polled by
/// `GET /health`.
///
/// [`HealthState`]: crate::health::HealthState
pub struct WorkerWatch {
    /// Human-readable worker name for the health response body.
    pub name: &'static str,
    /// Shared atomic that the worker updates via [`WorkerHeartbeat::tick`].
    last_tick: Arc<AtomicI64>,
    /// Maximum number of seconds the worker may be silent before being
    /// considered stale.
    max_stale_secs: i64,
}

impl WorkerWatch {
    /// Returns `true` if the worker has not ticked within `max_stale_secs`.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let last = self.last_tick.load(Ordering::Relaxed);
        // A last_tick of 0 means the worker has never started yet (pre-first tick).
        // We give it up to `max_stale_secs` before treating it as stale.
        now - last > self.max_stale_secs
    }
}

// ── Factory ───────────────────────────────────────────────────────────────────

/// Create a linked [`WorkerHeartbeat`] / [`WorkerWatch`] pair.
///
/// The initial timestamp is set to `now()` so a worker that has not yet
/// ticked its first cycle is given `max_stale_secs` before being considered
/// stale.
///
/// # Arguments
///
/// - `name` — human-readable name shown in health responses (e.g. `"outbox-worker"`)
/// - `max_stale_secs` — how many seconds the worker may be silent
pub fn new_heartbeat(name: &'static str, max_stale_secs: i64) -> (WorkerHeartbeat, WorkerWatch) {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let last_tick = Arc::new(AtomicI64::new(now));
    let beat = WorkerHeartbeat {
        last_tick: Arc::clone(&last_tick),
    };
    let watch = WorkerWatch {
        name,
        last_tick,
        max_stale_secs,
    };
    (beat, watch)
}
