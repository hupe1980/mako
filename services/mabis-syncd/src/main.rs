#![deny(unsafe_code)]

//! `mabis-syncd` — MaBiS Summenzeitreihe synchronisation daemon.
//!
//! `mako_service::run` owns the lifecycle (tracing, tuned pool, real DB-ping
//! readiness, graceful shutdown); this only supplies the migrations and the
//! domain router + scheduler worker.

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;
use mabis_syncd::{config, server, sync_engine::SyncEngine};
use mako_service::{Daemon, ServiceContext};
use tracing::info;

struct MabisSyncd;

impl Daemon for MabisSyncd {
    type Config = config::Config;
    const NAME: &'static str = "mabis-syncd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("src/migrations")
            .run(pool)
            .await
            .context("run mabis-syncd migrations")?;
        Ok(())
    }

    async fn build(cfg: Arc<config::Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
        // Resolve every `env:VARNAME` indirection (edmd/marktd/makod URLs + keys,
        // identity codes) before the config is shared with the engine and
        // handlers, which read those fields verbatim at request time.
        let mut cfg = (*cfg).clone();
        cfg.resolve_env_refs()?;
        let cfg = Arc::new(cfg);

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
            addr = cfg.http.addr,
            tenant = cfg.identity.tenant,
            bilanzierungsgebiet_id = cfg.identity.bilanzierungsgebiet_id,
            "mabis-syncd starting"
        );

        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &cfg.identity.tenant,
            ctx.shutdown.clone(),
        )
        .await?;
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/mabis-syncd.cedar"
            ))
            .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
        );

        let engine = Arc::new(SyncEngine::new(ctx.pool().clone(), cfg.clone()));

        // Background scheduler — one scheduled submission per Bilanzierungsmonat.
        {
            let engine = engine.clone();
            let cfg = cfg.clone();
            let shutdown = ctx.shutdown.clone();
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
                    tokio::select! {
                        () = shutdown.cancelled() => break,
                        () = tokio::time::sleep(tokio::time::Duration::from_secs(300)) => {}
                    }
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

        let state = server::ServerState {
            pool: ctx.pool().clone(),
            engine,
            cfg: cfg.clone(),
        };
        Ok(server::router(state)
            .layer(axum::Extension(cedar))
            .layer(axum::Extension(oidc)))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<MabisSyncd>().await
}
