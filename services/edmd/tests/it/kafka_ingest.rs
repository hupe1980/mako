//! Kafka ingest end to end: a real krafka producer and edmd's consumer against
//! krafka's in-process `FakeBroker` (feature `test-broker`, a TCP socket and no
//! Kafka), storing into the meterstore tiers over a testcontainers PostgreSQL —
//! so `#[ignore]`d, run by `just test-edmd-db`:
//!
//! ```bash
//! cargo test -p edmd --test it -- kafka_ingest:: --include-ignored --test-threads=1
//! ```
//!
//! What it pins:
//! - **A fresh group drains the backlog** — records produced before the group's
//!   first commit are stored, which holds only while the consumer resets to
//!   `Earliest`.
//! - **Poison pills are committed past** — an unparseable record and a record
//!   whose content is refused (an unknown Sparte) are skipped, and the committed
//!   offset moves beyond them; a refusal that aborted the poll loop instead
//!   would redeliver it forever and hold back every record behind it.
//! - **The same V-rules as the REST doors** — a negative interval is stored
//!   with its warning.

use std::time::Duration;

use edmd::domain::{TimeSeriesQuery, TimeSeriesRepository};
use krafka::Record;
use krafka::testing::FakeBroker;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;

use crate::meterstore_integration::setup;

const TOPIC: &str = "edmd.meter-reads";
const GROUP: &str = "edmd-e2e";
const TENANT: &str = "9910000000010";

fn interval(from: OffsetDateTime, value: &str) -> serde_json::Value {
    let to = from + time::Duration::minutes(15);
    serde_json::json!({
        "from": from.format(&Rfc3339).expect("rfc3339"),
        "to": to.format(&Rfc3339).expect("rfc3339"),
        "value_kwh": value,
        "quality": "MEASURED",
        "obis_code": "1-0:1.8.0",
    })
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn produced_batches_are_stored_and_poison_pills_committed_past() {
    let (repo, pool, _pg, _wh) = setup().await;

    let broker = FakeBroker::start().await.expect("fake broker");
    broker.create_topic(TOPIC, 1);
    let kafka = broker.kafka().connect().await.expect("connect");
    let producer = kafka.producer().build().await.expect("producer");

    let t = (OffsetDateTime::now_utc() - time::Duration::days(1))
        .replace_second(0)
        .and_then(|t| t.replace_nanosecond(0))
        .expect("whole minute");
    let t = t - time::Duration::minutes(i64::from(t.minute() % 15));

    let valid = serde_json::json!({
        "malo_id": "51238696012",
        "sparte": "STROM",
        "source": "IOT_PUSH",
        "intervals": [interval(t, "1.25"), interval(t + time::Duration::minutes(15), "2.5")],
    });
    let refused = serde_json::json!({
        "malo_id": "51238696012",
        "sparte": "KOHLE",
        "intervals": [interval(t + time::Duration::minutes(30), "9")],
    });
    let anomalous = serde_json::json!({
        "malo_id": "51238696012",
        "intervals": [interval(t + time::Duration::minutes(45), "-4")],
    });

    for value in [
        valid.to_string().into_bytes(),
        b"{ this is not json".to_vec(),
        refused.to_string().into_bytes(),
        anomalous.to_string().into_bytes(),
    ] {
        let _metadata = producer
            .send(Record::new(TOPIC, value).key(&b"51238696012"[..]))
            .await
            .expect("produce");
    }
    producer.close().await.expect("producer close");

    let cfg = edmd::config::KafkaIngestConfig {
        message_hmac_secret: None,
        enabled: true,
        bootstrap_servers: broker.bootstrap_servers(),
        topic: TOPIC.to_owned(),
        group_id: GROUP.to_owned(),
        poll_ms: 200,
    };
    let shutdown = CancellationToken::new();
    edmd::kafka_ingest::spawn(cfg, repo.clone(), TENANT.to_owned(), shutdown.clone());

    // Every offset — the two poison pills included — is committed past.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while broker.committed_offset(GROUP, TOPIC, 0) != Some(4) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected committed offset 4, got {:?}",
            broker.committed_offset(GROUP, TOPIC, 0)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    shutdown.cancel();

    let reads = repo
        .query(&TimeSeriesQuery {
            malo_id: "51238696012".to_owned(),
            from: t - time::Duration::hours(1),
            to: t + time::Duration::hours(2),
            sparte: None,
            tenant: TENANT.to_owned(),
        })
        .await
        .expect("query");
    let values: Vec<String> = reads
        .iter()
        .map(|r| r.quantity_kwh.normalize().to_string())
        .collect();
    assert_eq!(
        values,
        ["1.25", "2.5", "-4"],
        "the two well-formed batches are stored and the refused one is not"
    );
    // The read-back carries no verdict; the warning is the outbox event every
    // ingest door raises, naming this one.
    let doors: Vec<String> = sqlx::query_scalar(
        "SELECT envelope->'data'->>'ingest_door' FROM event_outbox WHERE ce_type = $1",
    )
    .bind(mako_events::messwert::READING_QUALITY_WARNING)
    .fetch_all(&pool)
    .await
    .expect("read outbox");
    assert_eq!(
        doors,
        ["kafka-ingest"],
        "the negative interval went through the V-rules and raised exactly one warning"
    );
}
