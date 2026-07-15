//! Core aggregation + submission engine for `mabis-syncd`.
//!
//! Orchestrates the full MaBiS UTILTS pipeline:
//! 1. Discover MaLos in Bilanzierungsgebiet (via edmd summenzeitreihe API)
//! 2. Aggregate using `mako-mabis::SummenzeitreiheBuilder`
//! 3. Build UTILTS command payload for makod
//! 4. Submit via makod command API
//! 5. Persist run status to PostgreSQL

use anyhow::{Context, Result};
use mako_edm::BilanzierungsgebietId;
use mako_mabis::{Summenzeitreihe, SummenzeitreiheBuilder};
use rust_decimal::Decimal;
use time::{Date, Duration, OffsetDateTime};
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::pg;

// ── SyncEngine ────────────────────────────────────────────────────────────────

/// Core aggregation and submission engine.
pub struct SyncEngine {
    pool: sqlx::PgPool,
    edmd_client: reqwest::Client,
    makod_client: reqwest::Client,
    cfg: std::sync::Arc<Config>,
}

impl SyncEngine {
    /// Create a new engine from configuration.
    #[must_use]
    pub fn new(pool: sqlx::PgPool, cfg: std::sync::Arc<Config>) -> Self {
        let edmd_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build edmd HTTP client");
        let makod_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build makod HTTP client");
        Self {
            pool,
            edmd_client,
            makod_client,
            cfg,
        }
    }

    /// Run the MaBiS aggregation pipeline for a given period and version.
    ///
    /// Returns the run ID of the created submission record.
    pub async fn run_aggregation(
        &self,
        period_from: Date,
        period_to: Date,
        version: &str,
    ) -> Result<Uuid> {
        let cfg = &self.cfg;
        let bilanzierungsgebiet_id = &cfg.identity.bilanzierungsgebiet_id;

        // Create submission record
        let run_id = pg::insert_run(
            &self.pool,
            pg::InsertRunParams {
                bilanzierungsgebiet_id,
                period_from,
                period_to,
                version,
                sender_mp_id: &cfg.identity.sender_mp_id,
                receiver_mp_id: &cfg.identity.receiver_mp_id,
                tenant: &cfg.identity.tenant,
            },
        )
        .await
        .context("failed to create submission_run record")?;

        info!(
            run_id = %run_id,
            bilanzierungsgebiet_id,
            period_from = %period_from,
            period_to = %period_to,
            version,
            "mabis-syncd: starting UTILTS aggregation run"
        );

        // Discover MaLos and aggregate
        match self
            .aggregate(run_id, period_from, period_to, version)
            .await
        {
            Ok(summenzeitreihe) => {
                let total_kwh = summenzeitreihe.total_kwh();
                let malo_count = self.malo_count_for_run(run_id).await;
                let interval_count = summenzeitreihe.interval_count() as i32;
                let has_substituted = summenzeitreihe.has_substituted_values();

                // Update run with aggregation result
                pg::update_run_aggregated(
                    &self.pool,
                    run_id,
                    malo_count,
                    interval_count,
                    &total_kwh,
                    has_substituted,
                )
                .await
                .context("failed to update submission_run")?;

                // Submit to BIKO via makod
                match self.submit_to_makod(&summenzeitreihe, run_id).await {
                    Ok((message_ref, process_id)) => {
                        pg::mark_acked(&self.pool, run_id, &message_ref, process_id)
                            .await
                            .context("failed to mark run as acked")?;
                        info!(
                            run_id = %run_id,
                            message_ref,
                            total_kwh = %total_kwh,
                            malo_count,
                            "mabis-syncd: UTILTS submission succeeded"
                        );
                    }
                    Err(e) => {
                        warn!(run_id = %run_id, error = %e, "mabis-syncd: UTILTS submission failed");
                        pg::mark_failed(&self.pool, run_id, &e.to_string())
                            .await
                            .ok();
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                warn!(run_id = %run_id, error = %e, "mabis-syncd: aggregation failed");
                pg::mark_failed(&self.pool, run_id, &e.to_string())
                    .await
                    .ok();
                return Err(e);
            }
        }

        Ok(run_id)
    }

    /// Aggregate all MaLo Lastgänge via edmd API.
    async fn aggregate(
        &self,
        run_id: Uuid,
        period_from: Date,
        period_to: Date,
        version: &str,
    ) -> Result<Summenzeitreihe> {
        let cfg = &self.cfg;
        let from_ts = OffsetDateTime::new_utc(period_from, time::Time::MIDNIGHT);
        let to_ts = OffsetDateTime::new_utc(period_to, time::Time::MIDNIGHT);

        // Discover MaLo list from edmd — query all billing periods in this period
        let malo_ids = self.discover_malos(from_ts, to_ts).await?;
        info!(
            malo_count = malo_ids.len(),
            "mabis-syncd: discovered MaLos for aggregation"
        );

        let mut builder = SummenzeitreiheBuilder::new(
            BilanzierungsgebietId(cfg.identity.bilanzierungsgebiet_id.clone()),
            from_ts,
            to_ts,
            version.to_owned(),
            cfg.identity.sender_mp_id.clone(),
            cfg.identity.receiver_mp_id.clone(),
        );

        for malo_id in &malo_ids {
            match self.fetch_lastgang(malo_id, from_ts, to_ts).await {
                Ok(intervals) => {
                    let interval_count = intervals.len() as i32;
                    let total_kwh: Decimal = intervals.iter().map(|iv| iv.value_kwh).sum();
                    let has_gaps = intervals.is_empty();
                    let substituted_count = intervals
                        .iter()
                        .filter(|iv| {
                            matches!(
                                iv.quality,
                                metering::QualityFlag::Substituted
                                    | metering::QualityFlag::Estimated
                                    | metering::QualityFlag::Preliminary
                            )
                        })
                        .count() as i32;

                    builder.add_malo(&intervals);

                    // Log per-MaLo contribution
                    pg::insert_malo_log(
                        &self.pool,
                        run_id,
                        malo_id,
                        interval_count,
                        &total_kwh,
                        has_gaps,
                        substituted_count,
                    )
                    .await
                    .ok(); // Non-fatal: log failure should not abort aggregation
                }
                Err(e) => {
                    warn!(malo_id, error = %e, "mabis-syncd: failed to fetch Lastgang for MaLo — skipping");
                }
            }
        }

        Ok(builder.build())
    }

    /// Discover MaLo IDs from edmd billing periods for the given time window.
    async fn discover_malos(
        &self,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<Vec<String>> {
        let cfg = &self.cfg;
        let from_date = from.date();
        let to_date = to.date();
        let url = format!(
            "{}/api/v1/billing-periods?from={from_date}&to={to_date}&tenant={}",
            cfg.edmd.url, cfg.identity.tenant,
        );

        let resp = self
            .edmd_client
            .get(&url)
            .bearer_auth(&cfg.edmd.api_key)
            .send()
            .await
            .context("edmd MaLo discovery request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("edmd /api/v1/billing-periods returned {status}: {body}");
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .context("failed to parse edmd billing-periods response")?;
        let malo_ids: Vec<String> = data["malo_ids"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();

        Ok(malo_ids)
    }

    /// Fetch per-MaLo Lastgang intervals from edmd Summenzeitreihe endpoint.
    async fn fetch_lastgang(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<Vec<metering::MeterInterval>> {
        let cfg = &self.cfg;
        use time::format_description::well_known::Rfc3339;
        let from_str = from.format(&Rfc3339).unwrap_or_default();
        let to_str = to.format(&Rfc3339).unwrap_or_default();

        let url = format!(
            "{}/api/v1/summenzeitreihe/{malo_id}?from={from_str}&to={to_str}",
            cfg.edmd.url
        );

        let resp = self
            .edmd_client
            .get(&url)
            .bearer_auth(&cfg.edmd.api_key)
            .send()
            .await
            .with_context(|| format!("edmd lastgang request failed for {malo_id}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            anyhow::bail!("edmd /summenzeitreihe/{malo_id} returned {status}");
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .with_context(|| format!("failed to parse lastgang response for {malo_id}"))?;

        // Parse the interval buckets from resampled monthly endpoint into MeterIntervals
        let intervals: Vec<metering::MeterInterval> = data["months"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|bucket| {
                use time::format_description::well_known::Rfc3339;
                let from = bucket["from"]
                    .as_str()
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())?;
                let to = bucket["to"]
                    .as_str()
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())?;
                let kwh_str = bucket["total_kwh"].as_str().unwrap_or("0");
                let value_kwh = kwh_str.parse::<Decimal>().ok()?;
                let has_missing = bucket["has_missing_data"].as_bool().unwrap_or(false);
                let quality = if has_missing {
                    metering::QualityFlag::Estimated
                } else {
                    metering::QualityFlag::Measured
                };
                Some(metering::MeterInterval {
                    from,
                    to,
                    value_kwh,
                    quality,
                    obis_code: None,
                })
            })
            .collect();

        Ok(intervals)
    }

    /// Submit the aggregated Summenzeitreihe to BIKO via makod.
    ///
    /// Returns `(message_ref, process_id)` on success.
    async fn submit_to_makod(
        &self,
        summenzeitreihe: &Summenzeitreihe,
        run_id: Uuid,
    ) -> Result<(String, Option<Uuid>)> {
        let cfg = &self.cfg;

        // Build makod command payload
        let command = serde_json::json!({
            "workflow": "mabis-billing",
            "command": "SubmitUtilts",
            "correlation_id": run_id.to_string(),
            "payload": {
                "bilanzierungsgebiet_id": summenzeitreihe.bilanzierungsgebiet_id.0,
                "period_from": summenzeitreihe.period_from,
                "period_to": summenzeitreihe.period_to,
                "version": summenzeitreihe.version,
                "sender_mp_id": summenzeitreihe.sender_mp_id,
                "receiver_mp_id": summenzeitreihe.receiver_mp_id,
                "total_kwh": summenzeitreihe.total_kwh().to_string(),
                "interval_count": summenzeitreihe.interval_count(),
                "has_substituted_values": summenzeitreihe.has_substituted_values(),
                // Monthly aggregates for UTILTS encoding
                "monthly_totals": summenzeitreihe.monthly_totals().iter().map(|b| serde_json::json!({
                    "from": b.from,
                    "to": b.to,
                    "total_kwh": b.total_kwh.to_string(),
                    "coverage_pct": b.coverage_pct(),
                })).collect::<Vec<_>>(),
            }
        });

        let url = format!("{}/api/v1/commands", cfg.makod.url);
        let resp = self
            .makod_client
            .post(&url)
            .bearer_auth(&cfg.makod.api_key)
            .json(&command)
            .send()
            .await
            .context("makod command submission request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("makod /api/v1/commands returned {status}: {body}");
        }

        let result: serde_json::Value = resp
            .json()
            .await
            .context("failed to parse makod response")?;
        let message_ref = result["message_ref"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned();
        let process_id = result["process_id"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok());

        Ok((message_ref, process_id))
    }

    /// Helper: count MaLos logged for a run.
    async fn malo_count_for_run(&self, run_id: Uuid) -> i32 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM submission_malo_log WHERE run_id = $1")
            .bind(run_id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0) as i32
    }
}

// ── Schedule helpers ──────────────────────────────────────────────────────────

/// Determine the billing period for the previous calendar month.
///
/// Called on the `preliminary_day` or `final_day` of the current month.
/// Returns `(period_from, period_to)` where `period_to` is the last day of
/// the previous month and `period_from` is the first day.
#[must_use]
pub fn previous_month_period(today: Date) -> (Date, Date) {
    let (year, month) = if today.month() == time::Month::January {
        (today.year() - 1, time::Month::December)
    } else {
        (today.year(), today.month().previous())
    };
    let first = Date::from_calendar_date(year, month, 1).expect("valid calendar date");
    let last = {
        let next_month_first = if month == time::Month::December {
            Date::from_calendar_date(year + 1, time::Month::January, 1)
        } else {
            Date::from_calendar_date(year, month.next(), 1)
        }
        .expect("valid calendar date");
        next_month_first - Duration::days(1)
    };
    (first, last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn previous_month_january() {
        let today = date!(2026 - 01 - 03);
        let (from, to) = previous_month_period(today);
        assert_eq!(from, date!(2025 - 12 - 01));
        assert_eq!(to, date!(2025 - 12 - 31));
    }

    #[test]
    fn previous_month_june() {
        let today = date!(2026 - 06 - 08);
        let (from, to) = previous_month_period(today);
        assert_eq!(from, date!(2026 - 05 - 01));
        assert_eq!(to, date!(2026 - 05 - 31));
    }

    #[test]
    fn previous_month_march_end() {
        let today = date!(2026 - 04 - 03);
        let (from, to) = previous_month_period(today);
        assert_eq!(from, date!(2026 - 03 - 01));
        assert_eq!(to, date!(2026 - 03 - 31));
    }

    #[test]
    fn previous_month_feb_non_leap() {
        let today = date!(2026 - 03 - 03);
        let (from, to) = previous_month_period(today);
        assert_eq!(from, date!(2026 - 02 - 01));
        assert_eq!(to, date!(2026 - 02 - 28));
    }

    #[test]
    fn previous_month_feb_leap() {
        let today = date!(2024 - 03 - 08);
        let (from, to) = previous_month_period(today);
        assert_eq!(from, date!(2024 - 02 - 01));
        assert_eq!(to, date!(2024 - 02 - 29));
    }
}
