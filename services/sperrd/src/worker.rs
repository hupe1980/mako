//! The Fristen worker: the IFTSTA 21039 retry queue and the execution-window
//! sweep.
//!
//! A terminal order whose IFTSTA has not been dispatched is an order whose
//! Lieferant does not know what happened — most often because the process
//! crashed between claiming the order and dispatching, or because `makod` was
//! unreachable.
//!
//! Retries carry a stable idempotency key, but `makod` logs that key rather than
//! comparing it — the deduplication that does exist is its per-family business
//! guard. So the thing that keeps one IFTSTA on the wire is the **claim**: only
//! one worker can lease an order at a time, and it holds the lease for the
//! backoff. Orders past the budget are announced once as
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
    did_work |= announce_overdue_executions(pool, tenant).await?;

    // The claim leases the order and spends the attempt, so a second replica
    // sweeping at the same moment finds nothing due and cannot send the same
    // IFTSTA 21039 a second time.
    if let Some(order) = pg::claim_iftsta_retry(pool, tenant).await? {
        let id = order.id;
        if pg::dispatch_iftsta(
            pool,
            makod,
            tenant,
            &order,
            pg::AttemptAccounting::CountedByClaim,
        )
        .await
        {
            tracing::info!(order_id = %id, "sperrd: IFTSTA 21039 re-sent successfully");
        }
        did_work = true;
    }
    Ok(did_work)
}

/// Announce every order that exhausted the retry budget, once.
///
/// The escalation is **claimed before it is announced**: `mark_iftsta_escalated`
/// only matches an order that has not been escalated yet, and it runs first
/// inside the transaction that also writes the event. Announcing first and
/// stamping afterwards let two replicas — which both read the same stuck list —
/// each emit `de.sperr.iftsta.ausstehend` for one order, and only then have one
/// of them lose the stamp.
async fn escalate_stuck(pool: &PgPool, tenant: &str) -> anyhow::Result<bool> {
    let stuck = pg::list_stuck_iftsta(pool, tenant).await?;
    if stuck.is_empty() {
        return Ok(false);
    }
    for order in stuck {
        let mut tx = pool.begin().await?;
        if !pg::mark_iftsta_escalated(&mut *tx, order.id, tenant).await? {
            // Another replica announced this one between the list and here.
            tx.rollback().await?;
            continue;
        }
        events::iftsta_ausstehend(&mut tx, tenant, &order).await?;
        tx.commit().await?;
        tracing::error!(
            order_id = %order.id, malo_id = %order.malo_id,
            last_error = %order.last_error, attempts = order.attempts,
            "sperrd: IFTSTA 21039 has not been dispatched (retry budget spent, or past the \
             1. WT nach Abschluss it was due) — the Lieferant has not been told the outcome \
             and their process cannot close",
        );
    }
    Ok(true)
}

/// Announce every pending order past the § 3.5.1.2 Nr. 1 execution window, once.
///
/// This is a **regulatory** deadline, not a queue-depth signal: GPKE Teil 2
/// gives the NB 6 Werktage after the frühestmöglicher Sperrtermin to carry the
/// disconnection out, and an order sitting past it is a compliance finding a
/// BNetzA audit can see.
async fn announce_overdue_executions(pool: &PgPool, tenant: &str) -> anyhow::Result<bool> {
    let overdue = pg::list_ausfuehrung_ueberfaellig(pool, tenant).await?;
    if overdue.is_empty() {
        return Ok(false);
    }
    for (id, malo_id, lf_mp_id, faellig_am) in overdue {
        let mut tx = pool.begin().await?;
        events::ausfuehrung_ueberfaellig(&mut tx, tenant, id, &malo_id, &lf_mp_id, faellig_am)
            .await?;
        pg::mark_ausfuehrung_escalated(&mut *tx, id, tenant).await?;
        tx.commit().await?;
        tracing::warn!(
            order_id = %id, %malo_id, %faellig_am,
            "sperrd: Sperrauftrag past the 6-Werktage execution window \
             (BK6-24-174 GPKE Teil 2 § 3.5.1.2 Nr. 1)"
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
