//! edmd's shared reads: one function per read, served by a documented REST
//! route (`crate::api`) and by an MCP tool (`crate::mcp_server`) alike.
//!
//! Each returns one response type that derives both `schemars::JsonSchema`
//! (the tool's `output_schema`) and `utoipa::ToSchema` (the route's response
//! in `openapi.json`), so an integrator, the Desk and an agent read one
//! contract. Dates are ISO `YYYY-MM-DD` strings (`format: date`), instants
//! RFC 3339 in UTC (`format: date-time`), quantities decimal strings
//! (`format: decimal`) — never a JSON number, which cannot carry `0.1`.

use std::collections::BTreeMap;

use mako_service::ApiError;
use metering::{QualityFlag, Sparte};
use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::Serialize;
use sqlx::Row as _;
use time::format_description::well_known::Rfc3339;
use time::{Date, Duration, OffsetDateTime};
use utoipa::ToSchema;

use crate::domain::TimeSeriesQuery;
use crate::domain::repository::TimeSeriesRepository as _;
use crate::store::MeterStoreTimeSeriesRepository;

/// The longest window `get_timeseries` answers: a quarter of fifteen-minute
/// values over several registers is already tens of thousands of rows.
pub const MAX_TIMESERIES_DAYS: i64 = 93;

/// The longest range `get_coverage` answers: a year and a leap day.
pub const MAX_COVERAGE_DAYS: i64 = 366;

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}

fn instant(name: &str, raw: &str) -> Result<OffsetDateTime, ApiError> {
    OffsetDateTime::parse(raw, &Rfc3339).map_err(|_| {
        ApiError::bad_request(format!("{name} {raw:?} is not an RFC 3339 instant"))
            .with_code("invalid_instant")
    })
}

fn date(name: &str, raw: &str) -> Result<Date, ApiError> {
    Date::parse(raw, &time::format_description::well_known::Iso8601::DATE).map_err(|_| {
        ApiError::bad_request(format!("{name} {raw:?} is not an ISO date (YYYY-MM-DD)"))
            .with_code("invalid_date")
    })
}

fn store_failure(what: &str, e: impl std::error::Error + Send + Sync + 'static) -> ApiError {
    ApiError::internal(anyhow::Error::new(e).context(what.to_owned()))
}

// ── Messwertqualität ─────────────────────────────────────────────────────────

/// The quality of one stored value — the MSCONS Messwertstatus: `MEASURED`
/// (Messwert), `ESTIMATED` (Prognosewert), `SUBSTITUTED` (Ersatzwert),
/// `CALCULATED` (Rechenwert), `CORRECTED` (Nachbearbeitungswert),
/// `PRELIMINARY` (vorläufig), `FAULTY` (fehlerhaft, not billable), `UNKNOWN`.
///
/// The variants carry no doc comment of their own: one would make schemars
/// describe the enum as a `oneOf` of constants where utoipa writes an `enum`,
/// and the two doors' schemas would differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Messwertqualitaet {
    Measured,
    Estimated,
    Substituted,
    Calculated,
    Corrected,
    Preliminary,
    Faulty,
    Unknown,
}

impl From<QualityFlag> for Messwertqualitaet {
    fn from(q: QualityFlag) -> Self {
        match q {
            QualityFlag::Measured => Self::Measured,
            QualityFlag::Estimated => Self::Estimated,
            QualityFlag::Substituted => Self::Substituted,
            QualityFlag::Calculated => Self::Calculated,
            QualityFlag::Corrected => Self::Corrected,
            QualityFlag::Preliminary => Self::Preliminary,
            QualityFlag::Faulty => Self::Faulty,
            _ => Self::Unknown,
        }
    }
}

impl Messwertqualitaet {
    fn from_db(raw: &str) -> Self {
        match raw {
            "MEASURED" => Self::Measured,
            "ESTIMATED" => Self::Estimated,
            "SUBSTITUTED" => Self::Substituted,
            "CALCULATED" => Self::Calculated,
            "CORRECTED" => Self::Corrected,
            "PRELIMINARY" => Self::Preliminary,
            "FAULTY" => Self::Faulty,
            _ => Self::Unknown,
        }
    }
}

// ── get_timeseries ───────────────────────────────────────────────────────────

/// One stored interval of a Marktlokation's series.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Messwert {
    /// Interval start.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub dtm_from: String,
    /// Interval end (exclusive).
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub dtm_to: String,
    /// The quantity in the Sparte's billing unit (kWh; m³ for water).
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub quantity_kwh: String,
    /// The value's quality.
    pub quality: Messwertqualitaet,
    /// The register's OBIS-Kennzahl, when the delivery named one.
    pub obis_code: Option<String>,
}

/// The version-resolved series of a Marktlokation over a window.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Messwerte {
    pub malo_id: String,
    /// Window start (inclusive).
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub from: String,
    /// Window end.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub to: String,
    /// How many intervals `readings` holds.
    pub count: usize,
    /// Every register's intervals, oldest first. Empty when nothing was
    /// delivered for the window.
    pub readings: Vec<Messwert>,
}

/// The series of `malo_id` between `from` and `to` (RFC 3339), at most
/// [`MAX_TIMESERIES_DAYS`] long.
///
/// # Errors
///
/// `invalid_instant`, `zeitraum_ungueltig` (`to` not after `from`),
/// `zeitraum_zu_lang`, or a store failure.
pub async fn timeseries(
    repo: &MeterStoreTimeSeriesRepository,
    tenant: &str,
    malo_id: &str,
    from: &str,
    to: &str,
) -> Result<Messwerte, ApiError> {
    let from_t = instant("from", from)?;
    let to_t = instant("to", to)?;
    if to_t <= from_t {
        return Err(ApiError::bad_request("to must lie after from").with_code("zeitraum_ungueltig"));
    }
    if to_t - from_t > Duration::days(MAX_TIMESERIES_DAYS) {
        return Err(ApiError::unprocessable(format!(
            "a window longer than {MAX_TIMESERIES_DAYS} days is refused; ask for it in parts"
        ))
        .with_code("zeitraum_zu_lang"));
    }
    let reads = repo
        .query(&TimeSeriesQuery {
            malo_id: malo_id.to_owned(),
            from: from_t,
            to: to_t,
            sparte: None,
            tenant: tenant.to_owned(),
        })
        .await
        .map_err(|e| store_failure("get_timeseries", e))?;
    let readings: Vec<Messwert> = reads
        .iter()
        .map(|r| Messwert {
            dtm_from: rfc3339(r.dtm_from),
            dtm_to: rfc3339(r.dtm_to),
            quantity_kwh: r.quantity_kwh.to_string(),
            quality: r.quality.into(),
            obis_code: r.obis_code.clone(),
        })
        .collect();
    Ok(Messwerte {
        malo_id: malo_id.to_owned(),
        from: rfc3339(from_t),
        to: rfc3339(to_t),
        count: readings.len(),
        readings,
    })
}

// ── get_quality_warnings ─────────────────────────────────────────────────────

/// A quality grade: `A` clean, `B` warnings, `C` errors an Ersatzwert
/// repairs, `F` blocking — the window is not billable as it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
pub enum Note {
    A,
    B,
    C,
    F,
}

impl Note {
    fn from_grade(grade: &str) -> Self {
        match grade {
            "A" => Self::A,
            "B" => Self::B,
            "C" => Self::C,
            _ => Self::F,
        }
    }
}

/// The quality verdict over a window, recomputed from the version-resolved
/// series by the scorer the ingest and rescore paths run.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Qualitaetsbefund {
    pub malo_id: String,
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub window_from: String,
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub window_to: String,
    /// Intervals scored.
    pub interval_count: usize,
    pub grade: Note,
    pub has_warnings: bool,
    /// Slots of the window no interval delivered.
    pub gaps_detected: usize,
    /// The longest run of zeros, in slots.
    pub zero_run_length: usize,
    /// Delivered time inside the window over its length, in percent.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub coverage_pct: String,
    /// `false`: a slot delivered twice or a span off the grid kept the
    /// content rules from running.
    pub intervals_consistent: bool,
    /// Slots the window holds on the observed grid; absent without one.
    pub expected_intervals: Option<u32>,
    /// Interval starts flagged as statistical outliers.
    pub outlier_intervals: Vec<String>,
    /// Interval starts above the plant's physical capacity.
    pub spike_intervals: Vec<String>,
    /// The validation that produced the verdict.
    pub algorithm: String,
}

/// The quality verdict for `malo_id` over `[from, to)`; the last 30 days when
/// either bound is absent.
///
/// # Errors
///
/// `invalid_instant`, `no_data` (404: nothing stored in the window), or a
/// store failure.
pub async fn quality_warnings(
    repo: &MeterStoreTimeSeriesRepository,
    tenant: &str,
    malo_id: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Qualitaetsbefund, ApiError> {
    use metering::MeterInterval;
    let now = OffsetDateTime::now_utc();
    let from_t = from
        .map(|s| instant("from", s))
        .transpose()?
        .unwrap_or(now - Duration::days(30));
    let to_t = to.map(|s| instant("to", s)).transpose()?.unwrap_or(now);
    let reads = repo
        .query(&TimeSeriesQuery {
            malo_id: malo_id.to_owned(),
            from: from_t,
            to: to_t,
            sparte: None,
            tenant: tenant.to_owned(),
        })
        .await
        .map_err(|e| store_failure("get_quality_warnings", e))?;
    let Some(sparte) = reads.first().map(|r| r.sparte) else {
        return Err(ApiError::not_found(format!(
            "no meter reads for MaLo {malo_id} in the window"
        ))
        .with_code("no_data"));
    };
    let intervals: Vec<MeterInterval> = reads.iter().filter_map(|r| r.interval().ok()).collect();
    let report = crate::server::compute_quality(&intervals, sparte, from_t, to_t);
    Ok(Qualitaetsbefund {
        malo_id: malo_id.to_owned(),
        window_from: rfc3339(from_t),
        window_to: rfc3339(to_t),
        interval_count: report.intervals_accepted,
        grade: Note::from_grade(report.grade),
        has_warnings: report.has_warnings,
        gaps_detected: report.gaps_detected,
        zero_run_length: report.zero_run_length,
        coverage_pct: report.coverage_pct.to_string(),
        intervals_consistent: report.intervals_consistent,
        expected_intervals: report.expected_intervals,
        outlier_intervals: report.outlier_intervals,
        spike_intervals: report.spike_intervals,
        algorithm: report.algorithm,
    })
}

// ── get_correction_history ───────────────────────────────────────────────────

/// One retroactive correction of a stored interval (§ 147 Abs. 1 AO /
/// § 146 Abs. 4 AO, GoBD).
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Korrektur {
    #[schemars(extend("format" = "uuid"))]
    #[schema(format = Uuid)]
    pub correction_id: String,
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub dtm_from: String,
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub dtm_to: String,
    /// The register corrected; absent when the correction named none.
    pub obis_code: Option<String>,
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub original_kwh: String,
    pub original_quality: Messwertqualitaet,
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub corrected_kwh: String,
    pub corrected_quality: Messwertqualitaet,
    pub reason: String,
    /// `MSCONS_UPDATE`, `OPERATOR`, `AUTO_SUBSTITUTE`, `IMSYS_DIRECT_PUSH` or
    /// `OTHER`.
    pub source: String,
    pub corrected_by: Option<String>,
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub corrected_at: String,
}

/// The corrections recorded for a Marktlokation in a window, newest first.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Korrekturen {
    pub malo_id: String,
    /// Window over `corrected_at`.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub from: String,
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub to: String,
    pub count: usize,
    /// `limit` corrections matched: older ones may exist beyond the page.
    pub saturated: bool,
    pub corrections: Vec<Korrektur>,
}

/// The corrections recorded for `malo_id` with `corrected_at` in `[from, to]`
/// — the last 90 days by default — at most `limit` (100, at most 1000).
///
/// # Errors
///
/// `invalid_instant` or a database failure.
pub async fn correction_history(
    repo: &MeterStoreTimeSeriesRepository,
    tenant: &str,
    malo_id: &str,
    from: Option<&str>,
    to: Option<&str>,
    limit: Option<u32>,
) -> Result<Korrekturen, ApiError> {
    let now = OffsetDateTime::now_utc();
    let from_t = from
        .map(|s| instant("from", s))
        .transpose()?
        .unwrap_or(now - Duration::days(90));
    let to_t = to.map(|s| instant("to", s)).transpose()?.unwrap_or(now);
    let limit = limit.unwrap_or(100).clamp(1, 1000);
    let rows = sqlx::query(
        r"SELECT correction_id, dtm_from, dtm_to, obis_code_norm,
                 original_kwh, original_quality, corrected_kwh, corrected_quality,
                 reason, source, corrected_by, corrected_at
          FROM meter_read_corrections
          WHERE malo_id = $1 AND tenant = $2
            AND corrected_at >= $3 AND corrected_at <= $4
          ORDER BY corrected_at DESC, correction_id
          LIMIT $5",
    )
    .bind(malo_id)
    .bind(tenant)
    .bind(from_t)
    .bind(to_t)
    .bind(i64::from(limit))
    .fetch_all(repo.pool())
    .await
    .map_err(|e| store_failure("get_correction_history", e))?;
    let corrections = rows
        .iter()
        .map(|r| -> Result<Korrektur, sqlx::Error> {
            let obis: String = r.try_get("obis_code_norm")?;
            Ok(Korrektur {
                correction_id: r.try_get::<uuid::Uuid, _>("correction_id")?.to_string(),
                dtm_from: rfc3339(r.try_get("dtm_from")?),
                dtm_to: rfc3339(r.try_get("dtm_to")?),
                obis_code: (!obis.is_empty()).then_some(obis),
                original_kwh: r.try_get::<Decimal, _>("original_kwh")?.to_string(),
                original_quality: Messwertqualitaet::from_db(r.try_get("original_quality")?),
                corrected_kwh: r.try_get::<Decimal, _>("corrected_kwh")?.to_string(),
                corrected_quality: Messwertqualitaet::from_db(r.try_get("corrected_quality")?),
                reason: r.try_get("reason")?,
                source: r.try_get("source")?,
                corrected_by: r.try_get("corrected_by")?,
                corrected_at: rfc3339(r.try_get("corrected_at")?),
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| store_failure("get_correction_history row", e))?;
    Ok(Korrekturen {
        malo_id: malo_id.to_owned(),
        from: rfc3339(from_t),
        to: rfc3339(to_t),
        count: corrections.len(),
        saturated: corrections.len() == limit as usize,
        corrections,
    })
}

// ── get_coverage ─────────────────────────────────────────────────────────────

/// What a day of a Marktlokation's series holds: `gemessen` — every slot of
/// every energy register a measured value; `ersatzwert` — complete, with at
/// least one Ersatz- or Prognosewert; `korrigiert` — complete, no Ersatzwert,
/// at least one corrected slot; `fehlt` — at least one slot of an energy
/// register without a billable value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tageszustand {
    Gemessen,
    Ersatzwert,
    Korrigiert,
    Fehlt,
}

/// One day of the ribbon.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Tag {
    /// The Berlin calendar day — for Gas the Gastag starting 06:00 that day.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub tag: String,
    pub zustand: Tageszustand,
}

/// One state per day for a Marktlokation's series.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Abdeckung {
    pub malo_id: String,
    /// `STROM`, `GAS`, `WAERME` or `WASSER` — Gas days are Gastage
    /// (06:00–06:00 Berlin), every other Sparte's calendar days.
    pub sparte: String,
    /// First day answered.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub von: String,
    /// Last day answered: the requested `to`, or the last day that has
    /// already ended when `to` lies later. Before `von` when no requested day
    /// has ended yet, and then `tage` is empty.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub bis: String,
    /// One entry per day, oldest first.
    pub tage: Vec<Tag>,
    pub gemessen: usize,
    pub ersatzwert: usize,
    pub korrigiert: usize,
    pub fehlt: usize,
}

/// One stored interval as the day classification reads it.
#[derive(Debug, Clone)]
pub struct Abschnitt {
    pub from: OffsetDateTime,
    pub to: OffsetDateTime,
    /// The register, `None` when the delivery named none.
    pub register: Option<String>,
    pub quality: QualityFlag,
}

/// The state of the day `[start, end)`.
///
/// **The rule, chosen so a gap is never hidden:** the energy registers are the
/// ones seen anywhere in the requested range (a day with nothing is `fehlt`).
/// Each must cover the whole day with billable values — measured by time, not
/// by counting slots, so a 23- or 25-hour day and any grid are judged alike;
/// a `FAULTY` or `UNKNOWN` value is no value. Any shortfall is `fehlt`. A
/// complete day with an Ersatz- or Prognosewert (`SUBSTITUTED`, `ESTIMATED`)
/// is `ersatzwert`; else one with a corrected value (`CORRECTED`, or a
/// correction recorded for an interval of the day) is `korrigiert`; else
/// `gemessen`.
#[must_use]
pub fn tageszustand(
    start: OffsetDateTime,
    end: OffsetDateTime,
    registers: &[Option<String>],
    abschnitte: &[Abschnitt],
    korrekturen: &[(OffsetDateTime, OffsetDateTime)],
) -> Tageszustand {
    if registers.is_empty() {
        return Tageszustand::Fehlt;
    }
    let mut ersatz = false;
    let mut korrigiert = korrekturen.iter().any(|(f, t)| *f < end && *t > start);
    for register in registers {
        let mut spans: Vec<(OffsetDateTime, OffsetDateTime)> = Vec::new();
        for a in abschnitte.iter().filter(|a| &a.register == register) {
            let (f, t) = (a.from.max(start), a.to.min(end));
            if f >= t || !a.quality.is_billable() {
                continue;
            }
            spans.push((f, t));
            ersatz |= matches!(a.quality, QualityFlag::Substituted | QualityFlag::Estimated);
            korrigiert |= a.quality == QualityFlag::Corrected;
        }
        spans.sort();
        let mut reached = start;
        for (f, t) in spans {
            if f > reached {
                return Tageszustand::Fehlt;
            }
            reached = reached.max(t);
        }
        if reached < end {
            return Tageszustand::Fehlt;
        }
    }
    if ersatz {
        Tageszustand::Ersatzwert
    } else if korrigiert {
        Tageszustand::Korrigiert
    } else {
        Tageszustand::Gemessen
    }
}

/// The instant the day `d` starts on `sparte`'s day boundary — 06:00 Berlin
/// for the Gastag, Berlin midnight otherwise.
#[must_use]
pub fn tagesbeginn(sparte: Sparte, d: Date) -> OffsetDateTime {
    mako_fristen::berlin_at(d, crate::domain::vee::day_boundary(sparte).local_start())
}

/// One state per day `from..=to` (ISO dates) for `malo_id`, at most
/// [`MAX_COVERAGE_DAYS`]; see [`tageszustand`] for the rule. Days that have
/// not ended yet are not answered. `sparte` as `get_billing_period` takes it:
/// absent, the one the MaLo's readings are stored under.
///
/// # Errors
///
/// `invalid_date`, `zeitraum_ungueltig`, `zeitraum_zu_lang`, `sparte_unklar`
/// (no Sparte given and none stored, or one contradicting the stored), or a
/// store failure.
pub async fn coverage(
    repo: &MeterStoreTimeSeriesRepository,
    tenant: &str,
    malo_id: &str,
    from: &str,
    to: &str,
    sparte: Option<&str>,
) -> Result<Abdeckung, ApiError> {
    let von = date("from", from)?;
    let bis = date("to", to)?;
    if bis < von {
        return Err(
            ApiError::bad_request("to must not lie before from").with_code("zeitraum_ungueltig")
        );
    }
    if (bis - von).whole_days() + 1 > MAX_COVERAGE_DAYS {
        return Err(ApiError::unprocessable(format!(
            "a range longer than {MAX_COVERAGE_DAYS} days is refused; ask for it in parts"
        ))
        .with_code("zeitraum_zu_lang"));
    }
    let stored = repo
        .latest_read(malo_id, tenant)
        .await
        .map_err(|e| store_failure("get_coverage", e))?
        .map(|r| r.sparte);
    let sparte = crate::domain::settlement_sparte(sparte, stored)
        .map_err(|e| ApiError::unprocessable(e).with_code("sparte_unklar"))?;

    // Only days that have ended: today's day is still being delivered.
    let now = OffsetDateTime::now_utc();
    let mut last = bis;
    while last >= von && last.next_day().is_none_or(|n| tagesbeginn(sparte, n) > now) {
        match last.previous_day() {
            Some(p) => last = p,
            None => break,
        }
    }
    let mut tage = Vec::new();
    if last >= von {
        let start = tagesbeginn(sparte, von);
        let end = tagesbeginn(sparte, last.next_day().unwrap_or(last));
        let reads = repo
            .query(&TimeSeriesQuery {
                malo_id: malo_id.to_owned(),
                from: start,
                to: end,
                sparte: None,
                tenant: tenant.to_owned(),
            })
            .await
            .map_err(|e| store_failure("get_coverage", e))?;
        let abschnitte: Vec<Abschnitt> = reads
            .iter()
            .filter_map(|r| {
                let iv = r.interval().ok()?;
                let obis = iv.obis();
                if !obis.is_none_or(crate::domain::register::is_energy_register) {
                    return None;
                }
                Some(Abschnitt {
                    from: r.dtm_from,
                    to: r.dtm_to,
                    register: obis.map(|c| c.to_string()),
                    quality: r.quality,
                })
            })
            .collect();
        let registers: Vec<Option<String>> = abschnitte
            .iter()
            .map(|a| a.register.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let korrekturen: Vec<(OffsetDateTime, OffsetDateTime)> = sqlx::query_as(
            r"SELECT dtm_from, dtm_to FROM meter_read_corrections
              WHERE malo_id = $1 AND tenant = $2 AND dtm_to > $3 AND dtm_from < $4",
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(start)
        .bind(end)
        .fetch_all(repo.pool())
        .await
        .map_err(|e| store_failure("get_coverage corrections", e))?;
        let mut d = von;
        loop {
            let (s, e) = (
                tagesbeginn(sparte, d),
                tagesbeginn(sparte, d.next_day().unwrap_or(d)),
            );
            tage.push(Tag {
                tag: d.to_string(),
                zustand: tageszustand(s, e, &registers, &abschnitte, &korrekturen),
            });
            if d >= last {
                break;
            }
            match d.next_day() {
                Some(n) => d = n,
                None => break,
            }
        }
    }
    let mut zaehler: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &tage {
        let k = match t.zustand {
            Tageszustand::Gemessen => "g",
            Tageszustand::Ersatzwert => "e",
            Tageszustand::Korrigiert => "k",
            Tageszustand::Fehlt => "f",
        };
        *zaehler.entry(k).or_default() += 1;
    }
    let n = |k: &str| zaehler.get(k).copied().unwrap_or(0);
    Ok(Abdeckung {
        malo_id: malo_id.to_owned(),
        sparte: sparte.as_str().to_owned(),
        von: von.to_string(),
        bis: last.to_string(),
        gemessen: n("g"),
        ersatzwert: n("e"),
        korrigiert: n("k"),
        fehlt: n("f"),
        tage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    fn slots(
        start: OffsetDateTime,
        end: OffsetDateTime,
        q: QualityFlag,
        register: &str,
    ) -> Vec<Abschnitt> {
        let mut out = Vec::new();
        let mut t = start;
        while t < end {
            out.push(Abschnitt {
                from: t,
                to: t + Duration::minutes(15),
                register: Some(register.to_owned()),
                quality: q,
            });
            t += Duration::minutes(15);
        }
        out
    }

    const BEZUG: &str = "1-1:1.29.0";

    /// 29.03.2026 is 23 hours long in Berlin: 92 slots cover it, and the
    /// day ends at 22:00 UTC, not 23:00.
    #[test]
    fn a_dst_day_is_complete_with_its_92_slots() {
        let s = tagesbeginn(Sparte::Strom, date!(2026 - 03 - 29));
        let e = tagesbeginn(Sparte::Strom, date!(2026 - 03 - 30));
        assert_eq!(s, datetime!(2026-03-28 23:00 UTC));
        assert_eq!(e, datetime!(2026-03-29 22:00 UTC));
        let a = slots(s, e, QualityFlag::Measured, BEZUG);
        assert_eq!(a.len(), 92);
        let regs = [Some(BEZUG.to_owned())];
        assert_eq!(tageszustand(s, e, &regs, &a, &[]), Tageszustand::Gemessen);
    }

    /// One missing quarter-hour makes the day `fehlt`, whatever else it holds.
    #[test]
    fn one_missing_slot_is_fehlt() {
        let s = tagesbeginn(Sparte::Strom, date!(2026 - 06 - 10));
        let e = tagesbeginn(Sparte::Strom, date!(2026 - 06 - 11));
        let mut a = slots(s, e, QualityFlag::Substituted, BEZUG);
        a.remove(40);
        let regs = [Some(BEZUG.to_owned())];
        assert_eq!(tageszustand(s, e, &regs, &a, &[]), Tageszustand::Fehlt);
        let mut a = slots(s, e, QualityFlag::Measured, BEZUG);
        a[3].quality = QualityFlag::Faulty;
        assert_eq!(tageszustand(s, e, &regs, &a, &[]), Tageszustand::Fehlt);
    }

    /// A second energy register missing the day hides behind the first only
    /// if registers are judged together — they are not.
    #[test]
    fn every_register_must_cover_the_day() {
        let s = tagesbeginn(Sparte::Strom, date!(2026 - 06 - 10));
        let e = tagesbeginn(Sparte::Strom, date!(2026 - 06 - 11));
        let a = slots(s, e, QualityFlag::Measured, BEZUG);
        let regs = [Some(BEZUG.to_owned()), Some("1-1:2.29.0".to_owned())];
        assert_eq!(tageszustand(s, e, &regs, &a, &[]), Tageszustand::Fehlt);
        assert_eq!(tageszustand(s, e, &[], &[], &[]), Tageszustand::Fehlt);
    }

    /// Ersatzwert outranks a correction; a correction outranks measured.
    #[test]
    fn precedence_is_ersatzwert_then_korrigiert() {
        let s = tagesbeginn(Sparte::Strom, date!(2026 - 06 - 10));
        let e = tagesbeginn(Sparte::Strom, date!(2026 - 06 - 11));
        let regs = [Some(BEZUG.to_owned())];
        let mut a = slots(s, e, QualityFlag::Measured, BEZUG);
        let korr = [(s + Duration::hours(2), s + Duration::hours(3))];
        assert_eq!(
            tageszustand(s, e, &regs, &a, &korr),
            Tageszustand::Korrigiert
        );
        a[5].quality = QualityFlag::Corrected;
        assert_eq!(tageszustand(s, e, &regs, &a, &[]), Tageszustand::Korrigiert);
        a[6].quality = QualityFlag::Estimated;
        assert_eq!(
            tageszustand(s, e, &regs, &a, &korr),
            Tageszustand::Ersatzwert
        );
        // A correction of the neighbouring day does not touch this one.
        let a = slots(s, e, QualityFlag::Measured, BEZUG);
        let nachbar = [(e, e + Duration::hours(1))];
        assert_eq!(
            tageszustand(s, e, &regs, &a, &nachbar),
            Tageszustand::Gemessen
        );
    }

    /// The Gastag starts at 06:00 Berlin: 05:00 UTC in winter, 04:00 in
    /// summer; an hourly gas series covering 00:00–00:00 leaves its first six
    /// hours `fehlt`.
    #[test]
    fn a_gas_day_runs_from_six_to_six() {
        assert_eq!(
            tagesbeginn(Sparte::Gas, date!(2026 - 01 - 15)),
            datetime!(2026-01-15 05:00 UTC)
        );
        assert_eq!(
            tagesbeginn(Sparte::Gas, date!(2026 - 07 - 15)),
            datetime!(2026-07-15 04:00 UTC)
        );
        let s = tagesbeginn(Sparte::Gas, date!(2026 - 07 - 15));
        let e = tagesbeginn(Sparte::Gas, date!(2026 - 07 - 16));
        let kalendertag = slots(
            tagesbeginn(Sparte::Strom, date!(2026 - 07 - 15)),
            tagesbeginn(Sparte::Strom, date!(2026 - 07 - 16)),
            QualityFlag::Measured,
            "7-20:3.0.0",
        );
        let regs = [Some("7-20:3.0.0".to_owned())];
        assert_eq!(
            tageszustand(s, e, &regs, &kalendertag, &[]),
            Tageszustand::Fehlt
        );
        let gastag = slots(s, e, QualityFlag::Measured, "7-20:3.0.0");
        assert_eq!(
            tageszustand(s, e, &regs, &gastag, &[]),
            Tageszustand::Gemessen
        );
    }
}
