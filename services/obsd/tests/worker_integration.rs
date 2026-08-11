//! Real-PostgreSQL integration tests for the `de.obs.*` sweep producers.
//!
//! Uses testcontainers (self-managing Docker) — runs under `cargo test` when a
//! Docker daemon is available, skipped gracefully otherwise.

use std::sync::Arc;

use obsd::worker::{WorkerRuntime, sweep_deadlines, sweep_parity};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Container guard the test holds until it ends — dropping it removes the
/// container (no leak, no external reaper).
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
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply obsd schema");
    Some((pool, container))
}

/// Minimal always-200 CloudEvent sink. The sweeps only stamp
/// `deadline_alerted_at` (and only report an alert) once the POST succeeded, so
/// they need a receiver that is actually up.
async fn event_sink() -> String {
    let app = axum::Router::new().route(
        "/events",
        axum::routing::post(|| async { axum::http::StatusCode::NO_CONTENT }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/events")
}

/// An outbound URL nothing listens on — every emit fails.
const DEAD_SINK: &str = "http://127.0.0.1:9/none";

fn runtime(pool: sqlx::PgPool, tenant: &str, outbound_url: &str) -> WorkerRuntime {
    WorkerRuntime {
        pool,
        client: Arc::new(reqwest::Client::new()),
        outbound_url: Arc::new(outbound_url.to_owned()),
        outbound_secret: None,
        tenant: tenant.to_owned(),
        deadline_sweep_secs: 900,
        deadline_warn_hours: 24,
        parity_sweep_secs: 86_400,
        parity_threshold_pp: 5.0,
        parity_window_days: 90,
    }
}

async fn insert_projection(
    pool: &sqlx::PgPool,
    tenant: &str,
    pid: i32,
    state: &str,
    deadline_at: Option<OffsetDateTime>,
    affiliate: bool,
) -> Uuid {
    let id = Uuid::new_v4();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        r"INSERT INTO process_projections
            (process_id, pid, family, workflow_name, state, deadline_at,
             initiator_is_affiliate, started_at, last_event_at, tenant)
          VALUES ($1,$2,'gpke','gpke-supplier-change',$3,$4,$5,$6,$6,$7)",
    )
    .bind(id)
    .bind(pid)
    .bind(state)
    .bind(deadline_at)
    .bind(affiliate)
    .bind(now)
    .bind(tenant)
    .execute(pool)
    .await
    .expect("insert projection");
    id
}

#[tokio::test]
async fn deadline_sweep_alerts_once_for_approaching_open_processes() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let tenant = "9900000000002";
    let now = OffsetDateTime::now_utc();

    // Approaching (12h) + open → should alert.
    let approaching = insert_projection(
        &pool,
        tenant,
        55001,
        "initiated",
        Some(now + Duration::hours(12)),
        false,
    )
    .await;
    // Far future (72h > 24h window) → not yet.
    insert_projection(
        &pool,
        tenant,
        55001,
        "initiated",
        Some(now + Duration::hours(72)),
        false,
    )
    .await;
    // Approaching but already completed → excluded.
    insert_projection(
        &pool,
        tenant,
        55001,
        "completed",
        Some(now + Duration::hours(6)),
        false,
    )
    .await;
    // Already overdue and still open → alerted as a breach. Skipping these
    // meant an obsd outage longer than the warn window produced no event at all.
    let breached = insert_projection(
        &pool,
        tenant,
        55001,
        "initiated",
        Some(now - Duration::hours(1)),
        false,
    )
    .await;

    let rt = runtime(pool.clone(), tenant, &event_sink().await);

    // First sweep alerts the approaching process and the breached one.
    assert_eq!(sweep_deadlines(&rt).await.expect("sweep"), 2);
    for id in [approaching, breached] {
        let alerted: Option<OffsetDateTime> = sqlx::query_scalar(
            "SELECT deadline_alerted_at FROM process_projections WHERE process_id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(alerted.is_some(), "{id} is marked alerted");
    }

    // Second sweep is idempotent — nothing new to alert.
    assert_eq!(sweep_deadlines(&rt).await.expect("sweep"), 0);
}

/// A downed webhook target must not consume the warning: nothing is stamped, so
/// the next sweep retries the same processes.
#[tokio::test]
async fn deadline_sweep_does_not_stamp_when_the_emit_fails() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let tenant = "9900000000004";
    let now = OffsetDateTime::now_utc();
    let id = insert_projection(
        &pool,
        tenant,
        55001,
        "initiated",
        Some(now + Duration::hours(6)),
        false,
    )
    .await;

    let dead = runtime(pool.clone(), tenant, DEAD_SINK);
    assert_eq!(sweep_deadlines(&dead).await.expect("sweep"), 0);
    let alerted: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT deadline_alerted_at FROM process_projections WHERE process_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(alerted.is_none(), "an undelivered alert is not stamped");

    // With the sink back up the same process is alerted.
    let live = runtime(pool.clone(), tenant, &event_sink().await);
    assert_eq!(sweep_deadlines(&live).await.expect("sweep"), 1);
}

#[tokio::test]
async fn parity_sweep_alerts_when_affiliate_is_favoured_beyond_threshold() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let tenant = "9900000000002";

    // 20 affiliate Anmeldungen, all completed (100%).
    for _ in 0..20 {
        insert_projection(&pool, tenant, 55001, "completed", None, true).await;
    }
    // 20 non-affiliate, half completed (50%) → 50 pp gap, affiliate favoured.
    for i in 0..20 {
        let state = if i % 2 == 0 { "completed" } else { "rejected" };
        insert_projection(&pool, tenant, 55001, state, None, false).await;
    }

    let sink = event_sink().await;
    let rt = runtime(pool.clone(), tenant, &sink);
    assert!(
        sweep_parity(&rt).await.expect("parity sweep"),
        "a 50 pp affiliate-favoured gap must alert"
    );

    // A near-parity tenant must not alert: affiliate 40/40 (100%),
    // non-affiliate 39/40 (97.5%) → 2.5 pp gap, below the 5 pp threshold.
    let tenant2 = "9900000000003";
    for _ in 0..40 {
        insert_projection(&pool, tenant2, 55001, "completed", None, true).await;
    }
    for i in 0..40 {
        let s = if i == 0 { "rejected" } else { "completed" };
        insert_projection(&pool, tenant2, 55001, s, None, false).await;
    }
    let rt2 = runtime(pool, tenant2, &sink);
    assert!(
        !sweep_parity(&rt2).await.expect("parity sweep"),
        "a within-threshold gap must not alert"
    );
}
