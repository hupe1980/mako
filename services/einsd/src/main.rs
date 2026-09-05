//! `einsd` — Einspeiser Registry + EEG Settlement daemon.
//!
//! Manages the lifecycle of decentralised feed-in plants (Einspeiseanlagen)
//! under the EEG (Erneuerbare-Energien-Gesetz) and calculates their monthly
//! feed-in remuneration according to the applicable settlement model:
//!
//! | Model | Regulation | Flow |
//! |---|---|---|
//! | `VERGUETUNG` | §21 EEG 2023 | Fixed tariff NB → Anlagenbetreiber |
//! | `DIREKTVERMARKTUNG` | §20 EEG 2023 | Marktprämie (max(0, AW−EPEX)) NB → ÜNB |
//! | `POST_EEG_SPOT` | post-20yr | Spot market reference value |
//! | `EIGENVERBRAUCH` | §21 Abs. 3 EEG | Self-consumption; no settlement |
//!
//! Emits CloudEvents:
//! - `de.eeg.verguetung.berechnet` — VERGUETUNG/POST_EEG_SPOT/EIGENVERBRAUCH settled
//! - `de.eeg.marktpraemie.berechnet` — DIREKTVERMARKTUNG settled
//! - `de.eeg.anlage.foerderung-auslaufend` — `foerderendedatum` within 180 days
//!
//! Port: `:9180`
//!
//! # Endpoints
//!
//! | Method   | Path | Description |
//! |---|---|---|
//! | `POST`   | `/api/v1/anlagen` | Register EEG plant |
//! | `GET`    | `/api/v1/anlagen` | List plants (`?erzeugungsart=&settlement_model=&status=`) |
//! | `GET`    | `/api/v1/anlagen/{tr_id}` | Fetch plant |
//! | `PUT`    | `/api/v1/anlagen/{tr_id}` | Update plant |
//! | `DELETE` | `/api/v1/anlagen/{tr_id}` | Decommission plant |
//! | `GET`    | `/api/v1/anlagen/foerderung-auslaufend` | Plants expiring within 180 days |
//! | `POST`   | `/api/v1/anlagen/{tr_id}/settle/{year}/{month}` | Trigger monthly settlement |
//! | `GET`    | `/api/v1/anlagen/{tr_id}/settlements` | Settlement history |
//! | `GET`    | `/api/v1/anlagen/{tr_id}/pflichtverstoesse` | §52 Abs. 1 breaches recorded against the plant |
//! | `POST`   | `/api/v1/anlagen/{tr_id}/pflichtverstoesse` | Record one — the only path for the nine Nummern einsd cannot derive |
//! | `PUT`    | `/api/v1/anlagen/{tr_id}/pflichtverstoesse/{typ}/behoben` | Record the cure (§52 Abs. 3 Satz 1 Nr. 1) |
//! | `PUT`    | `/api/v1/epex-monthly/{year}/{month}` | Import EPEX monthly price |
//! | `GET`    | `/api/v1/epex-monthly/{year}/{month}` | Fetch stored EPEX price |
//! | `PUT`    | `/api/v1/marktwert/{year}/{art}/{erzeugungsart}` | Import an Anlage 1 Nr. 3/4 Marktwert (`art` = `monat` \| `jahr`) |
//! | `GET`    | `/api/v1/marktwert/{year}/{art}/{erzeugungsart}` | Fetch a stored Marktwert |
//! | `GET`    | `/api/v1/marktwert/{year}/nachbewertung` | Months settled on a provisional Jahresmarktwert |
//! | `GET`    | `/health` | Liveness check |
//! | `GET`    | `/health/ready` | Readiness check |
//!
//! `mako_service::run` owns the lifecycle (tracing, tuned pool, real DB-ping
//! readiness, graceful shutdown); this only supplies the migrations, the domain
//! router and the background workers.

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;
use einsd::{config, handlers, mcp_server, pg};
use mako_service::{Daemon, ServiceContext};
use sqlx::PgPool;

struct Einsd;

impl Daemon for Einsd {
    type Config = config::EinsdConfig;
    const NAME: &'static str = "einsd";

    async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run einsd migrations")?;
        // Transactional outbox: settlement CloudEvents are enqueued in the same
        // tx as the settlement write, then drained with at-least-once delivery.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure event_outbox schema")?;
        Ok(())
    }

    async fn build(cfg: Arc<config::EinsdConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        // Settling a plant creates a payment obligation to the Anlagenbetreiber, so
        // the API is closed by default; running it open has to be stated explicitly.
        if cfg.oidc.is_none() && !cfg.allow_insecure_no_auth {
            anyhow::bail!(
                "einsd: no [oidc] section and allow_insecure_no_auth is not set — \
                 refusing to serve the settlement API unauthenticated"
            );
        }

        // Shared HTTP client from the runner, wrapped in `Arc` for the workers and
        // MCP state that hold onto it.
        let http_client: Arc<reqwest::Client> = Arc::new(ctx.http.clone());
        let pool = ctx.pool().clone();
        let ct = ctx.shutdown.clone();

        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &http_client,
            &cfg.tenant,
            ct.clone(),
        )
        .await?;

        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/einsd.cedar"
            ))
            .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
        );

        let mcp_state = std::sync::Arc::new(mcp_server::EinsdMcpState {
            pool: pool.clone(),
            tenant: cfg.tenant.clone(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
                &cfg.mcp,
                oidc.clone(),
                Some(cedar.clone()),
                &cfg.tenant,
            ),
            cfg: Arc::clone(&cfg),
            http_client: Arc::clone(&http_client),
        });

        // Transactional outbox drain worker — spawned when an ERP webhook is configured.
        if let Some(webhook_url) = cfg.erp_webhook_url.clone() {
            let worker = mako_service::outbox::OutboxWorker::new(
                pool.clone(),
                webhook_url,
                cfg.erp_hmac_secret.clone().map(Into::into),
            );
            tokio::spawn(worker.run(ct.clone()));
        }

        // Background worker: `de.eeg.anlage.foerderung-auslaufend`.
        //
        // The alert is emitted **once per plant**, not once per sweep. The window
        // is 180 days wide and the sweep ran every six hours, so every expiring
        // plant produced ~720 identical CloudEvents — enough to bury the one
        // event an operator was supposed to act on. `foerderung_alert_sent_at`
        // records the emission and the query excludes plants that already have
        // one; a repowering clears it, because the new Förderende is a new fact.
        let alert_pool = pool.clone();
        let alert_cfg = Arc::clone(&cfg);
        let alert_client = Arc::clone(&http_client);
        tokio::spawn(async move {
            let interval_secs = alert_cfg.alert_interval_secs.unwrap_or(21_600);
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
                match pg::list_expiring_unalerted(&alert_pool, &alert_cfg.tenant, 180).await {
                    Ok(plants) if !plants.is_empty() => {
                        let today = mako_fristen::heute();
                        for plant in &plants {
                            // The query selects on a BETWEEN, so a plant with no
                            // calendar Förderende is never in this list.
                            let Some(foerderende) = plant.foerderendedatum else {
                                continue;
                            };
                            let days_remaining = (foerderende - today).whole_days();
                            tracing::info!(
                                tr_id = %plant.tr_id,
                                foerderendedatum = %foerderende,
                                days_remaining,
                                "foerderung_auslaufend — emitting CloudEvent"
                            );
                            handlers::emit_foerderung_alert_ce(
                                &alert_cfg,
                                &alert_client,
                                &plant.tr_id,
                                &plant.malo_id,
                                foerderende,
                                days_remaining,
                            )
                            .await;
                            // Marked after a successful-or-failed delivery attempt:
                            // the alert is advisory and `GET /foerderung-auslaufend`
                            // still lists the plant, so a retry storm is the worse
                            // failure of the two.
                            if let Err(e) = pg::mark_foerderung_alert_sent(
                                &alert_pool,
                                &alert_cfg.tenant,
                                &plant.tr_id,
                            )
                            .await
                            {
                                tracing::warn!(tr_id = %plant.tr_id, error = %e,
                                    "alert worker: could not record the emission");
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("alert worker error: {e}"),
                }
            }
        });

        // Background worker: auto-settle any active plant with no successful
        // receipt for a recent month.
        //
        // Runs on a fixed ~23 h interval but only from `auto_settle_from_day`
        // onwards: the ÜNB publishes the Marktwert around the 5th and edmd's
        // month is not complete before then, so an earlier run only produced
        // `price_missing` / `no_data` receipts. Settling is idempotent per
        // (plant, period), so a plant already settled is skipped rather than
        // rebilled — and one that was not is retried.
        //
        // It sweeps `auto_settle_catchup_months` periods back, newest first, not
        // only the previous month. A month the service was down for, or whose
        // Marktwert arrived late, was otherwise never revisited: the window moved
        // on and the plant simply went unpaid, with nothing in the service that
        // would ever notice. §23 EEG 2023 makes the monthly payment the NB's
        // obligation, so silently skipping one is not an option.
        let auto_pool = pool.clone();
        let auto_cfg = Arc::clone(&cfg);
        let auto_client = Arc::clone(&http_client);
        tokio::spawn(async move {
            let from_day = auto_cfg.auto_settle_from_day.unwrap_or(7);
            let catchup = auto_cfg
                .auto_settle_catchup_months
                .unwrap_or(3)
                .clamp(1, 24);
            // Wait for startup before first run.
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            loop {
                // The ÜNB Marktwert window opens on a German calendar day, so
                // the day-of-month gate and the settlement period both read the
                // Berlin date.
                let today = mako_fristen::heute();
                if today.day() < from_day {
                    tracing::debug!(
                        day = today.day(),
                        from_day,
                        "auto-settle worker: waiting for the ÜNB Marktwert window"
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(82_800)).await;
                    continue;
                }
                for back in 1..=i32::from(catchup) {
                    let (year, month) = month_offset(today.year(), today.month() as i32, -back);
                    auto_settle_period(&auto_pool, &auto_cfg, &auto_client, year, month).await;
                }

                // Run again in ~23 h (drift-proof; avoids DST edge at midnight).
                tokio::time::sleep(tokio::time::Duration::from_secs(82_800)).await;
            }
        });

        // Background worker: auto-import Anlage 1 Nr. 3/4 EEG 2023 technology-specific
        // Jahresmarktwert from ÜNB publication (netztransparenz.de or custom aggregator).
        //
        // Runs once on startup (after 60s delay) and then every `jahresmarktwert_import_interval_secs`
        // (default 86400, once per day). The ÜNB publishes monthly values typically by the 5th of
        // each month. For MarketPremium (Direktvermarktung / Ausschreibung) settlements to use the
        // correct technology-specific AW, these values must be available before monthly settlement runs.
        //
        // The external URL must return JSON with the structure:
        //   `[{ "erzeugungsart": "WIND_ONSHORE", "avg_ct_kwh": "6.42" }, ...]`
        // where `erzeugungsart` matches the values in the `eeg_anlagen` table.
        if let Some(jmw_url_tpl) = cfg.jahresmarktwert_url.clone() {
            let jmw_pool = pool.clone();
            let jmw_client = Arc::clone(&http_client);
            let jmw_interval = cfg.jahresmarktwert_import_interval_secs.unwrap_or(86_400);
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                loop {
                    // Fetch for the previous month (published by ÜNB after month
                    // close). The month is a German calendar month.
                    let today = mako_fristen::heute();
                    let (year, month) = if today.month() as u8 == 1 {
                        (today.year() as i16 - 1, 12i16)
                    } else {
                        (today.year() as i16, today.month() as i16 - 1)
                    };

                    let url = jmw_url_tpl
                        .replace("{year}", &format!("{year:04}"))
                        .replace("{month}", &format!("{month:02}"));

                    tracing::debug!(url = %url, year, month, "auto-importing Jahresmarktwert");

                    match jmw_client.get(&url).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            if let Ok(items) = resp.json::<Vec<serde_json::Value>>().await {
                                for item in &items {
                                    let art = item
                                        .get("erzeugungsart")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("DEFAULT");
                                    // Parsed from the JSON number's own text, not
                                    // through f64: a Marktwert is money and 6.42
                                    // has no exact binary representation.
                                    let avg_ct = item
                                        .get("avg_ct_kwh")
                                        .and_then(|v| match v {
                                            serde_json::Value::Number(n) => {
                                                n.to_string().parse().ok()
                                            }
                                            serde_json::Value::String(s) => s.parse().ok(),
                                            _ => None,
                                        })
                                        .map(|d: rust_decimal::Decimal| d);
                                    // The ÜNB feed is the **monthly** series
                                    // (Anlage 1 Nr. 3), which is what this URL
                                    // template addresses. The Jahresmarktwert has
                                    // no month and is imported by hand or by the
                                    // operator's own job, because its binding
                                    // figure lands once a year.
                                    if let Some(ct) = avg_ct
                                        && let Err(e) = pg::upsert_marktwert(
                                            &jmw_pool,
                                            pg::MarktwertImport {
                                                year,
                                                serie: eeg_billing::Marktwertserie::Monatsmarktwert,
                                                month: Some(month),
                                                erzeugungsart: art,
                                                avg_ct_kwh: ct,
                                                vorlaeufig: false,
                                                source: "auto-import",
                                            },
                                        )
                                        .await
                                    {
                                        tracing::warn!(year, month, art, error = %e,
                                            "Jahresmarktwert import: upsert failed");
                                    }
                                }
                                tracing::info!(
                                    year,
                                    month,
                                    count = items.len(),
                                    "Anlage 1 Marktwert auto-imported from ÜNB"
                                );
                            }
                        }
                        Ok(resp) => {
                            tracing::warn!(
                                url = %url, status = %resp.status(),
                                "Jahresmarktwert auto-import: non-2xx response"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Jahresmarktwert auto-import: HTTP error");
                        }
                    }

                    tokio::time::sleep(tokio::time::Duration::from_secs(jmw_interval)).await;
                }
            });
        }

        Ok(einsd::routes::build_router(
            Arc::clone(&cfg),
            Arc::clone(&http_client),
            cedar,
            oidc,
            pool,
            mcp_state,
            ct,
        ))
    }
}

/// Shift `(year, month)` by `delta` months, wrapping the year.
fn month_offset(year: i32, month: i32, delta: i32) -> (i16, i16) {
    let zero_based = (year * 12 + (month - 1)) + delta;
    (
        i16::try_from(zero_based.div_euclid(12)).unwrap_or(i16::MAX),
        i16::try_from(zero_based.rem_euclid(12) + 1).unwrap_or(1),
    )
}

/// Settle every active plant that has no successful receipt for one period.
///
/// Each plant commits on its own transaction — the settlement write and its ERP
/// CloudEvent together — so one plant's failure cannot roll back the batch.
async fn auto_settle_period(
    pool: &PgPool,
    cfg: &config::EinsdConfig,
    client: &reqwest::Client,
    year: i16,
    month: i16,
) {
    // A missing price and a failed lookup are different problems — one waits for
    // an import, the other for an operator — so they are logged apart.
    let epex = match pg::fetch_epex_price(pool, year, month).await {
        Ok(price) => {
            if price.is_none() {
                tracing::info!(
                    year,
                    month,
                    "auto-settle worker: no EPEX monthly price imported yet"
                );
            }
            price
        }
        Err(e) => {
            tracing::warn!(
                year, month, error = %e,
                "auto-settle worker: EPEX monthly price lookup failed"
            );
            None
        }
    };

    let plants = match pg::list_unsettled(pool, &cfg.tenant, year, month).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(year, month, error = %e, "auto-settle worker: unsettled lookup failed");
            return;
        }
    };
    if plants.is_empty() {
        return;
    }

    tracing::info!(
        year,
        month,
        unsettled = plants.len(),
        "auto-settle worker: settling unsettled plants"
    );
    // Signal the batch to agentd's einsd-batch-agent (§52 sweep / review).
    handlers::emit_batch_due_ce(cfg, client, year, month, plants.len()).await;

    for anlage in &plants {
        // The same path REST, batch and MCP take: the worker overrides only the
        // market price it already resolved for the whole batch.
        if let Err(e) = einsd::settle::settle_plant(
            pool,
            cfg,
            client,
            anlage,
            year,
            month,
            einsd::settle::SettleRequest {
                epex_avg_ct_kwh: epex,
                ..Default::default()
            },
        )
        .await
        {
            tracing::warn!(tr_id = %anlage.tr_id, error = %e, "auto-settle: settlement failed");
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Einsd>().await
}

#[cfg(test)]
mod month_offset_tests {
    use super::month_offset;

    /// The catch-up sweep walks backwards across a year boundary every January.
    #[test]
    fn stepping_back_wraps_the_year() {
        assert_eq!(month_offset(2026, 3, -1), (2026, 2));
        assert_eq!(month_offset(2026, 1, -1), (2025, 12));
        assert_eq!(month_offset(2026, 1, -3), (2025, 10));
        assert_eq!(month_offset(2026, 2, -14), (2024, 12));
    }
}
