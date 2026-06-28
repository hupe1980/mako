//! [`ComdisBuilder`] — fluent type-safe builder for COMDIS messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments, dtm_today, format_dtm137};

macro_rules! emit_seg {
    ($writer:expr, $tag:expr, $($elem:expr),+ $(,)?) => {{
        let elements: &[&str] = &[$($elem),+];
        $writer.write_raw($tag, elements).map_err(|e| Error::Parse(e.into()))?;
    }};
}

#[derive(Debug, Clone)]
struct ComdisBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: String,
    document_id: Option<String>,
    document_date: Option<String>,
    pruefidentifikator: Option<u32>,
    rejected_docs: Vec<(String, String, String)>,
}

/// Fluent builder for `COMDIS` (Commercial Dispute) messages.
///
/// # Type-state
///
/// [`build`](ComdisBuilder::build) is only available once both
/// [`sender`](ComdisBuilder::sender) and [`receiver`](ComdisBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::ComdisBuilder;
///
/// let msg = ComdisBuilder::new(Release::new("1.0g"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .pruefidentifikator(29001)
///     .document_id("ABL00021000")
///     .reject_document("380", "REMADV-REF-001", "E_0265")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("4012345000023"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct ComdisBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: ComdisBuilderInner,
}

impl ComdisBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: ComdisBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: "456".to_owned(),
                document_id: None,
                document_date: None,
                pruefidentifikator: None,
                rejected_docs: Vec::new(),
            },
        }
    }
}

impl<S, R> ComdisBuilder<S, R> {
    fn transition<S2, R2>(self) -> ComdisBuilder<S2, R2> {
        ComdisBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> ComdisBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> ComdisBuilder<S, Set> {
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

    /// Set the BGM document identifier.
    pub fn document_id(mut self, id: impl Into<String>) -> Self {
        self.inner.document_id = Some(id.into());
        self
    }

    /// Override the BGM document type code.  Defaults to `"456"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
        self
    }

    /// Override the message reference number.  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Set the Prüfidentifikator (emitted as `RFF+Z13:{pid}`).
    ///
    /// Use `29001` for Ablehnung REMADV and `29002` for Ablehnung IFTSTA.
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Add a rejected document (DOC + AJT segments in the SG2 body).
    ///
    /// - `doc_type`: document type code (DE 1001), e.g. `"380"` (Rechnung).
    /// - `doc_ref`: reference number of the rejected document.
    /// - `ajt_reason`: AJT reason code (DE 4465), e.g. `"E_0265"`.
    pub fn reject_document(
        mut self,
        doc_type: impl Into<String>,
        doc_ref: impl Into<String>,
        ajt_reason: impl Into<String>,
    ) -> Self {
        self.inner
            .rejected_docs
            .push((doc_type.into(), doc_ref.into(), ajt_reason.into()));
        self
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let unh_type = format!("COMDIS:D:17A:UN:{}", self.inner.release.as_str());
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(dtm_today, format_dtm137);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_seg!(w, "UNH", &self.inner.message_ref, &unh_type);
        emit_seg!(w, "BGM", &self.inner.document_code, doc_id);
        if let Some(pid) = self.inner.pruefidentifikator {
            emit_seg!(w, "RFF", &format!("Z13:{pid}"));
        }
        emit_seg!(w, "DTM", &dtm_val);
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
        for (doc_type, doc_ref, ajt_reason) in &self.inner.rejected_docs {
            emit_seg!(w, "DOC", doc_type, doc_ref);
            emit_seg!(w, "AJT", ajt_reason);
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

impl ComdisBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::comdis::ComdisMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::comdis::ComdisMessage, Error> {
        let pid = self.inner.pruefidentifikator;
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::comdis::ComdisMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}
