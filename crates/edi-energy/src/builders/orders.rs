//! [`OrdersBuilder`] — fluent type-safe builder for ORDERS messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments};

#[derive(Debug, Clone)]
struct OrdersBuilderInner {
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
    location: Option<String>,
    // Additive ESA-Bestellung/Abbestellung (PID 17007/17008) content.
    /// SG1 references as `(1153 qualifier, 1154 value)`. An ESA Bestellung
    /// carries `RFF+AAG` (the Angebotsnummer) *and* `RFF+Z13`; an Abbestellung
    /// carries `RFF+ACW` *and* `RFF+Z13`. One slot cannot hold both.
    references: Vec<(String, String)>,
    /// `DTM+203` Ausführungsdatum — **Muss** on 17007/17008.
    ausfuehrungsdatum: Option<String>,
    /// `IMD+7081` — `Z01` Start Abo / `Z02` Ende Abo / `Z03` ohne Abo.
    abonnement: Option<String>,
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
                sender_agency: None,
                receiver_agency: None,
                message_ref: "1".to_owned(),
                document_code: None,
                document_id: None,
                pruefidentifikator: None,
                document_date: None,
                location: None,
                references: Vec::new(),
                ausfuehrungsdatum: None,
                abonnement: None,
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

    /// Set the Prüfidentifikator, emitted as `SG1 RFF+Z13:<pid>`.
    ///
    /// The AHBs give BGM DE 1004 as the **Dokumentennummer** and put the
    /// Prüfidentifikator in its own reference group, so this never touches BGM.
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.inner.pruefidentifikator = Some(pid);
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

    /// Add an SG1 reference `RFF+<qual>:<value>`.
    ///
    /// Additive. The ESA order PIDs need two (`AAG`/`ACW` plus `Z13`).
    pub fn reference(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner.references.push((qualifier.into(), value.into()));
        self
    }

    /// Set `DTM+203` Ausführungsdatum (`CCYYMMDDHHMM`).
    ///
    /// **Muss** on ORDERS 17007/17008 (ORDERS AHB 1.1b §4.15): on a Bestellung
    /// it is when the delivery is to start, on an Abbestellung when it stops.
    pub fn ausfuehrungsdatum(mut self, ccyymmddhhmm: impl Into<String>) -> Self {
        self.inner.ausfuehrungsdatum = Some(ccyymmddhhmm.into());
        self
    }

    /// Set the `IMD+7081` Abonnement code — `Z01` (Start Abo), `Z02` (Ende
    /// Abo) or `Z03` (ohne Abo).
    ///
    /// **Muss** on ORDERS 17007/17008. It is also what decides which EBD the
    /// MSB's ORDRSP must cite (`E_0256` for Z01/Z03, `E_0254` for Z02).
    pub fn abonnement(mut self, code: impl Into<String>) -> Self {
        self.inner.abonnement = Some(code.into());
        self
    }

    /// Set a free-form item description — emits `IMD+A`.
    ///
    /// Distinct from [`abonnement`](Self::abonnement), which emits the coded
    /// `IMD++<7081>` the ESA order PIDs require.
    pub fn item_description(mut self, text: impl Into<String>) -> Self {
        self.inner.item_description = Some(text.into());
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

        let code = self.inner.document_code.as_deref().unwrap_or("");
        // BGM DE 1004 is the **Dokumentennummer**, never the Prüfidentifikator
        // (ORDERS AHB 1.1b).
        let doc_id = self
            .inner
            .document_id
            .as_deref()
            .unwrap_or(&self.inner.message_ref);
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["ORDERS", "D", "09B", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", code, doc_id);
        // `DTM+137` Dokumentendatum. Every EDI@Energy AHB gives DE 2379 as
        // `303` (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to
        // `+00`; `[494]` requires the stamp to be the creation moment or
        // earlier. There is no Anwendungsfall in any AHB that takes `102`.
        emit_comp!(w, "DTM", ["137", &super::ccyymmddhhmm_utc(&dtm_val), "303"]);
        // `DTM+203` Ausführungsdatum, format 303 (CCYYMMDDHHMMZZZ).
        if let Some(ausf) = &self.inner.ausfuehrungsdatum {
            emit_comp!(w, "DTM", ["203", &super::ccyymmddhhmm_utc(ausf), "303"]);
        }
        // Abonnement — `IMD++<7081>`. DE 7081 sits in C272 (element 2), which
        // is why DE 7077 (element 1) stays empty.
        if let Some(code) = &self.inner.abonnement {
            emit_comp!(w, "IMD", [""], [code]);
        }
        // Free-form item description (7077 = A), for the non-ESA order PIDs.
        if self.inner.abonnement.is_none() && self.inner.item_description.is_some() {
            emit_comp!(w, "IMD", ["A"]);
        }
        // ── SG1: references (RFF+AAG / RFF+ACW / RFF+Z13) ────────────────────
        // The MIG lists the Referenz places (`RFF+AAG`, `RFF+ON`, `RFF+AAV`)
        // before the Prüfidentifikator's.
        for (q, v) in &self.inner.references {
            emit_comp!(w, "RFF", [q, v]);
        }
        if let Some(pid) = self.inner.pruefidentifikator {
            emit_comp!(w, "RFF", ["Z13", &pid.to_string()]);
        }
        // ── SG2: parties + location ──────────────────────────────────────────
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
        if let Some(loc) = &self.inner.location {
            // `SG2` Meldepunkt: the `NAD+DP` opens the group the `LOC+172`
            // belongs to (ORDERS MIG Nr 00028/00029).
            emit_seg!(w, "NAD", "DP");
            emit_seg!(w, "LOC", "172", loc);
        }
        // Section control — ORDERS requires UNS between header and summary.
        emit_seg!(w, "UNS", "S");
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
