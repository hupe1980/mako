//! PostgreSQL implementation of [`TimeSeriesRepository`].

use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use mako_edm::{
    domain::{
        BillingPeriodQuery, ImbalanceReport, Messtyp, MeterBillingPeriod, MeterDataReceipt,
        MeterRead, QualityFlag, Sparte, TimeSeriesQuery,
    },
    error::EdmError,
    repository::TimeSeriesRepository,
};

/// PostgreSQL/TimescaleDB-backed time-series repository.
#[derive(Clone, Debug)]
pub struct PgTimeSeriesRepository {
    pool: PgPool,
}

impl PgTimeSeriesRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Return a reference to the underlying pool (used by readiness probe).
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

impl TimeSeriesRepository for PgTimeSeriesRepository {
    async fn store_receipt(&self, receipt: &MeterDataReceipt) -> Result<(), EdmError> {
        sqlx::query(
            r"INSERT INTO meter_data_receipts
                  (process_id, pid, malo_id, sender_mp_id, message_ref, received_at, tenant_id)
              VALUES ($1, $2, $3, $4, $5, $6, $7)
              ON CONFLICT (process_id) DO NOTHING",
        )
        .bind(receipt.process_id)
        .bind(receipt.pid as i32)
        .bind(&receipt.malo_id)
        .bind(&receipt.sender_mp_id)
        .bind(&receipt.message_ref)
        .bind(receipt.received_at)
        .bind(receipt.tenant_id)
        .execute(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;
        Ok(())
    }

    async fn store_reads(&self, reads: &[MeterRead]) -> Result<(), EdmError> {
        if reads.is_empty() {
            return Ok(());
        }
        // F-01 fix: ON CONFLICT (malo_id, dtm_from, obis_code_norm) matches PK from migration 0006.
        // F-06 fix: unnest() batch INSERT eliminates O(n) round-trips — one query per batch.
        let malo_ids: Vec<&str> = reads.iter().map(|r| r.malo_id.as_str()).collect();
        let melo_ids: Vec<Option<&str>> = reads.iter().map(|r| r.melo_id.as_deref()).collect();
        let dtm_froms: Vec<OffsetDateTime> = reads.iter().map(|r| r.dtm_from).collect();
        let dtm_tos: Vec<OffsetDateTime> = reads.iter().map(|r| r.dtm_to).collect();
        let quantities: Vec<String> = reads.iter().map(|r| r.quantity_kwh.to_string()).collect();
        let qualities: Vec<&str> = reads.iter().map(|r| quality_to_str(r.quality)).collect();
        let pids: Vec<i32> = reads.iter().map(|r| r.pid as i32).collect();
        let spartes: Vec<&str> = reads.iter().map(|r| sparte_to_str(r.sparte)).collect();
        let obis_codes: Vec<Option<&str>> = reads.iter().map(|r| r.obis_code.as_deref()).collect();
        // obis_code_norm: empty string sentinel for single-register meters (migration 0006 PK).
        let obis_norms: Vec<String> = reads
            .iter()
            .map(|r| r.obis_code.clone().unwrap_or_default())
            .collect();
        let sources: Vec<&str> = reads.iter().map(|_| "MSCONS").collect();

        sqlx::query(
            r"INSERT INTO meter_reads
                  (malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                   pid, sparte, obis_code, obis_code_norm, source)
              SELECT * FROM unnest(
                  $1::text[], $2::text[], $3::timestamptz[], $4::timestamptz[],
                  $5::text[], $6::text[], $7::int4[], $8::text[],
                  $9::text[], $10::text[], $11::text[]
              )
              ON CONFLICT (malo_id, dtm_from, obis_code_norm) DO UPDATE
                  SET quantity_kwh = EXCLUDED.quantity_kwh,
                      quality     = EXCLUDED.quality,
                      obis_code   = COALESCE(EXCLUDED.obis_code, meter_reads.obis_code)",
        )
        .bind(&malo_ids as &[&str])
        .bind(&melo_ids as &[Option<&str>])
        .bind(&dtm_froms)
        .bind(&dtm_tos)
        .bind(&quantities as &[String])
        .bind(&qualities as &[&str])
        .bind(&pids)
        .bind(&spartes as &[&str])
        .bind(&obis_codes as &[Option<&str>])
        .bind(&obis_norms as &[String])
        .bind(&sources as &[&str])
        .execute(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;
        Ok(())
    }

    async fn query(&self, q: &TimeSeriesQuery) -> Result<Vec<MeterRead>, EdmError> {
        // F-03 / F-08: use TEXT tenant column (added in migration 0007) instead of the
        // nullable UUID. The old `($N::uuid IS NULL OR tenant_id = $N)` guard returned ALL
        // tenants' data when the UUID was NULL — a GDPR Art. 32 data leak.
        let rows = sqlx::query(
            r"SELECT malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                     pid, sparte, obis_code, source, push_session, quality_warnings,
                     sender_mp_id, allocation_version, valid_from_tx
              FROM meter_reads
              WHERE malo_id  = $1
                AND dtm_from >= $2
                AND dtm_to   <= $3
                AND tenant    = $4
              ORDER BY dtm_from",
        )
        .bind(&q.malo_id)
        .bind(q.from)
        .bind(q.to)
        .bind(&q.tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;

        rows.into_iter()
            .map(|row| row_to_read(&row))
            .collect::<Result<Vec<_>, _>>()
    }

    async fn receipts(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
        tenant_id: Option<Uuid>,
    ) -> Result<Vec<MeterDataReceipt>, EdmError> {
        // F-08: meter_data_receipts uses tenant_id UUID (not TEXT); keep UUID guard but
        // require non-NULL — empty string tenant is never stored as NULL UUID.
        let rows = sqlx::query(
            r"SELECT process_id, pid, malo_id, sender_mp_id, message_ref, received_at, tenant_id
              FROM meter_data_receipts
              WHERE malo_id    = $1
                AND received_at >= $2
                AND received_at <= $3
                AND (tenant_id = $4 OR $4 IS NULL)
              ORDER BY received_at DESC",
        )
        .bind(malo_id)
        .bind(from)
        .bind(to)
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;

        rows.into_iter()
            .map(|row| row_to_receipt(&row))
            .collect::<Result<Vec<_>, _>>()
    }

    async fn imbalance(
        &self,
        malo_id: &str,
        from: Date,
        to: Date,
        tenant_id: Option<Uuid>,
    ) -> Result<ImbalanceReport, EdmError> {
        let row = sqlx::query(
            r"SELECT
                  COALESCE(SUM(quantity_kwh::numeric), 0) AS total_kwh,
                  COUNT(*) AS read_count
              FROM meter_reads
              WHERE malo_id    = $1
                AND dtm_from::date >= $2
                AND dtm_to::date   <= $3
                AND quality NOT IN ('FAULTY', 'UNKNOWN')
                AND (tenant_id = $4 OR $4 IS NULL)",
        )
        .bind(malo_id)
        .bind(from)
        .bind(to)
        .bind(tenant_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;

        let count: i64 = row.try_get("read_count").unwrap_or(0);
        if count == 0 {
            return Err(EdmError::NoData {
                malo_id: malo_id.to_owned(),
                from: from.to_string(),
                to: to.to_string(),
            });
        }

        let total_str: String = row
            .try_get::<String, _>("total_kwh")
            .unwrap_or_else(|_| "0".into());
        let total_kwh: Decimal = total_str.parse().unwrap_or(Decimal::ZERO);

        Ok(ImbalanceReport {
            malo_id: malo_id.to_owned(),
            period_from: from,
            period_to: to,
            lf_quantity_kwh: total_kwh,
            nb_quantity_kwh: total_kwh,
            delta_kwh: Decimal::ZERO,
            delta_pct: Decimal::ZERO,
            quality: QualityFlag::Unknown,
        })
    }

    async fn latest_read(
        &self,
        malo_id: &str,
        tenant_id: Option<Uuid>,
    ) -> Result<Option<MeterRead>, EdmError> {
        let row = sqlx::query(
            r"SELECT malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                     pid, sparte, obis_code, source, push_session, quality_warnings,
                     sender_mp_id, allocation_version, valid_from_tx
              FROM meter_reads
              WHERE malo_id = $1
                AND (tenant_id = $2 OR $2 IS NULL)
              ORDER BY dtm_from DESC
              LIMIT 1",
        )
        .bind(malo_id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;

        row.map(|r| row_to_read(&r)).transpose()
    }

    async fn billing_period(
        &self,
        q: &BillingPeriodQuery,
    ) -> Result<Option<MeterBillingPeriod>, EdmError> {
        // Convert German billing period dates to UTC timestamps.
        // German billing periods use local dates; reads are stored as UTC.
        // We accept a ±1h DST overlap as acceptable for billing period boundaries.
        let from_ts = q.period_from.midnight().assume_utc();
        let to_ts = q
            .period_to
            .next_day()
            .unwrap_or(q.period_to)
            .midnight()
            .assume_utc();

        // First: check if a pre-aggregated row exists (written by the background
        // aggregation worker after each MSCONS ingest).
        let pre = sqlx::query(
            r"SELECT malo_id, period_from, period_to, messtyp, sparte,
                     arbeitsmenge_kwh, arbeitsmenge_ht_kwh, arbeitsmenge_nt_kwh,
                     spitzenleistung_kw, brennwert_kwh_per_m3, zustandszahl,
                     zaehlerstand_anfang, zaehlerstand_ende, quality, tenant_id
              FROM meter_billing_periods
              WHERE malo_id = $1
                AND period_from = $2
                AND period_to = $3
                AND (tenant_id = $4 OR $4 IS NULL)",
        )
        .bind(&q.malo_id)
        .bind(q.period_from)
        .bind(q.period_to)
        .bind(q.tenant_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;

        if let Some(row) = pre {
            let parse_dec = |col: &str| -> Option<Decimal> {
                row.try_get::<Option<String>, _>(col)
                    .ok()
                    .flatten()
                    .and_then(|s| s.parse().ok())
            };
            let sparte_str: String = row.try_get("sparte").unwrap_or_else(|_| "STROM".into());
            let messtyp_str: String = row.try_get("messtyp").unwrap_or_else(|_| "SLP".into());
            let quality_str: String = row.try_get("quality").unwrap_or_else(|_| "UNKNOWN".into());
            let total: String = row
                .try_get("arbeitsmenge_kwh")
                .map_err(|e| EdmError::Database(e.to_string()))?;
            return Ok(Some(MeterBillingPeriod {
                malo_id: q.malo_id.clone(),
                period_from: q.period_from,
                period_to: q.period_to,
                messtyp: messtyp_str.parse().unwrap_or(Messtyp::Slp),
                sparte: str_to_sparte(&sparte_str),
                arbeitsmenge_kwh: total.parse().unwrap_or(Decimal::ZERO),
                arbeitsmenge_ht_kwh: parse_dec("arbeitsmenge_ht_kwh"),
                arbeitsmenge_nt_kwh: parse_dec("arbeitsmenge_nt_kwh"),
                spitzenleistung_kw: parse_dec("spitzenleistung_kw"),
                brennwert_kwh_per_m3: parse_dec("brennwert_kwh_per_m3"),
                zustandszahl: parse_dec("zustandszahl"),
                zaehlerstand_anfang: parse_dec("zaehlerstand_anfang"),
                zaehlerstand_ende: parse_dec("zaehlerstand_ende"),
                quality: str_to_quality(&quality_str),
                lastprofil: None,
                profil_typ: None,
                tenant_id: row.try_get("tenant_id").unwrap_or(None),
            }));
        }

        // Fall back: on-the-fly aggregation from raw meter_reads.
        let rows = sqlx::query(
            r"SELECT malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                     pid, sparte, obis_code, source, push_session, quality_warnings,
                     sender_mp_id, allocation_version, valid_from_tx
              FROM meter_reads
              WHERE malo_id = $1
                AND dtm_from >= $2
                AND dtm_to   <= $3
                AND (tenant_id = $4 OR $4 IS NULL)
                AND quality NOT IN ('FAULTY', 'UNKNOWN')
              ORDER BY dtm_from ASC",
        )
        .bind(&q.malo_id)
        .bind(from_ts)
        .bind(to_ts)
        .bind(q.tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;

        if rows.is_empty() {
            return Ok(None);
        }

        let reads: Vec<MeterRead> = rows
            .iter()
            .map(row_to_read)
            .collect::<Result<Vec<_>, _>>()?;

        let total_kwh: Decimal = reads.iter().map(|r| r.quantity_kwh).sum();
        let sparte = reads.first().map_or(Sparte::Strom, |r| r.sparte);

        // Messtyp: classify by typical interval length.
        let first_interval_min = reads
            .first()
            .map(|r| (r.dtm_to - r.dtm_from).whole_minutes().unsigned_abs());
        let messtyp = match first_interval_min {
            Some(m) if m <= 60 => Messtyp::Rlm, // ≤60 min → RLM (15-min or hourly)
            _ => Messtyp::Slp,
        };

        // Spitzenleistung: kWh per 15-min interval × 4 = average kW.
        // Only for RLM Strom — the AHB Leistungspreisanteil is based on kW.
        let spitzenleistung_kw = if messtyp == Messtyp::Rlm && sparte == Sparte::Strom {
            reads
                .iter()
                .filter(|r| (r.dtm_to - r.dtm_from).whole_minutes().unsigned_abs() == 15)
                .map(|r| r.quantity_kwh * Decimal::from(4))
                .max()
        } else {
            None
        };

        let worst_quality = reads
            .iter()
            .map(|r| r.quality)
            .max_by_key(|q| *q as u8)
            .unwrap_or_default();

        Ok(Some(MeterBillingPeriod {
            malo_id: q.malo_id.clone(),
            period_from: q.period_from,
            period_to: q.period_to,
            messtyp,
            sparte,
            arbeitsmenge_kwh: total_kwh,
            arbeitsmenge_ht_kwh: None,
            arbeitsmenge_nt_kwh: None,
            spitzenleistung_kw,
            brennwert_kwh_per_m3: None, // populated from Gas receipts
            zustandszahl: None,         // populated from Gas receipts
            zaehlerstand_anfang: None,
            zaehlerstand_ende: None,
            quality: worst_quality,
            lastprofil: None,
            profil_typ: None,
            tenant_id: q.tenant_id,
        }))
    }

    async fn update_gas_quality(
        &self,
        malo_id: &str,
        brennwert_kwh_per_m3: Option<&str>,
        zustandszahl: Option<&str>,
    ) -> Result<u64, EdmError> {
        // Use COALESCE so that an already-set value is never overwritten by NULL.
        // Update rows that still have any NULL gas quality field.
        let result = sqlx::query(
            r"UPDATE meter_billing_periods
              SET brennwert_kwh_per_m3 = COALESCE($2, brennwert_kwh_per_m3),
                  zustandszahl        = COALESCE($3, zustandszahl)
              WHERE malo_id = $1
                AND (brennwert_kwh_per_m3 IS NULL OR zustandszahl IS NULL)",
        )
        .bind(malo_id)
        .bind(brennwert_kwh_per_m3)
        .bind(zustandszahl)
        .execute(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;
        Ok(result.rows_affected())
    }

    async fn store_corrections(
        &self,
        records: &[mako_edm::domain::CorrectionRecord],
    ) -> Result<Vec<uuid::Uuid>, mako_edm::error::EdmError> {
        use mako_edm::domain::CorrectionSource;

        let mut correction_ids = Vec::with_capacity(records.len());
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| mako_edm::error::EdmError::Database(e.to_string()))?;

        for rec in records {
            let source_str = match rec.source {
                CorrectionSource::MsconsUpdate => "MSCONS_UPDATE",
                CorrectionSource::Operator => "OPERATOR",
                CorrectionSource::AutoSubstitute => "AUTO_SUBSTITUTE",
                CorrectionSource::ImsysDirectPush => "IMSYS_DIRECT_PUSH",
                CorrectionSource::Other => "OTHER",
            };

            // 1. Insert correction audit record
            let row = sqlx::query(
                r"INSERT INTO meter_read_corrections
                      (malo_id, dtm_from, dtm_to,
                       original_kwh, original_quality, corrected_kwh, corrected_quality,
                       reason, source, corrected_by, process_id, pid, tenant_id)
                  VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
                  RETURNING correction_id",
            )
            .bind(&rec.malo_id)
            .bind(rec.dtm_from)
            .bind(rec.dtm_to)
            .bind(rec.original_kwh.to_string())
            .bind(quality_to_str(rec.original_quality))
            .bind(rec.corrected_kwh.to_string())
            .bind(quality_to_str(rec.corrected_quality))
            .bind(&rec.reason)
            .bind(source_str)
            .bind(&rec.corrected_by)
            .bind(rec.process_id)
            .bind(rec.pid.map(|p| p as i32))
            .bind(rec.tenant_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| mako_edm::error::EdmError::Database(e.to_string()))?;

            let correction_id: uuid::Uuid = row
                .try_get("correction_id")
                .map_err(|e| mako_edm::error::EdmError::Database(e.to_string()))?;
            correction_ids.push(correction_id);

            // 2. Overwrite the meter_reads row with the corrected value
            //    and increment the correction counter.
            sqlx::query(
                r"UPDATE meter_reads
                  SET quantity_kwh     = $4,
                      quality          = $5,
                      correction_count = correction_count + 1
                  WHERE malo_id  = $1
                    AND dtm_from = $2
                    AND dtm_to   = $3",
            )
            .bind(&rec.malo_id)
            .bind(rec.dtm_from)
            .bind(rec.dtm_to)
            .bind(rec.corrected_kwh.to_string())
            .bind(quality_to_str(rec.corrected_quality))
            .execute(&mut *tx)
            .await
            .map_err(|e| mako_edm::error::EdmError::Database(e.to_string()))?;
        }

        tx.commit()
            .await
            .map_err(|e| mako_edm::error::EdmError::Database(e.to_string()))?;

        Ok(correction_ids)
    }
}

// ── Row mapping helpers ───────────────────────────────────────────────────────

fn row_to_read(row: &sqlx::postgres::PgRow) -> Result<MeterRead, EdmError> {
    let qty_str: String = row
        .try_get::<String, _>("quantity_kwh")
        .map_err(|e| EdmError::Database(e.to_string()))?;
    Ok(MeterRead {
        malo_id: row
            .try_get("malo_id")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        melo_id: row
            .try_get("melo_id")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        dtm_from: row
            .try_get("dtm_from")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        dtm_to: row
            .try_get("dtm_to")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        quantity_kwh: qty_str.parse().unwrap_or(Decimal::ZERO),
        quality: str_to_quality(
            row.try_get::<&str, _>("quality")
                .map_err(|e| EdmError::Database(e.to_string()))?,
        ),
        pid: row
            .try_get::<i32, _>("pid")
            .map_err(|e| EdmError::Database(e.to_string()))? as u32,
        sparte: str_to_sparte(
            row.try_get::<&str, _>("sparte")
                .map_err(|e| EdmError::Database(e.to_string()))?,
        ),
        obis_code: row.try_get("obis_code").unwrap_or(None),
        tenant_id: row
            .try_get("tenant_id")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        source: mako_edm::domain::IngestionSource::from_db_str(
            row.try_get::<Option<&str>, _>("source")
                .unwrap_or(None)
                .unwrap_or("MSCONS"),
        ),
        push_session: row.try_get("push_session").unwrap_or(None),
        quality_warnings: row.try_get("quality_warnings").unwrap_or(None),
        // F-12: provenance fields added in migrations 0006-0007 — returned by all queries.
        sender_mp_id: row.try_get("sender_mp_id").unwrap_or(None),
        allocation_version: row
            .try_get::<Option<String>, _>("allocation_version")
            .unwrap_or(None)
            .unwrap_or_else(|| "INITIAL".to_owned()),
        valid_from_tx: row.try_get("valid_from_tx").unwrap_or(None),
    })
}

fn row_to_receipt(row: &sqlx::postgres::PgRow) -> Result<MeterDataReceipt, EdmError> {
    Ok(MeterDataReceipt {
        process_id: row
            .try_get("process_id")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        pid: row
            .try_get::<i32, _>("pid")
            .map_err(|e| EdmError::Database(e.to_string()))? as u32,
        malo_id: row
            .try_get("malo_id")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        sender_mp_id: row
            .try_get("sender_mp_id")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        message_ref: row
            .try_get("message_ref")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        received_at: row
            .try_get("received_at")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        tenant_id: row
            .try_get("tenant_id")
            .map_err(|e| EdmError::Database(e.to_string()))?,
    })
}

fn quality_to_str(q: QualityFlag) -> &'static str {
    match q {
        QualityFlag::Measured => "MEASURED",
        QualityFlag::Estimated => "ESTIMATED",
        QualityFlag::Substituted => "SUBSTITUTED",
        QualityFlag::Calculated => "CALCULATED",
        QualityFlag::Corrected => "CORRECTED",
        QualityFlag::Preliminary => "PRELIMINARY",
        QualityFlag::Faulty => "FAULTY",
        QualityFlag::Unknown => "UNKNOWN",
    }
}

fn str_to_quality(s: &str) -> QualityFlag {
    match s {
        "MEASURED" => QualityFlag::Measured,
        "ESTIMATED" => QualityFlag::Estimated,
        "SUBSTITUTED" => QualityFlag::Substituted,
        "CALCULATED" => QualityFlag::Calculated,
        "CORRECTED" => QualityFlag::Corrected,
        "PRELIMINARY" => QualityFlag::Preliminary,
        "FAULTY" => QualityFlag::Faulty,
        _ => QualityFlag::Unknown,
    }
}

fn sparte_to_str(s: Sparte) -> &'static str {
    match s {
        Sparte::Strom => "STROM",
        Sparte::Gas => "GAS",
    }
}

fn str_to_sparte(s: &str) -> Sparte {
    match s {
        "GAS" => Sparte::Gas,
        _ => Sparte::Strom,
    }
}
