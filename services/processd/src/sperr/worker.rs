//! The Fristen worker: the day an order becomes due, the IFTSTA 21039
//! escalation and the execution-window sweep.
//!
//! The IFTSTA itself is an outbox command committed with the order's outcome
//! and delivered by the outbox worker ([`crate::sperr::iftsta_worker`]). What
//! this worker adds is the alarm: an order whose command was dead-lettered, or
//! that is past the Frist undelivered, is announced once as
//! `de.mako.sperr.iftsta.ausstehend`.

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
    let mut did_work = announce_due(pool, tenant).await?;
    did_work |= escalate_stuck(pool, tenant).await?;
    did_work |= announce_overdue_executions(pool, tenant).await?;
    Ok(did_work)
}

/// Announce every pending order whose day has come, once: the requested date
/// has arrived, or the order names none.
///
/// The claim and the announcements are one transaction, so an order is
/// announced by one replica and never stamped without its event.
async fn announce_due(pool: &PgPool, tenant: &str) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let due = pg::claim_due(&mut tx, tenant).await?;
    if due.is_empty() {
        return Ok(false);
    }
    for order in &due {
        mako_service::outbox::enqueue(&mut tx, &events::auftrag_faellig(tenant, order)).await?;
    }
    tx.commit().await?;
    tracing::info!(count = due.len(), "processd sperr: orders became due");
    Ok(true)
}

/// Claim the undelivered IFTSTA and commit their notices together.
async fn escalate_stuck(pool: &PgPool, tenant: &str) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let stuck = pg::claim_stuck_iftsta(&mut tx, tenant).await?;
    for order in &stuck {
        events::iftsta_ausstehend(&mut tx, tenant, order).await?;
    }
    tx.commit().await?;
    for order in &stuck {
        tracing::error!(
            order_id = %order.id, malo_id = %order.malo_id,
            last_error = %order.last_error, attempts = order.attempts,
            "processd sperr: IFTSTA 21039 has not been dispatched (dead-lettered, or past the \
             1. WT nach Abschluss it was due) — the Lieferant has not been told the outcome \
             and their process cannot close",
        );
    }
    Ok(!stuck.is_empty())
}

/// Claim the missed execution windows and commit their notices together.
async fn announce_overdue_executions(pool: &PgPool, tenant: &str) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let overdue = pg::claim_overdue_executions(&mut tx, tenant).await?;
    for (id, order, faellig_am) in &overdue {
        events::ausfuehrung_ueberfaellig(&mut tx, tenant, *id, order, *faellig_am).await?;
    }
    tx.commit().await?;
    for (id, order, faellig_am) in &overdue {
        tracing::warn!(
            order_id = %id, malo_id = %order.malo_id, %faellig_am,
            "processd sperr: Sperrauftrag past the 6-Werktage execution window \
             (BK6-24-174 GPKE Teil 2 Kap. 3.5.1.2 Nr. 1)"
        );
    }
    Ok(!overdue.is_empty())
}
