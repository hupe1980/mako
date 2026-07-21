//! PostgreSQL implementation of [`TimeSeriesRepository`].

use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};

use mako_edm::{
    domain::{
        BillingPeriodQuery, ImbalanceReport, Messtyp, MeterBillingPeriod, MeterDataReceipt,
        MeterRead, QualityFlag, Sparte, TimeSeriesQuery,
    },
    error::EdmError,
    repository::TimeSeriesRepository,
};

/// PostgreSQL-backed time-series repository (monthly RANGE partitions).
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
                  (process_id, pid, malo_id, sender_mp_id, message_ref, received_at, tenant)
              VALUES ($1, $2, $3, $4, $5, $6, $7)
              ON CONFLICT (process_id) DO NOTHING",
        )
        .bind(receipt.process_id)
        .bind(receipt.pid as i32)
        .bind(&receipt.malo_id)
        .bind(&receipt.sender_mp_id)
        .bind(&receipt.message_ref)
        .bind(receipt.received_at)
        .bind(&receipt.tenant)
        .execute(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;
        Ok(())
    }

    async fn store_reads(&self, reads: &[MeterRead]) -> Result<(), EdmError> {
        if reads.is_empty() {
            return Ok(());
        }
        // Batch INSERT via unnest() — one round-trip per batch.
        // ON CONFLICT (tenant, malo_id, dtm_from, obis_code_norm) is the primary key.
        let malo_ids: Vec<&str> = reads.iter().map(|r| r.malo_id.as_str()).collect();
        let melo_ids: Vec<Option<&str>> = reads.iter().map(|r| r.melo_id.as_deref()).collect();
        let dtm_froms: Vec<OffsetDateTime> = reads.iter().map(|r| r.dtm_from).collect();
        let dtm_tos: Vec<OffsetDateTime> = reads.iter().map(|r| r.dtm_to).collect();
        let quantities: Vec<Decimal> = reads.iter().map(|r| r.quantity_kwh).collect();
        let qualities: Vec<&str> = reads.iter().map(|r| quality_to_str(r.quality)).collect();
        let pids: Vec<i32> = reads.iter().map(|r| r.pid as i32).collect();
        let spartes: Vec<&str> = reads.iter().map(|r| sparte_to_str(r.sparte)).collect();
        let obis_codes: Vec<Option<&str>> = reads.iter().map(|r| r.obis_code.as_deref()).collect();
        // Normalise before it enters the primary key: `1-0:1.8.0` and
        // `1-0:1.8.0*255` are the same register.
        let obis_norms: Vec<String> = reads
            .iter()
            .map(|r| normalise_obis(r.obis_code.as_deref()))
            .collect();
        // Provenance travels with the reading: `allocation_version` carries the
        // MaBiS vorläufig/endgültig distinction, and `sender_mp_id` carries §22
        // MessZV per-interval MSB attribution across a WiM switch.
        let sources: Vec<&str> = reads.iter().map(|r| r.source.as_str()).collect();
        let tenants: Vec<&str> = reads.iter().map(|r| r.tenant.as_str()).collect();
        let push_sessions: Vec<Option<&str>> =
            reads.iter().map(|r| r.push_session.as_deref()).collect();
        let quality_warnings: Vec<Option<serde_json::Value>> =
            reads.iter().map(|r| r.quality_warnings.clone()).collect();
        let sender_mp_ids: Vec<Option<&str>> =
            reads.iter().map(|r| r.sender_mp_id.as_deref()).collect();
        let allocation_versions: Vec<&str> = reads
            .iter()
            .map(|r| r.allocation_version.as_str())
            .collect();
        let units: Vec<&str> = reads
            .iter()
            .map(|r| r.sparte.billing_unit().as_str())
            .collect();

        sqlx::query(
            r"WITH incoming AS (
                  SELECT * FROM unnest(
                      $1::text[], $2::text[], $3::timestamptz[], $4::timestamptz[],
                      $5::numeric[], $6::text[], $7::int4[], $8::text[],
                      $9::text[], $10::text[], $11::text[], $12::text[],
                      $13::text[], $14::jsonb[], $15::text[], $16::text[], $17::text[]
                  ) AS t(malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                         pid, sparte, obis_code, obis_code_norm, source, tenant,
                         push_session, quality_warnings, sender_mp_id,
                         allocation_version, unit)
              ),
              -- §22 MessZV: an overwrite must never be silent. Any redelivery
              -- that changes a stored value or quality leaves an immutable
              -- audit row BEFORE the upsert applies — the CTE join sees the
              -- pre-statement snapshot, so `original_*` is the displaced value.
              -- This is what keeps the `as_of` bitemporal overlay complete.
              audit AS (
                  INSERT INTO meter_read_corrections
                      (malo_id, dtm_from, dtm_to, obis_code_norm,
                       original_kwh, original_quality,
                       corrected_kwh, corrected_quality,
                       reason, source, corrected_by, pid, tenant)
                  SELECT mr.malo_id, mr.dtm_from, mr.dtm_to, mr.obis_code_norm,
                         mr.quantity_kwh, mr.quality,
                         i.quantity_kwh, i.quality,
                         'Neulieferung überschreibt gespeichertes Intervall (automatischer §22-MessZV-Audit-Eintrag)',
                         CASE
                             WHEN i.source = 'MSCONS' THEN 'MSCONS_UPDATE'
                             WHEN i.source IN ('DIRECT_PUSH', 'DIRECT_GAS', 'IOT_PUSH')
                                 THEN 'IMSYS_DIRECT_PUSH'
                             WHEN i.source = 'AUTO_SUBSTITUTE' THEN 'AUTO_SUBSTITUTE'
                             ELSE 'OTHER'
                         END,
                         'edmd-ingest', i.pid, mr.tenant
                  FROM incoming i
                  JOIN meter_reads mr
                    ON mr.tenant = i.tenant
                   AND mr.malo_id = i.malo_id
                   AND mr.dtm_from = i.dtm_from
                   AND mr.obis_code_norm = i.obis_code_norm
                  WHERE mr.quantity_kwh IS DISTINCT FROM i.quantity_kwh
                     OR mr.quality IS DISTINCT FROM i.quality
              )
              INSERT INTO meter_reads
                  (malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                   pid, sparte, obis_code, obis_code_norm, source, tenant,
                   push_session, quality_warnings, sender_mp_id,
                   allocation_version, unit)
              SELECT malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                     pid, sparte, obis_code, obis_code_norm, source, tenant,
                     push_session, quality_warnings, sender_mp_id,
                     allocation_version, unit
              FROM incoming
              ON CONFLICT (tenant, malo_id, dtm_from, obis_code_norm) DO UPDATE
                  SET quantity_kwh       = EXCLUDED.quantity_kwh,
                      quality            = EXCLUDED.quality,
                      obis_code          = COALESCE(EXCLUDED.obis_code, meter_reads.obis_code),
                      quality_warnings   = COALESCE(EXCLUDED.quality_warnings,
                                                    meter_reads.quality_warnings),
                      sender_mp_id       = COALESCE(EXCLUDED.sender_mp_id,
                                                    meter_reads.sender_mp_id),
                      allocation_version = EXCLUDED.allocation_version,
                      -- `valid_from_tx` doubles as the row version. The archival
                      -- worker only marks a row archived if it still matches the
                      -- version it exported, so bumping it here makes a write
                      -- that races an in-flight export visible to that check.
                      valid_from_tx      = now(),
                      -- A row whose value changed after export no longer matches
                      -- what Iceberg holds, so it is owed a fresh export. Leaving
                      -- `archived` set would let partition release drop the
                      -- correction and leave the stale cold-tier value as the
                      -- only surviving copy.
                      archived           = false",
        )
        .bind(&malo_ids as &[&str])
        .bind(&melo_ids as &[Option<&str>])
        .bind(&dtm_froms)
        .bind(&dtm_tos)
        .bind(&quantities as &[Decimal])
        .bind(&qualities as &[&str])
        .bind(&pids)
        .bind(&spartes as &[&str])
        .bind(&obis_codes as &[Option<&str>])
        .bind(&obis_norms as &[String])
        .bind(&sources as &[&str])
        .bind(&tenants as &[&str])
        .bind(&push_sessions as &[Option<&str>])
        .bind(&quality_warnings as &[Option<serde_json::Value>])
        .bind(&sender_mp_ids as &[Option<&str>])
        .bind(&allocation_versions as &[&str])
        .bind(&units as &[&str])
        .execute(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;

        // Drop any cached billing-period aggregate the new readings fall inside.
        //
        // `meter_billing_periods` is populated read-through, so a query issued
        // mid-period caches a partial sum. Without this the cached total is
        // returned for that period forever — including to `billingd` — because
        // the read path prefers the cache and nothing else invalidates it.
        let periods: Vec<(String, String, OffsetDateTime, OffsetDateTime)> = {
            let mut acc: std::collections::HashMap<(&str, &str), (OffsetDateTime, OffsetDateTime)> =
                std::collections::HashMap::new();
            for r in reads {
                let e = acc
                    .entry((r.tenant.as_str(), r.malo_id.as_str()))
                    .or_insert((r.dtm_from, r.dtm_to));
                e.0 = e.0.min(r.dtm_from);
                e.1 = e.1.max(r.dtm_to);
            }
            acc.into_iter()
                .map(|((t, m), (from, to))| (t.to_owned(), m.to_owned(), from, to))
                .collect()
        };

        for (tenant, malo_id, from, to) in periods {
            if let Err(e) = sqlx::query(
                r"DELETE FROM meter_billing_periods
                  WHERE tenant  = $1
                    AND malo_id = $2
                    AND period_from <= $4::date
                    AND period_to   >= $3::date",
            )
            .bind(&tenant)
            .bind(&malo_id)
            .bind(from.date())
            .bind(to.date())
            .execute(&self.pool)
            .await
            {
                // The readings are committed; a stale aggregate is a wrong
                // answer rather than a lost one, so it is surfaced and the
                // ingest still succeeds.
                tracing::warn!(
                    %malo_id, error = %e,
                    "edmd: could not invalidate cached billing period after ingest"
                );
            }
        }

        Ok(())
    }

    async fn query(&self, q: &TimeSeriesQuery) -> Result<Vec<MeterRead>, EdmError> {
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
        tenant: &str,
    ) -> Result<Vec<MeterDataReceipt>, EdmError> {
        // `meter_data_receipts.tenant` is TEXT NOT NULL — exact match required.
        // Cross-tenant queries are not permitted.
        let rows = sqlx::query(
            r"SELECT process_id, pid, malo_id, sender_mp_id, message_ref, received_at, tenant
              FROM meter_data_receipts
              WHERE malo_id    = $1
                AND received_at >= $2
                AND received_at <= $3
                AND tenant       = $4
              ORDER BY received_at DESC",
        )
        .bind(malo_id)
        .bind(from)
        .bind(to)
        .bind(tenant)
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
        tenant: &str,
    ) -> Result<ImbalanceReport, EdmError> {
        let row = sqlx::query(
            r"SELECT
                  COALESCE(SUM(quantity_kwh), 0) AS total_kwh,
                  COUNT(*) AS read_count
              FROM meter_reads
              WHERE malo_id    = $1
                AND dtm_from::date >= $2
                AND dtm_to::date   <= $3
                AND quality NOT IN ('FAULTY', 'UNKNOWN')
                AND tenant = $4",
        )
        .bind(malo_id)
        .bind(from)
        .bind(to)
        .bind(tenant)
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

        let total_kwh: Decimal = row.try_get("total_kwh").unwrap_or(Decimal::ZERO);

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
        tenant: &str,
    ) -> Result<Option<MeterRead>, EdmError> {
        let row = sqlx::query(
            r"SELECT malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                     pid, sparte, obis_code, source, push_session, quality_warnings,
                     sender_mp_id, allocation_version, valid_from_tx
              FROM meter_reads
              WHERE malo_id = $1
                AND tenant  = $2
              ORDER BY dtm_from DESC
              LIMIT 1",
        )
        .bind(malo_id)
        .bind(tenant)
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
                     arbeitsmenge_kwh, spitzenleistung_kw,
                     arbeitsmenge_ht_kwh, arbeitsmenge_nt_kwh,
                     brennwert_kwh_per_m3, zustandszahl,
                     zaehlerstand_anfang, zaehlerstand_ende, quality
              FROM meter_billing_periods
              WHERE malo_id = $1
                AND period_from = $2
                AND period_to = $3
                AND tenant = $4",
        )
        .bind(&q.malo_id)
        .bind(q.period_from)
        .bind(q.period_to)
        .bind(&q.tenant)
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
            let arbeitsmenge_kwh: Decimal =
                row.try_get("arbeitsmenge_kwh").unwrap_or(Decimal::ZERO);
            let spitzenleistung_kw: Option<Decimal> =
                row.try_get("spitzenleistung_kw").unwrap_or(None);
            return Ok(Some(MeterBillingPeriod {
                malo_id: q.malo_id.clone(),
                period_from: q.period_from,
                period_to: q.period_to,
                messtyp: messtyp_str.parse().unwrap_or(Messtyp::Slp),
                sparte: str_to_sparte(&sparte_str),
                arbeitsmenge_kwh,
                arbeitsmenge_ht_kwh: parse_dec("arbeitsmenge_ht_kwh"),
                arbeitsmenge_nt_kwh: parse_dec("arbeitsmenge_nt_kwh"),
                spitzenleistung_kw,
                brennwert_kwh_per_m3: parse_dec("brennwert_kwh_per_m3"),
                zustandszahl: parse_dec("zustandszahl"),
                zaehlerstand_anfang: parse_dec("zaehlerstand_anfang"),
                zaehlerstand_ende: parse_dec("zaehlerstand_ende"),
                quality: str_to_quality(&quality_str),
                lastprofil: None,
                profil_typ: None,
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
                AND tenant = $4
                AND quality NOT IN ('FAULTY', 'UNKNOWN')
              ORDER BY dtm_from ASC",
        )
        .bind(&q.malo_id)
        .bind(from_ts)
        .bind(to_ts)
        .bind(&q.tenant)
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

        let result = MeterBillingPeriod {
            malo_id: q.malo_id.clone(),
            period_from: q.period_from,
            period_to: q.period_to,
            messtyp,
            sparte,
            arbeitsmenge_kwh: total_kwh,
            arbeitsmenge_ht_kwh: None,
            arbeitsmenge_nt_kwh: None,
            spitzenleistung_kw,
            brennwert_kwh_per_m3: None,
            zustandszahl: None,
            zaehlerstand_anfang: None,
            zaehlerstand_ende: None,
            quality: worst_quality,
            lastprofil: None,
            profil_typ: None,
        };

        // Cache the computed result so repeat calls don't re-aggregate.
        let _ = sqlx::query(
            r"INSERT INTO meter_billing_periods
                  (malo_id, period_from, period_to, messtyp, sparte,
                   arbeitsmenge_kwh, spitzenleistung_kw, quality, tenant)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
              ON CONFLICT ON CONSTRAINT mbp_tenant_period_unique
              DO UPDATE
                  SET arbeitsmenge_kwh   = EXCLUDED.arbeitsmenge_kwh,
                      spitzenleistung_kw = EXCLUDED.spitzenleistung_kw,
                      quality            = EXCLUDED.quality,
                      computed_at        = now()",
        )
        .bind(&result.malo_id)
        .bind(result.period_from)
        .bind(result.period_to)
        .bind(result.messtyp.to_string())
        .bind(sparte_to_str(result.sparte))
        .bind(result.arbeitsmenge_kwh)
        .bind(result.spitzenleistung_kw)
        .bind(quality_to_str(result.quality))
        .bind(&q.tenant)
        .execute(&self.pool)
        .await;

        Ok(Some(result))
    }

    async fn update_gas_quality(
        &self,
        tenant: &str,
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
                AND tenant  = $4
                AND (brennwert_kwh_per_m3 IS NULL OR zustandszahl IS NULL)",
        )
        .bind(malo_id)
        .bind(brennwert_kwh_per_m3)
        .bind(zustandszahl)
        .bind(tenant)
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
                      (malo_id, dtm_from, dtm_to, obis_code_norm,
                       original_kwh, original_quality, corrected_kwh, corrected_quality,
                       reason, source, corrected_by, process_id, pid, tenant)
                  VALUES ($1,$2,$3,$14,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
                  RETURNING correction_id",
            )
            .bind(&rec.malo_id)
            .bind(rec.dtm_from)
            .bind(rec.dtm_to)
            .bind(rec.original_kwh)
            .bind(quality_to_str(rec.original_quality))
            .bind(rec.corrected_kwh)
            .bind(quality_to_str(rec.corrected_quality))
            .bind(&rec.reason)
            .bind(source_str)
            .bind(&rec.corrected_by)
            .bind(rec.process_id)
            .bind(rec.pid.map(|p| p as i32))
            .bind(&rec.tenant)
            .bind(normalise_obis(rec.obis_code.as_deref()))
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| mako_edm::error::EdmError::Database(e.to_string()))?;

            let correction_id: uuid::Uuid = row
                .try_get("correction_id")
                .map_err(|e| mako_edm::error::EdmError::Database(e.to_string()))?;
            correction_ids.push(correction_id);

            // 2. Overwrite the meter_reads row with the corrected value
            //    and increment the correction counter.
            // Keyed on the full primary key, so a correction changes only the
            // register it names.
            //
            // `allocation_version` advances to CORRECTION so mabis-syncd can
            // find what changed since the last submission, and `archived` is
            // reset so the corrected value is re-exported to the cold tier.
            let obis_norm = normalise_obis(rec.obis_code.as_deref());
            sqlx::query(
                r"UPDATE meter_reads
                  SET quantity_kwh       = $4,
                      quality            = $5,
                      correction_count   = correction_count + 1,
                      allocation_version = 'CORRECTION',
                      valid_from_tx      = now(),
                      archived           = false
                  WHERE malo_id        = $1
                    AND dtm_from       = $2
                    AND obis_code_norm = $3
                    AND tenant         = $6",
            )
            .bind(&rec.malo_id)
            .bind(rec.dtm_from)
            .bind(&obis_norm)
            .bind(rec.corrected_kwh) // 0010: NUMERIC(18,5)
            .bind(quality_to_str(rec.corrected_quality))
            .bind(&rec.tenant)
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
    let quantity_kwh: Decimal = row.try_get("quantity_kwh").unwrap_or(Decimal::ZERO);
    Ok(MeterRead {
        malo_id: row
            .try_get("malo_id")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        melo_id: row.try_get("melo_id").unwrap_or(None),
        dtm_from: row
            .try_get("dtm_from")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        dtm_to: row
            .try_get("dtm_to")
            .map_err(|e| EdmError::Database(e.to_string()))?,
        quantity_kwh,
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
        tenant: row
            .try_get("tenant")
            .unwrap_or_else(|_| "default".to_owned()),
        source: mako_edm::domain::IngestionSource::from_db_str(
            row.try_get::<Option<&str>, _>("source")
                .unwrap_or(None)
                .unwrap_or("MSCONS"),
        ),
        push_session: row.try_get("push_session").unwrap_or(None),
        quality_warnings: row.try_get("quality_warnings").unwrap_or(None),
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
        tenant: row
            .try_get("tenant")
            .map_err(|e| EdmError::Database(e.to_string()))?,
    })
}

/// Canonical form of an OBIS code as it enters the primary key.
///
/// `1-0:1.8.0` and `1-0:1.8.0*255` name the same register, and an absent code
/// is the empty-string sentinel used by single-register meters. Every writer
/// goes through this: two spellings of one register would otherwise become two
/// rows, and every OBIS-blind aggregate would count the interval twice.
fn normalise_obis(obis_code: Option<&str>) -> String {
    obis_code.map_or_else(String::new, |s| {
        s.parse::<metering::obis::ObisCode>()
            .map_or_else(|_| s.to_owned(), |c| c.to_string())
    })
}

pub(crate) fn quality_to_str(q: QualityFlag) -> &'static str {
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
    s.as_str()
}

/// Parse a Sparte from its DB label.
///
/// Unknown values fall back to `Strom`. That fallback is lossy by design — the
/// column is CHECK-constrained, so an unknown value means the schema and this
/// code have diverged, which the `schema_code_guard` tests exist to catch.
fn str_to_sparte(s: &str) -> Sparte {
    match s {
        "GAS" => Sparte::Gas,
        "WAERME" => Sparte::Waerme,
        "WASSER" => Sparte::Wasser,
        _ => Sparte::Strom,
    }
}
