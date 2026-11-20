//! `obs::rebuild` against real PostgreSQL: a projection truncated and rebuilt
//! from `marktd`'s event feed equals the original, row for row.
//!
//! The feed is served by a stand-in for `marktd` `GET /api/v1/events` that
//! honours the same contract — commit order, the `type` pattern, `limit` rows
//! and an opaque `next_cursor` — with a page size small enough that the
//! rebuild has to page.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::Query;
use processd::obs::project::{ObsState, project};
use processd::obs::rebuild::rebuild;
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;

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
        .expect("apply processd schema");
    Some((pool, container))
}

const TENANT: &str = "9900357000004";

fn state(pool: &sqlx::PgPool) -> ObsState {
    ObsState {
        repo: processd::obs::pg::PgProcessProjectionRepository::new(pool.clone()),
        tenant: TENANT.to_owned(),
        verbundene_lieferanten: Arc::new(HashSet::from([TENANT.to_owned()])),
    }
}

/// One event of `marktd`'s outbox, as the feed serves it.
#[derive(Clone)]
struct Logged {
    envelope: serde_json::Value,
}

fn event(
    n: u32,
    ce_type: &str,
    process: &str,
    pid: u32,
    at: OffsetDateTime,
    data: serde_json::Value,
) -> Logged {
    Logged {
        envelope: serde_json::json!({
            "specversion": "1.0",
            "id": format!("evt-{n}"),
            "source": "makod",
            "type": ce_type,
            "subject": process,
            "time": at.format(&Rfc3339).expect("rfc3339"),
            "makopid": pid,
            "data": data,
        }),
    }
}

/// A log with several processes in every state the read model knows, and one
/// event outside `de.mako.engine.*`, which the feed's `type` filter leaves out.
fn replay_log() -> Vec<Logged> {
    let p1 = "00000000-0000-4000-8000-000000000001";
    let p2 = "00000000-0000-4000-8000-000000000002";
    let p3 = "00000000-0000-4000-8000-000000000003";
    let p4 = "00000000-0000-4000-8000-000000000004";
    let t0 = datetime!(2026-09-01 08:00 UTC);
    let tied = datetime!(2026-09-01 09:00 UTC);
    let lf = serde_json::json!({ "malo_id": "51238696012", "partner_mp_id": "9900012345678",
                                 "new_supplier": TENANT });
    vec![
        event(
            1,
            "de.mako.engine.process.initiated",
            p1,
            55_001,
            t0,
            lf.clone(),
        ),
        event(
            2,
            "de.mako.engine.process.initiated",
            p2,
            44_001,
            t0,
            serde_json::json!({
            "malo_id": "51238696012", "partner_mp_id": "9800000000003" }),
        ),
        event(
            3,
            "de.mako.engine.aperak.accepted",
            p1,
            55_001,
            tied,
            serde_json::json!({}),
        ),
        event(
            4,
            "de.mako.engine.aperak.rejected",
            p2,
            44_001,
            tied,
            serde_json::json!({
            "error_code": "Z29" }),
        ),
        event(
            5,
            "de.mako.engine.process.initiated",
            p3,
            13_003,
            tied,
            serde_json::json!({}),
        ),
        event(
            6,
            "de.mako.markt.versorgung.changed",
            p3,
            0,
            datetime!(2026-09-01 10:00 UTC),
            serde_json::json!({}),
        ),
        event(
            7,
            "de.mako.engine.process.completed",
            p1,
            55_001,
            datetime!(2026-09-02 07:00 UTC),
            serde_json::json!({}),
        ),
        event(
            8,
            "de.mako.engine.antwortfrist.abgelaufen",
            p4,
            55_010,
            datetime!(2026-09-02 08:00 UTC),
            serde_json::json!({ "sender": "9900012345678" }),
        ),
        event(
            9,
            "de.mako.engine.process.failed",
            p3,
            13_003,
            datetime!(2026-09-03 08:00 UTC),
            serde_json::json!({}),
        ),
    ]
}

#[derive(Deserialize)]
struct FeedQuery {
    #[serde(rename = "type")]
    event_type: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

/// A stand-in for `marktd` `GET /api/v1/events`: the log in commit order, the
/// cursor the index of the next row.
async fn marktd_feed(log: Vec<Logged>) -> String {
    let log = Arc::new(log);
    let app = axum::Router::new().route(
        "/api/v1/events",
        axum::routing::get(move |Query(q): Query<FeedQuery>| {
            let log = Arc::clone(&log);
            async move {
                let prefix = q
                    .event_type
                    .as_deref()
                    .map(|t| t.trim_end_matches('*').to_owned());
                let rows: Vec<&Logged> = log
                    .iter()
                    .filter(|e| {
                        prefix.as_deref().is_none_or(|p| {
                            e.envelope["type"]
                                .as_str()
                                .unwrap_or_default()
                                .starts_with(p)
                        })
                    })
                    .collect();
                let start: usize = q
                    .cursor
                    .as_deref()
                    .map_or(0, |c| c.parse().expect("cursor"));
                let limit = q.limit.unwrap_or(50);
                let items: Vec<serde_json::Value> = rows
                    .iter()
                    .skip(start)
                    .take(limit)
                    .map(|e| e.envelope.clone())
                    .collect();
                let next = (start + limit < rows.len()).then(|| (start + limit).to_string());
                axum::Json(serde_json::json!({
                    "items": items,
                    "next_cursor": next,
                    "total_estimate": rows.len(),
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn upstream(url: &str) -> mako_service::http::Upstream {
    mako_service::http::Upstream::new(
        "marktd",
        url,
        Some(secrecy::SecretString::from("test")),
        reqwest::Client::new(),
    )
}

/// Every projection row, minus the write timestamp, in a stable order.
async fn snapshot(pool: &sqlx::PgPool) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT to_jsonb(p) - 'updated_at' FROM process_projections p ORDER BY process_id",
    )
    .fetch_all(pool)
    .await
    .expect("snapshot")
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_truncated_projection_is_rebuilt_row_for_row() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let obs = state(&pool);
    let log = replay_log();

    // The live projection: every event through the webhook's function, once.
    for e in &log {
        project(&obs, &e.envelope).await.expect("live projection");
    }
    let original = snapshot(&pool).await;
    assert_eq!(original.len(), 4, "four processes: {original:#?}");

    sqlx::query("TRUNCATE process_projections")
        .execute(&pool)
        .await
        .expect("truncate");
    assert!(snapshot(&pool).await.is_empty());

    let marktd = marktd_feed(log.clone()).await;
    let report = rebuild(&obs, &upstream(&marktd), 4).await.expect("rebuild");

    assert_eq!(
        report.read, 8,
        "every de.mako.engine.* event is read exactly once"
    );
    assert_eq!(report.ignored, 0);
    assert_eq!(report.projected, 8);
    assert_eq!(
        report.oldest_event.as_deref(),
        Some("2026-09-01T08:00:00Z"),
        "the report names how far back the feed reached"
    );
    assert_eq!(
        snapshot(&pool).await,
        original,
        "the rebuilt rows equal the live ones"
    );
}

/// A rebuild over a populated table repairs rather than duplicates: running it
/// a second time leaves every row as it was.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_rebuild_is_idempotent() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let obs = state(&pool);
    let marktd = marktd_feed(replay_log()).await;

    rebuild(&obs, &upstream(&marktd), 4).await.expect("first");
    let once = snapshot(&pool).await;
    rebuild(&obs, &upstream(&marktd), 4).await.expect("second");
    assert_eq!(snapshot(&pool).await, once);
}
