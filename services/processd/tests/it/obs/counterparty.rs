//! The Gegenpartei-Akte against real PostgreSQL: one query reads a
//! counterparty's processes in a Berlin period, and the fold over them gives
//! the page's figures.

mod db_scenarios {
    use processd::obs::counterparty::{csv, record, rows};
    use time::macros::{date, datetime};
    use time::{Duration, OffsetDateTime};
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

    const TENANT: &str = mako_service::oidc::demo::TENANT;
    const NB: &str = "9900000000003";

    async fn insert(
        pool: &sqlx::PgPool,
        partner: &str,
        state: &str,
        started_at: OffsetDateTime,
        deadline_at: Option<OffsetDateTime>,
        completed_at: Option<OffsetDateTime>,
        antwort: Option<(&str, &str)>,
    ) {
        sqlx::query(
            "INSERT INTO process_projections
               (process_id, pid, family, workflow_name, state, partner_mp_id, mdm_role,
                deadline_at, started_at, last_event_at, completed_at, antwort_code,
                antwort_codeliste, tenant)
             VALUES ($1, 55001, 'gpke', 'gpke-lf-anmeldung', $2, $3, 'LF', $4, $5, $5, $6,
                     $7, $8, $9)",
        )
        .bind(Uuid::new_v4())
        .bind(state)
        .bind(partner)
        .bind(deadline_at)
        .bind(started_at)
        .bind(completed_at)
        .bind(antwort.map(|a| a.0))
        .bind(antwort.map(|a| a.1))
        .bind(TENANT)
        .execute(pool)
        .await
        .expect("insert projection");
    }

    /// The seeded set: one on time, one late by one minute, one
    /// unanswered, one without Frist, one rejected `A02` — and one started the
    /// day after the period, and one of another partner, neither read.
    #[tokio::test]
    #[ignore = "needs Docker — just test-processd-db"]
    async fn one_query_reads_the_partners_period_and_the_fold_counts_it() {
        let Some((pool, _c)) = pg_pool().await else {
            panic!("Docker is required for this suite");
        };
        // 01.10.2026 00:30 Berlin is 30.09. 22:30 UTC: inside a period that
        // starts on the 1st, though its UTC date is the 30th.
        let start = datetime!(2026-09-30 22:30 UTC);
        let frist = datetime!(2026-10-02 09:00 UTC);
        insert(
            &pool,
            NB,
            "completed",
            start,
            Some(frist),
            Some(frist),
            None,
        )
        .await;
        insert(
            &pool,
            NB,
            "completed",
            start,
            Some(frist),
            Some(frist + Duration::minutes(1)),
            None,
        )
        .await;
        insert(&pool, NB, "running", start, Some(frist), None, None).await;
        insert(&pool, NB, "running", start, None, None, None).await;
        insert(
            &pool,
            NB,
            "rejected",
            start,
            Some(frist),
            Some(frist - Duration::hours(2)),
            Some(("A02", "E_0622")),
        )
        .await;
        insert(
            &pool,
            NB,
            "running",
            datetime!(2026-10-31 23:30 UTC),
            Some(frist),
            None,
            None,
        )
        .await;
        insert(
            &pool,
            "9900000000010",
            "running",
            start,
            Some(frist),
            None,
            None,
        )
        .await;

        let (von, bis) = (date!(2026 - 10 - 01), date!(2026 - 10 - 31));
        let read = rows(&pool, TENANT, Some(NB), von, bis)
            .await
            .expect("the query");
        assert_eq!(
            read.len(),
            5,
            "the period is Berlin days, the partner is one MP-ID"
        );

        let a = record(NB, "NB", von, bis, &read, datetime!(2026-10-05 12:00 UTC));
        assert_eq!(
            (
                a.fristgerecht,
                a.verspaetet,
                a.unbeantwortet,
                a.ausserhalb.ohne_frist
            ),
            (2, 1, 1, 1)
        );
        assert_eq!(a.verspaetung.max_minuten, Some(1));
        assert_eq!(a.ablehnungen.len(), 1);
        assert!(
            a.ablehnungen[0].bedeutung.is_some(),
            "A02 is worded in E_0622"
        );
        assert_eq!(
            csv(&a).lines().count() - 1,
            5,
            "the export lists every process"
        );

        let all = rows(&pool, TENANT, None, von, bis)
            .await
            .expect("the query");
        assert_eq!(all.len(), 6);
    }

    /// Over MCP an LF-role caller's `get_counterparty_record` keeps
    /// the rows to the processes mako acted in as LF, as the REST route does.
    /// The NB side's 55010 — the partner owes the answer as LFA — is in the
    /// record of a caller holding NB and absent from the LF's.
    #[tokio::test]
    #[ignore = "needs Docker — just test-processd-db"]
    async fn an_lf_caller_reads_no_nb_process_over_mcp() {
        use mako_service::oidc::demo;

        let Some((pool, container)) = pg_pool().await else {
            panic!("Docker is required for this suite");
        };
        let port = container.get_host_port_ipv4(5432).await.expect("port");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        let mut cfg: toml::Value =
            toml::from_str(include_str!("../../../../../deploy/demo/processd.toml"))
                .expect("parses");
        cfg["database"]["url"] = toml::Value::String(url);
        if let Some(db) = cfg["database"].as_table_mut() {
            db.remove("migration_url");
        }
        let app = mako_service::build_for_test_on::<processd::Processd>(
            cfg.try_into().expect("a processd configuration"),
            pool.clone(),
        )
        .await
        .expect("the router builds");
        let endpoint = mako_service::mcp::conformance::Endpoint::serve(app, "/mcp").await;

        let started = OffsetDateTime::now_utc() - Duration::days(1);
        for (pid, role) in [(55_001, "LF"), (55_010, "NB")] {
            sqlx::query(
                "INSERT INTO process_projections
                   (process_id, pid, state, partner_mp_id, mdm_role, started_at,
                    last_event_at, tenant)
                 VALUES ($1, $2, 'running', $3, $4, $5, $5, $6)",
            )
            .bind(Uuid::new_v4())
            .bind(pid)
            .bind(NB)
            .bind(role)
            .bind(started)
            .bind(TENANT)
            .execute(&pool)
            .await
            .expect("insert projection");
        }

        let call = |persona: &str| {
            let token = demo::mint(demo::persona(persona).expect("persona"));
            let endpoint = &endpoint;
            async move {
                endpoint
                    .post(
                        &[
                            ("MCP-Protocol-Version", "2025-11-25"),
                            ("Authorization", &format!("Bearer {token}")),
                        ],
                        &serde_json::json!({
                            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                            "params": {
                                "name": "get_counterparty_record",
                                "arguments": { "mp_id": NB, "rolle": "LF" },
                            },
                        }),
                    )
                    .await
                    .message
            }
        };
        let prozesse = |answer: &serde_json::Value| {
            answer["result"]["structuredContent"]["prozesse"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_else(|| panic!("no prozesse in {answer}"))
        };
        let nb = call("nb").await;
        assert_eq!(prozesse(&nb), 1, "the NB reads its own 55010: {nb}");
        let lf = call("lf").await;
        assert_eq!(prozesse(&lf), 0, "the LF reads no NB process: {lf}");
    }
}
