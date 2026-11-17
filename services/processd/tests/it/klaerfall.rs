//! Klärfälle against real PostgreSQL: one case per message, a further signal
//! of any origin attaches, and the due date is the process's Antwortfrist.

mod db_scenarios {
    use processd::klaerfall::{Herkunft, list_open, open_or_attach, signal_of};
    use time::macros::datetime;
    use uuid::Uuid;

    type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

    async fn pg_pool() -> Option<(sqlx::PgPool, PgContainer)> {
        use testcontainers::ImageExt;
        use testcontainers::runners::AsyncRunner;
        use testcontainers_modules::postgres::Postgres;

        let container = Postgres::default()
            .with_tag("17-alpine")
            .start()
            .await
            .ok()?;
        let port = container.get_host_port_ipv4(5432).await.ok()?;
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .ok()?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("apply processd schema");
        Some((pool, container))
    }

    const TENANT: &str = "9900357000004";

    fn every_role() -> Vec<String> {
        ["NB", "LF", "MSB", "ADMIN"].map(str::to_owned).to_vec()
    }

    fn ce(ce_type: &str, process: Uuid, data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "type": ce_type, "subject": process.to_string(), "makopid": 55_001, "data": data,
        })
    }

    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn signals_open_one_case_per_message_and_attach_further_ones() {
        let Some((pool, _pg)) = pg_pool().await else {
            return;
        };
        let at = datetime!(2026-11-17 09:00 UTC);
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let due = datetime!(2026-11-18 10:00 UTC);
        sqlx::query(
            "INSERT INTO process_projections (process_id, pid, state, started_at, last_event_at,
               deadline_at, deadline_source, tenant)
             VALUES ($1, 55001, 'running', $2, $2, $3, 'BK6-24-174 GPKE Teil 2', $4)",
        )
        .bind(b)
        .bind(at)
        .bind(due)
        .bind(TENANT)
        .execute(&pool)
        .await
        .expect("seed projection");

        let signals = [
            ce(
                mako_events::mako::APERAK_REJECTED,
                a,
                serde_json::json!({ "error_code": "Z29", "bezug_message_ref": "UTILMD-1" }),
            ),
            ce(
                mako_events::mako::DEAD_LETTER_RECORDED,
                a,
                serde_json::json!({ "reason": "unknown_pid", "message_ref": "UTILMD-2" }),
            ),
            ce(
                mako_events::mako::PROCESS_COMPLETED,
                b,
                serde_json::json!({ "outcome": "rejected", "antwortcode": "A02", "codeliste": "E_0622" }),
            ),
            // The same message again, from another origin: attaches.
            ce(
                mako_events::mako::DEAD_LETTER_RECORDED,
                a,
                serde_json::json!({ "reason": "processing_error", "message_ref": "UTILMD-1" }),
            ),
            // The same process again: attaches to the refusal's case.
            ce(
                mako_events::mako::ANTWORTFRIST_ABGELAUFEN,
                b,
                serde_json::json!({}),
            ),
        ];
        for event in &signals {
            let signal = signal_of(event).expect("a signal");
            open_or_attach(&pool, TENANT, &signal, at)
                .await
                .expect("write");
        }
        // A redelivery attaches too.
        open_or_attach(&pool, TENANT, &signal_of(&signals[0]).expect("signal"), at)
            .await
            .expect("write");

        let open = list_open(&pool, TENANT, &every_role(), 50, at)
            .await
            .expect("list");
        assert_eq!(open.count, 3, "{:#?}", open.klaerfaelle);
        let first = &open.klaerfaelle[0];
        assert_eq!(
            first.due_at,
            Some(due),
            "the Frist-bearing case comes first"
        );
        assert_eq!(first.origins, ["refusal", "frist_missed"]);
        assert!(
            first.bedeutung.is_some(),
            "the refusal is worded in its tree"
        );
        let utilmd1 = open
            .klaerfaelle
            .iter()
            .find(|k| k.message_ref == "UTILMD-1")
            .expect("case");
        assert_eq!(
            utilmd1.origins,
            [Herkunft::Aperak.as_str(), Herkunft::DeadLetter.as_str()]
        );
        assert_eq!(utilmd1.occurrences, 3);
        assert_eq!(utilmd1.ursache_code, "Z29", "the first cause stands");
        assert_eq!(utilmd1.due_at, None, "no Frist is null, never a default");
    }

    /// A `message_ref` is unique only per sender. An APERAK from
    /// partner Q citing the reference of the message sent to P opens Q's own
    /// case; P's case keeps its count, its origins and its cause.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn signal_from_another_partner_does_not_touch_the_case() {
        let Some((pool, _pg)) = pg_pool().await else {
            return;
        };
        let at = datetime!(2026-11-17 09:00 UTC);
        let process = Uuid::new_v4();
        let aperak = |sender: &str, code: &str| {
            signal_of(&ce(
                mako_events::mako::APERAK_REJECTED,
                process,
                serde_json::json!({
                    "error_code": code, "bezug_message_ref": "UTILMD-9", "sender": sender,
                }),
            ))
            .expect("a signal")
        };
        let p = aperak("9900000000003", "Z29");
        assert_eq!(p.partner_mp_id.as_deref(), Some("9900000000003"));
        open_or_attach(&pool, TENANT, &p, at).await.expect("write");
        open_or_attach(&pool, TENANT, &aperak("9900000000010", "Z10"), at)
            .await
            .expect("write");

        let open = list_open(&pool, TENANT, &every_role(), 50, at)
            .await
            .expect("list");
        assert_eq!(open.count, 2, "Q's signal opens its own case: {open:#?}");
        let ps = open
            .klaerfaelle
            .iter()
            .find(|k| k.partner_mp_id.as_deref() == Some("9900000000003"))
            .expect("P's case");
        assert_eq!(ps.occurrences, 1, "nothing attached to P's case");
        assert_eq!(ps.ursache_code, "Z29");
    }

    /// The router on a migrated database, under the demo sign-in.
    async fn app_on(pool: &sqlx::PgPool, url: String) -> String {
        let mut cfg: toml::Value =
            toml::from_str(include_str!("../../../../deploy/demo/processd.toml")).expect("parses");
        cfg["database"]["url"] = toml::Value::String(url);
        if let Some(db) = cfg["database"].as_table_mut() {
            db.remove("migration_url");
        }
        cfg["subscription"]
            .as_table_mut()
            .expect("[subscription]")
            .remove("webhook_url");
        let app = mako_service::build_for_test_on::<processd::Processd>(
            cfg.try_into().expect("a processd configuration"),
            pool.clone(),
        )
        .await
        .expect("the router builds");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    /// A `tools/call` on 2025-11-25, without a session, as `token`.
    async fn mcp_call(base: &str, token: &str, tool: &str) -> serde_json::Value {
        let text = reqwest::Client::new()
            .post(format!("{base}/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .bearer_auth(token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": tool, "arguments": {} },
            }))
            .send()
            .await
            .expect("answered")
            .text()
            .await
            .expect("a body");
        serde_json::from_str(&text).unwrap_or_else(|_| {
            text.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str::<serde_json::Value>(d.trim()).ok())
                .rfind(|m| m.get("id").is_some())
                .unwrap_or_else(|| panic!("no JSON-RPC answer in {text}"))
        })
    }

    /// The routes: one case on an NB process, one on an LF process.
    /// The LF persona reads its own over REST and over MCP, never the NB's —
    /// and the case on no process is `ADMIN`'s alone.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn lf_role_lists_no_nb_cases() {
        use mako_service::oidc::demo;

        let container = {
            use testcontainers::ImageExt;
            use testcontainers::runners::AsyncRunner;
            testcontainers_modules::postgres::Postgres::default()
                .with_tag("17-alpine")
                .start()
                .await
                .expect("Docker is required for this suite")
        };
        let port = container.get_host_port_ipv4(5432).await.expect("port");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect");
        let base = app_on(&pool, url).await;

        let at = time::OffsetDateTime::now_utc();
        let (nb, lf) = (Uuid::new_v4(), Uuid::new_v4());
        for (process, role) in [(nb, "NB"), (lf, "LF")] {
            sqlx::query(
                "INSERT INTO process_projections (process_id, pid, state, started_at,
                   last_event_at, mdm_role, tenant)
                 VALUES ($1, 55001, 'running', $2, $2, $3, $4)",
            )
            .bind(process)
            .bind(at)
            .bind(role)
            .bind(TENANT)
            .execute(&pool)
            .await
            .expect("seed projection");
            let lapse = ce(
                mako_events::mako::ANTWORTFRIST_ABGELAUFEN,
                process,
                serde_json::json!({}),
            );
            open_or_attach(&pool, TENANT, &signal_of(&lapse).expect("signal"), at)
                .await
                .expect("write");
        }
        let dead_letter = serde_json::json!({
            "type": mako_events::mako::DEAD_LETTER_RECORDED,
            "data": { "reason": "unknown_pid", "dead_letter_id": "d-1" },
        });
        open_or_attach(&pool, TENANT, &signal_of(&dead_letter).expect("signal"), at)
            .await
            .expect("write");

        let lf_token = demo::mint(demo::persona("lf").expect("lf persona"));
        let rest: serde_json::Value = reqwest::Client::new()
            .get(format!("{base}/api/v1/klaerfaelle"))
            .bearer_auth(&lf_token)
            .send()
            .await
            .expect("answered")
            .json()
            .await
            .expect("json");
        let lf_ref = format!("process:{lf}");
        let refs = |cases: &serde_json::Value| -> Vec<String> {
            cases["klaerfaelle"]
                .as_array()
                .unwrap_or_else(|| panic!("no cases in {cases}"))
                .iter()
                .map(|k| k["message_ref"].as_str().unwrap_or_default().to_owned())
                .collect()
        };
        assert_eq!(refs(&rest), std::slice::from_ref(&lf_ref), "REST: {rest}");

        let mcp = mcp_call(&base, &lf_token, "list_klaerfaelle").await;
        let structured = &mcp["result"]["structuredContent"];
        assert_eq!(refs(structured), [lf_ref], "MCP: {mcp}");

        let admin = demo::mint(demo::persona("admin").expect("admin persona"));
        let all = mcp_call(&base, &admin, "list_klaerfaelle").await;
        assert_eq!(
            refs(&all["result"]["structuredContent"]).len(),
            3,
            "ADMIN reads the case on no process too: {all}"
        );
    }
}
