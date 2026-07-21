//! Kafka ingest end-to-end against krafka's in-process `FakeBroker`.
//!
//! A real `Producer` and the real edmd consumer talk to an in-process broker
//! over an actual TCP socket (krafka `test-broker` feature) — no Docker
//! Kafka. PostgreSQL is the same throwaway instance the other integration
//! suites use, because the property under test ends in SQL: produced batches
//! must land in `meter_reads` through the full V01–V10 + audit-trail path,
//! and a poison pill must be skipped without wedging the partition.
//!
//! ```bash
//! docker run -d --name edmd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=edmd -p 55432:5432 postgres:17-alpine
//! export EDMD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55432/edmd"
//! cargo test -p edmd --test kafka_ingest_e2e -- --include-ignored
//! ```

use std::time::Duration;

use krafka::producer::Producer;
use krafka::testing::FakeBroker;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("EDMD_TEST_DATABASE_URL").ok()?;
    let admin = PgPool::connect(&base).await.ok()?;

    let schema = format!("kfk_{test_name}");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("drop schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    admin.close().await;

    let opts: sqlx::postgres::PgConnectOptions = base.parse().expect("parse url");
    let pool = PgPool::connect_with(opts.options([("search_path", schema.as_str())]))
        .await
        .expect("connect to test schema");

    for stmt in split_statements(SCHEMA) {
        sqlx::query(&stmt)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("schema statement failed: {e}\n{stmt}"));
    }
    Some(pool)
}

fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_dollar = false;
    for line in sql.lines() {
        let dollars = line.matches("$$").count() + line.matches("$re$").count();
        if dollars % 2 == 1 {
            in_dollar = !in_dollar;
        }
        current.push_str(line);
        current.push('\n');
        if !in_dollar && line.trim_end().ends_with(';') {
            let stmt = current.trim().to_owned();
            if !stmt.is_empty() && !stmt.lines().all(|l| l.trim().starts_with("--")) {
                out.push(stmt);
            }
            current.clear();
        }
    }
    out
}

/// Wait until the row count reaches `expected` or the deadline passes.
async fn await_rows(pool: &PgPool, expected: i64, deadline: Duration) -> i64 {
    let start = std::time::Instant::now();
    loop {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM meter_reads")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        if count >= expected || start.elapsed() > deadline {
            return count;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn produced_batches_land_in_meter_reads_and_poison_is_skipped() {
    let Some(pool) = test_pool("e2e").await else {
        return;
    };

    let broker = FakeBroker::start().await.expect("fake broker");
    broker.create_topic("edmd.meter-reads", 1);
    let producer = Producer::builder()
        .bootstrap_servers(broker.bootstrap_servers())
        .build()
        .await
        .expect("producer");

    let topic = "edmd.meter-reads";
    let valid = serde_json::json!({
        "malo_id": "51238696781",
        "sparte": "STROM",
        "source": "IOT_PUSH",
        "intervals": [
            {"from": "2026-07-01T00:00:00Z", "to": "2026-07-01T00:15:00Z",
             "value_kwh": "1.25", "quality": "MEASURED", "obis_code": "1-0:1.8.0"},
            {"from": "2026-07-01T00:15:00Z", "to": "2026-07-01T00:30:00Z",
             "value_kwh": "2.5", "quality": "MEASURED", "obis_code": "1-0:1.8.0"}
        ]
    });
    // A negative-energy batch: stored, but annotated by V03.
    let anomalous = serde_json::json!({
        "malo_id": "98765432109",
        "intervals": [
            {"from": "2026-07-01T00:00:00Z", "to": "2026-07-01T00:15:00Z",
             "value_kwh": "-4", "obis_code": "1-0:1.8.0"}
        ]
    });

    let _ = producer
        .send(topic, Some(b"51238696781"), valid.to_string().as_bytes())
        .await
        .expect("produce valid");
    let _ = producer
        .send(topic, None, b"{ this is not json")
        .await
        .expect("produce poison");
    let _ = producer
        .send(
            topic,
            Some(b"98765432109"),
            anomalous.to_string().as_bytes(),
        )
        .await
        .expect("produce anomalous");

    let cfg = edmd::config::KafkaIngestConfig {
        enabled: true,
        bootstrap_servers: broker.bootstrap_servers(),
        topic: topic.to_owned(),
        group_id: "edmd-e2e".to_owned(),
        poll_ms: 200,
    };
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());
    let shutdown = CancellationToken::new();
    edmd::kafka_ingest::spawn(cfg, repo, "T1".to_owned(), shutdown.clone());

    // 2 valid + 1 anomalous interval; the poison record contributes nothing.
    let count = await_rows(&pool, 3, Duration::from_secs(20)).await;
    assert_eq!(
        count, 3,
        "the consumer must store the two well-formed batches and skip the poison pill"
    );

    // At-least-once contract: offsets commit only after the store, and the
    // skipped poison pill is committed past rather than replayed forever.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if broker.committed_offset("edmd-e2e", topic, 0) == Some(3) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected committed offset 3, got {:?}",
            broker.committed_offset("edmd-e2e", topic, 0)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    shutdown.cancel();

    use sqlx::Row as _;
    let rows = sqlx::query(
        "SELECT malo_id, quantity_kwh::text AS q, source, quality_warnings
         FROM meter_reads ORDER BY malo_id, dtm_from",
    )
    .fetch_all(&pool)
    .await
    .expect("select");

    let first = &rows[0];
    assert_eq!(first.get::<String, _>("malo_id"), "51238696781");
    assert_eq!(first.get::<String, _>("q"), "1.25000");
    assert_eq!(first.get::<String, _>("source"), "IOT_PUSH");

    // The negative value passed through the same V01–V10 pass as REST ingest.
    let neg = rows
        .iter()
        .find(|r| r.get::<String, _>("malo_id") == "98765432109")
        .expect("anomalous row stored");
    let warnings: Option<serde_json::Value> = neg.get("quality_warnings");
    let warnings = warnings.expect("V03 must annotate the negative reading");
    assert!(
        warnings.to_string().contains("V03"),
        "expected a V03 annotation, got: {warnings}"
    );
}
