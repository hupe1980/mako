//! The isolated composition uses the real tenant verifier and the installed generation.
#[cfg(feature = "role-nb-strom")]
mod db_scenarios {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use mako_service::oidc::demo::{self, Persona};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use testcontainers::{ImageExt, runners::AsyncRunner};
    use testcontainers_modules::postgres::Postgres;
    use tower::ServiceExt;

    const FIXTURE: &str =
        include_str!("../../../../makotest/tests/fixtures/shadow/reports/review.json");
    const REVIEW: &str = "/api/v1/shadow/review";

    async fn call(
        app: &Router,
        method: &str,
        path: &str,
        persona: Option<&Persona>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if let Some(p) = persona {
            req = req.header("Authorization", format!("Bearer {}", demo::mint(p)));
        }
        let response = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let raw = to_bytes(response.into_body(), 2 << 20).await.unwrap();
        (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
    }

    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn private_cohort_requires_a_named_nb_and_reopens_on_new_evidence() {
        let db = Postgres::default()
            .with_tag("17-alpine")
            .start()
            .await
            .unwrap();
        let port = db.get_host_port_ipv4(5432).await.unwrap();
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        let pool = serde_json::from_value::<mako_service::DatabaseConfig>(json!({"url":url}))
            .unwrap()
            .connect(&url, "processd-shadow-test")
            .await
            .unwrap();
        let reader = demo::persona("nb").unwrap();
        let resolver = Persona {
            id: "reviewer",
            name: "Review",
            sub: "review-nb",
            roles: &["NB"],
        };
        let lf = demo::persona("lf").unwrap();
        let path = std::env::temp_dir().join(format!("shadow-{}.json", uuid::Uuid::new_v4()));
        let install_for = |packet: &Value, generation, tenant: &str| {
            let bytes = serde_json::to_vec(packet).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            let cfg = json!({"http":{}, "database":{"url":url}, "identity":{"tenant":tenant,"nb_mp_ids":[packet["nb"]],"verbundene_lieferanten":[]},
                "oidc":{"mode":"demo"}, "shadow_review":{"file":path,"sha256":format!("{:x}",Sha256::digest(&bytes)), "scope":packet["scope"],"generation":generation,"required_werktage":1,"readers":[reader.sub,resolver.sub,lf.sub],"resolvers":[resolver.sub]}});
            mako_service::build_for_test_on::<processd::Processd>(
                serde_json::from_value(cfg).unwrap(),
                pool.clone(),
            )
        };
        let install = |packet: &Value, generation| install_for(packet, generation, demo::TENANT);
        let mut packet: Value = serde_json::from_str(FIXTURE).unwrap();
        packet["rows"][0]["assessment"]["actual"]["code"] = json!("A07");
        packet["rows"][0]["assessment"]["answer_sha256"] = json!("a".repeat(64));
        let app = install(&packet, 1).await.unwrap();
        let retry = install(&packet, 1).await.unwrap();
        let foreign = install_for(&packet, 1, "other-operator").await.unwrap();
        for (method, path) in [("GET", REVIEW), ("POST", "/mcp")] {
            let denied = call(&foreign, method, path, Some(reader), Value::Null).await;
            assert!(matches!(
                denied.0,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ));
            assert!(denied.1.get("cohort").is_none());
        }
        for (method, path) in [("GET", REVIEW), ("POST", "/mcp")] {
            assert_eq!(call(&app, method, path, Some(lf), Value::Null).await.0, 403);
            assert_eq!(call(&app, method, path, None, Value::Null).await.0, 401);
        }
        for path in ["/webhook", "/api/v1/queue", "/api/v1/nb/decisions"] {
            assert_eq!(
                call(&app, "POST", path, Some(&resolver), Value::Null)
                    .await
                    .0,
                404
            );
        }
        let (status, report) = call(&retry, "GET", REVIEW, Some(reader), Value::Null).await;
        assert_eq!(status, 200);
        for (key, n) in [
            ("eligible", 2),
            ("assessed", 1),
            ("unassessed", 1),
            ("open_differences", 1),
            ("timing_unassessed", 2),
        ] {
            assert_eq!(report[key], n);
        }
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../desk/fixtures/shadow-review.json");
        if std::env::var_os("BLESS_MCP_SNAPSHOT").is_some() {
            std::fs::write(
                &fixture,
                format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
            )
            .unwrap();
        }
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(fixture).unwrap()).unwrap(),
            report
        );
        let endpoint = mako_service::mcp::conformance::Endpoint::serve(app.clone(), "/mcp").await;
        let headers = [
            ("MCP-Protocol-Version", "2025-11-25"),
            ("Authorization", &format!("Bearer {}", demo::mint(reader))),
        ];
        let result = endpoint.post(&headers,&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_shadow_review","arguments":{}}})).await.message;
        assert_eq!(result["result"]["structuredContent"], report);
        let list = endpoint
            .post(
                &headers,
                &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
            )
            .await
            .message;
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "get_shadow_review");
        assert_eq!(report["ready"], false);
        assert_eq!(report["can_resolve"], false);
        let resolve = format!(
            "{REVIEW}/{}/resolve",
            packet["rows"][0]["id"].as_str().unwrap()
        );
        let missing = format!(
            "{REVIEW}/{}/resolve",
            packet["rows"][1]["id"].as_str().unwrap()
        );
        let decision = json!({"generation":1,"revision":report["revision"],"evidence":"local archive 42","reason":"Verified against the source"});
        assert_eq!(
            call(&app, "POST", &resolve, Some(reader), decision.clone())
                .await
                .0,
            403
        );
        assert_eq!(
            call(&app, "POST", &missing, Some(&resolver), decision.clone())
                .await
                .0,
            409
        );
        for _ in 0..2 {
            assert_eq!(
                call(&app, "POST", &resolve, Some(&resolver), decision.clone())
                    .await
                    .0,
                204
            );
        }
        let recorded = call(&app, "GET", REVIEW, Some(reader), Value::Null).await.1;
        assert_eq!(recorded["resolutions"][0]["actor"], resolver.sub);
        assert_eq!(recorded["resolutions"][0]["disposition"], "alt_falsch");
        assert_eq!(recorded["open_differences"], 0);
        let matched: Value = serde_json::from_str(FIXTURE).unwrap();
        let new = install(&matched, 2).await.unwrap();
        assert!(install(&packet, 1).await.is_err());
        assert!(install(&packet, 3).await.is_err());
        assert_eq!(
            call(&app, "GET", REVIEW, Some(reader), Value::Null).await.0,
            412
        );
        assert_eq!(
            call(&new, "POST", &resolve, Some(&resolver), decision.clone())
                .await
                .0,
            412
        );
        let read = call(&new, "GET", REVIEW, Some(&resolver), Value::Null)
            .await
            .1;
        assert_eq!(read["open_differences"], 1);
        let fix = json!({"generation":2,"revision":read["revision"],"reason":"Rechecked the corrected answer","evidence":"second comparison"});
        assert_eq!(
            call(&new, "POST", &resolve, Some(&resolver), fix).await.0,
            204
        );
        let read = call(&new, "GET", REVIEW, Some(reader), Value::Null).await.1;
        assert_eq!(read["resolutions"][0]["disposition"], "mako_korrigiert");
        assert_eq!(read["ready"], false);
        let mut complete = matched.clone();
        complete["scope"] = json!("e".repeat(64));
        complete["start"] = json!("2026-11-01T23:00:00Z");
        complete["end"] = json!("2026-11-02T23:00:00Z");
        complete["complete_werktage"] = json!(1);
        complete["rows"].as_array_mut().unwrap().pop();
        let assessed = &mut complete["rows"][0]["assessment"];
        assessed["incumbent_received_at"] = json!("2026-11-02T09:00:00Z");
        assessed["timing"] = json!("on_time");
        assessed["observation_fingerprint"] = json!("d".repeat(64));
        let fully = install(&complete, 3).await.unwrap();
        assert_eq!(
            call(&fully, "GET", REVIEW, Some(reader), Value::Null)
                .await
                .1["ready"],
            true
        );
        complete["scope"] = json!("f".repeat(64));
        complete["directions"] = json!(["inbound"]);
        let one_direction = install(&complete, 4).await.unwrap();
        assert_eq!(
            call(&one_direction, "GET", REVIEW, Some(reader), Value::Null)
                .await
                .1["ready"],
            false
        );
        complete["scope"] = json!("1".repeat(64));
        complete["rows"] = json!([]);
        complete["directions"] = json!(["outbound"]);
        let empty = install(&complete, 5).await.unwrap();
        let result = call(&empty, "GET", REVIEW, Some(reader), Value::Null)
            .await
            .1;
        assert_eq!(result["eligible"], 0);
        assert_eq!(result["ready"], false);
        complete["complete_werktage"] = json!(2);
        assert!(install(&complete, 6).await.is_err());
        std::fs::remove_file(path).unwrap();
    }
}
