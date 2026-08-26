//! `marktd` — the market data hub.
//!
//! Port: `:8180`
//!
//! `mako_service::run` owns the lifecycle — tracing, the tuned pool, migrations,
//! real DB-ping readiness, `/health/*`, `/metrics`, the HTTP trace layer, and
//! graceful SIGINT **and SIGTERM** shutdown. This file supplies only what is
//! marktd's own: the repositories, the domain router, and the two background
//! workers.
//!
//! The full endpoint surface is documented in the crate README and in
//! `site/content/docs/services/marktd.md`; `/swagger-ui/` serves it live.
#![deny(unsafe_code)]

use std::{sync::Arc, time::Duration};

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{delete, get, patch, post, put},
};
use mako_markt::repository::AppState;
use mako_service::{Daemon, ServiceContext, cedar::CedarEnforcer};
use sqlx::PgPool;
use tracing::info;

use marktd::{
    config::{self, Config},
    fanout::{FanoutConfig, spawn as spawn_fanout},
    handlers::{
        TenantGln,
        bilanzierung::{get_bilanzierung_at, get_bilanzierung_history, put_bilanzierung},
        correlation::{get_correlation, list_correlations},
        device::{
            delete_konfigurationsprodukt, get_geraet, get_geraet_konfigurationen,
            get_konfigurationsprodukte, get_sharing_eligibility, get_steuerbare_ressource,
            get_tariff_zone, get_technische_ressource, get_zaehlwerke, get_zaehlzeitdefinitionen,
            list_geraete, list_technische_ressourcen_by_malo, list_zaehler, list_zaehler_register,
            list_zaehler_saisons, put_geraet, put_geraet_konfigurationen,
            put_konfigurationsprodukte, put_steuerbare_ressource, put_technische_ressource,
            put_zaehler, put_zaehler_register, put_zaehler_saison,
        },
        dlq::{delete_dlq_entry, list_dlq, retry_dlq_entry},
        einwilligung::{
            consent_check, get_einwilligung, get_esa_preise, get_framework, grant_einwilligung,
            list_einwilligungen, put_esa_preise, put_framework, revoke_einwilligung,
        },
        event_ingest::{InboundWebhookSecret, ingest_event},
        event_log::list_event_log,
        grundversorger::{get_grundversorger, put_grundversorger},
        lokationszuordnung::{
            delete_lokationszuordnung, get_malo_buendel, get_malo_lokationen, get_melo_lokationen,
            put_lokationszuordnung,
        },
        mabis_zp::{get_mabis_zp, list_mabis_zp, put_mabis_zp},
        malo::{get_malo, get_malo_lastprofil, list_malo, put_malo},
        malo_grid::{get_malo_grid, put_malo_grid},
        melo::{get_melo, get_melo_standorteigenschaften, put_melo},
        melo_msb::{get_melo_msb_at, get_melo_msb_history, put_melo_msb},
        mmma_preise,
        msb_rahmenvertrag_gas::{get_msb_rv_gas, list_msb_rv_gas, upsert_msb_rv_gas},
        nb_contract::{
            get_nb_contract, get_nb_contract_by_malo, list_nb_contracts, put_nb_contract,
        },
        nb_energiemix::{get_nb_energiemix, get_nb_energiemix_history, put_nb_energiemix},
        nelo::{get_nelo, list_nelos, put_nelo},
        netzzugang::{get_antrag, list_antraege, set_antrag_status, upsert_antrag},
        partner::{
            get_as4_address, get_partner, get_partner_marktteilnehmer, list_partners, put_partner,
        },
        preisblatt::{
            get_preisblatt, get_preisblatt_dienstleistung, get_preisblatt_hardware,
            get_preisblatt_ka, get_preisblatt_messung, put_preisblatt,
            put_preisblatt_dienstleistung, put_preisblatt_hardware, put_preisblatt_ka,
            put_preisblatt_messung,
        },
        pricat::{get_dispatch_log, get_pricat_history, post_pricat_dispatch},
        subscription::{
            delete_subscription, get_subscription, list_subscriptions, put_subscription,
            test_subscription,
        },
        tranche::{get_tranche, list_tranchen, put_tranche},
        versorgung::{get_versorgungsstatus, get_versorgungsstatus_history, put_versorgungsstatus},
    },
    openapi::swagger_ui,
    pg,
};

/// The daemon.
struct Marktd;

impl Daemon for Marktd {
    type Config = Config;
    const NAME: &'static str = "marktd";

    async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run marktd migrations")
    }

    async fn build(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
        let pool = ctx.pool().clone();
        let shutdown = ctx.shutdown.clone();
        // The shared client: 5 s connect timeout and, critically for a hub that
        // POSTs to operator-supplied webhook URLs, no redirect following — so a
        // subscriber endpoint cannot bounce a delivery onto internal
        // infrastructure (see handlers::subscription::validate_webhook_url).
        let http = ctx.http.clone();
        let tenant = cfg.markt.tenant.clone();

        // Fail closed: without [oidc] every request is admitted with dev claims,
        // and without webhook.inbound_secret the inbound events endpoint accepts
        // unsigned events that mutate VersorgungsStatus and the device registry.
        // Either gap must be asked for by name, never reached by omission.
        if cfg.allow_insecure_no_auth {
            tracing::warn!(
                "marktd: allow_insecure_no_auth is set — dev claims accepted and unsigned \
                 inbound events accepted"
            );
        } else {
            anyhow::ensure!(
                cfg.oidc.is_some(),
                "no [oidc] section configured — every request would be admitted with dev \
                 claims. Configure [oidc], or set allow_insecure_no_auth = true."
            );
            anyhow::ensure!(
                cfg.webhook.inbound_secret.is_some(),
                "no webhook.inbound_secret configured — the inbound events endpoint would \
                 accept unsigned events that mutate master data. Configure it, or set \
                 allow_insecure_no_auth = true."
            );
        }

        let makod_api_key =
            config::resolve_env_secret(&cfg.makod.api_key).context("resolve makod.api_key")?;
        let inbound_secret = cfg
            .webhook
            .inbound_secret
            .as_deref()
            .map(config::resolve_env)
            .transpose()
            .context("resolve webhook.inbound_secret")?;

        let verifier = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &http,
            &tenant,
            shutdown.clone(),
        )
        .await?;

        let cedar = Arc::new(
            CedarEnforcer::from_policy_str(include_str!("../policies/marktd.cedar"))
                .context("load Cedar policies from policies/marktd.cedar")?,
        );

        let makod_client = Arc::new(mako_markt::makod_client::MakodClient::new(
            &cfg.makod.base_url,
            makod_api_key,
        ));

        // No in-memory event channel: producers persist every event to the
        // `event_log` outbox (marktd::outbox::enqueue) BEFORE any fan-out, and
        // the durable fan-out worker is the sole consumer of
        // `event_log`/`event_delivery`. `notify` is only a low-latency wake-up
        // hint — correctness rests on the tables, so a missed notification
        // delays delivery, never drops it.
        let notify = Arc::new(tokio::sync::Notify::new());

        let sub_repo = pg::PgSubscriptionRepository::new(pool.clone());
        let state = Arc::new(AppState {
            malo_repo: pg::PgMaloRepository::new(pool.clone()),
            melo_repo: pg::PgMeloRepository::new(pool.clone(), tenant.clone()),
            subscription_repo: sub_repo.clone(),
            correlation_index: pg::PgCorrelationIndex::new(pool.clone()),
            partner_repo: pg::PgPartnerRepository::new(pool.clone()),
            makod_client: Arc::clone(&makod_client),
            notify: Arc::clone(&notify),
            tenant_gln: tenant.clone(),
        });

        spawn_workers(&cfg, &pool, sub_repo, &http, &notify, &shutdown);

        let mcp = marktd::mcp_server::router(
            Arc::new(marktd::mcp_server::MdmdMcpState {
                pool: pool.clone(),
                tenant: tenant.clone(),
                auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
                    &cfg.mcp,
                    verifier.clone(),
                    Some(Arc::clone(&cedar)),
                    &tenant,
                ),
            }),
            shutdown,
        );

        let app = Router::new()
            .merge(malo_routes())
            .merge(melo_routes())
            .merge(device_routes())
            .merge(partner_routes())
            .merge(preisblatt_routes())
            .merge(registry_routes())
            .merge(esa_routes())
            .merge(subscription_routes())
            .merge(admin_routes())
            .route(
                &cfg.webhook.inbound_path,
                post(ingest_event::<_, _, _, _, _>),
            )
            .merge(swagger_ui())
            .with_state(state)
            .layer(Extension(verifier))
            .layer(Extension(InboundWebhookSecret(inbound_secret)))
            .layer(Extension(cedar))
            .layer(Extension(TenantGln(tenant.clone())))
            .layer(Extension(notify))
            .layer(Extension(makod_client))
            .layer(Extension(http))
            .layer(Extension(Arc::new(cfg.mmma_import.clone())))
            .layer(Extension(pool.clone()))
            // Bound request bodies so an accidental bulk upload cannot exhaust
            // memory. The largest legitimate body is a BO4E price sheet.
            .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024))
            .merge(mcp);

        let app = repository_layers(app, &pool);

        info!(%tenant, "marktd router built");
        Ok(app)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Marktd>().await
}

// ── Routes ────────────────────────────────────────────────────────────────────
//
// Collections are plural throughout (`/malos`, `/melos`, `/zaehler`), and a
// sub-resource hangs off the collection member, so a caller never has to
// memorise the pluralisation per endpoint.

type S = Arc<
    AppState<
        pg::PgMaloRepository,
        pg::PgMeloRepository,
        pg::PgSubscriptionRepository,
        pg::PgCorrelationIndex,
        pg::PgPartnerRepository,
    >,
>;

fn malo_routes() -> Router<S> {
    Router::new()
        .route("/api/v1/malos", get(list_malo::<_, _, _, _, _>))
        .route(
            "/api/v1/malos/{id}",
            get(get_malo::<_, _, _, _, _>).put(put_malo::<_, _, _, _, _>),
        )
        // SLP profile for the NNE tariff zone and billingd.
        .route(
            "/api/v1/malos/{id}/lastprofil",
            get(get_malo_lastprofil::<_, _, _, _, _>),
        )
        // NB grid topology (read by the processd NB module for Anmeldung STP).
        .route(
            "/api/v1/malos/{id}/grid",
            get(get_malo_grid).put(put_malo_grid),
        )
        // BO4E Bilanzierung — first-class temporal balancing resource.
        .route(
            "/api/v1/malos/{malo_id}/bilanzierung",
            get(get_bilanzierung_at).put(put_bilanzierung),
        )
        .route(
            "/api/v1/malos/{malo_id}/bilanzierung/history",
            get(get_bilanzierung_history),
        )
        .route(
            "/api/v1/malos/{malo_id}/technische-ressourcen",
            get(list_technische_ressourcen_by_malo),
        )
        .route("/api/v1/malos/{id}/lokationen", get(get_malo_lokationen))
        .route("/api/v1/malos/{id}/buendel", get(get_malo_buendel))
        // VersorgungsStatus per MaLo, with point-in-time and full history.
        .route(
            "/api/v1/versorgung/{malo_id}",
            get(get_versorgungsstatus::<_, _, _, _, _, pg::PgVersorgungsStatusRepository>)
                .put(put_versorgungsstatus),
        )
        .route(
            "/api/v1/versorgung/{malo_id}/history",
            get(get_versorgungsstatus_history::<_, _, _, _, _, pg::PgVersorgungsStatusRepository>),
        )
}

fn melo_routes() -> Router<S> {
    Router::new()
        .route(
            "/api/v1/melos/{id}",
            get(get_melo::<_, _, _, _, _>).put(put_melo::<_, _, _, _, _>),
        )
        .route(
            "/api/v1/melos/{id}/standorteigenschaften",
            get(get_melo_standorteigenschaften::<_, _, _, _, _>),
        )
        // Per-MeLo dated MSB timeline (WiM Teil 2 UC 4.1.1).
        .route(
            "/api/v1/melos/{melo_id}/msb",
            get(get_melo_msb_at).put(put_melo_msb),
        )
        .route(
            "/api/v1/melos/{melo_id}/msb/history",
            get(get_melo_msb_history),
        )
        .route("/api/v1/melos/{id}/lokationen", get(get_melo_lokationen))
        .route("/api/v1/melos/{melo_id}/zaehler", get(list_zaehler))
        .route(
            "/api/v1/melos/{melo_id}/sharing-eligibility",
            get(get_sharing_eligibility),
        )
}

fn device_routes() -> Router<S> {
    Router::new()
        .route("/api/v1/zaehler/{zaehler_id}", put(put_zaehler))
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
            "/api/v1/zaehler/{zaehler_id}/register",
            get(list_zaehler_register).put(put_zaehler_register),
        )
        .route(
            "/api/v1/zaehler/{zaehler_id}/zaehlzeitdefinitionen",
            get(get_zaehlzeitdefinitionen),
        )
        .route(
            "/api/v1/zaehler/{zaehler_id}/tariff-zone",
            get(get_tariff_zone),
        )
        .route(
            "/api/v1/zaehler-register/{register_id}/saisons",
            get(list_zaehler_saisons).put(put_zaehler_saison),
        )
        .route("/api/v1/geraete/{geraet_id}", put(put_geraet))
        // SteuerbareRessource + the §14a Konfigurationsprodukte processd reads
        // before auto-confirming a wim-steuerungsauftrag ORDERS.
        .route(
            "/api/v1/steuerbare-ressourcen/{sr_id}",
            get(get_steuerbare_ressource).put(put_steuerbare_ressource),
        )
        .route(
            "/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte",
            get(get_konfigurationsprodukte).put(put_konfigurationsprodukte),
        )
        .route(
            "/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte/{produktcode}",
            delete(delete_konfigurationsprodukt),
        )
        .route(
            "/api/v1/technische-ressourcen/{tr_id}",
            get(get_technische_ressource).put(put_technische_ressource),
        )
        .route("/api/v1/lokationszuordnungen", put(put_lokationszuordnung))
        .route(
            "/api/v1/lokationszuordnungen/{von_id}/{nach_id}",
            delete(delete_lokationszuordnung),
        )
}

fn partner_routes() -> Router<S> {
    Router::new()
        .route("/api/v1/partners", get(list_partners::<_, _, _, _, _>))
        .route(
            "/api/v1/partners/{mp_id}",
            get(get_partner::<_, _, _, _, _>).put(put_partner::<_, _, _, _, _>),
        )
        .route(
            "/api/v1/partners/{mp_id}/as4-address",
            get(get_as4_address::<_, _, _, _, _>),
        )
        .route(
            "/api/v1/partners/{mp_id}/marktteilnehmer",
            get(get_partner_marktteilnehmer::<_, _, _, _, _>),
        )
}

fn preisblatt_routes() -> Router<S> {
    Router::new()
        .route(
            "/api/v1/preisblaetter/{nb_mp_id}",
            get(get_preisblatt).put(put_preisblatt),
        )
        .route(
            "/api/v1/preisblaetter-messung/{msb_mp_id}",
            get(get_preisblatt_messung).put(put_preisblatt_messung),
        )
        .route(
            "/api/v1/preisblaetter-ka/{nb_mp_id}",
            get(get_preisblatt_ka).put(put_preisblatt_ka),
        )
        .route(
            "/api/v1/preisblaetter-dienstleistung/{msb_mp_id}",
            get(get_preisblatt_dienstleistung).put(put_preisblatt_dienstleistung),
        )
        .route(
            "/api/v1/preisblaetter-hardware/{msb_mp_id}",
            get(get_preisblatt_hardware).put(put_preisblatt_hardware),
        )
        // MMMA Gas (Trading Hub Europe) and MMM Strom (VNB, GPKE Teil 1 Kap. 8.4).
        .route("/api/v1/mmma-preise/gas", get(mmma_preise::list_mmma_gas))
        .route(
            "/api/v1/mmma-preise/gas/{year}/{month}",
            get(mmma_preise::get_mmma_gas).put(mmma_preise::put_mmma_gas),
        )
        .route(
            "/api/v1/mmm-preise/strom/{year}/{month}",
            get(mmma_preise::get_mmm_strom).put(mmma_preise::put_mmm_strom),
        )
        .route(
            "/api/v1/mmma-preise/import-trigger",
            post(mmma_preise::post_import_trigger),
        )
        // PRICAT version history + manual (re-)dispatch.
        .route("/api/v1/pricat/{nb_mp_id}/history", get(get_pricat_history))
        .route(
            "/api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}",
            get(get_dispatch_log),
        )
        .route(
            "/api/v1/pricat/{nb_mp_id}/dispatch",
            post(post_pricat_dispatch),
        )
}

fn registry_routes() -> Router<S> {
    Router::new()
        .route("/api/v1/nb-contracts", get(list_nb_contracts))
        .route(
            "/api/v1/nb-contracts/by-malo/{malo_id}",
            get(get_nb_contract_by_malo),
        )
        .route(
            "/api/v1/nb-contracts/{id}",
            get(get_nb_contract).put(put_nb_contract),
        )
        // §42 EnWG grid-area Energiemix (LFs disclose the Reststrommix from it).
        .route(
            "/api/v1/energiemix/{nb_mp_id}",
            get(get_nb_energiemix).put(put_nb_energiemix),
        )
        .route(
            "/api/v1/energiemix/{nb_mp_id}/history",
            get(get_nb_energiemix_history),
        )
        // Netz-Element-Lokationen + Tranchen (Redispatch 2.0, GPKE Teil 4).
        .route("/api/v1/nelos", get(list_nelos))
        .route("/api/v1/nelos/{id}", get(get_nelo).put(put_nelo))
        .route("/api/v1/tranchen", get(list_tranchen))
        .route("/api/v1/tranchen/{id}", get(get_tranche).put(put_tranche))
        // MaBiS-Zählpunkt per Bilanzierungsgebiet (read by mabis-syncd).
        .route("/api/v1/mabis-zp", get(list_mabis_zp))
        .route(
            "/api/v1/bilanzierungsgebiete/{eic}/mabis-zp",
            get(get_mabis_zp).put(put_mabis_zp),
        )
        // §36 Abs. 2 EnWG Grundversorger Feststellung (read by the processd EoG
        // gap closure).
        .route(
            "/api/v1/grundversorger/{nb_mp_id}",
            get(get_grundversorger).put(put_grundversorger),
        )
        // §20b EnWG Netzzugangsplattform request registry.
        .route(
            "/api/v1/netzzugang/antraege",
            put(upsert_antrag).get(list_antraege),
        )
        .route("/api/v1/netzzugang/antraege/{id}", get(get_antrag))
        .route(
            "/api/v1/netzzugang/antraege/{id}/status",
            patch(set_antrag_status),
        )
        // Gas MSB-Rahmenvertrag registry (GeLi Gas 3.0 Tenor 13–16).
        .route(
            "/api/v1/msb-rahmenvertraege-gas",
            put(upsert_msb_rv_gas).get(list_msb_rv_gas),
        )
        .route("/api/v1/msb-rahmenvertraege-gas/{id}", get(get_msb_rv_gas))
        .route(
            "/api/v1/correlations",
            get(list_correlations::<_, _, _, _, _>),
        )
        .route(
            "/api/v1/correlations/{id}",
            get(get_correlation::<_, _, _, _, _>),
        )
}

fn esa_routes() -> Router<S> {
    Router::new()
        .route(
            "/api/v1/esa/einwilligungen",
            post(grant_einwilligung).get(list_einwilligungen),
        )
        .route(
            "/api/v1/esa/einwilligungen/{id}",
            get(get_einwilligung).delete(revoke_einwilligung),
        )
        .route(
            "/api/v1/esa/framework/{msb_mp_id}/{esa_mp_id}",
            put(put_framework).get(get_framework),
        )
        // The accepted QUOTES 15003 Angebot — the ESA price basis, since there
        // is no published Preisblatt for Kapitel-4.6 Messprodukte (§35 MsbG).
        // `invoicd` checks INVOIC 31009 positions against it.
        .route(
            "/api/v1/esa/preise/{msb_mp_id}/{esa_mp_id}",
            put(put_esa_preise).get(get_esa_preise),
        )
        // Inbound-message gate: revoked consent / unestablished framework
        // → allowed:false (the Ablehnung clearing case).
        .route("/api/v1/esa/consent-check", get(consent_check))
}

fn subscription_routes() -> Router<S> {
    Router::new()
        .route(
            "/api/v1/subscriptions",
            get(list_subscriptions::<_, _, _, _, _>),
        )
        .route(
            "/api/v1/subscriptions/{id}",
            get(get_subscription::<_, _, _, _, _>)
                .put(put_subscription::<_, _, _, _, _>)
                .delete(delete_subscription::<_, _, _, _, _>),
        )
        .route(
            "/api/v1/subscriptions/{id}/test",
            post(test_subscription::<_, _, _, _, _>),
        )
}

fn admin_routes() -> Router<S> {
    Router::new()
        // Dead-letter queue (§ 147 AO / GoBD: a delivery is never dropped).
        .route("/admin/fanout/dlq", get(list_dlq))
        .route(
            "/admin/fanout/dlq/{event_id}/{subscriber_id}",
            delete(delete_dlq_entry),
        )
        .route(
            "/admin/fanout/dlq/{event_id}/{subscriber_id}/retry",
            post(retry_dlq_entry),
        )
        // Full-envelope CloudEvent replay log.
        .route("/admin/events", get(list_event_log))
}

// ── Wiring ────────────────────────────────────────────────────────────────────

/// Attach the repository handles that handlers pull as `Extension<Arc<Pg…>>`.
///
/// Kept in one function rather than trailing the router so that adding a
/// repository is a one-line edit in a list, not another link in a 30-deep
/// `.layer()` chain whose order carries no meaning.
fn repository_layers(app: Router, pool: &PgPool) -> Router {
    macro_rules! repos {
        ($app:expr, $($repo:ident),+ $(,)?) => {{
            let app = $app;
            $( let app = app.layer(Extension(Arc::new(pg::$repo::new(pool.clone())))); )+
            app
        }};
    }

    repos!(
        app,
        PgPreisblattRepository,
        PgPreisblattMessungRepository,
        PgPreisblattKaRepository,
        PgPreisblattDienstleistungRepository,
        PgPreisblattHardwareRepository,
        PgMmmaPreisGasRepository,
        PgMmmPreisStromRepository,
        PgPriCatRepository,
        PgNbContractRepository,
        PgVersorgungsStatusRepository,
        PgNeLoRepository,
        PgTrancheRepository,
        PgMaloGridRepository,
        PgMabisZpRepository,
        PgGrundversorgerRepository,
        PgMeloMsbRepository,
        PgBilanzierungRepository,
        PgSteuerbareRessourceRepository,
        PgTechnischeRessourceRepository,
        PgLokationszuordnungRepository,
        PgDeviceRepository,
        PgEinwilligungRepository,
        PgNetzzugangRepository,
        PgMsbRahmenvertragGasRepository,
        PgZaehlzeitRepository,
    )
}

/// Start the durable fan-out worker, the MMMA/MMM price import worker, and the
/// `processed_events` retention sweep.
fn spawn_workers(
    cfg: &Arc<Config>,
    pool: &PgPool,
    sub_repo: pg::PgSubscriptionRepository,
    http: &reqwest::Client,
    notify: &Arc<tokio::sync::Notify>,
    shutdown: &tokio_util::sync::CancellationToken,
) {
    spawn_fanout(
        pool.clone(),
        sub_repo,
        http.clone(),
        FanoutConfig {
            delivery_timeout: Duration::from_secs(cfg.webhook.delivery_timeout_secs),
            max_attempts: i16::try_from(cfg.webhook.max_retry_attempts).unwrap_or(i16::MAX),
            ..Default::default()
        },
        Arc::clone(notify),
        shutdown.clone(),
    );

    marktd::mmma_worker::spawn_mmma_worker(
        Arc::new(cfg.mmma_import.clone()),
        http.clone(),
        Arc::new(pg::PgMmmaPreisGasRepository::new(pool.clone())),
        Arc::new(pg::PgMmmPreisStromRepository::new(pool.clone())),
        cfg.markt.tenant.clone(),
        pool.clone(),
        Arc::clone(notify),
        shutdown.clone(),
    );

    marktd::retention::spawn_processed_events_sweep(pool.clone(), shutdown.clone());
    marktd::metrics::spawn_sampler(pool.clone(), shutdown.clone());
}
