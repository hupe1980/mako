//! PyO3 bindings backing the `makotest` Python toolkit.
//!
//! Only the concerns a *regulator* defines in a table are bound here — BDEW
//! identifier check digits, the Werktag/Feiertag calendar, and AHB/MIG message
//! validation. Everything shaped by test ergonomics (price curves, load
//! profiles, counterparty behaviour, fixtures) stays in Python.
//!
//! The reason for the split is drift: a second implementation of the AHB rule
//! tables or the BDEW check digit would disagree with production at the first
//! Formatumstellung, and a harness that disagrees with the system under test
//! about validity is worse than no harness.

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyModule;

use edi_energy::builders::{InterchangeBuilder, MsconsBuilder, UtilmdBuilder};
use edi_energy::{EdiEnergyMessage, ObjectType, Platform, Pruefidentifikator, Release};
use mako_engine::fristen::{self, HolidayCalendar};
use rubo4e::identifiers::{MaloId, MeloId};

// ── Identifiers ───────────────────────────────────────────────────────────────

/// `True` when `value` is a check-digit-valid 11-digit Marktlokations-ID.
#[pyfunction]
fn malo_is_valid(value: &str) -> bool {
    MaloId::new(value).is_ok()
}

/// `True` when `value` is a well-formed 33-character Messlokations-ID.
#[pyfunction]
fn melo_is_valid(value: &str) -> bool {
    MeloId::new(value).is_ok()
}

/// The BDEW check digit for a 10-digit MaLo base.
///
/// Raises `ValueError` when `base` is not exactly 10 digits.
#[pyfunction]
fn malo_check_digit(base: &str) -> PyResult<u8> {
    MaloId::check_digit(base).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Complete a 10-digit base into a valid 11-digit MaLo-ID.
///
/// This is the generator every consumer would otherwise hand-roll incorrectly:
/// a random 11-digit string is almost never a valid MaLo, so tests that invent
/// one silently exercise the rejection path instead of the happy path.
#[pyfunction]
fn malo_from_base(base: &str) -> PyResult<String> {
    MaloId::from_base(base)
        .map(|id| id.as_ref().to_owned())
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

// ── Fristen ───────────────────────────────────────────────────────────────────

fn parse_date(s: &str) -> PyResult<time::Date> {
    time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE)
        .map_err(|e| PyValueError::new_err(format!("invalid ISO 8601 date {s:?}: {e}")))
}

/// `True` when `date` (ISO 8601) is a Werktag under the BDEW MaKo calendar.
///
/// BDEW treats a day observed as a holiday in *any* German state as a
/// non-Werktag, so no Frist is ever computed shorter than the AHB requires for
/// any participant.
#[pyfunction]
fn is_werktag(date: &str) -> PyResult<bool> {
    let d = parse_date(date)?;
    Ok(fristen::next_werktag(d, HolidayCalendar::BdewMaKo) == d)
}

/// Add `n` Werktage to an ISO 8601 date, returning an ISO 8601 date.
#[pyfunction]
fn add_werktage(date: &str, n: u32) -> PyResult<String> {
    let d = parse_date(date)?;
    let out = fristen::add_werktage(d, n, HolidayCalendar::BdewMaKo);
    out.format(&time::format_description::well_known::Iso8601::DATE)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

/// The next Werktag on or after `date`.
#[pyfunction]
fn next_werktag(date: &str) -> PyResult<String> {
    let d = parse_date(date)?;
    let out = fristen::next_werktag(d, HolidayCalendar::BdewMaKo);
    out.format(&time::format_description::well_known::Iso8601::DATE)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

// ── EDIFACT validation ────────────────────────────────────────────────────────

/// One validation finding.
#[pyclass(get_all, frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct Finding {
    /// `"error"` or `"warning"`.
    pub severity: String,
    /// The AHB/MIG/semantic rule that fired, e.g. `"SEM-MSCONS-LOCATION-FORMAT"`.
    pub rule_id: Option<String>,
    pub segment: Option<String>,
    pub message: String,
}

#[pymethods]
impl Finding {
    fn __repr__(&self) -> String {
        let rule = self.rule_id.as_deref().unwrap_or("-");
        format!("Finding({} {}: {})", self.severity, rule, self.message)
    }
}

/// The outcome of parsing and validating one EDIFACT interchange.
#[pyclass(get_all, frozen)]
pub struct ValidationReport {
    /// `True` when the interchange carries no error-severity findings.
    pub is_valid: bool,
    /// Prüfidentifikator read from BGM, when the message carries one.
    pub pruefidentifikator: Option<u32>,
    pub message_type: Option<String>,
    pub findings: Vec<Finding>,
}

#[pymethods]
impl ValidationReport {
    fn __repr__(&self) -> String {
        format!(
            "ValidationReport(is_valid={}, pid={:?}, findings={})",
            if self.is_valid { "True" } else { "False" },
            self.pruefidentifikator,
            self.findings.len()
        )
    }

    /// Findings whose `rule_id` starts with `prefix`.
    fn by_rule(&self, prefix: &str) -> Vec<Finding> {
        self.findings
            .iter()
            .filter(|f| f.rule_id.as_deref().is_some_and(|r| r.starts_with(prefix)))
            .cloned()
            .collect()
    }
}

/// Parse and fully validate an EDIFACT interchange (MIG + AHB + semantic rules).
///
/// `reference_date` (ISO 8601) selects the BDEW format version in force, exactly
/// as the production ingest path does — validating a 2026-10-01 message against
/// the 2025 profile is a different question and would give a different answer.
/// Defaults to today when omitted.
///
/// Raises `ValueError` when the bytes are not parseable as EDIFACT at all;
/// rule violations come back as findings, not exceptions.
#[pyfunction]
#[pyo3(signature = (raw, reference_date=None))]
fn validate_edifact(raw: &[u8], reference_date: Option<&str>) -> PyResult<ValidationReport> {
    let msg = Platform::with_all_profiles()
        .parse(raw)
        .map_err(|e| PyValueError::new_err(format!("EDIFACT parse failed: {e}")))?;

    let on = match reference_date {
        Some(s) => parse_date(s)?,
        None => time::OffsetDateTime::now_utc().date(),
    };

    let report = msg
        .validate_on_date(on)
        .map_err(|e| PyRuntimeError::new_err(format!("validation failed: {e}")))?;

    let findings = report
        .iter_issues()
        .map(|f| Finding {
            severity: format!("{:?}", f.severity).to_lowercase(),
            rule_id: f.rule_id.clone(),
            segment: f.segment_tag.clone(),
            message: f.message.clone(),
        })
        .collect();

    Ok(ValidationReport {
        is_valid: report.is_valid(),
        pruefidentifikator: msg.detect_pruefidentifikator().ok().map(|p| p.as_u32()),
        message_type: msg
            .try_message_type()
            .map(|t| format!("{t:?}").to_uppercase()),
        findings,
    })
}

// ── EDIFACT builders ──────────────────────────────────────────────────────────
//
// The Rust builders are typestate-driven (`Set`/`Unset` phantom types), which
// does not translate to Python. Python collects the parameters and hands them
// over in one call; the typestate is satisfied internally.

/// One SG4/IDE transaction of a UTILMD message.
#[pyclass(get_all, set_all, from_py_object)]
#[derive(Clone, Default)]
pub struct UtilmdTransaction {
    /// `"malo"`, `"melo"`, `"nelo"`, `"tranche"`, `"tr"` or `"sr"`.
    pub object_type: String,
    pub object_id: String,
    /// `(qualifier, YYYYMMDD)` pairs, e.g. `("163", "20261101")` for delivery start.
    pub process_dates: Vec<(String, String)>,
    /// `(qualifier, value)` pairs, e.g. `("Z13", "55001")`.
    pub references: Vec<(String, String)>,
}

#[pymethods]
impl UtilmdTransaction {
    #[new]
    #[pyo3(signature = (object_type, object_id, process_dates=None, references=None))]
    fn new(
        object_type: String,
        object_id: String,
        process_dates: Option<Vec<(String, String)>>,
        references: Option<Vec<(String, String)>>,
    ) -> Self {
        Self {
            object_type,
            object_id,
            process_dates: process_dates.unwrap_or_default(),
            references: references.unwrap_or_default(),
        }
    }
}

fn object_type_from_str(s: &str) -> PyResult<ObjectType> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "malo" | "marktlokation" => ObjectType::Marktlokation,
        "melo" | "messlokation" => ObjectType::Messlokation,
        "nelo" | "netzlokation" => ObjectType::Netzlokation,
        "tranche" => ObjectType::Tranche,
        "tr" | "technische_ressource" => ObjectType::TechnischeRessource,
        "sr" | "steuerbare_ressource" => ObjectType::SteuerungRessource,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown object_type {other:?} — expected one of \
                 malo, melo, nelo, tranche, tr, sr"
            )));
        }
    })
}

fn pid(value: u32) -> PyResult<Pruefidentifikator> {
    Pruefidentifikator::new(value).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Build a UTILMD interchange and return the rendered EDIFACT bytes.
///
/// `release` is the wire release code (e.g. `"S2.2"`). The result is **not**
/// auto-validated: build and validation are separate steps so a test can
/// deliberately construct an invalid message and assert that the rule fires.
#[pyfunction]
#[pyo3(signature = (
    pruefidentifikator, sender, receiver, release="S2.2", message_ref="1",
    document_date=None, document_code="E01", transactions=None
))]
#[allow(clippy::too_many_arguments)]
fn build_utilmd(
    pruefidentifikator: u32,
    sender: &str,
    receiver: &str,
    release: &str,
    message_ref: &str,
    document_date: Option<&str>,
    document_code: &str,
    transactions: Option<Vec<UtilmdTransaction>>,
) -> PyResult<Vec<u8>> {
    let mut b = UtilmdBuilder::new(Release::new(release))
        .pruefidentifikator(pid(pruefidentifikator)?)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(document_code);
    if let Some(d) = document_date {
        b = b.document_date(d);
    }

    for tx in transactions.unwrap_or_default() {
        let mut t = b.transaction(object_type_from_str(&tx.object_type)?, &tx.object_id);
        for (q, d) in &tx.process_dates {
            t = t.process_date(q.as_str(), d.as_str());
        }
        for (q, r) in &tx.references {
            t = t.reference(q.as_str(), r.as_str());
        }
        b = t.done();
    }

    b.build()
        .map_err(|e| PyValueError::new_err(format!("UTILMD build failed: {e}")))?
        .serialize()
        .map_err(|e| PyRuntimeError::new_err(format!("UTILMD serialize failed: {e}")))
}

/// Build an MSCONS interchange and return the rendered EDIFACT bytes.
///
/// `quantities` are `(qualifier, value, unit)` triples, e.g.
/// `("220", "1234.567", "KWH")`.
#[pyfunction]
#[pyo3(signature = (
    pruefidentifikator, sender, receiver, metering_point, quantities,
    release="2.5", message_ref="1", document_date=None, obis=None
))]
#[allow(clippy::too_many_arguments)]
fn build_mscons(
    pruefidentifikator: u32,
    sender: &str,
    receiver: &str,
    metering_point: &str,
    quantities: Vec<(String, String, String)>,
    release: &str,
    message_ref: &str,
    document_date: Option<&str>,
    obis: Option<&str>,
) -> PyResult<Vec<u8>> {
    let mut b = MsconsBuilder::new(Release::new(release))
        .pruefidentifikator(pid(pruefidentifikator)?)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);
    if let Some(d) = document_date {
        b = b.document_date(d);
    }

    let mut mp = b.metering_point(metering_point);
    if let Some(code) = obis {
        let parsed = rubo4e::identifiers::ObisCode::new(code)
            .map_err(|e| PyValueError::new_err(format!("invalid OBIS code {code:?}: {e}")))?;
        mp = mp.obis(parsed);
    }
    for (q, v, u) in &quantities {
        mp = mp.quantity(q.as_str(), v.as_str(), u.as_str());
    }

    mp.done()
        .build()
        .map_err(|e| PyValueError::new_err(format!("MSCONS build failed: {e}")))?
        .serialize()
        .map_err(|e| PyRuntimeError::new_err(format!("MSCONS serialize failed: {e}")))
}

/// Wrap one or more messages in a UNB/UNZ interchange envelope.
///
/// A message (`UNH`…`UNT`) is not sendable on its own — the wire unit a market
/// partner receives over AS4 is the interchange. `build_utilmd` and
/// `build_mscons` return messages, so anything destined for a real endpoint
/// must pass through here.
///
/// `date`/`time` are `YYMMDD`/`HHMM` and default to zeros: the timestamp is a
/// parameter rather than a clock read so building stays deterministic.
#[pyfunction]
#[pyo3(signature = (sender, receiver, dar, messages, date="000000", time="0000"))]
fn build_interchange(
    sender: &str,
    receiver: &str,
    dar: &str,
    messages: Vec<Vec<u8>>,
    date: &str,
    time: &str,
) -> PyResult<Vec<u8>> {
    let mut b = InterchangeBuilder::new(sender, receiver, dar).transmission(date, time);
    for m in messages {
        b = b.message(m);
    }
    b.build()
        .map_err(|e| PyRuntimeError::new_err(format!("interchange build failed: {e}")))
}

// ── Module ────────────────────────────────────────────────────────────────────

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add(
        "__doc__",
        "Rust core of makotest — BDEW identifiers, Fristen, EDIFACT.",
    )?;

    m.add_function(wrap_pyfunction!(malo_is_valid, m)?)?;
    m.add_function(wrap_pyfunction!(melo_is_valid, m)?)?;
    m.add_function(wrap_pyfunction!(malo_check_digit, m)?)?;
    m.add_function(wrap_pyfunction!(malo_from_base, m)?)?;

    m.add_function(wrap_pyfunction!(is_werktag, m)?)?;
    m.add_function(wrap_pyfunction!(add_werktage, m)?)?;
    m.add_function(wrap_pyfunction!(next_werktag, m)?)?;

    m.add_function(wrap_pyfunction!(validate_edifact, m)?)?;
    m.add_function(wrap_pyfunction!(build_utilmd, m)?)?;
    m.add_function(wrap_pyfunction!(build_mscons, m)?)?;
    m.add_function(wrap_pyfunction!(build_interchange, m)?)?;
    m.add_class::<UtilmdTransaction>()?;
    m.add_class::<Finding>()?;
    m.add_class::<ValidationReport>()?;
    Ok(())
}
