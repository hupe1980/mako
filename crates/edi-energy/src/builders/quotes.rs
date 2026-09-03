//! [`QuotesBuilder`] — fluent type-safe builder for QUOTES messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments};

/// DE 2379 unit of a `DTM` that carries a **duration** rather than a date.
///
/// QUOTES AHB 1.1a §4.3 states the Bindungsfrist (`DTM+273`) and the
/// Einrichtungszeitspanne (`DTM+279`) as a count plus one of these units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DauerEinheit {
    /// `802` — Monat.
    Monat,
    /// `803` — Woche.
    Woche,
    /// `804` — Tag.
    Tag,
}

impl DauerEinheit {
    /// DE 2379 code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Monat => "802",
            Self::Woche => "803",
            Self::Tag => "804",
        }
    }

    /// Parse a DE 2379 code that denotes a duration unit.
    #[must_use]
    pub const fn from_code(code: &str) -> Option<Self> {
        match code.as_bytes() {
            b"802" => Some(Self::Monat),
            b"803" => Some(Self::Woche),
            b"804" => Some(Self::Tag),
            _ => None,
        }
    }

    /// Resolve `count` of these units against `anchor`.
    ///
    /// `DTM+273` and `DTM+279` state a **span**, not a date, and a span runs
    /// from the document that states it — the `DTM+137` Dokumentendatum of the
    /// QUOTES. Resolving against the moment the message is parsed instead makes
    /// the same message mean different things on a replay and silently extends
    /// a Bindungsfrist that a queued message should already have spent.
    ///
    /// A `Monat` is a calendar month, clamped to the last day of the target
    /// month (31 January + 1 Monat is 28/29 February). Returns `None` only when
    /// the result leaves the representable calendar range.
    #[must_use]
    pub fn resolve_from(
        self,
        anchor: time::OffsetDateTime,
        count: i64,
    ) -> Option<time::OffsetDateTime> {
        match self {
            Self::Monat => add_calendar_months(anchor, count),
            Self::Woche => anchor.checked_add(time::Duration::weeks(count)),
            Self::Tag => anchor.checked_add(time::Duration::days(count)),
        }
    }
}

/// Add `count` calendar months to `at`, clamping the day to the target month.
fn add_calendar_months(at: time::OffsetDateTime, count: i64) -> Option<time::OffsetDateTime> {
    let total = i64::from(at.year()) * 12 + i64::from(at.month() as u8) - 1 + count;
    let year = i32::try_from(total.div_euclid(12)).ok()?;
    let month = time::Month::try_from(u8::try_from(total.rem_euclid(12) + 1).ok()?).ok()?;
    let day = at.day().min(time::util::days_in_month(month, year));
    let date = time::Date::from_calendar_date(year, month, day).ok()?;
    Some(at.replace_date(date))
}

#[derive(Debug, Clone)]
struct QuotesBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: Option<AgencyCode>,
    receiver_agency: Option<AgencyCode>,
    message_ref: String,
    document_id: Option<String>,
    document_date: Option<String>,
    location: Option<String>,
    pruefidentifikator: Option<u32>,
    /// BGM DE 1001. Defaults to `310`; the ESA Angebot uses `Z57`.
    document_code: Option<String>,
    /// SG1 references as `(1153 qualifier, 1154 value)`. Additive.
    references: Vec<(String, String)>,
    /// `DTM+469` Startdatum/-zeitpunkt, frühestes — Muss on the ESA Angebot.
    fruehester_start: Option<String>,
    /// `DTM+279` Einrichtungszeitspanne as `(count, 2379 unit)`.
    einrichtungszeit: Option<(String, String)>,
    /// `NAD+DP` Liefer-/Bezugsort — Muss on the ESA Angebot.
    delivery_party: bool,
    /// SG27 `PIA+Z02` Artikel-IDs.
    artikel_ids: Vec<String>,
    /// SG27 `PIA+5 … :SRW` OBIS-Kennzahlen.
    obis: Vec<String>,
    /// SG31 prices as `(5118 Betrag, 5387 Preisart, 6411 Mengeneinheit)`.
    preise: Vec<(String, String, String)>,
    bindungsfrist: Option<String>,
    reason: Option<String>,
    // Additive ESA-Angebot (PID 15003) content — only emitted when set, so the
    // Geräteübernahme Angebote (15001/15002) that share this builder are unaffected.
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
                sender_agency: None,
                receiver_agency: None,
                message_ref: "1".to_owned(),
                document_id: None,
                document_date: None,
                location: None,
                pruefidentifikator: None,
                document_code: None,
                references: Vec::new(),
                fruehester_start: None,
                einrichtungszeit: None,
                delivery_party: false,
                artikel_ids: Vec::new(),
                obis: Vec::new(),
                preise: Vec::new(),
                bindungsfrist: None,
                reason: None,
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

    /// Set the location (MaLo-ID / ZPB / NeLo-ID) this Angebot concerns.
    ///
    /// Emits `LOC+172+<id>` so the ESA can correlate the answer to the process
    /// it started (the QUOTES otherwise carries no location).
    pub fn location(mut self, id: impl Into<String>) -> Self {
        self.inner.location = Some(id.into());
        self
    }

    /// Set the Bindungsfrist (Gültigkeitsdauer des Angebots) — emits
    /// `DTM+273+<count>:<unit>`.
    ///
    /// **This is a duration, not a date.** QUOTES AHB 1.1a §4.3 gives DE 2380
    /// as „Zeitraum“ with condition `[908]` („Mögliche Werte: 1 bis n“) and
    /// DE 2379 as `802` Monat / `803` Woche / `804` Tag. Rendering a
    /// A `CCYYMMDD` here is an eight-digit number where the AHB expects a
    /// count, and a receiver parsing it as a date finds none — which is how
    /// an Angebot gets read as an Ablehnung, since the segment's presence is
    /// what tells the two apart.
    ///
    /// Use [`DauerEinheit`] for `unit`.
    pub fn bindungsfrist(mut self, count: impl Into<String>, unit: DauerEinheit) -> Self {
        self.inner.bindungsfrist = Some(format!("{}\u{1}{}", count.into(), unit.as_str()));
        self
    }

    /// Set the BGM DE 1001 document code. Defaults to `310`.
    ///
    /// The ESA Angebot uses `Z57` („Übermittlung von Werten an ESA“).
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = Some(code.into());
        self
    }

    /// Set `DTM+469` — „Startdatum oder -zeitpunkt, frühestes/r“
    /// (`CCYYMMDDHHMM`). **Muss** on the ESA Angebot: it is when the MSB can
    /// first deliver, answering the ESA's Wunschtermin.
    pub fn fruehester_start(mut self, ccyymmddhhmm: impl Into<String>) -> Self {
        self.inner.fruehester_start = Some(ccyymmddhhmm.into());
        self
    }

    /// Set `DTM+279` — „Vom Bestelleingangsdatum bis Lieferdatum“, the lead
    /// time the MSB needs to set the delivery up. A *Kann* segment.
    pub fn einrichtungszeit(mut self, count: impl Into<String>, unit: DauerEinheit) -> Self {
        self.inner.einrichtungszeit = Some((count.into(), unit.as_str().to_owned()));
        self
    }

    /// Emit the `NAD+DP` Liefer-/Bezugsort party that introduces the `LOC+172`
    /// Meldepunkt group. **Muss** on the ESA Angebot.
    pub fn delivery_party(mut self) -> Self {
        self.inner.delivery_party = true;
        self
    }

    /// Add an SG27 `PIA+Z02+<id>:Z09` Artikel-ID.
    ///
    /// QUOTES AHB 1.1a §4.3 condition `[2042]`: at least one, at most three —
    /// and the last two digits of each ID select the matching
    /// [`price`](Self::price) kind (`01` Einrichtung, `02` Betrieb, `03`
    /// Transaktion).
    pub fn artikel_id(mut self, id: impl Into<String>) -> Self {
        self.inner.artikel_ids.push(id.into());
        self
    }

    /// Add an SG27 `PIA+5+<obis>:SRW` OBIS-Kennzahl for the offered values.
    pub fn obis_kennzahl(mut self, obis: impl Into<String>) -> Self {
        self.inner.obis.push(obis.into());
        self
    }

    /// Add an SG31 price — emits `PRI+CAL:<betrag>:<art>::1:<einheit>`.
    ///
    /// `art` ∈ `{Z01 Einrichtungspreis, Z02 Transaktionspreis, Z03
    /// Betriebspreis}`, `einheit` ∈ `{H87 Stück, DAY Tag}` (`DAY` only with
    /// `Z03`). One per Artikel-ID (condition `[2071]`).
    pub fn preis(
        mut self,
        betrag: impl Into<String>,
        art: impl Into<String>,
        einheit: impl Into<String>,
    ) -> Self {
        self.inner
            .preise
            .push((betrag.into(), art.into(), einheit.into()));
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

    /// Add an SG1 reference `RFF+<qual>:<value>`. **Additive** — call once per
    /// reference the AHB requires.
    ///
    /// The QUOTES MIG restricts SG1 `1153` to `{AAV, ACW, Z13}`. The ESA
    /// Angebot needs `AAV` (the REQOTE's Belegnummer — its published
    /// Zuordnungsschlüssel `ZG-T16`) **and** `Z13`, which is why there is no
    /// single-slot variant: one would silently drop the other.
    pub fn reference(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner.references.push((qualifier.into(), value.into()));
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
            .map_or_else(super::now_ccyymmddhhmm, str::to_owned);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        // BGM DE 1004 is the **Dokumentennummer** (QUOTES AHB 1.1a); the
        // Prüfidentifikator has its own `SG1 RFF+Z13`. Defaults to the message
        // reference so the document always carries a number.
        let bgm_1004 = self
            .inner
            .document_id
            .as_deref()
            .unwrap_or(&self.inner.message_ref);
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["QUOTES", "D", "10A", "UN", self.inner.release.as_str()]
        );
        emit_seg!(
            w,
            "BGM",
            self.inner.document_code.as_deref().unwrap_or("310"),
            bgm_1004
        );
        // `DTM+137` Dokumentendatum. Every EDI@Energy AHB gives DE 2379 as
        // `303` (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to
        // `+00`; `[494]` requires the stamp to be the creation moment or
        // earlier. There is no Anwendungsfall in any AHB that takes `102`.
        emit_comp!(w, "DTM", ["137", &super::ccyymmddhhmm_utc(&dtm_val), "303"]);
        // `DTM+469` — frühester Start der Übermittlung, format 303.
        if let Some(start) = &self.inner.fruehester_start {
            emit_comp!(w, "DTM", ["469", &super::ccyymmddhhmm_utc(start), "303"]);
        }
        // `DTM+279` — Einrichtungszeitspanne (count + 802/803/804 unit).
        if let Some((count, unit)) = &self.inner.einrichtungszeit {
            emit_comp!(w, "DTM", ["279", count, unit]);
        }
        // Bindungsfrist (Gültigkeitsdauer, DE 2005 = 273) — a *duration*
        // (count + 802/803/804 unit), present only on an Angebot.
        if let Some(bf) = &self.inner.bindungsfrist {
            let (count, unit) = bf.split_once('\u{1}').unwrap_or((bf.as_str(), "804"));
            emit_comp!(w, "DTM", ["273", count, unit]);
        }
        // Ablehnungsgrund (Ablehnung der Anfrage) — top-level FTX (DE 4451 = ACB),
        // before the SG1 reference group.
        if let Some(reason) = &self.inner.reason {
            emit_comp!(w, "FTX", ["ACB"], [""], [""], [reason]);
        }
        // ── SG1: references ──────────────────────────────────────────────────
        // The MIG lists the Referenz places (`RFF+AAG`, `RFF+ON`, `RFF+AAV`)
        // before the Prüfidentifikator's.
        for (q, v) in &self.inner.references {
            emit_comp!(w, "RFF", [q, v]);
        }
        if let Some(pid) = self.inner.pruefidentifikator {
            emit_comp!(w, "RFF", ["Z13", &pid.to_string()]);
        }
        // ── SG4: currency (CUX+2:<ISO>:4) ────────────────────────────────────
        if let Some(iso) = &self.inner.currency {
            emit_comp!(w, "CUX", ["2", iso, "4"]);
        }
        // ── SG11: parties + location ─────────────────────────────────────────
        // Segment sequence per the MIG: `NAD+MS` with its `SG14 CTA`/`COM`,
        // then `NAD+MR`, then `NAD+DP` with its `LOC+172`.
        if let Some(id) = &self.inner.sender_id {
            emit_comp!(
                w,
                "NAD",
                ["MS"],
                [id, "", super::agency_for(self.inner.sender_agency, id)]
            );
        }
        // `SG14 CTA`/`COM` — the Ansprechpartner of the sender, inside the
        // sender's `SG11`.
        if let Some((name, comm)) = &self.inner.contact {
            emit_comp!(w, "CTA", ["IC"], ["", name]);
            emit_comp!(w, "COM", [comm, "EM"]);
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_comp!(
                w,
                "NAD",
                ["MR"],
                [id, "", super::agency_for(self.inner.receiver_agency, id)]
            );
        }
        // ── SG11: Liefer-/Bezugsort + Meldepunkt ─────────────────────────────
        if self.inner.delivery_party {
            emit_seg!(w, "NAD", "DP");
        }
        if let Some(loc) = &self.inner.location {
            emit_seg!(w, "LOC", "172", loc);
        }
        // ── SG27: Messprodukt line ───────────────────────────────────────────
        //
        // `LIN+1+Z67` introduces the offered Messprodukt; `PIA+5+<code>:Z11`
        // names it (7143 is the *second* component of C212), `PIA+Z02` the
        // Artikel-IDs and `PIA+5+<obis>:SRW` the OBIS-Kennzahlen.
        if let Some(product) = &self.inner.product {
            emit_seg!(w, "LIN", "1", "Z67");
            emit_comp!(w, "PIA", ["5"], [product, "Z11"]);
            for id in &self.inner.artikel_ids {
                emit_comp!(w, "PIA", ["Z02"], [id, "Z09"]);
            }
            for obis in &self.inner.obis {
                emit_comp!(w, "PIA", ["5"], [obis, "SRW"]);
            }
            // ── SG31: prices ─────────────────────────────────────────────────
            // `PRI+CAL:<5118>::<5387>:<5284>:<6411>` — the Preisart (`Z01`
            // Arbeitspreis, `Z02` Grundpreis, `Z03` Betriebspreis) is DE 5387,
            // DE 5375 is not used; Einzelpreisbasis 5284 is fixed to 1 by
            // condition [903].
            for (betrag, art, einheit) in &self.inner.preise {
                emit_comp!(w, "PRI", ["CAL", betrag, "", art, "1", einheit]);
            }
            if self.inner.preise.is_empty()
                && let Some(price) = &self.inner.price
            {
                emit_comp!(w, "PRI", ["CAL", price]);
            }
        }
        // `UNS+S` — Muss on every Anwendungsfall.
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
