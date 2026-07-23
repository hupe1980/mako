//! [`OrdersBuilder`] — fluent type-safe builder for ORDERS messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments, today_ccyymmdd};

#[derive(Debug, Clone)]
struct OrdersBuilderInner {
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
    // Additive ESA-Bestellung/Abbestellung (PID 17007/17008) content.
    reference: Option<(String, String)>,
    item_description: Option<String>,
}

/// Fluent builder for `ORDERS` (Purchase Order) messages.
///
/// Wire type string: `ORDERS:D:09B:UN:{release}`.
///
/// # Type-state
///
/// [`build`](OrdersBuilder::build) is only available once both
/// [`sender`](OrdersBuilder::sender) and [`receiver`](OrdersBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::OrdersBuilder;
///
/// let msg = OrdersBuilder::new(Release::new("1.4b"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("4012345000023"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct OrdersBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: OrdersBuilderInner,
}

impl OrdersBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: OrdersBuilderInner {
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
                item_description: None,
            },
        }
    }
}

impl<S, R> OrdersBuilder<S, R> {
    fn transition<S2, R2>(self) -> OrdersBuilder<S2, R2> {
        OrdersBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> OrdersBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> OrdersBuilder<S, Set> {
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

    /// Override the BGM document type code (DE 1001).
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = Some(code.into());
        self
    }

    /// Set the BGM document identifier.
    pub fn document_id(mut self, id: impl Into<String>) -> Self {
        self.inner.document_id = Some(id.into());
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

    /// Set the location (MaLo-ID / ZPB / NeLo-ID) the order addresses.
    ///
    /// Emits `LOC+172+<id>`. An ESA Bestellung/Abbestellung (`WiM` Teil 2 UC 4.1
    /// Nr. 3 / UC 4.3 Nr. 1) names the location whose values are (un)ordered.
    pub fn location(mut self, id: impl Into<String>) -> Self {
        self.inner.location = Some(id.into());
        self
    }

    /// Add the SG1 reference `RFF+<qual>:<value>` (ORDERS SG1 `1153 = Z13`).
    pub fn reference(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner.reference = Some((qualifier.into(), value.into()));
        self
    }

    /// Set a coded item description — emits `IMD+A++:::<text>`.
    pub fn item_description(mut self, text: impl Into<String>) -> Self {
        self.inner.item_description = Some(text.into());
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

        let code = self.inner.document_code.as_deref().unwrap_or("");
        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["ORDERS", "D", "09B", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", code, doc_id);
        emit_comp!(w, "DTM", ["137", &dtm_val, "102"]);
        // Item description (IMD) — the AHB requires the segment; the free-form
        // indicator (7077 = A) satisfies it. `_text` is reserved for a future
        // coded C273 once the 7081 characteristic code list is wired.
        if let Some(_text) = &self.inner.item_description {
            emit_comp!(w, "IMD", ["A"]);
        }
        // ── SG1: reference (RFF+Z13 = Prüfidentifikator) ─────────────────────
        if let Some((q, v)) = &self.inner.reference {
            emit_comp!(w, "RFF", [q, v]);
        }
        // ── SG2: parties + location ──────────────────────────────────────────
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
        if let Some(loc) = &self.inner.location {
            emit_seg!(w, "LOC", "172", loc);
        }
        // Section control — ORDERS requires UNS between header and summary.
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

impl OrdersBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::orders::OrdersMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::orders::OrdersMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::orders::OrdersMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
