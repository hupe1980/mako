//! [`EegGesetz`] — typed EEG law version for a plant.
//!
//! Every plant that receives EEG/KWKG payments is governed by exactly one law version,
//! determined at commissioning and frozen for the plant's entire Förderdauer
//! (§100 EEG 2023 Übergangsbestimmungen, §100 EEG 2017 Übergangsbestimmungen).
//!
//! ## Bestandsschutz (grandfather clause)
//!
//! Old plants keep the rules of the EEG law in force when they were commissioned:
//!
//! | Commissioned | Governing law | §51 threshold | §51 kW exemption |
//! |---|---|---|---|
//! | before 2016-01-01 | EEG 2012 (or earlier) | **none** (§100 Abs. 1 Satz 4 EEG 2017) | — |
//! | 2016-01-01 – 2020-12-31 | EEG 2017 | ≥ **6h** | Wind <3 MW; other <500 kW |
//! | 2021-01-01 – 2022-12-31 | EEG 2021 | ≥ **4h** | <500 kW (all types) |
//! | 2023-01-01 + | EEG 2023 | **any** period | <100 kW (until iMSys, §51 Abs. 2) |
//!
//! ### Sources
//! - §100 Abs. 1 Satz 4 EEG 2017 (Bestandsschutz for pre-2016 plants: §51 never applies)
//! - §100 Abs. 2 Nr. 13 EEG 2021 (EEG 2017 plants keep 6h threshold under EEG 2021)
//! - §100 Abs. 1 EEG 2023 (old plants → EEG as of 31.12.2022 = EEG 2021 rules)
//! - §51 Abs. 3 Nr. 1 EEG 2017 (wind <3 MW), Nr. 2 (sonstige <500 kW)
//! - §51 Abs. 2 EEG 2021 (<500 kW, no wind exception)
//! - §51 Abs. 2 EEG 2023 (<100 kW until iMSys installed)

use crate::technology::ErzeugungsArt;

// ── EegGesetz ─────────────────────────────────────────────────────────────────

/// The EEG law version governing a plant.
///
/// Determines which version-specific rules apply:
/// - §51 Negativpreisregel (threshold hours + kW exemption)
/// - §52 Pflichtverstöße (Vergütung suspension vs. €10/kW penalty)
/// - §25 Förderdauer (20 calendar years, all versions)
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
    /// EEG 2000 (BGBl I 2000 S. 305). No §51 Negativpreisregel.
    Eeg2000,
    /// EEG 2004 (BGBl I 2004 S. 1918). No §51.
    Eeg2004,
    /// EEG 2009 (BGBl I 2009 S. 2633). No §51.
    Eeg2009,
    /// EEG 2012 + 2014 amendment (BGBl I 2012 S. 1754; BGBl I 2014 S. 1066).
    /// No §51 Negativpreisregel (§100 Abs. 1 Satz 4 EEG 2017: §51 only for plants from 2016-01-01).
    Eeg2012,
    /// EEG 2017 (BGBl I 2017 S. 2532).
    ///
    /// Applies to plants commissioned 2016-01-01 through 2020-12-31
    /// (§100 Abs. 1 Satz 4 EEG 2017, §100 EEG 2021 Abs. 2 Nr. 13, §100 EEG 2023 Abs. 1).
    ///
    /// §51: ≥6 consecutive hours; Wind <3 MW exempt, other <500 kW exempt.
    Eeg2017,
    /// EEG 2021 (BGBl I 2021 S. 3642).
    ///
    /// Applies to plants commissioned 2021-01-01 through 2022-12-31
    /// (§100 EEG 2023 Abs. 1).
    ///
    /// §51: ≥4 consecutive hours; all plants <500 kW exempt (wind exception removed).
    Eeg2021,
    /// EEG 2023 (BGBl I 2023 Nr. 1 vom 10.01.2023, last amended 23.12.2025).
    ///
    /// Applies to plants commissioned from 2023-01-01.
    ///
    /// §51: any negative-price period; plants <100 kW exempt until iMSys installed
    /// (§51 Abs. 2 Nr. 1 EEG 2023: transitional until iMSys Rollout complete).
    Eeg2023,
}

impl EegGesetz {
    // ── §51 Negativpreisregel ─────────────────────────────────────────────────

    /// Minimum number of **consecutive** negative-price hours that trigger §51
    /// (Verringerung des anzulegenden Werts auf null) for this EEG version.
    ///
    /// Returns `None` when §51 does not apply at all (pre-EEG 2017 or KWKG).
    ///
    /// The **caller** is responsible for pre-checking whether the threshold is met
    /// before passing `kwh_during_negative_epex` to `calculate_settlement`. The
    /// formula engine trusts the caller's pre-filtering and only checks the kW exemption.
    ///
    /// | `EegGesetz` | Threshold | Legal basis |
    /// |---|---|---|
    /// | KWKG / EEG ≤2012 | `None` | §100 Abs. 1 Satz 4 EEG 2017 |
    /// | EEG 2017 | `Some(6)` | §51 Abs. 1 EEG 2017 |
    /// | EEG 2021 | `Some(4)` | §51 Abs. 1 EEG 2021 |
    /// | EEG 2023 | `Some(1)` | §51 Abs. 1 EEG 2023 |
    pub fn negativpreis_stunden_schwelle(self) -> Option<u32> {
        match self {
            Self::Kwkg | Self::Eeg2000 | Self::Eeg2004 | Self::Eeg2009 | Self::Eeg2012 => None,
            Self::Eeg2017 => Some(6),
            Self::Eeg2021 => Some(4),
            Self::Eeg2023 => Some(1),
        }
    }

    /// Whether the given number of consecutive negative-price hours meets the §51 threshold.
    ///
    /// Convenience wrapper over [`EegGesetz::negativpreis_stunden_schwelle`].
    /// Always returns `false` for KWKG and EEG ≤2012.
    pub fn negativpreis_stunden_erreicht(self, consecutive_hours: u32) -> bool {
        self.negativpreis_stunden_schwelle()
            .is_some_and(|t| consecutive_hours >= t)
    }

    /// Minimum installed capacity in **kW** above which §51 applies for this
    /// EEG version and technology type.
    ///
    /// Plants **below** the returned threshold are **exempt** from §51.
    /// Returns `None` when §51 does not apply at all (pre-EEG 2017 or KWKG).
    ///
    /// ## EEG 2017 technology-specific thresholds
    ///
    /// §51 Abs. 3 EEG 2017 distinguishes:
    /// - Wind turbines: exempt if < **3 000 kW** (§51 Abs. 3 Nr. 1)
    /// - All other types (solar, biomasse, …): exempt if < **500 kW** (§51 Abs. 3 Nr. 2)
    ///
    /// This distinction was removed in EEG 2021 (uniform 500 kW for all types).
    ///
    /// ## EEG 2023 iMSys nuance
    ///
    /// The 100 kW threshold is **transitional**: plants below 100 kW lose the
    /// exemption in the year their intelligent metering system (iMSys) is installed.
    /// Model post-iMSys plants with a separate field (`has_imesys`) if needed.
    pub fn negativpreis_kw_grenze(self, art: &ErzeugungsArt) -> Option<u32> {
        match self {
            Self::Kwkg | Self::Eeg2000 | Self::Eeg2004 | Self::Eeg2009 | Self::Eeg2012 => None,
            Self::Eeg2017 => {
                // §51 Abs. 3 Nr. 1: Wind <3 MW exempt; Nr. 2: sonstige <500 kW exempt
                if art.is_wind() {
                    Some(3_000)
                } else {
                    Some(500)
                }
            }
            // EEG 2021: wind exception removed — uniform 500 kW
            Self::Eeg2021 => Some(500),
            // EEG 2023: 100 kW (transitional)
            Self::Eeg2023 => Some(100),
        }
    }

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
    /// Plants commissioned **before 2016-01-01** → `Eeg2012` (§51 NOT applicable).
    /// Plants commissioned from **2016-01-01** → `Eeg2017` (§51 ≥6h threshold applies).
    pub fn from_inbetriebnahme_year(year: i32) -> Self {
        match year {
            ..=2004 => Self::Eeg2000,
            2005..=2008 => Self::Eeg2004,
            2009..=2011 => Self::Eeg2009,
            2012..=2015 => Self::Eeg2012, // before 2016: §100 Abs. 1 Satz 4 EEG 2017 → §51 never applies
            2016..=2020 => Self::Eeg2017, // from 2016-01-01: §51 EEG 2017
            2021..=2022 => Self::Eeg2021,
            _ => Self::Eeg2023,
        }
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
    /// | 2012–2015 | `Eeg2012` | EEG 2012+2014 amendment; before 2016: §51 not applicable |
    /// | 2016–2020 | `Eeg2017` | §100 Abs. 1 Satz 4 EEG 2017: §51 applies from 01.01.2016 |
    /// | 2021–2022 | `Eeg2021` | §100 EEG 2023: old plants use EEG 2021 |
    /// | 2023 + | `Eeg2023` | Current law |
    ///
    /// Returns `Err` only for negative values or 0 if you expected a real EEG year.
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
    /// // Critical Bestandsschutz boundary: 2016 → EEG 2017 (§51 applies!)
    /// assert_eq!(EegGesetz::from_db_year(2016).unwrap(), EegGesetz::Eeg2017);
    /// // 2015 → EEG 2012 (§51 NOT applicable — §100 Abs. 1 Satz 4 EEG 2017)
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
            2012..=2015 => Ok(Self::Eeg2012), // 2013–2015: §100 Abs. 1 Satz 4 EEG 2017 → §51 not applicable
            2016..=2020 => Ok(Self::Eeg2017), // from 2016-01-01: §51 EEG 2017 (6h/3MW/500kW)
            2021..=2022 => Ok(Self::Eeg2021),
            2023.. => Ok(Self::Eeg2023),
            _ => Err(InvalidEegGesetz(y)), // negative values
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

// ── Error type ────────────────────────────────────────────────────────────────

/// Returned by [`EegGesetz::from_db_year`] when the value is not a known EEG year.
#[derive(Debug, thiserror::Error)]
#[error("unknown eeg_gesetz year: {0}")]
pub struct InvalidEegGesetz(pub i16);
