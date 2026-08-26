//! The daemon lifecycle runner — the SDK owns startup → serve → shutdown so a
//! service `main` shrinks to *config type + routes + workers*.
//!
//! Every mako daemon repeats the same spine: init tracing, load config, connect
//! a tuned `PostgreSQL` pool, run migrations, wire a shutdown token, assemble the
//! infra routes (health / metrics / tracing), bind, and serve with graceful
//! shutdown. [`run`] owns all of it; a daemon implements [`Daemon`] (its config
//! type, name, how to build its domain router + workers) and its `main` becomes:
//!
//! ```rust,no_run
//! # use mako_service::service::{Daemon, ServiceConfig, ServiceContext};
//! # use mako_service::config::DatabaseConfig;
//! # use axum::Router;
//! # #[derive(serde::Deserialize)]
//! # struct MyConfig { database: DatabaseConfig, http_addr: String }
//! # impl ServiceConfig for MyConfig {
//! #     fn database(&self) -> Option<&DatabaseConfig> { Some(&self.database) }
//! #     fn bind_addr(&self) -> String { self.http_addr.clone() }
//! # }
//! struct MyService;
//! impl Daemon for MyService {
//!     type Config = MyConfig;
//!     const NAME: &'static str = "myservice";
//!
//!     async fn build(cfg: std::sync::Arc<MyConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
//!         // spawn workers on ctx.shutdown; build the domain router with ctx.pool …
//!         Ok(Router::new())
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     mako_service::service::run::<MyService>().await
//! }
//! ```
//!
//! What the runner does NOT own is deliberately left to [`Daemon::build`]:
//! domain routes, background workers, and any service-specific wiring (OIDC
//! verifier, MCP auth, event-bus). Those need the daemon's own config and state,
//! so they belong with the daemon — the runner owns only the universal spine.

use std::sync::Arc;

use anyhow::Context as _;
use serde::de::DeserializeOwned;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::config::DatabaseConfig;

/// A daemon's config exposes the shared blocks the runner needs.
///
/// Implement it on the service's own `Config` struct (which embeds
/// [`DatabaseConfig`] under `[database]` and carries a bind address), so the
/// runner can build the pool and bind the listener without knowing the rest of
/// the config shape.
pub trait ServiceConfig: DeserializeOwned + Send + Sync + 'static {
    /// The `[database]` block (URL + pool tuning), or `None` for a stateless
    /// daemon (no pool is connected, no migrations run, readiness is
    /// [`Daemon::ready`] alone).
    fn database(&self) -> Option<&DatabaseConfig>;
    /// The socket address to bind, e.g. `"0.0.0.0:8580"` — owned so a service may
    /// format it from a port field.
    fn bind_addr(&self) -> String;
}

/// Resolved infrastructure handed to [`Daemon::build`]. `http` and the (optional)
/// pool are cheap to clone (`Arc`-backed); `shutdown` is the token every
/// background worker should observe.
#[derive(Clone)]
pub struct ServiceContext {
    /// The tuned `PostgreSQL` pool (`application_name` = the service name), or
    /// `None` for a stateless daemon. Use [`ServiceContext::pool`] when the
    /// daemon has a database.
    pub pool: Option<PgPool>,
    /// The shared inter-service HTTP client (timeouts + no-redirect SSRF guard).
    pub http: reqwest::Client,
    /// Cancelled on SIGINT/SIGTERM — pass `.clone()` to every worker.
    pub shutdown: CancellationToken,
}

impl ServiceContext {
    /// The pool, for a daemon that has a database.
    ///
    /// # Panics
    ///
    /// Panics if this daemon's [`ServiceConfig::database`] returned `None` — a
    /// DB-less daemon must not reach for a pool.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        self.pool
            .as_ref()
            .expect("ServiceContext::pool(): this daemon's ServiceConfig::database() is None")
    }
}

/// A mako daemon. The SDK ([`run`]) owns the lifecycle; the implementor supplies
/// the config type, the service name, and how to build the domain router +
/// workers from resolved infrastructure.
pub trait Daemon: Send + 'static {
    /// The service's config type (implements [`ServiceConfig`]).
    type Config: ServiceConfig;

    /// Service name — the config prefix (`NAME.toml` / `NAME_…` env vars), the
    /// tracing target, and the pool's `application_name`.
    const NAME: &'static str;

    /// Apply schema (`sqlx::migrate!`, `outbox::ensure_schema`, …) to the fresh
    /// pool before the router is built. Only called when the daemon has a
    /// database. Default: nothing.
    ///
    /// # Errors
    ///
    /// Return an error to abort startup (a service that cannot migrate must not
    /// start serving).
    fn migrate(_pool: &PgPool) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async { Ok(()) }
    }

    /// An extra readiness check beyond the built-in DB ping (a queue depth, a
    /// downstream probe via `ctx.http`, …). Default: always ready. For a daemon
    /// with a database, `/health/ready` returns 200 only when the DB ping **and**
    /// this both pass; for a stateless daemon this is the sole readiness signal.
    fn ready(_ctx: &ServiceContext) -> impl std::future::Future<Output = bool> + Send {
        async { true }
    }

    /// One extra `tracing` layer, installed before the filter and the formatter.
    ///
    /// The seam for a daemon that embeds a library which *emits* metrics without
    /// choosing an exporter. `agentplane` publishes its whole instrument
    /// catalogue as `tracing` events on a dedicated target and leaves the bridge
    /// to its embedder — so `agentd` returns a layer here that turns those
    /// events into Prometheus series on the same registry `GET /metrics` serves.
    /// Without it the plane's counters are emitted and collected by nobody,
    /// which reads on a dashboard exactly like a plane where nothing happens.
    ///
    /// Called once, before the config is loaded, so it takes no arguments and
    /// must not need any. Default: no extra layer.
    fn tracing_layer() -> Option<crate::ExtraLayer> {
        None
    }

    /// Build the domain [`axum::Router`] and spawn background workers on
    /// `ctx.shutdown`. The runner merges this with the infra routes (health,
    /// metrics, tracing) — do not add those here.
    ///
    /// # Errors
    ///
    /// Return an error to abort startup.
    fn build(
        cfg: Arc<Self::Config>,
        ctx: ServiceContext,
    ) -> impl std::future::Future<Output = anyhow::Result<axum::Router>> + Send;
}

/// Run a [`Daemon`]'s full lifecycle: init tracing, load config, connect a tuned
/// pool (with `application_name`), migrate, build the router + workers, mount the
/// infra routes, and serve with graceful SIGINT/SIGTERM shutdown.
///
/// # Errors
///
/// Propagates any startup failure (config, pool connect, migration, bind) and
/// the final serve error.
pub async fn run<D: Daemon>() -> anyhow::Result<()> {
    // `--check`: probe the already-running instance's readiness and exit — the
    // container HEALTHCHECK entrypoint. Handled before anything else so it neither
    // starts tracing nor binds a port.
    if std::env::args().skip(1).any(|a| a == "--check") {
        return health_check::<D>().await;
    }

    // Keep the tracing/OTel guard alive until serve returns (dropping it flushes
    // spans), so bind it to a name that lives for the whole function.
    let _guard = crate::init_tracing_from_env_with(D::NAME, D::tracing_layer());

    let cfg: D::Config =
        crate::load_config(D::NAME).with_context(|| format!("load {} config", D::NAME))?;
    let cfg = Arc::new(cfg);

    // Stateless daemons have no `[database]`: skip pool + migrations entirely.
    let pool: Option<PgPool> = if let Some(db) = cfg.database() {
        let url = crate::config::resolve_env(&db.url).context("resolve database url")?;
        let pool = db
            .connect(&url, D::NAME)
            .await
            .context("connect database pool")?;
        D::migrate(&pool).await.context("run migrations")?;
        Some(pool)
    } else {
        None
    };

    let shutdown = crate::shutdown::token();
    let ctx = ServiceContext {
        pool,
        http: crate::http::default_client(),
        shutdown: shutdown.clone(),
    };
    // Keep a clone for the readiness probe; `build` consumes `ctx`.
    let ready_ctx = ctx.clone();

    let domain = D::build(Arc::clone(&cfg), ctx)
        .await
        .context("build service router")?;

    // Real readiness: a fresh DB ping (bounded, so a stuck pool fails fast) when
    // the daemon has a database, plus the daemon's own check — never `|| true`.
    let app = crate::ServiceBuilder::new()
        .with_health(move || {
            let ctx = ready_ctx.clone();
            async move {
                let db_ok = match &ctx.pool {
                    Some(pool) => db_ready(pool).await,
                    None => true,
                };
                db_ok && D::ready(&ctx).await
            }
        })
        .with_trace_layer()
        .with_metrics()
        .merge(domain)
        .build();

    let addr = cfg.bind_addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    crate::shutdown::serve(listener, app, shutdown).await
}

/// The `--check` path: GET the running instance's `/health/ready` on loopback and
/// exit 0 (ready) or non-zero — the distroless-friendly container HEALTHCHECK
/// (no shell/curl in the image).
///
/// # Errors
///
/// Returns `Err` when the config cannot load or the instance is not ready.
async fn health_check<D: Daemon>() -> anyhow::Result<()> {
    let cfg: D::Config =
        crate::load_config(D::NAME).with_context(|| format!("load {} config", D::NAME))?;
    // Probe loopback regardless of the bind interface (0.0.0.0 → 127.0.0.1).
    let port = cfg
        .bind_addr()
        .rsplit(':')
        .next()
        .unwrap_or("8080")
        .to_owned();
    let url = format!("http://127.0.0.1:{port}/health/ready");
    let resp = crate::http::default_client()
        .get(&url)
        .send()
        .await
        .context("health-check request")?;
    anyhow::ensure!(
        resp.status().is_success(),
        "not ready: HTTP {}",
        resp.status()
    );
    Ok(())
}

/// A bounded `SELECT 1` liveness ping against the pool. `false` (not a hang) when
/// the DB is unreachable within 2 s, so a dead-DB pod is marked `NotReady` and
/// pulled from rotation instead of accepting traffic it cannot serve.
async fn db_ready(pool: &PgPool) -> bool {
    let ping = sqlx::query("SELECT 1").execute(pool);
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), ping).await,
        Ok(Ok(_))
    )
}
