//! What has to happen when an ESA consent stops being a lawful basis — whether
//! the customer withdrew it or its own validity window ran out.
//!
//! # Two ways to lose the basis, one obligation
//!
//! §49 Abs. 2 Nr. 9 MsbG makes an Art.-7-GDPR Einwilligung the ESA's *entire*
//! entitlement to a location's Messwerte. It can end two ways:
//!
//! - **Widerruf** (GDPR Art. 7(3)) — the customer withdraws it. `DELETE
//!   /api/v1/esa/einwilligungen/:id`.
//! - **Ablauf** — the window the consent was granted for closes. `E_0256`
//!   Prüfschritt 8 treats the two identically („widerrufen **oder ihre
//!   Gültigkeit ist abgelaufen**" → `A08`).
//!
//! The obligation is identical and so is the mechanism: the only protocol-level
//! way to stop a running delivery is the **ORDERS 17008 Abbestellung**, and it
//! is the ESA that has to send it. Nothing in the market stops on its own — the
//! MSB keeps delivering until it is told to stop.
//!
//! Nothing else closes the gap: `gate_outbound` refuses *new* orders, the
//! registry's list endpoint stops showing the row, and the MSB has no reason to
//! act. Hence [`spawn_expiry_sweep`], and hence one [`end_consent`] every
//! ending calls — Widerruf, Ablauf, and a new grant that replaces the consent.
//!
//! # The end, its event and its Abbestellungen are one transaction
//!
//! `revoked_at` is also every path's own filter: a consent that carries it can
//! never be selected again. Closing the row on its own therefore spends the
//! only retry that exists — if the `einwilligung.widerrufen` event or the
//! Abbestellung is then lost, nothing sees the consent again, no 17008 is ever
//! sent, and the ESA goes on receiving Typ-2 Messwerte with no lawful basis
//! (Art. 6 Abs. 1 lit. a DSGVO). So the row, its event and one
//! `mako_service::outbox` command per covered location commit together, and
//! the command worker delivers the 17008 to makod with retry and a
//! dead-letter budget. A makod outage delays the stop; it never loses it.

use std::sync::Arc;
use std::time::Duration;

use mako_markt::{makod_client::ForwardCommand, repository::EinwilligungRecord};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

/// How often to look for consents whose validity window has closed.
///
/// A consent expires on a **date**, so the finest resolution that means
/// anything is a day; hourly keeps the stop within an hour of midnight without
/// polling for no reason. The delivery cadence it is stopping is daily at the
/// earliest (Codeliste Kap. 4.6: „unverzüglich, jedoch spätestens bis 9:30
/// Uhr"), so an hourly sweep cannot let a whole delivery through.
const SWEEP_INTERVAL: Duration = Duration::from_secs(3_600);

/// The upstream name the Abbestellung commands are owed to; the worker that
/// drains them is built with the same name.
pub const MAKOD_UPSTREAM: &str = "makod";

/// Record the end of a consent **on the caller's transaction**: its
/// `einwilligung.widerrufen` event, and one ORDERS 17008 Abbestellung command
/// per location in `abbestellen`.
///
/// Every ending calls this — the Widerruf, the expiry sweep, and a grant that
/// replaces the consent — so the paths cannot diverge on what „the consent
/// ended" owes the market. `abbestellen` is every covered location for a
/// Widerruf or an Ablauf, and only the ones the replacing consent no longer
/// covers for a supersession: abbestellen a location still covered would stop
/// a lawful flow.
///
/// **No `messprodukt`, deliberately.** An ESA subscription is the
/// (Meldepunkt, Messprodukt) pair and one location may carry several — the
/// Codeliste offers `9991 00000 305 6` and `9991 00000 314 7` for the same
/// Marktlokation, among others. Losing the basis loses it for **all** of them,
/// and `marktd` has no idea how many are running: makod resolves an omitted
/// `messprodukt` to *every* live subscription at the location and stops each in
/// the shape its own Abo mode admits. Naming one here would stop one and leave
/// the rest delivering.
///
/// A redelivered Abbestellung sends no second 17008: makod resolves the command
/// against the ESA subscriptions still running at the location, so once the
/// first one went out either none is left to address (`404`) or the process
/// has moved past `Beliefert` and `EsaWertebestellungWorkflow` refuses
/// `SendAbbestellung` (`409`). Both are the command's settled statuses.
///
/// # Errors
///
/// Returns `sqlx::Error` when an INSERT fails; the caller rolls back and the
/// consent stays open.
pub async fn end_consent(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    rec: &EinwilligungRecord,
    grund: &str,
    abbestellen: &[String],
) -> Result<(), sqlx::Error> {
    // One CloudEvent for every way the basis ends: a consumer's obligation does
    // not change with *why*, and the `grund` is what tells them apart.
    let evt = mako_markt::cloudevents::MarktEvent::new(
        tenant,
        mako_events::markt::EINWILLIGUNG_WIDERRUFEN,
        rec.id.to_string(),
        serde_json::json!({
            "einwilligung_id": rec.id,
            "esa_mp_id": rec.esa_mp_id,
            "anschlussnutzer_ref": rec.anschlussnutzer_ref,
            "location_ids": rec.location_ids,
            "grund": grund,
            "valid_to": rec.valid_to.map(|d| d.to_string()),
        }),
    );
    crate::outbox::enqueue(&mut *conn, &evt).await?;
    for location_id in abbestellen {
        let cmd = ForwardCommand {
            command: mako_markt::commands::ESA_ABBESTELLUNG_BEAUFTRAGEN.to_owned(),
            marktrolle: Some("ESA".to_owned()),
            malo_id: Some(location_id.clone()),
            melo_id: None,
            payload: serde_json::json!({
                "malo_id": location_id,
                "esa_mp_id": rec.esa_mp_id,
                "grund": grund,
                "einwilligung_id": rec.id,
            }),
        };
        let body = serde_json::to_value(&cmd).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
        mako_service::outbox::enqueue_command(
            &mut *conn,
            &mako_service::outbox::Command {
                upstream: MAKOD_UPSTREAM.to_owned(),
                label: mako_markt::commands::ESA_ABBESTELLUNG_BEAUFTRAGEN.to_owned(),
                method: mako_service::outbox::CommandMethod::Post,
                path: "/api/v1/commands".to_owned(),
                body,
                // One obligation per (consent, location): a Widerruf racing the
                // sweep enqueues it once.
                key: format!("esa-abbestellung:{}:{location_id}", rec.id),
                settled_statuses: vec![404, 409],
            },
        )
        .await?;
    }
    Ok(())
}

/// The `grund` of a Widerruf (GDPR Art. 7(3)).
pub const GRUND_WIDERRUFEN: &str = "einwilligung_widerrufen";

/// The `grund` an expiry-driven Abbestellung states.
///
/// Distinct from `einwilligung_widerrufen`: both end the basis, but only one is
/// the customer exercising Art. 7(3), and an audit that cannot tell them apart
/// cannot answer „did anyone withdraw consent this quarter".
pub const GRUND_ABGELAUFEN: &str = "einwilligung_abgelaufen";

/// The `grund` of a consent ended because a new grant for the same ESA and
/// Anschlussnutzer replaced it.
pub const GRUND_ERSETZT: &str = "einwilligung_ersetzt";

/// Close expired consents hourly and owe makod the Abbestellungen.
pub fn spawn_expiry_sweep(
    repo: Arc<crate::pg::PgEinwilligungRepository>,
    pool: PgPool,
    tenant: String,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    sweep(repo.as_ref(), &pool, &tenant).await;
                }
            }
        }
    });
}

async fn sweep(repo: &crate::pg::PgEinwilligungRepository, pool: &PgPool, tenant: &str) {
    let today = mako_fristen::heute();
    let candidates = match repo.expired_candidates(today, tenant).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(error = %e, "marktd: ESA consent expiry sweep failed");
            return;
        }
    };
    if candidates.is_empty() {
        return;
    }
    tracing::info!(
        count = candidates.len(),
        "marktd: ESA consents expired — stopping the deliveries they authorised"
    );

    for id in candidates {
        match close_expired(pool, tenant, today, id).await {
            // Closed here, with its event and its Abbestellungen.
            Ok(Some(_)) => {}
            // Already closed, or held by another transaction — a customer's
            // Widerruf, a second replica's sweep. That transaction owes the
            // event, and a row it only holds is expired again next hour.
            Ok(None) => {}
            Err(e) => {
                tracing::error!(
                    error = %e, einwilligung_id = %id,
                    "marktd: closing the expired ESA consent failed — the consent stays \
                     open and the next sweep retries it"
                );
            }
        }
    }
}

/// Revoke one expired consent and persist its `einwilligung.widerrufen` event
/// and Abbestellungen in a **single transaction**, returning the record when
/// this call closed it.
///
/// # Errors
///
/// Returns an error when the claim, an INSERT or the commit fails; the consent
/// then stays open for the next sweep.
///
/// # One transaction per consent, not per sweep
///
/// `revoked_at` is what makes a consent visible to the sweep, so a revocation
/// that commits without its event is unrecoverable: no later sweep sees the row
/// and the ESA keeps receiving Typ-2 Messwerte with no lawful basis (Art. 6
/// Abs. 1 lit. a DSGVO). Rolling the row back on any failure is what makes the
/// retry real.
///
/// Per consent rather than per batch: the unit that has to be atomic is one
/// consent and its one event, and a batch would make every other lapsed consent
/// hostage to the one row whose INSERT failed — all of them rolled back, all of
/// them retried, and none of their Abbestellungen sent this hour. Per-row also
/// keeps the transaction short, which matters because a claimed row is locked
/// against the Widerruf handler for its duration.
pub async fn close_expired(
    pool: &PgPool,
    tenant: &str,
    today: time::Date,
    id: uuid::Uuid,
) -> anyhow::Result<Option<EinwilligungRecord>> {
    let mut tx = pool.begin().await?;
    let Some(rec) =
        crate::pg::PgEinwilligungRepository::claim_expired(&mut tx, id, today, tenant).await?
    else {
        return Ok(None);
    };

    end_consent(&mut tx, tenant, &rec, GRUND_ABGELAUFEN, &rec.location_ids).await?;
    tx.commit().await?;
    Ok(Some(rec))
}
