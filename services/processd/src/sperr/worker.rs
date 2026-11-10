//! The Fristen worker: the IFTSTA 21039 escalation and the execution-window
//! sweep.
//!
//! The IFTSTA itself is an outbox command committed with the order's outcome
//! and delivered by the outbox worker ([`crate::sperr::iftsta_worker`]). What
//! this worker adds is the alarm: an order whose command was dead-lettered, or
//! that is past the Frist undelivered, is announced once as
//! `de.sperr.iftsta.ausstehend`.

use std::time::Duration;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::sperr::{events, pg};

/// How often the sweep runs.
const INTERVAL: Duration = Duration::from_secs(60);

/// Run until shutdown.
pub async fn run(pool: PgPool, tenant: String, shutdown: CancellationToken) {
    tracing::info!("processd sperr: IFTSTA escalation and execution-window sweep started");
    loop {
        if let Err(e) = sweep(&pool, &tenant).await {
            tracing::error!(error = %e, "processd sperr: Fristen sweep failed");
        }
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("processd sperr: Fristen sweep stopping");
                return;
            }
            () = tokio::time::sleep(INTERVAL) => {}
        }
    }
}

/// One pass. Returns whether it announced anything.
///
/// # Errors
///
/// Propagates database errors.
pub async fn sweep(pool: &PgPool, tenant: &str) -> anyhow::Result<bool> {
    let mut did_work = escalate_stuck(pool, tenant).await?;
    did_work |= announce_overdue_executions(pool, tenant).await?;
    Ok(did_work)
}

/// Announce every order whose IFTSTA was dead-lettered or is overdue, once.
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
            "processd sperr: IFTSTA 21039 has not been dispatched (dead-lettered, or past the \
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
            "processd sperr: Sperrauftrag past the 6-Werktage execution window \
             (BK6-24-174 GPKE Teil 2 § 3.5.1.2 Nr. 1)"
        );
    }
    Ok(true)
}
