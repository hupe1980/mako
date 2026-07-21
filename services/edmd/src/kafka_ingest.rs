//! High-throughput Kafka ingest path for meter readings.
//!
//! The webhook path (marktd fan-out) and the direct-push REST endpoints cover
//! MaKo-driven and per-gateway delivery. At fleet scale — hundreds of
//! thousands of iMSys pushing quarter-hour values — per-request HTTP becomes
//! the bottleneck; a head-end system or LoRaWAN network server streams batches
//! into a Kafka topic instead, and this consumer drains it.
//!
//! ## Wire format
//!
//! One JSON document per Kafka record, the same batch shape the bulk REST
//! endpoint accepts:
//!
//! ```json
//! {
//!   "malo_id": "51238696781",
//!   "sparte": "STROM",
//!   "source": "IOT_PUSH",
//!   "intervals": [
//!     {"from": "2026-07-01T00:00:00Z", "to": "2026-07-01T00:15:00Z",
//!      "value_kwh": "1.25", "quality": "MEASURED", "obis_code": "1-0:1.8.0"}
//!   ]
//! }
//! ```
//!
//! ## Delivery semantics
//!
//! At-least-once: offsets are committed only after the batch is stored. A
//! replayed batch is idempotent — `store_reads` upserts on the primary key,
//! and a value-changing replay leaves a §22 MessZV audit row like any other
//! redelivery. Records that fail to parse are logged and skipped (a poison
//! pill must not wedge the partition); records that fail to store abort the
//! poll loop iteration without committing, so they are redelivered.
//!
//! Every batch runs the same V01–V10 `validate_and_annotate` pass as the REST
//! ingest paths — a "trusted" transport does not skip validation.

use std::time::Duration;

use krafka::consumer::Consumer;
use mako_edm::domain::{IngestionSource, MeterRead, QualityFlag, Sparte};
use mako_edm::repository::TimeSeriesRepository;
use rust_decimal::Decimal;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::KafkaIngestConfig;
use crate::pg::PgTimeSeriesRepository;

/// One interval inside a Kafka batch document.
#[derive(Debug, serde::Deserialize)]
struct WireInterval {
    from: String,
    to: String,
    value_kwh: Decimal,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    obis_code: Option<String>,
}

/// One Kafka record: a batch of intervals for one MaLo.
#[derive(Debug, serde::Deserialize)]
struct WireBatch {
    malo_id: String,
    #[serde(default)]
    melo_id: Option<String>,
    #[serde(default)]
    sparte: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    sender_mp_id: Option<String>,
    intervals: Vec<WireInterval>,
}

/// Spawn the Kafka ingest consumer. Runs until `shutdown` is cancelled.
pub fn spawn(
    cfg: KafkaIngestConfig,
    repo: PgTimeSeriesRepository,
    tenant: String,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            if shutdown.is_cancelled() {
                break;
            }
            match run_consumer(&cfg, &repo, &tenant, &shutdown).await {
                Ok(()) => break, // clean shutdown
                Err(e) => {
                    error!(error = %e, "edmd kafka-ingest: consumer failed — reconnecting in 5s");
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                }
            }
        }
        info!("edmd kafka-ingest: stopped");
    });
}

async fn run_consumer(
    cfg: &KafkaIngestConfig,
    repo: &PgTimeSeriesRepository,
    tenant: &str,
    shutdown: &CancellationToken,
) -> anyhow::Result<()> {
    let consumer: Consumer = Consumer::builder()
        .bootstrap_servers(cfg.bootstrap_servers.clone())
        .group_id(cfg.group_id.clone())
        .client_id("edmd-kafka-ingest")
        // A fresh group must start at the beginning of the topic: with the
        // client default (`Latest`), every record produced before the group's
        // first commit would be silently lost — meter readings are not a
        // live feed to tail but a backlog to drain (caught by the FakeBroker
        // e2e test).
        .auto_offset_reset(krafka::consumer::AutoOffsetReset::Earliest)
        // Offsets are committed manually after a successful store — the
        // at-least-once contract depends on it.
        .enable_auto_commit(false)
        .build()
        .await
        .map_err(|e| anyhow::anyhow!("kafka consumer build: {e}"))?;

    consumer
        .subscribe(&[cfg.topic.as_str()])
        .await
        .map_err(|e| anyhow::anyhow!("kafka subscribe {}: {e}", cfg.topic))?;

    info!(
        topic = %cfg.topic,
        group = %cfg.group_id,
        "edmd kafka-ingest: consuming"
    );

    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        let records = tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            r = consumer.poll(Duration::from_millis(cfg.poll_ms)) => {
                r.map_err(|e| anyhow::anyhow!("kafka poll: {e}"))?
            }
        };
        if records.is_empty() {
            continue;
        }

        let mut stored_batches = 0usize;
        for record in &records {
            let Some(value) = record.value.as_ref() else {
                continue; // tombstone
            };
            let batch: WireBatch = match serde_json::from_slice(value) {
                Ok(b) => b,
                Err(e) => {
                    // Poison pill: skipping is deliberate — one malformed
                    // producer must not wedge the whole partition.
                    warn!(
                        topic = %record.topic, partition = record.partition,
                        offset = record.offset, error = %e,
                        "edmd kafka-ingest: unparseable record skipped"
                    );
                    continue;
                }
            };
            match store_batch(repo, tenant, batch).await {
                Ok(n) => {
                    stored_batches += 1;
                    tracing::debug!(intervals = n, "edmd kafka-ingest: batch stored");
                }
                Err(e) => {
                    // Storage failure: abort without committing so the batch
                    // (and everything after it) is redelivered.
                    return Err(anyhow::anyhow!("store failed: {e}"));
                }
            }
        }

        consumer
            .commit()
            .await
            .map_err(|e| anyhow::anyhow!("kafka commit: {e}"))?;
        if stored_batches > 0 {
            info!(
                records = records.len(),
                stored_batches, "edmd kafka-ingest: offsets committed"
            );
        }
    }
}

/// Convert one wire batch into `MeterRead`s, run V01–V10, and store.
async fn store_batch(
    repo: &PgTimeSeriesRepository,
    tenant: &str,
    batch: WireBatch,
) -> anyhow::Result<usize> {
    let sparte = match batch
        .sparte
        .as_deref()
        .unwrap_or("STROM")
        .to_uppercase()
        .as_str()
    {
        "GAS" => Sparte::Gas,
        "WAERME" | "WÄRME" => Sparte::Waerme,
        "WASSER" => Sparte::Wasser,
        _ => Sparte::Strom,
    };
    let source = IngestionSource::from_db_str(batch.source.as_deref().unwrap_or("IOT_PUSH"));

    let mut reads: Vec<MeterRead> = Vec::with_capacity(batch.intervals.len());
    for iv in &batch.intervals {
        let (Ok(from), Ok(to)) = (
            OffsetDateTime::parse(&iv.from, &Rfc3339),
            OffsetDateTime::parse(&iv.to, &Rfc3339),
        ) else {
            anyhow::bail!(
                "malo {}: unparseable interval timestamps {:?}..{:?}",
                batch.malo_id,
                iv.from,
                iv.to
            );
        };
        if from >= to {
            anyhow::bail!("malo {}: interval from >= to at {from}", batch.malo_id);
        }
        let quality = iv
            .quality
            .as_deref()
            .map(|q| match q.to_uppercase().as_str() {
                "ESTIMATED" => QualityFlag::Estimated,
                "SUBSTITUTED" => QualityFlag::Substituted,
                "CALCULATED" => QualityFlag::Calculated,
                "CORRECTED" => QualityFlag::Corrected,
                "PRELIMINARY" => QualityFlag::Preliminary,
                "FAULTY" => QualityFlag::Faulty,
                "UNKNOWN" => QualityFlag::Unknown,
                _ => QualityFlag::Measured,
            })
            .unwrap_or(QualityFlag::Measured);
        reads.push(MeterRead {
            malo_id: batch.malo_id.clone(),
            melo_id: batch.melo_id.clone(),
            dtm_from: from,
            dtm_to: to,
            quantity_kwh: iv.value_kwh,
            quality,
            pid: 0,
            sparte,
            obis_code: iv.obis_code.clone(),
            tenant: tenant.to_owned(),
            source,
            push_session: None,
            quality_warnings: None,
            sender_mp_id: batch.sender_mp_id.clone(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: None,
        });
    }
    if reads.is_empty() {
        return Ok(0);
    }

    // Same V01–V10 pass as every REST ingest path.
    let malo_id = reads[0].malo_id.clone();
    let validation = crate::server::validate_and_annotate(&mut reads, "KAFKA_INGEST", &malo_id);
    if validation.billing_block_count > 0 {
        warn!(
            malo_id = %malo_id,
            billing_blocks = validation.billing_block_count,
            rules = ?validation.rules,
            "edmd kafka-ingest: validation issues annotated (§17 MessZV)"
        );
    }

    let n = reads.len();
    repo.store_reads(&reads)
        .await
        .map_err(|e| anyhow::anyhow!("store_reads: {e}"))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_batch_parses_the_documented_shape() {
        let json = r#"{
            "malo_id": "51238696781",
            "sparte": "STROM",
            "source": "IOT_PUSH",
            "intervals": [
                {"from": "2026-07-01T00:00:00Z", "to": "2026-07-01T00:15:00Z",
                 "value_kwh": "1.25", "quality": "MEASURED", "obis_code": "1-0:1.8.0"}
            ]
        }"#;
        let batch: WireBatch = serde_json::from_str(json).expect("documented shape parses");
        assert_eq!(batch.malo_id, "51238696781");
        assert_eq!(batch.intervals.len(), 1);
        assert_eq!(batch.intervals[0].value_kwh, rust_decimal::dec!(1.25));
    }

    #[test]
    fn unknown_fields_do_not_break_parsing() {
        let json = r#"{"malo_id": "51238696781", "intervals": [], "extra": 1}"#;
        let batch: WireBatch = serde_json::from_str(json).expect("lenient parse");
        assert!(batch.intervals.is_empty());
    }
}
