//! EDIFACT construction and validation, through the platform's own engine.
//!
//! Building and validating are deliberately **separate** steps: a test must be
//! able to construct a knowingly-invalid message and assert that the rule it
//! expects is the rule that rejects it.
//!
//! ## The format version is an argument, never a default
//!
//! A message valid under FV2025-10-01 can be invalid under FV2026-10-01, and
//! the release code on the wire has to match the profile it is validated
//! against. Both `build_*` and [`validate_edifact`] therefore take the date the
//! message would really be sent, and the builders resolve the release from it.
//! Defaulting either to "today" would make a suite's meaning change on a
//! Formatumstellung it never mentions.

use std::sync::OnceLock;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyModule;

use edi_energy::builders::{
    AperakBuilder, ContrlBuilder, InterchangeBuilder, MsconsBuilder, UtilmdBuilder,
};
use edi_energy::utilmd_codes::{AntwortStatus, Transaktionsgrund};
use edi_energy::{EdiEnergyMessage, Lokationstyp, MessageType, Platform, Pruefidentifikator};

use crate::fristen::{fmt_date, parse_date};
use crate::pids::resolve_release;

/// The profile registry is a few megabytes of static rule packs and wiring up
/// its directory validators is not free, so it is built once per process.
fn platform() -> &'static Platform {
    static PLATFORM: OnceLock<Platform> = OnceLock::new();
    PLATFORM.get_or_init(|| {
        let p = Platform::with_all_profiles();
        p.warm_up();
        p
    })
}

// ── Findings ──────────────────────────────────────────────────────────────────

/// One validation finding.
#[pyclass(get_all, frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct Finding {
    /// `"critical"`, `"error"`, `"warning"` or `"info"`.
    pub severity: String,
    /// The rule that fired, e.g. `"SEM-MSCONS-LOCATION-FORMAT"`.
    pub rule_id: Option<String>,
    /// Which validation layer produced it: `"parse"`, `"directory"`, `"mig"`,
    /// `"ahb"`, `"semantic"` or `"custom"`.
    ///
    /// This is the distinction between a **syntax** failure a counterparty
    /// answers with a CONTRL and an **application** failure it answers with an
    /// APERAK, so a simulator reads it rather than guessing from the text.
    pub rule_origin: Option<String>,
    /// Stable library error code, where the rule carries one.
    pub error_code: Option<String>,
    /// Segment tag, e.g. `"LOC"`.
    pub segment: Option<String>,
    /// Segment group the issue sits in, e.g. `"SG4"`.
    pub segment_group: Option<String>,
    /// 0-based data-element index inside the segment, when known.
    pub element: Option<u8>,
    /// 0-based component index inside the element, when known.
    pub component: Option<u8>,
    /// Remediation hint, where the rule offers one.
    pub suggestion: Option<String>,
    pub message: String,
}

#[pymethods]
impl Finding {
    /// `"SG4/LOC[2].0"` — the position the finding points at, or `None`.
    ///
    /// Segment group, segment tag, 0-based element index and 0-based component
    /// index, each present only when the rule reported it. Stable enough to
    /// assert on: it is assembled from the report's own fields rather than
    /// parsed out of the message text.
    #[getter]
    fn position(&self) -> Option<String> {
        let seg = self.segment.as_deref()?;
        let mut out = match &self.segment_group {
            Some(g) => format!("{g}/{seg}"),
            None => seg.to_owned(),
        };
        if let Some(e) = self.element {
            out.push_str(&format!("[{e}]"));
            if let Some(c) = self.component {
                out.push_str(&format!(".{c}"));
            }
        }
        Some(out)
    }

    /// `True` for a finding that makes the message invalid.
    #[getter]
    fn is_error(&self) -> bool {
        matches!(self.severity.as_str(), "error" | "critical")
    }

    fn __repr__(&self) -> String {
        format!(
            "Finding({} {}: {})",
            self.severity,
            self.rule_id.as_deref().unwrap_or("-"),
            self.message
        )
    }
}

// ── Per-message report ────────────────────────────────────────────────────────

/// The validation outcome for one message inside an interchange.
#[pyclass(get_all, frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct MessageReport {
    /// 0-based position in the interchange.
    pub index: usize,
    /// UNH message reference.
    pub message_ref: String,
    pub pruefidentifikator: Option<u32>,
    pub message_type: Option<String>,
    /// Wire release read off the UNH, e.g. `"S2.1"`.
    pub release: Option<String>,
    /// `True` when this message carries no error-severity finding.
    pub is_valid: bool,
    /// `False` when the profile set has **no AHB rules** for this PID, so
    /// `is_valid` was decided vacuously — the message "passed" because nothing
    /// was checked. An assertion over such a message proves nothing.
    pub rules_applied: bool,
    pub findings: Vec<Finding>,
}

#[pymethods]
impl MessageReport {
    /// Findings whose `rule_id` starts with `prefix`.
    fn by_rule(&self, prefix: &str) -> Vec<Finding> {
        self.findings
            .iter()
            .filter(|f| f.rule_id.as_deref().is_some_and(|r| r.starts_with(prefix)))
            .cloned()
            .collect()
    }

    /// Only the findings that make the message invalid.
    #[getter]
    fn errors(&self) -> Vec<Finding> {
        self.findings
            .iter()
            .filter(|f| f.is_error())
            .cloned()
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "MessageReport(#{} {:?} pid={:?} valid={} rules_applied={} findings={})",
            self.index,
            self.message_type.as_deref().unwrap_or("?"),
            self.pruefidentifikator,
            self.is_valid,
            self.rules_applied,
            self.findings.len()
        )
    }
}

// ── Interchange envelope ──────────────────────────────────────────────────────

/// The UNB/UNZ envelope, as the receiving platform reads it.
///
/// Worth asserting on: the MP-IDs in the envelope must match the NAD segments
/// inside (BDEW "Identifikatoren" §2.13), the qualifier is derived from the ID
/// rather than chosen, and `test_indicator` decides whether a real counterparty
/// would process the interchange at all.
#[pyclass(get_all, frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct Envelope {
    pub sender_id: String,
    /// UNB DE0007 for the sender — `"500"` BDEW, `"502"` DVGW, `"14"` GLN.
    pub sender_qualifier: String,
    pub receiver_id: String,
    pub receiver_qualifier: String,
    /// UNB DE0020 Datenaustauschreferenz, mirrored in UNZ DE0036.
    pub control_ref: String,
    /// UNB transmission date, ISO 8601, when the header carries a parseable one.
    pub transmission_date: Option<String>,
    /// UNB DE0035 — `True` marks the interchange as a test transmission.
    pub test_indicator: bool,
    /// Messages actually present.
    pub message_count: usize,
    /// Messages the UNZ trailer declares.
    pub declared_message_count: usize,
    /// `True` when the count matches the trailer and UNB/UNZ references agree.
    pub is_structurally_valid: bool,
}

#[pymethods]
impl Envelope {
    fn __repr__(&self) -> String {
        format!(
            "Envelope({}:{} -> {}:{}, ref={}, messages={})",
            self.sender_id,
            self.sender_qualifier,
            self.receiver_id,
            self.receiver_qualifier,
            self.control_ref,
            self.message_count
        )
    }
}

// ── Interchange report ────────────────────────────────────────────────────────

/// The outcome of parsing and validating one interchange.
///
/// The convenience accessors (`pruefidentifikator`, `message_type`, `release`,
/// `findings`, `rules_applied`) speak for the single message an interchange
/// usually carries and raise on a multi-message one, where a single answer
/// would have to be wrong for all but one of them. Use `messages` there.
#[pyclass(get_all, frozen)]
pub struct ValidationReport {
    /// `True` when every message is valid **and** the envelope is structurally
    /// sound. A message-level pass over a broken envelope is not a pass.
    pub is_valid: bool,
    /// `None` for a bare message (`UNH`…`UNT`) validated without an envelope.
    pub envelope: Option<Envelope>,
    pub messages: Vec<MessageReport>,
}

impl ValidationReport {
    fn only(&self) -> PyResult<&MessageReport> {
        match self.messages.as_slice() {
            [one] => Ok(one),
            other => Err(PyValueError::new_err(format!(
                "this interchange carries {} messages — use report.messages[i], \
                 because one answer would be wrong for all but one of them",
                other.len()
            ))),
        }
    }
}

#[pymethods]
impl ValidationReport {
    #[getter]
    fn pruefidentifikator(&self) -> PyResult<Option<u32>> {
        Ok(self.only()?.pruefidentifikator)
    }

    #[getter]
    fn message_type(&self) -> PyResult<Option<String>> {
        Ok(self.only()?.message_type.clone())
    }

    #[getter]
    fn release(&self) -> PyResult<Option<String>> {
        Ok(self.only()?.release.clone())
    }

    #[getter]
    fn rules_applied(&self) -> PyResult<bool> {
        Ok(self.only()?.rules_applied)
    }

    /// Every finding across every message in the interchange.
    #[getter]
    fn findings(&self) -> Vec<Finding> {
        self.messages
            .iter()
            .flat_map(|m| m.findings.clone())
            .collect()
    }

    /// Every error-severity finding across the interchange.
    #[getter]
    fn errors(&self) -> Vec<Finding> {
        self.findings()
            .into_iter()
            .filter(Finding::is_error)
            .collect()
    }

    /// Findings whose `rule_id` starts with `prefix`, across every message.
    fn by_rule(&self, prefix: &str) -> Vec<Finding> {
        self.findings()
            .into_iter()
            .filter(|f| f.rule_id.as_deref().is_some_and(|r| r.starts_with(prefix)))
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "ValidationReport(is_valid={}, messages={}, findings={})",
            self.is_valid,
            self.messages.len(),
            self.findings().len()
        )
    }
}

fn finding_of(s: edi_energy::ValidationIssueSummary) -> Finding {
    Finding {
        severity: s.severity.to_owned(),
        rule_id: s.rule_id,
        rule_origin: s.rule_origin.map(str::to_owned),
        error_code: s.error_code,
        segment: s.segment_tag,
        segment_group: s.segment_group,
        element: s.element_index,
        component: s.component_index,
        suggestion: s.suggestion,
        message: s.message,
    }
}

fn report_for(
    index: usize,
    msg: &edi_energy::AnyMessage,
    on: time::Date,
) -> PyResult<MessageReport> {
    let report = msg
        .validate_on_date(on)
        .map_err(|e| PyRuntimeError::new_err(format!("validation failed: {e}")))?;
    let pid = msg.detect_pruefidentifikator().ok();
    let mt = msg.try_message_type();
    // Vacuous-validation detection: a PID the profile set has no rules for
    // passes the AHB layer unchecked, so the pass must be reported as unearned.
    //
    // A message carrying **no** PID is a different case, and conflating the two
    // would cry wolf on every acknowledgement: CONTRL is assigned none at all,
    // and the compiled APERAK profile carries rules only for 29001/29002 (the
    // REMADV and IFTSTA rejections), so a positive APERAK has none to carry. In
    // both cases the AHB layer had nothing to apply rather than something it
    // failed to apply — and a type that *should* carry a PID and does not fails
    // the MIG layer anyway, on BGM DE 1004.
    let rules_applied = match (mt, pid) {
        (Some(mt), Some(pid)) => {
            edi_energy::registry::ReleaseRegistry::global().pid_has_ahb_rules(mt, pid)
        }
        (Some(_), None) => true,
        (None, _) => false,
    };
    Ok(MessageReport {
        index,
        message_ref: msg.message_ref().to_owned(),
        pruefidentifikator: pid.map(Pruefidentifikator::as_u32),
        message_type: mt.map(|t| t.as_str().to_owned()),
        release: msg.detect_release().ok().map(|r| r.as_str().to_owned()),
        is_valid: report.is_valid(),
        rules_applied,
        findings: report
            .iter_issues()
            .map(|i| {
                finding_of(edi_energy::ValidationIssueSummary::from_issue_with_pid(
                    i,
                    pid.map(Pruefidentifikator::as_u32),
                ))
            })
            .collect(),
    })
}

/// Count occurrences of `tag` that begin a segment.
///
/// A segment starts at the beginning of the blob or just after an unescaped
/// terminator `'`. EDIFACT escapes a literal terminator with `?`, so a `'`
/// preceded by an odd number of `?` is data rather than a boundary.
fn count_segment_starts(raw: &[u8], tag: &[u8]) -> usize {
    let mut count = 0;
    let mut at_start = true;
    for (i, b) in raw.iter().enumerate() {
        if at_start && !b.is_ascii_whitespace() {
            if raw[i..].starts_with(tag) {
                count += 1;
            }
            at_start = false;
        }
        if *b == b'\'' {
            let escapes = raw[..i].iter().rev().take_while(|c| **c == b'?').count();
            if escapes % 2 == 0 {
                at_start = true;
            }
        }
    }
    count
}

/// Parse and fully validate an EDIFACT interchange (MIG + AHB + semantic rules).
///
/// `on` (ISO 8601) selects the BDEW format version in force, exactly as the
/// production ingest path does. It is required: validating a 2026-10-01 message
/// against the 2025 profile is a different question with a different answer, and
/// a default of "today" would make the same test mean different things in
/// different months.
///
/// Accepts either a full interchange (`UNB`…`UNZ`) or a bare message
/// (`UNH`…`UNT`) — the builders return the latter, and being able to validate
/// one without wrapping it keeps the build/validate round-trip short.
///
/// Raises `ValueError` when the bytes are not parseable as EDIFACT at all; rule
/// violations come back as findings, not exceptions.
#[pyfunction]
pub fn validate_edifact(raw: &[u8], on: &str) -> PyResult<ValidationReport> {
    let date = parse_date(on)?;
    let p = platform();

    if raw.trim_ascii_start().starts_with(b"UNB") {
        let parsed = p
            .parse_interchange_full(raw)
            .map_err(|e| PyValueError::new_err(format!("EDIFACT parse failed: {e}")))?;
        let header = &parsed.header;
        let envelope = Envelope {
            sender_id: header.sender_id.to_string(),
            sender_qualifier: header.sender_qualifier.to_string(),
            receiver_id: header.receiver_id.to_string(),
            receiver_qualifier: header.receiver_qualifier.to_string(),
            control_ref: header.control_ref.to_string(),
            transmission_date: header.transmission_date().map(fmt_date).transpose()?,
            test_indicator: header.test_indicator,
            message_count: parsed.message_count(),
            declared_message_count: parsed.declared_message_count,
            is_structurally_valid: parsed.is_structurally_valid(),
        };
        let messages = parsed
            .messages
            .iter()
            .enumerate()
            .map(|(i, env)| report_for(i, &env.message, date))
            .collect::<PyResult<Vec<_>>>()?;
        let is_valid = envelope.is_structurally_valid && messages.iter().all(|m| m.is_valid);
        return Ok(ValidationReport {
            is_valid,
            envelope: Some(envelope),
            messages,
        });
    }

    // A bare blob carrying several UNH…UNT windows would be silently reduced to
    // its first message, leaving the rest unvalidated. Refuse instead: several
    // messages travel in an interchange, which has an envelope.
    //
    // Counted at segment starts only — the string `UNH+` inside an `FTX` free
    // text is data, not a message boundary, and refusing on it would reject a
    // perfectly good single message.
    let unh_count = count_segment_starts(raw, b"UNH+");
    if unh_count > 1 {
        return Err(PyValueError::new_err(format!(
            "{unh_count} messages without a UNB envelope — wrap them with \
             build_interchange(). Validating a bare blob would check only the \
             first of them"
        )));
    }

    let msg = p
        .parse(raw)
        .map_err(|e| PyValueError::new_err(format!("EDIFACT parse failed: {e}")))?;
    let message = report_for(0, &msg, date)?;
    Ok(ValidationReport {
        is_valid: message.is_valid,
        envelope: None,
        messages: vec![message],
    })
}

// ── Builders ──────────────────────────────────────────────────────────────────
//
// The Rust builders are typestate-driven (`Set`/`Unset` phantom types), which
// does not translate to Python. Python collects the parameters and hands them
// over in one call; the typestate is satisfied internally.

/// One SG4 Vorgang of a UTILMD message.
///
/// `IDE+24` carries the **Vorgangsnummer**; the Marktlokation or Messlokation
/// goes into `SG5 LOC+Z16` / `LOC+Z17` via [`locations`](Self::locations).
#[pyclass(get_all, set_all, from_py_object)]
#[derive(Clone, Default)]
pub struct UtilmdTransaction {
    /// `IDE+24` DE 7402 — the sender's own reference for this Vorgang.
    pub vorgangsnummer: String,
    /// SG4 `STS+7` DE 9013 element 2 — Transaktionsgrund, e.g. `"E01"`.
    pub transaktionsgrund: Option<String>,
    /// SG4 `STS+7` DE 9013 element 3 — Transaktionsgrundergänzung.
    ///
    /// Defaults to `"ZW4"` (verbrauchende Marktlokation) when a Grund is set,
    /// because the AHB marks the Ergänzung Muss wherever the Grund is.
    pub transaktionsgrund_ergaenzung: Option<String>,
    /// SG4 `STS+E01` DE 9013 — the EBD Antwortcode on a Bestätigung/Ablehnung.
    pub antwort_code: Option<String>,
    /// SG4 `STS+E01` DE 1131 — the EBD the Antwortcode comes from, e.g. `"E_0624"`.
    pub antwort_ebd: Option<String>,
    /// `(qualifier, YYYYMMDD)` SG4 DTM pairs — `("92", …)` Beginn zum,
    /// `("93", …)` Ende zum, `("154", …)` ÜT der Lieferanmeldung.
    pub dates: Vec<(String, String)>,
    /// `(qualifier, value)` SG6 RFF pairs, e.g. `("Z13", "55001")`.
    pub references: Vec<(String, String)>,
    /// `(Lokationstyp, id)` SG5 LOC pairs — `("malo", …)`, `("melo", …)`.
    pub locations: Vec<(String, String)>,
    /// `(party qualifier, id)` NAD pairs, e.g. `("UD", "…")` for the customer.
    pub customers: Vec<(String, String)>,
    /// `(text function, text)` FTX pairs — `("ACB", …)` is the Bemerkung.
    pub free_texts: Vec<(String, String)>,
}

#[pymethods]
impl UtilmdTransaction {
    #[new]
    #[pyo3(signature = (
        vorgangsnummer, transaktionsgrund=None, transaktionsgrund_ergaenzung=None,
        antwort_code=None, antwort_ebd=None, dates=None,
        references=None, locations=None, customers=None, free_texts=None
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        vorgangsnummer: String,
        transaktionsgrund: Option<String>,
        transaktionsgrund_ergaenzung: Option<String>,
        antwort_code: Option<String>,
        antwort_ebd: Option<String>,
        dates: Option<Vec<(String, String)>>,
        references: Option<Vec<(String, String)>>,
        locations: Option<Vec<(String, String)>>,
        customers: Option<Vec<(String, String)>>,
        free_texts: Option<Vec<(String, String)>>,
    ) -> Self {
        Self {
            vorgangsnummer,
            transaktionsgrund,
            transaktionsgrund_ergaenzung,
            antwort_code,
            antwort_ebd,
            dates: dates.unwrap_or_default(),
            references: references.unwrap_or_default(),
            locations: locations.unwrap_or_default(),
            customers: customers.unwrap_or_default(),
            free_texts: free_texts.unwrap_or_default(),
        }
    }

    fn __repr__(&self) -> String {
        format!("UtilmdTransaction({})", self.vorgangsnummer)
    }
}

/// Resolve a `SG5 LOC` DE 3227 qualifier from a Python-friendly name.
///
/// A raw wire code (`"Z16"`) is accepted too, so a test can pin an exact
/// qualifier without going through the alias table.
fn lokationstyp_from_str(s: &str) -> PyResult<Lokationstyp> {
    if let Some(t) = Lokationstyp::from_qualifier_code(s) {
        return Ok(t);
    }
    Ok(match s.to_ascii_lowercase().as_str() {
        "malo" | "marktlokation" => Lokationstyp::Marktlokation,
        "melo" | "messlokation" => Lokationstyp::Messlokation,
        "nelo" | "netzlokation" => Lokationstyp::Netzlokation,
        "tranche" => Lokationstyp::Tranche,
        "tr" | "technische_ressource" => Lokationstyp::TechnischeRessource,
        "sr" | "steuerbare_ressource" => Lokationstyp::SteuerbareRessource,
        "ruhende_malo" | "ruhende_marktlokation" => Lokationstyp::RuhendeMarktlokation,
        "mabis" | "mabis_zaehlpunkt" => Lokationstyp::MabisZaehlpunkt,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown Lokationstyp {other:?} — expected a LOC DE 3227 code \
                 (Z15…Z22) or one of malo, melo, nelo, tranche, tr, sr, \
                 ruhende_malo, mabis"
            )));
        }
    })
}

/// The `DTM+137` document date to write, as `YYYYMMDD`.
///
/// Defaults to the send date `on` rather than to the builders' "today". A
/// document date read from the clock makes every rendered message differ
/// between runs, which defeats golden-file comparison and turns a reproducible
/// failure into an intermittent one. `on` is the date the message would really
/// be sent, so it is also the date the document carries.
fn document_date_for(document_date: Option<&str>, on: Option<&str>) -> PyResult<Option<String>> {
    if let Some(d) = document_date {
        return Ok(Some(d.to_owned()));
    }
    match on {
        Some(date) => {
            let d = parse_date(date)?;
            Ok(Some(format!(
                "{:04}{:02}{:02}",
                d.year(),
                d.month() as u8,
                d.day()
            )))
        }
        None => Ok(None),
    }
}

fn pid(value: u32) -> PyResult<Pruefidentifikator> {
    Pruefidentifikator::new(value).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Build a UTILMD message and return the rendered EDIFACT bytes.
///
/// Pass `on` (the date the message would be sent) and the release is resolved
/// from the active profile, `sparte` selecting the Strom or Gas track; pass
/// `release` to pin a wire code explicitly, which is what a
/// cross-format-version test wants.
///
/// The result is a **message** (`UNH`…`UNT`), not an interchange, and it is not
/// auto-validated.
#[pyfunction]
#[pyo3(signature = (
    pruefidentifikator, sender, receiver, *, on=None, release=None, sparte=None,
    message_ref="1", document_date=None, document_code="E01", references=None,
    transactions=None
))]
#[allow(clippy::too_many_arguments)]
pub fn build_utilmd(
    pruefidentifikator: u32,
    sender: &str,
    receiver: &str,
    on: Option<&str>,
    release: Option<&str>,
    sparte: Option<&str>,
    message_ref: &str,
    document_date: Option<&str>,
    document_code: &str,
    references: Option<Vec<(String, String)>>,
    transactions: Option<Vec<UtilmdTransaction>>,
) -> PyResult<Vec<u8>> {
    // 55xxx is Strom and 44xxx is Gas, so the caller need not repeat it.
    let sparte = sparte.or(match pruefidentifikator {
        44000..=44999 => Some("GAS"),
        55000..=55999 => Some("STROM"),
        _ => None,
    });
    let rel = resolve_release(MessageType::Utilmd, release, on, sparte)?;
    let mut b = UtilmdBuilder::new(rel)
        .pruefidentifikator(pid(pruefidentifikator)?)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(document_code);
    if let Some(d) = document_date_for(document_date, on)? {
        b = b.document_date(d);
    }
    for (q, r) in references.unwrap_or_default() {
        b = b.rff(q, r);
    }

    for tx in transactions.unwrap_or_default() {
        let mut t = b.transaction(&tx.vorgangsnummer);
        for (q, d) in &tx.dates {
            t = t.date(q.as_str(), d.as_str());
        }
        if let Some(grund) = &tx.transaktionsgrund {
            let erg = tx
                .transaktionsgrund_ergaenzung
                .as_deref()
                .unwrap_or(edi_energy::utilmd_codes::ergaenzung::VERBRAUCHENDE_MALO);
            t = t.transaktionsgrund(Transaktionsgrund::new(grund.as_str(), erg));
        }
        if let Some(code) = &tx.antwort_code {
            t = t.antwort(match tx.antwort_ebd.as_deref() {
                Some(ebd) => AntwortStatus::from_ebd(code.as_str(), ebd),
                None => AntwortStatus::bare(code.as_str()),
            });
        }
        for (q, r) in &tx.references {
            t = t.reference(q.as_str(), r.as_str());
        }
        for (q, id) in &tx.locations {
            t = t.location(lokationstyp_from_str(q)?, id.as_str());
        }
        for (q, id) in &tx.customers {
            t = t.customer(q.as_str(), id.as_str());
        }
        for (f, text) in &tx.free_texts {
            t = t.free_text(f.as_str(), text.as_str());
        }
        b = t.done();
    }

    b.build()
        .map_err(|e| PyValueError::new_err(format!("UTILMD build failed: {e}")))?
        .serialize()
        .map_err(|e| PyRuntimeError::new_err(format!("UTILMD serialize failed: {e}")))
}

/// Build an MSCONS message and return the rendered EDIFACT bytes.
///
/// `quantities` are `(qualifier, value, unit)` triples, e.g.
/// `("220", "1234.567", "KWH")`. `bilanzierungsgebiet` populates the SG6
/// `LOC+Z17`; pass a real EIC (see `bilanzierungsgebiet_from_prefix`) — the
/// object-type character is what separates it from a Bilanzkreis.
#[pyfunction]
#[pyo3(signature = (
    pruefidentifikator, sender, receiver, metering_point, quantities, *,
    on=None, release=None, message_ref="1", document_date=None, obis=None,
    bilanzierungsgebiet=None
))]
#[allow(clippy::too_many_arguments)]
pub fn build_mscons(
    pruefidentifikator: u32,
    sender: &str,
    receiver: &str,
    metering_point: &str,
    quantities: Vec<(String, String, String)>,
    on: Option<&str>,
    release: Option<&str>,
    message_ref: &str,
    document_date: Option<&str>,
    obis: Option<&str>,
    bilanzierungsgebiet: Option<&str>,
) -> PyResult<Vec<u8>> {
    let rel = resolve_release(MessageType::Mscons, release, on, None)?;
    let mut b = MsconsBuilder::new(rel)
        .pruefidentifikator(pid(pruefidentifikator)?)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);
    if let Some(d) = document_date_for(document_date, on)? {
        b = b.document_date(d);
    }

    let mut mp = b.metering_point(metering_point);
    if let Some(code) = obis {
        let parsed = rubo4e::identifiers::ObisCode::new(code)
            .map_err(|e| PyValueError::new_err(format!("invalid OBIS code {code:?}: {e}")))?;
        mp = mp.obis(parsed);
    }
    if let Some(eic) = bilanzierungsgebiet {
        mp = mp.bilanzierungsgebiet(eic);
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

/// Build an APERAK — the **application-level** acknowledgement.
///
/// An APERAK reports whether a message could be *processed*: an AHB or semantic
/// violation, a business rejection, or a positive Anerkennungsmeldung. A syntax
/// failure is a CONTRL instead ([`build_contrl`]); answering an AHB violation
/// with a CONTRL, or a broken envelope with an APERAK, is the mistake this pair
/// of functions exists to keep apart.
///
/// `error_code` is the APERAK ERC. Supplying one selects BGM+313
/// (Verarbeitbarkeitsfehlermeldung) per BDEW APERAK AHB 1.0 §2.1.1; without it
/// the message is a positive acknowledgement.
#[pyfunction]
#[pyo3(signature = (
    sender, receiver, *, on=None, release=None, pruefidentifikator=None,
    acw_ref=None, error_code=None, error_text=None, message_ref="1",
    document_date=None, document_code=None
))]
#[allow(clippy::too_many_arguments)]
pub fn build_aperak(
    sender: &str,
    receiver: &str,
    on: Option<&str>,
    release: Option<&str>,
    pruefidentifikator: Option<u32>,
    acw_ref: Option<&str>,
    error_code: Option<&str>,
    error_text: Option<&str>,
    message_ref: &str,
    document_date: Option<&str>,
    document_code: Option<&str>,
) -> PyResult<Vec<u8>> {
    let rel = resolve_release(MessageType::Aperak, release, on, None)?;
    let mut b = AperakBuilder::new(rel)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);
    if let Some(p) = pruefidentifikator {
        b = b.pruefidentifikator(pid(p)?);
    }
    if let Some(r) = acw_ref {
        b = b.acw_ref(r);
    }
    if let Some(c) = error_code {
        b = b.error_code(c);
    }
    if let Some(t) = error_text {
        b = b.error_text(t);
    }
    if let Some(d) = document_date_for(document_date, on)? {
        b = b.document_date(d);
    }
    match document_code {
        Some(c) => b = b.document_code(c),
        // Mirrors the platform's own renderer: BGM+313 is mandatory for an
        // APERAK carrying an error code.
        None if error_code.is_some() => b = b.document_code("313"),
        None => b = b.document_code("312"),
    }
    b.serialize()
        .map_err(|e| PyRuntimeError::new_err(format!("APERAK serialize failed: {e}")))
}

/// Build a CONTRL — the **syntax-level** acknowledgement for an interchange.
///
/// `interchange_ref` is the UNB Datenaustauschreferenz of the interchange being
/// acknowledged; `accept=False` rejects it. Use this for a malformed envelope or
/// an unparseable segment, and [`build_aperak`] for anything the AHB decides.
#[pyfunction]
#[pyo3(signature = (
    sender, receiver, interchange_ref, *, on=None, release=None, accept=true,
    message_ref="1", action_code=None
))]
#[allow(clippy::too_many_arguments)]
pub fn build_contrl(
    sender: &str,
    receiver: &str,
    interchange_ref: &str,
    on: Option<&str>,
    release: Option<&str>,
    accept: bool,
    message_ref: &str,
    action_code: Option<&str>,
) -> PyResult<Vec<u8>> {
    let rel = resolve_release(MessageType::Contrl, release, on, None)?;
    let mut b = ContrlBuilder::new(rel)
        .sender(sender)
        .receiver(receiver)
        .interchange_ref(interchange_ref)
        .message_ref(message_ref);
    b = if accept { b.accept() } else { b.reject() };
    if let Some(code) = action_code {
        b = b.action_code(code);
    }
    b.serialize()
        .map_err(|e| PyRuntimeError::new_err(format!("CONTRL serialize failed: {e}")))
}

/// Build the APERAK that acknowledges `received`, with the fields mirrored.
///
/// Parses `received`, takes the first message's receipt context and lets the
/// platform's own builder mirror the parties, carry the acknowledged UNH
/// reference into `RFF+ACW` and adopt the transmission date. Those three fields
/// are what correlate an acknowledgement with what it acknowledges; deriving
/// them here is the only way a simulated counterparty cannot get them wrong.
///
/// Supplying `error_code` makes it a Verarbeitbarkeitsfehlermeldung (BGM+313).
///
/// `message_index` selects which message of the interchange is acknowledged —
/// `RFF+ACW` carries that message's UNH reference, so an interchange with
/// several messages needs several APERAKs, one per message.
#[pyfunction]
#[pyo3(signature = (
    received, *, on=None, release=None, error_code=None, error_text=None,
    pruefidentifikator=None, message_ref="1", message_index=0
))]
#[allow(clippy::too_many_arguments)]
pub fn build_aperak_for(
    received: &[u8],
    on: Option<&str>,
    release: Option<&str>,
    error_code: Option<&str>,
    error_text: Option<&str>,
    pruefidentifikator: Option<u32>,
    message_ref: &str,
    message_index: usize,
) -> PyResult<Vec<u8>> {
    let rel = resolve_release(MessageType::Aperak, release, on, None)?;
    let parsed = platform()
        .parse_interchange_full(received)
        .map_err(|e| PyValueError::new_err(format!("EDIFACT parse failed: {e}")))?;
    let envelope = parsed.messages.get(message_index).ok_or_else(|| {
        PyValueError::new_err(format!(
            "the interchange carries {} message(s), so there is no \
             message_index={message_index} to acknowledge",
            parsed.messages.len()
        ))
    })?;
    let mut b = AperakBuilder::new(rel)
        .for_receipt(&envelope.receipt_context())
        .message_ref(message_ref);
    if let Some(p) = pruefidentifikator {
        b = b.pruefidentifikator(pid(p)?);
    }
    if let Some(c) = error_code {
        b = b.error_code(c).document_code("313");
    } else {
        b = b.document_code("312");
    }
    if let Some(t) = error_text {
        b = b.error_text(t);
    }
    b.serialize()
        .map_err(|e| PyRuntimeError::new_err(format!("APERAK serialize failed: {e}")))
}

/// Build the UTILMD **business answer** to a received UTILMD request.
///
/// This is what a counterparty sends after acknowledging: a Bestätigung or an
/// Ablehnung under the answer Prüfidentifikator the AHB assigns. Everything
/// that correlates the answer with the request is mirrored from the request
/// itself — the parties are swapped and the SG4 `IDE+24` Vorgangsnummer is
/// echoed, together with the `RFF` references the requester matches on.
///
/// `answer_pid` is not derived here: which of the pair applies is the
/// counterparty's *decision*, and a simulator that always confirmed would never
/// exercise a rejection. Resolve it with `bestaetigung_pid` / `ablehnung_pid`.
///
/// `antwort_code` and `antwort_ebd` render the `SG4 STS+E01` the AHB marks Muss
/// on every Antwortnachricht — `("A36", "E_0624")` for a Zustimmung to a
/// Beendigung der Zuordnung. A simulator that omits them produces an answer no
/// conformant counterparty accepts, which is the opposite of a useful test.
///
/// `process_dates` and `references` are appended to the echoed ones — that is
/// where the answer's own content goes, e.g. `("93", "20261101")` for a
/// confirmed Zuordnungsende.
///
/// `message_index` selects which message of the interchange is being answered.
/// An interchange routinely carries several, and each is a separate Vorgang with
/// its own Prüfidentifikator — answering only the first and calling it the
/// answer to the interchange leaves the rest unanswered without saying so.
#[pyfunction]
#[pyo3(signature = (
    received, answer_pid, *, on=None, release=None, message_ref="1",
    document_date=None, document_code=None, antwort_code=None, antwort_ebd=None,
    process_dates=None, references=None, message_index=0
))]
#[allow(clippy::too_many_arguments)]
pub fn build_answer(
    received: &[u8],
    answer_pid: u32,
    on: Option<&str>,
    release: Option<&str>,
    message_ref: &str,
    document_date: Option<&str>,
    document_code: Option<&str>,
    antwort_code: Option<&str>,
    antwort_ebd: Option<&str>,
    process_dates: Option<Vec<(String, String)>>,
    references: Option<Vec<(String, String)>>,
    message_index: usize,
) -> PyResult<Vec<u8>> {
    let sparte = match answer_pid {
        44000..=44999 => Some("GAS"),
        55000..=55999 => Some("STROM"),
        _ => None,
    };
    let rel = resolve_release(MessageType::Utilmd, release, on, sparte)?;
    let parsed = platform()
        .parse_interchange_full(received)
        .map_err(|e| PyValueError::new_err(format!("EDIFACT parse failed: {e}")))?;
    let envelope = parsed.messages.get(message_index).ok_or_else(|| {
        PyValueError::new_err(format!(
            "the interchange carries {} message(s), so there is no message_index={} \
             to answer",
            parsed.messages.len(),
            message_index
        ))
    })?;
    let edi_energy::AnyMessage::Utilmd(request) = &envelope.message else {
        return Err(PyValueError::new_err(
            "build_answer answers a UTILMD request; acknowledge other message \
             types with build_aperak_for",
        ));
    };

    let mut b = UtilmdBuilder::new(rel)
        .pruefidentifikator(pid(answer_pid)?)
        // Mirrored: the request's receiver is the one answering.
        .sender(envelope.header.receiver_id.to_string())
        .receiver(envelope.header.sender_id.to_string())
        .message_ref(message_ref)
        // `BGM` DE 1001 is the Nachrichtenfunktion of the *process*, not of the
        // direction: the UTILMD AHB gives an Anwendungsfall one code across all
        // three of its PIDs, so a 55005 Bestätigung Abmeldung is `E02` just like
        // the 55004 it answers, and a 55017 is `E35` like its 55016. Echoing the
        // request is right by construction; hard-coding `E01` made the simulator
        // answer 55004 and 55016 with messages our own AHB layer rejects.
        .document_code(
            document_code
                .or_else(|| request.bgm().map(|b| b.document_code.as_str()))
                .unwrap_or("E01"),
        );
    if let Some(d) = document_date_for(document_date, on)? {
        b = b.document_date(d);
    }

    for tx in request.transactions() {
        let Some(vorgangsnummer) = tx.vorgangsnummer() else {
            continue;
        };
        // The answer echoes the request's Vorgangsnummer — that is what
        // correlates it on the counterparty's side.
        let mut a = b.transaction(vorgangsnummer);
        // The Lokation travels with the answer: it lives in `SG5 LOC`, not in
        // `IDE`, so it is echoed explicitly rather than riding the Vorgang.
        for loc in &tx.locations {
            if let (Some(lokationstyp), Some(id)) = (
                Lokationstyp::from_qualifier_code(&loc.qualifier),
                loc.location_id.as_deref(),
            ) {
                a = a.location(lokationstyp, id);
            }
        }
        for (q, d) in process_dates.iter().flatten() {
            a = a.date(q.as_str(), d.as_str());
        }
        if let Some(code) = antwort_code {
            a = a.antwort(match antwort_ebd {
                Some(ebd) => AntwortStatus::from_ebd(code, ebd),
                None => AntwortStatus::bare(code),
            });
        }
        for rff in &tx.references {
            if let Some(reference) = rff.reference.as_deref() {
                a = a.reference(rff.qualifier.as_str(), reference);
            }
        }
        for (q, r) in references.iter().flatten() {
            a = a.reference(q.as_str(), r.as_str());
        }
        b = a.done();
    }

    b.build()
        .map_err(|e| PyValueError::new_err(format!("UTILMD answer build failed: {e}")))?
        .serialize()
        .map_err(|e| PyRuntimeError::new_err(format!("UTILMD answer serialize failed: {e}")))
}

/// Build the CONTRL that acknowledges the interchange `received`.
///
/// Mirrors the UNB parties and echoes the Datenaustauschreferenz. A CONTRL
/// acknowledges the **envelope**, so this needs only the header — but the
/// header still has to parse: for bytes that are not EDIFACT at all, no
/// correlated CONTRL exists and [`build_contrl`] with a caller-supplied
/// reference is the honest fallback.
#[pyfunction]
#[pyo3(signature = (received, *, on=None, release=None, accept=true, message_ref="1"))]
pub fn build_contrl_for(
    received: &[u8],
    on: Option<&str>,
    release: Option<&str>,
    accept: bool,
    message_ref: &str,
) -> PyResult<Vec<u8>> {
    let rel = resolve_release(MessageType::Contrl, release, on, None)?;
    let parsed = platform()
        .parse_interchange_full(received)
        .map_err(|e| PyValueError::new_err(format!("EDIFACT parse failed: {e}")))?;
    let b = ContrlBuilder::new(rel)
        .for_interchange(&parsed.header)
        .message_ref(message_ref);
    let b = if accept { b.accept() } else { b.reject() };
    b.serialize()
        .map_err(|e| PyRuntimeError::new_err(format!("CONTRL serialize failed: {e}")))
}

/// Wrap one or more messages in a UNB/UNZ interchange envelope.
///
/// A message (`UNH`…`UNT`) is not sendable on its own — the wire unit a market
/// partner receives over AS4 is the interchange. The UNB qualifier after each
/// party ID is derived from the ID itself, so it cannot contradict it.
///
/// The UNB transmission timestamp is a parameter rather than a clock read, so
/// building stays deterministic: pass `on` (the send date, ISO 8601) and it is
/// rendered as `YYMMDD`, or `date`/`time` (`YYMMDD`/`HHMM`) to control both.
/// One of them is required — `000000:0000` parses to no date at all, and a
/// counterparty rejects an interchange it cannot date.
#[pyfunction]
#[pyo3(signature = (sender, receiver, dar, messages, *, on=None, date=None, time="0000"))]
pub fn build_interchange(
    sender: &str,
    receiver: &str,
    dar: &str,
    messages: Vec<Vec<u8>>,
    on: Option<&str>,
    date: Option<&str>,
    time: &str,
) -> PyResult<Vec<u8>> {
    if messages.is_empty() {
        return Err(PyValueError::new_err(
            "an interchange carries at least one message — UNZ+0 is not a thing a \
             counterparty accepts",
        ));
    }
    let date = match (date, on) {
        (Some(d), _) => d.to_owned(),
        (None, Some(iso)) => {
            let d = parse_date(iso)?;
            format!("{:02}{:02}{:02}", d.year() % 100, d.month() as u8, d.day())
        }
        (None, None) => {
            return Err(PyValueError::new_err(
                "pass either on= (the send date) or date= (YYMMDD) — an \
                 interchange with no transmission date is one a counterparty \
                 cannot process",
            ));
        }
    };
    let mut b = InterchangeBuilder::new(sender, receiver, dar).transmission(&date, time);
    for m in messages {
        b = b.message(m);
    }
    b.build()
        .map_err(|e| PyRuntimeError::new_err(format!("interchange build failed: {e}")))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(validate_edifact, m)?)?;
    m.add_function(wrap_pyfunction!(build_utilmd, m)?)?;
    m.add_function(wrap_pyfunction!(build_mscons, m)?)?;
    m.add_function(wrap_pyfunction!(build_aperak, m)?)?;
    m.add_function(wrap_pyfunction!(build_contrl, m)?)?;
    m.add_function(wrap_pyfunction!(build_aperak_for, m)?)?;
    m.add_function(wrap_pyfunction!(build_contrl_for, m)?)?;
    m.add_function(wrap_pyfunction!(build_answer, m)?)?;
    m.add_function(wrap_pyfunction!(build_interchange, m)?)?;
    m.add_class::<UtilmdTransaction>()?;
    m.add_class::<Finding>()?;
    m.add_class::<MessageReport>()?;
    m.add_class::<Envelope>()?;
    m.add_class::<ValidationReport>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unwrap without rendering the error.
    ///
    /// Displaying a `PyErr` acquires the GIL, and plain `cargo test` links this
    /// crate without `pyo3/extension-module` and starts no interpreter — so an
    /// `unwrap()` on `Err` aborts the process instead of failing the test. The
    /// message for any of these paths is asserted from the Python suite, which
    /// has an interpreter by construction.
    #[track_caller]
    fn ok<T>(r: PyResult<T>, what: &str) -> T {
        match r {
            Ok(v) => v,
            Err(_) => panic!("{what} returned Err — run `just test-makotest` for the message"),
        }
    }

    fn utilmd_55001() -> Vec<u8> {
        let built = build_utilmd(
            55001,
            "4012345000023",
            "9900357000003",
            Some("2025-10-01"),
            None,
            None,
            "MSG-1",
            Some("20251101"),
            "E01",
            None,
            Some(vec![UtilmdTransaction {
                vorgangsnummer: "VORGANG-0001".into(),
                dates: vec![("92".into(), "20251101".into())],
                locations: vec![("malo".into(), "51238696012".into())],
                references: vec![("Z13".into(), "55001".into())],
                ..Default::default()
            }]),
        );
        ok(built, "build_utilmd")
    }

    /// The release must come from the date, so a build and the validation of
    /// its output cannot disagree about which profile applies.
    #[test]
    fn a_dated_build_validates_on_that_date() {
        let report = ok(validate_edifact(&utilmd_55001(), "2025-10-01"), "validate");
        assert_eq!(report.messages.len(), 1);
        let m = &report.messages[0];
        assert_eq!(m.pruefidentifikator, Some(55001));
        assert_eq!(m.message_type.as_deref(), Some("UTILMD"));
        assert!(m.release.as_deref().is_some_and(|r| r.starts_with('S')));
        assert!(m.rules_applied, "55001 has real AHB rules");
    }

    /// Neither a release nor a date is not a defaultable situation.
    ///
    /// The error *text* is asserted from Python (`test_builders.py`) rather than
    /// here: rendering a `PyErr` needs a live interpreter, which plain
    /// `cargo test` deliberately does not link.
    #[test]
    fn building_without_a_release_or_a_date_is_refused() {
        assert!(
            build_utilmd(
                55001,
                "4012345000023",
                "9900357000003",
                None,
                None,
                None,
                "1",
                None,
                "E01",
                None,
                None,
            )
            .is_err()
        );
    }

    /// The envelope must be reported, and its qualifier derived from the ID —
    /// `4012…` is a GLN (14) and `99…` a BDEW code (500).
    #[test]
    fn the_envelope_is_reported_with_derived_qualifiers() {
        let wire = ok(
            build_interchange(
                "4012345000023",
                "9900357000003",
                "REF001",
                vec![utilmd_55001()],
                Some("2025-11-01"),
                None,
                "0915",
            ),
            "build_interchange",
        );
        let report = ok(validate_edifact(&wire, "2025-10-01"), "validate");
        let env = report.envelope.as_ref().expect("an interchange has one");
        assert_eq!(env.sender_qualifier, "14");
        assert_eq!(env.receiver_qualifier, "500");
        assert_eq!(env.control_ref, "REF001");
        assert_eq!(env.message_count, 1);
        assert!(env.is_structurally_valid);
    }

    /// A multi-message interchange must not answer a single-message question.
    #[test]
    fn multi_message_convenience_accessors_refuse_to_guess() {
        let wire = ok(
            build_interchange(
                "4012345000023",
                "9900357000003",
                "REF002",
                vec![utilmd_55001(), utilmd_55001()],
                Some("2025-11-01"),
                None,
                "0915",
            ),
            "build_interchange",
        );
        let report = ok(validate_edifact(&wire, "2025-10-01"), "validate");
        assert_eq!(report.messages.len(), 2);
        assert!(report.pruefidentifikator().is_err());
        assert_eq!(
            report.findings().len(),
            report
                .messages
                .iter()
                .map(|m| m.findings.len())
                .sum::<usize>()
        );
    }

    /// Both acknowledgement kinds must build and parse. They are different
    /// messages for different failures and a simulator has to pick correctly.
    #[test]
    fn both_acknowledgements_build_and_parse() {
        let aperak = ok(
            build_aperak(
                "9900357000003",
                "4012345000023",
                Some("2025-10-01"),
                None,
                Some(29002),
                Some("MSG-1"),
                Some("Z10"),
                Some("Marktlokation unbekannt"),
                "1",
                Some("20251101"),
                None,
            ),
            "build_aperak",
        );
        let text = String::from_utf8(aperak.clone()).unwrap();
        assert!(text.starts_with("UNH+1+APERAK"), "{text}");
        assert!(
            text.contains("BGM+313"),
            "an error APERAK is BGM+313: {text}"
        );
        assert!(text.contains("ERC+Z10"), "{text}");
        assert!(validate_edifact(&aperak, "2025-10-01").is_ok());

        let contrl = ok(
            build_contrl(
                "9900357000003",
                "4012345000023",
                "REF001",
                Some("2026-02-02"),
                None,
                false,
                "1",
                None,
            ),
            "build_contrl",
        );
        assert!(
            String::from_utf8(contrl)
                .unwrap()
                .starts_with("UNH+1+CONTRL")
        );
    }

    /// The acknowledgement must correlate itself with what it acknowledges.
    #[test]
    fn an_acknowledgement_mirrors_the_message_it_answers() {
        let wire = ok(
            build_interchange(
                "4012345000023",
                "9900357000003",
                "REF009",
                vec![utilmd_55001()],
                Some("2026-02-02"),
                None,
                "0915",
            ),
            "build_interchange",
        );
        let aperak = ok(
            build_aperak_for(
                &wire,
                Some("2026-02-02"),
                None,
                Some("Z10"),
                None,
                None,
                "1",
                0,
            ),
            "build_aperak_for",
        );
        let text = String::from_utf8(aperak).unwrap();
        // The receiver of the UTILMD answers, so it is now the sender.
        assert!(text.contains("NAD+MS+9900357000003"), "{text}");
        assert!(text.contains("NAD+MR+4012345000023"), "{text}");
        assert!(
            text.contains("RFF+ACW:MSG-1"),
            "the acknowledged UNH ref: {text}"
        );
        assert!(text.contains("BGM+313"), "{text}");

        let contrl = ok(
            build_contrl_for(&wire, Some("2026-02-02"), None, false, "1"),
            "build_contrl_for",
        );
        let text = String::from_utf8(contrl).unwrap();
        assert!(text.contains("REF009"), "echoes the DAR: {text}");
    }

    /// The answer must correlate with the request: mirrored parties, the same
    /// IDE object under the qualifier the request used, and its references.
    #[test]
    fn the_business_answer_mirrors_the_request() {
        let wire = ok(
            build_interchange(
                "4012345000023",
                "9900357000003",
                "REF010",
                vec![utilmd_55001()],
                Some("2025-11-01"),
                None,
                "0915",
            ),
            "build_interchange",
        );
        let answer = ok(
            build_answer(
                &wire,
                55002,
                Some("2025-10-01"),
                None,
                "ANS-1",
                Some("20251102"),
                // Left unset so the answer echoes the request's DE 1001.
                None,
                Some("A10"),
                Some("E_0609"),
                Some(vec![("92".into(), "20261101".into())]),
                None,
                0,
            ),
            "build_answer",
        );
        let text = String::from_utf8(answer.clone()).unwrap();
        assert!(text.contains("BGM+E01+55002"), "{text}");
        assert!(
            text.contains("NAD+MS+9900357000003"),
            "mirrored sender: {text}"
        );
        assert!(
            text.contains("IDE+24+VORGANG-0001"),
            "the request's Vorgangsnummer is echoed: {text}"
        );
        assert!(
            text.contains("RFF+Z13:55001"),
            "the request's RFF is echoed: {text}"
        );
        // `92` Beginn zum, not the Messperioden-Qualifier `163`.
        assert!(text.contains("DTM+92:20261101"), "{text}");
        // Every Antwortnachricht carries its EBD Antwortcode — AHB Muss.
        assert!(text.contains("STS+E01++A10:E_0609"), "{text}");

        let report = ok(validate_edifact(&answer, "2025-10-01"), "validate");
        assert_eq!(report.messages[0].pruefidentifikator, Some(55002));
    }

    /// An unparseable input is an exception, not a report full of findings —
    /// there is nothing to report findings *about*.
    #[test]
    fn unparseable_input_raises() {
        assert!(validate_edifact(b"this is not EDIFACT", "2025-10-01").is_err());
    }
}
