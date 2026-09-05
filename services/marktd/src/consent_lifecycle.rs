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
//! Only the Widerruf was wired. An expiring consent went on receiving
//! quarter-hourly values with no lawful basis, and the gap was invisible from
//! every direction: `gate_outbound` refuses *new* orders, the registry's list
//! endpoint stops showing the row, and the MSB has no reason to act. Hence
//! [`spawn_expiry_sweep`], and hence one [`stop_deliveries`] both paths call —
//! two copies of „emit the event, then fire an Abbestellung per covered
//! location" is how the two would drift.

use std::sync::Arc;
use std::time::Duration;

use mako_markt::{
    makod_client::{ForwardCommand, MakodClient},
    repository::{EinwilligungRecord, EinwilligungRepository},
};
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

/// Stop every ESA delivery a consent authorised, and announce that it ended.
///
/// Called from the Widerruf handler and from [`spawn_expiry_sweep`], so the two
/// paths cannot diverge on what „the consent ended" means.
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
/// Best-effort by design: the revocation itself has already been committed and
/// the CloudEvent already emitted, so a makod outage delays the stop but never
/// blocks the customer's Art.-7(3) right. The event is the durable signal a
/// consumer retries from.
pub async fn stop_deliveries(makod: &MakodClient, grund: &str, rec: &EinwilligungRecord) {
    for location_id in &rec.location_ids {
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
        // Keyed on (consent, location) so a redelivery of the same expiry — or
        // a Widerruf racing the sweep — is one Abbestellung, not two.
        let idem = format!("esa-abbestellung:{}:{location_id}", rec.id);
        if let Err(e) = makod.post_command(&idem, &cmd).await {
            tracing::warn!(
                error = %e, location_id, grund,
                einwilligung_id = %rec.id,
                "marktd: Abbestellung dispatch to makod failed — the consent is closed \
                 and the CloudEvent emitted; retry via that event"
            );
        }
    }
}

/// The `grund` an expiry-driven Abbestellung states.
///
/// Distinct from `einwilligung_widerrufen`: both end the basis, but only one is
/// the customer exercising Art. 7(3), and an audit that cannot tell them apart
/// cannot answer „did anyone withdraw consent this quarter".
pub const GRUND_ABGELAUFEN: &str = "einwilligung_abgelaufen";

/// Close expired consents hourly and stop the deliveries they authorised.
pub fn spawn_expiry_sweep(
    repo: Arc<crate::pg::PgEinwilligungRepository>,
    makod: Arc<MakodClient>,
    pool: PgPool,
    notify: Arc<tokio::sync::Notify>,
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
                    sweep(repo.as_ref(), &makod, &pool, &notify, &tenant).await;
                }
            }
        }
    });
}

async fn sweep(
    repo: &crate::pg::PgEinwilligungRepository,
    makod: &MakodClient,
    pool: &PgPool,
    notify: &tokio::sync::Notify,
    tenant: &str,
) {
    let today = mako_fristen::heute();
    let expired = match repo.revoke_expired(today).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "marktd: ESA consent expiry sweep failed");
            return;
        }
    };
    if expired.is_empty() {
        return;
    }
    tracing::info!(
        count = expired.len(),
        "marktd: ESA consents expired — stopping the deliveries they authorised"
    );

    for rec in &expired {
        // The same CloudEvent the Widerruf emits: a consumer's obligation does
        // not change with *why* the basis ended, and the `grund` in the payload
        // is what tells the two apart.
        let evt = mako_markt::cloudevents::MarktEvent::new(
            tenant,
            mako_events::markt::EINWILLIGUNG_WIDERRUFEN,
            rec.id.to_string(),
            serde_json::json!({
                "einwilligung_id": rec.id,
                "esa_mp_id": rec.esa_mp_id,
                "anschlussnutzer_ref": rec.anschlussnutzer_ref,
                "location_ids": rec.location_ids,
                "grund": GRUND_ABGELAUFEN,
                "valid_to": rec.valid_to.map(|d| d.to_string()),
            }),
        );
        if let Err(e) = crate::outbox::enqueue(pool, &evt, notify).await {
            // Do not send the Abbestellung without the durable record: the
            // event is what a consumer retries from, and stopping a delivery
            // nothing recorded leaves an unexplained gap in the Typ-2 stream.
            tracing::error!(
                error = %e, einwilligung_id = %rec.id,
                "marktd: expiry event enqueue failed — Abbestellung deferred to the next sweep"
            );
            continue;
        }
        stop_deliveries(makod, GRUND_ABGELAUFEN, rec).await;
    }
}
