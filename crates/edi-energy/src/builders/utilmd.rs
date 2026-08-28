//! [`UtilmdBuilder`] — fluent type-safe builder for UTILMD messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::utilmd_codes::{
    AntwortStatus, IDE_VORGANG, Produktpaket, STS_STATUS_ANTWORT, STS_TRANSAKTIONSGRUND,
    Transaktionsgrund,
};
use crate::{Error, Lokationstyp, Pruefidentifikator, Release};

use super::{Set, Unset, bytes_to_segments};

// ── Inner fields structs ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct UtilmdTransactionSpec {
    /// `IDE` DE 7495 — [`IDE_VORGANG`] on every process message.
    ide_qualifier: String,
    /// `IDE` DE 7402 — the **Vorgangsnummer**, never a location ID.
    vorgangsnummer: String,
    process_dates: Vec<(String, String)>,
    transaktionsgrund: Option<Transaktionsgrund>,
    antwort: Option<AntwortStatus>,
    free_texts: Vec<(String, String)>,
    agr: Option<(String, String)>,
    /// `SG5 LOC` — one entry per Lokation the Vorgang names.
    locations: Vec<(String, String)>,
    references: Vec<(String, String)>,
    /// `SG6 RFF+TN` — the Vorgangsnummer of the message being answered.
    referenz_vorgangsnummer: Option<String>,
    /// `SG8 SEQ+Z79` — the Produktpakete an Anmeldung and its Bestätigung
    /// carry. Muss on 55001, 55077, 55600, 55601, 55014 and 55608.
    produktpakete: Vec<Produktpaket>,
    /// `SG10 CCI` — Merkmale addressed by Klassentyp (DE 7059) with their value
    /// in DE 7037.
    merkmale: Vec<(String, String)>,
    customer_nad: Option<(String, String)>,
}

#[derive(Debug, Clone)]
struct UtilmdBuilderInner {
    release: Release,
    pruefidentifikator: Option<Pruefidentifikator>,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: String,
    document_date: Option<String>,
    rff_entries: Vec<(String, String)>,
    transactions: Vec<UtilmdTransactionSpec>,
}

// ── UtilmdBuilder ─────────────────────────────────────────────────────────────

/// Fluent builder for `UTILMD` (Utilities Master Data) messages.
///
/// # Type-state
///
/// [`build`](UtilmdBuilder::build) is only available once both
/// [`sender`](UtilmdBuilder::sender) and [`receiver`](UtilmdBuilder::receiver)
/// have been called. The compiler enforces this at the call site.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::{Release, Pruefidentifikator};
/// use edi_energy::builders::UtilmdBuilder;
///
/// let msg = UtilmdBuilder::new(Release::new("5.5.3a"))
///     .pruefidentifikator(Pruefidentifikator::new(55001).unwrap())
///     .sender("9900987654321")
///     .receiver("9900123456789")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("9900987654321"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct UtilmdBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: UtilmdBuilderInner,
}

impl UtilmdBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: UtilmdBuilderInner {
                release,
                pruefidentifikator: None,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: "E01".to_owned(),
                document_date: None,
                rff_entries: Vec::new(),
                transactions: Vec::new(),
            },
        }
    }
}

impl<S, R> UtilmdBuilder<S, R> {
    fn transition<S2, R2>(self) -> UtilmdBuilder<S2, R2> {
        UtilmdBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier (DE 3039).
    pub fn sender(mut self, id: impl Into<String>) -> UtilmdBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier (DE 3039).
    pub fn receiver(mut self, id: impl Into<String>) -> UtilmdBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Override the agency code for the sender's party identifier.
    ///
    /// Default: [`AgencyCode::Bdew`] (`"293"`). Use [`AgencyCode::Etso`] (`"305"`)
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

    /// Set the Pruefidentifikator (process-variant code, e.g. `55001`).
    pub fn pruefidentifikator(mut self, pid: Pruefidentifikator) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Override the message reference number (UNH / DE 0062).  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Override the BGM document name code (DE 1001).  Defaults to `"E01"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Add a reference segment (RFF, SG1) to the message header.
    ///
    /// `qualifier` is the DE 1153 reference qualifier (e.g. `"ACE"`, `"Z13"`).
    /// `reference` is the reference identifier (DE 1154).
    ///
    /// UTILMD MIG 5.5.3a requires at least one `RFF` in SG1 (max 99).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// let msg = UtilmdBuilder::new(Release::new("5.5.3a"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55001).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .rff("ACE", "20230701")
    ///     .build()?;
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn rff(mut self, qualifier: impl Into<String>, reference: impl Into<String>) -> Self {
        self.inner
            .rff_entries
            .push((qualifier.into(), reference.into()));
        self
    }

    /// Start configuring a Vorgang (SG4 / IDE block).
    ///
    /// `vorgangsnummer` is `IDE` DE 7402 — the sender's own reference for this
    /// transaction, unique across `IDE+24` **and** `IDE+Z01`. It is *not* a
    /// location ID: the Marktlokation goes into `SG5 LOC+Z16` via
    /// [`marktlokation`](UtilmdTransactionBuilder::marktlokation).
    ///
    /// Returns a [`UtilmdTransactionBuilder`] sub-builder. Call
    /// [`done`](UtilmdTransactionBuilder::done) to finalize and return.
    pub fn transaction(self, vorgangsnummer: impl Into<String>) -> UtilmdTransactionBuilder<S, R> {
        self.transaction_with_qualifier(IDE_VORGANG, vorgangsnummer)
    }

    /// Start an `IDE+Z01` list block (`MaBiS` Summenzeitreihen).
    ///
    /// UTILMD DE 7495 has exactly two values; this is the other one.
    pub fn list_transaction(self, list_id: impl Into<String>) -> UtilmdTransactionBuilder<S, R> {
        self.transaction_with_qualifier(crate::utilmd_codes::IDE_LISTE, list_id)
    }

    /// Start a Vorgang with an explicit DE 7495 qualifier.
    ///
    /// Reserved for round-tripping messages from counterparties that use a
    /// qualifier outside the MIG's `24` / `Z01` pair. New code should call
    /// [`transaction`](Self::transaction).
    pub fn transaction_with_qualifier(
        self,
        ide_qualifier: impl Into<String>,
        vorgangsnummer: impl Into<String>,
    ) -> UtilmdTransactionBuilder<S, R> {
        UtilmdTransactionBuilder {
            parent: self,
            spec: UtilmdTransactionSpec {
                ide_qualifier: ide_qualifier.into(),
                vorgangsnummer: vorgangsnummer.into(),
                ..Default::default()
            },
        }
    }
}

/// Emit the `SG6` references of one Vorgang, in AHB order.
///
/// `RFF+Z13` carries the **Prüfidentifikator** — DE 1154 format `R n5`, „genau
/// einmal je SG4 IDE (Vorgang) anzugeben". It belongs here and not in `BGM`
/// DE 1004, which every row of UTILMD AHB Strom 2.2 and Gas 1.2 names the
/// *Dokumentennummer*.
///
/// `RFF+TN` carries „Referenz Vorgangsnummer (aus Anfragenachricht)", Muss on
/// every Antwortnachricht. It is what ties an answer to its request, because
/// `IDE+24` DE 7402 must be a fresh number: the MIG's „Hinweis zu DE7402" makes
/// a Vorgangsnummer unusable once it has been sent.
fn emit_sg6<W: std::io::Write>(
    w: &mut Writer<W>,
    pid_str: &str,
    tx: &UtilmdTransactionSpec,
) -> Result<(), Error> {
    if !pid_str.is_empty() {
        emit_comp!(w, "RFF", ["Z13", pid_str]);
    }
    if let Some(referenz) = &tx.referenz_vorgangsnummer {
        emit_comp!(w, "RFF", ["TN", referenz]);
    }
    for (rff_q, rff_ref) in &tx.references {
        emit_comp!(w, "RFF", [rff_q, rff_ref]);
    }
    Ok(())
}

/// Emit the `SG8` / `SG10` Produktpakete of one Vorgang.
///
/// ```text
/// SEQ+Z79+1
/// PIA+5+9991000002082:Z11
/// CCI+Z66
/// CAV+ZV4:::11XBK-EEG-----1
/// ```
///
/// The Anmeldung einer Zuordnung des LFN is not complete without one: the AHB
/// marks `SG8 SEQ+Z79` Muss on 55001, 55077, 55600, 55601, 55014 and 55608, and
/// the Codeliste der Konfigurationen 1.4 Kap. 6.1.1 makes the Bilanzkreis
/// product unconditional inside it („zwingend anzugeben").
///
/// `CAV+ZH9` is conditional (Bedingung `[36]`): it appears only where the
/// Codeliste gives the product a Code der Produkteigenschaft. The Bilanzkreis
/// has none, so its package is `CCI+Z66` followed by `CAV+ZV4` alone.
fn emit_sg8_produktpakete<W: std::io::Write>(
    w: &mut Writer<W>,
    tx: &UtilmdTransactionSpec,
) -> Result<(), Error> {
    use crate::utilmd_codes::produkt;

    for paket in &tx.produktpakete {
        emit_seg!(
            w,
            "SEQ",
            produkt::SEQ_PRODUKTPAKET,
            &paket.paket_id.to_string()
        );
        for p in &paket.produkte {
            emit_comp!(
                w,
                "PIA",
                [produkt::PIA_ERFORDERLICHES_PRODUKT],
                [&p.produkt_code, produkt::PIA_TYP_PRODUKT]
            );
            emit_seg!(w, "CCI", produkt::CCI_PRODUKTEIGENSCHAFT);
            if let Some(eigenschaft) = &p.eigenschaft {
                emit_comp!(w, "CAV", [produkt::CAV_EIGENSCHAFT, "", "", eigenschaft]);
            }
            if let Some(wert) = &p.wert {
                emit_comp!(w, "CAV", [produkt::CAV_WERT, "", "", wert]);
            }
        }
    }
    // `SG8 SEQ+ZH0` — „so oft zu wiederholen, wie es Produktpaket-ID in einem
    // Geschäftsvorfall gibt" (AHB Kap. 5.3). The group is Muss wherever
    // `SEQ+Z79` is, so it follows every package block rather than being
    // optional: `CCI+Z65` DE 4051 tells the NB whether it may assign the LF on
    // a partial application of the package.
    //
    // The `CAV` Priorisierung (`Z75`…`Z79`) is Bedingung [42] — „wenn mehr als
    // ein SG8 SEQ+ZH0 vorhanden" — so a single package carries none.
    for (idx, paket) in tx.produktpakete.iter().enumerate() {
        emit_seg!(
            w,
            "SEQ",
            produkt::SEQ_PRIORISIERUNG,
            &paket.paket_id.to_string()
        );
        emit_seg!(
            w,
            "CCI",
            produkt::CCI_UMSETZUNGSGRAD,
            "",
            "",
            paket.umsetzung.code()
        );
        if tx.produktpakete.len() > 1
            && let Some(prio) = produkt::PRIORITAET.get(idx)
        {
            emit_seg!(w, "CAV", prio);
        }
    }
    Ok(())
}

/// Emit one `SG4 IDE` Vorgang and everything nested under it.
///
/// MIG Zähler order inside SG4: IDE (0190), DTM (0230), STS (0250), FTX (0280),
/// AGR (0290), then SG5 LOC (0330), SG6 RFF (0360), SG8/SG10 (Produktpakete)
/// and SG12 NAD. Layer 3.5 checks it, on both sides of the wire.
fn emit_sg4<W: std::io::Write>(
    w: &mut Writer<W>,
    pid_str: &str,
    tx: &UtilmdTransactionSpec,
) -> Result<(), Error> {
    // MIG Zähler order inside SG4: IDE (0190), DTM (0230), STS (0250),
    // FTX (0280), AGR (0290), then SG5 LOC (0330), SG6 RFF (0360) and
    // SG12 NAD. Layer 3.5 checks it, on both sides of the wire.
    emit_seg!(w, "IDE", &tx.ide_qualifier, &tx.vorgangsnummer);
    for (qualifier, date_val) in &tx.process_dates {
        let fmt = sg4_dtm_format(qualifier);
        let value = if fmt == "303" {
            super::ccyymmddhhmm_utc(date_val)
        } else {
            date_val.clone()
        };
        emit_comp!(w, "DTM", [qualifier, &value, fmt]);
    }
    if let Some(grund) = &tx.transaktionsgrund {
        // `STS+7++<grund>+<ergaenzung>+<befristet>` — Statuskategorie 7
        // in C601, then one repeated C556 per code. C555 sits between
        // C601 and the first C556 and is *nicht benutzt*, so it is
        // written empty rather than omitted. MIG example:
        // `STS+7++E01+ZW4+E03'`.
        let ergaenzung = grund.ergaenzung.as_deref().unwrap_or("");
        match grund.befristet.as_deref() {
            Some(befristet) => emit_seg!(
                w,
                "STS",
                STS_TRANSAKTIONSGRUND,
                "",
                &grund.grund,
                ergaenzung,
                befristet
            ),
            None if !ergaenzung.is_empty() => {
                emit_seg!(
                    w,
                    "STS",
                    STS_TRANSAKTIONSGRUND,
                    "",
                    &grund.grund,
                    ergaenzung
                );
            }
            None => emit_seg!(w, "STS", STS_TRANSAKTIONSGRUND, "", &grund.grund),
        }
    }
    if let Some(antwort) = &tx.antwort {
        // `STS+E01++<code>:<codeliste>` — the Prüfschritt code in C556
        // DE 9013 and the Codeliste it comes from in DE 1131. The AHB
        // marks this Muss on every Bestätigung and Ablehnung and
        // constrains the code to that list's Zustimmungs- or
        // Ablehnungs-Cluster.
        if let Some(cl) = antwort.codeliste.as_deref() {
            emit_comp!(w, "STS", [STS_STATUS_ANTWORT], [""], [&antwort.code, cl]);
        } else {
            emit_seg!(w, "STS", STS_STATUS_ANTWORT, "", &antwort.code);
        }
    }
    for (ftx_q, ftx_text) in &tx.free_texts {
        emit_comp!(w, "FTX", [ftx_q], [""], [""], [ftx_text]);
    }
    if let Some((svc_req, resp_type)) = &tx.agr {
        emit_comp!(w, "AGR", [svc_req, resp_type]);
    }
    for (loc_q, loc_id) in &tx.locations {
        emit_comp!(w, "LOC", [loc_q], [loc_id]);
    }
    emit_sg6(w, pid_str, tx)?;
    for (klassentyp, wert) in &tx.merkmale {
        // `CCI+<7059>++<7037>` — C502 „Einzelheiten zu Maßangaben" is
        // nicht benutzt and still occupies element 2.
        emit_seg!(w, "CCI", klassentyp, "", wert);
    }
    emit_sg8_produktpakete(w, tx)?;
    if let Some((nad_q, nad_id)) = &tx.customer_nad {
        emit_comp!(w, "NAD", [nad_q], [nad_id, "", "293"]);
    }
    Ok(())
}

impl<S, R> UtilmdBuilder<S, R> {
    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let pid_str = self
            .inner
            .pruefidentifikator
            .map(|p| format!("{:05}", p.as_u32()))
            .unwrap_or_default();
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(super::now_ccyymmddhhmm, str::to_owned);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["UTILMD", "D", "11A", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", &self.inner.document_code, &pid_str, "9");
        // `DTM+137` Dokumentendatum. Every EDI@Energy AHB gives DE 2379 as
        // `303` (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to
        // `+00`; `[494]` requires the stamp to be the creation moment or
        // earlier. There is no Anwendungsfall in any AHB that takes `102`.
        emit_comp!(w, "DTM", ["137", &super::ccyymmddhhmm_utc(&dtm_val), "303"]);
        for (qualifier, reference) in &self.inner.rff_entries {
            emit_comp!(w, "RFF", [qualifier, reference]);
        }
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
        for tx in &self.inner.transactions {
            emit_sg4(&mut w, &pid_str, tx)?;
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

impl UtilmdBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::utilmd::UtilmdMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::utilmd::UtilmdMessage, Error> {
        let pid = self
            .inner
            .pruefidentifikator
            .map(super::super::pruefidentifikator::Pruefidentifikator::as_u32);
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::utilmd::UtilmdMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}

// ── UtilmdTransactionBuilder ──────────────────────────────────────────────────

/// Sub-builder for a transaction (SG4 / IDE block) in a UTILMD message.
///
/// Obtained via [`UtilmdBuilder::transaction`]. Call
/// [`done`](UtilmdTransactionBuilder::done) to finalize and return to the
/// parent builder.
#[derive(Debug)]
#[must_use = "Sub-builder must be finalized with .done()"]
pub struct UtilmdTransactionBuilder<S = Unset, R = Unset> {
    parent: UtilmdBuilder<S, R>,
    spec: UtilmdTransactionSpec,
}

impl<S, R> UtilmdTransactionBuilder<S, R> {
    /// Set the SG4 **Transaktionsgrund** (`STS+7`, MIG Nr. 00033).
    ///
    /// Takes the whole [`Transaktionsgrund`] rather than a bare code because
    /// the AHB marks the *Ergänzung* Muss alongside the Grund on the GPKE and
    /// `GeLi` Gas core processes: `ZW3`/`ZW4`/`ZW5`/`ZAP` is what tells the
    /// receiver whether the Vorgang is about a verbrauchende or erzeugende
    /// Marktlokation, a Tranche or a ruhende Marktlokation — and therefore
    /// which branch of the answering EBD applies.
    ///
    /// ```rust
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// # use edi_energy::utilmd_codes::{Transaktionsgrund, transaktionsgrund, dtm, loc};
    /// let edi = UtilmdBuilder::new(Release::new("S2.2"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55001).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .transaction("VORGANG-0001")
    ///     .date(dtm::BEGINN_ZUM, "20261101")
    ///     .transaktionsgrund(Transaktionsgrund::verbrauchende_malo(transaktionsgrund::WECHSEL))
    ///     .marktlokation("51238696012")
    ///     .done()
    ///     .serialize()?;
    /// let text = String::from_utf8(edi).unwrap();
    /// assert!(text.contains("IDE+24+VORGANG-0001"));
    /// assert!(text.contains("DTM+92:202611010000?+00:303"));
    /// assert!(text.contains("STS+7++E03+ZW4"));
    /// assert!(text.contains("LOC+Z16+51238696012"));
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn transaktionsgrund(mut self, grund: Transaktionsgrund) -> Self {
        self.spec.transaktionsgrund = Some(grund);
        self
    }

    /// Set the SG4 **Status der Antwort** (`STS+E01`, MIG Nr. 00034).
    ///
    /// Emits `STS+E01++<code>:<codeliste>`. Every Bestätigung and Ablehnung
    /// needs one — the AHB marks the segment Muss and restricts the code to the
    /// named Codeliste's Zustimmungs- or Ablehnungs-Cluster.
    ///
    /// DE 1131 is the **Codeliste**, which is the EBD number only where the AHB
    /// says „EBD-Nummer". Every `WiM` MSB-Wechsel answer names an `S_00xx` (Strom) or
    /// `G_00xx` (Gas) list instead — see [`AntwortStatus::codeliste`].
    ///
    /// ```rust
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// # use edi_energy::utilmd_codes::AntwortStatus;
    /// let edi = UtilmdBuilder::new(Release::new("S2.2"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55011).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .transaction("VORGANG-0001")
    ///     .antwort(AntwortStatus::from_codeliste("A36", "E_0624"))
    ///     .done()
    ///     .serialize()?;
    /// assert!(String::from_utf8(edi).unwrap().contains("STS+E01++A36:E_0624"));
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn antwort(mut self, antwort: AntwortStatus) -> Self {
        self.spec.antwort = Some(antwort);
        self
    }

    /// Add a SG4 process-date segment.
    ///
    /// `qualifier` is DE 2005 — use the [`dtm`](crate::utilmd_codes::dtm)
    /// constants (`92` Beginn zum, `93` Ende zum, `154` ÜT der
    /// Lieferanmeldung, …). `value` is `CCYYMMDD`, or `CCYYMMDDHHMM+00` when
    /// the process needs the UTC instant; the DE 2379 format code follows.
    pub fn date(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec
            .process_dates
            .push((qualifier.into(), value.into()));
        self
    }

    /// Add a `SG5 LOC` location segment.
    ///
    /// Prefer [`marktlokation`](Self::marktlokation) /
    /// [`messlokation`](Self::messlokation); this is the escape hatch for the
    /// rarer Lokationstypen.
    pub fn location(mut self, lokationstyp: Lokationstyp, id: impl Into<String>) -> Self {
        self.spec
            .locations
            .push((lokationstyp.qualifier_code().to_owned(), id.into()));
        self
    }

    /// Add `SG5 LOC+Z16` — the Marktlokation this Vorgang is about.
    pub fn marktlokation(self, malo_id: impl Into<String>) -> Self {
        self.location(Lokationstyp::Marktlokation, malo_id)
    }

    /// Add `SG5 LOC+Z17` — the Messlokation this Vorgang is about.
    pub fn messlokation(self, melo_id: impl Into<String>) -> Self {
        self.location(Lokationstyp::Messlokation, melo_id)
    }

    /// Add a SG6/RFF reference segment.
    /// Set `SG6 RFF+TN` — „Referenz Vorgangsnummer (aus Anfragenachricht)".
    ///
    /// Pass the **request's** `IDE+24` DE 7402. The AHB marks the segment Muss
    /// on every Antwortnachricht, and it is the only correlation the answer
    /// carries: DE 7402 must be globally unique across every `IDE+24` and
    /// `IDE+Z01` ever sent (MIG S2.2, Hinweis zu DE7402), so an answer may not
    /// echo the request's number as its own.
    ///
    /// ```rust
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// # use edi_energy::utilmd_codes::dtm;
    /// let edi = UtilmdBuilder::new(Release::new("S2.2"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55017).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .transaction("ANTWORT-0001")
    ///     .date(dtm::ENDE_ZUM, "20261101")
    ///     .referenz_vorgangsnummer("NNV1234")
    ///     .marktlokation("51238696012")
    ///     .done()
    ///     .serialize()?;
    /// let text = String::from_utf8(edi).unwrap();
    /// assert!(text.contains("IDE+24+ANTWORT-0001"));
    /// assert!(text.contains("RFF+Z13:55017"));
    /// assert!(text.contains("RFF+TN:NNV1234"));
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn referenz_vorgangsnummer(mut self, vorgangsnummer: impl Into<String>) -> Self {
        self.spec.referenz_vorgangsnummer = Some(vorgangsnummer.into());
        self
    }

    /// Add a `SG10 CCI` Merkmal — Klassentyp in DE 7059, value in DE 7037.
    ///
    /// Emits `CCI+<klassentyp>++<wert>`. `GeLi` Gas carries the **Bilanzkreis**
    /// this way (`CCI+Z19`, Muss on 44001), where GPKE Strom uses the
    /// Produktpaket — see [`Self::produktpaket`]. The two Festlegungen model the
    /// same fact differently, so neither shape may be sent on the other's
    /// Sparte.
    pub fn merkmal(mut self, klassentyp: impl Into<String>, wert: impl Into<String>) -> Self {
        self.spec.merkmale.push((klassentyp.into(), wert.into()));
        self
    }

    /// Add a `SG8 SEQ+Z79` Produktpaket to this Vorgang.
    ///
    /// The AHB marks the segment group Muss on every Anmeldung einer Zuordnung
    /// des LFN and on its Bestätigung — 55001, 55077, 55600, 55601, 55014 and
    /// 55608 — and the Codeliste der Konfigurationen 1.4 Kap. 6.1.1 makes the
    /// Bilanzkreis product unconditional inside it.
    ///
    /// ```rust
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// # use edi_energy::utilmd_codes::{Produktpaket, dtm};
    /// let edi = UtilmdBuilder::new(Release::new("S2.2"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55608).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .transaction("VORGANG-0001")
    ///     .date(dtm::BEGINN_ZUM, "20261101")
    ///     .produktpaket(Produktpaket::bilanzkreis("11XBK-EEG-----1"))
    ///     .marktlokation("51238696012")
    ///     .done()
    ///     .serialize()?;
    /// let text = String::from_utf8(edi).unwrap();
    /// assert!(text.contains("SEQ+Z79+1"));
    /// assert!(text.contains("PIA+5+9991000002082:Z11"));
    /// assert!(text.contains("CCI+Z66"));
    /// assert!(text.contains("CAV+ZV4:::11XBK-EEG-----1"));
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn produktpaket(mut self, paket: Produktpaket) -> Self {
        self.spec.produktpakete.push(paket);
        self
    }

    /// Add a SG6/RFF reference segment.
    pub fn reference(mut self, qualifier: impl Into<String>, ref_id: impl Into<String>) -> Self {
        self.spec.references.push((qualifier.into(), ref_id.into()));
        self
    }

    /// Set the SG12/NAD customer segment.
    pub fn customer(mut self, party_qualifier: impl Into<String>, id: impl Into<String>) -> Self {
        self.spec.customer_nad = Some((party_qualifier.into(), id.into()));
        self
    }

    /// Add a free-text (FTX) segment inside SG4.
    ///
    /// `FTX+ACB` carries the Erläuterung the Gas Codelisten require whenever an
    /// Ablehnung uses the catch-all `E14` „Ablehnung Sonstiges".
    pub fn free_text(mut self, text_function: impl Into<String>, text: impl Into<String>) -> Self {
        self.spec
            .free_texts
            .push((text_function.into(), text.into()));
        self
    }

    /// Set the AGR (Agreement Identification) segment inside SG4.
    pub fn agr(
        mut self,
        service_requirement: impl Into<String>,
        response_type: impl Into<String>,
    ) -> Self {
        self.spec.agr = Some((service_requirement.into(), response_type.into()));
        self
    }

    /// Finalize this Vorgang and return to the parent [`UtilmdBuilder`].
    pub fn done(mut self) -> UtilmdBuilder<S, R> {
        self.parent.inner.transactions.push(self.spec);
        self.parent
    }
}

/// The DE 2379 format code an `SG4 DTM` qualifier takes.
///
/// Read off the Anwendungsfall tables of UTILMD AHB Strom 2.2 and Gas 1.2: in
/// SG4 every date qualifier is `303` (`CCYYMMDDHHMMZZZ`, zone `+00` by
/// condition `[931]`) except two — `154` „Annahmedatum eines Angebots" is
/// `102` and `Z10` „Kündigungstermin" is `106`. The date-only qualifiers those
/// AHBs do carry (`752`, `Z09`, `Z20`, `Z21`, `Z22`) all sit in **SG6**, which
/// this builder writes through a different path.
///
/// The code follows the qualifier, never the value's length: a `YYYYMMDD`
/// Vorgangsdatum is still `303`, padded and zoned on the way out.
fn sg4_dtm_format(qualifier: &str) -> &'static str {
    match qualifier {
        "154" => "102",
        "Z10" => "106",
        _ => "303",
    }
}
