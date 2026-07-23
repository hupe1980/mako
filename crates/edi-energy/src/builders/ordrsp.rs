//! [`OrdrespBuilder`] — fluent type-safe builder for ORDRSP messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments, today_ccyymmdd};

#[derive(Debug, Clone)]
struct OrdrespBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_id: Option<String>,
    document_date: Option<String>,
    pruefidentifikator: Option<u32>,
    order_reference: Option<String>,
    // Additive ESA-Antwort (PID 19011-19014) content — ORDRSP carries no LOC.
    adjustment: Option<String>,
    adjustment_reason: Option<String>,
    item_description: bool,
    line_item: bool,
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
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_id: None,
                document_date: None,
                pruefidentifikator: None,
                order_reference: None,
                adjustment: None,
                adjustment_reason: None,
                item_description: false,
                line_item: false,
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

    /// Set the Prüfidentifikator (BGM DE 1004) — the routing key of the answer
    /// (e.g. 19011 Bestätigung, 19012 Ablehnung for ESA Wertebestellung).
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Reference the original order/request this answers (RFF+ACW).
    pub fn order_reference(mut self, reference: impl Into<String>) -> Self {
        self.inner.order_reference = Some(reference.into());
        self
    }

    /// Set the SG2 adjustment code — emits `AJT+<code>` (DE 4465).
    pub fn adjustment(mut self, code: impl Into<String>) -> Self {
        self.inner.adjustment = Some(code.into());
        self
    }

    /// Set the SG2 adjustment reason — emits `FTX+<code>` after the AJT.
    ///
    /// The MIG caps this FTX at two elements, so the reason is carried as the
    /// coded 4451 qualifier (`∈ {AAP, ABO, Z27, Z28, Z33}`), not free text.
    pub fn adjustment_reason(mut self, ftx_qualifier: impl Into<String>) -> Self {
        self.inner.adjustment_reason = Some(ftx_qualifier.into());
        self
    }

    /// Emit a minimal item description `IMD+A` (the AHB requires the segment).
    pub fn item_description(mut self) -> Self {
        self.inner.item_description = true;
        self
    }

    /// Emit an SG27 line item `LIN+1` (the AHB requires one for PID 19011).
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

        // BGM DE 1004 carries the Prüfidentifikator (profile pid_source =
        // BgmDe1004); fall back to `document_id` for non-MaKo callers.
        let pid_str = self.inner.pruefidentifikator.map(|p| format!("{p:05}"));
        let bgm_1004 = pid_str
            .as_deref()
            .or(self.inner.document_id.as_deref())
            .unwrap_or("");
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["ORDRSP", "D", "10A", "UN", self.inner.release.as_str()]
        );
        // ORDRSP BGM: DE 1001 = 7 (the only value the MIG permits), DE 1004 =
        // the Prüfidentifikator. (An earlier draft emitted an invalid `231` and
        // a spurious third element.)
        emit_seg!(w, "BGM", "7", bgm_1004);
        emit_comp!(w, "DTM", ["137", &dtm_val, "102"]);
        // Item description (IMD) — before the SG1 reference.
        if self.inner.item_description {
            emit_comp!(w, "IMD", ["A"]);
        }
        // ── SG1: reference (RFF+ACW echoes the order this answers) ───────────
        if let Some(order_ref) = &self.inner.order_reference {
            emit_comp!(w, "RFF", ["ACW", order_ref]);
        }
        // ── SG2: adjustment + coded reason ───────────────────────────────────
        if let Some(code) = &self.inner.adjustment {
            emit_comp!(w, "AJT", [code]);
        }
        if let Some(code) = &self.inner.adjustment_reason {
            emit_comp!(w, "FTX", [code]);
        }
        // ── SG3: parties ─────────────────────────────────────────────────────
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
        // ── SG27: line item ──────────────────────────────────────────────────
        if self.inner.line_item {
            emit_seg!(w, "LIN", "1");
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
