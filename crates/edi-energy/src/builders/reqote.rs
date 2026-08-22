//! [`ReqoteBuilder`] — fluent type-safe builder for REQOTE messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments, today_ccyymmdd};

#[derive(Debug, Clone)]
struct ReqoteBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: Option<String>,
    document_id: Option<String>,
    /// `SG1 RFF+Z13` — the Prüfidentifikator. **Not** BGM DE 1004, which the
    /// AHB reserves for the Dokumentennummer.
    pruefidentifikator: Option<u32>,
    document_date: Option<String>,
    location: Option<String>,
    // Additive ESA-Werteanfrage (PID 35003) content — only emitted when set.
    /// SG1 references as `(1153 qualifier, 1154 value)`. A Werteanfrage needs
    /// more than one (`Z13` Prüfidentifikator alongside the process
    /// reference), so this is additive rather than a single slot.
    references: Vec<(String, String)>,
    /// `DTM+76` — Datum zum geplanten Leistungsbeginn (the ESA's Wunschtermin).
    leistungsbeginn: Option<String>,
    /// `NAD+DP` — Liefer-/Bezugsort. Muss on the ESA Werteanfrage.
    delivery_party: bool,
    /// SG27 `FTX+Z17/Z24/Z23` — SM-PKI delivery target for a Kapitel-4.6.2
    /// Messprodukt, as `(ipv4, ipv6, issuer, subject)`.
    smgw_delivery: Option<(String, String, String, String)>,
    /// SG28 `CCI+Z60` thresholds as `(Messprodukt-Position-Code, oberer, unterer)`.
    schwellwerte: Vec<(String, String, String)>,
    contact: Option<(String, String)>,
    line_item: bool,
    free_text: Option<(String, String)>,
    /// SG27 product lines as `(lin_qualifier, produkt_code)`.
    products: Vec<(String, String)>,
    /// SG10 characteristics as `(cci_qualifier, cav_code)`.
    characteristics: Vec<(String, String)>,
}

/// Fluent builder for `REQOTE` (Request for Quotation) messages.
///
/// Wire type string: `REQOTE:D:10A:UN:{release}`.
///
/// # Type-state
///
/// [`build`](ReqoteBuilder::build) is only available once both
/// [`sender`](ReqoteBuilder::sender) and [`receiver`](ReqoteBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::ReqoteBuilder;
///
/// let msg = ReqoteBuilder::new(Release::new("1.3c"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("4012345000023"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct ReqoteBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: ReqoteBuilderInner,
}

impl ReqoteBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: ReqoteBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: None,
                document_id: None,
                pruefidentifikator: None,
                document_date: None,
                location: None,
                references: Vec::new(),
                leistungsbeginn: None,
                delivery_party: false,
                smgw_delivery: None,
                schwellwerte: Vec::new(),
                contact: None,
                line_item: false,
                free_text: None,
                products: Vec::new(),
                characteristics: Vec::new(),
            },
        }
    }
}

impl<S, R> ReqoteBuilder<S, R> {
    fn transition<S2, R2>(self) -> ReqoteBuilder<S2, R2> {
        ReqoteBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> ReqoteBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> ReqoteBuilder<S, Set> {
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

    /// Override the BGM document type code.  Defaults to `"311"`.
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

    /// Set the location (MaLo-ID / ZPB / NeLo-ID) the request addresses.
    ///
    /// Emits `LOC+172+<id>`. The ESA Werteanfrage (`WiM` Teil 2 UC 4.1 Nr. 1)
    /// names the location the values are requested for.
    pub fn location(mut self, id: impl Into<String>) -> Self {
        self.inner.location = Some(id.into());
        self
    }

    /// Add an SG1 reference `RFF+<qual>:<value>` (REQOTE `1153 ∈ {Z13,AGO,AEP,AGK}`).
    ///
    /// Additive: call it once per reference the AHB requires.
    pub fn reference(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner.references.push((qualifier.into(), value.into()));
        self
    }

    /// Set `DTM+76` — „Datum zum geplanten Leistungsbeginn“ (`CCYYMMDDHHMM`).
    ///
    /// **Muss** on the ESA Werteanfrage (REQOTE AHB 1.2 §4.3): `WiM` Teil 2
    /// UC 4.1.2 Nr. 1 has the ESA state its Wunschtermin for the first
    /// delivery, and this segment is the only place it travels.
    pub fn leistungsbeginn(mut self, ccyymmddhhmm: impl Into<String>) -> Self {
        self.inner.leistungsbeginn = Some(ccyymmddhhmm.into());
        self
    }

    /// Emit the `NAD+DP` Liefer-/Bezugsort party that introduces the `LOC+172`
    /// Meldepunkt group. **Muss** on the ESA Werteanfrage.
    pub fn delivery_party(mut self) -> Self {
        self.inner.delivery_party = true;
        self
    }

    /// Set the SM-PKI delivery target for a Kapitel-4.6.2 Messprodukt —
    /// emits `FTX+Z17+++<ipv4>:<ipv6>`, `FTX+Z24+++<issuer>` and
    /// `FTX+Z23+++<subject>` inside the SG27 product group.
    ///
    /// **Muss** whenever a „Werte nach Typ 2 aus SMGW“ product is ordered
    /// (REQOTE AHB 1.2 §4.3, condition `[512]`); the certificate bodies are
    /// X.509 per BSI TR-03109-4.
    pub fn smgw_delivery(
        mut self,
        uri_ipv4: impl Into<String>,
        uri_ipv6: impl Into<String>,
        issuer: impl Into<String>,
        subject: impl Into<String>,
    ) -> Self {
        self.inner.smgw_delivery = Some((
            uri_ipv4.into(),
            uri_ipv6.into(),
            issuer.into(),
            subject.into(),
        ));
        self
    }

    /// Add an SG28 threshold pair — emits
    /// `CCI+Z60++<Messprodukt-Position-Code>:::<oberer>:<unterer>`.
    ///
    /// Required for every SMGW Messprodukt whose Auslöser is „Bei
    /// Schwellwertunter- / -überschreitung“ (REQOTE MIG 1.3c segment 00044).
    pub fn schwellwert(
        mut self,
        position_code: impl Into<String>,
        oberer: impl Into<String>,
        unterer: impl Into<String>,
    ) -> Self {
        self.inner
            .schwellwerte
            .push((position_code.into(), oberer.into(), unterer.into()));
        self
    }

    /// Set the SG14 contact — emits `CTA+IC+:<name>` and `COM+<comm>:EM`.
    pub fn contact(mut self, name: impl Into<String>, comm: impl Into<String>) -> Self {
        self.inner.contact = Some((name.into(), comm.into()));
        self
    }

    /// Emit a `LIN+1` line item (SG27).
    pub fn line_item(mut self) -> Self {
        self.inner.line_item = true;
        self
    }

    /// Add an `FTX+<qualifier>+++<text>` free-text segment.
    ///
    /// REQOTE AHB §4.3 uses `FTX+ACB` ("Zusätzliche Informationen") on the ESA
    /// Werteanfrage.
    pub fn free_text(mut self, qualifier: impl Into<String>, text: impl Into<String>) -> Self {
        self.inner.free_text = Some((qualifier.into(), text.into()));
        self
    }

    /// Add an SG27 product line — `LIN+<n>+<qualifier>` followed by
    /// `PIA+5+<produkt_code>:Z11`.
    ///
    /// REQOTE AHB §4.3 requires one SG27 per requested Messprodukt on the ESA
    /// Werteanfrage (PID 35003): `Z67` for "Erforderliches Messprodukt für Werte
    /// nach Typ 2 aus Backend", `Z68` for the SMGW Konfigurationserlaubnis.
    pub fn product(
        mut self,
        lin_qualifier: impl Into<String>,
        produkt_code: impl Into<String>,
    ) -> Self {
        self.inner
            .products
            .push((lin_qualifier.into(), produkt_code.into()));
        self
    }

    /// Add an SG10 characteristic — `CCI+++<qualifier>` followed by `CAV+<code>`.
    pub fn characteristic(
        mut self,
        cci_qualifier: impl Into<String>,
        cav_code: impl Into<String>,
    ) -> Self {
        self.inner
            .characteristics
            .push((cci_qualifier.into(), cav_code.into()));
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

        let code = self.inner.document_code.as_deref().unwrap_or("311");
        // BGM DE 1004 is the **Dokumentennummer**, never the Prüfidentifikator
        // (REQOTE AHB 1.2). It defaults to the message reference so the
        // document always carries a number the sender can correlate on.
        let doc_id = self
            .inner
            .document_id
            .as_deref()
            .unwrap_or(&self.inner.message_ref);
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["REQOTE", "D", "10A", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", code, doc_id);
        emit_comp!(w, "DTM", ["137", &dtm_val, "102"]);
        // `DTM+76` — geplanter Leistungsbeginn, format 303 (CCYYMMDDHHMMZZZ).
        if let Some(beginn) = &self.inner.leistungsbeginn {
            emit_comp!(w, "DTM", ["76", beginn, "303"]);
        }
        // `FTX+<4451>+++<C108>` — C108 sits at element position 4 in the
        // EDIFACT directory, with 4453 and C107 unused by this MIG. `FTX+ACB`
        // is a *Kann* segment: an empty free text would be a conformance
        // violation, so a blank note emits nothing at all.
        if let Some((qual, text)) = &self.inner.free_text
            && !text.is_empty()
        {
            emit_seg!(w, "FTX", qual, "", "", text);
        }
        // ── SG1: references ──────────────────────────────────────────────────
        // `RFF+Z13` carries the Prüfidentifikator; the AHB gives it its own
        // reference group rather than a slot in BGM.
        if let Some(pid) = self.inner.pruefidentifikator {
            emit_comp!(w, "RFF", ["Z13", &pid.to_string()]);
        }
        for (q, v) in &self.inner.references {
            emit_comp!(w, "RFF", [q, v]);
        }
        // ── SG11: parties ────────────────────────────────────────────────────
        // Segment sequence per the MIG: the SG11 parties, then the SG14
        // contact, then the SG11 Meldepunkt — `NAD, CTA, COM, LOC`. (The
        // AHB's 00016–00021 row numbers are guide positions, not a wire
        // order; they interleave CTA/COM between the NADs.)
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
        // ── SG11: Liefer-/Bezugsort + Meldepunkt ─────────────────────────────
        if self.inner.delivery_party {
            emit_seg!(w, "NAD", "DP");
        }
        if let Some((name, comm)) = &self.inner.contact {
            emit_comp!(w, "CTA", ["IC"], ["", name]);
            emit_comp!(w, "COM", [comm, "EM"]);
        }
        if let Some(loc) = &self.inner.location {
            emit_seg!(w, "LOC", "172", loc);
        }
        // ── SG10: characteristics ────────────────────────────────────────────
        // CAV is not part of the REQOTE MIG, so the characteristic is carried by
        // CCI alone.
        for (qual, _code) in &self.inner.characteristics {
            emit_seg!(w, "CCI", qual);
        }
        // ── SG27: line item / product lines ──────────────────────────────────
        if self.inner.line_item && self.inner.products.is_empty() {
            emit_seg!(w, "LIN", "1");
        }
        // `LIN+<1082 Positionsnummer>+<1229 Handlung, Code>` — the MIG's own
        // examples are `LIN+1+Z64'` / `LIN+1+Z65'`. The qualifier says which kind
        // of product line follows (REQOTE AHB §4.3: `Z67` "Erforderliches
        // Messprodukt für Werte nach Typ 2 aus Backend", `Z68` the SMGW
        // Konfigurationserlaubnis), and `PIA+5` names the product itself.
        //
        // `PIA+5+<7140 Produkt-Code>:<7143>` — 7143 is the *second* component
        // of C212, so the product code and its type code are adjacent. An
        // empty component between them would put `Z11` in DE 1131 (Codeliste),
        // which is „Nicht benutzt“ in this MIG.
        //
        // DE 1082 Positionsnummer is fixed to `1` by condition [903]: each
        // SG27 kind (`Z67`, `Z68`) may appear at most once per message, so the
        // lines are not numbered sequentially.
        for (qual, produkt) in &self.inner.products {
            emit_seg!(w, "LIN", "1", qual);
            emit_comp!(w, "PIA", ["5"], [produkt, "Z11"]);
            // SM-PKI delivery target belongs to the SMGW product line.
            if qual == "Z68"
                && let Some((ipv4, ipv6, issuer, subject)) = &self.inner.smgw_delivery
            {
                emit_comp!(w, "FTX", ["Z17"], [""], [""], [ipv4, ipv6]);
                emit_seg!(w, "FTX", "Z24", "", "", issuer);
                emit_seg!(w, "FTX", "Z23", "", "", subject);
                // ── SG28: Schwellwerte ───────────────────────────────────────
                // `CCI+Z60++<7037>:::<oberer 7036>:<unterer 7036>` — C240's
                // 1131 and 3055 are unused, which is what the `:::` skips.
                for (pos_code, oberer, unterer) in &self.inner.schwellwerte {
                    emit_comp!(w, "CCI", ["Z60"], [""], [pos_code, "", "", oberer, unterer]);
                }
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

impl ReqoteBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::reqote::ReqoteMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::reqote::ReqoteMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::reqote::ReqoteMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
