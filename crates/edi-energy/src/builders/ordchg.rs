//! [`OrdchgBuilder`] — fluent type-safe builder for ORDCHG messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments};

#[derive(Debug, Clone)]
struct OrdchgBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: Option<AgencyCode>,
    receiver_agency: Option<AgencyCode>,
    message_ref: String,
    document_code: Option<String>,
    document_id: Option<String>,
    /// `SG1 RFF+Z13` — the Prüfidentifikator. **Not** BGM DE 1004, which the
    /// AHB reserves for the Dokumentennummer.
    pruefidentifikator: Option<u32>,
    document_date: Option<String>,
    /// SG1 references as `(1153 qualifier, 1154 value)`. A Stornierung carries
    /// `RFF+ON` (the ORDERS' Belegnummer) *and* `RFF+Z13`.
    references: Vec<(String, String)>,
}

/// Fluent builder for `ORDCHG` (Purchase Order Change) messages.
///
/// Wire type string: `ORDCHG:D:20B:UN:{release}`.
///
/// # Type-state
///
/// [`build`](OrdchgBuilder::build) is only available once both
/// [`sender`](OrdchgBuilder::sender) and [`receiver`](OrdchgBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::OrdchgBuilder;
///
/// let msg = OrdchgBuilder::new(Release::new("1.1"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .document_code("Z51")
///     .document_id("ORDCHG20241001001")
///     .build()?;
///
/// assert_eq!(msg.assoc_code(), "1.1");
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct OrdchgBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: OrdchgBuilderInner,
}

impl OrdchgBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: OrdchgBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: None,
                receiver_agency: None,
                message_ref: "1".to_owned(),
                document_code: None,
                document_id: None,
                pruefidentifikator: None,
                document_date: None,
                references: Vec::new(),
            },
        }
    }
}

impl<S, R> OrdchgBuilder<S, R> {
    fn transition<S2, R2>(self) -> OrdchgBuilder<S2, R2> {
        OrdchgBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> OrdchgBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> OrdchgBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Override the agency code for the sender's party identifier.
    ///
    /// Leave unset and the agency is derived from the MP-ID itself —
    /// [`AgencyCode::for_mp_id`]: `99…` → BDEW `293`, `98…` → DVGW `332`, any
    /// other 13-digit code → GS1 `9`. Override only for a party whose
    /// registered code list differs from what its number implies.
    pub fn sender_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.sender_agency = Some(agency);
        self
    }

    /// Override the agency code for the receiver's party identifier.
    ///
    /// Derived from the MP-ID when unset — see
    /// [`sender_agency`](Self::sender_agency).
    pub fn receiver_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.receiver_agency = Some(agency);
        self
    }

    /// Set the BGM document type code (DE 1001).
    ///
    /// Valid codes: `"Z51"` (Sperrung), `"Z52"` (Entsperrung), `"Z57"` (Übermittlung von Werten).
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = Some(code.into());
        self
    }

    /// Set the BGM document identifier.
    pub fn document_id(mut self, id: impl Into<String>) -> Self {
        self.inner.document_id = Some(id.into());
        self
    }

    /// Set the Prüfidentifikator, emitted as `SG1 RFF+Z13:<pid>`.
    ///
    /// The AHBs give BGM DE 1004 as the **Dokumentennummer** and put the
    /// Prüfidentifikator in its own reference group, so this never touches BGM.
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.inner.pruefidentifikator = Some(pid);
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

    /// Add the SG1 reference `RFF+<qual>:<value>`.
    ///
    /// ORDCHG SG1 `1153 ∈ {ON, TN, Z13}`. Mandatory — the ORDCHG carries no LOC,
    /// so a Stornierung identifies its target through this reference (e.g.
    /// `ON` = the Bestellung's order number, `Z13` = Prüfidentifikator).
    pub fn reference(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner.references.push((qualifier.into(), value.into()));
        self
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(super::now_ccyymmddhhmm, str::to_owned);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        let code = self.inner.document_code.as_deref().unwrap_or("Z51");
        // BGM DE 1004 is the **Dokumentennummer**, never the Prüfidentifikator
        // (ORDCHG AHB 1.1).
        let doc_id = self
            .inner
            .document_id
            .as_deref()
            .unwrap_or(&self.inner.message_ref);
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["ORDCHG", "D", "20B", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", code, doc_id, "1");
        // `DTM+137` Dokumentendatum. Every EDI@Energy AHB gives DE 2379 as
        // `303` (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to
        // `+00`; `[494]` requires the stamp to be the creation moment or
        // earlier. There is no Anwendungsfall in any AHB that takes `102`.
        emit_comp!(w, "DTM", ["137", &super::ccyymmddhhmm_utc(&dtm_val), "303"]);
        // ── SG1: reference (mandatory; the ORDCHG has no LOC) ────────────────
        if let Some(pid) = self.inner.pruefidentifikator {
            emit_comp!(w, "RFF", ["Z13", &pid.to_string()]);
        }
        for (q, v) in &self.inner.references {
            emit_comp!(w, "RFF", [q, v]);
        }
        // ── SG3: parties ─────────────────────────────────────────────────────
        if let Some(id) = &self.inner.sender_id {
            emit_comp!(
                w,
                "NAD",
                ["MS"],
                [id, "", super::agency_for(self.inner.sender_agency, id)]
            );
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_comp!(
                w,
                "NAD",
                ["MR"],
                [id, "", super::agency_for(self.inner.receiver_agency, id)]
            );
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

impl OrdchgBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::ordchg::OrdchgMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::ordchg::OrdchgMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::ordchg::OrdchgMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
