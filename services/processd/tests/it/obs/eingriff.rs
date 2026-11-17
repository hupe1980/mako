//! How much ran without a person: each ended process counted once, by the
//! most human actor its events carried (`makoakteur`), against real
//! PostgreSQL.

mod db_scenarios {
    use std::collections::HashSet;
    use std::sync::Arc;

    use processd::obs::project::{ObsState, project};
    use processd::obs::repository::ProcessProjectionRepository as _;
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    use time::macros::{date, datetime};
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

    fn event(ce_type: &str, process: Uuid, at: OffsetDateTime, akteur: &str) -> serde_json::Value {
        serde_json::json!({
            "specversion": "1.0",
            "id": Uuid::new_v4().to_string(),
            "source": "makod",
            "type": ce_type,
            "subject": process.to_string(),
            "time": at.format(&Rfc3339).expect("rfc3339"),
            "makopid": 55_001,
            "makoworkflow": "gpke-lf-anmeldung",
            "makoakteur": akteur,
            "data": { "malo_id": "51238696012" },
        })
    }

    /// A seeded month: one Anmeldung that ran alone, one a person started,
    /// one approved unchanged, one approved after an edit — a quarter each —
    /// and one still open, which is not counted.
    #[tokio::test]
    #[ignore = "requires Docker (testcontainers PostgreSQL)"]
    async fn the_seeded_month_reports_a_quarter_in_each_class() {
        let Some((pool, _pg)) = pg_pool().await else {
            return;
        };
        let obs = ObsState {
            repo: processd::obs::pg::PgProcessProjectionRepository::new(pool.clone()),
            tenant: TENANT.to_owned(),
            verbundene_lieferanten: Arc::new(HashSet::new()),
        };
        let t0 = datetime!(2026-11-02 09:00 UTC);
        for (start, end) in [
            ("system", "system"),
            ("person", "system"),
            ("system", "freigabe"),
            ("system", "freigabe-geaendert"),
        ] {
            let process = Uuid::new_v4();
            project(
                &obs,
                &event(mako_events::mako::PROCESS_INITIATED, process, t0, start),
            )
            .await
            .expect("initiated");
            // A later system event does not undo a person's touch.
            project(
                &obs,
                &event(
                    mako_events::mako::PROCESS_COMPLETED,
                    process,
                    t0 + time::Duration::hours(2),
                    end,
                ),
            )
            .await
            .expect("completed");
        }
        let open = Uuid::new_v4();
        project(
            &obs,
            &event(mako_events::mako::PROCESS_INITIATED, open, t0, "person"),
        )
        .await
        .expect("initiated");

        let report = obs
            .repo
            .kpi_report(55_001, date!(2026 - 11 - 01), date!(2026 - 11 - 30), TENANT)
            .await
            .expect("report");
        let e = report.eingriff;
        assert_eq!(e.beendet, 4, "the open process is not counted");
        assert_eq!(
            (
                e.ohne_eingriff,
                e.freigabe_unveraendert,
                e.freigabe_geaendert,
                e.person
            ),
            (1, 1, 1, 1)
        );
        assert!(e.anteile.is_none(), "below 20 ended processes: counts only");
        assert!(e.definition.contains("unverändert"));
    }
}
