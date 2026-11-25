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

use mako_service::Daemon;
use serde_json::json;
use testcontainers::ImageExt as _;
use testcontainers::runners::AsyncRunner as _;

const TENANT: &str = "conformance-stadtwerk";
const LF_MP_ID: &str = "9900357000004";
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

fn historical_examples() -> std::collections::BTreeMap<String, serde_json::Value> {
    serde_json::from_str(include_str!("fixtures/vertragd-history.json")).unwrap()
}

/// A schema that admits every JSON object would pass the positive replay. Each
/// contract must refuse a payload missing its contract or order identity.
#[test]
fn the_historical_examples_need_an_identity_and_a_published_shape() {
    let mut examples = historical_examples();
    for (ty, text) in mako_conformance::documents::event_schemas("vertragd") {
        if ty == mako_events::vertrag::ERSATZVERSORGUNG_BEGONNEN {
            continue;
        }
        let mut example = examples
            .remove(ty)
            .expect("historical example for this type");
        let schema: serde_json::Value = serde_json::from_str(text).unwrap();
        let validator = jsonschema::draft202012::new(&schema).unwrap();
        assert!(
            validator.is_valid(&example),
            "invalid reference fixture for {ty}"
        );
        let fields = example.as_object_mut().unwrap();
        let identity = if fields.contains_key("bestellung_id") {
            "bestellung_id"
        } else {
            "vertrag_id"
        };
        fields.remove(identity);
        assert!(
            !validator.is_valid(&example),
            "{ty} admits a missing {identity}"
        );
    }
    assert!(examples.is_empty(), "fixture with no published schema");
}

/// Historical deliveries on an isolated reference target. The public seed
/// produces Ersatzversorgung itself. Every other event must survive replay
/// of its independently authored example through the production subscriber.
async fn seed_history(pool: &sqlx::PgPool) {
    let mut tx = pool.begin().await.unwrap();
    for (event_type, data) in historical_examples() {
        assert_ne!(event_type, mako_events::vertrag::ERSATZVERSORGUNG_BEGONNEN);
        let (subject, correlation) = if data.get("bestellung_id").is_some() {
            (&data["bestellreferenz"], &data["bestellung_id"])
        } else {
            (&data["vertrag_id"], &data["vertrag_id"])
        };
        let event = mako_service::CloudEvent::new(
            mako_service::source("vertragd", TENANT),
            &event_type,
            subject.as_str().unwrap().to_owned(),
            data.clone(),
        )
        .with_dataschema(mako_service::cloudevent::schema_url(&event_type))
        .extension("tenantid", TENANT)
        .extension("correlationid", correlation.as_str().unwrap());
        mako_service::outbox::enqueue(&mut tx, &event)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn vertragd_keeps_its_own_contract() {
    let pg = testcontainers_modules::postgres::Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("the explicit database gate requires PostgreSQL in Docker");
    let url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        pg.get_host_port_ipv4(5432).await.unwrap()
    );
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");

    let issuer = issuer().await;
    let cfg: vertragd::config::VertragdConfig = serde_json::from_value(json!({
        "database": { "url": url },
        "tenant": TENANT,
        "lf_mp_id": LF_MP_ID,
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
    seed_history(&pool).await;
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
    let directory = tempfile::tempdir().unwrap();
    let report = directory.path().join("reference.json");
    // Replay can be queued just after the seed notification has drained.
    // Allow the production worker its 30-second polling interval.
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_mako-conformance"))
        .args([
            "vertragd",
            &base,
            "--token",
            OPERATOR,
            "--refused-token",
            REFUSED,
            "--inbound-secret",
            INBOUND,
            "--listen",
            &listen,
            "--wait",
            "45",
            "--acceptance",
            "--target-build",
            "postgres-reference",
            "--report",
        ])
        .arg(&report)
        .output()
        .await
        .expect("run acceptance executable");
    let summary = String::from_utf8(output.stdout).unwrap();
    let evidence: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&report).expect("acceptance must produce its JSON report"),
    )
    .unwrap();
    let blockers: Vec<_> = evidence["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["state"] == "unknown" || c["state"] == "broken")
        .collect();
    assert_eq!(output.status.code(), Some(0), "{summary}\n{blockers:?}");
    assert!(summary.starts_with("PASSED  vertragd"), "{summary}");
    assert_eq!(evidence["verdict"], "passed");
    let counts = &evidence["counts"];
    assert!(counts["required"].as_u64().unwrap() > 0);
    assert_eq!(counts["held"], counts["required"]);
    assert_eq!(counts["broken"], 0);
    assert_eq!(counts["unknown"], 0);
    println!("{summary}");
}
