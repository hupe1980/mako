//! § 60 Abs. 2 MsbG — confirmation loop for estimated/substituted readings.
//!
//! Every stored ESTIMATED/SUBSTITUTED interval opens an entry in
//! `estimated_read_confirmations`: the Messstellenbetreiber owes a
//! plausibilised real value for that slot. Ingest and the correction path
//! discharge entries automatically when a MEASURED/CORRECTED value arrives.
//! This worker is the escalation half: entries older than the configured
//! deadline flip to `UEBERFAELLIG` and are reported as a
//! `de.edmd.reading.confirmation.overdue` CloudEvent.
//!
//! ## Deadline
//!
//! No statute fixes a replacement deadline — § 60 Abs. 2 MsbG establishes
//! the duty, not a date. The default of **8 weeks** aligns with the MaBiS
//! Bilanzkreisabrechnung correction window (after it, a stale estimate is
//! priced into balancing settlement); operators can tighten or relax it via
//! `[confirmation] deadline_weeks`.

use std::sync::Arc;

use sqlx::PgPool;

/// Flip open confirmations past `deadline_weeks` to UEBERFAELLIG.
///
/// Returns the number of newly overdue entries. Factored out of the worker
/// loop so the sweep is testable against real PostgreSQL without spawning.
pub async fn mark_overdue_confirmations(
    pool: &PgPool,
    tenant: &str,
    deadline_weeks: i64,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        r"UPDATE estimated_read_confirmations
          SET status = 'UEBERFAELLIG'
          WHERE tenant = $1
            AND status = 'OFFEN'
            AND created_at < now() - make_interval(weeks => $2::int)",
    )
    .bind(tenant)
    .bind(deadline_weeks)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Spawn the daily confirmation-deadline worker (no-op when disabled).
///
/// Same shape as the CLS compliance worker: initial delay, daily tick,
/// cancellation-aware, webhook notification best-effort.
pub fn spawn_confirmation_worker(
    pool: Arc<PgPool>,
    tenant: String,
    erp_webhook_url: Option<String>,
    deadline_weeks: i64,
    interval_secs: u64,
    shutdown_token: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(45)).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                () = shutdown_token.cancelled() => {
                    tracing::info!("edmd: confirmation-worker: shutdown requested");
                    break;
                }
            }

            let newly_overdue =
                match mark_overdue_confirmations(&pool, &tenant, deadline_weeks).await {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::warn!(error = %e, "edmd: confirmation-worker: sweep failed");
                        continue;
                    }
                };
            if newly_overdue == 0 {
                continue;
            }
            tracing::warn!(
                tenant = %tenant,
                newly_overdue,
                deadline_weeks,
                "edmd: confirmation-worker: estimated readings past the replacement deadline (§ 60 Abs. 2 MsbG)"
            );

            let Some(ref webhook_url) = erp_webhook_url else {
                continue;
            };
            // One aggregate event per sweep — the endpoint lists the details.
            let payload = serde_json::json!({
                "specversion": "1.0",
                "type": "de.edmd.reading.confirmation.overdue",
                "source": format!("urn:edmd:{tenant}"),
                "id": uuid::Uuid::new_v4().to_string(),
                "time": time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                "datacontenttype": "application/json",
                "data": {
                    "tenant": tenant,
                    "newly_overdue": newly_overdue,
                    "deadline_weeks": deadline_weeks,
                    "rechtsgrundlage": "§ 60 Abs. 2 MsbG (Ersatzwertbildung/Plausibilisierung)",
                    "hinweis": "GET /api/v1/confirmations?status=UEBERFAELLIG listet die offenen Intervalle",
                }
            });
            let client = reqwest::Client::new();
            match client.post(webhook_url).json(&payload).send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::info!(
                        newly_overdue,
                        "edmd: confirmation-worker: overdue notice delivered"
                    );
                }
                Ok(resp) => {
                    tracing::warn!(status = %resp.status(), "edmd: confirmation-worker: webhook failed");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "edmd: confirmation-worker: webhook error");
                }
            }
        }
    });
}
