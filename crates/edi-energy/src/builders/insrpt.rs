//! [`InsrptBuilder`] — fluent type-safe builder for INSRPT messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments, today_ccyymmdd};

#[derive(Debug, Clone)]
struct InsrptBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: String,
    document_id: Option<String>,
    document_date: Option<String>,
    /// SG3 `DOC` — Referenz auf das Dokument (qualifier, id).
    doc_reference: Option<(String, String)>,
    /// SG4 `RFF+Z13` — Prüfidentifikator.
    pruefidentifikator: Option<u32>,
    /// SG7 `LIN` — Positionsnummer.
    position: Option<String>,
    /// SG7 `STS` — Statuscode.
    status: Option<String>,
    /// SG8 `LOC` — addressed location (qualifier, id).
    location: Option<(String, String)>,
}

/// Fluent builder for `INSRPT` (Inspection Report) messages.
///
/// # Type-state
///
/// [`build`](InsrptBuilder::build) is only available once both
/// [`sender`](InsrptBuilder::sender) and [`receiver`](InsrptBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::InsrptBuilder;
///
/// let msg = InsrptBuilder::new(Release::new("1.1a"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .document_id("BEP00021000")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("4012345000023"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct InsrptBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: InsrptBuilderInner,
}

impl InsrptBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: InsrptBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: "4".to_owned(),
                document_id: None,
                document_date: None,
                doc_reference: None,
                pruefidentifikator: None,
                position: None,
                status: None,
                location: None,
            },
        }
    }
}

impl<S, R> InsrptBuilder<S, R> {
    fn transition<S2, R2>(self) -> InsrptBuilder<S2, R2> {
        InsrptBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> InsrptBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> InsrptBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Override the agency code for the sender's party identifier.
    ///
    /// Default: [`AgencyCode::Bdew`] (`"293"`). Use [`AgencyCode::Etso`] (`"305"`)
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

    /// Set the BGM document identifier.
    pub fn document_id(mut self, id: impl Into<String>) -> Self {
        self.inner.document_id = Some(id.into());
        self
    }

    /// Override the BGM document type code.  Defaults to `"4"` (Prüfbericht).
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
        self
    }

    /// Override the message reference number.  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Set the SG3 `DOC` Dokumentenreferenz (e.g. `Z41` + Förderreferenz).
    ///
    /// `DOC` is **mandatory** in the INSRPT AHB for every Prüfidentifikator;
    /// a message without it does not validate.
    pub fn doc_reference(mut self, qualifier: impl Into<String>, id: impl Into<String>) -> Self {
        self.inner.doc_reference = Some((qualifier.into(), id.into()));
        self
    }

    /// Set the SG4 `RFF+Z13` Prüfidentifikator.
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Set the SG7 `LIN` Positionsnummer (defaults to `1` when a position is
    /// required but unset).
    pub fn position(mut self, position: impl Into<String>) -> Self {
        self.inner.position = Some(position.into());
        self
    }

    /// Set the SG7 `STS` Statuscode (e.g. `Z01`).
    pub fn status(mut self, code: impl Into<String>) -> Self {
        self.inner.status = Some(code.into());
        self
    }

    /// Set the SG8 `LOC` addressed location (e.g. `172` + `MeLo`-ID).
    pub fn location(mut self, qualifier: impl Into<String>, id: impl Into<String>) -> Self {
        self.inner.location = Some((qualifier.into(), id.into()));
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
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

        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["INSRPT", "D", "96A", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", &self.inner.document_code, doc_id);
        emit_comp!(w, "DTM", ["137", &dtm_val, "102"]);
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
        // MIG order: SG3 DOC → SG4 RFF → SG7 LIN → STS → SG8 LOC.
        if let Some((qual, id)) = &self.inner.doc_reference {
            emit_seg!(w, "DOC", qual, id);
        }
        if let Some(pid) = self.inner.pruefidentifikator {
            emit_comp!(w, "RFF", ["Z13", &pid.to_string()]);
        }
        if let Some(pos) = &self.inner.position {
            emit_seg!(w, "LIN", pos);
        }
        if let Some(code) = &self.inner.status {
            emit_seg!(w, "STS", code);
        }
        if let Some((qual, id)) = &self.inner.location {
            emit_comp!(w, "LOC", [qual], [id]);
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

impl InsrptBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::insrpt::InsrptMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::insrpt::InsrptMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::insrpt::InsrptMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
