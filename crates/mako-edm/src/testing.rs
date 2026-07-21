//! In-memory [`TimeSeriesRepository`] for unit tests and integration fixtures.

use std::sync::Mutex;

use rust_decimal::Decimal;
use time::{Date, OffsetDateTime};

use crate::{
    domain::{
        BillingPeriodQuery, ImbalanceReport, Messtyp, MeterBillingPeriod, MeterDataReceipt,
        MeterRead, QualityFlag, Sparte, TimeSeriesQuery,
    },
    error::EdmError,
    repository::TimeSeriesRepository,
};

/// Thread-safe in-memory time-series store.
///
/// Suitable for unit tests and service integration tests that do not require
/// a real PostgreSQL backend.
#[derive(Debug, Default)]
pub struct InMemoryTimeSeriesRepository {
    receipts: Mutex<Vec<MeterDataReceipt>>,
    reads: Mutex<Vec<MeterRead>>,
}

impl InMemoryTimeSeriesRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl TimeSeriesRepository for InMemoryTimeSeriesRepository {
    async fn store_receipt(&self, receipt: &MeterDataReceipt) -> Result<(), EdmError> {
        let mut guard = self.receipts.lock().unwrap();
        // Idempotent: skip duplicates on process_id.
        if !guard.iter().any(|r| r.process_id == receipt.process_id) {
            guard.push(receipt.clone());
        }
        Ok(())
    }

    async fn store_reads(&self, reads: &[MeterRead]) -> Result<(), EdmError> {
        let mut guard = self.reads.lock().unwrap();
        guard.extend_from_slice(reads);
        Ok(())
    }

    async fn query(&self, q: &TimeSeriesQuery) -> Result<Vec<MeterRead>, EdmError> {
        let guard = self.reads.lock().unwrap();
        Ok(guard
            .iter()
            .filter(|r| {
                r.malo_id == q.malo_id
                    && r.tenant == q.tenant
                    && r.dtm_from >= q.from
                    && r.dtm_to <= q.to
            })
            .cloned()
            .collect())
    }

    async fn receipts(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
        tenant: &str,
    ) -> Result<Vec<MeterDataReceipt>, EdmError> {
        let guard = self.receipts.lock().unwrap();
        Ok(guard
            .iter()
            .filter(|r| {
                r.malo_id == malo_id
                    && r.received_at >= from
                    && r.received_at <= to
                    && r.tenant == tenant
            })
            .cloned()
            .collect())
    }

    async fn imbalance(
        &self,
        malo_id: &str,
        from: Date,
        to: Date,
        tenant: &str,
    ) -> Result<ImbalanceReport, EdmError> {
        let guard = self.reads.lock().unwrap();
        let relevant: Vec<_> = guard
            .iter()
            .filter(|r| r.malo_id == malo_id && r.tenant == tenant)
            .collect();
        if relevant.is_empty() {
            return Err(EdmError::NoData {
                malo_id: malo_id.to_owned(),
                from: from.to_string(),
                to: to.to_string(),
            });
        }
        let total: Decimal = relevant.iter().map(|r| r.quantity_kwh).sum();
        Ok(ImbalanceReport {
            malo_id: malo_id.to_owned(),
            period_from: from,
            period_to: to,
            lf_quantity_kwh: total,
            nb_quantity_kwh: total,
            delta_kwh: Decimal::ZERO,
            delta_pct: Decimal::ZERO,
            quality: QualityFlag::Measured,
        })
    }

    async fn latest_read(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Option<MeterRead>, EdmError> {
        let guard = self.reads.lock().unwrap();
        Ok(guard
            .iter()
            .filter(|r| r.malo_id == malo_id && r.tenant == tenant)
            .max_by_key(|r| r.dtm_from)
            .cloned())
    }

    async fn billing_period(
        &self,
        q: &BillingPeriodQuery,
    ) -> Result<Option<MeterBillingPeriod>, EdmError> {
        use time::macros::time;
        let from_ts = q.period_from.midnight().assume_utc();
        let to_ts = OffsetDateTime::new_utc(q.period_to, time!(23:59:59));

        let guard = self.reads.lock().unwrap();
        let relevant: Vec<&MeterRead> = guard
            .iter()
            .filter(|r| {
                r.malo_id == q.malo_id
                    && r.tenant == q.tenant
                    && r.dtm_from >= from_ts
                    && r.dtm_to <= to_ts
            })
            .collect();

        if relevant.is_empty() {
            return Ok(None);
        }

        let total_kwh: Decimal = relevant.iter().map(|r| r.quantity_kwh).sum();
        let sparte = relevant.first().map_or(Sparte::Strom, |r| r.sparte);

        // Estimate Messtyp from read interval: ≤ 15 min → RLM; otherwise SLP.
        let first_interval = relevant
            .first()
            .map(|r| (r.dtm_to - r.dtm_from).whole_minutes().unsigned_abs());
        let messtyp = match first_interval {
            Some(m) if m <= 15 => Messtyp::Rlm,
            _ => Messtyp::Slp,
        };

        // Spitzenleistung: max kW = max kWh per 15-min interval × 4
        let spitzenleistung_kw = if messtyp == Messtyp::Rlm && sparte == Sparte::Strom {
            relevant
                .iter()
                .map(|r| r.quantity_kwh * Decimal::from(4))
                .max()
        } else {
            None
        };

        let worst_quality = relevant
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
            brennwert_kwh_per_m3: None,
            zustandszahl: None,
            zaehlerstand_anfang: None,
            zaehlerstand_ende: None,
            quality: worst_quality,
            lastprofil: None,
            profil_typ: None,
        }))
    }

    async fn update_gas_quality(
        &self,
        _tenant: &str,
        _malo_id: &str,
        _brennwert_kwh_per_m3: Option<&str>,
        _zustandszahl: Option<&str>,
    ) -> Result<u64, EdmError> {
        // In-memory stub — no-op for testing.
        Ok(0)
    }

    async fn store_corrections(
        &self,
        records: &[crate::domain::CorrectionRecord],
    ) -> Result<Vec<uuid::Uuid>, EdmError> {
        use uuid::Uuid;
        // In-memory stub: apply corrections to stored reads, return dummy UUIDs.
        let mut ids = Vec::with_capacity(records.len());
        {
            let mut reads = self.reads.lock().unwrap();
            for rec in records {
                // Update any existing read in-memory.
                for read in reads.iter_mut() {
                    if read.malo_id == rec.malo_id
                        && read.dtm_from == rec.dtm_from
                        && read.dtm_to == rec.dtm_to
                    {
                        read.quantity_kwh = rec.corrected_kwh;
                        read.quality = rec.corrected_quality;
                        break;
                    }
                }
                ids.push(Uuid::new_v4());
            }
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;
    use uuid::Uuid;

    #[tokio::test]
    async fn store_and_query_receipt() {
        let repo = InMemoryTimeSeriesRepository::new();
        let pid = Uuid::new_v4();
        let receipt = MeterDataReceipt {
            process_id: pid,
            pid: 13005,
            malo_id: "DE00001".into(),
            sender_mp_id: "9900000000001".into(),
            message_ref: None,
            received_at: OffsetDateTime::now_utc(),
            tenant: "test-tenant".into(),
        };
        repo.store_receipt(&receipt).await.unwrap();
        // Idempotent second insert.
        repo.store_receipt(&receipt).await.unwrap();
        let receipts = repo
            .receipts(
                "DE00001",
                OffsetDateTime::UNIX_EPOCH,
                OffsetDateTime::now_utc(),
                "test-tenant",
            )
            .await
            .unwrap();
        assert_eq!(
            receipts.len(),
            1,
            "idempotency: second insert must be no-op"
        );
        // Cross-tenant query must return nothing.
        let other_tenant_receipts = repo
            .receipts(
                "DE00001",
                OffsetDateTime::UNIX_EPOCH,
                OffsetDateTime::now_utc(),
                "other-tenant",
            )
            .await
            .unwrap();
        assert!(
            other_tenant_receipts.is_empty(),
            "cross-tenant query must not return receipts from a different tenant"
        );
    }

    #[tokio::test]
    async fn imbalance_no_data_returns_error() {
        let repo = InMemoryTimeSeriesRepository::new();
        let result = repo
            .imbalance(
                "DE00002",
                time::Date::from_calendar_date(2025, time::Month::January, 1).unwrap(),
                time::Date::from_calendar_date(2025, time::Month::January, 31).unwrap(),
                "test-tenant",
            )
            .await;
        assert!(matches!(result, Err(EdmError::NoData { .. })));
    }
}
