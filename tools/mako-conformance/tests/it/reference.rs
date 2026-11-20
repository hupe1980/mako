//! Every suite holds against its reference service: vertragd, built through
//! its own `Daemon::build` with its subscription worker running, on a real
//! PostgreSQL, behind production OIDC whose principals are service keys.
//!
//! ```bash
//! just test-conformance-db
//! ```
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use std::sync::Arc;
use std::time::Duration;

use mako_conformance::report::Outcome;
use mako_conformance::suite::Target;
use mako_service::Daemon;
use serde_json::json;
use testcontainers::ImageExt as _;
use testcontainers::runners::AsyncRunner as _;

const TENANT: &str = "9900357000004";
const OPERATOR: &str = "conformance-operator-key";
const REFUSED: &str = "conformance-nb-key";
const INBOUND: &str = "conformance-inbound-secret";

/// A discovery document and an empty JWKS: production OIDC on this machine,
/// its principals the service keys.
async fn issuer() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let discovery = json!({ "issuer": base, "jwks_uri": format!("{base}/jwks") });
    let app = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || {
                let d = discovery.clone();
                async move { axum::Json(d) }
            }),
        )
        .route(
            "/jwks",
            axum::routing::get(|| async { axum::Json(json!({ "keys": [] })) }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    base
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn vertragd_keeps_its_own_contract() {
    let Ok(pg) = testcontainers_modules::postgres::Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
    else {
        eprintln!("no Docker — skipped");
        return;
    };
    let url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        pg.get_host_port_ipv4(5432).await.unwrap()
    );
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");

    let issuer = issuer().await;
    let cfg: vertragd::config::VertragdConfig = serde_json::from_value(json!({
        "database": { "url": url },
        "tenant": TENANT,
        "lf_mp_id": TENANT,
        "processd_url": "http://processd.invalid",
        "accountingd_url": "http://accountingd.invalid",
        "edmd_url": "http://edmd.invalid",
        "inbound_secret": INBOUND,
        "oidc": {
            "issuer": issuer,
            "audience": "mako",
            "service_keys": [
                { "name": "operator", "key": OPERATOR, "roles": ["LF", "ADMIN"] },
                { "name": "nb", "key": REFUSED, "roles": ["NB"] },
            ],
        },
        "subscriptions": { "allow_loopback": true },
        // MCP takes the same two principals as named keys.
        "mcp": {
            "named_keys": [
                { "name": "operator", "api_key": OPERATOR, "roles": ["LF", "ADMIN"] },
                { "name": "nb", "api_key": REFUSED, "roles": ["NB"] },
            ],
        },
    }))
    .expect("config");
    vertragd::daemon::Vertragd::migrate(&cfg, &pool)
        .await
        .expect("migrate");
    let ctx = mako_service::ServiceContext {
        pool: Some(pool),
        http: reqwest::Client::new(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        einrichtung: Arc::new(mako_service::einrichtung::Einrichtung::neu(
            "vertragd",
            Vec::new(),
            mako_service::policy::PolicyReport::new(),
            Vec::new(),
        )),
    };
    let router = vertragd::daemon::Vertragd::build(Arc::new(cfg), ctx)
        .await
        .expect("build");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen = probe.local_addr().unwrap().to_string();
    drop(probe);
    let report = mako_conformance::run(
        mako_conformance::suites::suite("vertragd").expect("a vertragd suite"),
        Target {
            base,
            token: OPERATOR.to_owned(),
            refused_token: Some(REFUSED.to_owned()),
            inbound_secret: Some(INBOUND.to_owned()),
            listen,
            callback: None,
            wait: Duration::from_secs(20),
        },
        None,
    )
    .await
    .expect("run");
    println!("{report}");
    assert!(
        report.gaps().is_empty(),
        "vertragd breaks its own contract:\n{report}"
    );
    // The typed event the seed causes is checked against its schema, not
    // merely seen.
    assert!(
        report.checks().iter().any(
            |c| c.name == "event de.mako.vertrag.ersatzversorgung.begonnen"
                && c.outcome == Outcome::Held
        ),
        "{report}"
    );
}
