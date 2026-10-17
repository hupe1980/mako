//! `obs::rebuild` against real PostgreSQL: a projection truncated and rebuilt
//! from `marktd`'s replay log equals the original, row for row.
//!
//! The replay log is served by a stand-in for `marktd` `GET /admin/events`
//! that honours the same contract — `from` and `to` inclusive, oldest first,
//! `limit` rows — with a page size small enough that the rebuild has to page,
//! and with events sharing one `received_at` across a page boundary, which is
//! where an inclusive lower bound re-serves what the previous page applied.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::Query;
use processd::obs::project::{ObsState, project};
use processd::obs::rebuild::{RebuildError, rebuild};
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
        own_mp_ids: Arc::new(HashSet::from([TENANT.to_owned()])),
    }
}

/// One replay-log row as `marktd` stores it: the envelope and when it arrived.
#[derive(Clone)]
struct Logged {
    event_id: String,
    received_at: OffsetDateTime,
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
        event_id: format!("evt-{n}"),
        received_at: at,
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

/// A log with several processes in every state the read model knows, some
/// events sharing one arrival instant, and one event outside `de.mako.*`.
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
        event(1, "de.mako.process.initiated", p1, 55_001, t0, lf.clone()),
        event(
            2,
            "de.mako.process.initiated",
            p2,
            44_001,
            t0,
            serde_json::json!({
            "malo_id": "51238696012", "partner_mp_id": "9800000000003" }),
        ),
        // Three events at one instant: with a page of four, the instant
        // straddles a page boundary.
        event(
            3,
            "de.mako.aperak.accepted",
            p1,
            55_001,
            tied,
            serde_json::json!({}),
        ),
        event(
            4,
            "de.mako.aperak.rejected",
            p2,
            44_001,
            tied,
            serde_json::json!({
            "error_code": "Z29" }),
        ),
        event(
            5,
            "de.mako.process.initiated",
            p3,
            13_003,
            tied,
            serde_json::json!({}),
        ),
        event(
            6,
            "de.markt.versorgung.changed",
            p3,
            0,
            datetime!(2026-09-01 10:00 UTC),
            serde_json::json!({}),
        ),
        event(
            7,
            "de.mako.process.completed",
            p1,
            55_001,
            datetime!(2026-09-02 07:00 UTC),
            serde_json::json!({}),
        ),
        event(
            8,
            "de.mako.aperak.timeout",
            p4,
            55_010,
            datetime!(2026-09-02 08:00 UTC),
            serde_json::json!({ "sender": "9900012345678" }),
        ),
        event(
            9,
            "de.mako.process.failed",
            p3,
            13_003,
            datetime!(2026-09-03 08:00 UTC),
            serde_json::json!({}),
        ),
    ]
}

#[derive(Deserialize)]
struct EventLogQuery {
    from: Option<String>,
    to: Option<String>,
    limit: Option<usize>,
}

/// A stand-in for `marktd` `GET /admin/events`.
async fn marktd_replay(log: Vec<Logged>) -> String {
    let log = Arc::new(log);
    let app = axum::Router::new().route(
        "/admin/events",
        axum::routing::get(move |Query(q): Query<EventLogQuery>| {
            let log = Arc::clone(&log);
            async move {
                let parse =
                    |s: Option<String>| s.and_then(|s| OffsetDateTime::parse(&s, &Rfc3339).ok());
                let (from, to) = (parse(q.from), parse(q.to));
                let mut rows: Vec<&Logged> = log
                    .iter()
                    .filter(|e| from.is_none_or(|f| e.received_at >= f))
                    .filter(|e| to.is_none_or(|t| e.received_at <= t))
                    .collect();
                rows.sort_by_key(|e| e.received_at);
                let body: Vec<serde_json::Value> = rows
                    .into_iter()
                    .take(q.limit.unwrap_or(500))
                    .map(|e| {
                        serde_json::json!({
                            "event_id": e.event_id,
                            "ce_type": e.envelope["type"],
                            "marktrole": null,
                            "sparte": null,
                            "envelope": e.envelope,
                            "fanned_out_at": null,
                            "received_at": e.received_at.format(&Rfc3339).expect("rfc3339"),
                        })
                    })
                    .collect();
                axum::Json(body)
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

    let marktd = marktd_replay(log.clone()).await;
    let report = rebuild(&obs, &upstream(&marktd), 4).await.expect("rebuild");

    assert_eq!(report.read, 9, "every logged event is read exactly once");
    assert_eq!(
        report.ignored, 1,
        "the versorgung event is not a process state"
    );
    assert_eq!(report.projected, 8);
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
    let marktd = marktd_replay(replay_log()).await;

    rebuild(&obs, &upstream(&marktd), 4).await.expect("first");
    let once = snapshot(&pool).await;
    rebuild(&obs, &upstream(&marktd), 4).await.expect("second");
    assert_eq!(snapshot(&pool).await, once);
}

/// As many events at one instant as a page holds cannot be paged by arrival
/// time; the rebuild says so rather than looping on the same page.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unpageable_log_is_refused() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let at = datetime!(2026-09-01 08:00 UTC);
    let log: Vec<Logged> = (0..3)
        .map(|n| {
            event(
                n,
                "de.mako.process.initiated",
                &format!("00000000-0000-4000-8000-00000000001{n}"),
                55_001,
                at,
                serde_json::json!({}),
            )
        })
        .collect();
    let marktd = marktd_replay(log).await;
    let err = rebuild(&state(&pool), &upstream(&marktd), 2)
        .await
        .expect_err("three events at one instant with a page of two");
    assert!(matches!(err, RebuildError::Stalled { .. }), "{err}");
}
