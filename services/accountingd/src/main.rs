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
//! | `de.accounting.payment.due` | SEPA collection run dispatched (once per run) |
//! | `de.accounting.erstattung.faellig` | Jahresabschluss refund (pain.001 attached) |
//!
//! All outbound CloudEvents are HMAC-signed (`X-Mako-Signature`) when
//! `erp_hmac_secret` is set. A Mahnstufe-3 case ≥ the Sperrung threshold is
//! handed to `sperrd` directly (`POST /api/v1/sperr-orders`), not as a CE.
//!
//! Port: `:9380`

use accountingd::{config, handlers, mcp_server};
use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_service::{Daemon, ServiceContext, oidc::OidcConfig};
use std::sync::Arc;
use tracing::info;

/// The `accountingd` daemon. `mako_service::run` owns the lifecycle (tracing,
/// tuned pool with `application_name`, real DB-ping readiness, graceful
/// shutdown); this supplies the migrations plus the domain router, its Extension
/// layers, the OIDC verifier, the MCP server, and every background worker (the
/// Abschlagslauf, SEPA N-5, auto-dunning and outbox-drain schedulers — the
/// multi-replica ones guarded by Postgres advisory worker-locks).
struct Accountingd;

impl Daemon for Accountingd {
    type Config = config::AccountingdConfig;
    const NAME: &'static str = "accountingd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .map_err(|e| anyhow::anyhow!("run accountingd migrations: {e}"))?;

        // Transactional outbox: create the event_outbox table (idempotent).
        // Outbound CloudEvents are persisted here in the same tx as their
        // ledger/state write, then drained by the OutboxWorker (spawned in
        // `build` when an ERP webhook is set).
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure outbox schema")?;
        Ok(())
    }

    async fn build(
        cfg: Arc<config::AccountingdConfig>,
        ctx: ServiceContext,
    ) -> anyhow::Result<Router> {
        // Validate SEPA schema-version config at startup — a bank-incompatible
        // schema version must fail loudly here, not silently on a rejected batch
        // downstream.
        let pain008_schema =
            accountingd::sepa::resolve_pain008_schema(cfg.pain008_schema.as_deref())
                .context("SEPA pain.008 schema config")?;
        accountingd::sepa::resolve_pain001_schema(cfg.pain001_schema.as_deref())
            .context("SEPA pain.001 schema config")?;

        let pool = ctx.pool().clone();
        let ct = ctx.shutdown.clone();

        // ── OIDC verifier (auth on financial REST endpoints) ──────────────
        let oidc =
            OidcConfig::build_verifier(cfg.oidc.as_ref(), &ctx.http, &cfg.tenant, ct.clone())
                .await
                .context("OIDC verifier init")?;
        if oidc.is_disabled() {
            tracing::warn!(
                "[WARN] OIDC disabled -- financial write endpoints accept all requests (dev mode)"
            );
        }

        // ── doubleentry ledger — accountingd's accounting/storage base ──────────
        // One ledger per deployment, in its own `doubleentry` PG schema sharing
        // this database. Constructing it applies the ledger schema and restores the
        // account registry (see `ledger.rs`).
        let ledger = Arc::new(
            accountingd::ledger::PgLedger::connect(&cfg.database.url, &cfg.tenant)
                .await
                .context("connect doubleentry ledger")?,
        );
        let iban_hash_key = cfg
            .iban_hash_secret
            .as_ref()
            .map(|s| accountingd::ledger::iban_hash_key(secrecy::ExposeSecret::expose_secret(s)));
        if iban_hash_key.is_none() {
            tracing::warn!(
                "[WARN] iban_hash_secret unset -- IBAN lookup hash is unkeyed (dev mode)"
            );
        }

        let app = Router::new()
            // ── CloudEvent ingest ──────────────────────────────────────────────────
            .route("/webhook", post(handlers::ingest_webhook))
            // ── Account endpoints ──────────────────────────────────────────────────
            .route(
                "/api/v1/accounts/{malo_id}",
                get(handlers::get_account).put(handlers::put_account),
            )
            .route(
                "/api/v1/accounts/{malo_id}/balance",
                get(handlers::get_balance),
            )
            .route(
                "/api/v1/accounts/{malo_id}/ledger",
                get(handlers::get_ledger),
            )
            .route(
                "/api/v1/accounts/{malo_id}/kontoauszug",
                get(handlers::get_kontoauszug),
            )
            .route(
                "/api/v1/accounts/{malo_id}/abschlag",
                put(handlers::put_abschlag),
            )
            .route(
                "/api/v1/accounts/{malo_id}/buchen",
                post(handlers::post_buchen),
            )
            // Vorauszahlung — BO4E typed advance-payment schedule (L12 — §40 Abs. 1 EnWG)
            .route(
                "/api/v1/accounts/{malo_id}/vorauszahlung",
                get(handlers::get_vorauszahlung).put(handlers::put_vorauszahlung),
            )
            // Zahlungsinformation typed BO4E REST (IBAN + BIC + SEPA, rubo4e::current::Zahlungsinformation)
            .route(
                "/api/v1/accounts/{malo_id}/zahlungsinformation",
                get(handlers::get_zahlungsinformation).put(handlers::put_zahlungsinformation),
            )
            // ── Payment import ─────────────────────────────────────────────────────
            .route("/api/v1/payments/import", post(handlers::import_payments))
            .route(
                "/api/v1/payments/import/camt054",
                post(handlers::import_payments_camt054),
            )
            // ── Offene Posten ──────────────────────────────────────────────────────
            .route(
                "/api/v1/accounts/{malo_id}/business-partner",
                axum::routing::put(handlers::put_account_business_partner),
            )
            .route(
                "/api/v1/business-partners/{kunden_nr}/accounts",
                get(handlers::get_bp_accounts),
            )
            .route(
                "/api/v1/business-partners/{kunden_nr}/balance",
                get(handlers::get_bp_balance),
            )
            .route("/metrics", get(handlers::metrics))
            .route("/api/v1/offene-posten", get(handlers::get_offene_posten))
            // ── Dunning ────────────────────────────────────────────────────────────
            .route("/api/v1/dunning", get(handlers::get_dunning))
            .route(
                "/api/v1/dunning/{account_id}/escalate",
                post(handlers::escalate_dunning),
            )
            .route(
                "/api/v1/dunning/{id}/resolve",
                post(handlers::resolve_dunning),
            )
            .route(
                "/api/v1/dunning/{id}/abwendung",
                post(handlers::abwendung_dunning),
            )
            .route(
                "/api/v1/dunning/{id}/unverhaeltnismaessig",
                post(handlers::unverhaeltnismaessig_dunning),
            )
            // ── SEPA ───────────────────────────────────────────────────────────────
            .route("/api/v1/sepa/mandates", post(handlers::post_mandate))
            .route(
                "/api/v1/sepa/mandates/{mandate_id}",
                get(handlers::get_mandate).delete(handlers::delete_mandate),
            )
            .route("/api/v1/sepa/run", post(handlers::run_sepa))
            // ── §25 EEG 2023 — SEPA Credit Transfer payout pipeline ───────────────
            // GET  /api/v1/eeg/payouts             — list payout orders (?status=PDNG|ACCP|RJCT|CANC)
            // GET  /api/v1/eeg/payouts/{id}        — single order with pain.001 XML
            // POST /api/v1/eeg/payouts/run         — batch-generate for unbatched EEG_GUTSCHRIFT entries
            // PUT  /api/v1/eeg/payouts/{id}/status — process pain.002 ACCP/RJCT/CANC
            .route("/api/v1/eeg/payouts", get(handlers::get_eeg_payouts))
            .route(
                "/api/v1/eeg/payouts/run",
                post(handlers::post_run_eeg_payouts),
            )
            .route(
                "/api/v1/eeg/payouts/{payout_id}",
                get(handlers::get_eeg_payout),
            )
            .route(
                "/api/v1/eeg/payouts/{payout_id}/status",
                axum::routing::put(handlers::put_eeg_payout_status),
            )
            .route(
                "/api/v1/jahresabschluss/{malo_id}",
                post(handlers::post_jahresabschluss),
            )
            // ── Balance reconciliation ──────────────────────────────────────
            // POST /api/v1/accounts/{malo_id}/reconcile?repair=true
            // Detects and optionally corrects balance_ct cache drift.
            .route(
                "/api/v1/accounts/{malo_id}/reconcile",
                post(handlers::post_reconcile),
            )
            // ── Open-item management ─────────────────────────────────────────
            // GET /api/v1/accounts/{malo_id}/open-items
            // FIFO-cleared list of unpaid/partially-paid invoice debits.
            .route(
                "/api/v1/accounts/{malo_id}/open-items",
                get(handlers::get_open_items),
            )
            // ── GDPR Art. 17 anonymization ──────────────────────────────────
            // POST /api/v1/accounts/{malo_id}/anonymize
            // Pseudonymizes PII while preserving ledger records (§238 HGB).
            .route(
                "/api/v1/accounts/{malo_id}/anonymize",
                post(handlers::post_anonymize),
            ) // ── Aging analysis ─────────────────────────────────────────────────
            // GET /api/v1/aging — overdue receivables grouped by age bucket (0-30d/31-60d/61-90d/>90d)
            .route("/api/v1/aging", get(handlers::get_aging))
            // ── Festschreibung (period seals) + audit proofs — GoBD / § 146 AO ──
            .route(
                "/api/v1/periods/{period_id}/seal",
                post(handlers::post_seal_period),
            )
            .route("/api/v1/periods/seals", get(handlers::get_seals))
            .route(
                "/api/v1/entries/{entry_id}/proof",
                get(handlers::get_entry_proof),
            )
            // ── Summen- und Saldenliste + Zahlungszuordnung (open-item clearing) ──
            .route("/api/v1/trial-balance", get(handlers::get_trial_balance))
            .route(
                "/api/v1/accounts/{malo_id}/clear",
                post(handlers::post_clear),
            )
            .route(
                "/api/v1/clearings/{clearing_id}/reset",
                post(handlers::post_reset_clearing),
            )
            // ── Verzugszinsen §288 BGB ─────────────────────────────────────────
            // GET  /api/v1/accounts/{malo_id}/interest-charges
            // POST /api/v1/accounts/{malo_id}/interest-charges
            .route(
                "/api/v1/accounts/{malo_id}/interest-charges",
                get(handlers::get_interest_charges).post(handlers::post_interest_charge),
            )
            // ── Payment plans (Zahlungsvereinbarung) ───────────────────────────
            // GET  /api/v1/accounts/{malo_id}/payment-plans
            // POST /api/v1/accounts/{malo_id}/payment-plans
            .route(
                "/api/v1/accounts/{malo_id}/payment-plans",
                get(handlers::get_payment_plans).post(handlers::post_payment_plan),
            )
            // GET    /api/v1/payment-plans/{id}
            // DELETE /api/v1/payment-plans/{id}
            .route(
                "/api/v1/payment-plans/{plan_id}",
                get(handlers::get_payment_plan).delete(handlers::delete_payment_plan),
            )
            .layer(Extension(Arc::clone(&cfg)))
            .layer(Extension(pool.clone()))
            .layer(Extension(Arc::clone(&ledger)))
            .layer(Extension(iban_hash_key))
            // OIDC verifier extension — enables Claims extractor on write endpoints
            .layer(Extension(oidc));

        // ── MCP server ────────────────────────────────────────────────────────────
        let mcp_state = std::sync::Arc::new(mcp_server::AccountingdMcpState {
            pool: pool.clone(),
            ledger: Arc::clone(&ledger),
            tenant: cfg.tenant.clone(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
            creditor_iban: cfg.creditor_iban.clone(),
            creditor_name: cfg.creditor_name.clone(),
            creditor_id: cfg.creditor_id.clone(),
            pain008_schema,
        });
        let app = app.merge(mcp_server::router(mcp_state, ct.clone()));

        // ── Background Abschlagslauf scheduler ──────────────────────────────────
        // Runs daily at approximately 06:00 and checks which accounts have their
        // billing_day = today. For each: posts an ABSCHLAG ledger entry.
        {
            let pool_bg = pool.clone();
            let ledger_bg = Arc::clone(&ledger);
            let tenant_bg = cfg.tenant.clone();
            tokio::spawn(async move {
                // Initial delay to let the service start up cleanly.
                tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                loop {
                    let Some(mut wlock) =
                        accountingd::pg::try_worker_lock(&pool_bg, accountingd::pg::LOCK_ABSCHLAG)
                            .await
                    else {
                        tracing::debug!(
                            "accountingd: Abschlag worker — another replica holds the lock"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                        continue;
                    };
                    let now_utc = time::OffsetDateTime::now_utc();
                    let today = now_utc.date();
                    let day_of_month = today.day() as i16;
                    match accountingd::pg::find_accounts_due(&pool_bg, &tenant_bg, day_of_month)
                        .await
                    {
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
                                if let Err(e) = accountingd::pg::post_entry(
                                    &ledger_bg,
                                    &pool_bg,
                                    &tenant_bg,
                                    &acct.malo_id,
                                    &acct.lf_mp_id,
                                    "ABSCHLAG",
                                    // Advance payment = CREDIT (negative): reduces the
                                    // customer's balance. The full annual Rechnung is
                                    // booked as a debit; balance nets to the Nachzahlung.
                                    -acct.abschlag_ct,
                                    // deterministic key → idempotent per (malo, month)
                                    &ref_id,
                                    None,
                                    Some(&ref_id),
                                    today,
                                    today,
                                    Some(&format!("Monatlicher Abschlag Tag {day_of_month}")),
                                    None,
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
                            tracing::debug!(
                                day = day_of_month,
                                "accountingd: no Abschläge due today"
                            );
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "accountingd: Abschlagslauf DB error");
                        }
                    }
                    accountingd::pg::release_worker_lock(
                        &mut wlock,
                        accountingd::pg::LOCK_ABSCHLAG,
                    )
                    .await;
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

                    match accountingd::pg::find_accounts_due_for_sepa(
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

                            // Build one pain.008 message — one PmtInf group per
                            // SequenceType (SEPA Rulebook §3.8), one audit row.
                            // hard error if creditor_iban is missing/invalid — skip run with error log.
                            let entries: Vec<(&accountingd::pg::SepaMandateRow, i64)> =
                                pairs.iter().map(|(m, a)| (m, a.abschlag_ct)).collect();
                            let creditor_iban = match cfg_sepa
                                .creditor_iban
                                .as_deref()
                                .filter(|s| !s.is_empty())
                            {
                                Some(iban) => iban,
                                None => {
                                    tracing::error!(
                                        "accountingd: SEPA N-5 — creditor_iban not configured; \
                                     pain.008 generation BLOCKED. Set creditor_iban in accountingd.toml."
                                    );
                                    tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600))
                                        .await;
                                    continue;
                                }
                            };
                            // Creditor name defaults to tenant if not configured separately
                            let creditor_name = cfg_sepa
                                .creditor_name
                                .as_deref()
                                .unwrap_or(&cfg_sepa.tenant);
                            let Some(creditor_id) =
                                cfg_sepa.creditor_id.as_deref().filter(|s| !s.is_empty())
                            else {
                                tracing::error!(
                                    "accountingd: SEPA N-5 — creditor_id (Gläubiger-ID) not \
                                 configured; the EPC rulebook mandates CdtrSchmeId. \
                                 pain.008 generation BLOCKED."
                                );
                                tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600))
                                    .await;
                                continue;
                            };

                            let run = match accountingd::sepa::build_pain_008(
                                creditor_iban,
                                creditor_name,
                                creditor_id,
                                target_date,
                                &entries,
                                pain008_schema,
                            ) {
                                Ok(r) => r,
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "accountingd: SEPA N-5 — pain.008 generation failed"
                                    );
                                    tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600))
                                        .await;
                                    continue;
                                }
                            };

                            {
                                let batch = &run;
                                // Persist the single pain.008 message for audit and ERP
                                // replay — exactly one row per (tenant, collection_date).
                                let run_id = match accountingd::pg::persist_sepa_collection(
                                    &pool_sepa,
                                    &cfg_sepa.tenant,
                                    target_date,
                                    &batch.xml,
                                    batch.total_ct,
                                    batch.entry_count,
                                )
                                .await
                                {
                                    Ok(id) => Some(id),
                                    Err(e) => {
                                        tracing::warn!(error = %e, "accountingd: SEPA N-5 — failed to persist sepa_collection_run");
                                        None
                                    }
                                };

                                // Claim the run for dispatch AND persist the CE atomically.
                                // Persist-before-dispatch fixes the pre-flip bug: the
                                // DISPATCHED flag flip and the `de.accounting.payment.due`
                                // outbox enqueue commit in ONE transaction, so the event is
                                // durable *exactly when* the run is marked dispatched. A
                                // replica or same-day restart still sees DISPATCHED and does
                                // not re-enqueue (no double collection); the outbox worker
                                // then signs and delivers with retry/dead-letter.
                                if let Some(id) = run_id {
                                    match pool_sepa.begin().await {
                                        Ok(mut tx) => {
                                            let claimed =
                                                accountingd::pg::mark_sepa_collection_dispatched(
                                                    &mut *tx, id,
                                                )
                                                .await
                                                .unwrap_or(false);
                                            let mut commit = true;
                                            if claimed {
                                                if cfg_sepa.erp_webhook_url.is_some() {
                                                    let ce = mako_service::CloudEvent::new(
                                                        mako_service::source(
                                                            "accountingd",
                                                            &cfg_sepa.tenant,
                                                        ),
                                                        mako_events::accounting::PAYMENT_DUE,
                                                        "",
                                                        serde_json::json!({
                                                            "due_date": target_date.to_string(),
                                                            "groups": batch.groups,
                                                            "account_count": batch.entry_count,
                                                            "total_ct": batch.total_ct,
                                                            "pain008_xml": &batch.xml,
                                                        }),
                                                    )
                                                    .without_subject();
                                                    match mako_service::outbox::enqueue(
                                                        &mut tx, &ce,
                                                    )
                                                    .await
                                                    {
                                                        Ok(()) => {
                                                            tracing::info!(
                                                                count = batch.entry_count,
                                                                due_date = %target_date,
                                                                "accountingd: SEPA N-5 pain.008 enqueued for delivery"
                                                            );
                                                        }
                                                        Err(e) => {
                                                            // Roll back the dispatch flag too, so
                                                            // the next run can retry cleanly.
                                                            tracing::error!(
                                                                error = %e,
                                                                "accountingd: SEPA N-5 — outbox enqueue failed; rolling back dispatch flag"
                                                            );
                                                            commit = false;
                                                        }
                                                    }
                                                } else {
                                                    tracing::warn!(
                                                        count = batch.entry_count,
                                                        "accountingd: SEPA N-5 — no erp_webhook_url configured; pain.008 generated but not dispatched"
                                                    );
                                                }
                                            }
                                            if commit {
                                                if let Err(e) = tx.commit().await {
                                                    tracing::error!(error = %e, "accountingd: SEPA N-5 — commit failed");
                                                }
                                            } else {
                                                let _ = tx.rollback().await;
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(error = %e, "accountingd: SEPA N-5 — begin tx failed");
                                        }
                                    }
                                }
                            } // end single-run scope
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

        // ── Auto-dunning background worker ────────────────────────────────
        // Runs daily when `dunning_auto_enabled = true` in config.
        // Creates Mahnstufe 1 for newly overdue accounts and escalates 1→2→3
        // when prior Mahnungen remain unresolved past their due dates.
        //
        // Idempotent: uses `auto_dunning_runs (tenant, run_date)` UNIQUE constraint
        // to prevent double-execution on crash+restart within the same calendar day.
        if cfg.dunning_auto_enabled {
            let pool_dun = pool.clone();
            let ledger_dun = Arc::clone(&ledger);
            let cfg_dun = Arc::clone(&cfg);
            tokio::spawn(async move {
                // Stagger start relative to other workers.
                tokio::time::sleep(tokio::time::Duration::from_secs(180)).await;
                loop {
                    let Some(mut wlock) =
                        accountingd::pg::try_worker_lock(&pool_dun, accountingd::pg::LOCK_DUNNING)
                            .await
                    else {
                        tracing::debug!(
                            "accountingd: dunning worker — another replica holds the lock"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                        continue;
                    };
                    let grace_days = cfg_dun.dunning_grace_days.unwrap_or(30);
                    let fee1 = cfg_dun.dunning_fee_stufe1_ct.unwrap_or(0);
                    let fee2 = cfg_dun.dunning_fee_stufe2_ct.unwrap_or(500); // 5.00 EUR default
                    let fee3 = cfg_dun.dunning_fee_stufe3_ct.unwrap_or(1000); // 10.00 EUR default

                    match accountingd::pg::run_auto_dunning(
                        &ledger_dun,
                        &pool_dun,
                        &cfg_dun.tenant,
                        grace_days,
                        fee1,
                        fee2,
                        fee3,
                    )
                    .await
                    {
                        Ok(result) => {
                            if result.mahnstufe1_created > 0 || result.escalated > 0 {
                                tracing::info!(
                                    mahnstufe1 = result.mahnstufe1_created,
                                    escalated = result.escalated,
                                    sperrauftrag = result.sperrauftrag_triggered,
                                    "accountingd: auto-dunning run completed"
                                );
                            } else {
                                tracing::debug!(
                                    "accountingd: auto-dunning — no new Mahnungen today"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "accountingd: auto-dunning worker error");
                        }
                    }

                    // §§41f/41g EnWG disconnection sequence — runs **every** cycle,
                    // independently of whether new Mahnungen were created today: a
                    // Stufe-3 case escalated on a previous day still needs its
                    // Androhung → Ankündigung → Sperrauftrag advanced on schedule.
                    // Idempotent (each phase query excludes already-advanced/halted
                    // cases); a no-op when `sperrd_url` is unset.
                    match accountingd::sperr::run_sperr_sequence(&pool_dun, &cfg_dun).await {
                        Ok(s) if s.androhungen + s.ankuendigungen + s.sperrauftraege > 0 => {
                            tracing::info!(
                                androhungen = s.androhungen,
                                ankuendigungen = s.ankuendigungen,
                                sperrauftraege = s.sperrauftraege,
                                "accountingd: §§41f/41g disconnection sequence advanced"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "accountingd: §§41f/41g sequence error");
                        }
                    }

                    accountingd::pg::release_worker_lock(&mut wlock, accountingd::pg::LOCK_DUNNING)
                        .await;
                    // Run daily; 23h to drift-proof against DST transitions.
                    tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                }
            });
        } else {
            tracing::info!("accountingd: auto-dunning disabled (dunning_auto_enabled = false)");
        }

        // ── Transactional outbox drain worker ───────────────────────────────────
        // One worker per service, guarded on an ERP webhook being configured.
        // Delivers persisted CloudEvents with signing, retry and dead-lettering;
        // multi-replica safe (FOR UPDATE SKIP LOCKED claim inside the worker).
        if let Some(ref url) = cfg.erp_webhook_url {
            let worker = mako_service::outbox::OutboxWorker::new(
                pool.clone(),
                url.clone(),
                cfg.erp_hmac_secret.clone(),
            );
            tokio::spawn(worker.run(ct.clone()));
            info!(url, "accountingd: outbox drain worker started");
        }

        Ok(app)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Accountingd>().await
}
