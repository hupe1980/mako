//! `marktd` — Master Data Manager daemon.
//!
//! Assembles all modules, connects to PostgreSQL, runs migrations, starts the
//! background fan-out worker, and serves the axum HTTP API.
#![deny(unsafe_code)]

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use axum::{
    Extension, Router,
    routing::{delete, get, post, put},
};
use clap::Parser;
use mako_markt::repository::AppState;
use sqlx::postgres::PgPoolOptions;
use tokio_util::sync::CancellationToken;
use tracing::info;

use mako_service::cedar::CedarEnforcer;
use mako_service::event_bus::{WebhookBus, WebhookBusConfig};
use marktd::{
    config::{self, Config},
    fanout::{FanoutConfig, spawn as spawn_fanout},
    handlers::{
        TenantGln,
        contract::{get_contract, put_contract},
        correlation::{get_correlation, list_correlations},
        device::{
            delete_konfigurationsprodukt, get_geraet, get_geraet_konfigurationen,
            get_konfigurationsprodukte, get_steuerbare_ressource, get_tariff_zone,
            get_technische_ressource, get_zaehlwerke, get_zaehlzeitdefinitionen, list_geraete,
            list_technische_ressourcen_by_malo, list_zaehler, list_zaehler_register,
            list_zaehler_saisons, put_geraet, put_geraet_konfigurationen,
            put_konfigurationsprodukte, put_steuerbare_ressource, put_technische_ressource,
            put_zaehler, put_zaehler_register, put_zaehler_saison,
        },
        dlq::{delete_dlq_entry, list_dlq, retry_dlq_entry},
        event_ingest::{InboundWebhookSecret, ingest_event},
        event_log::list_event_log,
        health::{health, health_ready},
        lokationszuordnung::{
            delete_lokationszuordnung, get_malo_lokationen, get_melo_lokationen,
            put_lokationszuordnung,
        },
        malo::{get_malo, get_malo_lastprofil, list_malo, put_malo},
        malo_grid::{get_malo_grid, put_malo_grid},
        melo::{get_melo, get_melo_standorteigenschaften, put_melo},
        metrics::metrics_handler,
        mmma_preise,
        nb_contract::{get_nb_contract, list_nb_contracts, put_nb_contract},
        nb_energiemix::{get_nb_energiemix, get_nb_energiemix_history, put_nb_energiemix},
        nelo::{get_nelo, list_nelos, put_nelo},
        partner::{get_as4_address, get_partner, list_partners, put_partner},
        preisblatt::{
            get_preisblatt, get_preisblatt_dienstleistung, get_preisblatt_hardware,
            get_preisblatt_ka, get_preisblatt_messung, put_preisblatt,
            put_preisblatt_dienstleistung, put_preisblatt_hardware, put_preisblatt_ka,
            put_preisblatt_messung,
        },
        pricat::{get_dispatch_log, get_pricat_history, post_pricat_dispatch},
        subscription::{get_subscription, list_subscriptions, put_subscription, test_subscription},
        versorgung::{get_versorgungsstatus, get_versorgungsstatus_history, put_versorgungsstatus},
    },
    openapi::swagger_ui,
    pg::{
        PgContractRepository, PgCorrelationIndex, PgDeviceRepository,
        PgLokationszuordnungRepository, PgMaloGridRepository, PgMaloRepository, PgMeloRepository,
        PgMmmPreisStromRepository, PgMmmaPreisGasRepository, PgNbContractRepository,
        PgNeLoRepository, PgPartnerRepository, PgPreisblattDienstleistungRepository,
        PgPreisblattHardwareRepository, PgPreisblattKaRepository, PgPreisblattMessungRepository,
        PgPreisblattRepository, PgPriCatRepository, PgSteuerbareRessourceRepository,
        PgSubscriptionRepository, PgTechnischeRessourceRepository, PgVersorgungsStatusRepository,
        PgZaehlzeitRepository,
    },
};

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(
    name = "marktd",
    about = "Master Data Manager for German energy market (MaKo)"
)]
struct Cli {
    /// Path to the `marktd.toml` configuration file.
    #[arg(short, long, default_value = "marktd.toml", env = "MARKTD_CONFIG")]
    config: std::path::PathBuf,

    /// Log level override (default: INFO).
    #[arg(long, default_value = "info", env = "RUST_LOG")]
    log_level: String,

    /// Validate configuration and database connectivity, then exit 0.
    ///
    /// Parses the TOML file, resolves all `env:` secrets, connects to
    /// PostgreSQL, runs migrations, and exits 0 on success, non-zero on
    /// any failure. No HTTP server or background workers are started.
    /// Suitable for Dockerfile HEALTHCHECK and Kubernetes init containers.
    #[arg(long, env = "MARKTD_CHECK", default_value_t = false)]
    check: bool,
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // ── Logging + OpenTelemetry ────────────────────────────────────────────
    // Config not yet loaded — use a temporary plain-logging setup so startup
    // errors are visible. Re-init happens after config is read below.
    //
    // NOTE: init_tracing is called once with the OTel config from the file.
    // We parse config first (plain logging), then hand off to init_tracing.
    // Use a two-stage approach: bootstrap logging here, then reinitialise
    // below after the config is available.
    //
    // Actually: parse config first, then init tracing with OTel endpoint.
    // We cannot init before config, so skip bootstrap and do it after.

    info!(config = %cli.config.display(), "marktd: starting");

    // ── Config ────────────────────────────────────────────────────────────────
    let cfg: Config = config::load_from_file(&cli.config)
        .with_context(|| format!("loading config from {}", cli.config.display()))?;

    // ── Logging + OpenTelemetry (uses config) ─────────────────────────────────
    let _otel_guard = mako_service::init_tracing(
        "marktd",
        &cli.log_level,
        cfg.otel.is_enabled().then_some(&cfg.otel),
    );

    // Resolve env-var references in secrets.
    let db_url =
        config::resolve_env(&cfg.storage.postgres.url).context("resolving DATABASE_URL")?;
    let makod_api_key =
        config::resolve_env_secret(&cfg.makod.api_key).context("resolving MAKOD_API_KEY")?;
    let inbound_secret = cfg
        .webhook
        .inbound_secret
        .as_deref()
        .map(config::resolve_env)
        .transpose()
        .context("resolving MAKOD_WEBHOOK_SECRET")?;

    // ── PostgreSQL ────────────────────────────────────────────────────────────
    info!("marktd: connecting to PostgreSQL");
    let pool = PgPoolOptions::new()
        .max_connections(cfg.storage.postgres.max_connections)
        .min_connections(cfg.storage.postgres.min_connections)
        .acquire_timeout(Duration::from_secs(
            cfg.storage.postgres.acquire_timeout_secs,
        ))
        .connect(&db_url)
        .await
        .context("connecting to PostgreSQL")?;

    // Apply schema migrations.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("marktd: running database migrations")?;

    // ── --check mode early exit ────────────────────────────────────────────────
    //
    // Config parsed, secrets resolved, PostgreSQL reachable, migrations applied.
    // In check mode we exit here — no HTTP server, no background workers.
    if cli.check {
        info!("marktd: check mode — config, secrets, and database connectivity verified");
        return Ok(());
    }

    // ── Repositories ──────────────────────────────────────────────────────────
    let malo_repo = PgMaloRepository::new(pool.clone());
    let melo_repo = PgMeloRepository::new(pool.clone());
    let contract_repo = PgContractRepository::new(pool.clone());
    let sub_repo = PgSubscriptionRepository::new(pool.clone());
    let ci = PgCorrelationIndex::new(pool.clone());
    let partner_repo = PgPartnerRepository::new(pool.clone());
    let preisblatt_repo = std::sync::Arc::new(PgPreisblattRepository::new(pool.clone()));
    let preisblatt_messung_repo =
        std::sync::Arc::new(PgPreisblattMessungRepository::new(pool.clone()));
    let preisblatt_ka_repo = std::sync::Arc::new(PgPreisblattKaRepository::new(pool.clone()));
    let preisblatt_dl_repo =
        std::sync::Arc::new(PgPreisblattDienstleistungRepository::new(pool.clone()));
    let preisblatt_hw_repo = std::sync::Arc::new(PgPreisblattHardwareRepository::new(pool.clone()));
    let sr_repo = std::sync::Arc::new(PgSteuerbareRessourceRepository::new(pool.clone()));
    let tr_repo = std::sync::Arc::new(PgTechnischeRessourceRepository::new(pool.clone()));
    let lz_repo = std::sync::Arc::new(PgLokationszuordnungRepository::new(pool.clone()));
    let device_repo = std::sync::Arc::new(PgDeviceRepository::new(pool.clone()));
    let zaehzeit_repo = std::sync::Arc::new(PgZaehlzeitRepository::new(pool.clone()));
    let nb_contract_repo = std::sync::Arc::new(PgNbContractRepository::new(pool.clone()));
    let vs_repo = std::sync::Arc::new(PgVersorgungsStatusRepository::new(pool.clone()));
    let pricat_repo = std::sync::Arc::new(PgPriCatRepository::new(pool.clone()));
    let nelo_repo = std::sync::Arc::new(PgNeLoRepository::new(pool.clone()));
    let malo_grid_repo = Arc::new(PgMaloGridRepository::new(pool.clone()));
    let mmma_gas_repo = std::sync::Arc::new(PgMmmaPreisGasRepository::new(pool.clone()));
    let mmm_strom_repo = std::sync::Arc::new(PgMmmPreisStromRepository::new(pool.clone()));

    // ── Graceful shutdown token ───────────────────────────────────────────────
    let shutdown = CancellationToken::new();

    // ── Background tasks ──────────────────────────────────────────────────────
    // OIDC JWKS refresh is handled by OidcConfig::build_verifier above.

    // ── OIDC verifier ─────────────────────────────────────────────────────────
    let http = reqwest::Client::builder()
        .user_agent("marktd/0.1 (+https://github.com/hupe1980/edi-energy-rs)")
        .timeout(Duration::from_secs(10))
        .build()
        .context("building HTTP client")?;

    let verifier = mako_service::oidc::OidcConfig::build_verifier(
        cfg.oidc.as_ref(),
        &http,
        &cfg.makod.tenant_id,
        shutdown.clone(),
    )
    .await?;

    // ── MaKod client ──────────────────────────────────────────────────────────
    let makod_client = Arc::new(mako_markt::makod_client::MakodClient::new(
        &cfg.makod.base_url,
        makod_api_key,
    ));

    // ── MPSC event channel ─────────────────────────────────────────────────
    // Unlike broadcast, unbounded MPSC never drops events when the receiver
    // lags.  There is exactly one consumer (the fan-out worker).
    //
    // `Value`-typed so the fan-out worker and the `EventBus` abstraction
    // share the same channel without a typed `MarktEvent` dep in mako-service.
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    // Clone for background workers that need to emit CloudEvents.
    let event_tx_for_workers = event_tx.clone();

    // ── EventBus abstraction ─────────────────────────────────────────────────
    // Wraps the fan-out MPSC channel behind `Arc<dyn EventBus>`.
    // Swap in `KafkaBus` here (feature "kafka") without touching any handler.
    let _event_bus: Arc<dyn mako_service::event_bus::EventBus> = Arc::new(
        WebhookBus::new(WebhookBusConfig {
            delivery_timeout: Duration::from_secs(cfg.webhook.delivery_timeout_secs),
            max_retry_attempts: cfg.webhook.max_retry_attempts,
        })
        .with_sender(event_tx.clone()),
    );

    // ── AppState ──────────────────────────────────────────────────────────────
    let state = Arc::new(AppState {
        malo_repo,
        melo_repo,
        contract_repo,
        subscription_repo: sub_repo.clone(),
        correlation_index: ci,
        partner_repo,
        makod_client,
        event_tx,
        tenant_gln: cfg.makod.tenant_id.clone(),
    });

    spawn_fanout(
        event_rx,
        sub_repo,
        http.clone(),
        FanoutConfig {
            delivery_timeout: Duration::from_secs(cfg.webhook.delivery_timeout_secs),
            max_retry_attempts: cfg.webhook.max_retry_attempts,
        },
        pool.clone(), // DLQ writes on delivery failure — §22 MessZV compliance
        shutdown.clone(),
    );

    // ── processed_events TTL cleanup ────────────────────────────────────────────
    // DELETE processed_events rows older than 7 days every hour so the
    // idempotency table does not grow without bound.
    {
        let cleanup_pool = pool.clone();
        let cleanup_shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3_600));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let cutoff = time::OffsetDateTime::now_utc()
                            - time::Duration::days(7);
                        match sqlx::query(
                            "DELETE FROM processed_events WHERE processed_at < $1",
                        )
                        .bind(cutoff)
                        .execute(&cleanup_pool)
                        .await
                        {
                            Ok(r) => {
                                if r.rows_affected() > 0 {
                                    tracing::info!(
                                        rows = r.rows_affected(),
                                        "processed_events: pruned old entries"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "processed_events: cleanup failed");
                            }
                        }
                    }
                    _ = cleanup_shutdown.cancelled() => break,
                }
            }
        });
    }

    // ── Cedar ABAC enforcer ──────────────────────────────────────────────────
    let cedar = Arc::new(
        CedarEnforcer::from_policy_str(include_str!("../policies/marktd.cedar"))
            .context("loading Cedar policies from policies/marktd.cedar")?,
    );

    // ── B12: MMMA/MMM price import background worker ──────────────────────────
    {
        use marktd::mmma_worker::spawn_mmma_worker;
        let mmma_gas_repo = Arc::new(marktd::pg::mmma_preise::PgMmmaPreisGasRepository::new(
            pool.clone(),
        ));
        let mmma_strom_repo = Arc::new(marktd::pg::mmma_preise::PgMmmPreisStromRepository::new(
            pool.clone(),
        ));
        spawn_mmma_worker(
            Arc::new(cfg.mmma_import.clone()),
            mmma_gas_repo,
            mmma_strom_repo,
            cfg.makod.tenant_id.clone(),
            event_tx_for_workers.clone(),
            shutdown.clone(),
        );
    }

    // ── MCP server ────────────────────────────────────────────────────────────
    let mcp_state = Arc::new(marktd::mcp_server::MdmdMcpState {
        pool: pool.clone(),
        tenant: cfg.makod.tenant_id.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
            &cfg.mcp,
            verifier.clone(),
            Some(cedar.clone()),
            &cfg.makod.tenant_id,
        ),
    });
    let inbound_path = cfg.webhook.inbound_path.clone();
    let app =
        Router::new()
            .route("/health", get(health))
            .route("/health/live", get(health))
            .route("/health/ready", get(health_ready))
            // MaLo
            .route("/api/v1/malo", get(list_malo::<_, _, _, _, _, _>))
            .route("/api/v1/malo/{id}", put(put_malo::<_, _, _, _, _, _>))
            .route("/api/v1/malo/{id}", get(get_malo::<_, _, _, _, _, _>))
            // Lastprofil derivation — SLP profile for NNE tariff zone + billingd (L7)
            .route(
                "/api/v1/malo/{id}/lastprofil",
                get(get_malo_lastprofil::<_, _, _, _, _, _>),
            )
            // MeLo
            .route("/api/v1/melo/{id}", put(put_melo::<_, _, _, _, _, _>))
            .route("/api/v1/melo/{id}", get(get_melo::<_, _, _, _, _, _>))
            .route(
                "/api/v1/melos/{id}/standorteigenschaften",
                get(get_melo_standorteigenschaften::<_, _, _, _, _, _>),
            )
            // Contracts
            .route(
                "/api/v1/contracts/{id}",
                put(put_contract::<_, _, _, _, _, _>),
            )
            .route(
                "/api/v1/contracts/{id}",
                get(get_contract::<_, _, _, _, _, _>),
            )
            // Subscriptions
            .route(
                "/api/v1/subscriptions",
                get(list_subscriptions::<_, _, _, _, _, _>),
            )
            .route(
                "/api/v1/subscriptions/{id}",
                put(put_subscription::<_, _, _, _, _, _>),
            )
            .route(
                "/api/v1/subscriptions/{id}",
                get(get_subscription::<_, _, _, _, _, _>),
            )
            .route(
                "/api/v1/subscriptions/{id}/test",
                post(test_subscription::<_, _, _, _, _, _>),
            )
            // Correlations
            .route(
                "/api/v1/correlations",
                get(list_correlations::<_, _, _, _, _, _>),
            )
            .route(
                "/api/v1/correlations/{id}",
                get(get_correlation::<_, _, _, _, _, _>),
            )
            // Partners
            .route("/api/v1/partners", get(list_partners::<_, _, _, _, _, _>))
            .route(
                "/api/v1/partners/{mp_id}",
                put(put_partner::<_, _, _, _, _, _>),
            )
            .route(
                "/api/v1/partners/{mp_id}",
                get(get_partner::<_, _, _, _, _, _>),
            )
            // Price sheets (PreisblattNetznutzung)
            .route(
                "/api/v1/preisblaetter/{nb_mp_id}",
                get(get_preisblatt).put(put_preisblatt),
            )
            // Price sheets (PreisblattMessung — MSB metering tariffs, B5)
            .route(
                "/api/v1/preisblaetter-messung/{msb_mp_id}",
                get(get_preisblatt_messung).put(put_preisblatt_messung),
            )
            // Konzessionsabgabe price sheets (B3 — §17 StromNZV)
            .route(
                "/api/v1/preisblaetter-ka/{nb_mp_id}",
                get(get_preisblatt_ka).put(put_preisblatt_ka),
            )
            // MMMA Gas settlement prices (C18 — Trading Hub Europe, monthly)
            .route("/api/v1/mmma-preise/gas", get(mmma_preise::list_mmma_gas))
            .route(
                "/api/v1/mmma-preise/gas/{year}/{month}",
                get(mmma_preise::get_mmma_gas).put(mmma_preise::put_mmma_gas),
            )
            // MMM Strom settlement prices (C18 — ÜNB per §22 StromNZV, monthly)
            .route(
                "/api/v1/mmm-preise/strom/{year}/{month}",
                get(mmma_preise::get_mmm_strom).put(mmma_preise::put_mmm_strom),
            )
            // B12: Manual import trigger — immediately runs the monthly import cycle.
            // Useful for catch-up after downtime or testing the configured import URLs.
            .route(
                "/api/v1/mmma-preise/import-trigger",
                axum::routing::post(mmma_preise::post_import_trigger),
            )
            // MSB service price sheets (PreisblattDienstleistung)
            .route(
                "/api/v1/preisblaetter-dienstleistung/{msb_mp_id}",
                get(get_preisblatt_dienstleistung).put(put_preisblatt_dienstleistung),
            )
            // MSB hardware rental price sheets (PreisblattHardware)
            .route(
                "/api/v1/preisblaetter-hardware/{msb_mp_id}",
                get(get_preisblatt_hardware).put(put_preisblatt_hardware),
            )
            // B2: AS4 address lookup (Marktteilnehmer.makoadresse)
            .route(
                "/api/v1/partners/{mp_id}/as4-address",
                get(get_as4_address::<_, _, _, _, _, _>),
            )
            // PRICAT version history + manual dispatch (Phase 2)
            .route("/api/v1/pricat/{nb_mp_id}/history", get(get_pricat_history))
            .route(
                "/api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}",
                get(get_dispatch_log),
            )
            .route(
                "/api/v1/pricat/{nb_mp_id}/dispatch",
                post(post_pricat_dispatch),
            )
            // NB network contracts (typed: netzebene, bilanzierungsmethode, billing_schedule)
            .route(
                "/api/v1/nb-contracts/{id}",
                get(get_nb_contract).put(put_nb_contract),
            )
            .route("/api/v1/nb-contracts", get(list_nb_contracts))
            // VersorgungsStatus per MaLo (Phase 1) + history / point-in-time (Phase 3)
            .route(
                "/api/v1/versorgung/{malo_id}",
                get(get_versorgungsstatus::<
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    marktd::pg::PgVersorgungsStatusRepository,
                >)
                .put(
                    put_versorgungsstatus::<
                        _,
                        _,
                        _,
                        _,
                        _,
                        _,
                        marktd::pg::PgVersorgungsStatusRepository,
                    >,
                ),
            )
            .route(
                "/api/v1/versorgung/{malo_id}/history",
                get(get_versorgungsstatus_history::<
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    marktd::pg::PgVersorgungsStatusRepository,
                >),
            )
            // NB Energiemix authority (§42 EnWG — N8)
            // NB publishes annual grid-area renewable mix; LFs use for Reststrommix disclosure.
            .route(
                "/api/v1/energiemix/{nb_mp_id}",
                get(get_nb_energiemix).put(put_nb_energiemix),
            )
            .route(
                "/api/v1/energiemix/{nb_mp_id}/history",
                get(get_nb_energiemix_history),
            )
            // Netz-Element-Lokationen (Redispatch 2.0, Phase 3)
            .route("/api/v1/nelo", get(list_nelos))
            .route("/api/v1/nelo/{id}", get(get_nelo).put(put_nelo))
            // MaLo grid topology (NB STP, N7)
            .route(
                "/api/v1/malo/{id}/grid",
                get(get_malo_grid).put(put_malo_grid),
            )
            // SteuerbareRessource registry (B4b — WiM iMS Steuerungsauftrag)
            .route(
                "/api/v1/steuerbare-ressourcen/{sr_id}",
                get(get_steuerbare_ressource).put(put_steuerbare_ressource),
            )
            // M1: typed Konfigurationsprodukte sub-resource — used by makod before
            // dispatching wim.steuerungsauftrag.bestaetigen to validate produktcode
            .route(
                "/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte",
                get(get_konfigurationsprodukte).put(put_konfigurationsprodukte),
            )
            // M1: individual product DELETE by produktcode
            .route(
                "/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte/{produktcode}",
                axum::routing::delete(delete_konfigurationsprodukt),
            )
            // TechnischeRessource registry (B9 — EMobility/Redispatch 2.0)
            .route(
                "/api/v1/technische-ressourcen/{tr_id}",
                get(get_technische_ressource).put(put_technische_ressource),
            )
            .route(
                "/api/v1/malos/{malo_id}/technische-ressourcen",
                get(list_technische_ressourcen_by_malo),
            )
            // Lokationszuordnung graph API (B5 — MaLo↔MeLo↔NeLo↔SR↔TR topology)
            .route(
                "/api/v1/lokationszuordnungen",
                axum::routing::put(put_lokationszuordnung),
            )
            .route(
                "/api/v1/lokationszuordnungen/{von_id}/{nach_id}",
                axum::routing::delete(delete_lokationszuordnung),
            )
            .route("/api/v1/malos/{id}/lokationen", get(get_malo_lokationen))
            .route("/api/v1/melos/{id}/lokationen", get(get_melo_lokationen))
            // Device registry: Zähler + Geräte (B3 — WiM MSB/NB device handover)
            .route("/api/v1/melos/{melo_id}/zaehler", get(list_zaehler))
            .route(
                "/api/v1/zaehler/{zaehler_id}",
                axum::routing::put(put_zaehler),
            )
            .route("/api/v1/zaehler/{zaehler_id}/geraete", get(list_geraete))
            .route(
                "/api/v1/zaehler/{zaehler_id}/geraete/{geraet_id}",
                get(get_geraet),
            )
            .route(
                "/api/v1/zaehler/{zaehler_id}/geraete/{geraet_id}/konfigurationen",
                get(get_geraet_konfigurationen).put(put_geraet_konfigurationen),
            )
            .route(
                "/api/v1/zaehler/{zaehler_id}/zaehlwerke",
                get(get_zaehlwerke),
            )
            .route(
                "/api/v1/geraete/{geraet_id}",
                axum::routing::put(put_geraet),
            )
            // ZaehlzeitRegister + ZaehlzeitSaison (M4 — iMSys TOU register definitions)
            .route(
                "/api/v1/zaehler/{zaehler_id}/register",
                get(list_zaehler_register).put(put_zaehler_register),
            )
            .route(
                "/api/v1/zaehler/{zaehler_id}/zaehlzeitdefinitionen",
                get(get_zaehlzeitdefinitionen),
            )
            .route(
                "/api/v1/zaehler-register/{register_id}/saisons",
                get(list_zaehler_saisons).put(put_zaehler_saison),
            )
            .route(
                "/api/v1/zaehler/{zaehler_id}/tariff-zone",
                get(get_tariff_zone),
            )
            // Inbound makod events
            .route(&inbound_path, post(ingest_event::<_, _, _, _, _, _>))
            // Dead-letter queue admin (F-003 — §22 MessZV compliance)
            .route("/admin/fanout/dlq", get(list_dlq))
            .route("/admin/fanout/dlq/{id}", delete(delete_dlq_entry))
            .route("/admin/fanout/dlq/{id}/retry", post(retry_dlq_entry))
            // CloudEvent replay log admin (B11)
            .route("/admin/events", get(list_event_log))
            // Prometheus-compatible metrics (F-006)
            .route("/metrics", get(metrics_handler))
            // Swagger UI
            .merge(swagger_ui())
            // State + extensions
            .with_state(state.clone())
            .layer(Extension(verifier))
            .layer(Extension(InboundWebhookSecret(inbound_secret)))
            // Pool extension for idempotency check in ingest_event
            .layer(Extension(pool.clone()))
            // Preisblatt repository extension (M4)
            .layer(Extension(preisblatt_repo))
            // PreisblattMessung repository extension (B5 — MSB metering tariffs)
            .layer(Extension(preisblatt_messung_repo))
            // KA price sheet repository extension (B3)
            .layer(Extension(preisblatt_ka_repo))
            // MMMA Gas + MMM Strom settlement price repos (C18)
            .layer(Extension(mmma_gas_repo))
            .layer(Extension(mmm_strom_repo))
            // B12: MMMA import config for manual trigger endpoint
            .layer(Extension(Arc::new(cfg.mmma_import.clone())))
            // MSB Dienstleistung + Hardware price sheet repos
            .layer(Extension(preisblatt_dl_repo))
            .layer(Extension(preisblatt_hw_repo))
            // PRICAT version history + dispatch extension (Phase 2)
            .layer(Extension(pricat_repo))
            // NB contract repository extension
            .layer(Extension(nb_contract_repo))
            // VersorgungsStatus repository extension (Phase 1)
            .layer(Extension(vs_repo))
            // NeLo repository extension (Phase 3)
            .layer(Extension(nelo_repo))
            // MaLo grid topology extension (N7)
            .layer(Extension(malo_grid_repo))
            // SteuerbareRessource registry extension (B4b)
            .layer(Extension(sr_repo))
            // TechnischeRessource registry extension (B9)
            .layer(Extension(tr_repo))
            // Lokationszuordnung graph extension (B5)
            .layer(Extension(lz_repo))
            // Device registry extension (B3)
            .layer(Extension(device_repo))
            // ZaehlzeitRegister + ZaehlzeitSaison (M4 — iMSys TOU)
            .layer(Extension(zaehzeit_repo))
            // event_tx extension for handlers that emit CloudEvents without AppState
            .layer(Extension(state.event_tx.clone()))
            // Cedar ABAC enforcer (M6)
            .layer(Extension(cedar))
            // Tenant GLN for handlers without AppState access (e.g. preisblatt)
            .layer(Extension(TenantGln(cfg.makod.tenant_id.clone())))
            // HTTP client extension for test_subscription direct delivery
            .layer(Extension(http.clone()))
            // Limit request bodies to 2 MiB to guard against accidental large payloads.
            .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024))
            // MCP server (M7)
            .merge(marktd::mcp_server::router(mcp_state, shutdown.clone()));

    // ── Listen ────────────────────────────────────────────────────────────────
    let addr: SocketAddr = cfg.http.addr.parse().context("parsing listen address")?;
    info!(%addr, "marktd: listening");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding to {addr}"))?;

    // ── Shutdown handler ──────────────────────────────────────────────────────
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("marktd: SIGINT received, shutting down");
        shutdown_clone.cancel();
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .context("serving HTTP")?;

    info!("marktd: shutdown complete");
    Ok(())
}
