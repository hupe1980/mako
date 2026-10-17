//! The router builds the way `mako_service::run` mounts it.
//!
//! The runner's infra routes are merged in exactly as at boot — so a route axum
//! refuses, a merge collision with the runner's `/metrics` or `/health/*`, or a
//! `build` that panics fails a test instead of the container start. This
//! daemon's `build` connects a ledger store of its own from `[database] url`,
//! so it starts against a real PostgreSQL (`just test-accountingd-db`).

use testcontainers::{ContainerAsync, ImageExt as _, runners::AsyncRunner as _};
use testcontainers_modules::postgres::Postgres;

/// A throwaway PostgreSQL and its URL, or `None` when Docker is unavailable.
async fn postgres() -> Option<(ContainerAsync<Postgres>, String, sqlx::PgPool)> {
    let pg = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    #[expect(
        clippy::disallowed_methods,
        reason = "the test connects straight to its throwaway container; pool sizing is a deployment concern"
    )]
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    Some((pg, url, pool))
}

/// Build the router from `cfg` with its `[database] url` pointed at `url`.
async fn build(mut cfg: toml::Value, url: String, pool: sqlx::PgPool) {
    cfg["database"]["url"] = toml::Value::String(url);
    let cfg = cfg
        .try_into()
        .expect("the configuration is an accountingd one");
    let _app = mako_service::build_for_test_on::<accountingd::Accountingd>(cfg, pool)
        .await
        .expect("the router builds");
}

/// `dev/config/accountingd.toml` — what `just dev accountingd` starts — builds a router.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_dev_config_builds_a_router() {
    let Some((_pg, url, pool)) = postgres().await else {
        return;
    };
    let cfg = toml::from_str(include_str!("../../../../dev/config/accountingd.toml"))
        .expect("the dev configuration parses");
    build(cfg, url, pool).await;
}

/// The shipped demo configuration builds a router.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_demo_config_builds_a_router() {
    let Some((_pg, url, pool)) = postgres().await else {
        return;
    };
    let cfg = toml::from_str(include_str!("../../../../demos/o2c/accountingd.toml"))
        .expect("the demo configuration parses");
    build(cfg, url, pool).await;
}
