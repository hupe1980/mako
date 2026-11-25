use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner as _;
use testcontainers::{ContainerAsync, ImageExt as _};
use testcontainers_modules::postgres::Postgres;

pub type Database = (PgPool, ContainerAsync<Postgres>);

/// The test owns the container for as long as it uses the pool.
pub async fn postgres(max_connections: u32) -> Database {
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("start PostgreSQL");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("PostgreSQL port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(&url)
            .await
        {
            Ok(pool) => return (pool, container),
            // Docker's port forwarder can start accepting connections before it forwards PostgreSQL.
            Err(sqlx::Error::Protocol(_) | sqlx::Error::Io(_))
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!("connect to PostgreSQL on port {port}: {error}"),
        }
    }
}
