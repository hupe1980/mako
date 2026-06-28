//! [`PartinBuilder`] — fluent type-safe builder for PARTIN messages.

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
struct PartinBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: Option<String>,
    document_id: Option<String>,
    document_date: Option<String>,
}

/// Fluent builder for `PARTIN` (Party Information) messages.
///
/// Wire type string: `PARTIN:D:20B:UN:{release}`.
///
/// # Type-state
///
/// [`build`](PartinBuilder::build) is only available once both
/// [`sender`](PartinBuilder::sender) and [`receiver`](PartinBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::PartinBuilder;
///
/// let msg = PartinBuilder::new(Release::new("1.0f"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("4012345000023"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct PartinBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: PartinBuilderInner,
}

impl PartinBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: PartinBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: None,
                document_id: None,
                document_date: None,
            },
        }
    }
}

impl<S, R> PartinBuilder<S, R> {
    fn transition<S2, R2>(self) -> PartinBuilder<S2, R2> {
        PartinBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> PartinBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> PartinBuilder<S, Set> {
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

    /// Override the BGM document type code (DE 1001).  Defaults to `"35"`.
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

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let unh_type = format!("PARTIN:D:20B:UN:{}", self.inner.release.as_str());
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(dtm_today, format_dtm137);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        let code = self.inner.document_code.as_deref().unwrap_or("35");
        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_seg!(w, "UNH", &self.inner.message_ref, &unh_type);
        emit_seg!(w, "BGM", code, doc_id);
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

impl PartinBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::partin::PartinMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::partin::PartinMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::partin::PartinMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
