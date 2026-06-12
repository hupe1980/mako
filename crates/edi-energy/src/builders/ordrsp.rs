//! [`OrdrespBuilder`] — fluent type-safe builder for ORDRSP messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments, dtm_today, format_dtm137};

macro_rules! emit_seg {
    ($writer:expr, $tag:expr, $($elem:expr),+ $(,)?) => {{
        let elements: &[&str] = &[$($elem),+];
        $writer.write_raw($tag, elements).map_err(|e| Error::Parse(e.into()))?;
    }};
}

#[derive(Debug, Clone)]
struct OrdrespBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    message_ref: String,
    document_id: Option<String>,
    document_date: Option<String>,
}

/// Fluent builder for `ORDRSP` (Purchase Order Response) messages.
///
/// Wire type string: `ORDRSP:D:10A:UN:{release}`.
///
/// # Type-state
///
/// [`build`](OrdrespBuilder::build) is only available once both
/// [`sender`](OrdrespBuilder::sender) and [`receiver`](OrdrespBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::OrdrespBuilder;
///
/// let msg = OrdrespBuilder::new(Release::new("1.4b"))
///     .sender("9900357000004")
///     .receiver("4012345000023")
///     .document_id("ORDRSP20251001001")
///     .build()?;
///
/// assert_eq!(msg.assoc_code(), "1.4b");
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct OrdrespBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: OrdrespBuilderInner,
}

impl OrdrespBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: OrdrespBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                message_ref: "1".to_owned(),
                document_id: None,
                document_date: None,
            },
        }
    }
}

impl<S, R> OrdrespBuilder<S, R> {
    fn transition<S2, R2>(self) -> OrdrespBuilder<S2, R2> {
        OrdrespBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> OrdrespBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> OrdrespBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
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
        let unh_type = format!("ORDRSP:D:10A:UN:{}", self.inner.release.as_str());
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(dtm_today, format_dtm137);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_seg!(w, "UNH", &self.inner.message_ref, &unh_type);
        emit_seg!(w, "BGM", "7", doc_id);
        emit_seg!(w, "DTM", &dtm_val);
        if let Some(id) = &self.inner.sender_id {
            emit_seg!(w, "NAD", "MS", &format!("{id}::293"));
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_seg!(w, "NAD", "MR", &format!("{id}::293"));
        }
        emit_seg!(w, "UNS", "D");
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

impl OrdrespBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::ordrsp::OrdrespMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::ordrsp::OrdrespMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::ordrsp::OrdrespMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
