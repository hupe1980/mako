//! Core aggregation + submission engine for `mabis-syncd`.
//!
//! Orchestrates the full MaBiS Summenzeitreihe pipeline:
//! 1. Discover MaLos in Bilanzierungsgebiet (edmd `/api/v1/billing-periods`, then `/api/v1/lastgang/{malo_id}`)
//! 2. Aggregate using `mako-mabis::SummenzeitreiheBuilder`
//! 3. Build the MSCONS 13003 command payload for makod
//! 4. Submit via makod command API
//! 5. Persist run status to PostgreSQL

use anyhow::{Context, Result};
use mako_engine::types::Pruefidentifikator;
use mako_mabis::BilanzierungsgebietId;
use mako_mabis::{Summenzeitreihe, SummenzeitreiheBuilder};
use rust_decimal::Decimal;
use time::{Date, Duration, OffsetDateTime};
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::pg;
use crate::pg::{Abrechnungslauf, SubmissionPhase};

/// Prüfidentifikator for "Übertragung Summenzeitreihe" (MSCONS AHB 3.2 §8.3.1).
pub const MSCONS_SUMMENZEITREIHE_PID: Pruefidentifikator = Pruefidentifikator::const_new(13003);

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

/// The Berlin civil date of an instant.
///
/// MaBiS Fristen and Bilanzierungsmonate run on the local calendar, so the
/// Werktag arithmetic must not shift by a day around midnight UTC.
#[must_use]
pub fn berlin_date(t: OffsetDateTime) -> Date {
    use time_tz::OffsetDateTimeExt as _;
    t.to_timezone(time_tz::timezones::db::europe::BERLIN).date()
}

/// Berlin local midnight of `d`, as a UTC instant.
///
/// Midnight never falls in a DST gap/overlap (transitions are at 02:00/03:00),
/// so `take_first` is unambiguous.
#[must_use]
pub fn berlin_midnight_utc(d: Date) -> OffsetDateTime {
    use time_tz::PrimitiveDateTimeExt as _;
    d.midnight()
        .assume_timezone(time_tz::timezones::db::europe::BERLIN)
        .take_first()
        .expect("Berlin local midnight is unambiguous")
        .to_offset(time::UtcOffset::UTC)
}

/// The settlement window `[from, to)` for a Bilanzierungsmonat, as UTC instants.
///
/// `period_to` is the **inclusive** last day of the month, so the exclusive end
/// is the following Berlin midnight. Taking `period_to` itself dropped the last
/// day of every month, and UTC midnight shifted the grid by an hour, which made
/// the two DST months come out four slots short or long.
#[must_use]
pub fn aggregation_window(period_from: Date, period_to: Date) -> (OffsetDateTime, OffsetDateTime) {
    let end = period_to.next_day().unwrap_or(period_to);
    (berlin_midnight_utc(period_from), berlin_midnight_utc(end))
}

/// Idempotency key for one Summenzeitreihe submission.
///
/// `makod` requires the header on every command and rejects the request with
/// 422 `missing_idempotency_key` otherwise, so an omitted key failed every run.
/// It never compares the key, but a retry must still present the same one: the
/// key ties the attempts of one submission together in makod's log, and the
/// identity of a submission is exactly (run, Bilanzierungsgebiet, Version).
#[must_use]
pub fn idempotency_key(
    run_id: Uuid,
    bilanzierungsgebiet_id: &str,
    version: OffsetDateTime,
) -> String {
    format!(
        "mabis-szr-{run_id}-{bilanzierungsgebiet_id}-{}",
        fmt_edifact_version(version)
    )
}

/// Truncate a version timestamp to whole seconds.
///
/// The wire value (MSCONS SG6 DTM+293, format 304) carries seconds and no more,
/// so a stored version with sub-second precision can never be matched again when
/// the BIKO echoes it back in a Datenstatus or Prüfmitteilung.
#[must_use]
pub fn truncate_to_seconds(t: OffsetDateTime) -> OffsetDateTime {
    t.replace_nanosecond(0).unwrap_or(t)
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
        // The Bilanzierungsmonat is civil, so the phase runs off the Berlin date
        // — a UTC date is a day behind for the first hour of every local day.
        let (abrechnungslauf, phase) = phase_for(period_to, berlin_date(OffsetDateTime::now_utc()));

        // The version is ascending per §3.8.2 and truncated to whole seconds,
        // because that is all the wire carries. A resubmission for the same
        // period is a new version rather than a replacement of the previous row.
        let version = truncate_to_seconds(OffsetDateTime::now_utc());
        let run_id = pg::insert_run(
            &self.pool,
            pg::InsertRunParams {
                bilanzierungsgebiet_id,
                period_from,
                period_to,
                version,
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
                self.mark_failed_and_emit(
                    run_id,
                    period_from,
                    period_to,
                    abrechnungslauf,
                    phase,
                    msg,
                )
                .await;
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
                        // §9.8.1: the corrected BG-SZR is the answer to the
                        // negative Prüfmitteilung, so the obligation closes here
                        // — otherwise it stays open on /korrekturbedarf forever.
                        if let Some(corrected) = corrects_run_id {
                            match pg::close_korrekturbedarf(&self.pool, corrected, run_id).await {
                                Ok(n) => info!(
                                    run_id = %run_id, corrects_run_id = %corrected, closed = n,
                                    "mabis-syncd: Korrekturbedarf closed by correcting submission"
                                ),
                                Err(e) => warn!(
                                    run_id = %run_id, error = %e,
                                    "mabis-syncd: failed to close Korrekturbedarf"
                                ),
                            }
                        }
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
                        self.mark_failed_and_emit(
                            run_id,
                            period_from,
                            period_to,
                            abrechnungslauf,
                            phase,
                            &e.to_string(),
                        )
                        .await;
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                warn!(run_id = %run_id, error = %e, "mabis-syncd: aggregation failed");
                self.mark_failed_and_emit(
                    run_id,
                    period_from,
                    period_to,
                    abrechnungslauf,
                    phase,
                    &e.to_string(),
                )
                .await;
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
        let (from_ts, to_ts) = aggregation_window(period_from, period_to);

        // Discover MaLo list from edmd — query all billing periods in this period
        let malo_ids = self.discover_malos(period_from, period_to).await?;
        info!(
            malo_count = malo_ids.len(),
            "mabis-syncd: discovered MaLos for aggregation"
        );

        let (by_gebiet, missing_gebiet) = self.resolve_bilanzierungsgebiete(&malo_ids).await;
        info!(
            gebiet_count = by_gebiet.len(),
            "mabis-syncd: MaLos grouped by Bilanzierungsgebiet"
        );

        // MaLos excluded from the aggregate, by reason. A Summenzeitreihe missing
        // a MaLo's energy is indistinguishable from a correct one once the BIKO
        // has acked it, so the run fails rather than filing a short submission.
        let mut excluded: Vec<String> = missing_gebiet
            .into_iter()
            .map(|m| format!("{m}: no Bilanzierungsgebiet in marktd master data"))
            .collect();

        // Territories whose grid still has holes after every MaLo aggregated.
        let mut incomplete: Vec<String> = Vec::new();
        let mut series: Vec<Summenzeitreihe> = Vec::with_capacity(by_gebiet.len());
        for (gebiet, gebiet_malos) in &by_gebiet {
            // A Summenzeitreihe is submitted under its MaBiS-Zählpunkt
            // (SG6 LOC+172); the Bilanzierungsgebiet EIC is a separate
            // LOC+107. Refuse rather than substitute: a Summenzeitreihe filed
            // against the wrong Meldepunkt is indistinguishable, to the BIKO,
            // from a correct one.
            let mabis_zp = self.resolve_mabis_zp(gebiet).await?;

            // The territory is a 16-character EIC of ENTSO-E object type `Y`
            // (Area). A Bilanzkreis (`X`, Party) is the same length, so only the
            // object type separates them, and `LOC+107` carries the value as
            // free text — the BIKO would accept either. Refuse here, inside the
            // submission window and naming the territory, rather than filing a
            // series against an object that is not a Bilanzierungsgebiet.
            let gebiet_id = BilanzierungsgebietId::new(gebiet).map_err(|e| {
                anyhow::anyhow!(
                    "Bilanzierungsgebiet {gebiet} (from marktd master data) is not a \
                     Bilanzierungsgebiet-EIC: {e}"
                )
            })?;

            let mut builder = SummenzeitreiheBuilder::new(
                gebiet_id,
                mabis_zp,
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
                        let total_kwh: Decimal = intervals.iter().map(|iv| iv.value).sum();
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
            // No Meldepunkt/territory swap check is needed here: the parse
            // refuses one. A Zählpunktbezeichnung is 33 characters and a
            // Bilanzierungsgebiet a 16-character Y-type EIC, so neither value can
            // inhabit the other's type.
            // MaBiS settles against a gap-free grid, so an empty slot omits energy
            // rather than reporting zero for it. That under-reports the territory,
            // and the BIKO cannot tell a short series from a complete one — the
            // same reason the excluded-MaLo check below fails the run. Warning and
            // filing anyway would settle the territory low, irreversibly once acked.
            if !szr.is_complete() {
                incomplete.push(format!(
                    "{gebiet}: {} of {} settlement slots carry no value",
                    szr.missing_slot_count(),
                    szr.expected_slot_count()
                ));
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

        // Every MaLo aggregated, yet the grid still has holes: the missing energy
        // is real absence, not an aggregation failure. It under-reports exactly
        // as an excluded MaLo does, so it fails the run for the same reason.
        if !incomplete.is_empty() {
            anyhow::bail!(
                "{} Summenzeitreihe(n) would be filed with gaps in the settlement grid, \
                 under-reporting the territory: {}",
                incomplete.len(),
                incomplete.join("; ")
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
    /// Returns the per-Bilanzierungsgebiet grouping plus the MaLos whose master
    /// data names **no** territory — those must be excluded and the run failed,
    /// never folded into a fallback zone (misfiling energy the BIKO cannot detect).
    /// The MaBiS-Zählpunkt a Bilanzierungsgebiet's Summenzeitreihen are filed
    /// under, from `marktd` master data.
    ///
    /// A Summenzeitreihe is submitted under its MaBiS-Zählpunkt (SG6 `LOC+172`);
    /// the Bilanzierungsgebiet EIC is a separate `LOC+107`. Both are free text
    /// at the MIG level, so a Summenzeitreihe filed against the wrong Meldepunkt
    /// is, to the BIKO, indistinguishable from a correct one.
    ///
    /// Every failure path therefore **refuses** rather than substituting: an
    /// unassigned territory, an unreachable marktd, and a malformed response all
    /// abort the submission. Falling back to the EIC would produce exactly the
    /// misfiled message this lookup exists to prevent.
    async fn resolve_mabis_zp(
        &self,
        gebiet: &str,
    ) -> anyhow::Result<mako_mabis::MabisZaehlpunktId> {
        let cfg = &self.cfg;
        let url = format!(
            "{}/api/v1/bilanzierungsgebiete/{gebiet}/mabis-zp",
            cfg.marktd.url
        );
        let resp = self
            .marktd_client
            .get(&url)
            .bearer_auth(&cfg.marktd.api_key)
            .send()
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "marktd unreachable while resolving the MaBiS-Zählpunkt for \
                     Bilanzierungsgebiet {gebiet}: {e}"
                )
            })?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!(
                "no MaBiS-Zählpunkt assigned to Bilanzierungsgebiet {gebiet} — assign one via \
                 PUT /api/v1/bilanzierungsgebiete/{gebiet}/mabis-zp on marktd; the \
                 Summenzeitreihe cannot be submitted without the SG6 LOC+172 Meldepunkt"
            );
        }
        if !resp.status().is_success() {
            anyhow::bail!(
                "marktd returned {} resolving the MaBiS-Zählpunkt for Bilanzierungsgebiet {gebiet}",
                resp.status()
            );
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| {
            anyhow::anyhow!("malformed marktd response for Bilanzierungsgebiet {gebiet}: {e}")
        })?;
        let zp = body["mabis_zp_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("marktd returned no mabis_zp_id for Bilanzierungsgebiet {gebiet}")
            })?;

        // marktd rejects a malformed Meldepunkt on write, but the submission
        // path is the one with the irreversible consequence, so it does not take
        // that on trust. Parsing here is the boundary: past this point the
        // identifier is a `MabisZaehlpunktId`, and putting a territory code in
        // its place stops being expressible.
        let zp = mako_mabis::MabisZaehlpunktId::new(zp).map_err(|e| {
            anyhow::anyhow!("marktd returned an unusable MaBiS-Zählpunkt for {gebiet}: {e}")
        })?;
        if zp.as_str() == gebiet {
            anyhow::bail!(
                "marktd returned the Bilanzierungsgebiet EIC as the MaBiS-Zählpunkt for \
                 {gebiet} — refusing to file the Summenzeitreihe under a territory code"
            );
        }
        Ok(zp)
    }

    async fn resolve_bilanzierungsgebiete(
        &self,
        malo_ids: &[String],
    ) -> (std::collections::BTreeMap<String, Vec<String>>, Vec<String>) {
        use std::collections::BTreeMap;
        let cfg = &self.cfg;
        let mut by_gebiet: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut missing_gebiet: Vec<String> = Vec::new();

        for malo_id in malo_ids {
            let url = format!("{}/api/v1/malos/{malo_id}", cfg.marktd.url);
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

            match gebiet {
                Some(key) => by_gebiet.entry(key).or_default().push(malo_id.clone()),
                None => {
                    warn!(
                        malo_id,
                        "mabis-syncd: MaLo has no Bilanzierungsgebiet in marktd — excluded from \
                         the run rather than misfiled into a fallback zone"
                    );
                    missing_gebiet.push(malo_id.clone());
                }
            }
        }
        (by_gebiet, missing_gebiet)
    }

    /// Discover MaLo IDs from edmd billing periods for the given time window.
    async fn discover_malos(&self, from: Date, to: Date) -> Result<Vec<String>> {
        let cfg = &self.cfg;
        let url = format!(
            "{}/api/v1/billing-periods?from={from}&to={to}&tenant={}",
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

        Self::parse_lastgaenge(malo_id, &data)
    }

    /// Turn edmd's BO4E `Lastgang` array into the MaLo's Bezugs-intervals.
    ///
    /// The endpoint returns **one Lastgang per OBIS code**. Flattening all of
    /// them summed a MaLo's Bezugs- (`1.x.y`) and Einspeisungsregister
    /// (`2.x.y`) into the same settlement slot, double-counting the MaLo into
    /// the Summenzeitreihe. The BG-SZR settles Bezug, so only the import
    /// direction contributes.
    fn parse_lastgaenge(
        malo_id: &str,
        data: &serde_json::Value,
    ) -> Result<Vec<metering::MeterInterval>> {
        let lastgaenge = data
            .as_array()
            .with_context(|| format!("edmd /lastgang/{malo_id} did not return an array"))?;

        let mut intervals: Vec<metering::MeterInterval> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        for lastgang in lastgaenge {
            // metering 0.16 types OBIS as `ObisCode`; parse and drop an unparseable code.
            let raw_obis = lastgang["obisKennzahl"].as_str().unwrap_or_default();
            let obis = metering::ObisCode::parse(raw_obis).ok();
            if !obis.as_ref().is_some_and(metering::ObisCode::is_import) {
                skipped.push(raw_obis.to_owned());
                continue;
            }
            for wert in lastgang["werte"].as_array().into_iter().flatten() {
                let zeitraum = &wert["zeitraum"];
                let from =
                    Self::parse_zeitraum_bound(&zeitraum["startdatum"], &zeitraum["startuhrzeit"]);
                let to = Self::parse_zeitraum_bound(&zeitraum["enddatum"], &zeitraum["enduhrzeit"]);
                let (Some(from), Some(to)) = (from, to) else {
                    continue;
                };
                let Some(value) = Self::parse_decimal(&wert["wert"]) else {
                    continue;
                };
                intervals.push(metering::MeterInterval {
                    from,
                    to,
                    value,
                    quality: Self::messwertstatus_to_quality(wert["status"].as_str()),
                    obis_code: obis,
                });
            }
        }
        intervals.sort_by_key(|iv| iv.from);

        // Every series was a non-Bezug register. Reporting nothing would look
        // like a MaLo without data and short the territory silently, so it is
        // named instead.
        if intervals.is_empty() && !skipped.is_empty() {
            anyhow::bail!(
                "edmd returned no Bezugs-Lastgang for {malo_id}; only these registers: {}",
                skipped.join(", ")
            );
        }
        if !skipped.is_empty() {
            info!(
                malo_id,
                skipped = skipped.join(", "),
                "mabis-syncd: non-Bezug registers excluded from the Summenzeitreihe"
            );
        }

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

        // Re-checked per submission, not only at startup: the target selects the
        // routing key (Bilanzierungsgebiet today, MaLo-ID under BK6-24-210), so a
        // future Hub arm must not fall through to the bilateral payload.
        cfg.submission_target.ensure_supported()?;

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
                "mabis_zp_id": summenzeitreihe.mabis_zp_id,
                "bilanzierungsgebiet_id": summenzeitreihe.bilanzierungsgebiet_id.as_ref(),
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

        let idempotency_key = idempotency_key(
            run_id,
            summenzeitreihe.bilanzierungsgebiet_id.as_ref(),
            summenzeitreihe.version,
        );

        let url = format!("{}/api/v1/commands", cfg.makod.url);
        let resp = self
            .makod_client
            .post(&url)
            .bearer_auth(&cfg.makod.api_key)
            .header("Idempotency-Key", &idempotency_key)
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

    /// Mark a run failed and announce it, in one transaction.
    ///
    /// The failure row and the `de.mabis.submission.failed` outbox record
    /// commit together (persist-before-dispatch): a failure nothing announces
    /// reads as a submission still pending — which is exactly how this
    /// service's failures were invisible to the agent plane before the event
    /// existed. Write errors are logged rather than dropped; swallowing one
    /// left the run in `pending`, where the retry list picks it up forever and
    /// nothing records why the submission never went out.
    async fn mark_failed_and_emit(
        &self,
        run_id: Uuid,
        period_from: Date,
        period_to: Date,
        abrechnungslauf: pg::Abrechnungslauf,
        phase: pg::SubmissionPhase,
        error_msg: &str,
    ) {
        let result = async {
            let mut tx = self.pool.begin().await?;
            let attempt_count = pg::mark_failed(&mut tx, run_id, error_msg).await?;
            let ce = mako_service::CloudEvent::new(
                mako_service::source("mabis-syncd", &self.cfg.identity.tenant),
                mako_events::mabis::SUBMISSION_FAILED,
                run_id.to_string(),
                serde_json::json!({
                    "run_id": run_id.to_string(),
                    "bilanzierungsgebiet_id": self.cfg.identity.bilanzierungsgebiet_id,
                    "period_from": period_from.to_string(),
                    "period_to": period_to.to_string(),
                    "abrechnungslauf": abrechnungslauf.as_str(),
                    "phase": phase.as_str(),
                    "attempt_count": attempt_count,
                    "error": error_msg,
                }),
            );
            mako_service::outbox::enqueue(&mut tx, &ce).await?;
            tx.commit().await
        }
        .await;
        if let Err(e) = result {
            warn!(
                run_id = %run_id, error = %e,
                "mabis-syncd: could not record the run failure — the run stays in 'pending' \
                 and no de.mabis.submission.failed event was enqueued"
            );
        }
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

    /// The stored version is what the BIKO echoes back in a Datenstatus, and it
    /// is matched by equality. Anything the wire cannot carry must therefore be
    /// gone before the row is written.
    #[test]
    fn the_stored_version_round_trips_through_the_wire_format() {
        let raw = datetime!(2026-06-14 05:07:09.123456 UTC);
        let stored = truncate_to_seconds(raw);

        assert_eq!(stored, datetime!(2026-06-14 05:07:09 UTC));
        assert_eq!(stored.nanosecond(), 0);
        // The wire value is identical either way — which is exactly why an
        // untruncated row could never be found again.
        assert_eq!(fmt_edifact_version(raw), fmt_edifact_version(stored));
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use time::macros::{date, datetime};

    /// Number of MaBiS quarter-hour slots the window spans — the same
    /// derivation `Summenzeitreihe::expected_slot_count` performs.
    fn slots(period_from: Date, period_to: Date) -> i64 {
        let (from, to) = aggregation_window(period_from, period_to);
        (to - from).whole_seconds() / (15 * 60)
    }

    /// `period_to` is the inclusive last day of the month, so the window has to
    /// run to the *following* midnight. Ending at `period_to` itself dropped the
    /// last day of every Bilanzierungsmonat — 96 slots of energy.
    #[test]
    fn a_plain_month_spans_every_slot_including_the_last_day() {
        assert_eq!(slots(date!(2026 - 06 - 01), date!(2026 - 06 - 30)), 30 * 96);
        let (from, to) = aggregation_window(date!(2026 - 06 - 01), date!(2026 - 06 - 30));
        assert_eq!(
            from,
            datetime!(2026-05-31 22:00 UTC),
            "Berlin midnight, CEST"
        );
        assert_eq!(to, datetime!(2026-06-30 22:00 UTC));
    }

    /// The MaBiS grid is a Berlin-local grid. A UTC window keeps every month at
    /// 24 h a day, which is wrong for exactly the two DST months — and the BIKO
    /// cannot tell a short series from a complete one.
    #[test]
    fn the_dst_months_are_short_and_long_by_one_hour() {
        // 2026-03-29: 02:00 → 03:00, so March holds 31 × 96 − 4 slots.
        assert_eq!(slots(date!(2026 - 03 - 01), date!(2026 - 03 - 31)), 2972);
        // 2026-10-25: 03:00 → 02:00, so October holds 31 × 96 + 4 slots.
        assert_eq!(slots(date!(2026 - 10 - 01), date!(2026 - 10 - 31)), 2980);
    }

    /// A January period must start in the previous UTC year (CET is +01).
    #[test]
    fn a_winter_window_starts_the_evening_before() {
        let (from, to) = aggregation_window(date!(2026 - 01 - 01), date!(2026 - 01 - 31));
        assert_eq!(from, datetime!(2025-12-31 23:00 UTC));
        assert_eq!(to, datetime!(2026-01-31 23:00 UTC));
        assert_eq!(slots(date!(2026 - 01 - 01), date!(2026 - 01 - 31)), 31 * 96);
    }
}

#[cfg(test)]
mod lastgang_tests {
    use super::*;

    fn lastgang(obis: &str, wert: &str) -> serde_json::Value {
        serde_json::json!({
            "obisKennzahl": obis,
            "werte": [{
                "zeitraum": {
                    "startdatum": "2026-06-01",
                    "startuhrzeit": "00:00:00+00:00",
                    "enddatum": "2026-06-01",
                    "enduhrzeit": "00:15:00+00:00",
                },
                "wert": wert,
                "status": "ABGELESEN",
            }],
        })
    }

    /// A MaLo with both a Bezugs- and an Einspeisungsregister returns two
    /// Lastgänge over the same slots. Summing both put the feed-in energy into
    /// the Summenzeitreihe as consumption.
    #[test]
    fn the_einspeisung_register_does_not_contribute() {
        let data = serde_json::json!([
            lastgang("1-0:1.29.0", "12.5"),
            lastgang("1-0:2.29.0", "40.0"),
        ]);
        let intervals = SyncEngine::parse_lastgaenge("50123456789", &data).expect("parses");
        assert_eq!(intervals.len(), 1, "one slot, not one per register");
        assert_eq!(intervals[0].value, Decimal::new(125, 1));
    }

    /// A MaLo whose only registers are non-Bezug would otherwise look like a
    /// MaLo without data, and short the territory without saying so.
    #[test]
    fn a_malo_with_no_bezug_register_is_an_error() {
        let data = serde_json::json!([lastgang("1-0:2.29.0", "40.0")]);
        let err = SyncEngine::parse_lastgaenge("50123456789", &data).expect_err("refuses");
        assert!(err.to_string().contains("1-0:2.29.0"), "{err}");
    }
}

#[cfg(test)]
mod idempotency_tests {
    use super::*;
    use time::macros::datetime;

    /// `makod` rejects a command without the header (422
    /// `missing_idempotency_key`), so the key must exist — and a retry must
    /// present the same one, which means it derives only from the submission's
    /// identity and never from the clock.
    #[test]
    fn the_key_is_stable_for_one_submission_and_distinct_across_them() {
        let run = uuid::uuid!("2f1a5b6c-0d3e-4f50-8a91-b2c3d4e5f607");
        let other_run = uuid::uuid!("3f1a5b6c-0d3e-4f50-8a91-b2c3d4e5f607");
        let version = datetime!(2026-07-14 05:07:09 UTC);

        let key = idempotency_key(run, "11XBG-DEMO-----9", version);
        assert_eq!(
            key,
            idempotency_key(run, "11XBG-DEMO-----9", version),
            "a retry must reuse the key"
        );
        assert_eq!(
            key,
            "mabis-szr-2f1a5b6c-0d3e-4f50-8a91-b2c3d4e5f607-11XBG-DEMO-----9-20260714050709+00"
        );

        // Each territory files its own MSCONS, and each version is a distinct
        // filing — neither may share a key with another.
        assert_ne!(key, idempotency_key(run, "11XBG-OTHER----2", version));
        assert_ne!(key, idempotency_key(other_run, "11XBG-DEMO-----9", version));
        assert_ne!(
            key,
            idempotency_key(run, "11XBG-DEMO-----9", datetime!(2026-07-14 05:07:10 UTC))
        );
    }
}
