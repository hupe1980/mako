//! `accountingd` — Massenkontokorrent / Customer Account Ledger.
//!
//! Manages the running customer account ledger for LF retail deployments.
//! `billingd` invoices without `accountingd` are fire-and-forget —
//! no Offene-Posten tracking, no Mahnwesen, no automated SEPA collection.
//!
//! ## CloudEvents consumed (inbound webhook)
//!
//! | CE type | Source | Effect |
//! |---|---|---|
//! | `de.billing.rechnung.erstellt` | `billingd` | Debit entry (Brutto-Betrag) |
//! | `de.invoic.receipt.settled` | `invoicd` | Credit entry (NNE invoice settled) |
//! | `de.eeg.verguetung.berechnet` | `einsd` | Credit entry (EEG settlement) |
//!
//! ## CloudEvents emitted (outbound webhook → ERP)
//!
//! | CE type | Trigger |
//! |---|---|
//! | `de.accounting.payment.due` | Upcoming SEPA direct debit |
//! | `de.accounting.mahnung.issued` | Dunning notice issued |
//! | `de.accounting.sperrauftrag` | Mahnstufe 3 → sperrd trigger |
//! | `de.accounting.bankruecklast` | SEPA return received |
//!
//! Port: `:9380`

mod config;
mod handlers;
mod pg;
mod sepa;

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_service::{health::health_routes, load_config};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cfg: config::AccountingdConfig = load_config("accountingd").context("load config")?;
    let cfg = Arc::new(cfg);

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    let app = Router::new()
        .merge(health_routes(|| async { true }))
        // ── CloudEvent ingest ──────────────────────────────────────────────────
        .route("/webhook", post(handlers::ingest_webhook))
        // ── Account endpoints ──────────────────────────────────────────────────
        .route(
            "/api/v1/accounts/:malo_id",
            get(handlers::get_account).put(handlers::put_account),
        )
        .route(
            "/api/v1/accounts/:malo_id/balance",
            get(handlers::get_balance),
        )
        .route(
            "/api/v1/accounts/:malo_id/ledger",
            get(handlers::get_ledger),
        )
        .route(
            "/api/v1/accounts/:malo_id/kontoauszug",
            get(handlers::get_kontoauszug),
        )
        .route(
            "/api/v1/accounts/:malo_id/abschlag",
            put(handlers::put_abschlag),
        )
        // Vorauszahlung — BO4E typed advance-payment schedule (L12 — §40 Abs. 1 EnWG)
        .route(
            "/api/v1/accounts/:malo_id/vorauszahlung",
            get(handlers::get_vorauszahlung).put(handlers::put_vorauszahlung),
        )
        // ── Payment import ─────────────────────────────────────────────────────
        .route("/api/v1/payments/import", post(handlers::import_payments))
        // ── Offene Posten ──────────────────────────────────────────────────────
        .route("/api/v1/offene-posten", get(handlers::get_offene_posten))
        // ── Dunning ────────────────────────────────────────────────────────────
        .route("/api/v1/dunning", get(handlers::get_dunning))
        .route(
            "/api/v1/dunning/:account_id/escalate",
            post(handlers::escalate_dunning),
        )
        .route(
            "/api/v1/dunning/:id/resolve",
            post(handlers::resolve_dunning),
        )
        // ── SEPA ───────────────────────────────────────────────────────────────
        .route("/api/v1/sepa/mandates", post(handlers::post_mandate))
        .route(
            "/api/v1/sepa/mandates/:mandate_id",
            get(handlers::get_mandate),
        )
        .route("/api/v1/sepa/run", post(handlers::run_sepa))
        .route(
            "/api/v1/jahresabschluss/:malo_id",
            post(handlers::post_jahresabschluss),
        )
        .layer(Extension(Arc::clone(&cfg)))
        .layer(Extension(pool.clone()));

    let port = cfg.port.unwrap_or(9380);
    let addr = format!("0.0.0.0:{port}");
    info!(%addr, "accountingd starting");

    // ── Background Abschlagslauf scheduler ──────────────────────────────────
    // Runs daily at approximately 06:00 and checks which accounts have their
    // billing_day = today. For each: posts an ABSCHLAG ledger entry.
    {
        let pool_bg = pool.clone();
        let tenant_bg = cfg.tenant.clone();
        tokio::spawn(async move {
            // Initial delay to let the service start up cleanly.
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
            loop {
                let now_utc = time::OffsetDateTime::now_utc();
                let today = now_utc.date();
                let day_of_month = today.day() as i16;
                match crate::pg::find_accounts_due(&pool_bg, &tenant_bg, day_of_month).await {
                    Ok(accounts) if !accounts.is_empty() => {
                        tracing::info!(
                            day = day_of_month,
                            count = accounts.len(),
                            "accountingd: Abschlagslauf — posting ABSCHLAG entries"
                        );
                        for acct in &accounts {
                            let ref_id = format!(
                                "ABSCHLAG-{}-{:04}-{:02}",
                                acct.malo_id,
                                today.year(),
                                today.month() as u8
                            );
                            if let Err(e) = crate::pg::write_entry(
                                &pool_bg,
                                acct.account_id,
                                &tenant_bg,
                                "ABSCHLAG",
                                acct.abschlag_ct,
                                Some(&ref_id),
                                Some("de.accounting.abschlag.posted"),
                                None,
                                today,
                                Some(&format!("Monatlicher Abschlag Tag {day_of_month}")),
                            )
                            .await
                            {
                                tracing::warn!(
                                    malo_id = %acct.malo_id,
                                    error = %e,
                                    "accountingd: Abschlag entry failed"
                                );
                            }
                        }
                    }
                    Ok(_) => {
                        tracing::debug!(day = day_of_month, "accountingd: no Abschläge due today");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "accountingd: Abschlagslauf DB error");
                    }
                }
                // Sleep ~24h; use 23h to drift-proof against DST transitions.
                tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
            }
        });
    }

    // ── SEPA N-5 Pre-Notification Scheduler (B7) ────────────────────────────
    // Runs daily and identifies accounts whose billing_day falls 5 days from now.
    // For each account with an active SEPA mandate, generates a pain.008 XML
    // batch and emits a `de.accounting.payment.due` CloudEvent to the ERP webhook.
    //
    // ISO 20022 SEPA Pre-Notification rule:
    //   - RCUR/FRST mandates require ≥ 2 banking days pre-notification to the debtor.
    //   - Standard practice: send at least 5 calendar days before due date
    //     (covers weekends + 1 business-day buffer).
    {
        let pool_sepa = pool.clone();
        let cfg_sepa = Arc::clone(&cfg);
        tokio::spawn(async move {
            // Offset start so N-5 and Abschlagslauf do not run simultaneously.
            tokio::time::sleep(tokio::time::Duration::from_secs(120)).await;
            loop {
                let today = time::OffsetDateTime::now_utc().date();
                // Target day = today + 5; wraps correctly across month end.
                let target_date = today + time::Duration::days(5);
                let target_billing_day = target_date.day() as i16;

                match crate::pg::find_accounts_due_for_sepa(
                    &pool_sepa,
                    &cfg_sepa.tenant,
                    target_billing_day,
                )
                .await
                {
                    Ok(pairs) if !pairs.is_empty() => {
                        tracing::info!(
                            target_billing_day,
                            count = pairs.len(),
                            "accountingd: SEPA N-5 — generating pain.008 pre-notifications"
                        );

                        // Build pain.008 XML batch
                        let entries: Vec<(&crate::pg::SepaMandateRow, i64)> =
                            pairs.iter().map(|(m, a)| (m, a.abschlag_ct)).collect();
                        let creditor_iban = cfg_sepa
                            .creditor_iban
                            .as_deref()
                            .unwrap_or("DE00000000000000000000");
                        let pain_xml = crate::sepa::build_pain_008(creditor_iban, &entries);

                        // Emit `de.accounting.payment.due` CE to ERP webhook
                        if let Some(ref url) = cfg_sepa.erp_webhook_url {
                            let ce = serde_json::json!({
                                "specversion": "1.0",
                                "type": "de.accounting.payment.due",
                                "source": format!("urn:accountingd:{}", cfg_sepa.tenant),
                                "id": uuid::Uuid::new_v4().to_string(),
                                "time": time::OffsetDateTime::now_utc().to_string(),
                                "datacontenttype": "application/json",
                                "data": {
                                    "due_date": target_date.to_string(),
                                    "account_count": pairs.len(),
                                    "total_ct": entries.iter().map(|(_, ct)| ct).sum::<i64>(),
                                    "pain008_xml": pain_xml,
                                }
                            });
                            let client = reqwest::Client::new();
                            match client
                                .post(url)
                                .header("Content-Type", "application/cloudevents+json")
                                .json(&ce)
                                .send()
                                .await
                            {
                                Ok(resp) if resp.status().is_success() => {
                                    tracing::info!(
                                        count = pairs.len(),
                                        due_date = %target_date,
                                        "accountingd: SEPA N-5 pain.008 dispatched to ERP"
                                    );
                                }
                                Ok(resp) => {
                                    tracing::warn!(
                                        status = %resp.status(),
                                        "accountingd: SEPA N-5 ERP webhook returned error"
                                    );
                                }
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "accountingd: SEPA N-5 ERP webhook failed"
                                    );
                                }
                            }
                        } else {
                            tracing::warn!(
                                count = pairs.len(),
                                "accountingd: SEPA N-5 — no erp_webhook_url configured; pain.008 generated but not dispatched"
                            );
                        }
                    }
                    Ok(_) => {
                        tracing::debug!(
                            target_billing_day,
                            "accountingd: SEPA N-5 — no mandates due"
                        );
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "accountingd: SEPA N-5 DB error");
                    }
                }

                // Sleep ~24h; use 23h to drift-proof against DST transitions.
                tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    axum::serve(listener, app).await.context("serve")
}
