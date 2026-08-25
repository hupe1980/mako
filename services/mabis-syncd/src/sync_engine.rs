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
use mako_service::http::Upstream;

/// How many `marktd` master-data lookups run at once.
///
/// High enough that a tenant with thousands of MaLos resolves inside the
/// submission window; low enough that the aggregation does not become the load
/// that takes `marktd` down.
const MARKTD_CONCURRENCY: usize = 16;

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
    use mako_fristen::{HolidayCalendar, add_werktage};

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

/// One Bilanzierungsgebiet's aggregate, plus how many MaLos went into it.
///
/// `Summenzeitreihe` is a sum over slots; it cannot say how many MaLos it came
/// from, and that count is what an operator reads to see whether a territory
/// was fully covered.
pub struct TerritorySeries {
    /// The aggregate filed for this territory.
    pub series: Summenzeitreihe,
    /// MaLos folded into it.
    pub malo_count: usize,
}

// ── SyncEngine ────────────────────────────────────────────────────────────────

/// Core aggregation and submission engine.
pub struct SyncEngine {
    pool: sqlx::PgPool,
    edmd: Upstream,
    marktd: Upstream,
    makod: Upstream,
    cfg: std::sync::Arc<Config>,
}

impl SyncEngine {
    /// Create a new engine from configuration.
    ///
    /// A Lastgang fetch reads a month of quarter-hourly values, so `edmd` gets a
    /// longer request timeout than the shared default; the master-data and
    /// command calls keep it. All three share one connection pool and the
    /// no-redirect SSRF guard from `mako_service::http`.
    #[must_use]
    pub fn new(pool: sqlx::PgPool, cfg: std::sync::Arc<Config>) -> Self {
        let key = |k: &str| Some(secrecy::SecretString::from(k.to_owned()));
        let default = mako_service::http::default_client();
        let slow = mako_service::http::default_client_with(std::time::Duration::from_secs(120));
        Self {
            edmd: Upstream::new("edmd", &cfg.edmd.url, key(&cfg.edmd.api_key), slow),
            marktd: Upstream::new(
                "marktd",
                &cfg.marktd.url,
                key(&cfg.marktd.api_key),
                default.clone(),
            ),
            makod: Upstream::new("makod", &cfg.makod.url, key(&cfg.makod.api_key), default),
            pool,
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
                let total_kwh: Decimal = series.iter().map(|t| t.series.total_kwh()).sum();
                let malo_count = self.malo_count_for_run(run_id).await;
                let interval_count: i32 = series
                    .iter()
                    .map(|t| t.series.interval_count() as i32)
                    .sum();
                let has_substituted = series.iter().any(|t| t.series.has_substituted_values());

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

                // A retry of a partly-filed run must not re-file the
                // territories the BIKO already acked: an acked Summenzeitreihe
                // cannot be withdrawn, and resending one under a new version is
                // a correction, not a retry.
                let already_acked = pg::acked_territories(&self.pool, run_id)
                    .await
                    .unwrap_or_default();

                // Submit to BIKO via makod — one MSCONS 13003 per Bilanzierungsgebiet.
                match self
                    .submit_all_to_makod(&series, run_id, &already_acked)
                    .await
                {
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
    ) -> Result<Vec<TerritorySeries>> {
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
        let mut series: Vec<TerritorySeries> = Vec::with_capacity(by_gebiet.len());
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
            series.push(TerritorySeries {
                series: szr,
                malo_count: gebiet_malos.len(),
            });
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
        let path = format!("/api/v1/bilanzierungsgebiete/{gebiet}/mabis-zp");
        let body: serde_json::Value = self
            .marktd
            .json(self.marktd.get(&path))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "resolving the MaBiS-Zählpunkt for Bilanzierungsgebiet {gebiet}: {e}"
                )
            })?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no MaBiS-Zählpunkt assigned to Bilanzierungsgebiet {gebiet} — assign one via \
                     PUT /api/v1/bilanzierungsgebiete/{gebiet}/mabis-zp on marktd; the \
                     Summenzeitreihe cannot be submitted without the SG6 LOC+172 Meldepunkt"
                )
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
        let mut by_gebiet: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut missing_gebiet: Vec<String> = Vec::new();

        // Resolved in bounded-concurrency batches. A territory lookup is one
        // round-trip per MaLo, and a tenant of any size has thousands: run
        // sequentially, that alone outlasts the submission window it is
        // supposed to fit inside. The cap keeps `marktd` from being the thing
        // that fails instead.
        for chunk in malo_ids.chunks(MARKTD_CONCURRENCY) {
            let looked_up = futures::future::join_all(chunk.iter().map(|malo_id| async move {
                let path = format!("/api/v1/malos/{malo_id}");
                let gebiet = match self
                    .marktd
                    .json::<serde_json::Value>(self.marktd.get(&path))
                    .await
                {
                    Ok(Some(v)) => v["bilanzierungsgebiet"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                    Ok(None) => None,
                    Err(e) => {
                        warn!(malo_id, error = %e, "mabis-syncd: marktd MaLo lookup failed");
                        None
                    }
                };
                (malo_id.clone(), gebiet)
            }))
            .await;

            for (malo_id, gebiet) in looked_up {
                match gebiet {
                    Some(key) => by_gebiet.entry(key).or_default().push(malo_id),
                    None => {
                        warn!(
                            malo_id,
                            "mabis-syncd: MaLo has no Bilanzierungsgebiet in marktd — excluded \
                             from the run rather than misfiled into a fallback zone"
                        );
                        missing_gebiet.push(malo_id);
                    }
                }
            }
        }
        (by_gebiet, missing_gebiet)
    }

    /// Discover MaLo IDs from edmd billing periods for the given time window.
    async fn discover_malos(&self, from: Date, to: Date) -> Result<Vec<String>> {
        let cfg = &self.cfg;
        let request = self.edmd.get("/api/v1/billing-periods").query(&[
            ("from", from.to_string()),
            ("to", to.to_string()),
            ("tenant", cfg.identity.tenant.clone()),
        ]);
        let data: serde_json::Value = self
            .edmd
            .json(request)
            .await
            .context("edmd MaLo discovery")?
            .context("edmd has no billing periods for the tenant in this period")?;
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

        // MaBiS is a **Strom** process. The Bilanzkreisabrechnung it feeds
        // exists only for electricity — gas balances through GaBi Gas, on the
        // 06:00 Gastag and against a Marktgebiet, not a Bilanzierungsgebiet.
        // `edmd` serves both commodities from one endpoint, so an unfiltered
        // discovery folded every gas MaLo of the tenant into the electricity
        // Summenzeitreihe: energy that is not the BIKO's to settle, on a grid
        // it does not use, and the BIKO cannot tell an over-reported territory
        // from a correct one.
        let mut skipped_sparten: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        let mut malo_ids: Vec<String> = Vec::new();
        for p in periods {
            let sparte = p["sparte"].as_str().unwrap_or_default();
            if !sparte.eq_ignore_ascii_case("STROM") {
                skipped_sparten.insert(sparte.to_owned());
                continue;
            }
            if let Some(id) = p["malo_id"].as_str() {
                malo_ids.push(id.to_owned());
            }
        }
        if !skipped_sparten.is_empty() {
            info!(
                sparten = ?skipped_sparten,
                "mabis-syncd: non-Strom MaLos excluded — MaBiS settles electricity only"
            );
        }
        malo_ids.sort_unstable();
        malo_ids.dedup();

        Ok(malo_ids)
    }

    /// Read a decimal that edmd may serialise as either a JSON string or number.
    fn parse_decimal(v: &serde_json::Value) -> Option<Decimal> {
        match v {
            serde_json::Value::String(s) => s.parse().ok(),
            serde_json::Value::Number(n) => n.to_string().parse().ok(),
            _ => None,
        }
    }

    // The BO4E `Zeitraum` re-assembler and the `Messwertstatus` reverse-mapping
    // that used to live here are gone with the `/lastgang` parse. `/energy`
    // carries RFC 3339 instants and the stored `QualityFlag` vocabulary
    // directly, so neither the date/offset recombination nor a lossy inverse of
    // edmd's own BO4E mapping has to be maintained on this side.

    /// Fetch a MaLo's quarter-hourly **Bezug** series from edmd.
    ///
    /// Reads `GET /api/v1/energy/{malo_id}?direction=BEZUG` — edmd's canonical
    /// register projection, one entry per metered slot. MaBiS settles on that
    /// grid, so the resampled endpoints are not interchangeable here: a coarser
    /// bucket preserves the period total but destroys the shape the BIKO settles
    /// against.
    ///
    /// # Why not `/api/v1/lastgang`
    ///
    /// That endpoint returns one BO4E object **per OBIS register**, which is the
    /// right shape for a BO4E export and the wrong input for a settlement
    /// figure — folding it back into one series is the projection, and doing it
    /// here got it wrong. The filter was `ObisCode::is_import`, so on a
    /// dual-tariff MaLo the total register `1-0:1.8.0` passed it **and so did**
    /// `1-0:1.8.1` and `1-0:1.8.2`, which are its own decomposition: the
    /// consumption went into the Summenzeitreihe twice, in a filing the BIKO
    /// cannot withdraw. `1-0:1.6.0` — a Jahreshöchstleistung in kW — is import
    /// too, and was summed in as though it were energy, as was the
    /// Fehlerregister `…63`.
    ///
    /// edmd's `domain::register` decides all of that once, and now serves the
    /// answer instead of the raw registers.
    async fn fetch_lastgang(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
        as_of: Option<OffsetDateTime>,
    ) -> Result<Vec<metering::MeterInterval>> {
        use time::format_description::well_known::Rfc3339;
        let mut query = vec![
            ("from", from.format(&Rfc3339).unwrap_or_default()),
            ("to", to.format(&Rfc3339).unwrap_or_default()),
            ("direction", "BEZUG".to_owned()),
        ];
        // `as_of` reconstructs the data as it stood when an earlier version was
        // filed. A correction under the KBKA has to be able to say what changed
        // since the version the BIKO settled, which requires the earlier state
        // and not just the current one.
        if let Some(ts) = as_of {
            query.push(("as_of", ts.format(&Rfc3339).unwrap_or_default()));
        }

        let path = format!("/api/v1/energy/{malo_id}");
        let data: serde_json::Value = self
            .edmd
            .json(self.edmd.get(&path).query(&query))
            .await
            .with_context(|| format!("edmd energy series for {malo_id}"))?
            .with_context(|| format!("edmd has no Bezugs-series for {malo_id} in this period"))?;

        Self::parse_energy_series(malo_id, &data)
    }

    /// Turn edmd's projected energy series into typed intervals.
    ///
    /// No register logic here, deliberately: the response is already one series
    /// in one direction, and re-deriving anything from it is how the previous
    /// version came to double-count. The only judgement left is that a MaLo the
    /// BG-SZR needs must actually have a Bezugs-series — reporting nothing would
    /// look like a MaLo without data and short the territory silently.
    fn parse_energy_series(
        malo_id: &str,
        data: &serde_json::Value,
    ) -> Result<Vec<metering::MeterInterval>> {
        use time::format_description::well_known::Rfc3339;

        let entries = data["intervals"]
            .as_array()
            .with_context(|| format!("edmd /energy/{malo_id} carried no `intervals` array"))?;

        let mut intervals: Vec<metering::MeterInterval> = Vec::with_capacity(entries.len());
        for iv in entries {
            let (Some(from), Some(to)) = (
                iv["start"]
                    .as_str()
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok()),
                iv["end"]
                    .as_str()
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok()),
            ) else {
                continue;
            };
            let Some(value) = Self::parse_decimal(&iv["kwh"]) else {
                continue;
            };
            intervals.push(metering::MeterInterval {
                from,
                to,
                value,
                quality: iv["quality"]
                    .as_str()
                    .and_then(|q| q.parse().ok())
                    .unwrap_or(metering::QualityFlag::Unknown),
                obis_code: None,
            });
        }
        intervals.sort_by_key(|iv| iv.from);

        if intervals.is_empty() {
            anyhow::bail!(
                "edmd returned no Bezugs-series for {malo_id}: the point reports \
                 {count} interval(s) in that direction",
                count = intervals.len()
            );
        }

        Ok(intervals)
    }

    /// File one MSCONS 13003 per Bilanzierungsgebiet, recording each.
    ///
    /// # Why every territory is recorded
    ///
    /// A Summenzeitreihe the BIKO has acked **cannot be withdrawn**. When
    /// territory 4 of 4 fails, the first three are already filed, so "the run
    /// failed" is only half true and acting on it as though nothing went out
    /// re-files three binding submissions.
    ///
    /// Each territory therefore gets a `submission_series` row saying `acked`
    /// or `failed` with its reference or its reason; `skip` carries the ones a
    /// retry must not re-file. The run still fails as a whole, because a month
    /// settled short is not a success.
    ///
    /// Returns the first territory's `(message_ref, process_id)` for the run
    /// row — the whole story in a single-territory deployment.
    async fn submit_all_to_makod(
        &self,
        series: &[TerritorySeries],
        run_id: Uuid,
        skip: &[String],
    ) -> Result<(String, Option<Uuid>)> {
        let mut first: Option<(String, Option<Uuid>)> = None;
        let mut failures: Vec<String> = Vec::new();

        for territory in series {
            let szr = &territory.series;
            let gebiet = szr.bilanzierungsgebiet_id.as_ref().to_owned();
            if skip.contains(&gebiet) {
                info!(
                    run_id = %run_id, gebiet,
                    "mabis-syncd: territory already acked by the BIKO — not re-filed"
                );
                continue;
            }

            let series_id = pg::insert_series(
                &self.pool,
                run_id,
                &gebiet,
                szr.mabis_zp_id.as_str(),
                territory.malo_count as i32,
                szr.interval_count() as i32,
                &szr.total_kwh(),
            )
            .await
            .with_context(|| format!("record the series for {gebiet}"))?;

            match self.submit_to_makod(szr, run_id).await {
                Ok((message_ref, process_id)) => {
                    pg::mark_series_acked(&self.pool, series_id, &message_ref, process_id)
                        .await
                        .with_context(|| format!("record the ack for {gebiet}"))?;
                    info!(
                        run_id = %run_id, gebiet, message_ref,
                        "mabis-syncd: Summenzeitreihe filed"
                    );
                    if first.is_none() {
                        first = Some((message_ref, process_id));
                    }
                }
                Err(e) => {
                    // Recorded, then the loop continues: the remaining
                    // territories are independent filings, and stopping here
                    // would leave them unfiled *and* unexplained.
                    let msg = e.to_string();
                    warn!(run_id = %run_id, gebiet, error = %msg, "mabis-syncd: territory not filed");
                    let _ = pg::mark_series_failed(&self.pool, series_id, &msg).await;
                    failures.push(format!("{gebiet}: {msg}"));
                }
            }
        }

        if !failures.is_empty() {
            anyhow::bail!(
                "{} of {} Bilanzierungsgebiete were not filed — the month is settled short. \
                 Territories already acked are recorded in submission_series and are NOT \
                 re-filed by a retry: {}",
                failures.len(),
                series.len(),
                failures.join("; ")
            );
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

        let request = self
            .makod
            .post("/api/v1/commands")
            .header("Idempotency-Key", &idempotency_key)
            .json(&command);
        let result: serde_json::Value = self
            .makod
            .json(request)
            .await
            .context("makod command submission")?
            .context("makod accepted the command without a response body")?;
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

    fn series(intervals: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "malo_id": "50123456789",
            "direction": "BEZUG",
            "resolution_min": 15,
            "coverage_pct": 100.0,
            "billable_pct": 100.0,
            "interval_count": 1,
            "intervals": intervals,
        })
    }

    fn slot(start: &str, end: &str, kwh: &str) -> serde_json::Value {
        serde_json::json!({ "start": start, "end": end, "kwh": kwh, "quality": "MEASURED" })
    }

    /// The projected series is taken as given — no register logic here.
    #[test]
    fn the_projected_series_is_parsed_slot_for_slot() {
        let data = series(serde_json::json!([
            slot("2026-06-01T00:00:00Z", "2026-06-01T00:15:00Z", "12.5"),
            slot("2026-06-01T00:15:00Z", "2026-06-01T00:30:00Z", "13.5"),
        ]));
        let intervals = SyncEngine::parse_energy_series("50123456789", &data).expect("parses");
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[0].value, Decimal::new(125, 1));
        assert_eq!(intervals[0].quality, metering::QualityFlag::Measured);
        assert_eq!(
            intervals[0].to,
            time::macros::datetime!(2026-06-01 00:15:00 UTC),
            "the interval's own end, not an assumed grid"
        );
    }

    /// A MaLo with no Bezugs-series would otherwise look like a MaLo without
    /// data, and short the territory without saying so.
    #[test]
    fn a_malo_with_no_bezug_series_is_an_error() {
        let data = series(serde_json::json!([]));
        let err = SyncEngine::parse_energy_series("50123456789", &data).expect_err("refuses");
        assert!(err.to_string().contains("50123456789"), "{err}");
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
