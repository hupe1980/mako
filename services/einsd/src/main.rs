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
//! - `de.eeg.anlage.foerderung_auslaufend` — `foerderendedatum` within 180 days
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
//! | `PUT`    | `/api/v1/epex-monthly/{year}/{month}` | Import EPEX monthly price |
//! | `GET`    | `/api/v1/epex-monthly/{year}/{month}` | Fetch stored EPEX price |
//! | `PUT`    | `/api/v1/jahresmarktwert/{year}/{month}/{erzeugungsart}` | Import §20 Abs. 2 technology-specific Marktwert (ÜNB) |
//! | `GET`    | `/api/v1/jahresmarktwert/{year}/{month}/{erzeugungsart}` | Fetch stored Jahresmarktwert |
//! | `GET`    | `/health` | Liveness check |
//! | `GET`    | `/health/ready` | Readiness check |

use anyhow::Context as _;
use einsd::{config, handlers, mcp_server, pg};
use mako_service::load_config;
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = mako_service::init_tracing_from_env("einsd");

    let cfg: config::EinsdConfig = load_config("einsd").context("load config")?;
    let cfg = Arc::new(cfg);

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    // Shared HTTP client — initialized once, reused for all outbound calls.
    // Never create per-request clients; that wastes connection pool slots.
    let http_client: Arc<reqwest::Client> = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("build HTTP client")?,
    );

    let ct = mako_service::shutdown::token();

    // Settling a plant creates a payment obligation to the Anlagenbetreiber, so
    // the API is closed by default; running it open has to be stated explicitly.
    if cfg.oidc.is_none() && !cfg.allow_insecure_no_auth {
        anyhow::bail!(
            "einsd: no [oidc] section and allow_insecure_no_auth is not set — \
             refusing to serve the settlement API unauthenticated"
        );
    }
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

    // Run database migrations at startup.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("run einsd migrations")?;

    // Background worker: emit de.eeg.anlage.foerderung_auslaufend every 6 h.
    let alert_pool = pool.clone();
    let alert_cfg = Arc::clone(&cfg);
    let alert_client = Arc::clone(&http_client);
    tokio::spawn(async move {
        let interval_secs = alert_cfg.alert_interval_secs.unwrap_or(21_600);
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
            match pg::list_expiring(&alert_pool, &alert_cfg.tenant, 180).await {
                Ok(plants) if !plants.is_empty() => {
                    let today = time::OffsetDateTime::now_utc().date();
                    for plant in &plants {
                        let days_remaining = (plant.foerderendedatum - today).whole_days();
                        tracing::info!(
                            tr_id = %plant.tr_id,
                            foerderendedatum = %plant.foerderendedatum,
                            days_remaining,
                            "foerderung_auslaufend — emitting CloudEvent"
                        );
                        handlers::emit_foerderung_alert_ce(
                            &alert_cfg,
                            &alert_client,
                            &plant.tr_id,
                            &plant.malo_id,
                            plant.foerderendedatum,
                            days_remaining,
                        )
                        .await;
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::error!("alert worker error: {e}"),
            }
        }
    });

    // Background worker: auto-settle any active plant with no receipt for the
    // previous month.
    //
    // Runs once 60 s after startup and then on a fixed ~23 h interval — it does
    // not wait for a particular day of the month. Settling is idempotent per
    // (plant, period), so a plant already settled is skipped rather than rebilled.
    //
    // EEG Vergütung must be paid monthly per §23 EEG 2023. The NB is responsible
    // for initiating payment within 30 days of the billing month end.
    //
    // EEG Vergütung must be paid monthly per §23 EEG 2023.  The NB is responsible
    // for initiating payment within 30 days of the billing month end.
    let auto_pool = pool.clone();
    let auto_cfg = Arc::clone(&cfg);
    let auto_client = Arc::clone(&http_client);
    tokio::spawn(async move {
        // Wait for startup before first run.
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        loop {
            let now = time::OffsetDateTime::now_utc();
            // Settle the previous calendar month.
            let prev_month_year = if now.month() as u8 == 1 {
                now.year() - 1
            } else {
                now.year()
            };
            let prev_month = if now.month() as u8 == 1 {
                12i16
            } else {
                now.month() as i16 - 1
            };

            // Resolve EPEX price for previous month.
            let epex = pg::fetch_epex_price(&auto_pool, prev_month_year as i16, prev_month)
                .await
                .ok()
                .flatten();

            let plants = pg::list_unsettled(
                &auto_pool,
                &auto_cfg.tenant,
                prev_month_year as i16,
                prev_month,
            )
            .await
            .unwrap_or_default();

            if !plants.is_empty() {
                tracing::info!(
                    year = prev_month_year,
                    month = prev_month,
                    unsettled = plants.len(),
                    "auto-settle worker: settling unsettled plants"
                );
                for anlage in &plants {
                    let kwh = handlers::fetch_einspeisemenge_from_edmd(
                        &auto_cfg,
                        &auto_client,
                        &anlage.malo_id,
                        prev_month_year as i16,
                        prev_month,
                    )
                    .await;
                    let input = einsd::pg::build_settle_input(
                        &auto_cfg.tenant,
                        anlage,
                        prev_month_year as i16,
                        prev_month,
                        einsd::pg::SettleOverrides {
                            einspeisemenge_kwh: kwh,
                            epex_avg_ct_kwh: epex,
                            managementpraemie_ct_override: None,
                            einspeisemanagement_kwh: None,
                            negative_price_quarter_hours: None,
                            correction_of: None,
                            correction_reason: None,
                            jahresmarktwert_ct_kwh: None,
                        },
                    );
                    if let Ok(result) = pg::run_settlement(&auto_pool, input).await
                        && (result.status == "calculated" || result.status == "foerderung_beendet")
                    {
                        let ce_type = match anlage.settlement_model.as_str() {
                            "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG" => {
                                "de.eeg.marktpraemie.berechnet"
                            }
                            _ => "de.eeg.verguetung.berechnet",
                        };
                        handlers::emit_settlement_ce(
                            &auto_cfg,
                            &auto_client,
                            ce_type,
                            &anlage.tr_id,
                            &anlage.malo_id,
                            &result,
                            prev_month_year as i16,
                            prev_month,
                            anlage.bank_iban.as_deref(),
                            anlage.bank_bic.as_deref(),
                            anlage.zahlungsempfaenger.as_deref(),
                        )
                        .await;
                    }
                }
            }

            // Run again in ~23 h (drift-proof; avoids DST edge at midnight).
            tokio::time::sleep(tokio::time::Duration::from_secs(82_800)).await;
        }
    });

    // Background worker: auto-import §20 Abs. 2 + Anlage 1 EEG 2023 technology-specific
    // Jahresmarktwert from ÜNB publication (netztransparenz.de or custom aggregator).
    //
    // Runs once on startup (after 60s delay) and then every `jahresmarktwert_import_interval_secs`
    // (default 86400, once per day). The ÜNB publishes monthly values typically by the 5th of
    // each month. For MarketPremium (Direktvermarktung / Ausschreibung) settlements to use the
    // correct technology-specific AW, these values must be available before monthly settlement runs.
    //
    // The external URL must return JSON with the structure:
    //   `[{ "erzeugungsart": "WIND_ONSHORE", "avg_ct_kwh": 6.42 }, ...]`
    // where `erzeugungsart` matches the values in the `eeg_anlagen` table.
    if let Some(jmw_url_tpl) = cfg.jahresmarktwert_url.clone() {
        let jmw_pool = pool.clone();
        let jmw_client = Arc::clone(&http_client);
        let jmw_interval = cfg.jahresmarktwert_import_interval_secs.unwrap_or(86_400);
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            loop {
                let now = time::OffsetDateTime::now_utc();
                // Fetch for the previous month (published by ÜNB after month close).
                let (year, month) = if now.month() as u8 == 1 {
                    (now.year() as i16 - 1, 12i16)
                } else {
                    (now.year() as i16, now.month() as i16 - 1)
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
                                let avg_ct = item
                                    .get("avg_ct_kwh")
                                    .and_then(|v| v.as_f64())
                                    .and_then(|f| rust_decimal::Decimal::try_from(f).ok());
                                if let Some(ct) = avg_ct
                                    && let Err(e) = pg::upsert_jahresmarktwert(
                                        &jmw_pool,
                                        year,
                                        month,
                                        art,
                                        ct,
                                        "auto-import",
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
                                "§20 Abs. 2 Jahresmarktwert auto-imported from ÜNB"
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

    let app = einsd::routes::build_router(
        Arc::clone(&cfg),
        Arc::clone(&http_client),
        cedar,
        oidc,
        pool,
        mcp_state,
        ct.clone(),
    );

    let port = cfg.port.unwrap_or(9180);
    let addr = format!("0.0.0.0:{port}");
    info!(%addr, "einsd starting");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    mako_service::shutdown::serve(listener, app, ct).await
}
