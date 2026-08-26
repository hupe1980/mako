//! The tick that makes a deadline mean something.
//!
//! Registering an obligation is not what enforces it. A run that suspends
//! waiting for a human, and a case carrying a Frist that is about to pass, both
//! sit in the store doing nothing until somebody looks — so `on_expiry: deny`
//! in a manifest is a promise no code keeps unless a sweeper runs.
//!
//! One call does all of it: [`Runtime::sweep`] warns on approaching deadlines,
//! breaches the ones that passed, expires and escalates overdue tasks, wakes
//! runs whose timer arrived, and dead-letters events nobody correlated. It is
//! idempotent, so two instances ticking at once — or a tick that runs twice
//! after a restart — is safe by construction rather than by our scheduling.
//!
//! ## What is worth logging, and what is worth alerting on
//!
//! A quiet tick logs nothing: a plane holding five hundred open cases with
//! nothing due is working, and a line per tick trains operators to ignore the
//! log. What is reported is what `SweepReport::needs_attention` flags — a
//! breached window, an expired approval, an event that correlated to nothing, a
//! recovery or a wake that failed, a census that could not be read, or a
//! **saturated** sweep, which is the one that reads like a normal result and is
//! not: the batch came back full, so more was waiting than was handled.
//!
//! The rule: **a line names every field the predicate above it reads**, or a
//! tick tripped by a stuck run reports "work a human should see" and then prints
//! zeros. `runs_recovered` and `events_redelivered` are on the quiet line
//! instead — neither is a fault, and an instance dying is still worth seeing.

use std::sync::Arc;
use std::time::Duration;

use agentplane::runtime::Runtime;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// How long an uncorrelated inbound event is kept before it is dead-lettered.
///
/// A message can legitimately arrive before the run that waits for it — an
/// APERAK that overtakes our own dispatch record — so events are buffered
/// rather than dropped. An hour is comfortably longer than any such race in
/// mako and short enough that a wrong correlation key is noticed the same day.
const EVENT_GRACE: Duration = Duration::from_secs(3600);

/// Run the sweeper until shutdown.
///
/// `every` is the tick interval. It bounds how late a warning can be, not how
/// late a breach is recorded: the breach instant is the deadline's, resolved
/// and journaled when the obligation was registered.
pub fn spawn(runtime: Arc<Runtime>, every: Duration, shutdown: CancellationToken) {
    // agentplane 0.11: one `Duration` on the public surface — `sweep` takes
    // `std::time::Duration` (unsigned), so the conversion shim is gone.
    let grace = EVENT_GRACE;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        // A missed tick is not a reason to run two immediately: the work is
        // idempotent, so catching up buys nothing and a burst after a pause is
        // exactly when the store is least likely to want one.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    info!("agent sweeper stopping");
                    return;
                }
                _ = ticker.tick() => {}
            }

            let now = time::OffsetDateTime::now_utc();
            match runtime.sweep(now, grace).await {
                Ok(report) if report.is_quiet() => {}
                Ok(report) => {
                    if report.evidence_lost {
                        // The state changed and the account of who changed it
                        // did not. Loudest thing this worker can say.
                        error!(
                            breached = report.breached,
                            tasks_expired = report.tasks_expired,
                            "agent sweep decided something and could not write its own record"
                        );
                    }
                    if report.needs_attention() {
                        // **Every field `needs_attention` reads is named here.**
                        // A warning that does not name its own reason prints
                        // "work a human should see" beside a row of zeros, and
                        // one people cannot act on is one they learn to close.
                        warn!(
                            warned = report.warned,
                            breached = report.breached,
                            tasks_expired = report.tasks_expired,
                            tasks_escalated = report.tasks_escalated,
                            dead_lettered = report.dead_lettered,
                            // A run nothing else will unstick. Retried next tick,
                            // so a steady count is one stuck run and not many.
                            recovery_failures = report.recovery_failures,
                            // A timer whose wake was recorded and whose resume
                            // died: the run is arriving late, not lost.
                            wake_failures = report.wake_failures,
                            // Gauges that could not be read. A blind spot wearing
                            // a default — the two below are missing, not zero.
                            census_unavailable = report.census_unavailable,
                            saturated = report.saturated.any(),
                            open_cases = report.census.open_cases,
                            open_tasks = report.census.open_tasks,
                            "agent sweep found work a human should see"
                        );
                    } else {
                        info!(
                            timers_fired = report.timers_fired,
                            warned = report.warned,
                            // An instance died holding these and the plane
                            // healed it. The healing is routine, which is why it
                            // is not on `needs_attention`; the dying is not, and
                            // a steady rate beside healthy instances is a
                            // contradiction somebody should be able to see.
                            runs_recovered = report.runs_recovered,
                            // A message whose delivery died between the claim and
                            // the resume, redelivered. Persistent counts mean
                            // deliveries keep dying, which is worth asking why.
                            events_redelivered = report.events_redelivered,
                            open_cases = report.census.open_cases,
                            open_tasks = report.census.open_tasks,
                            "agent sweep"
                        );
                    }
                }
                Err(e) => error!(error = %e, "agent sweep failed"),
            }
        }
    });
}

/// Retire admission keys older than `retention`, on a slow tick.
///
/// The index `run_correlated_once` claims into only grows, and nothing retires a
/// row by default — deliberately, upstream and here: **retiring a key reopens
/// the door it closed**, so the window is the operator's to choose and absent a
/// choice keys are kept. This loop exists so that a deployment which *has*
/// chosen one does not have to reach for a CLI on a cron.
///
/// It ticks once a day rather than on the sweep interval. The unit of the window
/// is days, so a minute-by-minute pass would do the same work 1 440 times to
/// retire the same rows, and there is nothing time-critical about forgetting: a
/// key retired an hour late is a key that worked for an hour longer.
pub fn spawn_admission_retention(
    journal: Arc<dyn agentplane::journal::JournalStore>,
    retention: Duration,
    shutdown: CancellationToken,
) {
    const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(EVERY);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // `interval` fires immediately; the first tick is deliberately taken so
        // a restart-loop deployment still retires, rather than never reaching
        // the 24-hour mark.
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    info!("agent admission-retention worker stopping");
                    return;
                }
                _ = ticker.tick() => {}
            }

            let older_than = time::OffsetDateTime::now_utc() - retention;
            match journal.forget_admissions(older_than).await {
                Ok(0) => {}
                Ok(retired) => info!(
                    retired,
                    days = retention.as_secs() / (24 * 60 * 60),
                    "agent admission keys retired"
                ),
                Err(e) => error!(error = %e, "agent admission retention failed"),
            }
        }
    });
}

/// Run the outbox delivery worker until shutdown.
///
/// Registering a destination is not what delivers to it. `Outbox` puts a run in
/// front of a receiver at admission; this is the loop that reads the run's own
/// journal records past a cursor, POSTs them, and advances the cursor **only on
/// 2xx**.
///
/// That ordering is the whole guarantee. A crash after the POST and before the
/// cursor moves re-delivers rather than loses — at-least-once, which is the
/// honest contract for a webhook and the one every other mako service gets from
/// its transactional outbox. The alternative this replaces was a decision POSTed
/// at request time and dropped on failure: the one outbound path in the system
/// with no persist-before-dispatch behind it.
///
/// A receiver that is down for a deploy is caught up afterwards. One that has
/// gone away, or that refuses permanently, is **parked** past the retry ceiling
/// and reported — its rows and cursor survive, listed by `PushStore::parked`, so
/// a registration nobody removes is a queue an operator can see rather than one
/// that only grows.
pub fn spawn_delivery(
    worker: Arc<agentplane::push::DeliveryWorker>,
    every: Duration,
    shutdown: CancellationToken,
) {
    /// Registrations handled per tick. Bounded so one saturated sweep cannot
    /// hold the store for an unbounded time; a full batch is reported, and the
    /// next tick continues.
    const BATCH: usize = 256;

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    info!("agent outbox worker stopping");
                    return;
                }
                _ = ticker.tick() => {}
            }

            #[allow(clippy::cast_sign_loss)]
            let now = time::OffsetDateTime::now_utc().unix_timestamp().max(0) as u64;
            match worker.run_once(now, BATCH).await {
                // A quiet outbox says nothing, like every other sweep here.
                Ok(report) if report.deliveries == 0 && !report.needs_attention() => {}
                Ok(report) if report.needs_attention() => {
                    // `parked` and `completed` are opposite outcomes wearing one
                    // shape: both take a registration out of the due order, and
                    // only one of them delivered anything. The rows survive with
                    // their cursors — `PushStore::parked` is the list — so this
                    // number is how an operator learns there is one to read.
                    warn!(
                        deliveries = report.deliveries,
                        parked = report.parked,
                        retries = report.retries,
                        saturated = report.saturated,
                        unserved = report.unserved,
                        "agent outbox needs attention — parked registrations are listed by \
                         PushStore::parked; a saturated tick means at least the cap was waiting"
                    );
                }
                Ok(report) => {
                    info!(
                        deliveries = report.deliveries,
                        completed = report.completed,
                        // Due rows in the other id namespace. Not part of
                        // `needs_attention` — with both workers running the
                        // other's rows are legitimately due between its sweeps
                        // — but invisible without saying it.
                        unserved = report.unserved,
                        "agent outbox delivered"
                    );
                }
                Err(e) => error!(error = %e, "agent outbox sweep failed"),
            }
        }
    });
}
