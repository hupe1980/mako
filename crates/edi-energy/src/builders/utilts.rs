//! [`UtiltsBuilder`] — fluent type-safe builder for UTILTS messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments};

macro_rules! emit_seg {
    ($writer:expr, $tag:expr, $($elem:expr),+ $(,)?) => {{
        let elements: &[&str] = &[$($elem),+];
        $writer.write_raw($tag, elements).map_err(|e| Error::Parse(e.into()))?;
    }};
}

// ── UTILTS-specific DTM helpers (format 303) ──────────────────────────────────

fn fmt303(qualifier: &str, date: &str) -> String {
    format!("{qualifier}:{date}0000+0000:303")
}

fn dtm_now_303() -> String {
    let now = time::OffsetDateTime::now_utc();
    let (y, mo, d, h, m) = (
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
    );
    format!("137:{y:04}{mo:02}{d:02}{h:02}{m:02}+0000:303")
}

// ── Body structs (public, re-exported from mod.rs) ────────────────────────────

/// A single usage-period block (SG6 `RFF+Z49/Z53` + `DTM+Z25` + optional `DTM+Z26`).
#[derive(Debug, Clone)]
pub struct UtiltsUsagePeriod {
    qualifier: String,
    period_id: u32,
    usage_from: String,
    usage_to: Option<String>,
}

impl UtiltsUsagePeriod {
    /// Create a usage-period block.
    ///
    /// - `period_id`: sequence number starting at 1 (oldest period first).
    /// - `usage_from`: format-303 datetime string, e.g. `"202710012200+00"`.
    pub fn new(period_id: u32, usage_from: impl Into<String>) -> Self {
        Self {
            qualifier: "Z49".to_owned(),
            period_id,
            usage_from: usage_from.into(),
            usage_to: None,
        }
    }

    /// Set qualifier to `"Z53"` (Viertelstundenwerte).  Defaults to `"Z49"`.
    pub fn qualifier(mut self, q: impl Into<String>) -> Self {
        self.qualifier = q.into();
        self
    }

    /// Set `DTM+Z26` (Verwendung der Daten bis) for this period.
    pub fn usage_to(mut self, datetime: impl Into<String>) -> Self {
        self.usage_to = Some(datetime.into());
        self
    }
}

/// A reference to an energy-amount result (SG8 `SEQ+Z36`).
#[derive(Debug, Clone)]
pub struct UtiltsEnergyAmountRef {
    time_period_id: u32,
    final_step_id: u32,
}

impl UtiltsEnergyAmountRef {
    /// Create a reference to the energy-amount result.
    ///
    /// - `time_period_id`: Zeitraum-ID (DE 1154 in `RFF+Z46`).
    /// - `final_step_id`: Rechenschrittidentifikator (DE 1154 in `RFF+Z23`).
    #[must_use]
    pub fn new(time_period_id: u32, final_step_id: u32) -> Self {
        Self {
            time_period_id,
            final_step_id,
        }
    }
}

/// One calculation-step component (SG8 `SEQ+Z37+{step_id}`).
///
/// # Example
///
/// ```no_run
/// use edi_energy::builders::UtiltsCalcStep;
/// let step = UtiltsCalcStep::new(1)
///     .time_period(1)
///     .messlokation("DE00000456789012345678900000000003D")
///     .operator("Z69")
///     .energy_direction("Z71");
/// ```
#[derive(Debug, Clone)]
pub struct UtiltsCalcStep {
    step_id: u32,
    time_period_id: Option<u32>,
    messlokation_id: Option<String>,
    ref_calc_step_id: Option<u32>,
    operator: Option<String>,
    energy_direction: Option<String>,
    loss_factor_trafo: Option<String>,
    loss_factor_line: Option<String>,
    split_factor: Option<String>,
}

impl UtiltsCalcStep {
    /// Create a calculation-step component.
    ///
    /// `step_id` is the Rechenschrittidentifikator (DE 1050 in `SEQ+Z37`).
    #[must_use]
    pub fn new(step_id: u32) -> Self {
        Self {
            step_id,
            time_period_id: None,
            messlokation_id: None,
            ref_calc_step_id: None,
            operator: None,
            energy_direction: None,
            loss_factor_trafo: None,
            loss_factor_line: None,
            split_factor: None,
        }
    }

    /// Set the Zeitraum-ID reference (`RFF+Z46:{id}`).
    #[must_use]
    pub fn time_period(mut self, id: u32) -> Self {
        self.time_period_id = Some(id);
        self
    }

    /// Set the Messlokations-ID reference (`RFF+Z19:{id}`).
    pub fn messlokation(mut self, id: impl Into<String>) -> Self {
        self.messlokation_id = Some(id.into());
        self
    }

    /// Set a reference to another calculation step (`RFF+Z23:{id}`).
    #[must_use]
    pub fn ref_calc_step(mut self, id: u32) -> Self {
        self.ref_calc_step_id = Some(id);
        self
    }

    /// Set the mathematical operator (`CAV` in `SG9 CCI+++Z86`).
    ///
    /// Codes: `Z69` (Addition), `Z70` (Subtraktion), `Z80`–`Z85` (other ops).
    pub fn operator(mut self, code: impl Into<String>) -> Self {
        self.operator = Some(code.into());
        self
    }

    /// Set the energy flow direction (`CAV` in `SG9 CCI+++Z87`).
    ///
    /// - `Z71` — Verbrauch, `Z72` — Erzeugung.
    pub fn energy_direction(mut self, code: impl Into<String>) -> Self {
        self.energy_direction = Some(code.into());
        self
    }

    /// Set the transformer loss factor (`SG9 CCI+++Z16`, `CAV+Z28:::{value}`).
    pub fn loss_factor_trafo(mut self, value: impl Into<String>) -> Self {
        self.loss_factor_trafo = Some(value.into());
        self
    }

    /// Set the line loss factor (`SG9 CCI+++ZB2`, `CAV+Z28:::{value}`).
    pub fn loss_factor_line(mut self, value: impl Into<String>) -> Self {
        self.loss_factor_line = Some(value.into());
        self
    }

    /// Set the energy split factor (`SG9 CCI+++ZG6`, `CAV+ZH6:::{value}`).
    pub fn split_factor(mut self, value: impl Into<String>) -> Self {
        self.split_factor = Some(value.into());
        self
    }
}

/// A time-switching / load-curve / time-registration definition block (SG8 `SEQ+Z42` etc.).
///
/// # Example
///
/// ```no_run
/// use edi_energy::builders::UtiltsDefinitionBlock;
/// let block = UtiltsDefinitionBlock::new("Z42")
///     .definition_code("ZZ1")
///     .frequency("Z33")
///     .transmissibility("Z23");
/// ```
#[derive(Debug, Clone)]
pub struct UtiltsDefinitionBlock {
    seq_qualifier: String,
    change_time: Option<String>,
    register_code: Option<String>,
    ref_definition_code: Option<String>,
    definition_code: Option<String>,
    frequency: Option<String>,
    transmissibility: Option<String>,
    peak_load_detection: Option<String>,
    orderable: Option<String>,
    switching_action: Option<String>,
}

impl UtiltsDefinitionBlock {
    /// Create a definition block.
    ///
    /// `seq_qualifier`: `Z42` (Zählzeitübersicht), `Z43` (ausgerollt), `Z41` (Register),
    /// `Z69` (Schaltzeitübersicht), `Z70`/`Z74` (Leistungskurve).
    pub fn new(seq_qualifier: impl Into<String>) -> Self {
        Self {
            seq_qualifier: seq_qualifier.into(),
            change_time: None,
            register_code: None,
            ref_definition_code: None,
            definition_code: None,
            frequency: None,
            transmissibility: None,
            peak_load_detection: None,
            orderable: None,
            switching_action: None,
        }
    }

    /// Set the change-time for this definition (DTM+Z33/Z44/Z45, format-303).
    pub fn change_time(mut self, datetime: impl Into<String>) -> Self {
        self.change_time = Some(datetime.into());
        self
    }

    /// Set the active register code (`RFF+Z28:{code}`).
    pub fn register_code(mut self, code: impl Into<String>) -> Self {
        self.register_code = Some(code.into());
        self
    }

    /// Set the parent-definition reference code (`RFF+Z28:{code}`) for Register blocks.
    pub fn ref_definition_code(mut self, code: impl Into<String>) -> Self {
        self.ref_definition_code = Some(code.into());
        self
    }

    /// Set the definition code emitted in `SG9 CCI+{type}++{code}`.
    pub fn definition_code(mut self, code: impl Into<String>) -> Self {
        self.definition_code = Some(code.into());
        self
    }

    /// Set the frequency code (`CAV+ZE0:::{code}`).  `Z33` or `Z34`.
    pub fn frequency(mut self, code: impl Into<String>) -> Self {
        self.frequency = Some(code.into());
        self
    }

    /// Set the transmissibility code (`CAV+ZD5:::{code}`).  `Z23` or `Z24`.
    pub fn transmissibility(mut self, code: impl Into<String>) -> Self {
        self.transmissibility = Some(code.into());
        self
    }

    /// Set the peak-load detection method (`CAV+ZD4:::{code}`).  `Z25` or `Z26`.
    pub fn peak_load_detection(mut self, code: impl Into<String>) -> Self {
        self.peak_load_detection = Some(code.into());
        self
    }

    /// Set the orderability code (`CAV+ZD7:::{code}`).  `Z27` or `Z28`.
    pub fn orderable(mut self, code: impl Into<String>) -> Self {
        self.orderable = Some(code.into());
        self
    }

    /// Set the switching action code for Schaltzeitdefinitionen (`SG9 CCI+Z58++{code}`).
    pub fn switching_action(mut self, code: impl Into<String>) -> Self {
        self.switching_action = Some(code.into());
        self
    }
}

// ── UtiltsVorgang ─────────────────────────────────────────────────────────────

/// A single Vorgang (SG5 transaction block) in a UTILTS message.
///
/// # Example
///
/// ```no_run
/// use edi_energy::builders::{UtiltsCalcStep, UtiltsEnergyAmountRef, UtiltsUsagePeriod, UtiltsVorgang};
/// let vorgang = UtiltsVorgang::new("V001", 25001)
///     .location("DE0000012345678901234567890123456789")
///     .formula_status("Z33", 1)
///     .add_usage_period(UtiltsUsagePeriod::new(1, "202710012200+00"))
///     .add_energy_ref(UtiltsEnergyAmountRef::new(1, 1))
///     .add_calc_step(
///         UtiltsCalcStep::new(1)
///             .time_period(1)
///             .messlokation("DE00000456789012345678900000000003D")
///             .operator("Z69")
///             .energy_direction("Z71"),
///     );
/// ```
#[derive(Debug, Clone)]
pub struct UtiltsVorgang {
    /// The Vorgangsnummer (DE 7402 in IDE+24).
    pub transaction_id: String,
    /// The Prüfidentifikator code (DE 1154 in SG6 RFF+Z13).
    pub pruefidentifikator: u32,
    location_id: Option<String>,
    definition_code: Option<String>,
    valid_from: Option<String>,
    formula_status_code: Option<String>,
    formula_status_period: Option<u32>,
    ref_transaction_id: Option<String>,
    usage_periods: Vec<UtiltsUsagePeriod>,
    energy_amount_refs: Vec<UtiltsEnergyAmountRef>,
    calc_steps: Vec<UtiltsCalcStep>,
    definition_blocks: Vec<UtiltsDefinitionBlock>,
}

impl UtiltsVorgang {
    /// Create a new Vorgang.
    ///
    /// - `transaction_id`: the Vorgangsnummer (globally unique, max 35 chars).
    /// - `pruefidentifikator`: the PID code (e.g. `25001`).
    pub fn new(transaction_id: impl Into<String>, pruefidentifikator: u32) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            pruefidentifikator,
            location_id: None,
            definition_code: None,
            valid_from: None,
            formula_status_code: None,
            formula_status_period: None,
            ref_transaction_id: None,
            usage_periods: Vec::new(),
            energy_amount_refs: Vec::new(),
            calc_steps: Vec::new(),
            definition_blocks: Vec::new(),
        }
    }

    /// Set the Meldepunkt ID for this Vorgang (`LOC+172`).
    pub fn location(mut self, malo_id: impl Into<String>) -> Self {
        self.location_id = Some(malo_id.into());
        self
    }

    /// Set the Code der Definition (`LOC+Z09+{code}`).
    pub fn definition_code(mut self, code: impl Into<String>) -> Self {
        self.definition_code = Some(code.into());
        self
    }

    /// Set the validity start time (`DTM+157:{datetime}:303`).
    pub fn valid_from(mut self, datetime: impl Into<String>) -> Self {
        self.valid_from = Some(datetime.into());
        self
    }

    /// Set the Berechnungsformel status (`STS+Z23+{code}+{period_id}`).
    pub fn formula_status(mut self, code: impl Into<String>, period_id: u32) -> Self {
        self.formula_status_code = Some(code.into());
        self.formula_status_period = Some(period_id);
        self
    }

    /// Set a reference to a previous Berechnungsformel Vorgang (`SG6 RFF+TN:{id}`).
    pub fn ref_transaction(mut self, id: impl Into<String>) -> Self {
        self.ref_transaction_id = Some(id.into());
        self
    }

    /// Add a usage-period block (SG6).
    #[must_use]
    pub fn add_usage_period(mut self, period: UtiltsUsagePeriod) -> Self {
        self.usage_periods.push(period);
        self
    }

    /// Add an energy-amount reference (SG8 `SEQ+Z36`).
    #[must_use]
    pub fn add_energy_ref(mut self, r: UtiltsEnergyAmountRef) -> Self {
        self.energy_amount_refs.push(r);
        self
    }

    /// Add a calculation-step component (SG8 `SEQ+Z37`).
    #[must_use]
    pub fn add_calc_step(mut self, step: UtiltsCalcStep) -> Self {
        self.calc_steps.push(step);
        self
    }

    /// Add a definition block (SG8 `SEQ+Z42`/`Z43`/`Z41`/`Z69`/`Z70`/`Z74`).
    #[must_use]
    pub fn add_definition_block(mut self, block: UtiltsDefinitionBlock) -> Self {
        self.definition_blocks.push(block);
        self
    }
}

// ── UtiltsBuilder ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct UtiltsBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    message_ref: String,
    document_code: String,
    document_id: Option<String>,
    document_date: Option<String>,
    vorgaenge: Vec<UtiltsVorgang>,
}

/// Fluent builder for `UTILTS` (Übertragung technischer Stammdaten) messages.
///
/// # Type-state
///
/// [`build`](UtiltsBuilder::build) is only available once both
/// [`sender`](UtiltsBuilder::sender) and [`receiver`](UtiltsBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::{UtiltsBuilder, UtiltsVorgang, UtiltsUsagePeriod,
///                             UtiltsEnergyAmountRef, UtiltsCalcStep};
///
/// let msg = UtiltsBuilder::new(Release::new("1.1e"))
///     .sender("9900259000002")
///     .receiver("9900259000001")
///     .document_id("MKIDI5422")
///     .add_vorgang(
///         UtiltsVorgang::new("V001", 25001)
///             .location("DE0000012345678901234567890123456789")
///             .formula_status("Z33", 1)
///             .add_usage_period(UtiltsUsagePeriod::new(1, "202710012200+00"))
///             .add_energy_ref(UtiltsEnergyAmountRef::new(1, 1))
///             .add_calc_step(
///                 UtiltsCalcStep::new(1)
///                     .time_period(1)
///                     .messlokation("DE00000456789012345678900000000003D")
///                     .operator("Z69")
///                     .energy_direction("Z71"),
///             ),
///     )
///     .build()?;
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct UtiltsBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: UtiltsBuilderInner,
}

impl UtiltsBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: UtiltsBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                message_ref: "1".to_owned(),
                document_code: "Z36".to_owned(),
                document_id: None,
                document_date: None,
                vorgaenge: Vec::new(),
            },
        }
    }
}

impl<S, R> UtiltsBuilder<S, R> {
    fn transition<S2, R2>(self) -> UtiltsBuilder<S2, R2> {
        UtiltsBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> UtiltsBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> UtiltsBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Set the BGM document identifier.
    pub fn document_id(mut self, id: impl Into<String>) -> Self {
        self.inner.document_id = Some(id.into());
        self
    }

    /// Override the BGM document type code.  Defaults to `"Z36"` (Berechnungsformel).
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
        self
    }

    /// Override the message reference number.  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Set the document date (`YYYYMMDD`) for DTM+137 (emitted in format 303, midnight UTC).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Set a full format-303 datetime (`CCYYMMDDHHMMZZZ`) for DTM+137.
    pub fn document_datetime(mut self, datetime: impl Into<String>) -> Self {
        self.inner.document_date = Some(format!("303:{}", datetime.into()));
        self
    }

    /// Add a Vorgang (SG5 transaction block) to the message.
    pub fn add_vorgang(mut self, vorgang: UtiltsVorgang) -> Self {
        self.inner.vorgaenge.push(vorgang);
        self
    }

    #[allow(clippy::too_many_lines)]
    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let unh_type = format!("UTILTS:D:18A:UN:{}", self.inner.release.as_str());
        let dtm_val = match self.inner.document_date.as_deref() {
            Some(d) if d.starts_with("303:") => {
                let raw = &d["303:".len()..];
                format!("137:{raw}:303")
            }
            Some(d) => fmt303("137", d),
            None => dtm_now_303(),
        };

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_seg!(w, "UNH", &self.inner.message_ref, &unh_type);
        emit_seg!(w, "BGM", &self.inner.document_code, doc_id);
        emit_seg!(w, "DTM", &dtm_val);
        if let Some(id) = &self.inner.sender_id {
            emit_seg!(w, "NAD", "MS", &format!("{id}::293"));
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_seg!(w, "NAD", "MR", &format!("{id}::293"));
        }

        for vorgang in &self.inner.vorgaenge {
            emit_seg!(w, "IDE", "24", &vorgang.transaction_id);
            if let Some(malo) = &vorgang.location_id {
                emit_seg!(w, "LOC", "172", malo);
            }
            if let Some(code) = &vorgang.definition_code {
                emit_seg!(w, "LOC", "Z09", code);
            }
            if let Some(vf) = &vorgang.valid_from {
                emit_seg!(w, "DTM", &format!("157:{vf}:303"));
            }
            if let (Some(code), Some(pid)) =
                (&vorgang.formula_status_code, &vorgang.formula_status_period)
            {
                emit_seg!(w, "STS", "Z23", code, &pid.to_string());
            }
            emit_seg!(w, "RFF", &format!("Z13:{}", vorgang.pruefidentifikator));
            if let Some(ref_id) = &vorgang.ref_transaction_id {
                emit_seg!(w, "RFF", &format!("TN:{ref_id}"));
            }
            for period in &vorgang.usage_periods {
                emit_seg!(
                    w,
                    "RFF",
                    &format!("{}::{}", period.qualifier, period.period_id)
                );
                emit_seg!(w, "DTM", &format!("Z25:{}:303", period.usage_from));
                if let Some(to) = &period.usage_to {
                    emit_seg!(w, "DTM", &format!("Z26:{to}:303"));
                }
            }
            for er in &vorgang.energy_amount_refs {
                emit_seg!(w, "SEQ", "Z36");
                emit_seg!(w, "RFF", &format!("Z46:{}", er.time_period_id));
                emit_seg!(w, "RFF", &format!("Z23:{}", er.final_step_id));
            }
            for step in &vorgang.calc_steps {
                emit_seg!(w, "SEQ", "Z37", &step.step_id.to_string());
                if let Some(tp) = step.time_period_id {
                    emit_seg!(w, "RFF", &format!("Z46:{tp}"));
                }
                if let Some(malo) = &step.messlokation_id {
                    emit_seg!(w, "RFF", &format!("Z19:{malo}"));
                }
                if let Some(rs) = step.ref_calc_step_id {
                    emit_seg!(w, "RFF", &format!("Z23:{rs}"));
                }
                if let Some(op) = &step.operator {
                    emit_seg!(w, "CCI", "", "", "Z86");
                    emit_seg!(w, "CAV", op);
                }
                if let Some(dir) = &step.energy_direction {
                    emit_seg!(w, "CCI", "", "", "Z87");
                    emit_seg!(w, "CAV", dir);
                }
                if let Some(vt) = &step.loss_factor_trafo {
                    emit_seg!(w, "CCI", "", "", "Z16");
                    emit_seg!(w, "CAV", &format!("Z28:::{vt}"));
                }
                if let Some(vl) = &step.loss_factor_line {
                    emit_seg!(w, "CCI", "", "", "ZB2");
                    emit_seg!(w, "CAV", &format!("Z28:::{vl}"));
                }
                if let Some(af) = &step.split_factor {
                    emit_seg!(w, "CCI", "", "", "ZG6");
                    emit_seg!(w, "CAV", &format!("ZH6:::{af}"));
                }
            }
            for block in &vorgang.definition_blocks {
                emit_seg!(w, "SEQ", &block.seq_qualifier);
                if let Some(ct) = &block.change_time {
                    let dtm_q = match block.seq_qualifier.as_str() {
                        "Z69" => "Z44",
                        "Z70" | "Z74" => "Z45",
                        _ => "Z33",
                    };
                    emit_seg!(w, "DTM", &format!("{dtm_q}:{ct}:303"));
                }
                if let Some(rc) = &block.register_code {
                    emit_seg!(w, "RFF", &format!("Z28:{rc}"));
                }
                if let Some(rc) = &block.ref_definition_code {
                    emit_seg!(w, "RFF", &format!("Z28:{rc}"));
                }
                if let Some(def_code) = &block.definition_code {
                    let cci_type = match block.seq_qualifier.as_str() {
                        "Z69" => "Z52",
                        "Z70" | "Z74" => "Z53",
                        _ => "Z39",
                    };
                    emit_seg!(w, "CCI", cci_type, "", def_code);
                    if let Some(freq) = &block.frequency {
                        emit_seg!(w, "CAV", &format!("ZE0:::{freq}"));
                    }
                    if let Some(tr) = &block.transmissibility {
                        emit_seg!(w, "CAV", &format!("ZD5:::{tr}"));
                    }
                    if let Some(pl) = &block.peak_load_detection {
                        emit_seg!(w, "CAV", &format!("ZD4:::{pl}"));
                    }
                    if let Some(ord) = &block.orderable {
                        emit_seg!(w, "CAV", &format!("ZD7:::{ord}"));
                    }
                }
                if let Some(sa) = &block.switching_action {
                    emit_seg!(w, "CCI", "Z58", "", sa);
                }
            }
        }

        w.finish_unt(&self.inner.message_ref)
            .map_err(Error::Parse)?;
        Ok(buf)
    }
    /// Build and serialize the message to EDIFACT bytes.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if serialization fails.
    pub fn serialize(self) -> Result<Vec<u8>, Error> {
        self.to_bytes()
    }
}

impl UtiltsBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::utilts::UtiltsMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::utilts::UtiltsMessage, Error> {
        let pid = self.inner.vorgaenge.first().map(|v| v.pruefidentifikator);
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::utilts::UtiltsMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}
