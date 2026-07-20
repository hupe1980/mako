#![deny(unsafe_code)]

use std::sync::Arc;
use tracing::info;

fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    use mabis_syncd::{config, server, sync_engine::SyncEngine};

    // Load configuration
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mabis-syncd.toml".to_owned());
    let cfg = config::load_from_file(std::path::Path::new(&config_path))?;
    let cfg = Arc::new(cfg);

    // Telemetry
    let _tracing = mako_service::telemetry::init_tracing_from_env("mabis-syncd");

    // A MaBiS submission settles a balance group and cannot be withdrawn once
    // the BIKO acks it. Running the trigger route unauthenticated is therefore
    // refused unless the operator asked for it by name — an omitted `[oidc]`
    // section is a mistake, not a request to disable authentication.
    if cfg.oidc.is_none() && !cfg.allow_insecure_no_auth {
        anyhow::bail!(
            "no [oidc] section configured. POST /api/v1/sync files a binding \
             Summenzeitreihe with the BIKO, so it is not served without token \
             verification. Configure [oidc], or set allow_insecure_no_auth = true \
             to accept an unauthenticated deployment."
        );
    }
    if cfg.allow_insecure_no_auth {
        tracing::warn!(
            "mabis-syncd: allow_insecure_no_auth is set — every caller can file a \
             Summenzeitreihe with the BIKO in this tenant's name"
        );
    }

    info!(
        config = config_path,
        addr = cfg.http.addr,
        tenant = cfg.identity.tenant,
        bilanzierungsgebiet_id = cfg.identity.bilanzierungsgebiet_id,
        "mabis-syncd starting"
    );

    // Database
    let pool = sqlx::PgPool::connect(&cfg.database.url)
        .await
        .map_err(|e| anyhow::anyhow!("database connection failed: {e}"))?;
    sqlx::migrate!("src/migrations").run(&pool).await?;

    let engine = Arc::new(SyncEngine::new(pool.clone(), cfg.clone()));

    // Background scheduler
    {
        let engine = engine.clone();
        let cfg = cfg.clone();
        tokio::spawn(async move {
            // BK6-24-174 Anlage 3 §3.10, Tabelle 2: the Erstaufschlag window for
            // a BG-SZR runs to the 10. Werktag after the Bilanzierungsmonat.
            // Submitting on that last Werktag gives the aggregate the most
            // complete input while the BIKO still assigns 'Abrechnungsdaten'
            // automatically — a later version starts as 'Prüfdaten' and needs a
            // positive Prüfmitteilung to settle.
            let submit_wt = cfg.schedule.erstaufschlag_werktag;
            let run_hour = cfg.schedule.run_hour_utc;

            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(300)).await; // check every 5 min
                let now = time::OffsetDateTime::now_utc();
                if u32::from(now.hour()) != u32::from(run_hour) {
                    continue;
                }

                let (from, to) = mabis_syncd::sync_engine::previous_month_period(now.date());
                let due = mako_engine::fristen::add_werktage(
                    to,
                    submit_wt,
                    mako_engine::fristen::HolidayCalendar::BdewMaKo,
                );
                if now.date() != due {
                    continue;
                }

                match engine.run_aggregation(from, to, None, None).await {
                    Ok(id) => tracing::info!(
                        run_id = %id, werktag = submit_wt,
                        "mabis-syncd: scheduled Summenzeitreihe submission completed"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e,
                        "mabis-syncd: scheduled Summenzeitreihe submission failed"
                    ),
                }
            }
        });
    }

    // HTTP server
    let http = mako_service::http::default_client();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let oidc = mako_service::oidc::OidcConfig::build_verifier(
        cfg.oidc.as_ref(),
        &http,
        &cfg.identity.tenant,
        shutdown.clone(),
    )
    .await?;
    let cedar = Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
            "../policies/mabis-syncd.cedar"
        ))
        .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
    );

    let state = server::ServerState {
        pool,
        engine,
        cfg: cfg.clone(),
    };
    let router = server::router(state)
        .layer(axum::Extension(cedar))
        .layer(axum::Extension(oidc));
    let addr: std::net::SocketAddr = cfg.http.addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(addr = %addr, "mabis-syncd HTTP server ready");
    axum::serve(listener, router).await?;
    Ok(())
}
