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
            let prelim_day = cfg.schedule.preliminary_day;
            let final_day = cfg.schedule.final_day;
            let run_hour = cfg.schedule.run_hour_utc;

            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(300)).await; // check every 5 min
                let now = time::OffsetDateTime::now_utc();
                let day = now.day();
                let hour = now.hour();

                if hour == run_hour {
                    if day == prelim_day {
                        let (from, to) =
                            mabis_syncd::sync_engine::previous_month_period(now.date());
                        match engine.run_aggregation(from, to, "vorlaeufig").await {
                            Ok(id) => {
                                tracing::info!(run_id = %id, "mabis-syncd: scheduled vorlaeufig run completed")
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "mabis-syncd: scheduled vorlaeufig run failed")
                            }
                        }
                    } else if day == final_day {
                        let (from, to) =
                            mabis_syncd::sync_engine::previous_month_period(now.date());
                        match engine.run_aggregation(from, to, "endgueltig").await {
                            Ok(id) => {
                                tracing::info!(run_id = %id, "mabis-syncd: scheduled endgueltig run completed")
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "mabis-syncd: scheduled endgueltig run failed")
                            }
                        }
                    }
                }
            }
        });
    }

    // HTTP server
    let state = server::ServerState {
        pool,
        engine,
        cfg: cfg.clone(),
    };
    let router = server::router(state);
    let addr: std::net::SocketAddr = cfg.http.addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(addr = %addr, "mabis-syncd HTTP server ready");
    axum::serve(listener, router).await?;
    Ok(())
}
