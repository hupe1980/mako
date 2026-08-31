//! [`IftstaBuilder`] — fluent type-safe builder for IFTSTA messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments};

#[derive(Debug, Clone)]
struct IftstaBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: Option<AgencyCode>,
    receiver_agency: Option<AgencyCode>,
    message_ref: String,
    document_code: String,
    document_id: Option<String>,
    document_date: Option<String>,
    // WiM status (SG14/SG15) — only emitted when set (e.g. 21042 Umsetzungsstatus).
    pruefidentifikator: Option<u32>,
    sts_category: Option<String>,
    sts_reason: Option<String>,
    sts_pruefschritt: Option<String>,
    sts_codeliste: Option<String>,
    meldepunkt: Option<String>,
    marktlokation: Option<String>,
    leistungsbeginn: Option<String>,
    vorgangsnummer: Option<String>,
    order_reference: Option<String>,
    vertragsende: Option<String>,
}

/// Fluent builder for `IFTSTA` (International Multimodal Status Report) messages.
///
/// # Type-state
///
/// [`build`](IftstaBuilder::build) is only available once both
/// [`sender`](IftstaBuilder::sender) and [`receiver`](IftstaBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::IftstaBuilder;
///
/// let msg = IftstaBuilder::new(Release::new("2.0g"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .document_id("00021000")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("4012345000023"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct IftstaBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: IftstaBuilderInner,
}

impl IftstaBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: IftstaBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: None,
                receiver_agency: None,
                message_ref: "1".to_owned(),
                document_code: "Z03".to_owned(),
                document_id: None,
                document_date: None,
                pruefidentifikator: None,
                sts_category: None,
                sts_reason: None,
                sts_pruefschritt: None,
                sts_codeliste: None,
                meldepunkt: None,
                marktlokation: None,
                leistungsbeginn: None,
                vorgangsnummer: None,
                order_reference: None,
                vertragsende: None,
            },
        }
    }
}

impl<S, R> IftstaBuilder<S, R> {
    fn transition<S2, R2>(self) -> IftstaBuilder<S2, R2> {
        IftstaBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> IftstaBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> IftstaBuilder<S, Set> {
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

    /// Set the BGM document identifier (Dokumentennummer).
    pub fn document_id(mut self, id: impl Into<String>) -> Self {
        self.inner.document_id = Some(id.into());
        self
    }

    /// Override the BGM document type code.  Defaults to `"Z03"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
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

    /// Set the `WiM` Prüfidentifikator, emitted as SG15 `RFF+Z13:<pid>`
    /// (e.g. `21042` — `WiM` / Umsetzungsstatus). Also stamped on the parsed
    /// message so `detect_pruefidentifikator()` and AHB validation resolve it.
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Set the SG15 `STS` status: DE9015 category + DE4405 reason
    /// (e.g. `("Z21", "105")` = „Bestellung / beendet" for UC 4.4).
    pub fn status(mut self, category: impl Into<String>, reason: impl Into<String>) -> Self {
        self.inner.sts_category = Some(category.into());
        self.inner.sts_reason = Some(reason.into());
        self
    }

    /// Set the SG15 `STS` Prüfschritt code (DE 9013) and the Codeliste it comes
    /// from (DE 1131).
    ///
    /// Both together, never one alone: an Antwortcode means different things in
    /// different Entscheidungsbäume, so DE 1131 is what makes DE 9013 readable.
    /// The Ersteinbau iMS answers (21030/21031) carry `E_0233` here — the
    /// Zustimmung cluster on 21030, the Ablehnung cluster on 21031 (IFTSTA AHB
    /// 2.1 Kap. 6.7, Bedingungen `[47]`/`[48]`).
    pub fn pruefschritt(mut self, code: impl Into<String>, codeliste: impl Into<String>) -> Self {
        self.inner.sts_pruefschritt = Some(code.into());
        self.inner.sts_codeliste = Some(codeliste.into());
        self
    }

    /// Set the SG14 `LOC+172` Meldepunkt — the Zählpunktbezeichnung of the
    /// Messlokation.
    ///
    /// `172` is the only DE 3227 value IFTSTA defines, in both Sparten, and the
    /// Ersteinbau Anwendungsfälle mark the segment Muss (IFTSTA AHB 2.1
    /// Kap. 6.7, Hinweis `[505]`).
    pub fn meldepunkt(mut self, zaehlpunkt: impl Into<String>) -> Self {
        self.inner.meldepunkt = Some(zaehlpunkt.into());
        self
    }

    /// Set the SG15 `RFF+AVE` — the Marktlokations-ID.
    ///
    /// Muss on a 21029 whose receiver holds the role LF (Bedingung `[20]`): the
    /// supplier is told about a Messlokation it knows only through its
    /// Marktlokation.
    pub fn marktlokation(mut self, malo_id: impl Into<String>) -> Self {
        self.inner.marktlokation = Some(malo_id.into());
        self
    }

    /// Set the SG15 `DTM+76` Lieferdatum/-zeit, geplant (`CCYYMMDD`).
    ///
    /// The planned Umstellungszeitpunkt of an Ersteinbau. DE 2379 is `102`
    /// here, not the `303` the rest of the message uses — the AHB names the
    /// format per date, and `[496]` requires it to be later than `DTM+137`.
    pub fn leistungsbeginn(mut self, date: impl Into<String>) -> Self {
        self.inner.leistungsbeginn = Some(date.into());
        self
    }

    /// Set the SG14 `CNI+<n>` Vorgangsnummer (mandatory for `WiM` status messages).
    pub fn vorgangsnummer(mut self, n: impl Into<String>) -> Self {
        self.inner.vorgangsnummer = Some(n.into());
        self
    }

    /// Set the SG15 `RFF+AGI:<ref>` Beantragungsnummer — the Belegnummer of the
    /// original Bestellung (ORDERS BGM) this status refers to.
    pub fn order_reference(mut self, reference: impl Into<String>) -> Self {
        self.inner.order_reference = Some(reference.into());
        self
    }

    /// Set the SG15 `DTM+93` Datum Vertragsende (the Beendigung date, `YYYYMMDD`).
    pub fn vertragsende(mut self, date: impl Into<String>) -> Self {
        self.inner.vertragsende = Some(date.into());
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

        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["IFTSTA", "D", "18A", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", &self.inner.document_code, doc_id);
        // `DTM+137` Dokumentendatum. Every EDI@Energy AHB gives DE 2379 as
        // `303` (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to
        // `+00`; `[494]` requires the stamp to be the creation moment or
        // earlier. There is no Anwendungsfall in any AHB that takes `102`.
        emit_comp!(w, "DTM", ["137", &super::ccyymmddhhmm_utc(&dtm_val), "303"]);
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

        // ── SG14 CNI — Vorgangsnummer ────────────────────────────────────────
        if let Some(vn) = &self.inner.vorgangsnummer {
            emit_seg!(w, "CNI", vn);
        }
        // ── SG14 LOC+172 — Meldepunkt ────────────────────────────────────────
        //
        // Between `CNI` and the SG15 group, which is where the AHB's segment
        // numbering puts it (`SG14 LOC` 00018, before `SG15 STS` 00034).
        if let Some(zp) = &self.inner.meldepunkt {
            emit_comp!(w, "LOC", ["172"], [zp]);
        }
        // ── SG15 STS — DE9015 category, DE4405 reason, DE9013 Prüfschritt,
        //    DE1131 Codeliste ──────────────────────────────────────────────────
        if let (Some(cat), Some(reason)) = (&self.inner.sts_category, &self.inner.sts_reason) {
            if let (Some(code), Some(liste)) =
                (&self.inner.sts_pruefschritt, &self.inner.sts_codeliste)
            {
                emit_comp!(w, "STS", [cat], ["", reason], [code, liste]);
            } else {
                emit_comp!(w, "STS", [cat], ["", reason]);
            }
        }
        // ── SG15 RFF+Z13 — Prüfidentifikator ─────────────────────────────────
        if let Some(pid) = self.inner.pruefidentifikator {
            let pid_str = pid.to_string();
            emit_comp!(w, "RFF", ["Z13", &pid_str]);
        }
        // ── SG15 RFF+AVE — Referenz auf die Marktlokation ────────────────────
        if let Some(malo) = &self.inner.marktlokation {
            emit_comp!(w, "RFF", ["AVE", malo]);
        }
        // ── SG15 RFF+AGI — Beantragungsnummer (ref to the Bestellung) ────────
        if let Some(order_ref) = &self.inner.order_reference {
            emit_comp!(w, "RFF", ["AGI", order_ref]);
        }
        // ── SG15 DTM+76 — Lieferdatum/-zeit, geplant ─────────────────────────
        //
        // `102` `CCYYMMDD`, which is what the AHB names for this date alone.
        if let Some(geplant) = &self.inner.leistungsbeginn {
            emit_comp!(w, "DTM", ["76", geplant, "102"]);
        }
        // ── SG15 DTM+93 — Datum Vertragsende (Beendigung) ───────────────────
        //
        // IFTSTA AHB 2.1 gives DE 2379 as `303` here, like every other date in
        // the message; `[931]` fixes the zone to `+00`.
        if let Some(ende) = &self.inner.vertragsende {
            emit_comp!(w, "DTM", ["93", &super::ccyymmddhhmm_utc(ende), "303"]);
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

impl IftstaBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::iftsta::IftstaMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::iftsta::IftstaMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        let pruefidentifikator = self.inner.pruefidentifikator;
        Ok(crate::messages::iftsta::IftstaMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pruefidentifikator,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EdiEnergyMessage;

    /// UC 4.4 „Beendigung durch MSB" (IFTSTA 21042) builds a message that
    /// carries the Prüfidentifikator (SG15 RFF+Z13), the STS Z21/105 status,
    /// the Vorgangsnummer (SG14 CNI) and passes AHB validation.
    #[test]
    fn builds_a_conformant_21042_beendigung() {
        let msg = IftstaBuilder::new(Release::new("2.0g"))
            .sender("4012345000023")
            .receiver("9900555000005")
            .document_code("Z09")
            .document_id("00021042")
            .pruefidentifikator(21042)
            .status("Z21", "105")
            .vorgangsnummer("1")
            .order_reference("BEST-4711")
            .vertragsende("20260801")
            .build()
            .expect("build 21042");

        assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 21042);

        // The wire form carries the mandatory WiM status segments.
        let wire = String::from_utf8(msg.serialize().expect("serialize")).unwrap();
        assert!(
            wire.contains("RFF+Z13:21042"),
            "PID in SG15 RFF+Z13: {wire}"
        );
        assert!(
            wire.contains("STS+Z21+:105"),
            "STS Z21/105 „beendet\": {wire}"
        );
        assert!(wire.contains("CNI+1"), "Vorgangsnummer SG14 CNI: {wire}");
        assert!(
            wire.contains("RFF+AGI:BEST-4711"),
            "order ref SG15 RFF+AGI: {wire}"
        );

        let report = msg.validate().unwrap();
        assert!(
            report.is_valid(),
            "21042 must be AHB-conformant: {report}\n{:#?}",
            report.errors()
        );
    }
}
