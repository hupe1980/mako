//! Veräußerungsformen and the Direktvermarktungspflicht — §§ 20–22 EEG 2023.
//!
//! The EEG does not contain a section called „Pflicht zur Direktvermarktung".
//! The duty is the **shadow of § 21 Abs. 1 Satz 1 Nr. 1**: the paid
//! Einspeisevergütung with a gesetzlich bestimmtem anzulegenden Wert exists only
//! „für Strom aus Anlagen mit einer installierten Leistung von **bis zu 100
//! Kilowatt**". A larger plant that wants a payment at all has to take the
//! Marktprämie (§ 20) — which is only payable for months in which the Strom is
//! *direkt vermarktet* — and that is what makes it direktvermarktungspflichtig.
//!
//! ## The sections this module works from
//!
//! | § | Subject |
//! |---|---|
//! | § 20 | **Marktprämie** — payable only for months in which the Strom is direkt vermarktet (Satz 1 Nr. 1) and balanced in a Bilanz- oder Unterbilanzkreis holding nothing else (Nr. 3) |
//! | § 21 Abs. 1 | **Einspeisevergütung** — Nr. 1 ≤ 100 kW mit gesetzlichem AW, Nr. 2 < 200 kW zum Anspruch null (unentgeltliche Abnahme), Nr. 3 Ausfallvergütung, Nr. 4 ausgeförderte Anlagen |
//! | § 21a | **Sonstige Direktvermarktung** — marketing without any § 19 Abs. 1 claim |
//! | § 21b | **Zuordnung zu einer Veräußerungsform, Wechsel** — Abs. 1 Satz 1 lists the four forms, Satz 2 allows a change only „zum ersten Kalendertag eines Monats" |
//! | § 21c | **Verfahren** — Abs. 1 Satz 1: the Mitteilung is due *before the start of the preceding calendar month*; Satz 2 gives the Ausfallvergütung a Werktag deadline instead |
//! | § 22 | **Wettbewerbliche Ermittlung der Marktprämie** — Abs. 2–4 say which plants need a Zuschlag, Abs. 5 Satz 2 names the technologies whose AW stays gesetzlich bestimmt |
//!
//! ## § 20 Satz 1 Nr. 3 is why a residual share matters
//!
//! The Marktprämie requires the Strom to be balanced in a Bilanz- oder
//! Unterbilanzkreis in which *nothing but* direkt vermarkteter EE-Strom is
//! balanced. A share of a direktvermarktungspflichtige Marktlokation left in the
//! Netzbetreiber's own Bilanzkreis therefore does not merely look untidy — it
//! costs the operator the claim. That is the reason `E_0623` Prüfschritt 540
//! asks about the Direktvermarktungspflicht at all, and why its `A55` triggers
//! „Herstellung einer 100 % LF-Zuordnung".
//!
//! ## No separate Managementprämie
//!
//! Anlage 1 Nr. 3.1.2 EEG 2023 is `MP = AW – MW` and nothing more. Since the
//! EEG 2014 the marketing cost sits *inside* the anzulegender Wert; the additive
//! Managementprämie of the EEG 2012 era is gone. Its mirror image on the
//! Einspeisevergütung route is the § 53 Abs. 1 deduction of 0,4 / 0,2 ct.

use rust_decimal::Decimal;
use rust_decimal::dec;
use time::Date;
use time::macros::date;

use crate::technology::ErzeugungsArt;

// ── Direktvermarktungspflicht ─────────────────────────────────────────────────

/// § 21 Abs. 1 Satz 1 Nr. 1 EEG — the installed capacity up to which a plant may
/// still claim the Einspeisevergütung mit gesetzlich bestimmtem anzulegenden
/// Wert. Above it, only a Direktvermarktung pays.
///
/// The wording is „bis zu 100 Kilowatt", so 100 kW itself is still inside.
pub const DIREKTVERMARKTUNG_PFLICHT_KW: Decimal = dec!(100);

/// The first Inbetriebnahmedatum for which mako states the threshold.
///
/// The 100-kW wording is verified in EEG 2017, EEG 2021 and EEG 2023, each of
/// which governs plants commissioned from 2016-01-01 (§ 100). The staged EEG
/// 2014 thresholds that applied to earlier plants are **not** in mako's
/// regulatory corpus, so this module does not assert them — see
/// [`direktvermarktungspflicht`].
pub const SCHWELLE_VERIFIZIERT_AB: Date = date!(2016 - 01 - 01);

/// Whether the plant is **direktvermarktungspflichtig**.
///
/// `None` means the question is unanswered rather than answered „nein": a plant
/// commissioned before [`SCHWELLE_VERIFIZIERT_AB`] falls under the EEG 2014
/// staging (500 kW from 01.08.2014, 100 kW from 01.01.2016) or, earlier still,
/// under an EEG that knew no duty at all — neither text is in mako's regulatory
/// mirror, and inventing a threshold here would decide a settlement and an
/// `E_0623` Prüfschritt on an unsourced number.
///
/// A caller that gets `None` should escalate, exactly as it would for any other
/// fact it cannot read.
///
/// # Example
///
/// ```rust
/// use eeg_billing::direktverm::direktvermarktungspflicht;
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// // 150 kW, commissioned 2024 — the Einspeisevergütung is closed to it.
/// assert_eq!(direktvermarktungspflicht(dec!(150), date!(2024-05-01)), Some(true));
/// // Exactly 100 kW is still „bis zu 100 Kilowatt".
/// assert_eq!(direktvermarktungspflicht(dec!(100), date!(2024-05-01)), Some(false));
/// // 2013: governed by a text mako does not mirror.
/// assert_eq!(direktvermarktungspflicht(dec!(600), date!(2013-05-01)), None);
/// ```
#[must_use]
pub fn direktvermarktungspflicht(leistung_kw: Decimal, inbetriebnahme: Date) -> Option<bool> {
    (inbetriebnahme >= SCHWELLE_VERIFIZIERT_AB)
        .then_some(leistung_kw > DIREKTVERMARKTUNG_PFLICHT_KW)
}

// ── Ausschreibungspflicht ─────────────────────────────────────────────────────

/// Which Ausschreibungssegment a Solaranlage belongs to — § 3 Nr. 41a / 41b.
///
/// The names are counter-intuitive and getting them the wrong way round moves
/// the threshold by 250 kW:
///
/// - **erstes Segment** (§ 3 Nr. 41a) — „jede Freiflächenanlage und jede
///   Solaranlage auf, an oder in einer baulichen Anlage, die weder Gebäude noch
///   Lärmschutzwand ist". Exempt bis einschließlich **1 MW**.
/// - **zweites Segment** (§ 3 Nr. 41b) — „jede Solaranlage auf, an oder in einem
///   Gebäude oder einer Lärmschutzwand". Exempt bis einschließlich **750 kW**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SolarSegment {
    /// Freiflächenanlagen and solar on baulichen Anlagen that are neither
    /// Gebäude nor Lärmschutzwand — § 3 Nr. 41a.
    Erstes,
    /// Solar on, at or in a Gebäude or a Lärmschutzwand — § 3 Nr. 41b.
    Zweites,
}

impl SolarSegment {
    /// § 22 Abs. 3 Satz 2 EEG 2023 — the installed capacity up to and including
    /// which this segment is exempt from the Zuschlags-/Zahlungsberechtigungs-
    /// erfordernis.
    #[must_use]
    pub fn ausschreibungsfreie_leistung_kw(self) -> Decimal {
        match self {
            Self::Erstes => dec!(1000),
            Self::Zweites => dec!(750),
        }
    }
}

/// § 22 Abs. 2 Satz 2 Nr. 1 EEG 2023 — Windenergieanlagen an Land bis
/// einschließlich 1 Megawatt need no Zuschlag.
pub const WIND_AN_LAND_AUSSCHREIBUNGSFREI_KW: Decimal = dec!(1000);

/// § 22 Abs. 4 Satz 2 EEG 2023 — Biomasseanlagen bis einschließlich 150 Kilowatt
/// need no Zuschlag, „es sei denn, es handelt sich um bestehende
/// Biomasseanlagen nach § 39g".
pub const BIOMASSE_AUSSCHREIBUNGSFREI_KW: Decimal = dec!(150);

/// Whether a plant needs a BNetzA Zuschlag (resp. Zahlungsberechtigung) before a
/// § 19 Abs. 1 claim exists — § 22 Abs. 2 to 5 EEG 2023.
///
/// `solar_segment` is read only for solar technologies and ignored otherwise;
/// pass [`SolarSegment::Erstes`] for a Freiflächenanlage and
/// [`SolarSegment::Zweites`] for anything on a Gebäude or a Lärmschutzwand.
///
/// ## What this deliberately does **not** decide
///
/// Two of the § 22 exemptions are not capacity tests and cannot be answered
/// here, so a plant that qualifies for one will be reported as needing a
/// Zuschlag when it does not:
///
/// - Pilotwindenergieanlagen an Land, capped at 125 MW per year in total
///   (Abs. 2 Satz 2 Nr. 2) — a property of the year's cohort, not of the plant.
/// - Anlagen von Bürgerenergiegesellschaften — Wind bis 18 MW, Solar bis 6 MW,
///   „nach Maßgabe des § 22b" (Abs. 2 Satz 2 Nr. 3, Abs. 3 Satz 2 Nr. 2) — a
///   property of the operator.
///
/// Likewise § 22 Abs. 4 Satz 2's carve-out for bestehende Biomasseanlagen nach
/// § 39g is not applied: those need a Zuschlag *regardless* of size, and whether
/// a plant is one is register data.
///
/// # Example
///
/// ```rust
/// use eeg_billing::direktverm::{SolarSegment, requires_ausschreibung};
/// use eeg_billing::ErzeugungsArt;
/// use rust_decimal::dec;
///
/// // A 900 kW rooftop plant is zweites Segment: above 750 kW, so it needs one.
/// assert!(requires_ausschreibung(dec!(900), ErzeugungsArt::SolarAufdach, SolarSegment::Zweites));
/// // The same 900 kW on a field is erstes Segment and stays exempt.
/// assert!(!requires_ausschreibung(dec!(900), ErzeugungsArt::SolarFreiflaeche, SolarSegment::Erstes));
/// // Wind an Land is exempt bis einschließlich 1 MW.
/// assert!(!requires_ausschreibung(dec!(1000), ErzeugungsArt::WindOnshore, SolarSegment::Erstes));
/// // Wasserkraft is never tendered — § 22 Abs. 5 Satz 2 keeps its AW statutory.
/// assert!(!requires_ausschreibung(dec!(5000), ErzeugungsArt::Wasserkraft, SolarSegment::Erstes));
/// ```
#[must_use]
pub fn requires_ausschreibung(
    leistung_kw: Decimal,
    art: ErzeugungsArt,
    solar_segment: SolarSegment,
) -> bool {
    match art {
        // § 22 Abs. 3 — Solaranlagen, by segment.
        ErzeugungsArt::SolarAufdach
        | ErzeugungsArt::SolarFreiflaeche
        | ErzeugungsArt::SolarAgriPv
        | ErzeugungsArt::SolarMieterstrom
        | ErzeugungsArt::SolarStecker => {
            leistung_kw > solar_segment.ausschreibungsfreie_leistung_kw()
        }

        // § 22 Abs. 2 — Windenergieanlagen an Land.
        ErzeugungsArt::WindOnshore => leistung_kw > WIND_AN_LAND_AUSSCHREIBUNGSFREI_KW,

        // Offshore sits outside the EEG's own rate sections: the Zuschlag comes
        // from the Windenergie-auf-See-Gesetz, to which § 22 Abs. 1 refers.
        ErzeugungsArt::WindOffshore => true,

        // § 22 Abs. 4 — Biomasseanlagen. `BiomassHolz` is feste Biomasse and
        // settles as Biomasse; Biomethan is excluded from the § 42 rate but not
        // from the Ausschreibungserfordernis.
        ErzeugungsArt::Biomasse
        | ErzeugungsArt::BiomassHolz
        | ErzeugungsArt::Biogas
        | ErzeugungsArt::Biomethan => leistung_kw > BIOMASSE_AUSSCHREIBUNGSFREI_KW,

        // § 22 Abs. 5 Satz 2 — „Für Anlagen nach Satz 1 und für Anlagen zur
        // Erzeugung von Strom aus Wasserkraft, Deponiegas, Klärgas, Grubengas
        // oder Geothermie wird die Höhe des anzulegenden Werts durch die
        // §§ 40 bis 49 gesetzlich bestimmt." No Ausschreibung exists for them at
        // any size. Gezeitenenergie and KWKG have no EEG-Ausschreibung either.
        ErzeugungsArt::Wasserkraft
        | ErzeugungsArt::Geothermie
        | ErzeugungsArt::Klaergas
        | ErzeugungsArt::Grubengas
        | ErzeugungsArt::Deponiegas
        | ErzeugungsArt::Gezeiten
        | ErzeugungsArt::Kwk => false,
    }
}

// ── Veräußerungsformen ────────────────────────────────────────────────────────

/// The four Veräußerungsformen of § 21b Abs. 1 Satz 1 EEG 2023.
///
/// Every Anlage is assigned to exactly one of them (or, per Abs. 2, to several
/// with percentage shares). The variants keep the statute's own order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Veraeusserungsform {
    /// Nr. 1 — die Marktprämie nach § 20. A Direktvermarktung.
    Marktpraemie,
    /// Nr. 2 — die Einspeisevergütung nach § 21 Abs. 1 Satz 1, in one of its
    /// four Varianten.
    Einspeiseverguetung(EinspeiseverguetungsVariante),
    /// Nr. 3 — der Mieterstromzuschlag nach § 21 Abs. 3.
    Mieterstromzuschlag,
    /// Nr. 4 — die sonstige Direktvermarktung nach § 21a. A Direktvermarktung
    /// with no § 19 Abs. 1 claim at all.
    SonstigeDirektvermarktung,
}

/// The four Varianten § 21 Abs. 1 Satz 1 EEG 2023 gives the Einspeisevergütung.
///
/// They are not interchangeable: only the first is closed to a
/// direktvermarktungspflichtige Anlage, which is why the whole enum exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum EinspeiseverguetungsVariante {
    /// Nr. 1 — bis zu 100 kW, anzulegender Wert gesetzlich bestimmt, gekürzt
    /// nach § 53 Abs. 1. **This is the one the Direktvermarktungspflicht
    /// closes.**
    GesetzlicherWert,
    /// Nr. 2 — weniger als 200 kW, „dabei verringert sich in diesem Fall der
    /// Anspruch auf null": the unentgeltliche Abnahme. Open to a plant above
    /// 100 kW, because it pays nothing.
    UnentgeltlicheAbnahme,
    /// Nr. 3 — Ausfallvergütung: bis zu drei aufeinanderfolgende und insgesamt
    /// sechs Kalendermonate pro Jahr für Anlagen über 100 kW, gekürzt nach
    /// § 53 Abs. 3. It exists *for* plants that are otherwise in the
    /// Direktvermarktung.
    ///
    /// Under EEG 2017 this was Nr. 2; the EEG 2023 inserted the unentgeltliche
    /// Abnahme in front of it.
    Ausfallverguetung,
    /// Nr. 4 — ausgeförderte Anlagen, gekürzt nach § 53 Abs. 4.
    AusgefoerderteAnlage,
}

impl Veraeusserungsform {
    /// Whether this form is a **Direktvermarktung** in the sense of § 20 /
    /// § 21a — the two forms in which the Strom is sold to a third party.
    #[must_use]
    pub fn ist_direktvermarktung(self) -> bool {
        matches!(self, Self::Marktpraemie | Self::SonstigeDirektvermarktung)
    }
}

// ── Zuordnung und Wechsel ─────────────────────────────────────────────────────

/// Why a Zuordnung to a Veräußerungsform is not admissible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WechselAblehnung {
    /// § 21b Abs. 1 Satz 2 — „Sie dürfen mit jeder Anlage nur zum ersten
    /// Kalendertag eines Monats zwischen den Veräußerungsformen wechseln."
    NichtZumMonatsersten,
    /// § 21c Abs. 1 Satz 1 — the Mitteilung must reach the Netzbetreiber „vor
    /// Beginn des jeweils vorangehenden Kalendermonats". A change effective
    /// 1 July therefore has to be notified before 1 June.
    ///
    /// Satz 2 replaces this with a Werktag deadline (bis zum fünftletzten
    /// Werktag des Vormonats) when the change is into or out of the
    /// Ausfallvergütung; that variant needs a Werktagskalender and is decided by
    /// the caller, not here.
    MitteilungZuSpaet {
        /// The last day on which the Mitteilung would have been in time — the
        /// day before the first of the preceding calendar month.
        spaetestens_am: Date,
    },
    /// § 21 Abs. 1 Satz 1 Nr. 1 — the Einspeisevergütung mit gesetzlichem Wert
    /// is only open to plants bis zu 100 kW. A larger plant may still take the
    /// unentgeltliche Abnahme, the Ausfallvergütung or, once ausgefördert,
    /// Nr. 4.
    DirektvermarktungspflichtVerletzt,
    /// § 21b Abs. 1 Satz 4 — the Ausfallvergütung is closed to an Anlage that
    /// „innerhalb der letzten 24 Monate zumindest zeitweise der unentgeltlichen
    /// Abnahme zugeordnet war".
    AusfallverguetungNachUnentgeltlicherAbnahme,
}

/// The facts § 21b/§ 21c decide a Zuordnung or a Wechsel on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wechsel {
    /// The form the Anlage is to be assigned to.
    pub ziel: Veraeusserungsform,
    /// The day the change takes effect. § 21b Abs. 1 Satz 2 requires the first
    /// of a calendar month.
    pub wirksam_ab: Date,
    /// The day the Mitteilung reached the Netzbetreiber — § 21c Abs. 1 Satz 1.
    pub mitgeteilt_am: Date,
    /// Installed capacity in kW, for § 21 Abs. 1 Satz 1 Nr. 1.
    pub leistung_kw: Decimal,
    /// Inbetriebnahmedatum, so the Direktvermarktungspflicht is answered on the
    /// regime that actually governs the plant.
    pub inbetriebnahme: Date,
    /// § 21b Abs. 1 Satz 4 — was the Anlage assigned to the unentgeltliche
    /// Abnahme at any point in the last 24 months?
    pub unentgeltliche_abnahme_in_24_monaten: bool,
}

/// Decide a Zuordnung or a Wechsel under § 21b Abs. 1 and § 21c Abs. 1 Satz 1.
///
/// # Errors
///
/// Returns the first rule the request breaks, in the statute's own order:
/// the Monatserste (§ 21b Abs. 1 Satz 2), the Mitteilungsfrist (§ 21c Abs. 1
/// Satz 1), the Direktvermarktungspflicht (§ 21 Abs. 1 Satz 1 Nr. 1) and the
/// Ausfallvergütungssperre (§ 21b Abs. 1 Satz 4).
///
/// A plant whose Direktvermarktungspflicht is unanswerable — see
/// [`direktvermarktungspflicht`] — is **not** refused on that ground; the other
/// three rules still apply.
///
/// # Example
///
/// ```rust
/// use eeg_billing::direktverm::{
///     EinspeiseverguetungsVariante, Veraeusserungsform, Wechsel, WechselAblehnung,
///     validate_wechsel,
/// };
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// let ziel = Veraeusserungsform::Einspeiseverguetung(
///     EinspeiseverguetungsVariante::GesetzlicherWert,
/// );
/// let w = Wechsel {
///     ziel,
///     wirksam_ab: date!(2026-07-01),
///     mitgeteilt_am: date!(2026-05-20), // before 1 June — in time
///     leistung_kw: dec!(80),
///     inbetriebnahme: date!(2024-03-01),
///     unentgeltliche_abnahme_in_24_monaten: false,
/// };
/// assert!(validate_wechsel(&w).is_ok());
///
/// // A day late: § 21c Abs. 1 Satz 1 wants it before the preceding month begins.
/// let spaet = Wechsel { mitgeteilt_am: date!(2026-06-01), ..w };
/// assert!(matches!(
///     validate_wechsel(&spaet),
///     Err(WechselAblehnung::MitteilungZuSpaet { .. })
/// ));
///
/// // 150 kW cannot take the Einspeisevergütung mit gesetzlichem Wert.
/// let gross = Wechsel { leistung_kw: dec!(150), ..w };
/// assert_eq!(
///     validate_wechsel(&gross),
///     Err(WechselAblehnung::DirektvermarktungspflichtVerletzt)
/// );
/// ```
pub fn validate_wechsel(w: &Wechsel) -> Result<(), WechselAblehnung> {
    if w.wirksam_ab.day() != 1 {
        return Err(WechselAblehnung::NichtZumMonatsersten);
    }
    let spaetestens_am = mitteilungsfrist(w.wirksam_ab);
    if w.mitgeteilt_am > spaetestens_am {
        return Err(WechselAblehnung::MitteilungZuSpaet { spaetestens_am });
    }
    if let Veraeusserungsform::Einspeiseverguetung(variante) = w.ziel {
        if variante == EinspeiseverguetungsVariante::GesetzlicherWert
            && direktvermarktungspflicht(w.leistung_kw, w.inbetriebnahme) == Some(true)
        {
            return Err(WechselAblehnung::DirektvermarktungspflichtVerletzt);
        }
        if variante == EinspeiseverguetungsVariante::Ausfallverguetung
            && w.unentgeltliche_abnahme_in_24_monaten
        {
            return Err(WechselAblehnung::AusfallverguetungNachUnentgeltlicherAbnahme);
        }
    }
    Ok(())
}

/// § 21c Abs. 1 Satz 1 — the last day on which a Mitteilung for a change
/// effective on `wirksam_ab` is still „vor Beginn des jeweils vorangehenden
/// Kalendermonats".
///
/// For 1 July that is 31 May: the preceding calendar month is June, and the
/// notice has to be in before it begins.
///
/// # Panics
///
/// Never for a date the calendar can express; `wirksam_ab` is expected to be the
/// first of a month, which [`validate_wechsel`] checks first.
#[must_use]
pub fn mitteilungsfrist(wirksam_ab: Date) -> Date {
    let vormonat = wirksam_ab.replace_day(1).unwrap_or(wirksam_ab);
    // One day before the first of the preceding calendar month.
    vormonat
        .previous_day()
        .and_then(|d| d.replace_day(1).ok())
        .and_then(time::Date::previous_day)
        .unwrap_or(wirksam_ab)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Direktvermarktungspflicht ────────────────────────────────────────────

    #[test]
    fn the_threshold_is_bis_zu_100_kw() {
        let ibn = date!(2024 - 01 - 01);
        assert_eq!(direktvermarktungspflicht(dec!(100.001), ibn), Some(true));
        // „bis zu 100 Kilowatt" includes 100 itself.
        assert_eq!(direktvermarktungspflicht(dec!(100), ibn), Some(false));
        assert_eq!(direktvermarktungspflicht(dec!(50), ibn), Some(false));
    }

    #[test]
    fn a_pre_2016_plant_is_unanswered_not_exempt() {
        // The EEG 2014 staging (500 kW, then 100 kW) is outside the corpus, so
        // the honest answer is „unknown" — a `false` here would let a 600 kW
        // plant keep an Einspeisevergütung it may not be entitled to.
        assert_eq!(
            direktvermarktungspflicht(dec!(600), date!(2015 - 12 - 31)),
            None
        );
        assert_eq!(
            direktvermarktungspflicht(dec!(600), date!(2016 - 01 - 01)),
            Some(true)
        );
    }

    // ── Ausschreibungspflicht ────────────────────────────────────────────────

    #[test]
    fn solar_thresholds_follow_the_segment_not_the_technology() {
        // § 22 Abs. 3 Satz 2 Nr. 1 — erstes Segment bis einschließlich 1 MW.
        assert!(!requires_ausschreibung(
            dec!(1000),
            ErzeugungsArt::SolarFreiflaeche,
            SolarSegment::Erstes
        ));
        assert!(requires_ausschreibung(
            dec!(1000.001),
            ErzeugungsArt::SolarFreiflaeche,
            SolarSegment::Erstes
        ));
        // Nr. 1a — zweites Segment bis einschließlich 750 kW. The 900 kW rooftop
        // plant the old flat 1-MW rule let through is the whole point.
        assert!(!requires_ausschreibung(
            dec!(750),
            ErzeugungsArt::SolarAufdach,
            SolarSegment::Zweites
        ));
        assert!(requires_ausschreibung(
            dec!(900),
            ErzeugungsArt::SolarAufdach,
            SolarSegment::Zweites
        ));
    }

    #[test]
    fn agri_pv_is_erstes_segment_and_has_no_6_mw_exemption() {
        // Six megawatts appears exactly once in the EEG 2023 — as the
        // Bürgerenergiegesellschaften exemption of § 22 Abs. 3 Satz 2 Nr. 2. It
        // is not an Agri-PV rule.
        assert!(requires_ausschreibung(
            dec!(5000),
            ErzeugungsArt::SolarAgriPv,
            SolarSegment::Erstes
        ));
    }

    #[test]
    fn wind_an_land_is_exempt_up_to_one_megawatt() {
        assert!(!requires_ausschreibung(
            dec!(1000),
            ErzeugungsArt::WindOnshore,
            SolarSegment::Erstes
        ));
        assert!(requires_ausschreibung(
            dec!(1000.001),
            ErzeugungsArt::WindOnshore,
            SolarSegment::Erstes
        ));
    }

    #[test]
    fn biomasse_is_exempt_up_to_150_kw() {
        assert!(!requires_ausschreibung(
            dec!(150),
            ErzeugungsArt::Biomasse,
            SolarSegment::Erstes
        ));
        assert!(requires_ausschreibung(
            dec!(151),
            ErzeugungsArt::Biogas,
            SolarSegment::Erstes
        ));
    }

    #[test]
    fn statutory_technologies_are_never_tendered() {
        // § 22 Abs. 5 Satz 2 names them: Wasserkraft, Deponiegas, Klärgas,
        // Grubengas, Geothermie. Any size.
        for art in [
            ErzeugungsArt::Wasserkraft,
            ErzeugungsArt::Geothermie,
            ErzeugungsArt::Deponiegas,
            ErzeugungsArt::Klaergas,
            ErzeugungsArt::Grubengas,
        ] {
            assert!(
                !requires_ausschreibung(dec!(50000), art, SolarSegment::Erstes),
                "{art:?} has no EEG-Ausschreibung"
            );
        }
    }

    // ── Veräußerungsformen ───────────────────────────────────────────────────

    #[test]
    fn only_two_forms_are_a_direktvermarktung() {
        assert!(Veraeusserungsform::Marktpraemie.ist_direktvermarktung());
        assert!(Veraeusserungsform::SonstigeDirektvermarktung.ist_direktvermarktung());
        assert!(!Veraeusserungsform::Mieterstromzuschlag.ist_direktvermarktung());
        assert!(
            !Veraeusserungsform::Einspeiseverguetung(
                EinspeiseverguetungsVariante::GesetzlicherWert
            )
            .ist_direktvermarktung()
        );
    }

    // ── Zuordnung und Wechsel ────────────────────────────────────────────────

    fn wechsel(ziel: Veraeusserungsform) -> Wechsel {
        Wechsel {
            ziel,
            wirksam_ab: date!(2026 - 07 - 01),
            mitgeteilt_am: date!(2026 - 05 - 20),
            leistung_kw: dec!(80),
            inbetriebnahme: date!(2024 - 03 - 01),
            unentgeltliche_abnahme_in_24_monaten: false,
        }
    }

    #[test]
    fn the_mitteilungsfrist_is_the_end_of_the_month_before_last() {
        assert_eq!(
            mitteilungsfrist(date!(2026 - 07 - 01)),
            date!(2026 - 05 - 31)
        );
        assert_eq!(
            mitteilungsfrist(date!(2026 - 01 - 01)),
            date!(2025 - 11 - 30)
        );
        assert_eq!(
            mitteilungsfrist(date!(2026 - 03 - 01)),
            date!(2026 - 01 - 31)
        );
    }

    #[test]
    fn a_change_only_takes_effect_on_the_first() {
        let w = Wechsel {
            wirksam_ab: date!(2026 - 07 - 15),
            ..wechsel(Veraeusserungsform::Marktpraemie)
        };
        assert_eq!(
            validate_wechsel(&w),
            Err(WechselAblehnung::NichtZumMonatsersten)
        );
    }

    #[test]
    fn the_notice_must_precede_the_preceding_month() {
        let base = wechsel(Veraeusserungsform::Marktpraemie);
        assert!(
            validate_wechsel(&Wechsel {
                mitgeteilt_am: date!(2026 - 05 - 31),
                ..base
            })
            .is_ok()
        );
        assert_eq!(
            validate_wechsel(&Wechsel {
                mitgeteilt_am: date!(2026 - 06 - 01),
                ..base
            }),
            Err(WechselAblehnung::MitteilungZuSpaet {
                spaetestens_am: date!(2026 - 05 - 31)
            })
        );
    }

    #[test]
    fn a_large_plant_may_not_take_the_statutory_einspeiseverguetung() {
        let ziel =
            Veraeusserungsform::Einspeiseverguetung(EinspeiseverguetungsVariante::GesetzlicherWert);
        let w = Wechsel {
            leistung_kw: dec!(150),
            ..wechsel(ziel)
        };
        assert_eq!(
            validate_wechsel(&w),
            Err(WechselAblehnung::DirektvermarktungspflichtVerletzt)
        );
    }

    #[test]
    fn a_large_plant_may_still_take_the_other_three_varianten() {
        for variante in [
            EinspeiseverguetungsVariante::UnentgeltlicheAbnahme,
            EinspeiseverguetungsVariante::Ausfallverguetung,
            EinspeiseverguetungsVariante::AusgefoerderteAnlage,
        ] {
            let w = Wechsel {
                leistung_kw: dec!(150),
                ..wechsel(Veraeusserungsform::Einspeiseverguetung(variante))
            };
            assert!(validate_wechsel(&w).is_ok(), "{variante:?}");
        }
    }

    #[test]
    fn the_ausfallverguetung_is_closed_after_an_unentgeltliche_abnahme() {
        let ziel = Veraeusserungsform::Einspeiseverguetung(
            EinspeiseverguetungsVariante::Ausfallverguetung,
        );
        let w = Wechsel {
            unentgeltliche_abnahme_in_24_monaten: true,
            ..wechsel(ziel)
        };
        assert_eq!(
            validate_wechsel(&w),
            Err(WechselAblehnung::AusfallverguetungNachUnentgeltlicherAbnahme)
        );
    }

    #[test]
    fn an_unanswerable_direktvermarktungspflicht_does_not_refuse() {
        let ziel =
            Veraeusserungsform::Einspeiseverguetung(EinspeiseverguetungsVariante::GesetzlicherWert);
        let w = Wechsel {
            leistung_kw: dec!(600),
            inbetriebnahme: date!(2013 - 05 - 01),
            ..wechsel(ziel)
        };
        assert!(validate_wechsel(&w).is_ok());
    }
}
