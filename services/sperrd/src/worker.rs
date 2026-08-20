//! The IFTSTA 21039 retry worker.
//!
//! A terminal order whose IFTSTA has not been dispatched is an order whose
//! Lieferant does not know what happened — most often because the process
//! crashed between claiming the order and dispatching, or because `makod` was
//! unreachable.
//!
//! Retries use the same idempotency key `makod` deduplicates on, so a re-send
//! after a lost response is the same command rather than a second IFTSTA on the
//! wire. Orders past the budget are announced once as
//! `de.sperr.iftsta.ausstehend` and left for a human.

use std::sync::Arc;
use std::time::Duration;

use mako_markt::makod_client::MakodClient;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::{events, pg};

/// How often the queue is swept when it was empty last time.
const IDLE_INTERVAL: Duration = Duration::from_secs(60);
/// Pause between two dispatches while the queue still has work.
const BUSY_INTERVAL: Duration = Duration::from_millis(250);

/// Run until shutdown.
pub async fn run(
    pool: PgPool,
    makod: Arc<MakodClient>,
    tenant: String,
    shutdown: CancellationToken,
) {
    tracing::info!(
        max_attempts = pg::IFTSTA_MAX_ATTEMPTS,
        "sperrd: IFTSTA 21039 retry worker started"
    );
    loop {
        let delay = match sweep(&pool, &makod, &tenant).await {
            Ok(true) => BUSY_INTERVAL,
            Ok(false) => IDLE_INTERVAL,
            Err(e) => {
                tracing::error!(error = %e, "sperrd: IFTSTA retry sweep failed");
                IDLE_INTERVAL
            }
        };
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("sperrd: IFTSTA retry worker stopping");
                return;
            }
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// One pass. Returns whether it did any work.
///
/// # Errors
///
/// Propagates database errors.
pub async fn sweep(pool: &PgPool, makod: &Arc<MakodClient>, tenant: &str) -> anyhow::Result<bool> {
    // Escalate first: an order that has run out of attempts must be announced
    // before the sweep spends its cycle on the ones that are still trying.
    let mut did_work = escalate_stuck(pool, tenant).await?;

    if let Some(order) = pg::claim_iftsta_retry(pool, tenant).await? {
        let id = order.id;
        if pg::dispatch_iftsta(pool, makod, tenant, &order).await {
            tracing::info!(order_id = %id, "sperrd: IFTSTA 21039 re-sent successfully");
        }
        did_work = true;
    }
    Ok(did_work)
}

/// Announce every order that exhausted the retry budget, once.
async fn escalate_stuck(pool: &PgPool, tenant: &str) -> anyhow::Result<bool> {
    let stuck = pg::list_stuck_iftsta(pool, tenant).await?;
    if stuck.is_empty() {
        return Ok(false);
    }
    for (id, malo_id, lf_mp_id, last_error) in stuck {
        let mut tx = pool.begin().await?;
        events::iftsta_ausstehend(&mut tx, tenant, id, &malo_id, &lf_mp_id, &last_error).await?;
        pg::mark_iftsta_escalated(&mut *tx, id, tenant).await?;
        tx.commit().await?;
        tracing::error!(
            order_id = %id, %malo_id, %last_error,
            "sperrd: IFTSTA 21039 could not be dispatched after {} attempts — the \
             Lieferant has not been told the outcome and their process cannot close",
            pg::IFTSTA_MAX_ATTEMPTS,
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_busy_interval_does_not_starve_the_idle_one() {
        // A queue with work is drained promptly; an empty one is not polled at
        // that rate, which would be a per-second query for nothing.
        assert!(BUSY_INTERVAL < IDLE_INTERVAL);
        assert!(IDLE_INTERVAL <= Duration::from_secs(300));
    }
}
