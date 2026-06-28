//! [`MsconsBuilder`] — fluent type-safe builder for MSCONS messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Pruefidentifikator, Release};

use super::{Set, Unset, bytes_to_segments, dtm_today, format_dtm137};

macro_rules! emit_seg {
    ($writer:expr, $tag:expr, $($elem:expr),+ $(,)?) => {{
        let elements: &[&str] = &[$($elem),+];
        $writer.write_raw($tag, elements).map_err(|e| Error::Parse(e.into()))?;
    }};
}

// ── Inner spec structs ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct QuantitySpec {
    qualifier: String,
    value: String,
    unit: String,
}

#[derive(Debug, Clone)]
struct LineItemSpec {
    line_number: usize,
    obis_code: Option<String>,
    quantities: Vec<QuantitySpec>,
}

#[derive(Debug, Clone)]
struct MeteringPointSpec {
    malo_id: String,
    location_id: Option<String>,
    line_items: Vec<LineItemSpec>,
}

#[derive(Debug, Clone)]
struct MsconsBuilderInner {
    release: Release,
    pruefidentifikator: Option<Pruefidentifikator>,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: String,
    document_date: Option<String>,
    header_references: Vec<(String, String)>,
    metering_points: Vec<MeteringPointSpec>,
}

// ── MsconsBuilder ─────────────────────────────────────────────────────────────

/// Fluent builder for `MSCONS` (Metered Services Consumption Report) messages.
///
/// # Type-state
///
/// [`build`](MsconsBuilder::build) is only available once both
/// [`sender`](MsconsBuilder::sender) and [`receiver`](MsconsBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::{Release, Pruefidentifikator};
/// use edi_energy::builders::MsconsBuilder;
///
/// let msg = MsconsBuilder::new(Release::new("2.4c"))
///     .pruefidentifikator(Pruefidentifikator::new(13002).unwrap())
///     .sender("9900111222333")
///     .receiver("9900444555666")
///     .metering_point("DE0001234567890")
///         .location_id("12345678901")
///         .quantity("220", "1000.500", "KWH")
///     .done()
///     .build()?;
///
/// assert_eq!(msg.delivery_points().len(), 1);
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct MsconsBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: MsconsBuilderInner,
}

impl MsconsBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: MsconsBuilderInner {
                release,
                pruefidentifikator: None,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: "7".to_owned(),
                document_date: None,
                header_references: Vec::new(),
                metering_points: Vec::new(),
            },
        }
    }
}

impl<S, R> MsconsBuilder<S, R> {
    fn transition<S2, R2>(self) -> MsconsBuilder<S2, R2> {
        MsconsBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> MsconsBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> MsconsBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Override the agency code for the sender's party identifier.
    ///
    /// Default: [`AgencyCode::Bdew`] (`"293"`). Use [`AgencyCode::Entso`] (`"305"`)
    /// for TSO/ÜNB parties that carry a 16-char EIC code.
    pub fn sender_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.sender_agency = agency;
        self
    }

    /// Override the agency code for the receiver's party identifier.
    ///
    /// Default: [`AgencyCode::Bdew`] (`"293"`).
    pub fn receiver_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.receiver_agency = agency;
        self
    }

    /// Set the Pruefidentifikator (e.g. `21001`).
    pub fn pruefidentifikator(mut self, pid: Pruefidentifikator) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Override the message reference number.  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Override the BGM document name code (DE 1001).  Defaults to `"7"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Add a header-section reference (RFF segment) before NAD.
    pub fn header_reference(
        mut self,
        qualifier: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.inner
            .header_references
            .push((qualifier.into(), value.into()));
        self
    }

    /// Start configuring a metering-point (NAD+DP) block in the detail section.
    ///
    /// Returns a [`MeteringPointBuilder`] sub-builder. Call
    /// [`done`](MeteringPointBuilder::done) to return to this builder.
    pub fn metering_point(self, malo_id: impl Into<String>) -> MeteringPointBuilder<S, R> {
        MeteringPointBuilder {
            parent: self,
            spec: MeteringPointSpec {
                malo_id: malo_id.into(),
                location_id: None,
                line_items: Vec::new(),
            },
            current_item: None,
        }
    }
}

impl<S, R> MsconsBuilder<S, R> {
    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let pid_str = self
            .inner
            .pruefidentifikator
            .map(|p| format!("{:05}", p.as_u32()))
            .unwrap_or_default();
        let unh_type = format!("MSCONS:D:04B:UN:{}", self.inner.release.as_str());
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(dtm_today, format_dtm137);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        emit_seg!(w, "UNH", &self.inner.message_ref, &unh_type);
        emit_seg!(w, "BGM", &self.inner.document_code, &pid_str, "9");
        emit_seg!(w, "DTM", &dtm_val);
        for (qualifier, value) in &self.inner.header_references {
            emit_seg!(w, "RFF", &format!("{qualifier}:{value}"));
        }
        if let Some(id) = &self.inner.sender_id {
            emit_seg!(
                w,
                "NAD",
                "MS",
                &self.inner.sender_agency.format_nad_c082(id)
            );
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_seg!(
                w,
                "NAD",
                "MR",
                &self.inner.receiver_agency.format_nad_c082(id)
            );
        }
        if !self.inner.metering_points.is_empty() {
            emit_seg!(w, "UNS", "D");
            for mp in &self.inner.metering_points {
                emit_seg!(w, "NAD", "DP", &format!("{}::293", mp.malo_id));
                if !mp.line_items.is_empty() {
                    if let Some(loc_id) = &mp.location_id {
                        emit_seg!(w, "LOC", "172", loc_id);
                    }
                    for item in &mp.line_items {
                        let ln = item.line_number.to_string();
                        emit_seg!(w, "LIN", &ln);
                        if let Some(obis) = &item.obis_code {
                            emit_seg!(w, "PIA", "5", obis, "Z12");
                        }
                        for qty in &item.quantities {
                            let qty_val = format!("{}:{}:{}", qty.qualifier, qty.value, qty.unit);
                            emit_seg!(w, "QTY", &qty_val);
                        }
                    }
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

impl MsconsBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::mscons::MsconsMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::mscons::MsconsMessage, Error> {
        let pid = self
            .inner
            .pruefidentifikator
            .map(super::super::pruefidentifikator::Pruefidentifikator::as_u32);
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::mscons::MsconsMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}

// ── MeteringPointBuilder ──────────────────────────────────────────────────────

/// Sub-builder for a metering-point (NAD+DP) block in an MSCONS message.
///
/// Obtained via [`MsconsBuilder::metering_point`]. Call
/// [`done`](MeteringPointBuilder::done) to finalize and return to the parent.
#[derive(Debug)]
#[must_use = "Sub-builder must be finalized with .done()"]
pub struct MeteringPointBuilder<S = Unset, R = Unset> {
    parent: MsconsBuilder<S, R>,
    spec: MeteringPointSpec,
    current_item: Option<LineItemSpec>,
}

impl<S, R> MeteringPointBuilder<S, R> {
    /// Set the grid location / Messlokation ID (LOC+172).
    pub fn location_id(mut self, id: impl Into<String>) -> Self {
        self.flush_item();
        self.spec.location_id = Some(id.into());
        self
    }

    /// Set the OBIS code (PIA+5) for the current line item.
    ///
    /// A new line item is created automatically if none is in progress.
    pub fn obis(mut self, code: impl Into<String>) -> Self {
        self.current_item
            .get_or_insert_with(|| LineItemSpec {
                line_number: self.spec.line_items.len() + 1,
                obis_code: None,
                quantities: Vec::new(),
            })
            .obis_code = Some(code.into());
        self
    }

    /// Start a new line item (LIN segment) with an optional OBIS code (PIA).
    pub fn line_item(mut self, obis_code: impl Into<String>) -> Self {
        self.flush_item();
        let idx = self.spec.line_items.len() + 1;
        self.current_item = Some(LineItemSpec {
            line_number: idx,
            obis_code: Some(obis_code.into()),
            quantities: Vec::new(),
        });
        self
    }

    /// Finalize the current line item and start a new one.
    pub fn next_line_item(mut self) -> Self {
        self.flush_item();
        self
    }

    /// Add a quantity (QTY segment) to the current line item.
    ///
    /// If no line item is active, starts a new one without an OBIS code.
    ///
    /// `qualifier` — DE 6063 (e.g. `"220"` for metered quantity).
    /// `value` — numeric string (e.g. `"1000.500"`).
    /// `unit` — DE 6411 (e.g. `"KWH"`).
    pub fn quantity(
        mut self,
        qualifier: impl Into<String>,
        value: impl Into<String>,
        unit: impl Into<String>,
    ) -> Self {
        if self.current_item.is_none() {
            let idx = self.spec.line_items.len() + 1;
            self.current_item = Some(LineItemSpec {
                line_number: idx,
                obis_code: None,
                quantities: Vec::new(),
            });
        }
        if let Some(item) = &mut self.current_item {
            item.quantities.push(QuantitySpec {
                qualifier: qualifier.into(),
                value: value.into(),
                unit: unit.into(),
            });
        }
        self
    }

    fn flush_item(&mut self) {
        if let Some(item) = self.current_item.take() {
            self.spec.line_items.push(item);
        }
    }

    /// Finalize this metering-point block and return to the parent [`MsconsBuilder`].
    pub fn done(mut self) -> MsconsBuilder<S, R> {
        self.flush_item();
        self.parent.inner.metering_points.push(self.spec);
        self.parent
    }
}
