//! The runner migrates as the schema owner and serves as the runtime role.
//!
//! With `[database] migration_url` set, `Daemon::migrate` runs on a connection
//! of the owning role and the runtime role is granted data access to what it
//! created — so a service can run as a role without `CREATE`, and a table's
//! own append-only trigger is not undone by the role that owns it serving
//! requests.
//!
//! `#[ignore]`d by default (Docker required):
//!
//! ```bash
//! cargo test -p mako-service --test it -- --include-ignored --test-threads=1 migration_role::
//! ```

use std::sync::Arc;

use mako_service::config::DatabaseConfig;
use mako_service::{Daemon, ServiceConfig, ServiceContext};
use testcontainers::ImageExt as _;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

#[derive(serde::Deserialize)]
struct Cfg {
    database: DatabaseConfig,
}

impl ServiceConfig for Cfg {
    fn database(&self) -> Option<&DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        "127.0.0.1:0".to_owned()
    }
}

struct Probe;

impl Daemon for Probe {
    type Config = Cfg;
    const NAME: &'static str = "migration-probe";

    async fn migrate(_cfg: &Cfg, pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::query("CREATE TABLE IF NOT EXISTS probe (id INT)")
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn build(_cfg: Arc<Cfg>, _ctx: ServiceContext) -> anyhow::Result<axum::Router> {
        Ok(axum::Router::new())
    }
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn the_owner_migrates_and_the_runtime_role_is_granted_data_access() {
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("start postgres");
    let port = container.get_host_port_ipv4(5432).await.expect("port");
    let owner_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let app_url = format!("postgres://probe_app:app@127.0.0.1:{port}/postgres");
    #[expect(
        clippy::disallowed_methods,
        reason = "the test connects straight to its throwaway container"
    )]
    let owner = sqlx::PgPool::connect(&owner_url).await.expect("owner");
    sqlx::query("CREATE ROLE probe_app LOGIN PASSWORD 'app'")
        .execute(&owner)
        .await
        .expect("runtime role");

    let cfg: Cfg = serde_json::from_value(serde_json::json!({
        "database": { "url": app_url, "migration_url": owner_url }
    }))
    .expect("config");
    let shutdown = tokio_util::sync::CancellationToken::new();
    shutdown.cancel();
    mako_service::serve::<Probe>(cfg, shutdown)
        .await
        .expect("the runtime role, which may not CREATE, still boots");

    let table_owner: String =
        sqlx::query_scalar("SELECT tableowner FROM pg_tables WHERE tablename = 'probe'")
            .fetch_one(&owner)
            .await
            .expect("probe exists");
    assert_eq!(table_owner, "postgres", "the migration role created it");

    #[expect(
        clippy::disallowed_methods,
        reason = "the test connects straight to its throwaway container"
    )]
    let app = sqlx::PgPool::connect(&app_url).await.expect("app");
    sqlx::query("INSERT INTO probe VALUES (1)")
        .execute(&app)
        .await
        .expect("the runtime role is granted data access");
    assert!(
        sqlx::query("CREATE TABLE app_owned (id INT)")
            .execute(&app)
            .await
            .is_err(),
        "the runtime role has no CREATE in public"
    );
}
