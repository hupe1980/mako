//! [`QuotesBuilder`] — fluent type-safe builder for QUOTES messages.

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
struct QuotesBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    message_ref: String,
    document_id: Option<String>,
    document_date: Option<String>,
}

/// Fluent builder for `QUOTES` (Quotation) messages.
///
/// Wire type string: `QUOTES:D:10A:UN:{release}`.
///
/// # Type-state
///
/// [`build`](QuotesBuilder::build) is only available once both
/// [`sender`](QuotesBuilder::sender) and [`receiver`](QuotesBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::QuotesBuilder;
///
/// let msg = QuotesBuilder::new(Release::new("1.3b"))
///     .sender("9900357000004")
///     .receiver("4012345000023")
///     .document_id("QUOTES20250401001")
///     .build()?;
///
/// assert_eq!(msg.assoc_code(), "1.3b");
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct QuotesBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: QuotesBuilderInner,
}

impl QuotesBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: QuotesBuilderInner {
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

impl<S, R> QuotesBuilder<S, R> {
    fn transition<S2, R2>(self) -> QuotesBuilder<S2, R2> {
        QuotesBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> QuotesBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> QuotesBuilder<S, Set> {
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
        let unh_type = format!("QUOTES:D:10A:UN:{}", self.inner.release.as_str());
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(dtm_today, format_dtm137);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_seg!(w, "UNH", &self.inner.message_ref, &unh_type);
        emit_seg!(w, "BGM", "310", doc_id);
        emit_seg!(w, "DTM", &dtm_val);
        if let Some(id) = &self.inner.sender_id {
            emit_seg!(w, "NAD", "MS", &format!("{id}::293"));
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_seg!(w, "NAD", "MR", &format!("{id}::293"));
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

impl QuotesBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::quotes::QuotesMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::quotes::QuotesMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::quotes::QuotesMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
