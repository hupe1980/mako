//! The MCP round trips every mako server is held to, against this service's
//! `/mcp` as the runner mounts it: the declared revisions on `server/discover`,
//! requests on 2026-07-28 and on 2025-11-25 without a session, `Mcp-Name`
//! read, and a revision outside the declared set refused
//! (`mako_service::mcp::conformance::round_trips`).

use mako_service::Daemon as _;

type Served = billingd::daemon::Billingd;

fn demo_config() -> billingd::config::BillingdConfig {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../deploy/demo")
        .join(format!("{}.toml", Served::NAME));
    mako_service::load_config_file::<<Served as mako_service::Daemon>::Config>(Served::NAME, &path)
        .expect("the demo configuration loads")
}

#[tokio::test]
async fn the_mcp_server_passes_the_round_trips() {
    let cfg = demo_config();
    let app = mako_service::build_for_test::<Served>(cfg)
        .await
        .expect("the router builds");
    mako_service::mcp::conformance::round_trips(app, "/mcp", "get_policy").await;
}

/// The same LF evidence and Cedar action back both transports (SR-056, SR-115).
pub(super) async fn check_lf_watch(pool: sqlx::PgPool) {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use mako_service::{mcp::conformance::Endpoint, oidc::demo};
    use serde_json::{Value, json};
    use tower::ServiceExt as _;

    let mut cfg = demo_config();
    cfg.vertragd_url = Some("http://127.0.0.1:1".into());
    cfg.mcp
        .named_keys
        .push(mako_service::mcp_auth::McpAuthNamedKey {
            name: "metering-reader".into(),
            api_key: "fixture-msb-key".into(),
            roles: vec!["MSB".into()],
        });
    let tenant = cfg.tenant.clone();
    let app = mako_service::build_for_test_on::<Served>(cfg, pool.clone())
        .await
        .unwrap();
    billingd::pg::record_unbilled_period(
        &pool,
        &billingd::pg::NewUnbilledPeriod {
            tenant: &tenant,
            lf_mp_id: "9900357000028",
            malo_id: "51238696012",
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            final_invoice: false,
            reason_code: "RATE_REFUSED",
            due_on: time::macros::date!(2026 - 03 - 14),
        },
    )
    .await
    .unwrap();
    let mcp = Endpoint::serve(app.clone(), "/mcp").await;
    for (route, tool) in [
        ("/api/v1/billing/unbilled", "list_unbilled_periods"),
        (
            "/api/v1/billing/deadline-summary",
            "summarize_billing_deadlines",
        ),
    ] {
        let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":{}}});
        for (role, status) in [("nb", 403), ("lf", 200)] {
            let bearer = format!("Bearer {}", demo::mint(demo::persona(role).unwrap()));
            let rest = app
                .clone()
                .oneshot(
                    Request::get(route)
                        .header("Authorization", &bearer)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(rest.status(), status, "{role} reading {route}");
            let answer = mcp
                .post(
                    &[
                        ("MCP-Protocol-Version", "2025-11-25"),
                        ("Authorization", &bearer),
                    ],
                    &call,
                )
                .await;
            assert_eq!(answer.status, status, "{role} calling {tool}: {answer:?}");
            if status == 200 {
                let response = to_bytes(rest.into_body(), 65536).await.unwrap();
                let rest: Value = serde_json::from_slice(&response).unwrap();
                let rows = if tool == "list_unbilled_periods" {
                    "periods"
                } else {
                    "lieferanten"
                };
                assert_eq!(rest[rows][0]["lf_mp_id"], "9900357000028");
                assert_eq!(
                    answer.message["result"]["structuredContent"], rest,
                    "{tool}"
                );
            }
        }
        let refused = mcp
            .post(
                &[
                    ("MCP-Protocol-Version", "2025-11-25"),
                    ("Authorization", "Bearer fixture-msb-key"),
                ],
                &call,
            )
            .await;
        assert_eq!(refused.status, 403, "{tool}: {refused:?}");
    }
    let id = uuid::Uuid::new_v4().to_string();
    for role in [None, Some("nb")] {
        assert_eq!(sweep_request(&app, role, &[&id], "{}").await.0, 403);
    }
    for keys in [&[][..], &["invalid"][..], &[id.as_str(), id.as_str()][..]] {
        let (status, problem) = sweep_request(&app, Some("lf"), keys, "{}").await;
        assert_eq!(status, 400);
        assert_eq!(problem["code"], "invalid_idempotency_key");
    }
    let (status, _) = sweep_request(&app, Some("lf"), &[&id], r#"{"dry_run":true}"#).await;
    assert_eq!(status, 422);
    for _ in 0..2 {
        let (status, problem) = sweep_request(&app, Some("lf"), &[&id], "{}").await;
        assert_eq!(status, 502);
        assert_eq!(problem["code"], "billing_sweep_failed");
        assert!(uuid::Uuid::parse_str(problem["error_id"].as_str().unwrap()).is_ok());
    }
    let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM billing_discovery WHERE tenant=$1 AND status='failed' AND origin='manual'")
        .bind(&tenant).fetch_one(&pool).await.unwrap();
    assert_eq!(attempts, 1, "replaying a failed key preserves its attempt");
}

async fn sweep_request(
    app: &axum::Router,
    role: Option<&str>,
    keys: &[&str],
    body: &'static str,
) -> (u16, serde_json::Value) {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use mako_service::oidc::demo;
    use tower::ServiceExt as _;
    let mut request =
        Request::post("/api/v1/billing/runs/sweep").header("Content-Type", "application/json");
    if let Some(role) = role {
        request = request.header(
            "Authorization",
            format!("Bearer {}", demo::mint(demo::persona(role).unwrap())),
        );
    }
    for key in keys {
        request = request.header("Idempotency-Key", *key);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
