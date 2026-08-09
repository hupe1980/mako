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
//! log. What is reported is what `SweepReport::needs_attention` flags —
//! a breached window, an expired approval, an event that correlated to nothing,
//! or a **saturated** sweep, which is the one that reads like a normal result
//! and is not: the batch came back full, so more was waiting than was handled.

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
    let grace = time::Duration::try_from(EVENT_GRACE).unwrap_or(time::Duration::HOUR);
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
                        warn!(
                            warned = report.warned,
                            breached = report.breached,
                            tasks_expired = report.tasks_expired,
                            tasks_escalated = report.tasks_escalated,
                            dead_lettered = report.dead_lettered,
                            saturated = report.saturated.any(),
                            open_cases = report.census.open_cases,
                            open_tasks = report.census.open_tasks,
                            "agent sweep found work a human should see"
                        );
                    } else {
                        info!(
                            timers_fired = report.timers_fired,
                            warned = report.warned,
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
