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
//! All outbound CloudEvents are signed with Standard Webhooks when
//! `erp_hmac_secret` is set. The §§41f/41g Sperr- and Entsperrauftrag are
//! **market messages** — ORDERS 17115/17117 dispatched through `makod` — and the
//! CloudEvent announces the dispatch rather than carrying it.
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

        // ── Cedar ABAC ────────────────────────────────────────────────────
        // Authentication says *who* is calling; this says what they may do.
        // accountingd enabled the `cedar` feature and enforced nothing, and
        // twenty-four endpoints named no `Claims` extractor at all — so customer
        // balances, full Kontokorrent histories, SEPA mandates, IBANs, the aging
        // list for the whole book and generated pain.001 payout XML were served
        // to any caller that could open a socket.
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/accountingd.cedar"
            ))
            .context("accountingd.cedar must parse at startup")?,
        );

        // ── doubleentry ledger — accountingd's accounting/storage base ──────────
        // One ledger per deployment, in its own `doubleentry` PG schema sharing
        // this database. Constructing it applies the ledger schema and restores the
        // account registry (see `ledger.rs`).
        let ledger = Arc::new(
            accountingd::ledger::PgLedger::connect(&cfg.database.url, &cfg.tenant)
                .await
                .context("connect doubleentry ledger")?,
        );
        // ── makod command client — the §§41f/41g market channel ────────────
        // Phase 3/4 of the disconnection sequence are ORDERS 17115/17117, so they
        // go out over the market, not as an HTTP call into the NB's own queue.
        let makod = match (cfg.makod_url.as_deref(), cfg.makod_api_key.as_ref()) {
            (Some(url), Some(key)) => Some(Arc::new(mako_markt::makod_client::MakodClient::new(
                url,
                key.clone(),
            ))),
            (Some(_), None) => anyhow::bail!(
                "makod_url is set but makod_api_key is not. The §§41f/41g sequence \
                 dispatches ORDERS 17115/17117 through makod's command API, which is \
                 bearer-authenticated; an unauthenticated dispatch is silently refused \
                 and the sequence stalls at the Sperrauftrag."
            ),
            (None, _) => {
                tracing::warn!(
                    "accountingd: no makod_url — the §§41f/41g EnWG disconnection sequence \
                     is disabled (no Sperr-/Entsperrauftrag can be issued)"
                );
                None
            }
        };

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
            // The advances a settling invoice may deduct (§ 14 Abs. 5 Satz 2
            // UStG), with the rate each was raised at. billingd's §40b sweep
            // reads it; without it an automated Jahresrechnung cannot itemise
            // the Abschläge § 40 Abs. 1 EnWG requires it to show.
            .route(
                "/api/v1/accounts/{malo_id}/abschlaege",
                get(handlers::get_abschlaege),
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
            // camt.053 end-of-day statement — same booking rules, plus the
            // bank's own closing balance for reconciliation.
            .route(
                "/api/v1/payments/import/camt053",
                post(handlers::import_payments_camt053),
            )
            // camt.052 intraday report — the door for a bank that reports
            // intraday as camt.052 rather than camt.054. Only booked entries
            // post; the provisional ones are reported.
            .route(
                "/api/v1/payments/import/camt052",
                post(handlers::import_payments_camt052),
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
            // §41g Abs. 1 S. 2 — the offer of an Abwendungsvereinbarung, recorded
            // separately from its acceptance: a supplier that never offered one
            // was otherwise indistinguishable from one that offered and was
            // refused, and only the first breaches the statute.
            .route(
                "/api/v1/dunning/{id}/abwendung/angebot",
                post(handlers::abwendung_angebot),
            )
            // ── Mahnsperren — every §§41f/41g halt, with a reason and an end ──
            // One endpoint replaces `.../abwendung` and `.../unverhaeltnismaessig`,
            // which set bare timestamps that nothing could lift.
            .route(
                "/api/v1/dunning/{id}/locks",
                get(handlers::get_locks).post(handlers::place_lock),
            )
            .route(
                "/api/v1/dunning/locks/review",
                get(handlers::get_locks_due_review),
            )
            .route(
                "/api/v1/dunning/locks/{lock_id}",
                axum::routing::delete(handlers::lift_lock),
            )
            // ── Forderungseinwände (§41f Abs. 3 S. 3–5) ──────────────────────
            // Amounts that stay out of the Verzug calculation. Not halts: they
            // reduce what the threshold is measured against.
            .route(
                "/api/v1/dunning/{id}/einwaende",
                get(handlers::get_einwaende).post(handlers::place_einwand),
            )
            .route(
                "/api/v1/einwaende/{einwand_id}/erledigen",
                post(handlers::close_einwand),
            )
            // ── SEPA ───────────────────────────────────────────────────────────────
            .route("/api/v1/sepa/mandates", post(handlers::post_mandate))
            .route(
                "/api/v1/sepa/mandates/{mandate_id}",
                get(handlers::get_mandate).delete(handlers::delete_mandate),
            )
            .route("/api/v1/sepa/run", post(handlers::run_sepa))
            // EPC 36-month dormancy — mandates that are, or are about to become,
            // uncollectable. Tracked here because no bank tracks it for us.
            .route(
                "/api/v1/sepa/mandates/dormant",
                get(handlers::get_dormant_mandates),
            )
            // What a collection run collected, and where each entry stands —
            // the list a reversal is chosen from.
            .route(
                "/api/v1/sepa/collections/{run_id}/entries",
                get(handlers::get_collection_entries),
            )
            // pain.002 Payment Status Report — applies to whatever the bank's
            // references point at (pain.001 payouts, pain.008 collections),
            // including the Verification of Payee outcome.
            .route("/api/v1/sepa/pain002", post(handlers::import_pain002))
            // pain.007 reversal — the creditor giving a settled collection back.
            .route("/api/v1/sepa/reversals", post(handlers::post_sepa_reversal))
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
            // Proves a customer's closing balance for a sealed period, not just
            // that a booking exists — the question a Betriebsprüfung asks.
            .route(
                "/api/v1/periods/{period_id}/balance-proof",
                get(handlers::get_period_balance_proof),
            )
            // Append-only evidence for an auditor holding an archived head.
            .route(
                "/api/v1/entries/consistency-proof",
                get(handlers::get_consistency_proof),
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
            .layer(Extension(Arc::clone(&cedar)))
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
            creditor_address: cfg.creditor_address.clone(),
            iban_key: iban_hash_key,
            pain008_schema,
        });
        let app = app.merge(mcp_server::router(mcp_state, ct.clone()));

        // ── Background Abschlagslauf scheduler ──────────────────────────────
        //
        // Daily: every account whose `billing_day` is today gets one
        // **Abschlagsforderung** — a Kontokorrent debit against Erhaltene
        // Anzahlungen, plus a register row carrying the § 14 Abs. 5 Satz 2 UStG
        // rate it was raised at.
        //
        // The demand is not the money: payment arrives afterwards as a
        // `ZAHLUNG` credit that clears this debit FIFO. A credit here would
        // assert the collection had already settled — crediting the customer
        // twice for one payment, and leaving an unpaid advance invisible to
        // both the Mahnwesen and the § 41f Abs. 3 Verzug.
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
                                "accountingd: Abschlagslauf — raising Abschlagsforderungen"
                            );
                            // The advance covers the month it is raised in and is
                            // owed on the day it is raised.
                            let periode = today.replace_day(1).unwrap_or(today);
                            for acct in &accounts {
                                let raised = accountingd::pg::raise_abschlagsforderung(
                                    &ledger_bg, &pool_bg, &tenant_bg, acct, periode, today, today,
                                )
                                .await;
                                let reference = match raised {
                                    Ok((reference, _)) => reference,
                                    Err(e) => {
                                        tracing::warn!(
                                            malo_id = %acct.malo_id,
                                            error = %e,
                                            "accountingd: Abschlagsforderung failed"
                                        );
                                        continue;
                                    }
                                };

                                // Announced once per (MaLo, month). The
                                // raise is idempotent on its key but still
                                // returns `Ok`, so announcing on that alone
                                // would re-send whenever the 23-hour loop
                                // drifted across midnight. The CloudEvent id is
                                // the same key and `outbox::enqueue` is
                                // `ON CONFLICT DO NOTHING`, so a replay drops
                                // at the outbox.
                                let ce = mako_service::CloudEvent::new(
                                    mako_service::source("accountingd", &tenant_bg),
                                    mako_events::accounting::ABSCHLAG_POSTED,
                                    &acct.malo_id,
                                    serde_json::json!({
                                        "malo_id":      acct.malo_id,
                                        "lf_mp_id":     acct.lf_mp_id,
                                        "amount_ct":    acct.abschlag_ct,
                                        "amount_eur":   format!("{:.2}", acct.abschlag_ct as f64 / 100.0),
                                        "ust_satz":     acct.abschlag_ust_satz.to_string(),
                                        "faellig_am":   today.to_string(),
                                        "period_month": format!("{:04}-{:02}", today.year(), today.month() as u8),
                                        "reference":    reference,
                                    }),
                                )
                                .with_id(reference.clone());
                                let announced = async {
                                    let mut tx = pool_bg.begin().await?;
                                    mako_service::outbox::enqueue(&mut tx, &ce).await?;
                                    tx.commit().await?;
                                    anyhow::Ok(())
                                }
                                .await;
                                if let Err(e) = announced {
                                    tracing::warn!(
                                        malo_id = %acct.malo_id,
                                        error = %e,
                                        "accountingd: Abschlag announcement failed"
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

        // ── SEPA pre-notification scheduler ─────────────────────────────────────
        // Runs daily, finds the accounts whose `billing_day` falls
        // `sepa_pre_notification_days` ahead, builds one pain.008 for that
        // collection date and announces it as `de.accounting.payment.due` — the
        // event the ERP turns into the debtor's pre-notification.
        //
        // **14 calendar days by default.** The EPC SDD Core Rulebook requires the
        // creditor to notify the debtor at least 14 calendar days before the due
        // date unless the contract agrees a shorter period. This was hard-coded
        // to 5 and named "N-5" — which is the *bank submission* lead time, a
        // different deadline owed to a different party — while the config field
        // that was supposed to set it was read by nothing.
        {
            let pool_sepa = pool.clone();
            let cfg_sepa = Arc::clone(&cfg);
            tokio::spawn(async move {
                // Offset start so N-5 and Abschlagslauf do not run simultaneously.
                tokio::time::sleep(tokio::time::Duration::from_secs(120)).await;
                let lead_days = cfg_sepa
                    .sepa_pre_notification_days
                    .unwrap_or(14)
                    .clamp(1, 60);
                loop {
                    let today = time::OffsetDateTime::now_utc().date();
                    // Wraps correctly across month end.
                    let target_date = today + time::Duration::days(lead_days);
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
                                "accountingd: SEPA — generating pain.008 pre-notifications"
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

                            let creditor = accountingd::sepa::CreditorIdentity {
                                iban: creditor_iban,
                                name: creditor_name,
                                creditor_id,
                                address: Some(&cfg_sepa.creditor_address),
                            };
                            let run = match accountingd::sepa::build_pain_008(
                                &creditor,
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
                                    batch,
                                )
                                .await
                                {
                                    Ok(id) => Some(id),
                                    Err(e) => {
                                        tracing::warn!(error = %e, "accountingd: SEPA — failed to persist sepa_collection_run");
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
        // ── The document side of the Mahnwesen ────────────────────────────
        // `outputd` renders and delivers; `vertragd` says who the customer is.
        // Both or neither — an unaddressed Mahnung is not Textform.
        let documents = match (cfg.outputd_url.as_deref(), cfg.vertragd_url.as_deref()) {
            (Some(out), Some(vert)) => Some((
                Arc::new(accountingd::clients::OutputdClient::new(
                    out,
                    cfg.outputd_api_key
                        .as_ref()
                        .map(|k| secrecy::ExposeSecret::expose_secret(k).to_owned()),
                )),
                Arc::new(accountingd::clients::VertragdClient::new(
                    vert,
                    cfg.vertragd_api_key
                        .as_ref()
                        .map(|k| secrecy::ExposeSecret::expose_secret(k).to_owned()),
                )),
            )),
            (None, None) => None,
            (out, _) => {
                anyhow::bail!(
                    "[{}] is configured without the other: rendering a Mahnung needs `outputd_url` \
                     and addressing it needs `vertragd_url` (§ 126b BGB names the recipient). \
                     Configure both, or neither and let the ERP send the letters off the \
                     de.accounting.mahnung.issued CloudEvent.",
                    if out.is_some() {
                        "outputd_url"
                    } else {
                        "vertragd_url"
                    }
                );
            }
        };
        if documents.is_none() && cfg.erp_webhook_url.is_none() && cfg.dunning_auto_enabled {
            // Not fatal — an operator may read `dunning_cases` directly — but
            // it is the state in which customers are escalated toward a
            // disconnection without being told.
            tracing::warn!(
                "accountingd: auto-dunning is on with neither outputd_url nor erp_webhook_url — \
                 Mahnungen will be recorded and never sent to anyone"
            );
        }

        if cfg.dunning_auto_enabled {
            let pool_dun = pool.clone();
            let ledger_dun = Arc::clone(&ledger);
            let cfg_dun = Arc::clone(&cfg);
            let makod_dun = makod.clone();
            let documents_dun = documents.clone();
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

                    // Give every open case a document. Separate from the
                    // escalation because it fails differently: the escalation
                    // is arithmetic on the ledger, while issuing a document
                    // needs a rolled-out template, a customer on file and a
                    // reachable channel. Folded together, one missing e-mail
                    // address would roll back a Mahnstufe.
                    if let Some((outputd_dun, vertragd_dun)) = documents_dun.as_ref() {
                        match accountingd::mahnung::issue_pending(
                            &pool_dun,
                            &ledger_dun,
                            outputd_dun,
                            vertragd_dun,
                            &cfg_dun,
                        )
                        .await
                        {
                            Ok(s) if s.issued + s.unaddressable + s.errors > 0 => {
                                tracing::info!(
                                    issued = s.issued,
                                    unaddressable = s.unaddressable,
                                    errors = s.errors,
                                    "accountingd: Mahnung documents issued"
                                );
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::error!(error = %e, "accountingd: Mahnung document sweep failed");
                            }
                        }
                    }

                    // §§41f/41g EnWG disconnection sequence — runs **every** cycle,
                    // independently of whether new Mahnungen were created today: a
                    // Stufe-3 case escalated on a previous day still needs its
                    // Androhung → Ankündigung → Sperrauftrag advanced on schedule.
                    // Idempotent (each phase query excludes already-advanced,
                    // halted, or no-longer-in-Verzug cases); skipped entirely when
                    // no makod endpoint is configured.
                    if let Some(makod_dun) = makod_dun.as_ref() {
                        match accountingd::sperr::run_sperr_sequence(&pool_dun, makod_dun, &cfg_dun)
                            .await
                        {
                            Ok(s)
                                if s.androhungen
                                    + s.ankuendigungen
                                    + s.sperrauftraege
                                    + s.entsperrauftraege
                                    > 0 =>
                            {
                                tracing::info!(
                                    androhungen = s.androhungen,
                                    ankuendigungen = s.ankuendigungen,
                                    sperrauftraege = s.sperrauftraege,
                                    entsperrauftraege = s.entsperrauftraege,
                                    "accountingd: §§41f/41g disconnection sequence advanced"
                                );
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::error!(error = %e, "accountingd: §§41f/41g sequence error");
                            }
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

        // ── Annual Jahresabschluss worker (§ 40b Abs. 1 EnWG) ───────────────
        //
        // Settles the previous year for every account that has none, through
        // the same function the operator's POST drives.
        //
        // Off by default, and no earlier than `jahresabschluss_start_day`,
        // because the settlement moves money: an overpaid year is refunded by
        // pain.001 the moment it is settled, and settling on 1 January would
        // refund against December invoices the § 40c Abs. 2 six-week window
        // means nobody has issued yet.
        if cfg.jahresabschluss_auto_enabled {
            let pool_ja = pool.clone();
            let ledger_ja = Arc::clone(&ledger);
            let cfg_ja = Arc::clone(&cfg);
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(240)).await;
                let (start_month, start_day) = parse_start_day(
                    cfg_ja
                        .jahresabschluss_start_day
                        .as_deref()
                        .unwrap_or("02-01"),
                );
                loop {
                    let Some(mut wlock) = accountingd::pg::try_worker_lock(
                        &pool_ja,
                        accountingd::pg::LOCK_JAHRESABSCHLUSS,
                    )
                    .await
                    else {
                        tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                        continue;
                    };
                    let today = time::OffsetDateTime::now_utc().date();
                    let open = (u8::from(today.month()), today.day()) >= (start_month, start_day);
                    if open {
                        let year = today.year() - 1;
                        run_jahresabschluss_sweep(&pool_ja, &ledger_ja, &cfg_ja, year).await;
                    } else {
                        tracing::debug!(
                            start = %format!("{start_month:02}-{start_day:02}"),
                            "accountingd: Jahresabschluss window not open yet"
                        );
                    }
                    accountingd::pg::release_worker_lock(
                        &mut wlock,
                        accountingd::pg::LOCK_JAHRESABSCHLUSS,
                    )
                    .await;
                    tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                }
            });
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

/// Parse a `MM-DD` window start, falling back to 1 February.
///
/// A malformed value is a configuration mistake, and the fallback is the
/// documented default rather than "today": settling from an unparsed string on
/// 1 January would refund a year whose December invoices the § 40c Abs. 2 EnWG
/// six-week window means nobody has issued yet.
fn parse_start_day(raw: &str) -> (u8, u8) {
    let parsed = raw
        .split_once('-')
        .and_then(|(m, d)| Some((m.parse::<u8>().ok()?, d.parse::<u8>().ok()?)))
        .filter(|(m, d)| (1..=12).contains(m) && (1..=28).contains(d));
    match parsed {
        Some(v) => v,
        None => {
            tracing::warn!(
                value = raw,
                "accountingd: jahresabschluss_start_day is not a MM-DD day of the first 28 — \
                 falling back to 02-01"
            );
            (2, 1)
        }
    }
}

/// Settle `year` for every account that has no settlement for it.
async fn run_jahresabschluss_sweep(
    pool: &sqlx::PgPool,
    ledger: &Arc<accountingd::ledger::PgLedger>,
    cfg: &Arc<accountingd::config::AccountingdConfig>,
    year: i32,
) {
    let year_i16 = match i16::try_from(year) {
        Ok(y) => y,
        Err(_) => return,
    };
    // Bounded per pass: the sweep runs daily and the candidate query selects on
    // the absence of a settlement, so whatever is left is picked up tomorrow.
    // An unbounded pass over a large portfolio would hold the advisory lock for
    // hours and issue refunds faster than a bank adapter accepts them.
    const BATCH: i64 = 500;
    let candidates =
        match accountingd::pg::list_jahresabschluss_candidates(pool, &cfg.tenant, year_i16, BATCH)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "accountingd: Jahresabschluss candidate scan failed");
                return;
            }
        };
    if candidates.is_empty() {
        tracing::debug!(year, "accountingd: every account is settled for the year");
        return;
    }
    let (mut settled, mut refused, mut failed) = (0u32, 0u32, 0u32);
    for (malo_id, lf_mp_id) in &candidates {
        let q = accountingd::handlers::JahresabschlussQuery {
            lf_mp_id: Some(lf_mp_id.clone()),
            year: Some(year),
            dry_run: Some(false),
        };
        match accountingd::handlers::settle_jahresabschluss(pool, ledger, cfg, malo_id, &q).await {
            Ok(_) => settled += 1,
            Err(e) if e.is_transient() => {
                failed += 1;
                tracing::error!(%malo_id, year, error = %e, "accountingd: Jahresabschluss failed — retried tomorrow");
            }
            Err(e) => {
                // This account's own state, and it will look the same tomorrow:
                // no IBAN and no creditor account, an unbuildable pain.001.
                // Counted and named rather than retried forever.
                refused += 1;
                tracing::warn!(%malo_id, year, error = %e, "accountingd: Jahresabschluss refused");
            }
        }
    }
    tracing::info!(
        year,
        settled,
        refused,
        failed,
        remaining = candidates.len() == usize::try_from(BATCH).unwrap_or(usize::MAX),
        "accountingd: Jahresabschluss sweep complete"
    );
}
