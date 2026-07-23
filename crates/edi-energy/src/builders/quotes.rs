//! [`QuotesBuilder`] — fluent type-safe builder for QUOTES messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments, today_ccyymmdd};

#[derive(Debug, Clone)]
struct QuotesBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_id: Option<String>,
    document_date: Option<String>,
    location: Option<String>,
    pruefidentifikator: Option<u32>,
    order_reference: Option<String>,
    bindungsfrist: Option<String>,
    reason: Option<String>,
    // Additive ESA-Angebot (PID 15003) content — only emitted when set, so the
    // Geräteübernahme Angebote (15001/15002) that share this builder are unaffected.
    reference: Option<(String, String)>,
    currency: Option<String>,
    contact: Option<(String, String)>,
    product: Option<String>,
    price: Option<String>,
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
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_id: None,
                document_date: None,
                location: None,
                pruefidentifikator: None,
                order_reference: None,
                bindungsfrist: None,
                reason: None,
                reference: None,
                currency: None,
                contact: None,
                product: None,
                price: None,
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

    /// Set the Prüfidentifikator (BGM DE 1004) — e.g. 15003 (ESA Angebot).
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Reference the original request this quotes (RFF+ACW).
    pub fn order_reference(mut self, reference: impl Into<String>) -> Self {
        self.inner.order_reference = Some(reference.into());
        self
    }

    /// Set the location (MaLo-ID / ZPB / NeLo-ID) this Angebot concerns.
    ///
    /// Emits `LOC+172+<id>` so the ESA can correlate the answer to the process
    /// it started (the QUOTES otherwise carries no location).
    pub fn location(mut self, id: impl Into<String>) -> Self {
        self.inner.location = Some(id.into());
        self
    }

    /// Set the Bindungsfrist (offer validity) — emits `DTM+273+<CCYYMMDD>:102`.
    ///
    /// DE 2005 `273` ("Validity period") is the QUOTES MIG-permitted qualifier
    /// for the offer's binding period (the MIG restricts 2005 to
    /// `{137,76,203,469,472,279,273}`). Present on an Angebot; **absent** on an
    /// Ablehnung der Anfrage — its presence is what tells the ESA an Angebot
    /// apart from a rejection.
    pub fn bindungsfrist(mut self, date_ccyymmdd: impl Into<String>) -> Self {
        self.inner.bindungsfrist = Some(date_ccyymmdd.into());
        self
    }

    /// Set the rejection reason — emits `FTX+ACB` free text (Ablehnung).
    ///
    /// DE 4451 `ACB` ("Additional information") is the only FTX qualifier the
    /// QUOTES MIG permits.
    pub fn reason(mut self, text: impl Into<String>) -> Self {
        self.inner.reason = Some(text.into());
        self
    }

    /// Add an SG1 reference `RFF+<qual>:<value>` (e.g. `Z13` = Prüfidentifikator).
    ///
    /// The QUOTES MIG restricts SG1 `1153` to `{AAV, ACW, Z13}`.
    pub fn reference(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner.reference = Some((qualifier.into(), value.into()));
        self
    }

    /// Set the currency (SG4) — emits `CUX+2:<ISO>:4` (`6347=2`, `6343=4`).
    pub fn currency(mut self, iso: impl Into<String>) -> Self {
        self.inner.currency = Some(iso.into());
        self
    }

    /// Set the SG14 contact — emits `CTA+IC+:<name>` and `COM+<comm>:EM`.
    pub fn contact(mut self, name: impl Into<String>, comm: impl Into<String>) -> Self {
        self.inner.contact = Some((name.into(), comm.into()));
        self
    }

    /// Set the SG27 line-item product — emits `LIN+1` and `PIA+5+<product>:SRW`.
    pub fn product(mut self, product: impl Into<String>) -> Self {
        self.inner.product = Some(product.into());
        self
    }

    /// Set the SG31 price — emits `PRI+CAL:<value>`.
    pub fn price(mut self, value: impl Into<String>) -> Self {
        self.inner.price = Some(value.into());
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

        let pid_str = self.inner.pruefidentifikator.map(|p| format!("{p:05}"));
        let bgm_1004 = pid_str
            .as_deref()
            .or(self.inner.document_id.as_deref())
            .unwrap_or("");
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["QUOTES", "D", "10A", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", "310", bgm_1004);
        emit_comp!(w, "DTM", ["137", &dtm_val, "102"]);
        // Bindungsfrist (offer validity, DE 2005 = 273) — present only on an
        // Angebot; the MIG does not permit Z12 here.
        if let Some(bf) = &self.inner.bindungsfrist {
            emit_comp!(w, "DTM", ["273", bf, "102"]);
        }
        // Ablehnungsgrund (Ablehnung der Anfrage) — top-level FTX (DE 4451 = ACB),
        // before the SG1 reference group.
        if let Some(reason) = &self.inner.reason {
            emit_comp!(w, "FTX", ["ACB"], [""], [""], [reason]);
        }
        // ── SG1: references ──────────────────────────────────────────────────
        if let Some(order_ref) = &self.inner.order_reference {
            emit_comp!(w, "RFF", ["ACW", order_ref]);
        }
        if let Some((q, v)) = &self.inner.reference {
            emit_comp!(w, "RFF", [q, v]);
        }
        // ── SG4: currency (CUX+2:<ISO>:4) ────────────────────────────────────
        if let Some(iso) = &self.inner.currency {
            emit_comp!(w, "CUX", ["2", iso, "4"]);
        }
        // ── SG11: parties + location ─────────────────────────────────────────
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
        // ── SG14: contact — before LOC per the QUOTES segment order ──────────
        if let Some((name, comm)) = &self.inner.contact {
            emit_comp!(w, "CTA", ["IC"], ["", name]);
            emit_comp!(w, "COM", [comm, "EM"]);
        }
        if let Some(loc) = &self.inner.location {
            emit_seg!(w, "LOC", "172", loc);
        }
        // ── SG27: line item + product ────────────────────────────────────────
        if let Some(product) = &self.inner.product {
            emit_seg!(w, "LIN", "1");
            emit_comp!(w, "PIA", ["5"], [product, "SRW"]);
            // ── SG31: price ──────────────────────────────────────────────────
            if let Some(price) = &self.inner.price {
                emit_comp!(w, "PRI", ["CAL", price]);
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
