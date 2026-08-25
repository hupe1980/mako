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
//!   "malo_id": "51238696012",
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
//! and a value-changing replay leaves a § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit row like any other
//! redelivery. Records that fail to parse are logged and skipped (a poison
//! pill must not wedge the partition); records that fail to store abort the
//! poll loop iteration without committing, so they are redelivered.
//!
//! Every batch runs the same V01–V09/V11/V12 `ValidatedReads::validate` pass and
//! the same Hampel grading as the REST ingest doors, and records the verdict in
//! `quality_assessments` — a "trusted" transport does not skip validation, and a
//! head-end feed is the least supervised door there is. An unrecognised `sparte`
//! or `source` is refused for the same reason: coercing them stored a gas batch
//! as electricity, in the wrong unit, with `MSCONS` provenance on values that
//! never went near EDIFACT.

use std::time::Duration;

use crate::domain::repository::TimeSeriesRepository;
use crate::domain::{IngestionSource, MeterRead, QualityFlag, Sparte};
use krafka::consumer::Consumer;
use rust_decimal::Decimal;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::KafkaIngestConfig;
use crate::store::MeterStoreTimeSeriesRepository;

/// One interval inside a Kafka batch document.
#[derive(Debug, serde::Deserialize)]
struct WireInterval {
    from: String,
    to: String,
    /// Energy in kWh. The name is the wire contract and it is *deliberately*
    /// `value_kwh` where `metering::MeterInterval` says `value`: that rename
    /// exists because an interval's unit is the Sparte's own (m³ for gas), but
    /// this wire carries no unit field and stores straight into `quantity_kwh` —
    /// it is kWh by contract, and the name should say so. (A blanket rename
    /// during the metering-0.17 migration changed this silently; head-end
    /// producers would have had every batch rejected with a missing-field
    /// error. Reverted, with this comment as the tripwire.)
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
    /// MSCONS correction version this batch is delivered under (≥ 14 digits).
    /// Present, it decides resolution; absent, arrival order does.
    #[serde(default)]
    mscons_version: Option<u128>,
    /// Physical capacity ceiling of the metered plant, in kW, enabling **V12**
    /// (`ImplausiblePower`). Absent, the rule stays off.
    #[serde(default)]
    max_plant_power_kw: Option<Decimal>,
    intervals: Vec<WireInterval>,
}

/// Outbound-webhook coordinates for the consumer's quality warnings.
///
/// The consumer runs outside the HTTP stack and so has no `HandlerState`; this
/// carries the two fields it needs rather than the whole thing.
#[derive(Clone)]
pub struct QualityAlertTarget {
    pub webhook_url: Option<String>,
    pub secret: Option<secrecy::SecretString>,
}

impl QualityAlertTarget {
    fn secret_bytes(&self) -> Option<&[u8]> {
        use secrecy::ExposeSecret;
        self.secret.as_ref().map(|s| s.expose_secret().as_bytes())
    }
}

/// Spawn the Kafka ingest consumer. Runs until `shutdown` is cancelled.
///
/// `alerts` is where a quality warning goes. A head-end feed is the least
/// supervised ingest door there is, so a V-rule finding on it has to reach the
/// same CloudEvent the REST doors raise, not just the log.
pub fn spawn(
    cfg: KafkaIngestConfig,
    repo: MeterStoreTimeSeriesRepository,
    tenant: String,
    alerts: QualityAlertTarget,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            if shutdown.is_cancelled() {
                break;
            }
            match run_consumer(&cfg, &repo, &tenant, &alerts, &shutdown).await {
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
    repo: &MeterStoreTimeSeriesRepository,
    tenant: &str,
    alerts: &QualityAlertTarget,
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

    // Per-message HMAC (optional): resolve the secret once. `env:` refs are
    // resolved here so the TOML never carries the raw key.
    let message_secret: Option<String> = match cfg.message_hmac_secret.as_deref() {
        Some(raw) => Some(
            crate::config::resolve_env(raw)
                .map_err(|e| anyhow::anyhow!("kafka message_hmac_secret: {e}"))?,
        ),
        None => None,
    };

    info!(
        topic = %cfg.topic,
        group = %cfg.group_id,
        message_auth = message_secret.is_some(),
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
            // Per-message authentication when configured: unauthenticated
            // records are skipped like poison pills — one forged producer
            // must not wedge the partition, and must not store data either.
            //
            // Standard Webhooks headers, on a Kafka record rather than an HTTP
            // request: `webhook-id` is the producer's message id and is what
            // binds the signature to *this* record, so a value replayed onto
            // another offset does not verify. The broker gives ordering and
            // retention that HTTP does not, so there is no timestamp-freshness
            // check here — the offset is the replay boundary.
            if let Some(ref secret) = message_secret {
                let header = |name: &[u8]| {
                    record
                        .headers
                        .iter()
                        .find(|(k, _)| k.as_ref() == name)
                        .and_then(|(_, v)| v.as_ref())
                        .and_then(|v| std::str::from_utf8(v).ok())
                };
                let ok = match (
                    header(mako_service::webhook::ID_HEADER.as_bytes()),
                    header(mako_service::webhook::TIMESTAMP_HEADER.as_bytes())
                        .and_then(|t| t.trim().parse::<i64>().ok()),
                    header(mako_service::webhook::SIGNATURE_HEADER.as_bytes()),
                ) {
                    (Some(id), Some(ts), Some(sig)) => mako_service::webhook::verify_signature(
                        secret.as_bytes(),
                        id,
                        ts,
                        value,
                        sig,
                    ),
                    _ => false,
                };
                if !ok {
                    warn!(
                        topic = %record.topic, partition = record.partition,
                        offset = record.offset,
                        "edmd kafka-ingest: message signature missing/invalid — record skipped"
                    );
                    continue;
                }
            }
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
            match store_batch(repo, tenant, alerts, batch).await {
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

/// Convert one wire batch into `MeterRead`s, run the V-rules, and store.
async fn store_batch(
    repo: &MeterStoreTimeSeriesRepository,
    tenant: &str,
    alerts: &QualityAlertTarget,
    batch: WireBatch,
) -> anyhow::Result<usize> {
    // An unrecognised Sparte or source is **refused**, not coerced — the same
    // rule the REST doors follow. Falling back silently put a gas batch into an
    // electricity store labelled STROM, and stamped `MSCONS` provenance on a
    // head-end feed that never went near EDIFACT (`IngestionSource::from_db_str`
    // is for read-back, where the column CHECK has already vouched for the
    // value). A record that fails here is a poison pill: it is skipped and
    // logged like any other unparseable one, so one broken producer does not
    // wedge the partition.
    let sparte = match batch.sparte.as_deref() {
        None => Sparte::Strom,
        Some(raw) => raw.trim().parse::<Sparte>().map_err(|_| {
            anyhow::anyhow!(
                "malo {}: unknown sparte `{raw}`; expected one of {:?}",
                batch.malo_id,
                Sparte::CODES
            )
        })?,
    };
    let source = match batch
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => IngestionSource::IotPush,
        Some(raw) => IngestionSource::parse_db_str(raw).ok_or_else(|| {
            anyhow::anyhow!(
                "malo {}: unknown source `{raw}`; expected one of {:?}",
                batch.malo_id,
                IngestionSource::ALL.map(IngestionSource::as_str)
            )
        })?,
    };

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
        // An unrecognised flag is refused, exactly as the REST doors refuse it.
        // Defaulting it to `MEASURED` would let a head-end typo turn a
        // non-billable reading into a billable one, on the least supervised
        // ingest door there is.
        let quality = match iv.quality.as_deref() {
            None => QualityFlag::Measured,
            Some(raw) => match crate::server::quality_flag_from_wire(raw) {
                Some(q) => q,
                None => anyhow::bail!(
                    "malo {}: unknown quality `{raw}` at {}",
                    batch.malo_id,
                    iv.from
                ),
            },
        };
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
            mscons_version: batch.mscons_version,
        });
    }
    if reads.is_empty() {
        return Ok(0);
    }

    // Same V-rule pass — and the same Hampel grade — as every REST ingest path.
    let malo_id = reads[0].malo_id.clone();
    let (validated, validation) = crate::domain::ValidatedReads::validate(
        reads,
        crate::domain::IngestContext::new("KAFKA_INGEST", &malo_id)
            .with_capacity_kw(batch.max_plant_power_kw),
    );
    // A Kafka record carries no session identifier, so the correlation id is
    // minted here and both extensions share it — better an honest per-batch id
    // than a borrowed field that means something else.
    let correlation_id = uuid::Uuid::new_v4().to_string();
    let (period_from, period_to) = crate::domain::batch_period(validated.as_slice());
    let hampel = crate::server::score_batch(validated.as_slice());

    let n = validated.len();
    repo.store_reads(validated)
        .await
        .map_err(|e| anyhow::anyhow!("store_reads: {e}"))?;
    if let Some(q) = &hampel {
        q.record(repo.pool(), tenant, &malo_id).await;
    }

    let alert = crate::server::quality_alert::QualityAlert {
        malo_id: &malo_id,
        door: "kafka-ingest",
        correlation_id: &correlation_id,
        causation_id: &correlation_id,
        sparte: Some(sparte.as_str()),
        period_from,
        period_to,
        validation: &validation,
        hampel: hampel
            .as_ref()
            .map(|q| crate::server::hampel_summary(&q.report)),
    };
    crate::server::quality_alert::raise_quality_warning(
        alerts.webhook_url.as_deref(),
        alerts.secret_bytes(),
        tenant,
        &alert,
    )
    .await;
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
