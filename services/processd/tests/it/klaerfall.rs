//! Klärfälle against real PostgreSQL: one case per message, a further signal
//! of any origin attaches, and the due date is the process's Antwortfrist.

mod db_scenarios {
    use processd::klaerfall::{Herkunft, list_open, open_or_attach, signal_of};
    use processd::pg::approval::{ApprovalQueueEntry, PgApprovalQueue};
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
        app_with_makod(pool, url, None).await
    }

    /// The router on a migrated database, under the demo sign-in, talking to
    /// `makod` where one is given.
    async fn app_with_makod(pool: &sqlx::PgPool, url: String, makod: Option<String>) -> String {
        let mut cfg: toml::Value =
            toml::from_str(include_str!("../../../../deploy/demo/processd.toml")).expect("parses");
        cfg["database"]["url"] = toml::Value::String(url);
        if let Some(makod) = makod {
            cfg["makod"]["url"] = toml::Value::String(makod);
        }
        if let Some(db) = cfg["database"].as_table_mut() {
            db.remove("migration_url");
        }
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
        mcp_call_with(base, token, tool, serde_json::json!({})).await
    }

    /// A `tools/call` of `tool` with `arguments`.
    async fn mcp_call_with(
        base: &str,
        token: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        let text = reqwest::Client::new()
            .post(format!("{base}/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .bearer_auth(token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": tool, "arguments": arguments },
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

    /// A Docker PostgreSQL with processd's schema, its URL and its pool.
    async fn database() -> (sqlx::PgPool, String, PgContainer) {
        use testcontainers::ImageExt;
        use testcontainers::runners::AsyncRunner;
        let container = testcontainers_modules::postgres::Postgres::default()
            .with_tag("17-alpine")
            .start()
            .await
            .expect("Docker is required for this suite");
        let port = container.get_host_port_ipv4(5432).await.expect("port");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("apply processd schema");
        (pool, url, container)
    }

    async fn seed_projection(
        pool: &sqlx::PgPool,
        process: Uuid,
        role: &str,
        due: Option<time::OffsetDateTime>,
        malo: Option<&str>,
    ) {
        let at = datetime!(2026-11-17 09:00 UTC);
        sqlx::query(
            "INSERT INTO process_projections (process_id, pid, state, started_at, last_event_at,
               deadline_at, deadline_source, mdm_role, malo_id, tenant)
             VALUES ($1, 55001, 'running', $2, $2, $3, 'BK6-24-174 GPKE Teil 2', $4, $5, $6)",
        )
        .bind(process)
        .bind(at)
        .bind(due)
        .bind(role)
        .bind(malo)
        .bind(TENANT)
        .execute(pool)
        .await
        .expect("seed projection");
    }

    /// A held Freigabe of `process`: the NB's Bestätigung, frozen.
    fn held(process: Uuid, expires_at: time::OffsetDateTime) -> ApprovalQueueEntry {
        ApprovalQueueEntry::pending(
            process,
            55_001,
            Some("51238696012".into()),
            "held for review".into(),
            expires_at,
            TENANT.into(),
            "test-rule",
        )
        .with_commands(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
            Some("NB"),
        )
        .with_answer_payload(serde_json::json!({ "antwort_code": "A51" }))
    }

    /// A Freigabe is a case of origin `operator_decision`, opened in the
    /// enqueue's own transaction. `due_at` is the Frist from the read model;
    /// the row's `expires_at` — the Frist less the operator's headroom — is
    /// kept apart as `operator_window_ends_at`. An expired row's case stays
    /// open and reads overdue once the Frist passed; a lapse of the same
    /// process attaches to it.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn a_freigabe_is_a_case_with_its_own_operator_window() {
        let (pool, _url, _pg) = database().await;
        let process = Uuid::new_v4();
        let due = datetime!(2026-11-18 10:00 UTC);
        let window = datetime!(2026-11-18 09:00 UTC);
        seed_projection(&pool, process, "NB", Some(due), None).await;
        let queue = PgApprovalQueue::new(pool.clone());
        queue
            .enqueue(&held(process, window))
            .await
            .expect("enqueue");
        queue
            .enqueue(&held(process, window))
            .await
            .expect("a second entry for the process is not queued");

        let before = list_open(
            &pool,
            TENANT,
            &every_role(),
            50,
            datetime!(2026-11-17 12:00 UTC),
        )
        .await
        .expect("list");
        assert_eq!(before.count, 1, "{before:#?}");
        let case = &before.klaerfaelle[0];
        assert_eq!(case.origins, [Herkunft::OperatorDecision.as_str()]);
        assert_eq!(case.due_at, Some(due), "the Frist, not the operator window");
        assert_eq!(case.operator_window_ends_at, Some(window));
        assert_eq!(
            case.korrektur.as_ref().map(|k| k.befehl.as_str()),
            Some(mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN)
        );
        assert!(!case.overdue);

        sqlx::query("UPDATE approval_queue SET expires_at = now() - interval '1 hour'")
            .execute(&pool)
            .await
            .expect("age the row");
        assert_eq!(queue.expire_stale().await.expect("expire"), 1);
        let lapse = ce(
            mako_events::mako::ANTWORTFRIST_ABGELAUFEN,
            process,
            serde_json::json!({}),
        );
        open_or_attach(&pool, TENANT, &signal_of(&lapse).expect("signal"), due)
            .await
            .expect("write");
        let after = list_open(
            &pool,
            TENANT,
            &every_role(),
            50,
            datetime!(2026-11-19 00:00 UTC),
        )
        .await
        .expect("list");
        assert_eq!(
            after.count, 1,
            "the expired row's case stays open: {after:#?}"
        );
        let case = &after.klaerfaelle[0];
        assert!(case.overdue, "past its Frist");
        assert_eq!(
            case.origins,
            [
                Herkunft::OperatorDecision.as_str(),
                Herkunft::FristMissed.as_str()
            ]
        );
    }

    /// Dead letters that name no message aggregate into one
    /// open case per partner and reason, counting each.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn dead_letter_flood_opens_one_case() {
        let (pool, _url, _pg) = database().await;
        let at = datetime!(2026-11-17 09:00 UTC);
        for n in 0..1_000 {
            let event = serde_json::json!({
                "type": mako_events::mako::DEAD_LETTER_RECORDED,
                "data": { "reason": "unknown_pid", "dead_letter_id": format!("d-{n}"),
                          "sender": "9900000000003" },
            });
            open_or_attach(&pool, TENANT, &signal_of(&event).expect("signal"), at)
                .await
                .expect("write");
        }
        let open = list_open(&pool, TENANT, &every_role(), 50, at)
            .await
            .expect("list");
        assert_eq!(open.count, 1, "{open:#?}");
        assert_eq!(open.klaerfaelle[0].occurrences, 1_000);
    }

    /// A date the refusal carries never moves `due_at`; it is
    /// the read model's Frist for mako's own process.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn refusal_date_does_not_move_due_at() {
        let (pool, _url, _pg) = database().await;
        let process = Uuid::new_v4();
        let due = datetime!(2026-11-18 10:00 UTC);
        seed_projection(&pool, process, "LF", Some(due), None).await;
        let refusal = ce(
            mako_events::mako::PROCESS_COMPLETED,
            process,
            serde_json::json!({
                "outcome": "rejected", "antwortcode": "A02", "codeliste": "E_0622",
                "lieferbeginn": "2027-03-01", "due_at": "2027-03-01T00:00:00Z",
            }),
        );
        open_or_attach(
            &pool,
            TENANT,
            &signal_of(&refusal).expect("signal"),
            datetime!(2026-11-17 09:00 UTC),
        )
        .await
        .expect("write");
        let open = list_open(
            &pool,
            TENANT,
            &every_role(),
            50,
            datetime!(2026-11-17 09:00 UTC),
        )
        .await
        .expect("list");
        assert_eq!(open.klaerfaelle[0].due_at, Some(due));
    }

    /// A makod whose pre-send check refuses until `pass` is set, counting the
    /// commands it is sent.
    async fn makod_mock(
        pass: std::sync::Arc<std::sync::atomic::AtomicBool>,
        sent: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> String {
        use std::sync::atomic::Ordering;
        let app = axum::Router::new()
            .route(
                "/api/v1/probe/{command}",
                axum::routing::post(move || {
                    let pass = pass.load(Ordering::SeqCst);
                    async move {
                        axum::Json(if pass {
                            serde_json::json!({ "urteil": "besteht" })
                        } else {
                            serde_json::json!({
                                "urteil": "abgelehnt", "stufe": "pruefung",
                                "grund": "the UTILMD violates its AHB profile",
                            })
                        })
                    }
                }),
            )
            .route(
                "/api/v1/commands",
                axum::routing::post(move || {
                    sent.fetch_add(1, Ordering::SeqCst);
                    async {
                        (
                            axum::http::StatusCode::ACCEPTED,
                            axum::Json(serde_json::json!({
                                "process_id": Uuid::nil(), "command": "c", "idempotency_key": null,
                            })),
                        )
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    async fn post(
        base: &str,
        token: &str,
        path: &str,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let resp = reqwest::Client::new()
            .post(format!("{base}{path}"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .expect("answered");
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or_default())
    }

    /// A case is worked over REST, and every
    /// step is journaled against the token's principal.
    ///
    /// Another command than the proposal needs a reason; a command outside
    /// the Freigabe's pair is refused; a body naming `decided_by` is refused
    /// as an unknown field; another role reads the case as absent and `ADMIN`
    /// holds no market role to send it. makod's pre-send check refuses in
    /// place — the case stays open and nothing is sent — and once it passes
    /// the proposal goes out through the Freigabe, which is decided and the
    /// case closed with the proposal and the command journaled.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn a_case_is_corrected_and_sent_through_the_freigabe_path() {
        use mako_markt::commands as c;
        use mako_service::oidc::demo;
        use std::sync::atomic::Ordering;

        let (pool, url, _pg) = database().await;
        let pass = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let makod = makod_mock(pass.clone(), sent.clone()).await;
        let base = app_with_makod(&pool, url, Some(makod)).await;

        let process = Uuid::new_v4();
        seed_projection(
            &pool,
            process,
            "NB",
            Some(time::OffsetDateTime::now_utc() + time::Duration::days(1)),
            Some("51238696012"),
        )
        .await;
        PgApprovalQueue::new(pool.clone())
            .enqueue(&held(
                process,
                time::OffsetDateTime::now_utc() + time::Duration::hours(20),
            ))
            .await
            .expect("enqueue");
        let id = list_open(
            &pool,
            TENANT,
            &every_role(),
            50,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("list")
        .klaerfaelle[0]
            .id;
        let nb = demo::mint(demo::persona("nb").expect("nb persona"));
        let lf = demo::mint(demo::persona("lf").expect("lf persona"));
        let dispatch = format!("/api/v1/klaerfaelle/{id}/dispatch");

        let (status, body) = post(
            &base,
            &nb,
            &dispatch,
            serde_json::json!({ "befehl": c::GPKE_LIEFERBEGINN_ABLEHNEN }),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (422, Some("begruendung_required")),
            "{body}"
        );
        let (status, body) = post(
            &base,
            &nb,
            &dispatch,
            serde_json::json!({ "befehl": c::GELI_EOG_BESTAETIGEN, "begruendung": "x" }),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (422, Some("command_not_allowed")),
            "{body}"
        );
        let (status, _) = post(
            &base,
            &nb,
            &dispatch,
            serde_json::json!({ "decided_by": "someone-else" }),
        )
        .await;
        assert_eq!(status, 422, "decided_by is never a request field");
        let (status, _) = post(&base, &lf, &dispatch, serde_json::json!({})).await;
        assert_eq!(status, 404, "the NB's case reads as absent to the LF");

        let (status, body) = post(&base, &nb, &dispatch, serde_json::json!({})).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (422, Some("pruefung_abgelehnt")),
            "{body}"
        );
        assert_eq!(sent.load(Ordering::SeqCst), 0, "a refused body is not sent");

        let (status, body) = post(
            &base,
            &nb,
            &format!("/api/v1/klaerfaelle/{id}/assign"),
            serde_json::json!({ "assignee": "sachbearbeiterin" }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["klaerfall"]["assignee"], "sachbearbeiterin");

        pass.store(true, Ordering::SeqCst);
        let (status, body) = post(&base, &nb, &dispatch, serde_json::json!({})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(sent.load(Ordering::SeqCst), 1);
        assert_eq!(body["klaerfall"]["zustand"], "dispatched", "{body}");
        assert_eq!(body["klaerfall"]["decided_by"], "nb-sachbearbeitung");
        let verlauf = body["verlauf"].as_array().expect("history");
        assert_eq!(verlauf.len(), 2, "{verlauf:#?}");
        assert_eq!(verlauf[0]["schritt"], "assign");
        assert_eq!(verlauf[1]["schritt"], "dispatch");
        assert_eq!(verlauf[1]["akteur"], "nb-sachbearbeitung");
        assert_eq!(
            verlauf[1]["vorgeschlagen"],
            c::GPKE_LIEFERBEGINN_BESTAETIGEN
        );
        assert_eq!(verlauf[1]["befehl"], c::GPKE_LIEFERBEGINN_BESTAETIGEN);
        let status: String = sqlx::query_scalar("SELECT status FROM approval_queue")
            .fetch_one(&pool)
            .await
            .expect("row");
        assert_eq!(
            status, "Approved",
            "the Freigabe was decided by its own route"
        );

        let (status, body) = post(
            &base,
            &nb,
            &format!("/api/v1/klaerfaelle/{id}/resolve"),
            serde_json::json!({ "begruendung": "erledigt" }),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (409, Some("case_closed")),
            "{body}"
        );

        // The counterparty refuses the answer that went out: an APERAK case of
        // its own, whose correction re-sends the decided Freigabe's answer.
        let aperak = ce(
            mako_events::mako::APERAK_REJECTED,
            process,
            serde_json::json!({
                "error_code": "Z29", "bezug_message_ref": "UTILMD-55002", "sender": "9900000000003",
            }),
        );
        open_or_attach(
            &pool,
            TENANT,
            &signal_of(&aperak).expect("signal"),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("write");
        let refused = list_open(
            &pool,
            TENANT,
            &every_role(),
            50,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("list");
        assert_eq!(refused.count, 1, "{refused:#?}");
        let case = &refused.klaerfaelle[0];
        assert_eq!(
            case.korrektur.as_ref().map(|k| k.befehl.as_str()),
            Some(c::GPKE_LIEFERBEGINN_BESTAETIGEN),
            "the answer the Freigabe sent"
        );
        let (status, body) = post(
            &base,
            &nb,
            &format!("/api/v1/klaerfaelle/{}/dispatch", case.id),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(sent.load(Ordering::SeqCst), 2, "re-sent once");
        assert_eq!(body["klaerfall"]["zustand"], "dispatched");
    }

    /// `ADMIN` closes a case with a reason but sends nothing
    /// — dispatch needs a market role; a resolve without a reason is refused.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn a_case_on_no_process_is_resolved_with_a_reason_and_never_dispatched() {
        use mako_service::oidc::demo;
        let (pool, url, _pg) = database().await;
        let base = app_on(&pool, url).await;
        let event = serde_json::json!({
            "type": mako_events::mako::DEAD_LETTER_RECORDED,
            "data": { "reason": "unknown_pid", "dead_letter_id": "d-1" },
        });
        open_or_attach(
            &pool,
            TENANT,
            &signal_of(&event).expect("signal"),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("write");
        let id = list_open(
            &pool,
            TENANT,
            &every_role(),
            50,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("list")
        .klaerfaelle[0]
            .id;
        let admin = demo::mint(demo::persona("admin").expect("admin persona"));
        let (status, body) = post(
            &base,
            &admin,
            &format!("/api/v1/klaerfaelle/{id}/dispatch"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (422, Some("no_correction")),
            "{body}"
        );
        let resolve = format!("/api/v1/klaerfaelle/{id}/resolve");
        let (status, _) = post(
            &base,
            &admin,
            &resolve,
            serde_json::json!({ "begruendung": " " }),
        )
        .await;
        assert_eq!(status, 422, "a reason is required");
        let (status, body) = post(
            &base,
            &admin,
            &resolve,
            serde_json::json!({ "begruendung": "PID im Partnerprofil nachgetragen" }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["klaerfall"]["zustand"], "resolved");
        assert_eq!(body["verlauf"][0]["akteur"], "admin");
        assert_eq!(
            body["verlauf"][0]["begruendung"],
            "PID im Partnerprofil nachgetragen"
        );
    }

    /// The case an agent reads over MCP carries no MaLo-ID,
    /// no customer reference and no payload — even when its process names a
    /// MaLo.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn get_case_for_agentd_holds_no_malo() {
        use mako_service::oidc::demo;
        let (pool, url, _pg) = database().await;
        let base = app_on(&pool, url).await;
        let process = Uuid::new_v4();
        seed_projection(&pool, process, "NB", None, Some("51238696012")).await;
        let refusal = ce(
            mako_events::mako::PROCESS_COMPLETED,
            process,
            serde_json::json!({
                "outcome": "rejected", "antwortcode": "A02", "codeliste": "E_0622",
                "malo_id": "51238696012",
            }),
        );
        open_or_attach(
            &pool,
            TENANT,
            &signal_of(&refusal).expect("signal"),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("write");
        let id = list_open(
            &pool,
            TENANT,
            &every_role(),
            50,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("list")
        .klaerfaelle[0]
            .id;
        let agent = demo::mint(demo::persona("agent").expect("agent persona"));
        let answer = mcp_call_with(
            &base,
            &agent,
            "get_klaerfall",
            serde_json::json!({ "id": id.to_string() }),
        )
        .await;
        let akte = &answer["result"]["structuredContent"];
        assert_eq!(akte["klaerfall"]["id"], id.to_string(), "{answer}");
        let text = akte.to_string();
        assert!(
            !text.contains("51238696012"),
            "a MaLo-ID reached the agent: {text}"
        );
        for key in ["malo_id", "payload", "kunde", "customer"] {
            assert!(!text.contains(key), "{key} reached the agent: {text}");
        }
    }

    /// The Klärfälle of a year per affiliate class sum to the
    /// cases opened; a case on no process and one on a process no Lieferant
    /// initiates are unattributed, never dropped.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn the_classes_of_a_year_sum_to_the_cases_opened() {
        let (pool, _url, _pg) = database().await;
        let at = datetime!(2026-06-01 09:00 UTC);
        for (pid, affiliate) in [(55_001, true), (55_001, false), (55_016, false)] {
            let process = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO process_projections (process_id, pid, state, started_at,
                   last_event_at, mdm_role, initiator_is_affiliate, tenant)
                 VALUES ($1, $2, 'running', $3, $3, 'NB', $4, $5)",
            )
            .bind(process)
            .bind(pid)
            .bind(at)
            .bind(affiliate)
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
        sqlx::query(
            "UPDATE klaerfaelle SET state = 'resolved', closed_at = opened_at + interval '2 hours'
              WHERE message_ref LIKE 'dead-letter:%'",
        )
        .execute(&pool)
        .await
        .expect("close one");
        let year = processd::klaerfall::paritaet(&pool, TENANT, 2026)
            .await
            .expect("report");
        assert_eq!(year.affiliate.opened, 1, "{year:#?}");
        assert_eq!(year.third_party.opened, 1);
        assert_eq!(year.unattributed.opened, 2);
        assert_eq!(year.unattributed.resolved, 1);
        assert_eq!(year.unattributed.median_resolution_seconds, Some(7_200));
        assert_eq!(
            processd::klaerfall::paritaet(&pool, TENANT, 2025)
                .await
                .expect("report")
                .affiliate
                .opened,
            0,
            "keyed on the year the case opened"
        );
    }
}
