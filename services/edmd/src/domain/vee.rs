//! edmd's bridge onto `metering`'s VEE pipeline: one register's readings as a
//! validated [`Series`], and its findings over the window a caller asked about.
//!
//! `metering` validates a [`Series`] — sorted, unique, on one grid, one
//! channel — against [`Rules`] that name a calendar [`Period`] (a day, a month
//! or a year on the Sparte's [`DayBoundary`]). edmd's windows are not calendar
//! periods: an ingest batch covers whatever span it was delivered for, and a
//! quality question names two arbitrary instants. So a window is validated
//! over the smallest calendar period that covers it — one per year where it
//! crosses New Year — and the findings are then **clipped to the window**:
//!
//! - a `GAP` run is cut to the window and its slot count recomputed, so the
//!   head and tail of the covering month do not read as missing data;
//! - every other finding is kept where it overlaps the window;
//! - coverage is the delivered time inside the window over the window's
//!   length, never against the covering period.
//!
//! A register whose intervals do not form a series — two values for one slot,
//! a span off the grid, no observable grid at all — is a [`SeriesDefect`]: the
//! content rules cannot run on it, and that is a finding of its own rather
//! than a clean report.

use metering::series::SeriesError;
use metering::vee::validation::{Finding, Report, Rule, Rules, Severity, validate};
use metering::{DayBoundary, MeterInterval, Period, Resolution, Series, Sparte};
use rust_decimal::Decimal;
use time::{Duration, OffsetDateTime};

/// The day boundary a Sparte balances on: the 06:00 Gastag for gas, the
/// calendar day for everything else.
#[must_use]
pub fn day_boundary(sparte: Sparte) -> DayBoundary {
    match sparte {
        Sparte::Gas => DayBoundary::Gas,
        _ => DayBoundary::Strom,
    }
}

/// The grid a register's intervals are delivered on, read off the median
/// interval length.
///
/// The median rather than the first interval, so one odd span does not decide
/// the grid every other interval is then judged against. `None` for no
/// intervals, or a median length that is no resolution (seven minutes).
#[must_use]
pub fn observed_resolution(intervals: &[MeterInterval]) -> Option<Resolution> {
    let mut secs: Vec<i64> = intervals.iter().map(MeterInterval::duration_secs).collect();
    if secs.is_empty() {
        return None;
    }
    secs.sort_unstable();
    Resolution::from_observed_seconds(secs[secs.len() / 2])
}

/// Why a register's intervals could not be validated as one series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SeriesDefect {
    /// The intervals have no observable grid: none at all, or a median length
    /// that is no resolution.
    #[error("the intervals have no observable grid")]
    NoGrid,
    /// The intervals are not one series on their grid.
    #[error(transparent)]
    Structure(#[from] SeriesError),
}

impl SeriesDefect {
    /// Stable code for the wire and the stored annotation.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoGrid => "NO_GRID",
            Self::Structure(SeriesError::Duplicate { .. }) => "DUPLICATE",
            Self::Structure(SeriesError::Overlap { .. }) => "OVERLAP",
            Self::Structure(SeriesError::Misaligned { .. }) => "MISALIGNED",
            Self::Structure(SeriesError::OutOfRange { .. }) => "OUT_OF_RANGE",
            Self::Structure(SeriesError::MixedChannels { .. }) => "MIXED_CHANNELS",
            Self::Structure(_) => "NOT_A_SERIES",
        }
    }

    /// The interval starts the defect names, so it can be attached to the rows
    /// it is about.
    #[must_use]
    pub fn instants(&self) -> Vec<OffsetDateTime> {
        match self {
            Self::NoGrid => Vec::new(),
            Self::Structure(SeriesError::Duplicate { from })
            | Self::Structure(SeriesError::Misaligned { from, .. })
            | Self::Structure(SeriesError::OutOfRange { from })
            | Self::Structure(SeriesError::MixedChannels { from }) => vec![*from],
            Self::Structure(SeriesError::Overlap { earlier, later }) => vec![*earlier, *later],
            Self::Structure(_) => Vec::new(),
        }
    }
}

/// One register's intervals as a series on its observed grid, cut on the
/// Sparte's day boundary.
///
/// # Errors
///
/// [`SeriesDefect`] when the intervals have no grid or do not form a series on
/// it.
pub fn series_of(sparte: Sparte, intervals: Vec<MeterInterval>) -> Result<Series, SeriesDefect> {
    let resolution = observed_resolution(&intervals).ok_or(SeriesDefect::NoGrid)?;
    Ok(Series::new(resolution, day_boundary(sparte), intervals)?)
}

/// A projected energy series — [`crate::domain::energy_intervals`]'s output —
/// as a series of its own.
///
/// The projection folds a meter's tariff registers into one figure per slot,
/// so its intervals no longer share an OBIS channel; they share a direction,
/// which is what the projection was asked for. The channel is dropped so the
/// intervals form one series.
///
/// # Errors
///
/// [`SeriesDefect`] when the intervals have no grid or do not form a series on
/// it.
pub fn energy_series(sparte: Sparte, intervals: &[MeterInterval]) -> Result<Series, SeriesDefect> {
    let unlabelled = intervals
        .iter()
        .filter_map(|iv| MeterInterval::new(iv.from(), iv.to(), iv.value(), iv.quality()).ok())
        .collect();
    series_of(sparte, unlabelled)
}

/// The calendar periods `[from, to)` is validated over: the smallest day,
/// month or year that covers it, else one period per year it touches.
///
/// `None` outside the calendar's supported years, or for an empty window.
#[must_use]
pub fn covering_periods(
    boundary: DayBoundary,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Option<Vec<Period>> {
    if from >= to {
        return None;
    }
    let first = boundary.day_of(from)?;
    let last = boundary.day_of(to - Duration::nanoseconds(1))?;
    if first == last {
        return Some(vec![boundary.day(first)?]);
    }
    if (first.year(), first.month()) == (last.year(), last.month()) {
        return Some(vec![boundary.month(first.year(), first.month())?]);
    }
    (first.year()..=last.year())
        .map(|year| boundary.year(year))
        .collect()
}

/// One calendar period's series and its report, findings clipped to the
/// window.
#[derive(Debug, Clone)]
pub struct PeriodReport {
    /// The part of the series inside the period.
    pub series: Series,
    /// The report over it, findings clipped to the window.
    pub report: Report,
}

/// What a series looks like over a window.
#[derive(Debug, Clone)]
pub struct WindowReport {
    /// One entry per covering calendar period.
    pub periods: Vec<PeriodReport>,
    /// The window's start.
    pub from: OffsetDateTime,
    /// The window's end (exclusive).
    pub to: OffsetDateTime,
    /// The series' grid.
    pub resolution: Resolution,
    /// Delivered time (any quality) inside the window.
    pub covered: Duration,
}

impl WindowReport {
    /// Every finding inside the window, by start.
    pub fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.periods.iter().flat_map(|p| p.report.findings.iter())
    }

    /// The rules that ran over **every** period.
    #[must_use]
    pub fn evaluated(&self) -> Vec<Rule> {
        Rule::ALL
            .into_iter()
            .filter(|r| self.periods.iter().all(|p| p.report.evaluated.contains(r)))
            .collect()
    }

    /// The rules that did not run over at least one period.
    #[must_use]
    pub fn skipped(&self) -> Vec<Rule> {
        let evaluated = self.evaluated();
        Rule::ALL
            .into_iter()
            .filter(|r| !evaluated.contains(r))
            .collect()
    }

    /// `A` none, `B` warnings only, `C` errors, `F` blocking.
    #[must_use]
    pub fn grade(&self) -> metering::vee::validation::Grade {
        metering::vee::validation::Grade::of(self.findings())
    }

    /// Delivered time inside the window over the window's length, in percent,
    /// rounded kaufmännisch to two places.
    #[must_use]
    pub fn coverage_pct(&self) -> Decimal {
        let expected = (self.to - self.from).whole_seconds();
        if expected <= 0 {
            return Decimal::ZERO;
        }
        (Decimal::from(self.covered.whole_seconds()) * Decimal::ONE_HUNDRED
            / Decimal::from(expected))
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
        .clamp(Decimal::ZERO, Decimal::ONE_HUNDRED)
    }

    /// Slots of the grid that start inside the window — the denominator a
    /// coverage percentage alone cannot supply.
    #[must_use]
    pub fn expected_slots(&self) -> u32 {
        let boundary = self
            .periods
            .first()
            .map_or(DayBoundary::Strom, |p| p.series.boundary());
        count_slots(boundary, self.resolution, self.from, self.to)
    }
}

/// Validate `series` over `[from, to)`: the rules `metering` presets for
/// `sparte`, the plant capacity when one is known, and `now` for `FUTURE`.
///
/// # Errors
///
/// `None` when the window lies outside the supported calendar or is empty.
#[must_use]
pub fn validate_window(
    series: &Series,
    sparte: Sparte,
    from: OffsetDateTime,
    to: OffsetDateTime,
    now: OffsetDateTime,
    capacity_kw: Option<Decimal>,
) -> Option<WindowReport> {
    let boundary = series.boundary();
    let periods = covering_periods(boundary, from, to)?;
    let mut out = Vec::with_capacity(periods.len());
    for period in periods {
        let inside: Vec<MeterInterval> = series
            .iter()
            .filter(|iv| period.start() <= iv.from() && iv.to() <= period.end())
            .cloned()
            .collect();
        // A subset of a series is a series on the same grid.
        let part = Series::new(series.resolution(), boundary, inside).ok()?;
        let mut report = validate(&part, &Rules::new(sparte, period, now, capacity_kw));
        clip_report(&mut report, boundary, series.resolution(), from, to);
        out.push(PeriodReport {
            series: part,
            report,
        });
    }
    let covered = series
        .iter()
        .map(|iv| (iv.to().min(to) - iv.from().max(from)).max(Duration::ZERO))
        .fold(Duration::ZERO, |sum, d| sum + d);
    Some(WindowReport {
        periods: out,
        from,
        to,
        resolution: series.resolution(),
        covered,
    })
}

/// Keep the findings that overlap `[from, to)`, cut to it; a `GAP` run's slot
/// count is recomputed over the part that is left.
///
/// A report clipped this way still describes its series, so it can be handed
/// to `metering::vee::substitute::substitute`, which then fills only the
/// window.
pub fn clip_report(
    report: &mut Report,
    boundary: DayBoundary,
    resolution: Resolution,
    from: OffsetDateTime,
    to: OffsetDateTime,
) {
    report.findings.retain(|f| f.from < to && f.to > from);
    for f in &mut report.findings {
        if f.from >= from && f.to <= to {
            continue;
        }
        f.from = f.from.max(from);
        f.to = f.to.min(to);
        if f.rule == Rule::Gap {
            let n = count_slots(boundary, resolution, f.from, f.to);
            f.reference = Some(Decimal::from(n));
            f.message = format!("{n} slot(s) missing from {} to {}", f.from, f.to);
        }
    }
    report
        .findings
        .retain(|f| f.rule != Rule::Gap || f.reference != Some(Decimal::ZERO));
}

/// Grid slots that start in `[from, to)`.
fn count_slots(
    boundary: DayBoundary,
    resolution: Resolution,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> u32 {
    let mut n = 0_u32;
    let mut cursor = boundary.bucket(from, resolution);
    while let Some(slot) = cursor {
        if slot.start() >= to {
            break;
        }
        if slot.start() >= from {
            n = n.saturating_add(1);
        }
        cursor = boundary.bucket(slot.end(), resolution);
    }
    n
}

/// `true` when a finding of this severity keeps its slots out of a bill until a
/// substitute or a clarification stands in.
#[must_use]
pub fn blocks_billing(severity: Severity) -> bool {
    severity >= Severity::Error
}

#[cfg(test)]
mod tests {
    use super::*;
    use metering::QualityFlag;
    use rust_decimal::dec;
    use time::macros::datetime;

    /// `n` quarter-hours of a ramp, so no run of equal values reads as frozen.
    fn qh(from: OffsetDateTime, n: i64) -> Vec<MeterInterval> {
        (0..n)
            .map(|i| {
                MeterInterval::quarter_hour(
                    from + Duration::minutes(15 * i),
                    dec!(1) + Decimal::new(i, 2),
                    QualityFlag::Measured,
                )
                .expect("a quarter-hour")
            })
            .collect()
    }

    /// A window inside one day is validated over that day, and the day's head
    /// and tail outside the window are not reported as gaps.
    #[test]
    fn the_covering_day_does_not_leak_gaps_outside_the_window() {
        let from = datetime!(2026-07-01 10:00 UTC);
        let series = series_of(Sparte::Strom, qh(from, 8)).expect("a series");
        let report = validate_window(
            &series,
            Sparte::Strom,
            from,
            from + Duration::hours(2),
            datetime!(2026-08-01 0:00 UTC),
            None,
        )
        .expect("in the calendar");
        assert!(report.findings().next().is_none(), "{:?}", report.periods);
        assert_eq!(report.coverage_pct(), dec!(100));
        assert_eq!(report.expected_slots(), 8);
    }

    /// A gap inside the window is reported with its own slot count.
    #[test]
    fn a_gap_inside_the_window_counts_its_slots() {
        let from = datetime!(2026-07-01 10:00 UTC);
        let mut ivs = qh(from, 8);
        ivs.drain(2..5);
        let series = series_of(Sparte::Strom, ivs).expect("a series");
        let report = validate_window(
            &series,
            Sparte::Strom,
            from,
            from + Duration::hours(2),
            datetime!(2026-08-01 0:00 UTC),
            None,
        )
        .expect("in the calendar");
        let gaps: Vec<&Finding> = report.findings().filter(|f| f.rule == Rule::Gap).collect();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].reference, Some(dec!(3)));
        assert_eq!(report.coverage_pct(), dec!(62.5));
    }

    /// A window across New Year is validated per year.
    #[test]
    fn a_window_across_new_year_is_split_by_year() {
        let periods = covering_periods(
            DayBoundary::Strom,
            datetime!(2026-12-30 0:00 UTC),
            datetime!(2027-01-02 0:00 UTC),
        )
        .expect("in the calendar");
        assert_eq!(periods.len(), 2);
    }

    /// Two values for one slot of one register are a structural defect, named
    /// by the instant, not a content finding.
    #[test]
    fn a_duplicate_slot_is_a_structural_defect() {
        let from = datetime!(2026-07-01 10:00 UTC);
        let mut ivs = qh(from, 2);
        ivs.push(ivs[0].clone());
        let defect = series_of(Sparte::Strom, ivs).expect_err("not a series");
        assert_eq!(defect.code(), "DUPLICATE");
        assert_eq!(defect.instants(), vec![from]);
    }
}
