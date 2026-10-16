//! [`EegGesetz`] — typed EEG law version for a plant.
//!
//! Every plant that receives EEG/KWKG payments is governed by exactly one law version,
//! determined at commissioning and frozen for the plant's entire Förderdauer
//! (§100 EEG 2023 Übergangsbestimmungen, §100 EEG 2017 Übergangsbestimmungen).
//!
//! ## Bestandsschutz (grandfather clause)
//!
//! A plant keeps the rules of the EEG in force when it was commissioned. This
//! type carries that vintage for the rules that are **keyed on the law version**:
//!
//! | Commissioned | Governing law | §52 Pflichtverstoß regime |
//! |---|---|---|
//! | before 2016-01-01 | EEG 2012 (or earlier) | Vergütung → 0 |
//! | 2016-01-01 – 2020-12-31 | EEG 2017 | Vergütung → 0 |
//! | 2021-01-01 – 2022-12-31 | EEG 2021 | Vergütung → 0 |
//! | 2023-01-01 + | EEG 2023 | separate Pflichtzahlung, Vergütung intact |
//!
//! ## §51 lives in [`crate::negativpreis`], not here
//!
//! §51 is **not** a function of the law year. The Solarspitzengesetz rewrote it
//! with effect from **25.02.2025**, mid-year, so two plants that are both
//! "EEG 2023" are governed by different §51 rules depending on the day they were
//! commissioned. [`crate::negativpreis::NegativpreisRegime::fuer_inbetriebnahme`]
//! is the only correct source for the §51 threshold and kW exemption; this type
//! deliberately exposes neither.
//!
//! ### Sources
//! - §100 Abs. 1 Satz 4 EEG 2017, §100 Abs. 2 Nr. 13 EEG 2021, §100 Abs. 1 EEG 2023
//! - §52 EEG 2023 (Pflichtzahlung) vs. §52 EEG ≤2021 (Vergütungskürzung)

// ── EegGesetz ─────────────────────────────────────────────────────────────────

/// The EEG law version governing a plant.
///
/// Determines the version-specific rules that are genuinely keyed on the law
/// year: §52 Pflichtverstöße (Vergütung suspension vs. €10/kW Pflichtzahlung)
/// and the §100 Übergangsbestimmungen. **§51 is not one of them** — use
/// [`crate::negativpreis::NegativpreisRegime::fuer_inbetriebnahme`].
///
/// ## Setting `eeg_gesetz` correctly
///
/// Store the EEG version in force when the plant was commissioned.
/// Use [`EegGesetz::from_inbetriebnahme_year`] as a fallback when not explicitly known.
///
/// ## DB mapping
///
/// | `EegGesetz` | DB `eeg_gesetz` SMALLINT |
/// |---|---|
/// | `Kwkg` | `0` |
/// | `Eeg2000` | `2000` |
/// | `Eeg2004` | `2004` |
/// | `Eeg2009` | `2009` |
/// | `Eeg2012` | `2012` or `2014` (EEG 2014 was an amendment to EEG 2012) |
/// | `Eeg2017` | `2017` |
/// | `Eeg2021` | `2021` |
/// | `Eeg2023` | `2023` |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum EegGesetz {
    /// KWKG — Kraft-Wärme-Kopplungsgesetz. No EEG §51/§52 rules.
    Kwkg,
    /// EEG 2000 (BGBl I 2000 S. 305).
    Eeg2000,
    /// EEG 2004 (BGBl I 2004 S. 1918).
    Eeg2004,
    /// EEG 2009 (BGBl I 2009 S. 2633).
    Eeg2009,
    /// EEG 2012 + 2014 amendment (BGBl I 2012 S. 1754; BGBl I 2014 S. 1066).
    Eeg2012,
    /// EEG 2017 (BGBl I 2017 S. 2532).
    ///
    /// Applies to plants commissioned 2016-01-01 through 2020-12-31
    /// (§100 Abs. 1 Satz 4 EEG 2017, §100 EEG 2021 Abs. 2 Nr. 13, §100 EEG 2023 Abs. 1).
    Eeg2017,
    /// EEG 2021 (BGBl I 2021 S. 3642).
    ///
    /// Applies to plants commissioned 2021-01-01 through 2022-12-31
    /// (§100 EEG 2023 Abs. 1).
    Eeg2021,
    /// EEG 2023 (BGBl I 2023 Nr. 1 vom 10.01.2023, last amended 23.12.2025).
    ///
    /// Applies to plants commissioned from 2023-01-01. §51 splits again inside
    /// this range at the Solarspitzengesetz cut-over (25.02.2025), which is why
    /// §51 is keyed on the commissioning date rather than on this variant.
    Eeg2023,
}

impl EegGesetz {
    // ── §52 Pflichtverstöße ───────────────────────────────────────────────────

    /// Whether MaStR non-registration **suspends Vergütung to EUR 0** for this EEG version.
    ///
    /// - `true` → old §52 regime (EEG ≤2021 via §100 Übergangsregelung): Vergütung = 0
    ///   until the plant is registered. Use `SettleInput.is_sanctioned = true`.
    /// - `false` → new §52 EEG 2023 regime: the operator pays a **separate penalty**
    ///   of €10/kW/month (§52 Abs. 2 EEG 2023); Vergütung is NOT suspended.
    ///   Use `SettleInput.pflichtverstoss` instead.
    pub fn mastr_nichtregistrierung_suspendiert_verguetung(self) -> bool {
        match self {
            Self::Eeg2023 => false, // §52 EEG 2023: €10/kW penalty, Vergütung intact
            // EEG ≤2021 and KWKG: old §52 regime (Vergütung → 0)
            _ => true,
        }
    }

    // ── Inbetriebnahme-based inference ───────────────────────────────────────

    /// Infer the governing EEG version from the **commissioning year**.
    ///
    /// Use this only as a fallback when the operator has not explicitly stored
    /// `eeg_gesetz` in the plant registry. Operators should set `eeg_gesetz`
    /// explicitly to `EegGesetz::from_db_year(anlage.eeg_gesetz)`.
    ///
    /// ## Key boundary: §100 Abs. 1 Satz 4 EEG 2017
    ///
    /// Plants commissioned **before 2016-01-01** → `Eeg2012`.
    /// Plants commissioned from **2016-01-01** → `Eeg2017`.
    ///
    /// `None` for a commissioning year after
    /// [`LETZTE_TABELLIERTE_INBETRIEBNAHME`]: no Fassung is tabled for it.
    #[must_use]
    pub fn from_inbetriebnahme_year(year: i32) -> Option<Self> {
        Some(match year {
            ..=2004 => Self::Eeg2000,
            2005..=2008 => Self::Eeg2004,
            2009..=2011 => Self::Eeg2009,
            2012..=2015 => Self::Eeg2012, // §100 Abs. 1 Satz 4 EEG 2017
            2016..=2020 => Self::Eeg2017,
            2021..=2022 => Self::Eeg2021,
            y if y <= LETZTE_TABELLIERTE_INBETRIEBNAHME.year() => Self::Eeg2023,
            _ => return None,
        })
    }

    // ── DB round-trip ─────────────────────────────────────────────────────────

    /// Parse from the `eeg_gesetz` DB column (SMALLINT).
    ///
    /// Accepts **both canonical values** (0, 2000, 2004, 2009, 2012, 2017, 2021, 2023)
    /// and **non-canonical years** by mapping commissioning year ranges to the governing law:
    ///
    /// | DB value | Maps to | Reason |
    /// |---|---|---|
    /// | `0` | `Kwkg` | KWKG, no EEG rules |
    /// | 1–2003 | `Eeg2000` | EEG 2000 era |
    /// | 2004–2008 | `Eeg2004` | EEG 2004 era |
    /// | 2009–2011 | `Eeg2009` | EEG 2009 era |
    /// | 2012–2015 | `Eeg2012` | EEG 2012 + 2014 amendment |
    /// | 2016–2020 | `Eeg2017` | §100 Abs. 1 Satz 4 EEG 2017 |
    /// | 2021–2022 | `Eeg2021` | §100 EEG 2023: old plants use EEG 2021 |
    /// | 2023–2026 | `Eeg2023` | Current law |
    ///
    /// Returns `Err` for a non-positive year (`<= 0`), and for a year after
    /// [`LETZTE_TABELLIERTE_INBETRIEBNAHME`], for which no Fassung is tabled.
    ///
    /// # Example
    ///
    /// ```rust
    /// use eeg_billing::EegGesetz;
    ///
    /// // Canonical values
    /// assert_eq!(EegGesetz::from_db_year(2017).unwrap(), EegGesetz::Eeg2017);
    /// assert_eq!(EegGesetz::from_db_year(2023).unwrap(), EegGesetz::Eeg2023);
    /// assert_eq!(EegGesetz::from_db_year(0).unwrap(),    EegGesetz::Kwkg);
    ///
    /// // Non-canonical years map to the governing law (defensive correctness)
    /// assert_eq!(EegGesetz::from_db_year(2018).unwrap(), EegGesetz::Eeg2017);
    /// assert_eq!(EegGesetz::from_db_year(2020).unwrap(), EegGesetz::Eeg2017);
    /// assert_eq!(EegGesetz::from_db_year(2022).unwrap(), EegGesetz::Eeg2021);
    /// assert_eq!(EegGesetz::from_db_year(2024).unwrap(), EegGesetz::Eeg2023);
    ///
    /// // Bestandsschutz boundary (§100 Abs. 1 Satz 4 EEG 2017)
    /// assert_eq!(EegGesetz::from_db_year(2016).unwrap(), EegGesetz::Eeg2017);
    /// assert_eq!(EegGesetz::from_db_year(2015).unwrap(), EegGesetz::Eeg2012);
    ///
    /// // 2014 = EEG 2014 amendment to EEG 2012 base law
    /// assert_eq!(EegGesetz::from_db_year(2014).unwrap(), EegGesetz::Eeg2012);
    /// ```
    pub fn from_db_year(y: i16) -> Result<Self, InvalidEegGesetz> {
        match y {
            0 => Ok(Self::Kwkg),
            1..=2003 => Ok(Self::Eeg2000),
            2004..=2008 => Ok(Self::Eeg2004),
            2009..=2011 => Ok(Self::Eeg2009),
            2012..=2015 => Ok(Self::Eeg2012),
            2016..=2020 => Ok(Self::Eeg2017),
            2021..=2022 => Ok(Self::Eeg2021),
            2023..=2026 => Ok(Self::Eeg2023),
            _ => Err(InvalidEegGesetz(y)), // negative values, untabled vintages
        }
    }

    /// The canonical DB column value for this variant.
    pub fn to_db_year(self) -> i16 {
        match self {
            Self::Kwkg => 0,
            Self::Eeg2000 => 2000,
            Self::Eeg2004 => 2004,
            Self::Eeg2009 => 2009,
            Self::Eeg2012 => 2012,
            Self::Eeg2017 => 2017,
            Self::Eeg2021 => 2021,
            Self::Eeg2023 => 2023,
        }
    }
}

impl Default for EegGesetz {
    /// Default to EEG 2023 (current law) — safe for new plants.
    fn default() -> Self {
        Self::Eeg2023
    }
}

/// The last commissioning day a tabled EEG Fassung governs: **31 December 2026**.
///
/// A plant keeps the EEG in force at its commissioning. Its successor, the
/// EEG 2027 (BT-Drs. 21/7867), is not verkündet, so no Fassung is tabled for a
/// plant commissioned later: [`EegGesetz::from_inbetriebnahme_year`] answers
/// `None`, and [`crate::calculate_settlement`] refuses its settlement as
/// [`crate::SettlementStatus::FassungNichtTabelliert`] rather than computing it
/// on EEG 2023 arithmetic. A plant commissioned on or before this day keeps its
/// own Fassung in every later period.
pub const LETZTE_TABELLIERTE_INBETRIEBNAHME: time::Date = time::macros::date!(2026 - 12 - 31);

// ── § 101 Abs. 1 EEG 2023 — § 100 Abs. 47 under the Genehmigungsvorbehalt ────

/// The day the European Commission's state-aid approval made **§ 100 Abs. 47
/// EEG 2023** applicable — `None` until it is given.
///
/// § 101 Abs. 1 Satz 1 EEG 2023 reserves „§ 100 Absatz 35 und 47" until „der
/// beihilferechtlichen Genehmigung durch die Europäische Kommission"
/// (Arbeitsausgabe 23.12.2025, p. 218). Abs. 47 is the opt-in of a
/// Bestandsanlage into §§ 51 and 51a with the 0,6 ct/kWh uplift
/// ([`crate::negativpreis::SECT51_OPTIN_ZUSCHLAG_CT_KWH`]); without the
/// approval a declaration neither earns the uplift nor moves the plant off its
/// own Fassung's negative-price rule.
pub const SECT100_ABS47_GENEHMIGT_AB: Option<time::Date> = None;

/// Whether § 100 Abs. 47 EEG 2023 may be applied to the period starting
/// `periodenbeginn`, given the approval date `genehmigt_ab`.
///
/// Keyed on the supply period like [`sect51b_anwendbar`]; an unknown period or
/// an absent approval is `false`.
#[must_use]
pub fn sect100_abs47_anwendbar_ab(
    genehmigt_ab: Option<time::Date>,
    periodenbeginn: Option<time::Date>,
) -> bool {
    matches!((genehmigt_ab, periodenbeginn), (Some(ab), Some(d)) if d >= ab)
}

/// Whether § 100 Abs. 47 EEG 2023 may be applied to the period starting
/// `periodenbeginn` — [`sect100_abs47_anwendbar_ab`] at
/// [`SECT100_ABS47_GENEHMIGT_AB`].
#[must_use]
pub fn sect100_abs47_anwendbar(periodenbeginn: Option<time::Date>) -> bool {
    sect100_abs47_anwendbar_ab(SECT100_ABS47_GENEHMIGT_AB, periodenbeginn)
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Returned by [`EegGesetz::from_db_year`] when the value is not a known EEG year.
#[derive(Debug, thiserror::Error)]
#[error("unknown eeg_gesetz year: {0}")]
pub struct InvalidEegGesetz(pub i16);

// ── § 101 EEG 2023 — beihilferechtlicher Genehmigungsvorbehalt ───────────────

/// The day the European Commission's state-aid approval made **§ 51b EEG 2023**
/// applicable: **18 September 2025**.
///
/// § 101 Abs. 2 Satz 1 lists § 51b among the provisions that may be applied
/// "erst nach der beihilferechtlichen Genehmigung durch die Europäische
/// Kommission". Unlike the provisions in Satz 2 it names no fallback version, so
/// there is nothing to apply to an earlier period — and § 51b zeroes a tendered
/// biogas plant's anzulegender Wert whenever the spot price is at or below
/// 2 ct/kWh, which was common well before September 2025.
pub const SECT51B_GENEHMIGT_AB: time::Date = time::macros::date!(2025 - 09 - 18);

/// Whether § 51b EEG 2023 may be applied to a settlement of the period starting
/// on `periodenbeginn` ([`SettleInput::billing_date`](crate::SettleInput)).
///
/// Keyed on the **supply period**, not on when the invoice is written: a
/// Gutschrift issued in 2026 for August 2025 settles a supply § 51b did not
/// reach.
///
/// Two answers are deliberately conservative. An unknown period is `false` — a
/// provision under a Genehmigungsvorbehalt is not applied on the strength of not
/// knowing when the supply happened. So is a period *starting* before the
/// approval day: a month straddling it has qualifying quarter-hours on both
/// sides, and the input carries one Marktwert for the period. A settlement that
/// needs the split bills the two parts as separate periods.
#[must_use]
pub fn sect51b_anwendbar(periodenbeginn: Option<time::Date>) -> bool {
    periodenbeginn.is_some_and(|d| d >= SECT51B_GENEHMIGT_AB)
}

// ── Beihilfevorbehalt for every Teil-3 payment from 01.01.2027 ───────────────

/// The last supply day the European Commission's state-aid approval of the
/// EEG 2023 covers: **31 December 2026**.
///
/// BT-Drs. 21/7867, Begründung zu § 104 EEG 2027: „Der Genehmigungsvorbehalt
/// ist weit gestaltet, weil die Beihilfegenehmigung für das EEG 2023 bis zum
/// 31. Dezember 2026 befristet ist. Es sind also auch solche
/// beihilferelevanten Bestimmungen unter Beihilfevorbehalt zu stellen, die
/// durch das EEG 2027 nicht angepasst werden." § 104 Abs. 1 of the Entwurf
/// lets the Bestimmungen des Teils 3 „erst nach der beihilferechtlichen
/// Genehmigung durch die Europäische Kommission und nur nach Maßgabe dieser
/// Genehmigung" be applied; Abs. 2 has the BMWE announce it in the
/// Bundesanzeiger.
pub const EEG2023_BEIHILFEGENEHMIGUNG_BIS: time::Date = time::macros::date!(2026 - 12 - 31);

/// A recorded state-aid approval of the Commission that opens Teil-3 payments
/// for the supply periods it covers.
///
/// The scope is recorded as the decision states it. A decision „nach Maßgabe"
/// of conditions is still one record; whether a given rule lies inside its
/// scope is read off `umfang` by the operator, not inferred here.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Beihilfegenehmigung {
    /// When the decision reached the Bundesregierung — the date the
    /// Begründung zu § 104 Abs. 2 calls „zeitlich relevant".
    pub eingang: time::Date,
    /// First supply day the decision covers.
    pub gilt_ab: time::Date,
    /// Last supply day the decision covers, if it is limited.
    pub gilt_bis: Option<time::Date>,
    /// The Bundesanzeiger notice (§ 104 Abs. 2).
    pub bundesanzeiger: String,
    /// The scheme and scope as the decision states them.
    pub umfang: String,
}

/// Whether a Teil-3 payment for a supply period may be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beihilfevorbehalt<'a> {
    /// The period lies inside the EEG 2023 approval.
    Eeg2023Genehmigt,
    /// A recorded approval covers the period.
    Genehmigt(&'a Beihilfegenehmigung),
    /// No approval covers the period — or the period is unknown.
    Offen,
}

impl Beihilfevorbehalt<'_> {
    /// `true` unless the payment is under the open reservation.
    #[must_use]
    pub const fn anwendbar(&self) -> bool {
        !matches!(self, Self::Offen)
    }
}

/// The Beihilfevorbehalt for the supply period starting `periodenbeginn`.
///
/// Keyed on the supply period, like [`sect51b_anwendbar`]: a Gutschrift issued
/// in 2027 for December 2026 settles a supply the EEG 2023 approval covers. An
/// unknown period is [`Beihilfevorbehalt::Offen`] — a payment under a
/// reservation is not made on the strength of not knowing when the supply
/// happened.
#[must_use]
pub fn beihilfevorbehalt(
    periodenbeginn: Option<time::Date>,
    genehmigungen: &[Beihilfegenehmigung],
) -> Beihilfevorbehalt<'_> {
    let Some(d) = periodenbeginn else {
        return Beihilfevorbehalt::Offen;
    };
    if d <= EEG2023_BEIHILFEGENEHMIGUNG_BIS {
        return Beihilfevorbehalt::Eeg2023Genehmigt;
    }
    genehmigungen
        .iter()
        .find(|g| g.gilt_ab <= d && g.gilt_bis.is_none_or(|bis| d <= bis))
        .map_or(Beihilfevorbehalt::Offen, Beihilfevorbehalt::Genehmigt)
}

// ── The Fassung of a payment period ──────────────────────────────────────────

/// The EEG Fassung that governs a **payment period** — distinct from
/// [`EegGesetz`], the plant's vintage.
///
/// The vintage still decides § 52 and the § 100 Übergangsbestimmungen; the
/// Fassung decides which statute a reduction of the anzulegender Wert is
/// read from, because the same § number carries a different rule in another
/// Fassung. Only Fassungen read from their text are tabled; a period outside
/// them has none, and a settlement that needs one is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Fassung {
    /// EEG 2023, in force from 01.01.2023; tabled to the last day its state-aid
    /// approval covers, [`EEG2023_BEIHILFEGENEHMIGUNG_BIS`].
    Eeg2023,
}

/// First supply day of the EEG 2023.
const EEG2023_AB: time::Date = time::macros::date!(2023 - 01 - 01);

impl Fassung {
    /// The Fassung governing the payment period starting `periodenbeginn`, or
    /// `None` where none is tabled.
    #[must_use]
    pub fn fuer_periode(periodenbeginn: Option<time::Date>) -> Option<Self> {
        let d = periodenbeginn?;
        (EEG2023_AB..=EEG2023_BEIHILFEGENEHMIGUNG_BIS)
            .contains(&d)
            .then_some(Self::Eeg2023)
    }

    /// The suffix a § is cited with in this Fassung — `"EEG 2023"`.
    #[must_use]
    pub const fn zitat(self) -> &'static str {
        match self {
            Self::Eeg2023 => "EEG 2023",
        }
    }
}

#[cfg(test)]
mod beihilfe_tests {
    use super::*;
    use time::macros::date;

    fn genehmigung() -> Beihilfegenehmigung {
        Beihilfegenehmigung {
            eingang: date!(2027 - 02 - 10),
            gilt_ab: date!(2027 - 01 - 01),
            gilt_bis: None,
            bundesanzeiger: "BAnz AT (Fixture)".to_owned(),
            umfang: "EEG 2027 Teil 3".to_owned(),
        }
    }

    /// December 2026 is inside the EEG 2023 approval; January 2027 is not, and
    /// stays under the reservation until an approval is recorded.
    #[test]
    fn january_2027_is_reserved_until_an_approval_is_recorded() {
        assert_eq!(
            beihilfevorbehalt(Some(date!(2026 - 12 - 01)), &[]),
            Beihilfevorbehalt::Eeg2023Genehmigt
        );
        assert_eq!(
            beihilfevorbehalt(Some(date!(2027 - 01 - 01)), &[]),
            Beihilfevorbehalt::Offen
        );
        let g = [genehmigung()];
        assert_eq!(
            beihilfevorbehalt(Some(date!(2027 - 01 - 01)), &g),
            Beihilfevorbehalt::Genehmigt(&g[0])
        );
        assert_eq!(beihilfevorbehalt(None, &g), Beihilfevorbehalt::Offen);
    }

    /// A limited approval does not cover a period after it.
    #[test]
    fn a_limited_approval_ends_where_it_says() {
        let g = [Beihilfegenehmigung {
            gilt_bis: Some(date!(2027 - 06 - 30)),
            ..genehmigung()
        }];
        assert!(beihilfevorbehalt(Some(date!(2027 - 06 - 01)), &g).anwendbar());
        assert!(!beihilfevorbehalt(Some(date!(2027 - 07 - 01)), &g).anwendbar());
    }

    /// The Fassung follows the payment period, not the plant.
    #[test]
    fn the_fassung_follows_the_payment_period() {
        assert_eq!(
            Fassung::fuer_periode(Some(date!(2026 - 12 - 01))),
            Some(Fassung::Eeg2023)
        );
        assert_eq!(Fassung::fuer_periode(Some(date!(2027 - 01 - 01))), None);
        assert_eq!(Fassung::fuer_periode(Some(date!(2022 - 12 - 01))), None);
        assert_eq!(Fassung::fuer_periode(None), None);
    }
}
