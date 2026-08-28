//! [`RemadvBuilder`] — fluent type-safe builder for REMADV messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments};

#[derive(Debug, Clone)]
struct RemadvBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: Option<String>,
    document_id: Option<String>,
    document_date: Option<String>,
    pruefidentifikator: Option<String>,
    waehrung: String,
    rechnung: Option<Rechnungsbezug>,
    abweichungsgruende: Vec<Abweichungsgrund>,
    positionsfehler: Vec<Positionsfehler>,
}

/// `SG10 DLI` + `SG12 AJT` — the defects of **one** Rechnungsposition.
///
/// This is what makes a REMADV *positionsscharf*, and it is the whole reason
/// 33004 („Abweisung Position") exists beside 33003 („Abweisung Kopf und
/// Summe"). `SG10` is Muss on 33004 and „so oft zu wiederholen, bis alle Fehler
/// der Positionsebene genannt sind" (REMADV AHB 1.0a `[525]`); a refusal that
/// names one defect where the walk found four tells the issuer to correct one
/// of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Positionsfehler {
    /// `SG10 DLI` DE 1082 — the Positionsnummer, „Wert aus DE1082 der SG26 in
    /// der sich der nachfolgende Fehler in der INVOIC befindet" (`[526]`).
    pub positionsnummer: u16,
    /// `SG12 AJT` — the codes this position was refused with, and the tree.
    pub gruende: Vec<Abweichungsgrund>,
    /// `SG12 FTX+ABO` — „die Befüllung ergibt sich aus dem zugehörigen EBD"
    /// (`[548]`), i.e. the Erläuterung a catch-all code requires.
    pub erlaeuterung: Option<String>,
}

/// `SG5` — the invoice this REMADV answers, and the two amounts it states.
///
/// **Muss** on every REMADV use case (REMADV AHB 1.0a § 3.1.1 / § 3.1.2,
/// segments 00012–00015). Without it the answer names no invoice: the issuer
/// correlates on `SG5 DOC` DE 1004, which is the BGM DE 1004 of the INVOIC
/// being confirmed (`[515]`) or refused (`[511]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rechnungsbezug {
    /// `SG5 DOC` DE 1001 — `380` Handelsrechnung, `389` selbst ausgestellt,
    /// `457` Storno für Belastung, `Z25` Storno für selbst ausgestellte
    /// Rechnung.
    pub dokumentenart: String,
    /// `SG5 DOC` DE 1004 — the answered invoice's own Dokumentennummer.
    pub rechnungsnummer: String,
    /// `SG5 MOA+9` — Fälliger Betrag inkl. Umsatzsteuer, taken over unchanged from the
    /// invoice's `SG50 MOA+9` (condition `[501]`). Max two decimals.
    pub faelliger_betrag: String,
    /// `SG5 MOA+12` — Überweisungsbetrag. On a **Zahlungsavis** this is the
    /// fällige Betrag, negated for a Gutschrift (`389`/`Z25`, condition `[3]`)
    /// and unchanged otherwise (`380`/`457`, `[4]`). On an **Abweisung** it is
    /// `0` and nothing else: condition `[926]` fixes it, because refusing an
    /// invoice transfers nothing.
    pub ueberweisungsbetrag: String,
    /// `SG5 DTM+137` — the answered invoice's Rechnungsdatum, `CCYYMMDDHHMM`.
    pub rechnungsdatum: String,
}

/// `AJT` — an Abweichungsgrund on a REMADV Rückmeldung.
///
/// The invoice recipient's rejection reason: DE 4465 carries the **Antwortcode**
/// („Code des Prüfschritts") and DE 1082 the **EBD** it is drawn from —
/// `AJT+A70+E_0406'`. Structurally the REMADV twin of UTILMD's
/// `STS+E01++<code>:<ebd>`, and required for the same reason: a rejection
/// without its code gives the sender nothing to correct.
///
/// The MIG places it at two levels — `SG7` on the Kopfebene (once) and `SG12`
/// per Rechnungsposition (up to ten), which is the shape `E_0406`'s
/// Kopf-/Positions-/Summenebene traversal produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abweichungsgrund {
    /// DE 4465 — the Antwortcode.
    pub code: String,
    /// DE 1082 — the EBD that publishes it (`E_0406`, `E_0519`, …).
    pub ebd: Option<String>,
}

impl Abweichungsgrund {
    /// An Abweichungsgrund drawn from a named EBD.
    #[must_use]
    pub fn new(code: impl Into<String>, ebd: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            ebd: Some(ebd.into()),
        }
    }
}

/// Fluent builder for `REMADV` (Remittance Advice) messages.
///
/// Wire type string: `REMADV:D:05A:UN:{release}`.
///
/// # Type-state
///
/// [`build`](RemadvBuilder::build) is only available once both
/// [`sender`](RemadvBuilder::sender) and [`receiver`](RemadvBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::RemadvBuilder;
///
/// let msg = RemadvBuilder::new(Release::new("2.9e"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("4012345000023"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct RemadvBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: RemadvBuilderInner,
}

impl RemadvBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: RemadvBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: None,
                document_id: None,
                document_date: None,
                pruefidentifikator: None,
                // `SG4 CUX` DE 6345. EUR is the only value any EDI@Energy AHB
                // publishes, and DE 6347 `2` / DE 6343 `11` are fixed with it.
                waehrung: "EUR".to_owned(),
                rechnung: None,
                abweichungsgruende: Vec::new(),
                positionsfehler: Vec::new(),
            },
        }
    }
}

impl<S, R> RemadvBuilder<S, R> {
    fn transition<S2, R2>(self) -> RemadvBuilder<S2, R2> {
        RemadvBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> RemadvBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> RemadvBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// `RFF+Z13` — the Prüfidentifikator (`33001`, `33002`, `33003`, `33004`).
    ///
    /// **Muss** on every REMADV use case (REMADV AHB 1.0a, segment 00006). It
    /// is what the receiving system routes on, so a REMADV without it reaches
    /// no process on the other side.
    pub fn pruefidentifikator(mut self, pid: impl Into<String>) -> Self {
        self.inner.pruefidentifikator = Some(pid.into());
        self
    }

    /// `SG5` — the invoice being answered and the amounts. **Muss**.
    pub fn rechnungsbezug(mut self, bezug: Rechnungsbezug) -> Self {
        self.inner.rechnung = Some(bezug);
        self
    }

    /// `SG10`/`SG12` — one entry per refused Rechnungsposition. Muss on 33004.
    pub fn positionsfehler(mut self, fehler: Vec<Positionsfehler>) -> Self {
        self.inner.positionsfehler = fehler;
        self
    }

    /// `SG4 CUX` DE 6345. Defaults to `EUR`, the only published value.
    pub fn waehrung(mut self, code: impl Into<String>) -> Self {
        self.inner.waehrung = code.into();
        self
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

    /// Override the BGM document type code.  Defaults to `"239"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = Some(code.into());
        self
    }

    /// Set the BGM document identifier (Avisnummer).
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

    /// Add an `SG7 AJT` Abweichungsgrund (Kopfebene).
    ///
    /// A REMADV Abweisung must state why: DE 4465 the Antwortcode, DE 1082 the
    /// EBD it comes from.
    ///
    /// ```rust
    /// # use edi_energy::{Release, builders::{RemadvBuilder, Abweichungsgrund}};
    /// let edi = RemadvBuilder::new(Release::new("2.9e"))
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .abweichungsgrund(Abweichungsgrund::new("A70", "E_0406"))
    ///     .serialize()?;
    /// assert!(String::from_utf8(edi).unwrap().contains("AJT+A70+E_0406"));
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn abweichungsgrund(mut self, grund: Abweichungsgrund) -> Self {
        self.inner.abweichungsgruende.push(grund);
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

        let code = self.inner.document_code.as_deref().unwrap_or("239");
        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["REMADV", "D", "05A", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", code, doc_id);
        // `DTM+137` Dokumentendatum. Every EDI@Energy AHB gives DE 2379 as
        // `303` (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to
        // `+00`; `[494]` requires the stamp to be the creation moment or
        // earlier. There is no Anwendungsfall in any AHB that takes `102`.
        emit_comp!(w, "DTM", ["137", &super::ccyymmddhhmm_utc(&dtm_val), "303"]);
        // `RFF+Z13` Prüfidentifikator — Muss, and what the receiving system
        // routes on.
        // `RFF` DE 1153/1154 are components of `C506`, so the qualifier and the
        // value share one composite: `RFF+Z13:33004`, never `RFF+Z13+33004`.
        if let Some(pid) = &self.inner.pruefidentifikator {
            emit_comp!(w, "RFF", ["Z13", pid]);
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
        // `SG4 CUX` — DE 6347 `2` Referenzwährung, DE 6343 `11`
        // Zahlungswährung. Muss.
        // `CUX` DE 6347/6345/6343 are all components of one `C504`.
        emit_comp!(w, "CUX", ["2", &self.inner.waehrung, "11"]);
        // `SG5` — the invoice, its fällige Betrag, the Überweisungsbetrag and
        // its Rechnungsdatum. All Muss.
        if let Some(r) = &self.inner.rechnung {
            // `DOC` DE 1001 is in `C002`, DE 1004 in `C503` — two composites,
            // so the two values sit in two data elements.
            emit_comp!(w, "DOC", [&r.dokumentenart], [&r.rechnungsnummer]);
            // `MOA` DE 5025/5004 are both components of `C516`.
            emit_comp!(w, "MOA", ["9", &r.faelliger_betrag]);
            emit_comp!(w, "MOA", ["12", &r.ueberweisungsbetrag]);
            emit_comp!(
                w,
                "DTM",
                ["137", &super::ccyymmddhhmm_utc(&r.rechnungsdatum), "303"]
            );
        }
        for grund in &self.inner.abweichungsgruende {
            // `SG7 AJT` — Abweichungsgrund auf Kopf- und Summenebene.
            if let Some(ebd) = grund.ebd.as_deref() {
                emit_seg!(w, "AJT", &grund.code, ebd);
            } else {
                emit_seg!(w, "AJT", &grund.code);
            }
        }
        // `SG10 DLI` + `SG12 AJT`/`FTX` — the Positionsebene, repeated until
        // every position-level defect is named (`[525]`).
        for pf in &self.inner.positionsfehler {
            // `DLI` DE 1073/1082 are components of one composite: `DLI+1:7`.
            emit_comp!(w, "DLI", ["1", &pf.positionsnummer.to_string()]);
            for grund in &pf.gruende {
                if let Some(ebd) = grund.ebd.as_deref() {
                    emit_seg!(w, "AJT", &grund.code, ebd);
                } else {
                    emit_seg!(w, "AJT", &grund.code);
                }
            }
            if let Some(text) = pf.erlaeuterung.as_deref() {
                emit_comp!(w, "FTX", ["ABO"], [], [text]);
            }
        }
        // `UNS+S` — Trennung von Positions- und Summenteil. Muss on every use
        // case, and the summary that follows it is a second `MOA+12`: the
        // Überweisungsbetrag, which condition `[926]` fixes to `0` on an
        // Abweisung because refusing an invoice transfers nothing.
        emit_seg!(w, "UNS", "S");
        let summe = self
            .inner
            .rechnung
            .as_ref()
            .map_or("0", |r| r.ueberweisungsbetrag.as_str());
        emit_comp!(w, "MOA", ["12", summe]);
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

impl RemadvBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::remadv::RemadvMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::remadv::RemadvMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::remadv::RemadvMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
