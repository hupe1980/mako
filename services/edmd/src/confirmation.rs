//! § 60 Abs. 1 MsbG — confirmation loop for estimated/substituted readings.
//!
//! Every stored ESTIMATED/SUBSTITUTED interval opens an entry in
//! `estimated_read_confirmations`: the Messstellenbetreiber owes a
//! plausibilised real value for that slot. Ingest and the correction path
//! discharge entries automatically when a MEASURED/CORRECTED value arrives.
//! This worker is the escalation half: entries older than the configured
//! deadline flip to `UEBERFAELLIG` and are reported as a
//! `de.messwert.reading.confirmation.overdue` CloudEvent.
//!
//! ## Deadline
//!
//! No statute fixes a replacement deadline — § 60 Abs. 1 MsbG establishes
//! the duty (the berechtigten Stellen are owed the aufbereiteten Messwerte, and
//! an Ersatzwert is a stand-in for one that was never measured), not a date. The default of **8 weeks** aligns with the MaBiS
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
    erp_webhook_secret: Option<String>,
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
                "edmd: confirmation-worker: estimated readings past the replacement deadline (§ 60 Abs. 1 MsbG)"
            );

            let Some(ref webhook_url) = erp_webhook_url else {
                continue;
            };
            // One aggregate event per sweep — the endpoint lists the details.
            // Tenant-wide aggregate: no per-object business subject.
            let event = mako_service::CloudEvent::new(
                mako_service::source("edmd", &tenant),
                mako_events::messwert::READING_CONFIRMATION_OVERDUE,
                String::new(),
                serde_json::json!({
                    "tenant": tenant,
                    "newly_overdue": newly_overdue,
                    "deadline_weeks": deadline_weeks,
                    "rechtsgrundlage": "§ 60 Abs. 1 MsbG (Aufbereitung und Übermittlung an die berechtigten Stellen)",
                    "hinweis": "GET /api/v1/confirmations?status=UEBERFAELLIG listet die offenen Intervalle",
                }),
            )
            .without_subject()
            .extension("tenantid", tenant.clone());
            let client = mako_service::http::default_client();
            if let Err(e) = mako_service::post_ce_with_retry(
                &client,
                webhook_url,
                &event,
                erp_webhook_secret.as_deref().map(str::as_bytes),
            )
            .await
            {
                tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
            }
        }
    });
}
