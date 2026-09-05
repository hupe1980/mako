//! §§ 7–9 KWKG — Höhe und Dauer des Zuschlags für KWK-Strom.
//!
//! Two things about § 7 shape this module and are easy to get wrong:
//!
//! 1. **The rate ladders are per Leistungsanteil, not per plant.** Every Nummer
//!    of § 7 Abs. 1 and Abs. 2 reads „für den KWK-Leistungsanteil von …", so a
//!    2-MW plant is paid 8 ct on its first 50 kW, 6 ct on the next 50, 5 ct on
//!    the next 150 and 4,4 ct on the remaining 1 750 — a blended Mischsatz, not
//!    the top band's rate on the whole capacity. Taking one band's rate for the
//!    whole plant underpays every plant above 50 kW.
//! 2. **Abs. 1 and Abs. 2 are different ladders.** Abs. 1 prices KWK-Strom „der
//!    in ein Netz der allgemeinen Versorgung eingespeist wird"; Abs. 2 prices
//!    KWK-Strom that is not, at markedly lower rates that depend on which
//!    Nummer of § 6 Abs. 3 opens the claim. [`KwkVerwendung`] forces the caller
//!    to say which, so no plant is paid Abs. 1 rates for Abs. 2 electricity.
//!
//! 3. **§ 35 Abs. 20 Satz 1 dates Nr. 5.** The band above 2 MW is „anzuwenden
//!    auf KWK-Anlagen, die nach dem 31. Dezember 2020 den Dauerbetrieb
//!    aufgenommen oder nach einer erfolgten Modernisierung wieder aufgenommen
//!    haben". An older plant above 2 MW is priced by § 7 Abs. 4 KWKG in its
//!    31 December 2020 version (Satz 2), which is not this module's ladder.
//!
//! § 8 measures the Förderdauer in Vollbenutzungsstunden and gives a KWK plant
//! no calendar Förderende: Abs. 1–3 set a lifetime figure and Abs. 4 caps „pro
//! Kalenderjahr […] **bis zu**" a falling number of them. Both are counters
//! against generation, so neither can be resolved to a date in advance.
//!
//! The rates live in § 7 itself. There is no Anlage to § 7.

use crate::rounding::RoundMoney;
use rust_decimal::Decimal;
use rust_decimal::dec;
use time::Date;

// ── Plant classification ──────────────────────────────────────────────────────

/// Which of the three KWKG plant classes a plant belongs to (§ 6 Abs. 1).
///
/// The class decides the § 7 Abs. 1 Nr. 5 rate above 2 MW and the § 8
/// Förderdauer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum KwkAnlagenart {
    /// Neue KWK-Anlage — § 7 Abs. 1 Satz 1 Nr. 5 lit. a, § 8 Abs. 1.
    Neu,
    /// Modernisierte KWK-Anlage — § 7 Abs. 1 Satz 1 Nr. 5 lit. b, § 8 Abs. 2.
    Modernisiert,
    /// Nachgerüstete KWK-Anlage — § 7 Abs. 1 Satz 1 Nr. 5 lit. c, § 8 Abs. 3.
    Nachgeruestet,
}

/// What the KWK-Strom is used for — which of § 7 Abs. 1, Abs. 2 or Abs. 3 prices it.
///
/// § 7 Abs. 2 does not have one ladder but three, one per Nummer of § 6 Abs. 3,
/// so the variant names the Nummer that opens the claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum KwkVerwendung {
    /// § 7 Abs. 1 — KWK-Strom, der in ein Netz der allgemeinen Versorgung
    /// eingespeist wird.
    NetzDerAllgemeinenVersorgung,
    /// § 7 Abs. 2 Nr. 1 i.V.m. § 6 Abs. 3 Nr. 1 — nicht eingespeist, Anlage mit
    /// einer elektrischen KWK-Leistung von bis zu 100 Kilowatt.
    NichtEingespeistBis100Kw,
    /// § 7 Abs. 2 Nr. 2 i.V.m. § 6 Abs. 3 Nr. 2 — nicht eingespeist, Lieferung
    /// an Letztverbraucher in einer Kundenanlage oder einem geschlossenen
    /// Verteilernetz.
    NichtEingespeistKundenanlage,
    /// § 7 Abs. 2 Nr. 3 i.V.m. § 6 Abs. 3 Nr. 3 — nicht eingespeist, Einsatz in
    /// einem stromkostenintensiven Unternehmen, das den KWK-Strom selbst
    /// verbraucht.
    NichtEingespeistStromkostenintensiv,
    /// § 7 Abs. 3 i.V.m. § 6 Abs. 3 Nr. 4 — nicht eingespeist, Betreiber einer
    /// Branche nach Anlage 2 des Energiefinanzierungsgesetzes.
    ///
    /// The statute sets no rate: it leaves the Zuschlag to a Verordnung nach
    /// § 33 Abs. 2 Nr. 1, capped at the difference between Gesamtgestehungskosten
    /// and Marktpreis. [`zuschlag_ct_kwh`] answers `None` for it rather than
    /// borrowing another Absatz's ladder.
    NichtEingespeistBrancheAnlage2,
}

impl KwkVerwendung {
    /// Whether the KWK-Strom is fed into a Netz der allgemeinen Versorgung.
    #[must_use]
    pub fn ist_eingespeist(self) -> bool {
        self == Self::NetzDerAllgemeinenVersorgung
    }
}

// ── § 7 rate ladders ──────────────────────────────────────────────────────────

/// One band of a § 7 Staffel: the upper bound of the Leistungsanteil in kW
/// (`None` = open top) and its rate in ct/kWh.
type Staffel = &'static [(Option<Decimal>, Decimal)];

/// § 7 Abs. 1 Satz 1 Nr. 1–4 — the bands below 2 MW, identical for all three
/// Anlagenarten. Nr. 5 supplies the open top and is appended per Anlagenart.
static ABS1_UNTER_2MW: [(Option<Decimal>, Decimal); 4] = [
    (Some(dec!(50)), dec!(8)),
    (Some(dec!(100)), dec!(6)),
    (Some(dec!(250)), dec!(5)),
    (Some(dec!(2_000)), dec!(4.4)),
];

/// § 7 Abs. 1 Satz 1 Nr. 5 lit. a und b — neue und modernisierte KWK-Anlagen.
const ABS1_NR5_NEU_MODERNISIERT: Decimal = dec!(3.4);

/// § 7 Abs. 1 Satz 1 Nr. 5 lit. c — nachgerüstete KWK-Anlagen.
const ABS1_NR5_NACHGERUESTET: Decimal = dec!(3.1);

/// § 7 Abs. 1 Satz 2 — the uplift on Nr. 5 lit. a.
const ABS1_SATZ2_ERHOEHUNG_CT: Decimal = dec!(0.5);

/// § 35 Abs. 18 KWKG — § 7 Abs. 1 Satz 2 is not applied to plants that took up
/// operation before this date.
const SATZ2_FRUEHESTE_INBETRIEBNAHME: Date = time::macros::date!(2023 - 01 - 01);

/// § 35 Abs. 20 Satz 1 KWKG — „§ 7 Absatz 1 Satz 1 Nummer 5 ist anzuwenden auf
/// KWK-Anlagen, die **nach dem 31. Dezember 2020** den Dauerbetrieb aufgenommen
/// oder nach einer erfolgten Modernisierung wieder aufgenommen haben."
///
/// Nr. 5 is the only band above 2 MW, so for a plant that started earlier the
/// Abs. 1 ladder stops at 2 MW and prices nothing beyond it. Satz 2 sends those
/// plants to § 7 Abs. 4 KWKG in its 31 December 2020 version, which this crate
/// does not carry — [`zuschlag_ct_kwh`] answers `None` rather than paying them a
/// rate the current § 7 does not give them.
const ABS1_NR5_FRUEHESTER_DAUERBETRIEB: Date = time::macros::date!(2020 - 12 - 31);

/// § 7 Abs. 2 Nr. 1 — § 6 Abs. 3 Nr. 1 plants. The ladder closes at 100 kW,
/// because § 6 Abs. 3 Nr. 1 opens the claim only up to that capacity.
static ABS2_NR1: [(Option<Decimal>, Decimal); 2] =
    [(Some(dec!(50)), dec!(4)), (Some(dec!(100)), dec!(3))];

/// § 7 Abs. 2 Nr. 2 — Lieferung an Letztverbraucher in einer Kundenanlage oder
/// einem geschlossenen Verteilernetz.
static ABS2_NR2: [(Option<Decimal>, Decimal); 5] = [
    (Some(dec!(50)), dec!(4)),
    (Some(dec!(100)), dec!(3)),
    (Some(dec!(250)), dec!(2)),
    (Some(dec!(2_000)), dec!(1.5)),
    (None, dec!(1)),
];

/// § 7 Abs. 2 Nr. 3 — Einsatz in stromkostenintensiven Unternehmen.
static ABS2_NR3: [(Option<Decimal>, Decimal); 4] = [
    (Some(dec!(50)), dec!(5.41)),
    (Some(dec!(250)), dec!(4)),
    (Some(dec!(2_000)), dec!(2.4)),
    (None, dec!(1.8)),
];

/// § 7 Abs. 3a — the flat rates for **neue** KWK-Anlagen up to 50 kW.
///
/// Abs. 3a is keyed on the plant's KWK-Leistung, not on a Leistungsanteil, so it
/// is a flat rate on the whole plant and displaces the Abs. 1 / Abs. 2 ladders
/// for the plants it covers.
const ABS3A_EINGESPEIST_CT: Decimal = dec!(16);
/// § 7 Abs. 3a Nr. 2 — the same plants where the KWK-Strom is not fed in.
const ABS3A_NICHT_EINGESPEIST_CT: Decimal = dec!(8);
/// § 7 Abs. 3a upper capacity bound.
const ABS3A_GRENZE_KW: Decimal = dec!(50);

/// § 35 Abs. 17 Satz 2 KWKG — § 7 Abs. 3a is applied from the 2020 calendar year
/// to plants that took up Dauerbetrieb after this date.
const ABS3A_FRUEHESTER_DAUERBETRIEB: Date = time::macros::date!(2019 - 12 - 31);

// ── Input ─────────────────────────────────────────────────────────────────────

/// Everything § 7 needs to price one plant's KWK-Strom.
#[derive(Debug, Clone, PartialEq)]
pub struct KwkZuschlagInput {
    /// The plant's elektrische KWK-Leistung in kW.
    pub kwk_leistung_kw: Decimal,
    /// Which § 6 Abs. 1 class the plant belongs to.
    pub anlagenart: KwkAnlagenart,
    /// Whether the KWK-Strom is fed into a Netz der allgemeinen Versorgung, and
    /// if not, which Nummer of § 6 Abs. 3 opens the claim.
    pub verwendung: KwkVerwendung,
    /// Date the plant took up (or, after a Modernisierung, resumed) Dauerbetrieb.
    ///
    /// § 35 gates two rate rules on it: Abs. 18 excludes plants before 1 January
    /// 2023 from the § 7 Abs. 1 Satz 2 uplift, and Abs. 17 Satz 2 applies § 7
    /// Abs. 3a to plants after 31 December 2019.
    pub dauerbetrieb: Date,
    /// § 7 Abs. 1 Satz 2 — whether the Bundesministerium für Wirtschaft und
    /// Energie found the uplift on Nr. 5 lit. a angemessen and published that
    /// finding in the Bundesanzeiger.
    ///
    /// The uplift is payable only where it did, so this defaults to `false` and
    /// the caller has to assert the publication before the 0,5 ct is paid.
    pub bmwk_feststellung_veroeffentlicht: bool,
}

impl KwkZuschlagInput {
    /// A new plant of `kwk_leistung_kw` feeding a Netz der allgemeinen
    /// Versorgung, without the § 7 Abs. 1 Satz 2 uplift.
    #[must_use]
    pub fn neu_eingespeist(kwk_leistung_kw: Decimal, dauerbetrieb: Date) -> Self {
        Self {
            kwk_leistung_kw,
            anlagenart: KwkAnlagenart::Neu,
            verwendung: KwkVerwendung::NetzDerAllgemeinenVersorgung,
            dauerbetrieb,
            bmwk_feststellung_veroeffentlicht: false,
        }
    }
}

// ── Output ────────────────────────────────────────────────────────────────────

/// One priced Leistungsanteil of a § 7 Staffel — the audit trail behind the
/// blended rate.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KwkLeistungsanteil {
    /// Lower bound of the Leistungsanteil in kW (exclusive).
    pub von_kw: Decimal,
    /// Upper bound in kW (inclusive), `None` for the open top band.
    pub bis_kw: Option<Decimal>,
    /// How much of the plant's capacity falls in this band, in kW.
    pub anteil_kw: Decimal,
    /// The band's rate in ct/kWh, including the § 7 Abs. 1 Satz 2 uplift where
    /// that is payable.
    pub satz_ct: Decimal,
    /// The Nummer the rate rests on.
    pub rechtsgrundlage: &'static str,
}

// ── The computation ───────────────────────────────────────────────────────────

fn abs1_staffel(anlagenart: KwkAnlagenart) -> (Staffel, Decimal, &'static str) {
    let (top_ct, top_basis) = match anlagenart {
        KwkAnlagenart::Neu => (
            ABS1_NR5_NEU_MODERNISIERT,
            "§ 7 Abs. 1 Satz 1 Nr. 5 lit. a KWKG",
        ),
        KwkAnlagenart::Modernisiert => (
            ABS1_NR5_NEU_MODERNISIERT,
            "§ 7 Abs. 1 Satz 1 Nr. 5 lit. b KWKG",
        ),
        KwkAnlagenart::Nachgeruestet => (
            ABS1_NR5_NACHGERUESTET,
            "§ 7 Abs. 1 Satz 1 Nr. 5 lit. c KWKG",
        ),
    };
    (&ABS1_UNTER_2MW, top_ct, top_basis)
}

/// The Staffel that prices this plant, plus the citation for each band.
fn staffel(input: &KwkZuschlagInput) -> Option<Vec<(Option<Decimal>, Decimal, &'static str)>> {
    let mut bands: Vec<(Option<Decimal>, Decimal, &'static str)> = Vec::new();
    match input.verwendung {
        KwkVerwendung::NetzDerAllgemeinenVersorgung => {
            let (unter_2mw, top_ct, top_basis) = abs1_staffel(input.anlagenart);
            const NUMMERN: [&str; 4] = [
                "§ 7 Abs. 1 Satz 1 Nr. 1 KWKG",
                "§ 7 Abs. 1 Satz 1 Nr. 2 KWKG",
                "§ 7 Abs. 1 Satz 1 Nr. 3 KWKG",
                "§ 7 Abs. 1 Satz 1 Nr. 4 KWKG",
            ];
            for (i, (bis, ct)) in unter_2mw.iter().enumerate() {
                bands.push((*bis, *ct, NUMMERN[i]));
            }
            // § 7 Abs. 1 Satz 2: the uplift is on Nr. 5 lit. a only, runs from
            // 1 January 2023, and is payable only where the Bundesministerium
            // published the Angemessenheits-Feststellung in the Bundesanzeiger.
            // § 35 Abs. 18 excludes plants that started before that date.
            let satz2 = input.anlagenart == KwkAnlagenart::Neu
                && input.bmwk_feststellung_veroeffentlicht
                && input.dauerbetrieb >= SATZ2_FRUEHESTE_INBETRIEBNAHME;
            // § 35 Abs. 20 Satz 1: Nr. 5 reaches only plants in Dauerbetrieb
            // after 31 December 2020. Leaving the band off closes the ladder at
            // 2 MW, which is what prices an older plant above that at nothing
            // here rather than at a rate it has no claim to.
            if input.dauerbetrieb > ABS1_NR5_FRUEHESTER_DAUERBETRIEB {
                let top = if satz2 {
                    top_ct + ABS1_SATZ2_ERHOEHUNG_CT
                } else {
                    top_ct
                };
                bands.push((None, top, top_basis));
            }
        }
        KwkVerwendung::NichtEingespeistBis100Kw => {
            for (i, (bis, ct)) in ABS2_NR1.iter().enumerate() {
                bands.push((
                    *bis,
                    *ct,
                    [
                        "§ 7 Abs. 2 Nr. 1 lit. a KWKG",
                        "§ 7 Abs. 2 Nr. 1 lit. b KWKG",
                    ][i],
                ));
            }
        }
        KwkVerwendung::NichtEingespeistKundenanlage => {
            const LIT: [&str; 5] = [
                "§ 7 Abs. 2 Nr. 2 lit. a KWKG",
                "§ 7 Abs. 2 Nr. 2 lit. b KWKG",
                "§ 7 Abs. 2 Nr. 2 lit. c KWKG",
                "§ 7 Abs. 2 Nr. 2 lit. d KWKG",
                "§ 7 Abs. 2 Nr. 2 lit. e KWKG",
            ];
            for (i, (bis, ct)) in ABS2_NR2.iter().enumerate() {
                bands.push((*bis, *ct, LIT[i]));
            }
        }
        KwkVerwendung::NichtEingespeistStromkostenintensiv => {
            const LIT: [&str; 4] = [
                "§ 7 Abs. 2 Nr. 3 lit. a KWKG",
                "§ 7 Abs. 2 Nr. 3 lit. b KWKG",
                "§ 7 Abs. 2 Nr. 3 lit. c KWKG",
                "§ 7 Abs. 2 Nr. 3 lit. d KWKG",
            ];
            for (i, (bis, ct)) in ABS2_NR3.iter().enumerate() {
                bands.push((*bis, *ct, LIT[i]));
            }
        }
        // § 7 Abs. 3 leaves the rate to a Verordnung nach § 33 Abs. 2 Nr. 1.
        KwkVerwendung::NichtEingespeistBrancheAnlage2 => return None,
    }
    Some(bands)
}

/// Whether § 7 Abs. 3a displaces the Staffeln for this plant, and at what rate.
fn abs3a_satz_ct(input: &KwkZuschlagInput) -> Option<(Decimal, &'static str)> {
    if input.anlagenart != KwkAnlagenart::Neu
        || input.kwk_leistung_kw > ABS3A_GRENZE_KW
        || input.dauerbetrieb <= ABS3A_FRUEHESTER_DAUERBETRIEB
    {
        return None;
    }
    match input.verwendung {
        KwkVerwendung::NetzDerAllgemeinenVersorgung => {
            Some((ABS3A_EINGESPEIST_CT, "§ 7 Abs. 3a Nr. 1 KWKG"))
        }
        KwkVerwendung::NichtEingespeistBrancheAnlage2 => None,
        _ => Some((ABS3A_NICHT_EINGESPEIST_CT, "§ 7 Abs. 3a Nr. 2 KWKG")),
    }
}

/// Split a plant's KWK-Leistung across the § 7 Staffel that prices it.
///
/// Returns `None` where the statute sets no rate: a § 7 Abs. 3 plant (the rate
/// is left to a Verordnung), a plant with a non-positive capacity, or one whose
/// capacity runs past the top of a closed ladder. Two ladders close: § 7 Abs. 2
/// Nr. 1 stops at 100 kW, which is exactly where § 6 Abs. 3 Nr. 1 stops opening
/// the claim, and § 7 Abs. 1 stops at 2 MW for a plant § 35 Abs. 20 Satz 1
/// keeps out of Nr. 5.
///
/// ```rust
/// use eeg_billing::kwkg::{KwkZuschlagInput, zuschlag_leistungsanteile};
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// let anteile = zuschlag_leistungsanteile(&KwkZuschlagInput::neu_eingespeist(
///     dec!(2000),
///     date!(2024 - 03 - 01),
/// ))
/// .unwrap();
/// // 50 + 50 + 150 + 1 750 kW, priced 8 / 6 / 5 / 4,4 ct.
/// assert_eq!(anteile.len(), 4);
/// assert_eq!(anteile[3].anteil_kw, dec!(1750));
/// assert_eq!(anteile[3].satz_ct, dec!(4.4));
/// ```
#[must_use]
pub fn zuschlag_leistungsanteile(input: &KwkZuschlagInput) -> Option<Vec<KwkLeistungsanteil>> {
    if input.kwk_leistung_kw <= Decimal::ZERO {
        return None;
    }
    if let Some((ct, basis)) = abs3a_satz_ct(input) {
        return Some(vec![KwkLeistungsanteil {
            von_kw: Decimal::ZERO,
            bis_kw: Some(ABS3A_GRENZE_KW),
            anteil_kw: input.kwk_leistung_kw,
            satz_ct: ct,
            rechtsgrundlage: basis,
        }]);
    }

    let bands = staffel(input)?;
    let mut anteile = Vec::with_capacity(bands.len());
    let mut untergrenze = Decimal::ZERO;
    for (bis, ct, basis) in bands {
        let obergrenze = bis
            .unwrap_or(input.kwk_leistung_kw)
            .min(input.kwk_leistung_kw);
        let anteil = obergrenze - untergrenze;
        if anteil > Decimal::ZERO {
            anteile.push(KwkLeistungsanteil {
                von_kw: untergrenze,
                bis_kw: bis,
                anteil_kw: anteil,
                satz_ct: ct,
                rechtsgrundlage: basis,
            });
        }
        untergrenze = obergrenze;
        if untergrenze >= input.kwk_leistung_kw {
            break;
        }
    }
    // A closed ladder that does not reach the plant's capacity prices nothing:
    // § 7 Abs. 2 Nr. 1 has no band above 100 kW because § 6 Abs. 3 Nr. 1 has no
    // claim above 100 kW.
    (untergrenze >= input.kwk_leistung_kw).then_some(anteile)
}

/// The § 7 KWK-Zuschlag in ct/kWh for one plant — the capacity-weighted mean of
/// the Leistungsanteile from [`zuschlag_leistungsanteile`].
///
/// The result is exact: § 7 rounds nothing, and the rounding belongs at the euro
/// amount the Zuschlag is paid as.
///
/// ```rust
/// use eeg_billing::kwkg::{KwkZuschlagInput, zuschlag_ct_kwh};
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// // (50×8 + 50×6 + 150×5 + 1 750×4,4) / 2 000 = 4,575 ct/kWh.
/// let ibn = date!(2024 - 03 - 01);
/// assert_eq!(
///     zuschlag_ct_kwh(&KwkZuschlagInput::neu_eingespeist(dec!(2000), ibn)),
///     Some(dec!(4.575))
/// );
/// // 200 kW: 50×8 + 50×6 + 100×5, over 200 kW = 6 ct/kWh.
/// assert_eq!(
///     zuschlag_ct_kwh(&KwkZuschlagInput::neu_eingespeist(dec!(200), ibn)),
///     Some(dec!(6))
/// );
/// ```
#[must_use]
pub fn zuschlag_ct_kwh(input: &KwkZuschlagInput) -> Option<Decimal> {
    let anteile = zuschlag_leistungsanteile(input)?;
    let summe: Decimal = anteile.iter().map(|a| a.anteil_kw * a.satz_ct).sum();
    Some(summe / input.kwk_leistung_kw)
}

// ── § 8 — Dauer der Zuschlagzahlung ───────────────────────────────────────────

/// Everything § 8 needs to decide a plant's Förderdauer in Vollbenutzungsstunden.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KwkFoerderdauerInput {
    /// Which § 6 Abs. 1 class the plant belongs to.
    pub anlagenart: KwkAnlagenart,
    /// The cost of the Modernisierung or Nachrüstung as a fraction of the cost
    /// of building the same plant new to the current state of the art
    /// (`0.25` = 25 %). Ignored for [`KwkAnlagenart::Neu`].
    pub kostenanteil: Option<Decimal>,
    /// Whole years between the plant first taking up Dauerbetrieb (or resuming
    /// it after an earlier Modernisierung) and this Modernisierung — the
    /// Karenzzeit of § 8 Abs. 2 Nr. 1 lit. b, Nr. 2 lit. b and Nr. 3 lit. b.
    pub jahre_seit_dauerbetrieb: Option<u32>,
    /// § 8 Abs. 2 Nr. 1 lit. c — whether the plant is a
    /// Dampfsammelschienen-KWK-Anlage with more than 50 MW electrical capacity.
    pub ist_dampfsammelschiene_ueber_50_mw: bool,
}

/// § 8 Abs. 1–3 KWKG — the Vollbenutzungsstunden the Zuschlag is paid for.
///
/// | Anlagenart | Kostenanteil | Karenzzeit | Vollbenutzungsstunden |
/// |---|---|---|---|
/// | neu (Abs. 1) | — | — | 30 000 |
/// | modernisiert (Abs. 2 Nr. 1) | ≥ 10 % | 2 Jahre | 6 000, nur Dampfsammelschiene > 50 MW |
/// | modernisiert (Abs. 2 Nr. 2) | ≥ 25 % | 5 Jahre | 15 000 |
/// | modernisiert (Abs. 2 Nr. 3) | ≥ 50 % | 10 Jahre | 30 000 |
/// | nachgerüstet (Abs. 3 Nr. 1) | ≥ 10 % und < 25 % | — | 10 000 |
/// | nachgerüstet (Abs. 3 Nr. 2) | ≥ 25 % und < 50 % | — | 15 000 |
/// | nachgerüstet (Abs. 3 Nr. 3) | ≥ 50 % | — | 30 000 |
///
/// The Abs. 2 tiers are cumulative conditions, so the longest one whose
/// Kostenanteil **and** Karenzzeit are both met is the one that applies.
/// Returns `None` where no Nummer is met — a Modernisierung below 10 % of the
/// Neuerrichtungskosten, or one inside the Karenzzeit, buys no Förderdauer at
/// all.
///
/// ```rust
/// use eeg_billing::kwkg::{KwkAnlagenart, KwkFoerderdauerInput, foerderdauer_vollbenutzungsstunden};
/// use rust_decimal::dec;
///
/// // § 8 Abs. 1: a new plant, whatever its capacity.
/// let neu = KwkFoerderdauerInput {
///     anlagenart: KwkAnlagenart::Neu,
///     kostenanteil: None,
///     jahre_seit_dauerbetrieb: None,
///     ist_dampfsammelschiene_ueber_50_mw: false,
/// };
/// assert_eq!(foerderdauer_vollbenutzungsstunden(&neu), Some(30_000));
///
/// // § 8 Abs. 3 Nr. 2: a 30 % Nachrüstung.
/// let nachgeruestet = KwkFoerderdauerInput {
///     anlagenart: KwkAnlagenart::Nachgeruestet,
///     kostenanteil: Some(dec!(0.30)),
///     ..neu
/// };
/// assert_eq!(foerderdauer_vollbenutzungsstunden(&nachgeruestet), Some(15_000));
/// ```
#[must_use]
pub fn foerderdauer_vollbenutzungsstunden(input: &KwkFoerderdauerInput) -> Option<u32> {
    match input.anlagenart {
        // § 8 Abs. 1 — one figure, no capacity band and no further condition.
        KwkAnlagenart::Neu => Some(30_000),
        KwkAnlagenart::Modernisiert => {
            let anteil = input.kostenanteil?;
            let jahre = input.jahre_seit_dauerbetrieb.unwrap_or(0);
            if anteil >= dec!(0.50) && jahre >= 10 {
                Some(30_000)
            } else if anteil >= dec!(0.25) && jahre >= 5 {
                Some(15_000)
            } else if anteil >= dec!(0.10) && jahre >= 2 && input.ist_dampfsammelschiene_ueber_50_mw
            {
                Some(6_000)
            } else {
                None
            }
        }
        KwkAnlagenart::Nachgeruestet => {
            let anteil = input.kostenanteil?;
            if anteil >= dec!(0.50) {
                Some(30_000)
            } else if anteil >= dec!(0.25) {
                Some(15_000)
            } else if anteil >= dec!(0.10) {
                Some(10_000)
            } else {
                None
            }
        }
    }
}

/// § 8 Abs. 4 KWKG — the Vollbenutzungsstunden a single calendar year may be
/// paid for.
///
/// | ab Kalenderjahr | Vollbenutzungsstunden |
/// |---|---|
/// | 2021 | 5 000 |
/// | 2023 | 4 000 |
/// | 2025 | 3 500 |
/// | 2026 | 3 300 |
/// | 2027 | 3 100 |
/// | 2028 | 2 900 |
/// | 2029 | 2 700 |
/// | 2030 | 2 500 |
///
/// This is the cap that limits what a calendar year can be paid, independently
/// of the Abs. 1–3 lifetime figure. Returns `None` for years before 2021, which
/// Abs. 4 does not reach.
///
/// ```rust
/// use eeg_billing::kwkg::jahreshoechstbetrag_vollbenutzungsstunden;
///
/// assert_eq!(jahreshoechstbetrag_vollbenutzungsstunden(2024), Some(4_000));
/// assert_eq!(jahreshoechstbetrag_vollbenutzungsstunden(2026), Some(3_300));
/// assert_eq!(jahreshoechstbetrag_vollbenutzungsstunden(2035), Some(2_500));
/// ```
#[must_use]
pub fn jahreshoechstbetrag_vollbenutzungsstunden(kalenderjahr: i32) -> Option<u32> {
    Some(match kalenderjahr {
        ..=2020 => return None,
        2021 | 2022 => 5_000,
        2023 | 2024 => 4_000,
        2025 => 3_500,
        2026 => 3_300,
        2027 => 3_100,
        2028 => 2_900,
        2029 => 2_700,
        _ => 2_500,
    })
}

/// § 8 Abs. 4 KWKG — the kWh one calendar year may be paid the Zuschlag for.
///
/// `kwk_leistung_kw × Jahreshöchstbetrag`. Returns `None` for a year Abs. 4 does
/// not reach.
#[must_use]
pub fn jahreskontingent_kwh(kwk_leistung_kw: Decimal, kalenderjahr: i32) -> Option<Decimal> {
    let stunden = jahreshoechstbetrag_vollbenutzungsstunden(kalenderjahr)?;
    Some((kwk_leistung_kw * Decimal::from(stunden)).round_kfm(3))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod statutory_kwkg_tests {
    use super::*;
    use time::macros::date;

    const IBN: Date = date!(2024 - 03 - 01);

    fn neu(kw: Decimal) -> KwkZuschlagInput {
        KwkZuschlagInput::neu_eingespeist(kw, IBN)
    }

    /// **§ 7 Abs. 1 Satz 1 Nr. 1–4 KWKG** — the four bands below 2 MW.
    ///
    /// The invariant is the wording „für den KWK-Leistungsanteil von …": each
    /// band prices only the slice of capacity that falls in it, so a plant's
    /// rate is the capacity-weighted mean of the bands it spans.
    #[test]
    fn abs1_prices_each_leistungsanteil_at_its_own_rate() {
        // The ladder starts above 50 kW for a neue Anlage: at or below that,
        // § 7 Abs. 3a fixes a flat rate for the plant instead of pricing it by
        // Leistungsanteil (`abs3a_displaces_the_ladders_for_new_plants_up_to_50_kw`).
        // A plant that spans the boundary is still priced band by band, so the
        // 0–50 kW slice keeps its Nr. 1 rate.
        //
        // 50 kW at 8 ct + 50 kW at 6 ct = 7 ct.
        assert_eq!(zuschlag_ct_kwh(&neu(dec!(100))), Some(dec!(7)));
        // (400 + 300 + 750) / 250 = 5,8 ct.
        assert_eq!(zuschlag_ct_kwh(&neu(dec!(250))), Some(dec!(5.8)));
        // (400 + 300 + 750 + 1 750×4,4) / 2 000 = 4,575 ct.
        assert_eq!(zuschlag_ct_kwh(&neu(dec!(2_000))), Some(dec!(4.575)));
    }

    /// **§ 7 Abs. 1 Satz 1 Nr. 5 KWKG** — above 2 MW the rate depends on the
    /// Anlagenart: 3,4 ct for neue and modernisierte, 3,1 ct for nachgerüstete.
    #[test]
    fn abs1_nr5_splits_by_anlagenart() {
        let at = |art| {
            zuschlag_leistungsanteile(&KwkZuschlagInput {
                anlagenart: art,
                ..neu(dec!(5_000))
            })
            .expect("§ 7 Abs. 1 has an open top band")
            .last()
            .expect("five bands")
            .satz_ct
        };
        assert_eq!(at(KwkAnlagenart::Neu), dec!(3.4));
        assert_eq!(at(KwkAnlagenart::Modernisiert), dec!(3.4));
        assert_eq!(at(KwkAnlagenart::Nachgeruestet), dec!(3.1));
    }

    /// **§ 7 Abs. 1 Satz 2 KWKG** — the 0,5 ct uplift on Nr. 5 lit. a is
    /// payable only where the Bundesministerium published its
    /// Angemessenheits-Feststellung in the Bundesanzeiger, and § 35 Abs. 18
    /// keeps it away from plants that started before 1 January 2023.
    #[test]
    fn abs1_satz2_uplift_needs_the_bundesanzeiger_publication() {
        let top = |input: &KwkZuschlagInput| {
            zuschlag_leistungsanteile(input)
                .expect("open top band")
                .last()
                .expect("bands")
                .satz_ct
        };
        let ohne = neu(dec!(5_000));
        assert_eq!(top(&ohne), dec!(3.4));

        let mit = KwkZuschlagInput {
            bmwk_feststellung_veroeffentlicht: true,
            ..ohne.clone()
        };
        assert_eq!(top(&mit), dec!(3.9));

        // § 35 Abs. 18: not for a plant in Dauerbetrieb before 1 January 2023.
        let alt = KwkZuschlagInput {
            dauerbetrieb: date!(2022 - 12 - 31),
            ..mit.clone()
        };
        assert_eq!(top(&alt), dec!(3.4));

        // Nr. 5 lit. b and c carry no uplift.
        let modernisiert = KwkZuschlagInput {
            anlagenart: KwkAnlagenart::Modernisiert,
            ..mit
        };
        assert_eq!(top(&modernisiert), dec!(3.4));
    }

    /// **§ 7 Abs. 2 KWKG** — KWK-Strom that is not fed into a Netz der
    /// allgemeinen Versorgung is paid a different, lower ladder per § 6 Abs. 3
    /// Nummer, never the Abs. 1 rates.
    #[test]
    fn abs2_is_its_own_ladder_per_sect6_abs3_nummer() {
        let with = |v| KwkZuschlagInput {
            verwendung: v,
            anlagenart: KwkAnlagenart::Modernisiert,
            ..neu(dec!(100))
        };
        // Nr. 1: 50 × 4 + 50 × 3 = 3,5 ct — well below the Abs. 1 7 ct.
        assert_eq!(
            zuschlag_ct_kwh(&with(KwkVerwendung::NichtEingespeistBis100Kw)),
            Some(dec!(3.5))
        );
        assert_eq!(
            zuschlag_ct_kwh(&with(KwkVerwendung::NichtEingespeistKundenanlage)),
            Some(dec!(3.5))
        );
        // Nr. 3: 50 × 5,41 + 50 × 4 = 4,705 ct.
        assert_eq!(
            zuschlag_ct_kwh(&with(KwkVerwendung::NichtEingespeistStromkostenintensiv)),
            Some(dec!(4.705))
        );
    }

    /// **§ 7 Abs. 2 Nr. 1 KWKG** stops at 100 kW because § 6 Abs. 3 Nr. 1 opens
    /// the claim only up to 100 kW — a larger plant has no rate under it.
    #[test]
    fn abs2_nr1_has_no_rate_above_100_kw() {
        assert_eq!(
            zuschlag_ct_kwh(&KwkZuschlagInput {
                verwendung: KwkVerwendung::NichtEingespeistBis100Kw,
                anlagenart: KwkAnlagenart::Modernisiert,
                ..neu(dec!(150))
            }),
            None
        );
    }

    /// **§ 7 Abs. 3 KWKG** leaves the rate for § 6 Abs. 3 Nr. 4 plants to a
    /// Verordnung, so the statute supplies none.
    #[test]
    fn abs3_has_no_statutory_rate() {
        assert_eq!(
            zuschlag_ct_kwh(&KwkZuschlagInput {
                verwendung: KwkVerwendung::NichtEingespeistBrancheAnlage2,
                ..neu(dec!(80))
            }),
            None
        );
    }

    /// **§ 7 Abs. 3a KWKG** — a new plant up to 50 kW is paid a flat 16 ct fed
    /// in and 8 ct not fed in, displacing the Abs. 1 / Abs. 2 ladders. § 35
    /// Abs. 17 Satz 2 applies it to plants in Dauerbetrieb after 31 December 2019.
    #[test]
    fn abs3a_displaces_the_ladders_for_new_plants_up_to_50_kw() {
        assert_eq!(zuschlag_ct_kwh(&neu(dec!(30))), Some(dec!(16)));
        assert_eq!(
            zuschlag_ct_kwh(&KwkZuschlagInput {
                verwendung: KwkVerwendung::NichtEingespeistKundenanlage,
                ..neu(dec!(30))
            }),
            Some(dec!(8))
        );
        // 51 kW is past it, and the Abs. 1 Staffel prices the plant again.
        assert_eq!(
            zuschlag_ct_kwh(&neu(dec!(100))),
            Some(dec!(7)),
            "above 50 kW the Abs. 1 Staffel applies"
        );
        // A plant that started before the § 35 Abs. 17 Satz 2 cut-off, and a
        // modernisierte Anlage, both stay on the Abs. 1 ladder.
        assert_eq!(
            zuschlag_ct_kwh(&KwkZuschlagInput {
                dauerbetrieb: date!(2019 - 06 - 01),
                ..neu(dec!(30))
            }),
            Some(dec!(8))
        );
        assert_eq!(
            zuschlag_ct_kwh(&KwkZuschlagInput {
                anlagenart: KwkAnlagenart::Modernisiert,
                ..neu(dec!(30))
            }),
            Some(dec!(8))
        );
    }

    /// **§ 8 Abs. 1–3 KWKG** — the Förderdauer is Vollbenutzungsstunden keyed on
    /// the Anlagenart and, for modernisierte and nachgerüstete Anlagen, on the
    /// share of the Neuerrichtungskosten the work cost. No capacity band and no
    /// number of years appears in § 8.
    #[test]
    fn sect8_keys_on_anlagenart_and_kostenanteil() {
        let base = KwkFoerderdauerInput {
            anlagenart: KwkAnlagenart::Neu,
            kostenanteil: None,
            jahre_seit_dauerbetrieb: None,
            ist_dampfsammelschiene_ueber_50_mw: false,
        };
        // Abs. 1 — a 30 kW plant and a 50 MW plant get the same 30 000 h.
        assert_eq!(foerderdauer_vollbenutzungsstunden(&base), Some(30_000));

        // Abs. 2 — Kostenanteil and Karenzzeit are cumulative.
        let modern = |anteil, jahre| KwkFoerderdauerInput {
            anlagenart: KwkAnlagenart::Modernisiert,
            kostenanteil: Some(anteil),
            jahre_seit_dauerbetrieb: Some(jahre),
            ist_dampfsammelschiene_ueber_50_mw: false,
        };
        assert_eq!(
            foerderdauer_vollbenutzungsstunden(&modern(dec!(0.50), 10)),
            Some(30_000)
        );
        assert_eq!(
            foerderdauer_vollbenutzungsstunden(&modern(dec!(0.50), 6)),
            Some(15_000),
            "the 30 000 h tier needs ten years, the 15 000 h tier five"
        );
        assert_eq!(
            foerderdauer_vollbenutzungsstunden(&modern(dec!(0.30), 4)),
            None,
            "inside the Karenzzeit no Nummer is met"
        );
        // Abs. 2 Nr. 1's 6 000 h tier is only for a Dampfsammelschiene > 50 MW.
        assert_eq!(
            foerderdauer_vollbenutzungsstunden(&modern(dec!(0.10), 3)),
            None
        );
        assert_eq!(
            foerderdauer_vollbenutzungsstunden(&KwkFoerderdauerInput {
                ist_dampfsammelschiene_ueber_50_mw: true,
                ..modern(dec!(0.10), 3)
            }),
            Some(6_000)
        );

        // Abs. 3 — Kostenanteil alone, with a 10 000 h bottom tier.
        let nach = |anteil| KwkFoerderdauerInput {
            anlagenart: KwkAnlagenart::Nachgeruestet,
            kostenanteil: Some(anteil),
            ..base.clone()
        };
        assert_eq!(
            foerderdauer_vollbenutzungsstunden(&nach(dec!(0.10))),
            Some(10_000)
        );
        assert_eq!(
            foerderdauer_vollbenutzungsstunden(&nach(dec!(0.25))),
            Some(15_000)
        );
        assert_eq!(
            foerderdauer_vollbenutzungsstunden(&nach(dec!(0.50))),
            Some(30_000)
        );
        assert_eq!(foerderdauer_vollbenutzungsstunden(&nach(dec!(0.09))), None);
    }

    /// **§ 8 Abs. 4 KWKG** — the annual cap, falling from 5 000 to 2 500
    /// Vollbenutzungsstunden. It is what limits a calendar year's payment.
    #[test]
    fn sect8_abs4_caps_each_calendar_year() {
        for (jahr, stunden) in [
            (2021, 5_000),
            (2022, 5_000),
            (2023, 4_000),
            (2024, 4_000),
            (2025, 3_500),
            (2026, 3_300),
            (2027, 3_100),
            (2028, 2_900),
            (2029, 2_700),
            (2030, 2_500),
            (2040, 2_500),
        ] {
            assert_eq!(
                jahreshoechstbetrag_vollbenutzungsstunden(jahr),
                Some(stunden),
                "{jahr}"
            );
        }
        assert_eq!(jahreshoechstbetrag_vollbenutzungsstunden(2020), None);
        // A 500 kW plant may be paid 500 × 3 300 kWh in 2026.
        assert_eq!(
            jahreskontingent_kwh(dec!(500), 2026),
            Some(dec!(1650000.000))
        );
    }

    /// **§ 35 Abs. 20 Satz 1 KWKG dates § 7 Abs. 1 Satz 1 Nr. 5.**
    ///
    /// Nr. 5 „ist anzuwenden auf KWK-Anlagen, die nach dem 31. Dezember 2020 den
    /// Dauerbetrieb aufgenommen oder nach einer erfolgten Modernisierung wieder
    /// aufgenommen haben", so it is the only band above 2 MW and an older plant
    /// runs off the top of the ladder. Satz 2 prices those plants by § 7 Abs. 4
    /// in its 31 December 2020 version, which this module does not carry, so it
    /// answers `None` instead of paying them Nr. 5.
    #[test]
    fn abs1_nr5_reaches_only_dauerbetrieb_after_2020() {
        let am = |tag| KwkZuschlagInput::neu_eingespeist(dec!(5_000), tag);

        // 31 December 2020 is „vor dem 1. Januar 2021" — Satz 2's plant.
        assert_eq!(zuschlag_ct_kwh(&am(date!(2020 - 12 - 31))), None);
        assert_eq!(zuschlag_ct_kwh(&am(date!(2019 - 07 - 01))), None);

        // One day later Nr. 5 applies: (400 + 300 + 750 + 1 750×4,4 + 3 000×3,4)
        // / 5 000 = 3,87 ct.
        assert_eq!(
            zuschlag_ct_kwh(&am(date!(2021 - 01 - 01))),
            Some(dec!(3.87))
        );

        // Below 2 MW the ladder never needs Nr. 5, so the date does not reach it.
        assert_eq!(
            zuschlag_ct_kwh(&KwkZuschlagInput::neu_eingespeist(
                dec!(2_000),
                date!(2019 - 07 - 01)
            )),
            Some(dec!(4.575))
        );
    }
}
