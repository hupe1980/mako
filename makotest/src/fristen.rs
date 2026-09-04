//! The Werktag calendar, the acknowledgement clocks, and the published
//! answer-Frist table.
//!
//! Three different questions live here and they have three different answers.
//!
//! 1. **Which date** — `add_werktage` / `next_werktag` / `is_werktag` do
//!    calendar arithmetic and return a date.
//! 2. **Which moment** — `deadline_at_werktage`, `contrl_due_at` and the
//!    `aperak_*` helpers return the instant a clock expires.
//! 3. **Which Frist applies at all** — [`antwort_obligation`] answers that from
//!    the platform's own table, per inbound Prüfidentifikator.
//!
//! Question 3 is the one a harness gets wrong. "A Werktage Frist expires at
//! 17:00 Europe/Berlin" is true of the WiM MSB-Wechsel windows and of nothing
//! else: a GPKE answer window is a **clock time on the *n*-th Werktag after the
//! Übertragungstag** (11:00 / 06:00 / 05:00 / 09:00) — or, for the Ersatz-/
//! Grundversorgung and the LF-Zuordnung, that clock time **on the ÜT itself** —
//! and a GeLi Gas window runs to the **end** of the *n*-th Werktag. Asserting a
//! GPKE deadline with Werktage-plus-cutoff arithmetic is wrong by hours in one
//! direction or six in the other, and the loose direction is silent.
//!
//! The two GPKE shapes share a clock time and land a day apart, so the Werktag
//! count is load-bearing: a window read off `clock_time` alone is a day late for
//! every „am ÜT" obligation.

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use mako_fristen::{self as fristen, HolidayCalendar, antwort};

// ── Parsing helpers ───────────────────────────────────────────────────────────

pub(crate) fn parse_date(s: &str) -> PyResult<time::Date> {
    time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE)
        .map_err(|e| PyValueError::new_err(format!("invalid ISO 8601 date {s:?}: {e}")))
}

pub(crate) fn fmt_date(d: time::Date) -> PyResult<String> {
    d.format(&time::format_description::well_known::Iso8601::DATE)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

fn parse_dt(s: &str) -> PyResult<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .map_err(|e| PyValueError::new_err(format!("invalid RFC 3339 datetime {s:?}: {e}")))
}

fn fmt_dt(t: time::OffsetDateTime) -> PyResult<String> {
    t.format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

// ── Calendar ──────────────────────────────────────────────────────────────────

/// `True` when `date` (ISO 8601) is a Werktag under the BDEW MaKo calendar.
///
/// The calendar is conservative-inclusive: a day observed as a holiday in *any*
/// German state is a non-Werktag, and 24.12. and 31.12. count as holidays
/// (GPKE Teil 1). No Frist is therefore ever computed shorter than the
/// Festlegung requires for some participant.
#[pyfunction]
pub fn is_werktag(date: &str) -> PyResult<bool> {
    let d = parse_date(date)?;
    Ok(fristen::next_werktag(d, HolidayCalendar::BdewMaKo) == d)
}

/// Add `n` Werktage to an ISO 8601 date, returning an ISO 8601 date.
#[pyfunction]
pub fn add_werktage(date: &str, n: u32) -> PyResult<String> {
    let d = parse_date(date)?;
    fmt_date(fristen::add_werktage(d, n, HolidayCalendar::BdewMaKo))
}

/// The next Werktag on or after `date`.
#[pyfunction]
pub fn next_werktag(date: &str) -> PyResult<String> {
    let d = parse_date(date)?;
    fmt_date(fristen::next_werktag(d, HolidayCalendar::BdewMaKo))
}

// ── Instants ──────────────────────────────────────────────────────────────────

/// The instant `werktage` Werktage after `received` expires — 17:00 Berlin.
///
/// The result carries the **Europe/Berlin offset**, not UTC: rendering it as
/// UTC hides the CET/CEST transition that makes it correct.
///
/// This is the WiM MSB-Wechsel shape. For a Frist you are asserting against a
/// real process, prefer [`antwort_deadline`], which picks the shape the
/// Festlegung actually states for that Prüfidentifikator.
#[pyfunction]
pub fn deadline_at_werktage(received: &str, werktage: u32) -> PyResult<String> {
    let t = parse_dt(received)?;
    fmt_dt(fristen::deadline_at_werktage(
        t,
        werktage,
        HolidayCalendar::BdewMaKo,
    ))
}

/// The instant the **end** of the `n`-th Werktag after `received` is reached.
///
/// The GeLi Gas shape: „bis zum Ablauf des n. Werktages nach Eingang". The
/// arrival day does not count (§ 187 Abs. 1 BGB).
#[pyfunction]
pub fn end_of_werktag_after(received: &str, werktage: u32) -> PyResult<String> {
    let t = parse_dt(received)?;
    fmt_dt(fristen::end_of_werktag_after(
        t,
        werktage,
        HolidayCalendar::BdewMaKo,
    ))
}

/// `at` (`"HH:MM"`) Berlin time on the first Werktag strictly after `received`.
///
/// The GPKE shape: „unverzüglich, jedoch spätester ÜZ ist HH:00 Uhr des 1. WT
/// nach dem ÜT".
#[pyfunction]
pub fn next_werktag_at(received: &str, at: &str) -> PyResult<String> {
    let t = parse_dt(received)?;
    let time = parse_clock(at)?;
    fmt_dt(fristen::next_werktag_at(t, time, HolidayCalendar::BdewMaKo))
}

/// Parse `"HH:MM"` or `"HH:MM:SS"` into a clock time.
fn parse_clock(at: &str) -> PyResult<time::Time> {
    let bad = |e: &dyn std::fmt::Display| {
        PyValueError::new_err(format!(
            "clock time must be \"HH:MM[:SS]\", got {at:?}: {e}"
        ))
    };
    let mut parts = at.split(':');
    let mut next = |what: &str| -> PyResult<u8> {
        parts
            .next()
            .ok_or_else(|| bad(&format!("no {what}")))?
            .parse::<u8>()
            .map_err(|e| bad(&e))
    };
    let (h, m) = (next("hour")?, next("minute")?);
    let s = match parts.next() {
        Some(s) => s.parse::<u8>().map_err(|e| bad(&e))?,
        None => 0,
    };
    if parts.next().is_some() {
        return Err(bad(&"too many components"));
    }
    time::Time::from_hms(h, m, s).map_err(|e| bad(&e))
}

/// The RFC 3339 instant of `at` (`"HH:MM[:SS]"`) on `date`, Europe/Berlin.
///
/// A German local time is not a fixed offset from UTC — it is `+01:00` for part
/// of the year and `+02:00` for the rest — so a test clock that carries an
/// offset it was constructed with reports the wrong instant on the other side of
/// a transition. This resolves the offset from the platform's own timezone
/// database for the date in question, which is the only way a test clock and the
/// Fristen it feeds cannot disagree.
///
/// A folded local time (the repeated hour in October) resolves to the **earlier**
/// instant, matching the Frist arithmetic: a Frist is never widened by an
/// accident of the calendar.
#[pyfunction]
pub fn berlin_instant(date: &str, at: &str) -> PyResult<String> {
    let d = parse_date(date)?;
    fmt_dt(fristen::berlin_at(d, parse_clock(at)?))
}

/// `received` plus `hours` wall-clock hours — runs through weekends.
#[pyfunction]
pub fn add_hours(received: &str, hours: u32) -> PyResult<String> {
    let t = parse_dt(received)?;
    fmt_dt(fristen::add_hours(t, hours))
}

/// When the CONTRL for a message received at `received` is due (6 hours).
#[pyfunction]
pub fn contrl_due_at(received: &str) -> PyResult<String> {
    let t = parse_dt(received)?;
    fmt_dt(fristen::contrl_due_at(t))
}

/// When the APERAK for an inbound **Strom** message is due.
///
/// 45 minutes on a weekday; a Saturday arrival is due Sunday noon. This is the
/// acknowledgement clock and it is **not** the business answer window — see
/// [`antwort_obligation`]. Conflating the two is the classic WiM error.
#[pyfunction]
pub fn aperak_strom_due_at(received: &str) -> PyResult<String> {
    let t = parse_dt(received)?;
    fmt_dt(fristen::aperak_strom_due_at(t))
}

/// When the APERAK for an inbound **Gas Folgeprozess** message is due
/// (next Werktag, 12:00).
#[pyfunction]
pub fn aperak_gas_folgeprozess_due_at(received: &str) -> PyResult<String> {
    let t = parse_dt(received)?;
    fmt_dt(fristen::aperak_gas_folgeprozess_due_at(t))
}

/// When the APERAK for an inbound **Gas Initialprozess** message is due
/// (3 Werktage).
#[pyfunction]
pub fn aperak_gas_initialprozess_due_at(received: &str) -> PyResult<String> {
    let t = parse_dt(received)?;
    fmt_dt(fristen::aperak_gas_initialprozess_due_at(t))
}

/// An RFC 3339 instant as EDIFACT DE 2379 format `303` — `CCYYMMDDHHMMZZZ`.
///
/// The zone suffix is part of the value, not decoration: a zone-less `303` is
/// malformed, and BDEW fixes the zone to UTC (`+00`) wherever the format is
/// used. The instant is converted rather than assumed, so a caller holding a
/// Berlin-offset timestamp gets the right minute.
///
/// Bound rather than formatted in Python because this is a wire format a
/// regulator defines, and a second implementation drifts from the builders at
/// the first change — `the_bound_formatter_matches_what_a_builder_emits` pins
/// the two together.
#[pyfunction]
pub fn format_303(instant: &str) -> PyResult<String> {
    let t = parse_dt(instant)?.to_offset(time::UtcOffset::UTC);
    Ok(format!(
        "{:04}{:02}{:02}{:02}{:02}+00",
        t.year(),
        t.month() as u8,
        t.day(),
        t.hour(),
        t.minute()
    ))
}

/// The half-open UTC bounds of one Europe/Berlin calendar day, RFC 3339.
///
/// A German delivery day is a **local** day, and it is 23, 24 or 25 hours long
/// depending on the DST transition — so at quarter-hourly resolution it carries
/// 92, 96 or 100 market time units. A generator that assumed 96 emits four
/// phantom MTUs on the last Sunday in March and drops four real ones in
/// October, and both errors land in the middle of the day where a curve looks
/// plausible.
///
/// The timezone resolution is the platform's own, so a curve and the Fristen
/// it is asserted against cannot disagree about when a day starts.
#[pyfunction]
pub fn berlin_day_bounds(date: &str) -> PyResult<(String, String)> {
    let d = parse_date(date)?;
    let midnight = time::Time::from_hms(0, 0, 0).expect("00:00 is a valid time");
    let next = d
        .next_day()
        .ok_or_else(|| PyValueError::new_err(format!("{date} has no next day")))?;
    Ok((
        fmt_dt(fristen::berlin_at(d, midnight).to_offset(time::UtcOffset::UTC))?,
        fmt_dt(fristen::berlin_at(next, midnight).to_offset(time::UtcOffset::UTC))?,
    ))
}

/// How many market time units of `mtu_minutes` the Europe/Berlin day `date` has.
///
/// 96 on an ordinary day at quarter-hourly resolution, **92** on the short March
/// day and **100** on the long October one. Every consumer that lays values out
/// across a delivery day asks this rather than assuming 96: four phantom MTUs in
/// March and four dropped ones in October land mid-day, where a curve or a
/// Zählerstandsgang still looks plausible and the settlement is quietly wrong.
///
/// Raises `ValueError` when `mtu_minutes` does not divide the day evenly — a
/// resolution that leaves a remainder cannot tile a delivery day, and silently
/// truncating would reintroduce exactly the class of error this exists to stop.
#[pyfunction]
pub fn berlin_mtu_count(date: &str, mtu_minutes: u32) -> PyResult<u32> {
    if mtu_minutes == 0 {
        return Err(PyValueError::new_err("mtu_minutes must be positive"));
    }
    let d = parse_date(date)?;
    let midnight = time::Time::from_hms(0, 0, 0).expect("00:00 is a valid time");
    let next = d
        .next_day()
        .ok_or_else(|| PyValueError::new_err(format!("{date} has no next day")))?;
    let span = (fristen::berlin_at(next, midnight) - fristen::berlin_at(d, midnight))
        .whole_minutes()
        .unsigned_abs();
    // A calendar day is at most 25 hours, so this cannot truncate.
    let minutes = u32::try_from(span).unwrap_or(u32::MAX);
    if minutes % mtu_minutes != 0 {
        return Err(PyValueError::new_err(format!(
            "{mtu_minutes}-minute units do not tile the {}-hour Europe/Berlin day \
             on {date}",
            minutes / 60
        )));
    }
    Ok(minutes / mtu_minutes)
}

// ── The answer-Frist table ────────────────────────────────────────────────────

/// One published answer obligation: who owes what, by when, and where it says so.
///
/// Bound rather than re-tabulated because four services already share this
/// table — `makod` registers the deadline, `processd` sizes its operator queue,
/// `obsd` raises the breach and `agentd` classifies it. A harness with its own
/// copy would disagree with all four.
#[pyclass(get_all, frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct AntwortObligation {
    /// The **inbound** Prüfidentifikator that starts the clock. Never an answer
    /// PID: an answer discharges a Frist rather than starting one.
    pub trigger_pid: u32,
    /// Process name, as the Anwendungsübersicht names it.
    pub name: String,
    /// The Marktrolle that owes the answer.
    pub answered_by: String,
    /// The Bestätigung PID for this process.
    pub bestaetigung_pid: u32,
    /// The Ablehnung PID for this process.
    pub ablehnung_pid: u32,
    /// The Entscheidungsbaum that decides the answer, where one is published.
    pub ebd: Option<String>,
    /// `"gpke"`, `"geli-gas"`, `"wim"` or `"wim-gas"`.
    pub family: String,
    /// How the window is measured. One of:
    ///
    /// | `shape` | Window | `werktage` | `clock_time` |
    /// |---|---|---|---|
    /// | `"werktag_at"` | that clock time on the *n*-th Werktag after the ÜT | *n* | set |
    /// | `"same_day_at"` | that clock time **on the ÜT itself** | `0` | set |
    ///
    /// `"same_day_at"` rolls to the same clock time on the next Werktag when the
    /// ÜT is not a Werktag or the cut-off has already passed at arrival — the
    /// literal reading would otherwise place the deadline behind the message.
    /// | `"same_day"` | the anchor's own day, no cut-off stated | `0` | `None` |
    /// | `"end_of_werktag"` | the **end** of the *n*-th Werktag | *n* | `None` |
    /// | `"werktage_at_cutoff"` | 17:00 Europe/Berlin on the *n*-th Werktag | *n* | `None` |
    ///
    /// Rendering a window from `clock_time` alone is a day out on
    /// `"same_day_at"`: „15:00 Uhr **am ÜT**" and „15:00 Uhr des 1. WT nach dem
    /// ÜT" are different obligations, and `werktage` is what separates them.
    pub shape: String,
    /// Werktage the window runs for — `0` for the two same-day shapes. Always
    /// present, so a consumer formats the window from `shape` and this pair
    /// without a fallback branch.
    pub werktage: Option<u32>,
    /// `"HH:MM"` Berlin, for the two wall-clock shapes. `None` for the others.
    pub clock_time: Option<String>,
    /// Citation, for the audit trail.
    pub source: String,
}

#[pymethods]
impl AntwortObligation {
    /// The instant the answer is due for a message that arrived at `received`.
    fn due_at(&self, received: &str) -> PyResult<String> {
        let t = parse_dt(received)?;
        let o = antwort::antwort_obligation(self.trigger_pid).ok_or_else(|| {
            PyRuntimeError::new_err(format!("obligation for {} vanished", self.trigger_pid))
        })?;
        fmt_dt(o.frist.due_at(t, HolidayCalendar::BdewMaKo))
    }

    /// The window in words — `"15:00 on the ÜT"`, `"11:00 on the 1. WT"`,
    /// `"4 WT"`.
    ///
    /// Formatted from `shape` and `werktage` together, never from `clock_time`
    /// alone: „15:00 Uhr am ÜT" and „15:00 Uhr des 1. WT nach dem ÜT" are a day
    /// apart and differ only in the Werktag count.
    #[getter]
    fn window(&self) -> String {
        match (&self.clock_time, self.werktage) {
            (Some(t), Some(0)) => format!("{t} on the ÜT"),
            (Some(t), Some(n)) => format!("{t} on the {n}. WT"),
            (None, Some(0)) => "on the ÜT".to_owned(),
            (None, Some(n)) => format!("{n} WT"),
            // `werktage` is present on every published obligation — pinned by
            // `every_obligation_renders_its_window`.
            (Some(t), None) => t.clone(),
            (None, None) => "?".to_owned(),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "AntwortObligation({} {} — {} answers within {}, {})",
            self.trigger_pid,
            self.name,
            self.answered_by,
            self.window(),
            self.family
        )
    }
}

fn convert(o: &antwort::AntwortObligation) -> AntwortObligation {
    let (shape, werktage, clock_time) = match o.frist {
        antwort::FristShape::WerktagAt { werktage, at } => (
            "werktag_at",
            Some(werktage),
            Some(format!("{:02}:{:02}", at.hour(), at.minute())),
        ),
        antwort::FristShape::EndOfWerktag(n) => ("end_of_werktag", Some(n), None),
        antwort::FristShape::WerktageAtCutoff(n) => ("werktage_at_cutoff", Some(n), None),
        // „Spätester ÜZ ist HH:MM Uhr **am ÜT**" — the same wall clock as
        // `werktag_at`, but on the arrival day itself, so `werktage` is 0
        // rather than absent.
        antwort::FristShape::SameDayAt(at) => (
            "same_day_at",
            Some(0),
            Some(format!("{:02}:{:02}", at.hour(), at.minute())),
        ),
        // „Am selben Tag wie …" — the anchor's own day with no wall-clock time
        // attached, so `werktage` is 0 like `same_day_at` but `clock_time` is
        // absent rather than a cut-off the document does not state.
        antwort::FristShape::SameDay => ("same_day", Some(0), None),
    };
    AntwortObligation {
        trigger_pid: o.trigger_pid,
        name: o.name.to_owned(),
        answered_by: o.answered_by.to_owned(),
        bestaetigung_pid: o.antwort_pids.0,
        ablehnung_pid: o.antwort_pids.1,
        ebd: o.ebd.map(str::to_owned),
        family: o.family.as_str().to_owned(),
        shape: shape.to_owned(),
        werktage,
        clock_time,
        source: o.source.to_owned(),
    }
}

/// The published answer obligation for an inbound Prüfidentifikator.
///
/// `None` means **unknown** — no Festlegung this codebase has read quantifies
/// the window. It never means unbounded, and a test that treats it that way is
/// asserting the absence of an obligation rather than its content.
#[pyfunction]
pub fn antwort_obligation(trigger_pid: u32) -> Option<AntwortObligation> {
    antwort::antwort_obligation(trigger_pid).map(convert)
}

/// Every published obligation, across GPKE, GeLi Gas, WiM Strom and WiM Gas.
///
/// Parametrize over this to assert a platform registers the right deadline for
/// every process it claims to run — the list grows when a Festlegung is read,
/// and a test written against it grows with it.
#[pyfunction]
pub fn antwort_obligations() -> Vec<AntwortObligation> {
    antwort::all().map(convert).collect()
}

/// The instant an answer to `trigger_pid` is due, or `None` when unquantified.
///
/// This picks the shape the Festlegung states — a clock time on the next
/// Werktag for GPKE, the end of the *n*-th Werktag for GeLi Gas, the 17:00
/// cut-off for WiM. Prefer it to [`deadline_at_werktage`] whenever the process
/// is known: the Werktage form is right for one family out of four.
#[pyfunction]
pub fn antwort_deadline(trigger_pid: u32, received: &str) -> PyResult<Option<String>> {
    let t = parse_dt(received)?;
    antwort::antwort_deadline(trigger_pid, t)
        .map(fmt_dt)
        .transpose()
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(is_werktag, m)?)?;
    m.add_function(wrap_pyfunction!(add_werktage, m)?)?;
    m.add_function(wrap_pyfunction!(next_werktag, m)?)?;

    m.add_function(wrap_pyfunction!(deadline_at_werktage, m)?)?;
    m.add_function(wrap_pyfunction!(end_of_werktag_after, m)?)?;
    m.add_function(wrap_pyfunction!(next_werktag_at, m)?)?;
    m.add_function(wrap_pyfunction!(add_hours, m)?)?;
    m.add_function(wrap_pyfunction!(contrl_due_at, m)?)?;
    m.add_function(wrap_pyfunction!(aperak_strom_due_at, m)?)?;
    m.add_function(wrap_pyfunction!(aperak_gas_folgeprozess_due_at, m)?)?;
    m.add_function(wrap_pyfunction!(aperak_gas_initialprozess_due_at, m)?)?;

    m.add_function(wrap_pyfunction!(berlin_day_bounds, m)?)?;
    m.add_function(wrap_pyfunction!(format_303, m)?)?;
    m.add_function(wrap_pyfunction!(berlin_instant, m)?)?;
    m.add_function(wrap_pyfunction!(berlin_mtu_count, m)?)?;
    m.add_function(wrap_pyfunction!(antwort_obligation, m)?)?;
    m.add_function(wrap_pyfunction!(antwort_obligations, m)?)?;
    m.add_function(wrap_pyfunction!(antwort_deadline, m)?)?;
    m.add_class::<AntwortObligation>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The families must produce different instants for the same arrival, or
    /// binding the table adds nothing over a Werktage count.
    #[test]
    fn the_families_are_genuinely_different_instants() {
        let received = "2026-03-02T09:00:00Z"; // a Monday
        // GPKE Anmeldung — 11:00 on the 1. WT after the ÜT.
        let gpke = antwort_deadline(55_001, received).unwrap().unwrap();
        // GeLi Gas Anmeldung — end of the 4th Werktag.
        let gas = antwort_deadline(44_001, received).unwrap().unwrap();
        // WiM Kündigung — 17:00 on the 3rd Werktag.
        let wim = antwort_deadline(55_039, received).unwrap().unwrap();

        assert!(gpke.starts_with("2026-03-03T11:00:00"), "{gpke}");
        assert!(wim.starts_with("2026-03-05T17:00:00"), "{wim}");
        assert!(gas > wim, "4 WT to end-of-day outlasts 3 WT to 17:00");

        // The trap this table exists to prevent: the Werktage form applied to a
        // GPKE PID is a different instant entirely.
        assert_ne!(gpke, deadline_at_werktage(received, 1).unwrap());
    }

    /// The two DST days are the whole reason this exists.
    #[test]
    fn a_berlin_day_is_23_24_or_25_hours() {
        for (date, hours) in [
            ("2026-03-29", 23), // CET → CEST, the short day
            ("2026-06-21", 24),
            ("2026-10-25", 25), // CEST → CET, the long day
        ] {
            let (start, end) = berlin_day_bounds(date).unwrap();
            let parse = |s: &str| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                    .unwrap()
            };
            let span = parse(&end) - parse(&start);
            assert_eq!(span.whole_hours(), hours, "{date}: {start} .. {end}");
        }
    }

    /// The whole reason `berlin_instant` exists: the offset is a property of the
    /// date, not of whatever the caller happened to construct a clock with.
    #[test]
    fn a_berlin_instant_carries_the_offset_of_its_own_date() {
        assert_eq!(
            berlin_instant("2026-03-27", "09:00").unwrap(),
            "2026-03-27T09:00:00+01:00"
        );
        assert_eq!(
            berlin_instant("2026-03-31", "09:00").unwrap(),
            "2026-03-31T09:00:00+02:00"
        );
        assert_eq!(
            berlin_instant("2026-11-03", "09:00:30").unwrap(),
            "2026-11-03T09:00:30+01:00"
        );
        assert!(berlin_instant("2026-11-03", "0900").is_err());
        assert!(berlin_instant("2026-11-03", "25:00").is_err());
    }

    #[test]
    fn a_delivery_day_is_measured_in_its_own_market_time_units() {
        for (date, quarters, hours) in [
            ("2026-03-29", 92, 23),
            ("2026-06-21", 96, 24),
            ("2026-10-25", 100, 25),
        ] {
            assert_eq!(berlin_mtu_count(date, 15).unwrap(), quarters, "{date}");
            assert_eq!(berlin_mtu_count(date, 60).unwrap(), hours, "{date}");
        }
        // A resolution that leaves a remainder cannot tile the day; truncating
        // would reintroduce the phantom-MTU error this guards against.
        assert!(berlin_mtu_count("2026-06-21", 7).is_err());
        assert!(berlin_mtu_count("2026-06-21", 0).is_err());
    }

    /// `add_werktage` has to compose, or a Frist computed in two steps differs
    /// from the same Frist computed in one — the shape a `FrozenClock` produces
    /// when a test advances it repeatedly.
    #[test]
    fn advancing_werktage_composes() {
        for start in ["2026-12-22", "2026-03-27", "2026-01-02"] {
            for (m, n) in [(1u32, 1u32), (2, 3), (0, 4), (5, 0)] {
                let one_step = add_werktage(start, m + n).unwrap();
                let two_steps = add_werktage(&add_werktage(start, m).unwrap(), n).unwrap();
                assert_eq!(one_step, two_steps, "{start}: {m}+{n}");
            }
        }
    }

    /// The bound formatter and the builders must agree, or a test asserting a
    /// measurement period compares two different renderings of one instant.
    #[test]
    fn the_bound_formatter_matches_what_a_builder_emits() {
        // The 25-hour October day starts at 22:00 UTC the previous day.
        assert_eq!(
            format_303("2026-10-24T22:00:00Z").unwrap(),
            "202610242200+00"
        );
        // A Berlin-offset instant converts rather than being taken at face value.
        assert_eq!(
            format_303("2026-10-25T00:00:00+02:00").unwrap(),
            "202610242200+00"
        );
        assert!(format_303("not a timestamp").is_err());
    }

    #[test]
    fn an_unquantified_pid_reports_unknown_rather_than_a_default() {
        assert!(antwort_obligation(44_020).is_none());
        assert!(
            antwort_deadline(44_020, "2026-03-02T09:00:00Z")
                .unwrap()
                .is_none()
        );
    }

    /// Every obligation names a Werktag count, and only the `werktag_at` shape
    /// adds a clock time — so a consumer can format the window from the shape
    /// alone, without a fallback branch.
    #[test]
    fn every_obligation_renders_its_window() {
        let all = antwort_obligations();
        assert!(
            all.len() >= 20,
            "expected every published family, got {}",
            all.len()
        );
        for o in &all {
            assert!(
                o.werktage.is_some(),
                "{}: every window is measured in Werktage",
                o.trigger_pid
            );
            // The two wall-clock shapes carry a time; the day-granular ones
            // must not, or a queue would expire hours before the Frist does.
            assert_eq!(
                o.clock_time.is_some(),
                matches!(o.shape.as_str(), "werktag_at" | "same_day_at"),
                "{}: only the wall-clock shapes carry a clock time",
                o.trigger_pid
            );
            assert!(!o.source.is_empty(), "{} has no Fundstelle", o.trigger_pid);
        }
    }

    /// „15:00 Uhr am ÜT" and „15:00 Uhr des 1. WT nach dem ÜT" are a day apart
    /// and share a clock time, so the rendered window has to come from the
    /// Werktag count as well.
    #[test]
    fn a_same_day_window_is_not_rendered_as_the_next_werktag() {
        let same_day = antwort_obligation(55_013).expect("Ersatz-/Grundversorgung");
        assert_eq!(same_day.shape, "same_day_at");
        assert_eq!(same_day.window(), "15:00 on the ÜT");
        // …and the instant agrees: the ÜT itself, not the day after.
        assert!(
            same_day
                .due_at("2026-03-02T09:00:00Z")
                .unwrap()
                .starts_with("2026-03-02T15:00:00")
        );

        let next_werktag = antwort_obligation(55_001).expect("Anmeldung");
        assert_eq!(next_werktag.window(), "11:00 on the 1. WT");
        assert_eq!(
            antwort_obligation(44_001)
                .expect("GeLi Gas Anmeldung")
                .window(),
            "4 WT"
        );
    }
}
