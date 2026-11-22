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
//! | `de.mako.billing.rechnung.erstellt` | `billingd` | Debit entry (Brutto-Betrag) |
//! | `de.mako.eeg.verguetung.berechnet` | `einsd` | Credit entry (EEG settlement) |
//!
//! ## CloudEvents emitted (outbound webhook → ERP)
//!
//! | CE type | Trigger |
//! |---|---|
//! | `de.mako.accounting.payment.due` | SEPA collection run dispatched (once per run) |
//! | `de.mako.accounting.erstattung.faellig` | Jahresabschluss refund (pain.001 attached) |
//!
//! All outbound CloudEvents are signed with Standard Webhooks when
//! `erp_hmac_secret` is set. The §§41f/41g Sperr- and Entsperrauftrag are
//! **market messages** — ORDERS 17115/17117 dispatched through `makod` — and the
//! CloudEvent announces the dispatch rather than carrying it.
//!
//! Port: `:9380`

use crate::{config, handlers, mcp_server};
use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post},
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
pub struct Accountingd;

impl Daemon for Accountingd {
    type Config = config::AccountingdConfig;
    const NAME: &'static str = "accountingd";

    async fn migrate(cfg: &Self::Config, pool: &sqlx::PgPool) -> anyhow::Result<()> {
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

        // The ledger is created by the role that owns the schema — its `create`
        // runs DDL and installs the triggers that refuse every change to the
        // log — and the runtime role is then granted what doubleentry generates
        // from its schema: read and append, update only the maintained
        // projections, never `DELETE`, `TRUNCATE` or DDL.
        let db = &cfg.database;
        let owner_url =
            mako_service::config::resolve_env(db.migration_url.as_deref().unwrap_or(&db.url))
                .context("resolve the ledger owner's database url")?;
        let ledger =
            crate::ledger::PgLedger::connect(&owner_url, &cfg.tenant, mako_fristen::heute())
                .await
                .context("create the ledger")?;
        if let Some(role) = mako_service::config::runtime_role(db)? {
            ledger
                .grant_runtime(&role)
                .await
                .context("grant the runtime role its ledger privileges")?;
        }
        Ok(())
    }

    /// `[eeg] auto_payout = true` pays every Vergütung receipt as it arrives,
    /// so the Netzbetreiber it pays as, the account it pays from and the
    /// pain.001 version it writes must be usable before the first receipt
    /// does: a payout owed is never skipped for want of any of them.
    fn validate(cfg: &config::AccountingdConfig) -> Vec<mako_service::Befund> {
        use mako_service::befund::{Befund, Bereich};
        let mut out = Vec::new();
        if let Err(e) = cfg.check_auth_posture() {
            out.push(Befund::aus_fehler("oidc", e).bereich(Bereich::Zugriff));
        }
        if let Err(e) = rubo4e::identifiers::MarktpartnerId::new(cfg.lf_mp_id.trim()) {
            out.push(Befund::blockiert(
                "lf_mp_id",
                format!(
                    "lf_mp_id {:?} is not an MP-ID ({e}) — set the Lieferant's 13-digit \
                     BDEW- or DVGW-Codenummer",
                    cfg.lf_mp_id
                ),
            ));
        }
        if cfg.creditor_name().is_none() {
            out.push(Befund::warnung(
                "creditor_name",
                "creditor_name is not set — every pain.008 and Mahnung names the Lieferant by \
                 it and is refused without it",
            ));
        }
        // § 19 Abs. 1 EEG 2023: the EEG payout is owed „gegen den Netzbetreiber",
        // so its plant accounts and its pain.001 are the Netzbetreiber's — never
        // `lf_mp_id` or `creditor_name` in their place.
        let eeg_pflicht = |pfad: &'static str, text: &'static str| {
            if cfg.eeg.auto_payout {
                Befund::blockiert(pfad, text)
            } else {
                Befund::warnung(pfad, text)
            }
        };
        match cfg.eeg.nb_mp_id() {
            Some(nb) => {
                if let Err(e) = rubo4e::identifiers::MarktpartnerId::new(nb) {
                    out.push(Befund::blockiert(
                        "eeg.nb_mp_id",
                        format!(
                            "[eeg] nb_mp_id {nb:?} is not an MP-ID ({e}) — set the \
                             Netzbetreiber's 13-digit BDEW- or DVGW-Codenummer"
                        ),
                    ));
                }
            }
            None => out.push(eeg_pflicht(
                "eeg.nb_mp_id",
                "[eeg] nb_mp_id is not set — the EEG payout is owed by the Netzbetreiber \
                 (§ 19 Abs. 1 EEG 2023), and every EEG receipt is refused until its MP-ID is \
                 configured",
            )),
        }
        if cfg.eeg.debtor_name().is_none() {
            out.push(eeg_pflicht(
                "eeg.debtor_name",
                "[eeg] debtor_name is not set — every EEG pain.001 names the Netzbetreiber as \
                 debtor by it and is refused without it",
            ));
        }
        if let Err(e) = crate::sepa::resolve_pain001_schema(cfg.pain001_schema.as_deref()) {
            out.push(Befund::aus_fehler(
                "pain001_schema",
                format!("SEPA pain.001 schema config: {e:#}"),
            ));
        }
        if let Err(e) = crate::sepa::resolve_pain008_schema(cfg.pain008_schema.as_deref()) {
            out.push(Befund::aus_fehler(
                "pain008_schema",
                format!("SEPA pain.008 schema config: {e:#}"),
            ));
        }
        match cfg.eeg.debtor_iban.as_deref() {
            Some(iban) => {
                if let Err(e) = crate::sepa::validate_iban(iban) {
                    out.push(Befund::blockiert(
                        "eeg.debtor_iban",
                        format!("[eeg].debtor_iban is not a valid IBAN: {e}"),
                    ));
                }
            }
            None if cfg.eeg.auto_payout => out.push(Befund::blockiert(
                "eeg.debtor_iban",
                "[eeg] auto_payout = true needs [eeg].debtor_iban — the account EEG payouts \
                 are debited from",
            )),
            None => {}
        }
        if cfg.dunning_auto_enabled {
            for (pfad, tage) in cfg.zahlungsfristen() {
                if tage.is_none() {
                    out.push(Befund::blockiert(
                        pfad,
                        "dunning_auto_enabled = true needs the Zahlungsfrist every automatic \
                         Mahnung of this Stufe states, in days (at least 1)",
                    ));
                }
            }
            for (pfad, ct) in cfg.mahngebuehren() {
                if ct.is_none() {
                    out.push(Befund::blockiert(
                        pfad,
                        "dunning_auto_enabled = true needs the Mahngebühr every automatic \
                         Mahnung of this Stufe charges, in ct (0 charges none)",
                    ));
                }
            }
        }
        out
    }

    fn policy(cfg: &config::AccountingdConfig) -> mako_service::policy::PolicyReport {
        mako_service::policy::PolicyReport::new()
            .posture_flag("allow_insecure_no_auth", cfg.allow_insecure_no_auth)
            .posture_presence("oidc", cfg.oidc.is_some(), true)
            .posture_presence("erp_hmac_secret", cfg.erp_hmac_secret.is_some(), true)
    }

    async fn build(
        cfg: Arc<config::AccountingdConfig>,
        ctx: ServiceContext,
    ) -> anyhow::Result<Router> {
        // Fail closed on both doors before anything else: the REST API and the
        // inbound webhook each reach the ledger and the personal data behind
        // it.
        cfg.check_auth_posture()?;

        // Validate SEPA schema-version config at startup — a bank-incompatible
        // schema version must fail loudly here, not silently on a rejected batch
        // downstream.
        let pain008_schema = crate::sepa::resolve_pain008_schema(cfg.pain008_schema.as_deref())
            .context("SEPA pain.008 schema config")?;
        crate::sepa::resolve_pain001_schema(cfg.pain001_schema.as_deref())
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
                "accountingd: OIDC disabled — financial write endpoints accept all requests \
                 (allow_insecure_no_auth, dev mode)"
            );
        }

        // ── Cedar ABAC ────────────────────────────────────────────────────
        // Authentication says *who* is calling; this says what they may do.
        // Both are required on every route: customer balances, full
        // Kontokorrent histories, SEPA mandates, IBANs, the aging list for the
        // whole book and generated pain.001 payout XML are behind this surface,
        // and a token valid for the deployment is not by itself authority over
        // any of them.
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/accountingd.cedar"
            ))
            .context("accountingd.cedar must parse at startup")?,
        );

        // ── doubleentry ledger — accountingd's accounting/storage base ──────────
        // One ledger per deployment, in its own `ledger` schema of this
        // database, created by `migrate`; here it is opened as the runtime role.
        // The URL is resolved like the runner's own, so `env:` works for both.
        let ledger_url = mako_service::config::resolve_env(&cfg.database.url)
            .context("resolve database url for the ledger")?;
        let ledger = Arc::new(
            crate::ledger::PgLedger::connect(&ledger_url, &cfg.tenant, mako_fristen::heute())
                .await
                .context("open the ledger")?,
        );
        // Books whatever a crash or a failed booking left `offen`.
        tokio::spawn(crate::booking::run_recovery(
            ledger.clone(),
            pool.clone(),
            cfg.tenant.clone(),
            std::time::Duration::from_secs(30),
            ct.clone(),
        ));
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
            .map(|s| crate::ledger::iban_hash_key(secrecy::ExposeSecret::expose_secret(s)));
        if iban_hash_key.is_none() {
            tracing::warn!(
                "[WARN] iban_hash_secret unset -- IBAN lookup hash is unkeyed (dev mode)"
            );
        }

        // Every `/api/v1` route (`openapi::api_router`), each in the document
        // `GET /api/v1/openapi.json` serves.
        let (documented, doc) = crate::openapi::api_router();
        let app = Router::new()
            .merge(documented)
            .merge(mako_service::openapi::serve(doc))
            // ── Readiness and operator posture: GET /admin/einrichtung (Cedar
            // `read-config`) and GET /api/v1/policy (Cedar `read-policy`) ──────
            .merge(mako_service::einrichtung::router(Arc::clone(
                &ctx.einrichtung,
            )))
            // ── CloudEvent ingest ──────────────────────────────────────────────────
            .route("/webhook", post(handlers::ingest_webhook))
            // Not `/metrics`: `mako_service::run` mounts that itself, and
            // merging two routers that both claim `GET /metrics` panics while
            // the router is assembled — i.e. at startup. The ledger gauges get
            // their own path, the way `invoicd` and `processd` already do.
            .route("/accountingd/metrics", get(handlers::metrics))
            .layer(Extension(Arc::clone(&cfg)))
            .layer(Extension(Arc::clone(&cedar)))
            .layer(Extension(pool.clone()))
            .layer(Extension(Arc::clone(&ledger)))
            .layer(Extension(iban_hash_key))
            // OIDC verifier extension — enables Claims extractor on write endpoints
            .layer(Extension(mako_service::oidc::ExpectedTenant(
                cfg.tenant.clone(),
            )))
            .layer(Extension(oidc.clone()));

        // ── MCP server ────────────────────────────────────────────────────────────
        // Gated by the same verifier and the same policy as the REST surface.
        // This surface is not read-only: five of its fourteen tools post a
        // Buchung, raise the month's Abschlagsforderungen, book a CAMT.054
        // payment, rewrite a customer's advance, or emit a bank-submittable
        // pain.008 — so `mcp_server::tool_action` maps every tool to the Cedar
        // action its REST twin enforces, and an unmapped tool is refused.
        let mcp_state = std::sync::Arc::new(mcp_server::AccountingdMcpState {
            pool: pool.clone(),
            ledger: Arc::clone(&ledger),
            tenant: cfg.tenant.clone(),
            lf_mp_id: cfg.lf_mp_id.clone(),
            eeg_nb_mp_id: cfg.eeg.nb_mp_id().map(str::to_owned),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
                &cfg.mcp,
                oidc,
                Some(Arc::clone(&cedar)),
                &cfg.tenant,
            ),
            creditor_iban: cfg.creditor_iban.clone(),
            creditor_name: cfg.creditor_name().map(str::to_owned),
            creditor_id: cfg.creditor_id.clone(),
            creditor_address: cfg.creditor_address.clone(),
            iban_key: iban_hash_key,
            pain008_schema,
            policy: Arc::new(ctx.einrichtung.policy.clone()),
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
                        crate::pg::try_worker_lock(&pool_bg, crate::pg::LOCK_ABSCHLAG).await
                    else {
                        tracing::debug!(
                            "accountingd: Abschlag worker — another replica holds the lock"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                        continue;
                    };
                    // The Abschlagstag is a German calendar day: `find_accounts_due`
                    // matches it against the day-of-month each account agreed to,
                    // and `periode` stamps the month the advance covers. The same
                    // cycle run through the MCP tool reads `heute()`, so both paths
                    // raise the same cohort for the same month.
                    let today = mako_fristen::heute();
                    let day_of_month = today.day() as i16;
                    match crate::pg::find_accounts_due(&pool_bg, &tenant_bg, day_of_month).await {
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
                                let raised = crate::pg::raise_abschlagsforderung(
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
                                // drifts across midnight. The CloudEvent id is
                                // the same key and `outbox::enqueue` is
                                // `ON CONFLICT DO NOTHING`, so a replay drops
                                // at the outbox.
                                let ce = mako_service::CloudEvent::new(
                                    mako_service::source("accountingd", &tenant_bg),
                                    mako_events::accounting::ABSCHLAG_POSTED,
                                    &acct.malo_id,
                                    serde_json::json!({
                                        "malo_id":      acct.malo_id,
                                        "lf_mp_id":     acct.mp_id,
                                        "amount_ct":    acct.abschlag_ct,
                                        "amount_eur":   crate::handlers::format_ct_as_eur(acct.abschlag_ct),
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
                    crate::pg::release_worker_lock(&mut wlock, crate::pg::LOCK_ABSCHLAG).await;
                    // Sleep ~24h; use 23h to drift-proof against DST transitions.
                    tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                }
            });
        }

        // ── SEPA pre-notification scheduler ─────────────────────────────────────
        // Runs daily, finds the accounts whose `billing_day` falls
        // `sepa_pre_notification_days` ahead, builds one pain.008 for that
        // collection date and announces it as `de.mako.accounting.payment.due` — the
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
                    // Serialised across replicas, like the Abschlag, Dunning and
                    // Jahresabschluss workers. The dispatch claim below keeps the
                    // *event* from going out twice; the lock keeps two replicas
                    // from racing over `sepa_collection_runs`/`_entries`, the rows
                    // a bank reply is attributed to.
                    let Some(mut wlock) =
                        crate::pg::try_worker_lock(&pool_sepa, crate::pg::LOCK_SEPA_N5).await
                    else {
                        tracing::debug!(
                            "accountingd: SEPA N-5 worker — another replica holds the lock"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                        continue;
                    };
                    let today = mako_fristen::heute();
                    // Wraps correctly across month end.
                    let target_date = today + time::Duration::days(lead_days);
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
                                "accountingd: SEPA — generating pain.008 pre-notifications"
                            );

                            // Build one pain.008 message — one PmtInf group per
                            // SequenceType (SEPA Rulebook §3.8), one audit row.
                            // hard error if creditor_iban is missing/invalid — skip run with error log.
                            let entries: Vec<(&crate::pg::SepaMandateRow, i64)> =
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
                                    crate::pg::release_worker_lock(
                                        &mut wlock,
                                        crate::pg::LOCK_SEPA_N5,
                                    )
                                    .await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600))
                                        .await;
                                    continue;
                                }
                            };
                            let Some(creditor_name) = cfg_sepa.creditor_name() else {
                                tracing::error!(
                                    "accountingd: SEPA N-5 — {}; pain.008 generation BLOCKED.",
                                    crate::config::CREDITOR_NAME_FEHLT
                                );
                                crate::pg::release_worker_lock(&mut wlock, crate::pg::LOCK_SEPA_N5)
                                    .await;
                                tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600))
                                    .await;
                                continue;
                            };
                            let Some(creditor_id) =
                                cfg_sepa.creditor_id.as_deref().filter(|s| !s.is_empty())
                            else {
                                tracing::error!(
                                    "accountingd: SEPA N-5 — creditor_id (Gläubiger-ID) not \
                                 configured; the EPC rulebook mandates CdtrSchmeId. \
                                 pain.008 generation BLOCKED."
                                );
                                crate::pg::release_worker_lock(&mut wlock, crate::pg::LOCK_SEPA_N5)
                                    .await;
                                tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600))
                                    .await;
                                continue;
                            };

                            let creditor = crate::sepa::CreditorIdentity {
                                iban: creditor_iban,
                                name: creditor_name,
                                creditor_id,
                                address: Some(&cfg_sepa.creditor_address),
                            };
                            let run = match crate::sepa::build_pain_008(
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
                                    crate::pg::release_worker_lock(
                                        &mut wlock,
                                        crate::pg::LOCK_SEPA_N5,
                                    )
                                    .await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600))
                                        .await;
                                    continue;
                                }
                            };

                            {
                                let batch = &run;
                                // Persist the single pain.008 message for audit and ERP
                                // replay — exactly one row per (tenant, collection_date).
                                let run_id = match crate::pg::persist_sepa_collection(
                                    &pool_sepa,
                                    &cfg_sepa.tenant,
                                    target_date,
                                    batch,
                                )
                                .await
                                {
                                    Ok(id) => Some(id),
                                    Err(e)
                                        if e.downcast_ref::<crate::pg::SepaRunAlreadyDispatched>()
                                            .is_some() =>
                                    {
                                        // Expected on a same-day restart: the
                                        // day's pain.008 is already with the
                                        // bank and its rows are the record of
                                        // what was submitted.
                                        tracing::debug!(due_date = %target_date, "accountingd: SEPA — run already dispatched, nothing to rebuild");
                                        None
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "accountingd: SEPA — failed to persist sepa_collection_run");
                                        None
                                    }
                                };

                                // Claim the run for dispatch AND persist the CE atomically.
                                // Persist-before-dispatch fixes the pre-flip bug: the
                                // DISPATCHED flag flip and the `de.mako.accounting.payment.due`
                                // outbox enqueue commit in ONE transaction, so the event is
                                // durable *exactly when* the run is marked dispatched. A
                                // replica or same-day restart still sees DISPATCHED and does
                                // not re-enqueue (no double collection); the outbox worker
                                // then signs and delivers with retry/dead-letter.
                                if let Some(id) = run_id {
                                    match pool_sepa.begin().await {
                                        Ok(mut tx) => {
                                            // `Ok(false)` is the idempotency guard —
                                            // another replica or an earlier run of the
                                            // same day already claimed it. An `Err` is
                                            // not that: collapsing the two commits an
                                            // empty transaction, and because the next
                                            // cycle computes a different `target_date`
                                            // this day's pain.008 would never be
                                            // retried. Roll back instead, so the run
                                            // stays unclaimed and the next cycle can
                                            // take it.
                                            let mut commit = true;
                                            let claimed =
                                                match crate::pg::mark_sepa_collection_dispatched(
                                                    &mut *tx, id,
                                                )
                                                .await
                                                {
                                                    Ok(claimed) => claimed,
                                                    Err(e) => {
                                                        tracing::error!(
                                                            error = %e,
                                                            due_date = %target_date,
                                                            "accountingd: SEPA N-5 — claiming the run failed; rolling back so the run stays unclaimed"
                                                        );
                                                        commit = false;
                                                        false
                                                    }
                                                };
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

                    crate::pg::release_worker_lock(&mut wlock, crate::pg::LOCK_SEPA_N5).await;
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
                Arc::new(crate::clients::OutputdClient::new(
                    out,
                    cfg.outputd_api_key
                        .as_ref()
                        .map(|k| secrecy::ExposeSecret::expose_secret(k).to_owned()),
                )),
                Arc::new(crate::clients::VertragdClient::new(
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
                     de.mako.accounting.mahnung.issued CloudEvent.",
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
                        crate::pg::try_worker_lock(&pool_dun, crate::pg::LOCK_DUNNING).await
                    else {
                        tracing::debug!(
                            "accountingd: dunning worker — another replica holds the lock"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                        continue;
                    };
                    let grace_days = cfg_dun.dunning_grace_days.unwrap_or(30);
                    // `validate` refused a start without all six; a run
                    // without them is skipped rather than defaulted.
                    let [(_, f1), (_, f2), (_, f3)] = cfg_dun.zahlungsfristen();
                    let [(_, g1), (_, g2), (_, g3)] = cfg_dun.mahngebuehren();
                    let run = match (f1, f2, f3, g1, g2, g3) {
                        (Some(f1), Some(f2), Some(f3), Some(g1), Some(g2), Some(g3)) => {
                            let stufen = crate::pg::Mahnstufen {
                                gebuehr_ct: [g1, g2, g3],
                                zahlungsfrist_tage: [f1, f2, f3],
                            };
                            crate::pg::run_auto_dunning(
                                &ledger_dun,
                                &pool_dun,
                                &cfg_dun.tenant,
                                grace_days,
                                &stufen,
                            )
                            .await
                        }
                        _ => Err(anyhow::anyhow!(
                            "dunning Zahlungsfristen and Mahngebühren are not all set"
                        )),
                    };
                    match run {
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
                        match crate::mahnung::issue_pending(
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

                    // Instalment plans: match the cash booked since each plan
                    // began, then default a plan with an instalment past due —
                    // before the sequence below, which an ACTIVE plan holds back
                    // and a broken Abwendungsvereinbarung re-opens.
                    match crate::pg::run_payment_plans(&pool_dun, &cfg_dun.tenant).await {
                        Ok(s) if s.paid + s.completed + s.defaulted > 0 => {
                            tracing::info!(
                                paid = s.paid,
                                completed = s.completed,
                                defaulted = s.defaulted,
                                vereinbarungen_gebrochen = s.vereinbarungen_gebrochen,
                                "accountingd: payment plans matched"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "accountingd: payment-plan run failed");
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
                        match crate::sperr::run_sperr_sequence(&pool_dun, makod_dun, &cfg_dun).await
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

                    crate::pg::release_worker_lock(&mut wlock, crate::pg::LOCK_DUNNING).await;
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
                    let Some(mut wlock) =
                        crate::pg::try_worker_lock(&pool_ja, crate::pg::LOCK_JAHRESABSCHLUSS).await
                    else {
                        tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
                        continue;
                    };
                    let today = mako_fristen::heute();
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
                    crate::pg::release_worker_lock(&mut wlock, crate::pg::LOCK_JAHRESABSCHLUSS)
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
    ledger: &Arc<crate::ledger::PgLedger>,
    cfg: &Arc<crate::config::AccountingdConfig>,
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
    let candidates = match crate::pg::list_jahresabschluss_candidates(
        pool,
        &cfg.tenant,
        year_i16,
        BATCH,
    )
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
        let q = crate::handlers::JahresabschlussQuery {
            lf_mp_id: Some(lf_mp_id.clone()),
            year: Some(year),
            dry_run: Some(false),
        };
        match crate::handlers::settle_jahresabschluss(pool, ledger, cfg, malo_id, &q).await {
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
