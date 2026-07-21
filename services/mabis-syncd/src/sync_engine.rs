//! Core aggregation + submission engine for `mabis-syncd`.
//!
//! Orchestrates the full MaBiS Summenzeitreihe pipeline:
//! 1. Discover MaLos in Bilanzierungsgebiet (edmd `/api/v1/billing-periods`, then `/api/v1/lastgang/{malo_id}`)
//! 2. Aggregate using `mako-mabis::SummenzeitreiheBuilder`
//! 3. Build the MSCONS 13003 command payload for makod
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
use crate::pg::{Abrechnungslauf, SubmissionPhase};

/// Prüfidentifikator for "Übertragung Summenzeitreihe" (MSCONS AHB 3.2 §8.3.1).
pub const MSCONS_SUMMENZEITREIHE_PID: u32 = 13003;

/// Bilanzierungsmonat as EDIFACT format 610 (`CCYYMM`).
fn fmt_edifact_month(d: OffsetDateTime) -> String {
    format!("{:04}{:02}", d.year(), u8::from(d.month()))
}

/// Versionsangabe as EDIFACT format 304 (`CCYYMMDDHHMMSSZZZ`).
///
/// The offset is written explicitly because the version orders submissions; a
/// value whose zone is implied cannot be compared across a DST boundary.
fn fmt_edifact_version(t: OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}{:02}{:02}{:02}+00",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// Interval bound as EDIFACT format 303 (`CCYYMMDDHHMMZZZ`).
fn fmt_edifact_instant(t: OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}{:02}{:02}+00",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute()
    )
}

/// Deadlines for a Bilanzierungsgebiets-Summenzeitreihe (BG-SZR, Kategorie B),
/// counted in Werktagen after the end of the Bilanzierungsmonat.
///
/// BK6-24-174 Anlage 3 §3.10, Tabelle 2.
mod fristen {
    /// Last Werktag of the Erstaufschlag window for a BG-SZR.
    ///
    /// Within it a new version is assigned `Abrechnungsdaten` directly; after
    /// it a new version starts as `Prüfdaten` and needs a positive
    /// Prüfmitteilung to be promoted.
    pub const ERSTAUFSCHLAG_LAST_WT: u32 = 10;

    /// Last Werktag of the Clearingphase for the ordinary BKA.
    ///
    /// A submission arriving after this belongs to the KBKA, whose own window
    /// runs to the end of the seventh month.
    pub const CLEARING_LAST_WT_BKA: u32 = 30;
}

/// Which submission window `today` falls in, for a period ending `period_to`.
///
/// The Datenstatus the BIKO will assign follows from this, so the phase is
/// derived from the calendar rather than passed in by the caller.
#[must_use]
pub fn phase_for(period_to: Date, today: Date) -> (Abrechnungslauf, SubmissionPhase) {
    use mako_engine::fristen::{HolidayCalendar, add_werktage};

    let cal = HolidayCalendar::BdewMaKo;
    let erstaufschlag_ends = add_werktage(period_to, fristen::ERSTAUFSCHLAG_LAST_WT, cal);
    let clearing_ends = add_werktage(period_to, fristen::CLEARING_LAST_WT_BKA, cal);

    if today <= erstaufschlag_ends {
        (Abrechnungslauf::Bka, SubmissionPhase::Erstaufschlag)
    } else if today <= clearing_ends {
        (Abrechnungslauf::Bka, SubmissionPhase::Clearing)
    } else {
        // Past the BKA Clearingfrist the submission enters the KBKA, where it
        // starts as Prüfdaten regardless of how early in that window it lands.
        (Abrechnungslauf::Kbka, SubmissionPhase::Clearing)
    }
}

// ── SyncEngine ────────────────────────────────────────────────────────────────

/// Core aggregation and submission engine.
pub struct SyncEngine {
    pool: sqlx::PgPool,
    edmd_client: reqwest::Client,
    marktd_client: reqwest::Client,
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
        let marktd_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build marktd HTTP client");
        let makod_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build makod HTTP client");
        Self {
            pool,
            edmd_client,
            marktd_client,
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
        corrects_run_id: Option<Uuid>,
        as_of: Option<OffsetDateTime>,
    ) -> Result<Uuid> {
        let cfg = &self.cfg;
        let bilanzierungsgebiet_id = &cfg.identity.bilanzierungsgebiet_id;

        // The window decides the Datenstatus the BIKO will assign, so it is
        // derived from the settlement calendar rather than chosen by the caller.
        let (abrechnungslauf, phase) = phase_for(period_to, OffsetDateTime::now_utc().date());

        // Create submission record. `version` defaults to `now()` — ascending
        // per §3.8.2, and a resubmission for the same period is a new version
        // rather than a replacement of the previous row.
        let run_id = pg::insert_run(
            &self.pool,
            pg::InsertRunParams {
                bilanzierungsgebiet_id,
                period_from,
                period_to,
                abrechnungslauf,
                phase,
                corrects_run_id,
                sender_mp_id: &cfg.identity.sender_mp_id,
                receiver_mp_id: &cfg.identity.receiver_mp_id,
                tenant: &cfg.identity.tenant,
            },
        )
        .await
        .context("failed to create submission_run record")?;

        // The version the row was assigned, which travels into the message.
        let version: OffsetDateTime =
            sqlx::query_scalar("SELECT version FROM submission_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&self.pool)
                .await
                .context("failed to read back the assigned version")?;

        info!(
            run_id = %run_id,
            bilanzierungsgebiet_id,
            period_from = %period_from,
            period_to = %period_to,
            abrechnungslauf = abrechnungslauf.as_str(),
            phase = phase.as_str(),
            "mabis-syncd: starting Summenzeitreihe aggregation run"
        );

        // Discover MaLos and aggregate
        match self
            .aggregate(run_id, period_from, period_to, version, as_of)
            .await
        {
            Ok(series) if series.is_empty() => {
                // No MaLos discovered. Failing loudly beats acking an empty
                // submission as though it were a successful one.
                let msg = "no MaLos discovered for the period — nothing to submit";
                warn!(run_id = %run_id, "mabis-syncd: {msg}");
                pg::mark_failed(&self.pool, run_id, msg).await.ok();
                anyhow::bail!("{msg}");
            }
            Ok(series) => {
                // One submission per Bilanzierungsgebiet.
                let total_kwh: Decimal = series.iter().map(Summenzeitreihe::total_kwh).sum();
                let malo_count = self.malo_count_for_run(run_id).await;
                let interval_count: i32 = series.iter().map(|s| s.interval_count() as i32).sum();
                let has_substituted = series.iter().any(Summenzeitreihe::has_substituted_values);

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

                // Submit to BIKO via makod — one MSCONS 13003 per Bilanzierungsgebiet.
                match self.submit_all_to_makod(&series, run_id).await {
                    Ok((message_ref, process_id)) => {
                        pg::mark_acked(&self.pool, run_id, &message_ref, process_id)
                            .await
                            .context("failed to mark run as acked")?;
                        info!(
                            run_id = %run_id,
                            message_ref,
                            total_kwh = %total_kwh,
                            malo_count,
                            "mabis-syncd: Summenzeitreihe submission succeeded"
                        );
                    }
                    Err(e) => {
                        warn!(run_id = %run_id, error = %e, "mabis-syncd: Summenzeitreihe submission failed");
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

    /// Aggregate all MaLo Lastgänge via edmd API, **one Summenzeitreihe per
    /// Bilanzierungsgebiet**.
    ///
    /// MaBiS settles per territory. Emitting a single series for the whole
    /// tenant put every MaLo into whichever zone the config happened to name,
    /// which misfiles the submission for any tenant spanning more than one.
    async fn aggregate(
        &self,
        run_id: Uuid,
        period_from: Date,
        period_to: Date,
        version: OffsetDateTime,
        as_of: Option<OffsetDateTime>,
    ) -> Result<Vec<Summenzeitreihe>> {
        let cfg = &self.cfg;
        let from_ts = OffsetDateTime::new_utc(period_from, time::Time::MIDNIGHT);
        let to_ts = OffsetDateTime::new_utc(period_to, time::Time::MIDNIGHT);

        // Discover MaLo list from edmd — query all billing periods in this period
        let malo_ids = self.discover_malos(from_ts, to_ts).await?;
        info!(
            malo_count = malo_ids.len(),
            "mabis-syncd: discovered MaLos for aggregation"
        );

        let by_gebiet = self.resolve_bilanzierungsgebiete(&malo_ids).await;
        info!(
            gebiet_count = by_gebiet.len(),
            "mabis-syncd: MaLos grouped by Bilanzierungsgebiet"
        );

        // MaLos excluded from the aggregate, by reason. A Summenzeitreihe missing
        // a MaLo's energy is indistinguishable from a correct one once the BIKO
        // has acked it, so the run fails rather than filing a short submission.
        let mut excluded: Vec<String> = Vec::new();

        let mut series: Vec<Summenzeitreihe> = Vec::with_capacity(by_gebiet.len());
        for (gebiet, gebiet_malos) in &by_gebiet {
            let mut builder = SummenzeitreiheBuilder::new(
                BilanzierungsgebietId(gebiet.clone()),
                from_ts,
                to_ts,
                version,
                cfg.identity.sender_mp_id.clone(),
                cfg.identity.receiver_mp_id.clone(),
                mako_mabis::MABIS_SLOT,
            );

            for malo_id in gebiet_malos {
                match self.fetch_lastgang(malo_id, from_ts, to_ts, as_of).await {
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

                        if let Err(e) = builder.add_malo(&intervals) {
                            warn!(
                                malo_id,
                                error = %e,
                                "mabis-syncd: Lastgang resolution does not match the settlement grid"
                            );
                            excluded.push(format!("{malo_id}: {e}"));
                            continue;
                        }

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
                        warn!(malo_id, error = %e, "mabis-syncd: failed to fetch Lastgang for MaLo");
                        excluded.push(format!("{malo_id}: fetch failed: {e}"));
                    }
                }
            }

            let szr = builder.build();
            if !szr.is_complete() {
                warn!(
                    gebiet,
                    missing_slots = szr.missing_slot_count(),
                    expected_slots = szr.expected_slot_count(),
                    "mabis-syncd: Summenzeitreihe has empty settlement slots — energy is omitted, not zeroed"
                );
            }
            series.push(szr);
        }

        // A MaBiS filing cannot be withdrawn once acked, and the BIKO cannot
        // tell a short Summenzeitreihe from a complete one. Discovering MaLos
        // and then omitting some of them is therefore a failed run, not a
        // partial success.
        if !excluded.is_empty() {
            anyhow::bail!(
                "{} of {} discovered MaLos could not be aggregated, so the Summenzeitreihe \
                 would under-report energy: {}",
                excluded.len(),
                malo_ids.len(),
                excluded.join("; ")
            );
        }

        Ok(series)
    }

    /// Resolve each MaLo's Bilanzierungsgebiet from `marktd`.
    ///
    /// MaBiS aggregates per Bilanzierungsgebiet, so this determines which
    /// Summenzeitreihe a MaLo belongs to. A MaLo whose master data names no
    /// territory is reported rather than silently folded into the configured
    /// fallback — misfiling energy into the wrong zone is a settlement error the
    /// BIKO cannot detect.
    async fn resolve_bilanzierungsgebiete(
        &self,
        malo_ids: &[String],
    ) -> std::collections::BTreeMap<String, Vec<String>> {
        use std::collections::BTreeMap;
        let cfg = &self.cfg;
        let mut by_gebiet: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for malo_id in malo_ids {
            let url = format!("{}/api/v1/malo/{malo_id}", cfg.marktd.url);
            let gebiet = match self
                .marktd_client
                .get(&url)
                .bearer_auth(&cfg.marktd.api_key)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    resp.json::<serde_json::Value>().await.ok().and_then(|v| {
                        v["bilanzierungsgebiet"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                    })
                }
                Ok(resp) => {
                    warn!(malo_id, status = %resp.status(), "mabis-syncd: marktd MaLo lookup failed");
                    None
                }
                Err(e) => {
                    warn!(malo_id, error = %e, "mabis-syncd: marktd unreachable for MaLo");
                    None
                }
            };

            let key = gebiet.unwrap_or_else(|| {
                warn!(
                    malo_id,
                    fallback = %cfg.identity.bilanzierungsgebiet_id,
                    "mabis-syncd: MaLo has no Bilanzierungsgebiet in marktd — using configured fallback"
                );
                cfg.identity.bilanzierungsgebiet_id.clone()
            });
            by_gebiet.entry(key).or_default().push(malo_id.clone());
        }
        by_gebiet
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
        // edmd returns `{"billing_periods": [{malo_id, messtyp, sparte, ...}], "count": n}`.
        // Reading a `malo_ids` array that the response never contained yielded an
        // empty set on every run — and an empty Summenzeitreihe still submits and
        // acks, so a zero-energy MaBiS submission looked like a successful one.
        let Some(periods) = data["billing_periods"].as_array() else {
            anyhow::bail!(
                "edmd billing-periods response has no `billing_periods` array; got keys: {:?}",
                data.as_object().map(|o| o.keys().collect::<Vec<_>>())
            );
        };
        let mut malo_ids: Vec<String> = periods
            .iter()
            .filter_map(|p| p["malo_id"].as_str().map(str::to_owned))
            .collect();
        malo_ids.sort_unstable();
        malo_ids.dedup();

        Ok(malo_ids)
    }

    /// Reconstruct an instant from a BO4E `Zeitraum` date/time pair.
    ///
    /// `startdatum` is an ISO date and `startuhrzeit` a time carrying its own
    /// UTC offset, which the pair must be recombined through — reading the date
    /// as if it were already UTC would shift every slot by the offset.
    fn parse_zeitraum_bound(
        date: &serde_json::Value,
        uhrzeit: &serde_json::Value,
    ) -> Option<OffsetDateTime> {
        let date = date.as_str()?;
        let uhrzeit = uhrzeit.as_str()?;
        OffsetDateTime::parse(
            &format!("{date}T{uhrzeit}"),
            &time::format_description::well_known::Rfc3339,
        )
        .ok()
    }

    /// Read a decimal that BO4E may serialise as either a JSON string or number.
    fn parse_decimal(v: &serde_json::Value) -> Option<Decimal> {
        match v {
            serde_json::Value::String(s) => s.parse().ok(),
            serde_json::Value::Number(n) => n.to_string().parse().ok(),
            _ => None,
        }
    }

    /// Map a BO4E `Messwertstatus` onto the metering quality flag.
    ///
    /// The forward mapping in edmd is lossy, so this errs toward treating a
    /// value as non-measured: over-reporting substitution costs a flag in the
    /// MaBiS log, under-reporting it lets an estimate settle as a reading.
    fn messwertstatus_to_quality(status: Option<&str>) -> metering::QualityFlag {
        use metering::QualityFlag as Q;
        match status {
            Some("ABGELESEN") => Q::Measured,
            Some("ERSATZWERT") => Q::Substituted,
            Some("PROGNOSEWERT" | "VORSCHLAGSWERT") => Q::Estimated,
            Some("VORLAEUFIGERWERT") => Q::Preliminary,
            Some("NICHT_VERWENDBAR") => Q::Faulty,
            _ => Q::Unknown,
        }
    }

    /// Fetch a MaLo's quarter-hourly Lastgang from edmd.
    ///
    /// Reads the BO4E `Lastgang` projection, which carries one `Zeitreihenwert`
    /// per metered slot. MaBiS settles on that grid, so the resampled endpoints
    /// are not interchangeable here: a coarser bucket preserves the period total
    /// but destroys the shape the BIKO settles against.
    async fn fetch_lastgang(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
        as_of: Option<OffsetDateTime>,
    ) -> Result<Vec<metering::MeterInterval>> {
        let cfg = &self.cfg;
        use time::format_description::well_known::Rfc3339;
        let from_str = from.format(&Rfc3339).unwrap_or_default();
        let to_str = to.format(&Rfc3339).unwrap_or_default();

        // `as_of` reconstructs the data as it stood when an earlier version was
        // filed (§ 60 Abs. 6 MsbG). A correction under the KBKA has to be able to say
        // what changed since the version the BIKO settled, which requires the
        // earlier state, not just the current one.
        let url = match as_of {
            Some(ts) => {
                let ts = ts.format(&Rfc3339).unwrap_or_default();
                format!(
                    "{}/api/v1/lastgang/{malo_id}?from={from_str}&to={to_str}&as_of={ts}",
                    cfg.edmd.url
                )
            }
            None => format!(
                "{}/api/v1/lastgang/{malo_id}?from={from_str}&to={to_str}",
                cfg.edmd.url
            ),
        };

        let resp = self
            .edmd_client
            .get(&url)
            .bearer_auth(&cfg.edmd.api_key)
            .send()
            .await
            .with_context(|| format!("edmd lastgang request failed for {malo_id}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            anyhow::bail!("edmd /lastgang/{malo_id} returned {status}");
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .with_context(|| format!("failed to parse lastgang response for {malo_id}"))?;

        // The endpoint returns one BO4E `Lastgang` per OBIS code, each holding
        // the slot values under `werte`.
        let lastgaenge = data
            .as_array()
            .with_context(|| format!("edmd /lastgang/{malo_id} did not return an array"))?;

        let mut intervals: Vec<metering::MeterInterval> = Vec::new();
        for lastgang in lastgaenge {
            let obis = lastgang["obisKennzahl"].as_str().map(str::to_owned);
            for wert in lastgang["werte"].as_array().into_iter().flatten() {
                let zeitraum = &wert["zeitraum"];
                let from =
                    Self::parse_zeitraum_bound(&zeitraum["startdatum"], &zeitraum["startuhrzeit"]);
                let to = Self::parse_zeitraum_bound(&zeitraum["enddatum"], &zeitraum["enduhrzeit"]);
                let (Some(from), Some(to)) = (from, to) else {
                    continue;
                };
                let Some(value_kwh) = Self::parse_decimal(&wert["wert"]) else {
                    continue;
                };
                intervals.push(metering::MeterInterval {
                    from,
                    to,
                    value_kwh,
                    quality: Self::messwertstatus_to_quality(wert["status"].as_str()),
                    obis_code: obis.clone(),
                });
            }
        }
        intervals.sort_by_key(|iv| iv.from);

        Ok(intervals)
    }

    /// Submit the aggregated Summenzeitreihe to BIKO via makod.
    ///
    /// Returns `(message_ref, process_id)` on success.
    /// Submit every Bilanzierungsgebiet's Summenzeitreihe, one MSCONS 13003 each.
    ///
    /// Returns the first submission's reference for the run record. A failure on
    /// any territory fails the whole run: a partially-submitted MaBiS period is
    /// harder to reconcile than one that plainly did not go out.
    async fn submit_all_to_makod(
        &self,
        series: &[Summenzeitreihe],
        run_id: Uuid,
    ) -> Result<(String, Option<Uuid>)> {
        let mut first: Option<(String, Option<Uuid>)> = None;
        for s in series {
            let res = self.submit_to_makod(s, run_id).await?;
            if first.is_none() {
                first = Some(res);
            }
        }
        first.ok_or_else(|| anyhow::anyhow!("no Summenzeitreihe to submit"))
    }

    async fn submit_to_makod(
        &self,
        summenzeitreihe: &Summenzeitreihe,
        run_id: Uuid,
    ) -> Result<(String, Option<Uuid>)> {
        let cfg = &self.cfg;

        // Build makod command payload
        // A Summenzeitreihe is an MSCONS message, Prüfidentifikator 13003
        // ("Übertragung Summenzeitreihe", MSCONS AHB 3.2 §8.3.1). UTILTS carries
        // Berechnungsformel and Zählzeitdefinitionen and has no Summenzeitreihe
        // use case at all.
        // MSCONS Prüfidentifikator 13003, "Übertragung Summenzeitreihe"
        // (MSCONS AHB 3.2 §8.3.1). UTILTS carries Berechnungsformel and
        // Zählzeitdefinitionen and has no Summenzeitreihe use case.
        //
        // EDIFACT wants its own date formats: the Bilanzierungsmonat is
        // `CCYYMM` (DTM+492, format 610), the Versionsangabe
        // `CCYYMMDDHHMMSSZZZ` (DTM+293, format 304), and each slot bound
        // `CCYYMMDDHHMMZZZ` (format 303).
        let command = serde_json::json!({
            "command": "mabis.summenzeitreihe.uebermitteln",
            "marktrolle": "ÜNB",
            "correlation_id": run_id.to_string(),
            "payload": {
                "bilanzierungsgebiet_id": summenzeitreihe.bilanzierungsgebiet_id.0,
                "balancing_period": fmt_edifact_month(summenzeitreihe.period_from),
                "version": fmt_edifact_version(summenzeitreihe.version),
                "sender_mp_id": summenzeitreihe.sender_mp_id,
                "receiver_mp_id": summenzeitreihe.receiver_mp_id,
                "intervals": summenzeitreihe.intervals.iter().map(|iv| serde_json::json!({
                    "from": fmt_edifact_instant(iv.from),
                    "to": fmt_edifact_instant(iv.to),
                    "quantity_kwh": iv.quantity_kwh.to_string(),
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
/// Called on the `erstaufschlag_werktag` Werktag after the Bilanzierungsmonat.
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

#[cfg(test)]
mod edifact_format_tests {
    use super::*;
    use time::macros::datetime;

    /// The three EDIFACT date formats MSCONS 13003 requires are distinct, and
    /// sending one where another is expected is not detectable downstream.
    #[test]
    fn each_edifact_date_uses_its_own_format() {
        let t = datetime!(2026-06-14 05:07:09 UTC);
        assert_eq!(fmt_edifact_month(t), "202606", "DTM+492 is CCYYMM");
        assert_eq!(
            fmt_edifact_version(t),
            "20260614050709+00",
            "DTM+293 is CCYYMMDDHHMMSSZZZ"
        );
        assert_eq!(
            fmt_edifact_instant(t),
            "202606140507+00",
            "DTM+163/164 is CCYYMMDDHHMMZZZ — no seconds"
        );
    }

    /// A non-UTC input must be converted, not truncated: the version orders
    /// submissions, so a mis-zoned value can invert two corrections.
    #[test]
    fn a_non_utc_instant_is_converted_before_formatting() {
        let berlin_summer = datetime!(2026-06-14 07:07:09 +02:00);
        assert_eq!(fmt_edifact_version(berlin_summer), "20260614050709+00");
        assert_eq!(fmt_edifact_instant(berlin_summer), "202606140507+00");
    }
}
