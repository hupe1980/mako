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
    /// `IMD++<7081>` — the coded Produkt-/Leistungsbeschreibung: `Z01` Start
    /// Abo / `Z02` Ende Abo / `Z03` ohne Abo on 17007/17008, `Z10`
    /// Abrechnungsbrennwert und Zustandszahl on 17103.
    merkmal: Option<String>,
    item_description: Option<String>,
    /// `SG29 LIN` with `DTM+163` / `DTM+164` — the Zeitraum a Wertanfrage asks
    /// for, as two `303` instants `(beginn, ende)`.
    positionszeitraum: Option<(String, String)>,
    /// `SG5 CTA+IC` name and its `COM`s as `(DE 3148 address, DE 3155 code)`,
    /// in the sender's `SG2`.
    contact: Option<(String, Vec<(String, String)>)>,
    /// `SG29` positions, numbered from 1 in order.
    positions: Vec<OrdersPosition>,
    /// `DTM+273` Betrachtungszeitintervall, `CCYYMM` (format `610`).
    zeitintervall: Option<String>,
    /// `IMD` DE 7009 beside the coded DE 7081 — `IMD++Z14+Z06` Lieferrichtung
    /// Erzeugung on 17134.
    merkmal_detail: Option<String>,
    /// A second coded `IMD++<7081>` — `Z66`/`Z67` on 17134.
    zweites_merkmal: Option<String>,
    /// `FTX+AFH++<code>:<EBD>` Reklamation (17211).
    reklamation: Option<(String, String)>,
    /// `LOC` in the receiver's `SG2` — `107` Bilanzierungsgebiet / `231`
    /// Regelzone.
    receiver_location: Option<(String, String)>,
    /// `SG3 RFF` in the Meldepunkt's `SG2` — `Z18` Marktlokation, `Z20`
    /// Tranche, `Z59` …
    location_references: Vec<(String, String)>,
    /// Further `SG2 NAD` parties after the Meldepunkt, each with its `SG3 RFF`s.
    parties: Vec<OrdersParty>,
}

/// A further `SG2 NAD` party of an ORDERS — `DEB` MSB der Marktlokation,
/// `SU` Lieferant, `Z31` ÜNB — with the `SG3 RFF` references of its group.
#[derive(Debug, Clone, Default)]
pub struct OrdersParty {
    /// `NAD` DE 3035.
    pub qualifier: String,
    /// `NAD` DE 3039 — the MP-ID.
    pub mp_id: String,
    /// `SG3 RFF` as `(1153, 1154)`.
    pub referenzen: Vec<(String, String)>,
}

/// One `SG29` position of an ORDERS.
///
/// Each field writes the segment the AHB column names for it; an empty field
/// writes nothing.
#[derive(Debug, Clone, Default)]
pub struct OrdersPosition {
    /// `SG29 IMD++<7081>` — the coded Merkmal of the position (`Z46`–`Z49`
    /// Grund der Anforderung on 17004).
    pub merkmal: Option<String>,
    /// `SG29 DTM+7` Gültigkeitsdatum, a `303` instant (17004).
    pub gueltig: Option<String>,
    /// `SG30 CCI+++<7037>` — the Geräteart (17009).
    pub geraeteart: Option<String>,
    /// `SG34 RFF` references as `(1153, 1154)` — `Z03` Positionsnummer des
    /// Angebots (17001), `Z09` Gerätenummer (17009), `AUU` Version (`MaBiS`).
    pub referenzen: Vec<(String, String)>,
    /// `LIN` DE 1229 — the Handlungsbedarf (`Z19` Erforderliches Produkt der
    /// Messlokation on 17011).
    pub handlung: Option<String>,
    /// `PIA+5+<code>:Z11` — a Produkt-Code (17011).
    pub produkt: Option<String>,
    /// `SG30 CCI+<7059>` — a coded Merkmal (Profilgruppe on 17201).
    pub klasse: Option<String>,
    /// `SG38 LOC` as `(3227, 3225)` — `172` Meldepunkt, `237` Bilanzkreis.
    pub orte: Vec<(String, String)>,
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
                merkmal: None,
                item_description: None,
                positionszeitraum: None,
                contact: None,
                positions: Vec::new(),
                zeitintervall: None,
                merkmal_detail: None,
                zweites_merkmal: None,
                reklamation: None,
                receiver_location: None,
                location_references: Vec::new(),
                parties: Vec::new(),
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

    /// Set the document date for DTM+137 — the UTC instant `CCYYMMDDHHMM`
    /// (format 303, optionally `+00`); a bare `CCYYMMDD` is refused with
    /// [`Error::DtmNotAnInstant`](crate::Error::DtmNotAnInstant).
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
    /// **Muss** on ORDERS 17007/17008 (ORDERS AHB 1.1b Kap. 4.15): on a Bestellung
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
        self.inner.merkmal = Some(code.into());
        self
    }

    /// Set the coded `IMD++<7081>` Produkt-/Leistungsbeschreibung — `Z10`
    /// (Abrechnungsbrennwert und Zustandszahl) on ORDERS 17103 (ORDERS AHB
    /// 1.1b Kap. 4.1.3). The same place [`abonnement`](Self::abonnement)
    /// fills on the ESA order PIDs.
    pub fn leistung(mut self, code: impl Into<String>) -> Self {
        self.inner.merkmal = Some(code.into());
        self
    }

    /// Set the Zeitraum of a Wertanfrage — one `SG29 LIN+1` carrying
    /// `DTM+163` (Beginn) and `DTM+164` (Ende), each a UTC instant
    /// `CCYYMMDDHHMM` (format 303, optionally `+00`).
    ///
    /// **Muss** on ORDERS 17103 (ORDERS AHB 1.1b Kap. 4.1.3, „Pro Nachricht ist
    /// die SG29 genau einmal anzugeben"). The Ende is the instant the Zeitraum
    /// ends, i.e. the beginning of the day after its last day (Allgemeine
    /// Festlegungen V6.1d Kap. 3.1).
    pub fn positionszeitraum(mut self, beginn: impl Into<String>, ende: impl Into<String>) -> Self {
        self.inner.positionszeitraum = Some((beginn.into(), ende.into()));
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

    /// Set the sender's `SG5 CTA+IC` Ansprechpartner and its `COM`s, each as
    /// `(DE 3148 address, DE 3155 code)` — Muss on 17001 (ORDERS AHB 1.1b Kap.
    /// 4.9.2).
    pub fn contact<I, A, C>(mut self, name: impl Into<String>, channels: I) -> Self
    where
        I: IntoIterator<Item = (A, C)>,
        A: Into<String>,
        C: Into<String>,
    {
        self.inner.contact = Some((
            name.into(),
            channels
                .into_iter()
                .map(|(a, c)| (a.into(), c.into()))
                .collect(),
        ));
        self
    }

    /// Set `DTM+273` Betrachtungszeitintervall — a month `CCYYMM`, format
    /// `610` (`MaBiS` Anforderungen, ORDERS AHB 1.1b Kap. 5).
    pub fn zeitintervall(mut self, ccyymm: impl Into<String>) -> Self {
        self.inner.zeitintervall = Some(ccyymm.into());
        self
    }

    /// Set the `IMD` DE 7009 beside the coded DE 7081 — `IMD++Z14+Z06` on
    /// 17134 (ORDERS AHB 1.1b Kap. 4.17.1).
    pub fn merkmal_detail(mut self, code: impl Into<String>) -> Self {
        self.inner.merkmal_detail = Some(code.into());
        self
    }

    /// A second coded `IMD++<7081>` after the first (`Z66`/`Z67` on 17134).
    pub fn zweites_merkmal(mut self, code: impl Into<String>) -> Self {
        self.inner.zweites_merkmal = Some(code.into());
        self
    }

    /// Set `FTX+AFH++<code>:<EBD>` — the Reklamation of a Profil (17211,
    /// ORDERS AHB 1.1b Kap. 5.5).
    pub fn reklamation(mut self, code: impl Into<String>, ebd: impl Into<String>) -> Self {
        self.inner.reklamation = Some((code.into(), ebd.into()));
        self
    }

    /// Set the `LOC` of the receiver's `SG2` — `107` Bilanzierungsgebiet,
    /// `231` Regelzone (17203).
    pub fn receiver_location(
        mut self,
        qualifier: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        self.inner.receiver_location = Some((qualifier.into(), id.into()));
        self
    }

    /// Add an `SG3 RFF` to the Meldepunkt's `SG2` — `Z18` Marktlokation on
    /// 17011.
    pub fn location_reference(
        mut self,
        qualifier: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.inner
            .location_references
            .push((qualifier.into(), value.into()));
        self
    }

    /// Add a further `SG2 NAD` party after the Meldepunkt.
    pub fn party(mut self, party: OrdersParty) -> Self {
        self.inner.parties.push(party);
        self
    }

    /// Append an `SG29` position; positions are numbered `LIN+1`, `LIN+2`, …
    pub fn position(mut self, position: OrdersPosition) -> Self {
        self.inner.positions.push(position);
        self
    }

    /// The `SG29` positions, numbered from 1.
    fn emit_positions<W: std::io::Write>(&self, w: &mut Writer<W>) -> Result<(), Error> {
        for (n, position) in self.inner.positions.iter().enumerate() {
            if let Some(h) = &position.handlung {
                emit_seg!(w, "LIN", &(n + 1).to_string(), h);
            } else {
                emit_seg!(w, "LIN", &(n + 1).to_string());
            }
            if let Some(code) = &position.produkt {
                emit_comp!(w, "PIA", ["5"], [code, "Z11"]);
            }
            if let Some(code) = &position.merkmal {
                emit_comp!(w, "IMD", [""], [code]);
            }
            if let Some(instant) = &position.gueltig {
                emit_comp!(
                    w,
                    "DTM",
                    ["7", &super::ccyymmddhhmm_utc("7", instant)?, "303"]
                );
            }
            if let Some(klasse) = &position.klasse {
                emit_seg!(w, "CCI", klasse);
            }
            if let Some(art) = &position.geraeteart {
                emit_comp!(w, "CCI", [""], [""], [art]);
            }
            for (q, v) in &position.referenzen {
                emit_comp!(w, "RFF", [q, v]);
            }
            for (q, id) in &position.orte {
                emit_seg!(w, "LOC", q, id);
            }
        }
        Ok(())
    }

    /// `SG2`: the sender (with its `SG5`), the receiver (with its `LOC`),
    /// the Meldepunkt (with its `SG3 RFF`s) and every further party.
    fn emit_parties<W: std::io::Write>(&self, w: &mut Writer<W>) -> Result<(), Error> {
        // ── SG2: parties + location ──────────────────────────────────────────
        if let Some(id) = &self.inner.sender_id {
            emit_comp!(
                w,
                "NAD",
                ["MS"],
                [id, "", super::agency_for(self.inner.sender_agency, id)?]
            );
            // `SG5` Ansprechpartner belongs to the sender's `SG2`.
            if let Some((name, channels)) = &self.inner.contact {
                emit_comp!(w, "CTA", ["IC"], ["", name]);
                for (address, code) in channels {
                    emit_comp!(w, "COM", [address, code]);
                }
            }
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_comp!(
                w,
                "NAD",
                ["MR"],
                [id, "", super::agency_for(self.inner.receiver_agency, id)?]
            );
            if let Some((q, loc)) = &self.inner.receiver_location {
                emit_seg!(w, "LOC", q, loc);
            }
        }
        if let Some(loc) = &self.inner.location {
            // `SG2` Meldepunkt: the `NAD+DP` opens the group the `LOC+172`
            // belongs to (ORDERS MIG Nr 00028/00029), and its `SG3 RFF`s follow.
            emit_seg!(w, "NAD", "DP");
            emit_seg!(w, "LOC", "172", loc);
            for (q, v) in &self.inner.location_references {
                emit_comp!(w, "RFF", [q, v]);
            }
        }
        for party in &self.inner.parties {
            emit_comp!(
                w,
                "NAD",
                [&party.qualifier],
                [&party.mp_id, "", super::agency_for(None, &party.mp_id)?]
            );
            for (q, v) in &party.referenzen {
                emit_comp!(w, "RFF", [q, v]);
            }
        }
        Ok(())
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
        emit_comp!(
            w,
            "DTM",
            ["137", &super::ccyymmddhhmm_utc("137", &dtm_val)?, "303"]
        );
        // `DTM+203` Ausführungsdatum, format 303 (CCYYMMDDHHMMZZZ).
        if let Some(ausf) = &self.inner.ausfuehrungsdatum {
            emit_comp!(
                w,
                "DTM",
                ["203", &super::ccyymmddhhmm_utc("203", ausf)?, "303"]
            );
        }
        // `DTM+273` Betrachtungszeitintervall — format `610` `CCYYMM`.
        if let Some(monat) = &self.inner.zeitintervall {
            emit_comp!(w, "DTM", ["273", monat, "610"]);
        }
        // Abonnement — `IMD++<7081>`. DE 7081 sits in C272 (element 2), which
        // is why DE 7077 (element 1) stays empty; DE 7009 is C273 (element 3).
        if let Some(code) = &self.inner.merkmal {
            if let Some(detail) = &self.inner.merkmal_detail {
                emit_comp!(w, "IMD", [""], [code], [detail]);
            } else {
                emit_comp!(w, "IMD", [""], [code]);
            }
        }
        if let Some(code) = &self.inner.zweites_merkmal {
            emit_comp!(w, "IMD", [""], [code]);
        }
        // `FTX+AFH++<code>:<EBD>` — the Reklamation (Nr 00014), after the IMDs.
        if let Some((code, ebd)) = &self.inner.reklamation {
            emit_comp!(w, "FTX", ["AFH"], [""], [code, ebd]);
        }
        // Free-form item description (7077 = A), for the non-ESA order PIDs.
        if self.inner.merkmal.is_none() && self.inner.item_description.is_some() {
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
        self.emit_parties(&mut w)?;
        // ── SG29: Positionsdaten — the Zeitraum of a Wertanfrage ─────────────
        if let Some((beginn, ende)) = &self.inner.positionszeitraum {
            emit_seg!(w, "LIN", "1");
            emit_comp!(
                w,
                "DTM",
                ["163", &super::ccyymmddhhmm_utc("163", beginn)?, "303"]
            );
            emit_comp!(
                w,
                "DTM",
                ["164", &super::ccyymmddhhmm_utc("164", ende)?, "303"]
            );
        }
        self.emit_positions(&mut w)?;
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
