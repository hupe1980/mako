//! Assembly of the three listening ports.
//!
//! Each `serve_*` function builds its router, binds its socket, and spawns the
//! serving task, returning the `JoinHandle` so the shutdown path can join it
//! before the event store is closed.
//!
//! ## Why these live here
//!
//! They were inline in `async_main`, which made that function long enough that
//! the boot *order* — the part that actually carries meaning, because `--check`
//! exits partway through it — was buried under three hundred lines of state
//! construction. Splitting them out leaves `async_main` reading as the sequence
//! it documents.
//!
//! ## Shared invariants
//!
//! - Health routes are merged **before** any `layer`, so probes are neither
//!   authenticated nor rate-limited (see [`crate::health::is_health_path`]).
//! - `Router::layer` wraps only what is already merged, so every `.layer` call
//!   comes last; merging a route afterwards would leave it untraced.
//! - Every port is served with `into_make_service_with_connect_info`, because
//!   the rate limiters key on the peer address.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context as _;
use edi_energy::Platform;
use mako_engine::{
    ids::TenantId,
    store_slatedb::{SlateDbDeadLetterSink, SlateDbInboxStore, SlateDbStore},
};
use secrecy::{ExposeSecret as _, SecretString};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::{
    api::{
        edifact_api, invoic_api, malo_admin_api, mcp_server, metrics_api, migration_api, openapi,
        partner_api,
    },
    cedar_authz::CedarAuthorizer,
    commands_api, contrl_ack, health, ingest_dispatcher, malo_cache,
    party_registry::MpIdRegistry,
    transport::{as4_ingest, webdienste},
};

/// State every port needs, built once in `async_main` and shared by reference.
pub(crate) struct ServerDeps {
    pub store: SlateDbStore,
    pub pid_router: mako_engine::pid_router::PidRouter,
    pub mp_id_registry: Arc<MpIdRegistry>,
    /// Derived from the primary Marktpartner-ID. `makod` is single-tenant, so
    /// it is computed once here rather than re-derived at each call site.
    pub tenant_id: TenantId,
    pub cedar: Arc<CedarAuthorizer>,
    pub platform: Arc<Platform>,
    pub health_state: health::HealthState,
    pub shutdown_token: CancellationToken,
    pub ingest_dispatcher: Arc<ingest_dispatcher::EdifactIngestDispatcher>,
    /// Durable dead-letter sink. Both ingest paths share it: a message rejected
    /// at the REST boundary is as much a § 147 AO record as one that arrived
    /// over AS4.
    pub dead_letter_sink: SlateDbDeadLetterSink,
}

impl ServerDeps {
    /// A `ContrlAckService` for this tenant.
    fn contrl_ack(&self) -> Arc<contrl_ack::ContrlAckService> {
        Arc::new(contrl_ack::ContrlAckService::new(
            Arc::new(self.store.clone()),
            self.tenant_id,
            Arc::clone(&self.mp_id_registry),
        ))
    }

    /// The ingest state shared by the REST and AS4 EDIFACT entry points.
    ///
    /// `cedar` differs between them — the REST port authenticates its caller,
    /// while the AS4 port has already authenticated the *envelope* via
    /// WS-Security and passes an unauthenticated authorizer.
    fn ingest_state(
        &self,
        cedar: Arc<CedarAuthorizer>,
        max_body_bytes: usize,
        contrl_ack: Option<Arc<contrl_ack::ContrlAckService>>,
    ) -> Arc<edifact_api::EdifactApiState> {
        Arc::new(edifact_api::EdifactApiState {
            platform: Arc::clone(&self.platform),
            pid_router: self.pid_router.clone(),
            mp_id_registry: Arc::clone(&self.mp_id_registry),
            cedar,
            max_body_bytes,
            partner_store: Some(Arc::new(self.store.as_partner_store())),
            tenant_id: self.tenant_id,
            dl_sink: Arc::new(self.dead_letter_sink.clone()),
            dispatcher: Some(Arc::clone(&self.ingest_dispatcher)),
            contrl_ack,
        })
    }
}

/// Bind `addr` and spawn `app` on it, shutting down with the token.
///
/// `name` appears in the error log; it is the only thing that differs between
/// the three otherwise identical serve loops.
async fn bind_and_spawn(
    name: &'static str,
    addr: SocketAddr,
    app: axum::Router,
    shutdown: CancellationToken,
) -> anyhow::Result<JoinHandle<()>> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("{name} server bind {addr}"))?;
    Ok(tokio::spawn(async move {
        // `into_make_service_with_connect_info` supplies the peer address the
        // rate limiters key on.
        if let Err(e) = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        {
            tracing::error!(error = %e, "{name} server error");
        }
    }))
}

// ── HTTP REST API (:8080) ─────────────────────────────────────────────────────

/// Everything the REST port needs beyond [`ServerDeps`].
pub(crate) struct HttpServerConfig<'a> {
    pub addr: SocketAddr,
    pub max_body_bytes: usize,
    pub snapshot_interval: u64,
    /// Marktrollen this instance accepts commands for; a command for any other
    /// role is refused with 422.
    pub marktrollen: Vec<String>,
    pub malo_cache: Arc<malo_cache::SlateDbMaloCache>,
    pub marktd_client: Option<Arc<mako_markt::marktd_client::MarktdClient>>,
    /// `--as4-partner GLN=URL` pairs, bootstrapped into the durable partner
    /// store on first start.
    pub as4_partner: &'a [String],
    /// Surfaced by the metrics API so an operator can tell a volatile
    /// development instance from a durable one.
    pub volatile_mode: bool,
}

/// Build and start the HTTP REST API server.
///
/// This port carries the ERP command gateway, the admin APIs, the MCP endpoint
/// and the OpenAPI spec. It is always authenticated — the preflight refuses to
/// start it without an API key or an OIDC issuer.
pub(crate) async fn serve_http(
    deps: &ServerDeps,
    cfg: HttpServerConfig<'_>,
) -> anyhow::Result<JoinHandle<()>> {
    let primary_mp_id = deps.mp_id_registry.primary_mp_id().to_owned();

    let api_state = deps.ingest_state(
        Arc::clone(&deps.cedar),
        cfg.max_body_bytes,
        Some(deps.contrl_ack()),
    );

    let admin_state = Arc::new(malo_admin_api::MaloAdminState {
        cache: malo_cache::SlateDbMaloCache::new(deps.store.clone()),
        cedar: Arc::clone(&deps.cedar),
        tenant_id: primary_mp_id.clone(),
    });

    let partner_store = deps.store.as_partner_store();
    partner_api::seed_from_config(&partner_store, deps.tenant_id, cfg.as4_partner)
        .await
        .context("seeding partner store from config")?;
    let partner_admin_state = Arc::new(partner_api::PartnerAdminState {
        store: partner_store,
        tenant_id: deps.tenant_id,
        cedar: Arc::clone(&deps.cedar),
        platform: Arc::clone(&deps.platform),
    });

    let commands_state = Arc::new(commands_api::CommandsApiState {
        tenant_id: deps.tenant_id,
        sender_party_id: primary_mp_id.clone(),
        configured_marktrollen: cfg.marktrollen,
        max_body_bytes: cfg.max_body_bytes,
        snapshot_interval: cfg.snapshot_interval,
        cedar: Arc::clone(&deps.cedar),
        store: Arc::new(deps.store.clone()),
        snapshot_store: deps.store.as_snapshot_store(),
        malo_cache: Arc::clone(&cfg.malo_cache),
        maloid_result_cache: malo_cache::MaloIdentResultCache::new(deps.store.clone()),
        // M1: Konfigurationsprodukt guard — enabled when --marktd-url is set.
        // Falls back to `None` (guard disabled) when not configured.
        marktd_client: cfg.marktd_client,
    });

    let metrics_state = Arc::new(
        metrics_api::MetricsState::new(
            deps.store.clone(),
            Arc::clone(&deps.cedar),
            primary_mp_id.clone(),
        )
        .with_volatile_mode(cfg.volatile_mode),
    );

    let migration_state = Arc::new(migration_api::MigrationApiState {
        store: Arc::new(deps.store.clone()),
        cedar: Arc::clone(&deps.cedar),
        tenant: primary_mp_id.clone(),
    });

    let mcp_state = Arc::new(mcp_server::MakodMcpState {
        tenant: primary_mp_id.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        cedar: Arc::clone(&deps.cedar),
        commands: Arc::clone(&commands_state),
        malo_cache: Arc::clone(&cfg.malo_cache),
        partner_store: Arc::new(deps.store.as_partner_store()),
        process_store: Arc::new(deps.store.clone()),
        deadline_store: deps.store.as_deadline_store(),
    });

    let invoic_api_state = Arc::new(invoic_api::InvoicApiState {
        store: Arc::new(deps.store.clone()),
        tenant_id: deps.tenant_id,
        cedar: Arc::clone(&deps.cedar),
        tenant: primary_mp_id,
    });

    let app = edifact_api::router(api_state)
        .merge(malo_admin_api::router(admin_state))
        .merge(partner_api::router(partner_admin_state))
        .merge(commands_api::router(commands_state))
        .merge(invoic_api::router(invoic_api_state))
        .merge(metrics_api::router(metrics_state))
        .merge(migration_api::router(migration_state))
        .merge(mcp_server::router(mcp_state, deps.shutdown_token.clone()))
        .merge(health::router(deps.health_state.clone()))
        .merge(openapi::router())
        // Per-peer rate limit, the same GCRA policy the AS4 and API-Webdienste
        // ports carry. The port is authenticated, but a holder of one valid key
        // could still exhaust the event-store write budget through the command
        // API or the MCP endpoint.
        .layer(axum::middleware::from_fn(as4_ingest::rate_limit_middleware))
        // W3C trace-context capture for end-to-end tracing. `Router::layer`
        // wraps only the routes already merged, so these must come last:
        // anything merged after the trace layer runs untraced.
        .layer(axum::middleware::from_fn(super::trace_ctx_middleware));

    info!(
        addr         = %cfg.addr,
        max_body_mib = cfg.max_body_bytes / (1024 * 1024),
        "HTTP REST API listening",
    );
    bind_and_spawn("HTTP", cfg.addr, app, deps.shutdown_token.clone()).await
}

// ── AS4 inbound (:4080) ───────────────────────────────────────────────────────

/// Everything the AS4 port needs beyond [`ServerDeps`].
///
/// The key material is not `Option` here: the preflight has already refused to
/// start an AS4 listener without it, so this struct can only be built from a
/// configuration that proved usable.
pub(crate) struct As4ServerConfig {
    pub addr: SocketAddr,
    /// `<eb:From>/<eb:PartyId>` — must match the signing certificate's subject
    /// (BDEW AS4-Profil §2.3.2).
    pub party_id: String,
    pub signing_key_pem: SecretString,
    pub signing_cert_pem: String,
    pub trust_anchor_pem: Option<String>,
    pub decryption_key_pem: Option<SecretString>,
    pub inbox_store: SlateDbInboxStore,
    /// `false` in volatile mode: dedup state is not preserved across restarts,
    /// and the asx-rs pipeline is told so rather than left to assume durability.
    pub dedup_is_durable: bool,
}

/// Build and start the AS4/ebMS3 inbound transport.
///
/// The mandatory production transport for BDEW MaKo since 2024-04-01
/// (electricity) / 2025-04-01 (gas).
pub(crate) async fn serve_as4(
    deps: &ServerDeps,
    cfg: As4ServerConfig,
) -> anyhow::Result<JoinHandle<()>> {
    let session = {
        let session_id = format!("makod-{}", uuid::Uuid::new_v4());
        let trust_anchor = cfg
            .trust_anchor_pem
            .clone()
            .unwrap_or_else(|| cfg.signing_cert_pem.clone());
        asx_rs::core::SessionContextBuilder::new(&session_id, &cfg.party_id)
            .with_signing_material(
                cfg.signing_cert_pem.clone(),
                cfg.signing_key_pem.expose_secret(),
            )
            .with_trust_anchor_pem(trust_anchor)
            .build()
            .map_err(|e| anyhow::anyhow!("AS4 SessionContext build failed: {e}"))?
    };

    let event_bus = Arc::new(
        asx_rs::observability::EventBus::new(256)
            .map_err(|e| anyhow::anyhow!("AS4 EventBus init failed: {e}"))?,
    );

    let dedup: Arc<dyn asx_rs::storage::DedupStorage> = Arc::new(
        as4_ingest::SlateDbDedupBridge::new(Arc::new(cfg.inbox_store), cfg.dedup_is_durable),
    );

    // The AS4 envelope is authenticated by WS-Security, not by a bearer token,
    // so the ingest path behind it carries an unauthenticated authorizer.
    let ingest_state = deps.ingest_state(
        Arc::new(
            CedarAuthorizer::unauthenticated()
                .expect("CedarAuthorizer::unauthenticated is infallible"),
        ),
        mako_as4::bdew_router_config().max_body_bytes,
        Some(deps.contrl_ack()),
    );

    let handler = Arc::new(
        as4_ingest::BdewAs4IngestHandler::new(ingest_state, Arc::new(session), event_bus, dedup)
            .with_decryption_key_pem(
                cfg.decryption_key_pem
                    .as_ref()
                    .map(|s| s.expose_secret().as_bytes().to_vec()),
            )
            // BDEW AS4-Profil §2.2.4: sign synchronous receipts (NRR) with the
            // operator's signing key pair — the same material used for the AS4
            // session signing context above.
            .with_receipt_credentials(
                cfg.signing_key_pem.expose_secret().as_bytes().to_vec(),
                cfg.signing_cert_pem.into_bytes(),
            )
            .with_contrl_ack(deps.contrl_ack()),
    );

    let app = as4_ingest::router(handler, mako_as4::bdew_router_config())
        .merge(health::router(deps.health_state.clone()))
        // OWASP A05 — rate limit the AS4 inbound endpoint to prevent capacity
        // exhaustion by a misconfigured or malicious counterparty. Per-peer GCRA
        // token bucket: 100 req/s sustained, burst of 50, keyed by client IP.
        // Returns HTTP 429 when a peer's bucket is exhausted.
        .layer(axum::middleware::from_fn(
            as4_ingest::as4_rate_limit_middleware,
        ))
        // W3C trace-context capture for end-to-end tracing.
        .layer(axum::middleware::from_fn(super::trace_ctx_middleware));

    info!(
        addr     = %cfg.addr,
        party_id = %cfg.party_id,
        "AS4 inbound transport listening (BDEW MaKo mandatory since 2024-04-01)",
    );
    bind_and_spawn("AS4", cfg.addr, app, deps.shutdown_token.clone()).await
}

// ── API-Webdienste Strom (:8090) ──────────────────────────────────────────────

/// Everything the API-Webdienste port needs beyond [`ServerDeps`].
pub(crate) struct WebdiensteServerConfig {
    pub addr: SocketAddr,
    pub max_body_bytes: usize,
    /// Drops the auth layer. Only sound behind a proxy that terminates mTLS
    /// with the BDEW PKI CA and enforces access there.
    pub allow_unauthenticated: bool,
}

/// Build and start the BDEW API-Webdienste Strom server.
///
/// This port carries iMSys Steuerbefehle — the REST/JSON path by which a
/// Steuerbefehl reaches a controllable consumption device — so the BDEW
/// specification requires authenticated access.
pub(crate) async fn serve_webdienste(
    deps: &ServerDeps,
    cfg: WebdiensteServerConfig,
) -> anyhow::Result<JoinHandle<()>> {
    let handler = Arc::new(webdienste::MakodApiHandler {
        store: deps.store.clone(),
        tenant_id: deps.tenant_id,
    });

    let auth = if cfg.allow_unauthenticated {
        tracing::warn!(
            addr = %cfg.addr,
            "--webdienste-allow-unauthenticated: API-Webdienste Strom port has NO \
             authentication. Only acceptable behind a proxy that terminates mTLS \
             with the BDEW PKI CA.",
        );
        None
    } else {
        Some(webdienste::WebdiensteAuthState {
            cedar: Arc::clone(&deps.cedar),
            tenant: Arc::from(deps.mp_id_registry.primary_mp_id()),
        })
    };

    let app = webdienste::build_app(handler, auth, cfg.max_body_bytes)
        .merge(health::router(deps.health_state.clone()))
        // Per-peer rate limit, same GCRA policy as the AS4 port, and W3C
        // trace-context capture. Merged routes first: `Router::layer` wraps only
        // what is already in the router.
        .layer(axum::middleware::from_fn(as4_ingest::rate_limit_middleware))
        .layer(axum::middleware::from_fn(super::trace_ctx_middleware));

    info!(
        addr = %cfg.addr,
        primary_mp_id = deps.mp_id_registry.primary_mp_id(),
        "API-Webdienste Strom server listening (MaLo Identification active)",
    );
    bind_and_spawn("API-Webdienste", cfg.addr, app, deps.shutdown_token.clone()).await
}
