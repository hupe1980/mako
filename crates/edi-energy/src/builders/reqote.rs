//! [`ReqoteBuilder`] — fluent type-safe builder for REQOTE messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments, today_ccyymmdd};

#[derive(Debug, Clone)]
struct ReqoteBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: Option<String>,
    document_id: Option<String>,
    document_date: Option<String>,
    location: Option<String>,
    // Additive ESA-Werteanfrage (PID 35002) content — only emitted when set.
    reference: Option<(String, String)>,
    contact: Option<(String, String)>,
    line_item: bool,
}

/// Fluent builder for `REQOTE` (Request for Quotation) messages.
///
/// Wire type string: `REQOTE:D:10A:UN:{release}`.
///
/// # Type-state
///
/// [`build`](ReqoteBuilder::build) is only available once both
/// [`sender`](ReqoteBuilder::sender) and [`receiver`](ReqoteBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::ReqoteBuilder;
///
/// let msg = ReqoteBuilder::new(Release::new("1.3c"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("4012345000023"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct ReqoteBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: ReqoteBuilderInner,
}

impl ReqoteBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: ReqoteBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: None,
                document_id: None,
                document_date: None,
                location: None,
                reference: None,
                contact: None,
                line_item: false,
            },
        }
    }
}

impl<S, R> ReqoteBuilder<S, R> {
    fn transition<S2, R2>(self) -> ReqoteBuilder<S2, R2> {
        ReqoteBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> ReqoteBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> ReqoteBuilder<S, Set> {
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

    /// Override the BGM document type code.  Defaults to `"311"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = Some(code.into());
        self
    }

    /// Set the BGM document identifier.
    pub fn document_id(mut self, id: impl Into<String>) -> Self {
        self.inner.document_id = Some(id.into());
        self
    }

    /// Override the message reference number. Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Set the location (MaLo-ID / ZPB / NeLo-ID) the request addresses.
    ///
    /// Emits `LOC+172+<id>`. The ESA Werteanfrage (`WiM` Teil 2 UC 4.1 Nr. 1)
    /// names the location the values are requested for.
    pub fn location(mut self, id: impl Into<String>) -> Self {
        self.inner.location = Some(id.into());
        self
    }

    /// Add an SG1 reference `RFF+<qual>:<value>` (REQOTE `1153 ∈ {Z13,AGO,AEP,AGK}`).
    pub fn reference(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner.reference = Some((qualifier.into(), value.into()));
        self
    }

    /// Set the SG14 contact — emits `CTA+IC+:<name>` and `COM+<comm>:EM`.
    pub fn contact(mut self, name: impl Into<String>, comm: impl Into<String>) -> Self {
        self.inner.contact = Some((name.into(), comm.into()));
        self
    }

    /// Emit a `LIN+1` line item (SG27) — the AHB requires one for PID 35002.
    pub fn line_item(mut self) -> Self {
        self.inner.line_item = true;
        self
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(today_ccyymmdd, str::to_owned);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        let code = self.inner.document_code.as_deref().unwrap_or("311");
        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["REQOTE", "D", "10A", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", code, doc_id);
        emit_comp!(w, "DTM", ["137", &dtm_val, "102"]);
        // ── SG1: reference (RFF+Z13 = Prüfidentifikator) ─────────────────────
        if let Some((q, v)) = &self.inner.reference {
            emit_comp!(w, "RFF", [q, v]);
        }
        // ── SG11: parties ────────────────────────────────────────────────────
        if let Some(id) = &self.inner.sender_id {
            emit_comp!(
                w,
                "NAD",
                ["MS"],
                [id, "", self.inner.sender_agency.as_str()]
            );
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_comp!(
                w,
                "NAD",
                ["MR"],
                [id, "", self.inner.receiver_agency.as_str()]
            );
        }
        // ── SG14: contact — before LOC per the REQOTE segment order ──────────
        if let Some((name, comm)) = &self.inner.contact {
            emit_comp!(w, "CTA", ["IC"], ["", name]);
            emit_comp!(w, "COM", [comm, "EM"]);
        }
        if let Some(loc) = &self.inner.location {
            emit_seg!(w, "LOC", "172", loc);
        }
        // ── SG27: line item ──────────────────────────────────────────────────
        if self.inner.line_item {
            emit_seg!(w, "LIN", "1");
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

impl ReqoteBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::reqote::ReqoteMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::reqote::ReqoteMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::reqote::ReqoteMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
