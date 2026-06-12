//! [`AperakBuilder`] — fluent type-safe builder for APERAK messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::{Error, Pruefidentifikator, Release};

use super::{Set, Unset, bytes_to_segments, dtm_today, format_dtm137};

macro_rules! emit_seg {
    ($writer:expr, $tag:expr, $($elem:expr),+ $(,)?) => {{
        let elements: &[&str] = &[$($elem),+];
        $writer.write_raw($tag, elements).map_err(|e| Error::Parse(e.into()))?;
    }};
}

#[derive(Debug, Clone)]
struct AperakBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    message_ref: String,
    document_code: String,
    document_id: Option<String>,
    acw_ref: Option<String>,
    error_code: Option<String>,
    error_text: Option<String>,
    document_date: Option<String>,
}

/// Fluent builder for `APERAK` (Application Error and Acknowledgement) messages.
///
/// # Type-state
///
/// [`build`](AperakBuilder::build) is only available once both
/// [`sender`](AperakBuilder::sender) and [`receiver`](AperakBuilder::receiver)
/// have been called.
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct AperakBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: AperakBuilderInner,
}

impl AperakBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: AperakBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                message_ref: "1".to_owned(),
                document_code: "1000".to_owned(),
                document_id: None,
                acw_ref: None,
                error_code: None,
                error_text: None,
                document_date: None,
            },
        }
    }
}

impl<S, R> AperakBuilder<S, R> {
    fn transition<S2, R2>(self) -> AperakBuilder<S2, R2> {
        AperakBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> AperakBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> AperakBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Set the Prüfidentifikator (BGM document identifier).
    pub fn pruefidentifikator(mut self, pid: Pruefidentifikator) -> Self {
        self.inner.document_id = Some(pid.as_u32().to_string());
        self
    }

    /// Set the acknowledgement reference number (RFF+ACW).
    pub fn acw_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.acw_ref = Some(reference.into());
        self
    }

    /// Set an application error code (ERC segment).
    pub fn error_code(mut self, code: impl Into<String>) -> Self {
        self.inner.error_code = Some(code.into());
        self
    }

    /// Set a free-text error description (FTX+AAI).
    pub fn error_text(mut self, text: impl Into<String>) -> Self {
        self.inner.error_text = Some(text.into());
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

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let unh_type = format!("APERAK:D:07B:UN:{}", self.inner.release.as_str());
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(dtm_today, format_dtm137);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_seg!(w, "UNH", &self.inner.message_ref, &unh_type);
        emit_seg!(w, "BGM", &self.inner.document_code, doc_id, "9");
        emit_seg!(w, "DTM", &dtm_val);
        if let Some(id) = &self.inner.sender_id {
            emit_seg!(w, "NAD", "MS", &format!("{id}::293"));
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_seg!(w, "NAD", "MR", &format!("{id}::293"));
        }
        if let Some(r) = &self.inner.acw_ref {
            emit_seg!(w, "RFF", &format!("ACW:{r}"));
        }
        if let Some(code) = &self.inner.error_code {
            emit_seg!(w, "ERC", code);
        }
        if let Some(text) = &self.inner.error_text {
            emit_seg!(w, "FTX", "AAI", "", "", text);
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

impl AperakBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::aperak::AperakMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::aperak::AperakMessage, Error> {
        let pid = self
            .inner
            .document_id
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok());
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::aperak::AperakMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}
