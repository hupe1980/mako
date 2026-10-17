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

/// A further port a daemon serves besides [`ServiceConfig::bind_addr`] — an
/// AS4 endpoint, a partner API — returned by [`Daemon::listeners`].
///
/// The runner binds it, merges `/health/*` into it before any of its own
/// layers (so a probe on that port is neither authenticated nor
/// rate-limited), serves it on the shutdown token every port shares, and joins
/// it with the main port.
pub struct Listener {
    /// What the port is for, as logged at bind.
    pub name: &'static str,
    /// The socket address to bind.
    pub addr: String,
    /// The port's router, with the daemon's own layers already applied.
    pub router: axum::Router,
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

    /// Refuse a config that loads but cannot behave as documented — values
    /// that contradict each other, a removed key, a posture that must be asked
    /// for by name. Default: nothing to refuse.
    ///
    /// The runner calls it from `--print-config` and before [`Daemon::build`]
    /// at boot, from one place, so a config `--print-config` accepts is a
    /// config the daemon starts on.
    ///
    /// # Errors
    ///
    /// Name every problem; the runner refuses to start.
    fn validate(_cfg: &Self::Config) -> anyhow::Result<()> {
        Ok(())
    }

    /// The operator policy and posture this config runs under. Default: none.
    ///
    /// The runner logs it at startup (`info` per setting, `warn` per insecure
    /// posture and per setting past its review date), exports it as
    /// `mako_policy_info`, prints it from `--print-config` and, for a daemon
    /// with a database, appends every changed value to `service_policy_log`.
    /// A daemon that reports a policy setting serves the same report at
    /// `GET /api/v1/policy` through [`crate::policy::router`].
    fn policy(_cfg: &Self::Config) -> crate::policy::PolicyReport {
        crate::policy::PolicyReport::default()
    }

    /// Further ports besides [`ServiceConfig::bind_addr`], built after
    /// [`Daemon::build`]. Default: none.
    ///
    /// # Errors
    ///
    /// Return an error to abort startup.
    fn listeners(
        _cfg: Arc<Self::Config>,
        _ctx: ServiceContext,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<Listener>>> + Send {
        async { Ok(Vec::new()) }
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
    // Handled before tracing, config or a port: `--help` must print on a
    // machine with no config at all, and `--check` is the container
    // HEALTHCHECK, which binds nothing.
    match Mode::parse(std::env::args().skip(1)) {
        Ok(Mode::Help) => {
            print!("{}", usage(D::NAME));
            return Ok(());
        }
        Ok(Mode::PrintConfig) => return print_config::<D>(),
        Ok(Mode::Check) => return health_check::<D>().await,
        Ok(Mode::Serve) => {}
        Err(arg) => {
            eprint!("{}", usage(D::NAME));
            anyhow::bail!("unknown argument {arg:?}");
        }
    }

    // Keep the tracing/OTel guard alive until serve returns (dropping it flushes
    // spans), so bind it to a name that lives for the whole function.
    let _guard = crate::init_tracing_from_env_with(D::NAME, D::tracing_layer());

    let cfg: D::Config = crate::load_config(D::NAME)?;
    serve::<D>(cfg, crate::shutdown::token()).await
}

/// Everything [`run`] does once the config is loaded: validate, report, pool,
/// migrations, `service_policy_log`, build, bind every port, serve until
/// `shutdown` is cancelled.
///
/// # Errors
///
/// As [`run`].
pub async fn serve<D: Daemon>(cfg: D::Config, shutdown: CancellationToken) -> anyhow::Result<()> {
    let report = validate_config::<D>(&cfg)?;
    report.log(D::NAME, time::OffsetDateTime::now_utc());
    crate::policy::export_metric(D::NAME, &report);
    let cfg = Arc::new(cfg);

    // Stateless daemons have no `[database]`: skip pool + migrations entirely.
    let pool: Option<PgPool> = if let Some(db) = cfg.database() {
        let url = crate::config::resolve_env(&db.url).context("resolve database url")?;
        let pool = db
            .connect(&url, D::NAME)
            .await
            .context("connect database pool")?;
        D::migrate(&pool).await.context("run migrations")?;
        if !report.is_empty() {
            crate::policy::ensure_schema(&pool)
                .await
                .context("ensure service_policy_log schema")?;
            let changes = crate::policy::record(&pool, D::NAME, &report)
                .await
                .context("record the policy report in service_policy_log")?;
            crate::policy::log_changes(D::NAME, &changes);
        }
        Some(pool)
    } else {
        None
    };

    let ctx = ServiceContext {
        pool,
        http: crate::http::default_client(),
        shutdown: shutdown.clone(),
    };
    // Keep a clone for the readiness probe and the further ports; `build`
    // consumes `ctx`.
    let ready_ctx = ctx.clone();

    let domain = D::build(Arc::clone(&cfg), ctx)
        .await
        .context("build service router")?;
    let extra = D::listeners(Arc::clone(&cfg), ready_ctx.clone())
        .await
        .context("build further listeners")?;

    let app = mount::<D>(domain, ready_ctx.clone());

    let mut ports = Vec::with_capacity(1 + extra.len());
    let addr = cfg.bind_addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    ports.push(("http", listener, app));
    for l in extra {
        let listener = tokio::net::TcpListener::bind(&l.addr)
            .await
            .with_context(|| format!("bind {} ({})", l.addr, l.name))?;
        ports.push((
            l.name,
            listener,
            mount_listener::<D>(l.router, ready_ctx.clone()),
        ));
    }
    serve_all(ports, shutdown).await
}

/// Serve every port on one shutdown token and join them all.
///
/// A port that fails cancels the token, so the others drain and stop rather
/// than leaving a daemon that answers on half its ports; the first error is
/// returned once every port has stopped.
async fn serve_all(
    ports: Vec<(&'static str, tokio::net::TcpListener, axum::Router)>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let mut tasks = tokio::task::JoinSet::new();
    for (name, listener, app) in ports {
        let ct = shutdown.clone();
        tasks.spawn(async move {
            let r = crate::shutdown::serve(listener, app, ct.clone())
                .await
                .with_context(|| format!("serve the {name} port"));
            if r.is_err() {
                ct.cancel();
            }
            r
        });
    }
    let mut first_err = None;
    while let Some(joined) = tasks.join_next().await {
        let r = joined.map_err(anyhow::Error::from).and_then(|r| r);
        if let Err(e) = r
            && first_err.is_none()
        {
            first_err = Some(e);
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// [`Daemon::validate`], then [`Daemon::policy`] — what `--print-config` and
/// boot both run, from this one function, so the two cannot disagree.
///
/// # Errors
///
/// Whatever [`Daemon::validate`] refuses.
pub fn validate_config<D: Daemon>(cfg: &D::Config) -> anyhow::Result<crate::policy::PolicyReport> {
    D::validate(cfg).with_context(|| format!("{} config is invalid", D::NAME))?;
    Ok(D::policy(cfg))
}

/// A further port as [`run`] serves it: `/health/*` merged before the
/// runner's trace layer, with the daemon's router — and its layers — beside.
fn mount_listener<D: Daemon>(router: axum::Router, ready_ctx: ServiceContext) -> axum::Router {
    crate::ServiceBuilder::new()
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
        .merge(router)
        .with_trace_layer()
        .build()
}

/// The daemon's router as [`run`] serves it: the infra routes (health,
/// metrics, tracing) with the domain router merged in.
///
/// One function for [`run`] and [`build_for_test`], so a startup test merges
/// exactly what production merges — a daemon registering `/metrics` or
/// `/health/*` collides with the runner here, in the test, rather than at boot.
fn mount<D: Daemon>(domain: axum::Router, ready_ctx: ServiceContext) -> axum::Router {
    // Real readiness: a fresh DB ping (bounded, so a stuck pool fails fast) when
    // the daemon has a database, plus the daemon's own check — never `|| true`.
    crate::ServiceBuilder::new()
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
        .build()
}

/// Build `D`'s router the way [`run`] does — [`Daemon::build`], then the infra
/// routes — without binding a port or reaching a database.
///
/// The startup test every daemon carries. A route axum refuses (a `/:id`
/// capture under axum 0.8), a merge collision with the runner's `/metrics` or
/// `/health/*`, or a `build` that panics on its own config all fail here, in
/// `cargo test`, instead of when the container starts.
///
/// A daemon with a database gets a **lazy** pool on an address nothing listens
/// on: `build` may hand it to workers and routers, and a statement executed
/// against it fails fast instead of connecting. The shutdown token is cancelled
/// once the router is built, so the workers `build` spawned stop.
///
/// # Errors
///
/// Whatever [`Daemon::build`] refuses — a config that must not start.
#[cfg(feature = "testing")]
pub async fn build_for_test<D: Daemon>(cfg: D::Config) -> anyhow::Result<axum::Router> {
    let report = validate_config::<D>(&cfg)?;
    crate::policy::export_metric(D::NAME, &report);
    let pool = match cfg.database() {
        Some(_) => Some(
            sqlx::postgres::PgPoolOptions::new()
                .acquire_timeout(std::time::Duration::from_millis(200))
                .connect_lazy("postgres://startup-test@127.0.0.1:1/startup-test")
                .context("lazy pool")?,
        ),
        None => None,
    };
    build_with::<D>(cfg, pool).await
}

/// [`build_for_test`] against a real database: [`Daemon::migrate`] runs on
/// `pool` first, as at boot.
///
/// For a daemon whose `build` itself reaches the database — one that connects
/// a store of its own from `[database] url` — and therefore cannot start on a
/// lazy pool. Point `cfg`'s `[database] url` at the same database.
///
/// # Errors
///
/// A failed migration, or whatever [`Daemon::build`] refuses.
#[cfg(feature = "testing")]
pub async fn build_for_test_on<D: Daemon>(
    cfg: D::Config,
    pool: PgPool,
) -> anyhow::Result<axum::Router> {
    let report = validate_config::<D>(&cfg)?;
    crate::policy::export_metric(D::NAME, &report);
    D::migrate(&pool).await.context("run migrations")?;
    build_with::<D>(cfg, Some(pool)).await
}

#[cfg(feature = "testing")]
async fn build_with<D: Daemon>(
    cfg: D::Config,
    pool: Option<PgPool>,
) -> anyhow::Result<axum::Router> {
    let shutdown = CancellationToken::new();
    let ctx = ServiceContext {
        pool,
        http: crate::http::default_client(),
        shutdown: shutdown.clone(),
    };
    let ready_ctx = ctx.clone();
    let domain = D::build(Arc::new(cfg), ctx).await;
    shutdown.cancel();
    Ok(mount::<D>(domain?, ready_ctx))
}

/// What the command line asks the runner to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// No argument: start the daemon.
    Serve,
    /// `--check`: probe the running instance's readiness.
    Check,
    /// `-h` / `--help`: print usage, read nothing.
    Help,
    /// `--print-config`: print the merged config layers, secrets redacted, check
    /// them as boot does, and print the policy report.
    PrintConfig,
}

impl Mode {
    /// The first recognised flag wins; an unrecognised one is refused rather
    /// than ignored, so `--chek` does not start a daemon.
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut mode = Self::Serve;
        for arg in args {
            let next = match arg.as_str() {
                "-h" | "--help" => Self::Help,
                "--check" => Self::Check,
                "--print-config" => Self::PrintConfig,
                _ => return Err(arg),
            };
            if mode == Self::Serve {
                mode = next;
            }
        }
        Ok(mode)
    }
}

/// The `--help` text: the flags, where the config comes from, and the env
/// prefix — everything needed to write a first config without reading source.
fn usage(name: &str) -> String {
    let prefix = crate::config::env_prefix(name);
    format!(
        "{name} — a mako service daemon

Usage: {name} [--check | --print-config | --help]

Options:
  (none)          start the daemon
  --check         probe the running instance's /health/ready and exit 0 when ready
  --print-config  print the merged file + environment config (secrets redacted),
                  report every missing, unknown or invalid key, run the
                  daemon's own validation and print the policy it runs under
  -h, --help      print this text

Configuration:
  file            ${prefix}CONFIG, else ./{name}.toml (absent: environment only)
  environment     {prefix}<KEY> overrides a key; nested keys join with `__`
                  ({prefix}DATABASE__URL sets [database] url)
  secrets         {prefix}<KEY>_FILE reads the value from a file;
                  a value \"env:VAR\" in the file reads it from VAR
  logging         {prefix}LOG_LEVEL, {prefix}LOG_FORMAT, RUST_LOG
"
    )
}

/// The `--print-config` path: the merged layers as TOML, then the same checks
/// startup runs — loading, [`Daemon::validate`] — and the policy report, so a
/// config can be validated without a database.
///
/// # Errors
///
/// Returns the [`ConfigError`](crate::ConfigError) when the config would not
/// load.
fn print_config<D: Daemon>() -> anyhow::Result<()> {
    print!("{}", crate::config::render_config(D::NAME, None)?);
    let cfg: D::Config = crate::load_config(D::NAME)?;
    let report = validate_config::<D>(&cfg)?;
    if !report.is_empty() {
        println!("\n# The policy and posture this config runs under:");
        for line in report.lines(D::NAME) {
            println!("# {line}");
        }
    }
    Ok(())
}

/// The `--check` path: GET the running instance's `/health/ready` on loopback and
/// exit 0 (ready) or non-zero — the distroless-friendly container HEALTHCHECK
/// (no shell/curl in the image).
///
/// # Errors
///
/// Returns `Err` when the config cannot load or the instance is not ready.
async fn health_check<D: Daemon>() -> anyhow::Result<()> {
    let cfg: D::Config = crate::load_config(D::NAME)?;
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

#[cfg(test)]
mod mode_tests {
    use super::{Mode, usage};

    fn parse(args: &[&str]) -> Result<Mode, String> {
        Mode::parse(args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn no_argument_serves() {
        assert_eq!(parse(&[]), Ok(Mode::Serve));
    }

    #[test]
    fn help_is_recognised_in_both_spellings() {
        assert_eq!(parse(&["--help"]), Ok(Mode::Help));
        assert_eq!(parse(&["-h"]), Ok(Mode::Help));
    }

    #[test]
    fn check_and_print_config_are_recognised() {
        assert_eq!(parse(&["--check"]), Ok(Mode::Check));
        assert_eq!(parse(&["--print-config"]), Ok(Mode::PrintConfig));
    }

    /// A typo must not start the daemon as though no flag were given.
    #[test]
    fn an_unknown_argument_is_refused() {
        assert_eq!(parse(&["--chek"]), Err("--chek".to_owned()));
    }

    #[test]
    fn usage_names_the_file_and_the_env_prefix() {
        let text = usage("netzbilanzd");
        assert!(text.contains("NETZBILANZD_CONFIG"), "{text}");
        assert!(text.contains("./netzbilanzd.toml"), "{text}");
        assert!(text.contains("NETZBILANZD_DATABASE__URL"), "{text}");
    }
}

#[cfg(test)]
mod listener_tests {
    use std::sync::Arc;

    use axum::Router;
    use axum::http::StatusCode;

    use super::{Daemon, Listener, ServiceConfig, ServiceContext};

    #[derive(serde::Deserialize)]
    struct Cfg {
        main: String,
        extra: String,
    }
    impl ServiceConfig for Cfg {
        fn database(&self) -> Option<&crate::config::DatabaseConfig> {
            None
        }
        fn bind_addr(&self) -> String {
            self.main.clone()
        }
    }

    struct TwoPorts;
    impl Daemon for TwoPorts {
        type Config = Cfg;
        const NAME: &'static str = "two-ports";
        async fn build(_cfg: Arc<Cfg>, _ctx: ServiceContext) -> anyhow::Result<Router> {
            Ok(Router::new())
        }
        async fn listeners(cfg: Arc<Cfg>, _ctx: ServiceContext) -> anyhow::Result<Vec<Listener>> {
            // The daemon's own layer refuses every request it wraps: a health
            // probe that answers 200 on this port was merged outside it.
            let router = Router::new()
                .route("/api/x", axum::routing::get(|| async { "x" }))
                .layer(axum::middleware::from_fn(
                    |_req: axum::extract::Request, _next: axum::middleware::Next| async {
                        axum::response::IntoResponse::into_response(StatusCode::UNAUTHORIZED)
                    },
                ));
            Ok(vec![Listener {
                name: "partner",
                addr: cfg.extra.clone(),
                router,
            }])
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// M66 T016: a second listener is bound, served with an unauthenticated
    /// `/health/live`, and stops with the main port on shutdown.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_declared_listener_is_served_with_health_outside_its_layers() {
        let (main, extra) = (free_port(), free_port());
        let cfg = Cfg {
            main: format!("127.0.0.1:{main}"),
            extra: format!("127.0.0.1:{extra}"),
        };
        let ct = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn(super::serve::<TwoPorts>(cfg, ct.clone()));
        let client = reqwest::Client::new();
        let get = |port: u16, path: &str| {
            let url = format!("http://127.0.0.1:{port}{path}");
            let client = client.clone();
            async move {
                for _ in 0..100 {
                    if let Ok(r) = client.get(&url).send().await {
                        return r.status().as_u16();
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                panic!("{url} never answered");
            }
        };
        assert_eq!(get(extra, "/health/live").await, 200);
        assert_eq!(get(extra, "/api/x").await, 401);
        assert_eq!(get(main, "/health/live").await, 200);
        ct.cancel();
        task.await.unwrap().unwrap();
    }
}
