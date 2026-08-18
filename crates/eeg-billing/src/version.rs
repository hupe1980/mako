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
    pub fn from_inbetriebnahme_year(year: i32) -> Self {
        match year {
            ..=2004 => Self::Eeg2000,
            2005..=2008 => Self::Eeg2004,
            2009..=2011 => Self::Eeg2009,
            2012..=2015 => Self::Eeg2012, // §100 Abs. 1 Satz 4 EEG 2017
            2016..=2020 => Self::Eeg2017,
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
    /// | 2012–2015 | `Eeg2012` | EEG 2012 + 2014 amendment |
    /// | 2016–2020 | `Eeg2017` | §100 Abs. 1 Satz 4 EEG 2017 |
    /// | 2021–2022 | `Eeg2021` | §100 EEG 2023: old plants use EEG 2021 |
    /// | 2023 + | `Eeg2023` | Current law |
    ///
    /// Returns `Err` for a non-positive year (`<= 0`).
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
