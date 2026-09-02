//! §49 EEG 2023 — halbjährliche Absenkung der solaren anzulegenden Werte.
//!
//! §49 EEG 2023, in full:
//!
//! > "Die anzulegenden Werte nach § 48 Absatz 1, 2 und 2a und § 48a verringern
//! > sich **ab dem 1. Februar 2024 und sodann alle sechs Monate** für die nach
//! > diesem Zeitpunkt in Betrieb genommenen Anlagen um **1 Prozent** gegenüber
//! > den in dem jeweils vorangegangenen Zeitraum geltenden anzulegenden Werten
//! > und werden auf zwei Stellen nach dem Komma gerundet. Für die Berechnung der
//! > Höhe der anzulegenden Werte aufgrund einer erneuten Anpassung nach Satz 1
//! > sind die **ungerundeten** Werte zugrunde zu legen."
//!
//! Three things follow, and all three are load-bearing:
//!
//! 1. The step is **semi-annual**, not quarterly: 1 February and 1 August.
//! 2. The rate is a **fixed 1 %**. The "atmender Deckel" — a degression rate
//!    keyed to the previous year's GW of new capacity — belonged to §49 EEG 2021
//!    and is gone. (§23a EEG 2023 is one sentence long and says only that the
//!    Marktprämie is computed per Anlage 1 — it carries no degression table, so
//!    it is not where the tiered model belongs.)
//! 3. Compounding runs on the **unrounded** chain; the 2-dp rounding is a
//!    presentation step applied to each published window, never fed forward.
//!
//! The window series this produces reproduces the Bundesnetzagentur's published
//! "Anzulegende Werte für Solaranlagen" spreadsheet exactly — see the tests.

use crate::rounding::RoundMoney;
use rust_decimal::Decimal;
use rust_decimal::dec;
use time::{Date, Month};

// ── §49 constants ─────────────────────────────────────────────────────────────

/// §49 Satz 1 — the first semi-annual step takes effect on 1 February 2024.
pub const DEGRESSIONSBEGINN: Date = time::macros::date!(2024 - 02 - 01);

/// §49 Satz 1 — each step reduces the anzulegender Wert by 1 %.
pub const DEGRESSIONSSATZ: Decimal = dec!(0.01);

// ── Step counting ─────────────────────────────────────────────────────────────

/// The number of §49 steps that have taken effect for a plant commissioned on
/// `inbetriebnahme`.
///
/// Zero for anything before 1 February 2024; then one per completed half-year
/// window (1 Feb / 1 Aug).
///
/// ```rust
/// use eeg_billing::degression::degressionsstufen;
/// use time::macros::date;
///
/// assert_eq!(degressionsstufen(date!(2024-01-31)), 0);
/// assert_eq!(degressionsstufen(date!(2024-02-01)), 1);
/// assert_eq!(degressionsstufen(date!(2024-07-31)), 1);
/// assert_eq!(degressionsstufen(date!(2024-08-01)), 2);
/// assert_eq!(degressionsstufen(date!(2025-01-31)), 2);
/// assert_eq!(degressionsstufen(date!(2026-02-01)), 5);
/// ```
#[must_use]
pub fn degressionsstufen(inbetriebnahme: Date) -> u32 {
    if inbetriebnahme < DEGRESSIONSBEGINN {
        return 0;
    }
    let months = (inbetriebnahme.year() - DEGRESSIONSBEGINN.year()) * 12
        + (inbetriebnahme.month() as i32 - DEGRESSIONSBEGINN.month() as i32);
    // `months` is non-negative here: anything earlier was filtered above.
    (months as u32) / 6 + 1
}

/// The first day of the §49 window a plant commissioned on `inbetriebnahme`
/// falls into, or `None` before the degression starts.
///
/// ```rust
/// use eeg_billing::degression::degressionsfenster;
/// use time::macros::date;
///
/// assert_eq!(degressionsfenster(date!(2025-05-17)), Some(date!(2025-02-01)));
/// assert_eq!(degressionsfenster(date!(2023-06-01)), None);
/// ```
#[must_use]
pub fn degressionsfenster(inbetriebnahme: Date) -> Option<Date> {
    let stufen = degressionsstufen(inbetriebnahme);
    if stufen == 0 {
        return None;
    }
    let months_after_start = (stufen - 1) * 6;
    let total = u32::from(DEGRESSIONSBEGINN.month() as u8 - 1) + months_after_start;
    let year = DEGRESSIONSBEGINN.year() + (total / 12) as i32;
    let month = Month::try_from((total % 12 + 1) as u8).ok()?;
    Date::from_calendar_date(year, month, 1).ok()
}

// ── The degression itself ─────────────────────────────────────────────────────

/// §49 Satz 1 + 2 — apply `stufen` semi-annual 1 % steps to a base value.
///
/// The compounding uses the **unrounded** chain (Satz 2); only the returned
/// figure is rounded to two decimals (Satz 1). Feeding a rounded value back in
/// would drift away from the published series within a couple of windows.
///
/// ```rust
/// use eeg_billing::degression::abgesenkter_wert;
/// use rust_decimal::dec;
///
/// // §48 Abs. 2 Nr. 1 base 8.60 ct → the published 1 Feb 2024 window value.
/// assert_eq!(abgesenkter_wert(dec!(8.60), 1), dec!(8.51));
/// // …and the 1 Aug 2024 window: 8.60 × 0.99² = 8.42886, not 8.51 × 0.99.
/// assert_eq!(abgesenkter_wert(dec!(8.60), 2), dec!(8.43));
/// ```
#[must_use]
pub fn abgesenkter_wert(basiswert_ct: Decimal, stufen: u32) -> Decimal {
    // Kaufmännisch, not bankers': 7,50 × 0,99 = 7,425 and §49's "gerundet" is
    // the published 7,43. `round_dp` alone would round half-to-even and give
    // 7,42 — a cent per kWh, for twenty years, on every ≤ 40 kW plant.
    abgesenkter_wert_ungerundet(basiswert_ct, stufen)
        .round_kfm(2)
}

/// The unrounded §49 chain — the basis every further step is computed from
/// (§49 Satz 2).
#[must_use]
pub fn abgesenkter_wert_ungerundet(basiswert_ct: Decimal, stufen: u32) -> Decimal {
    let factor = Decimal::ONE - DEGRESSIONSSATZ;
    (0..stufen).fold(basiswert_ct, |acc, _| acc * factor)
}

/// The anzulegender Wert for a plant commissioned on `inbetriebnahme`, given its
/// §48 base value — [`abgesenkter_wert`] with the step count resolved from the date.
///
/// ```rust
/// use eeg_billing::degression::anzulegender_wert_bei_inbetriebnahme;
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// // §48 Abs. 2 Nr. 1: 8.60 ct base, commissioned in the 1 Feb 2026 window.
/// assert_eq!(
///     anzulegender_wert_bei_inbetriebnahme(dec!(8.60), date!(2026-03-15)),
///     dec!(8.18)
/// );
/// ```
#[must_use]
pub fn anzulegender_wert_bei_inbetriebnahme(
    basiswert_ct: Decimal,
    inbetriebnahme: Date,
) -> Decimal {
    abgesenkter_wert(basiswert_ct, degressionsstufen(inbetriebnahme))
}

// ── Die jährlichen Absenkungen der nicht-solaren anzulegenden Werte ──────────

/// One Erzeugungsart's statutory annual Absenkung.
///
/// Solar degresses semi-annually under § 49; everything else degresses **once a
/// year**, at its own rate, from its own start date — and biomass from 1 July
/// rather than 1 January, which is the detail a single shared helper would lose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JaehrlicheAbsenkung {
    /// First day the first step applies to.
    pub beginn: Date,
    /// Reduction per step, as a fraction (`0.005` = 0,5 %).
    pub satz: Decimal,
    /// The § it comes from, for the audit trail.
    pub rechtsgrundlage: &'static str,
}

impl JaehrlicheAbsenkung {
    /// § 40 Abs. 3 EEG 2023 — Wasserkraft, 0,5 % a year from 1 January 2024.
    pub const WASSERKRAFT: Self = Self {
        beginn: time::macros::date!(2024 - 01 - 01),
        satz: dec!(0.005),
        rechtsgrundlage: "§ 40 Abs. 3 EEG 2023",
    };
    /// § 41 Abs. 4 EEG 2023 — Deponie-, Klär- und Grubengas, 1,5 % a year from
    /// 1 January 2024.
    pub const GASE: Self = Self {
        beginn: time::macros::date!(2024 - 01 - 01),
        satz: dec!(0.015),
        rechtsgrundlage: "§ 41 Abs. 4 EEG 2023",
    };
    /// § 44a EEG 2023 — Biomasse (§§ 42–44), 0,5 % a year from **1 July** 2024.
    pub const BIOMASSE: Self = Self {
        beginn: time::macros::date!(2024 - 07 - 01),
        satz: dec!(0.005),
        rechtsgrundlage: "§ 44a EEG 2023",
    };
    /// § 45 Abs. 2 EEG 2023 — Geothermie, 0,5 % a year from 1 January 2024.
    pub const GEOTHERMIE: Self = Self {
        beginn: time::macros::date!(2024 - 01 - 01),
        satz: dec!(0.005),
        rechtsgrundlage: "§ 45 Abs. 2 EEG 2023",
    };

    /// How many steps a plant commissioned on `inbetriebnahme` has accumulated.
    ///
    /// Zero before the first step date; then one per completed year.
    #[must_use]
    pub fn stufen(self, inbetriebnahme: Date) -> u32 {
        if inbetriebnahme < self.beginn {
            return 0;
        }
        let mut jahre = u32::try_from(inbetriebnahme.year() - self.beginn.year()).unwrap_or(0);
        // Not yet past this year's anniversary → one fewer step.
        let anniversary_passed = (inbetriebnahme.month() as u8, inbetriebnahme.day())
            >= (self.beginn.month() as u8, self.beginn.day());
        if !anniversary_passed {
            jahre = jahre.saturating_sub(1);
        }
        jahre + 1
    }

    /// The anzulegender Wert for a plant commissioned on `inbetriebnahme`.
    ///
    /// Compounds on the **unrounded** chain and rounds the answer to two
    /// decimals — the same discipline § 49 Satz 2 states for solar, and the same
    /// wording each of these Absenkungen uses ("werden auf zwei Stellen nach dem
    /// Komma gerundet").
    #[must_use]
    pub fn anzulegender_wert(self, basiswert_ct: Decimal, inbetriebnahme: Date) -> Decimal {
        let faktor = Decimal::ONE - self.satz;
        (0..self.stufen(inbetriebnahme))
            .fold(basiswert_ct, |acc, _| acc * faktor)
            .round_kfm(2)
    }
}

#[cfg(test)]
mod jaehrliche_absenkung_tests {
    use super::JaehrlicheAbsenkung;
    use rust_decimal::dec;
    use time::macros::date;

    /// A plant commissioned before the first step keeps the base value.
    #[test]
    fn before_the_first_step_nothing_is_deducted() {
        let a = JaehrlicheAbsenkung::WASSERKRAFT;
        assert_eq!(a.stufen(date!(2023 - 06 - 01)), 0);
        assert_eq!(
            a.anzulegender_wert(dec!(12.03), date!(2023 - 06 - 01)),
            dec!(12.03)
        );
    }

    /// § 40 Abs. 3 — 0,5 % a year, compounding on the unrounded chain.
    #[test]
    fn hydro_degresses_half_a_percent_a_year() {
        let a = JaehrlicheAbsenkung::WASSERKRAFT;
        assert_eq!(a.stufen(date!(2024 - 03 - 01)), 1);
        // 12,03 × 0,995 = 11,96985 → 11,97
        assert_eq!(
            a.anzulegender_wert(dec!(12.03), date!(2024 - 03 - 01)),
            dec!(11.97)
        );
        assert_eq!(a.stufen(date!(2026 - 03 - 01)), 3);
        // 12,03 × 0,995³ = 11,85094… → 11,85
        assert_eq!(
            a.anzulegender_wert(dec!(12.03), date!(2026 - 03 - 01)),
            dec!(11.85)
        );
    }

    /// § 44a starts on **1 July**, not 1 January — a plant commissioned in
    /// spring 2024 has not reached the first step yet.
    #[test]
    fn biomass_steps_on_the_first_of_july() {
        let a = JaehrlicheAbsenkung::BIOMASSE;
        assert_eq!(a.stufen(date!(2024 - 05 - 01)), 0);
        assert_eq!(a.stufen(date!(2024 - 07 - 01)), 1);
        assert_eq!(a.stufen(date!(2025 - 06 - 30)), 1);
        assert_eq!(a.stufen(date!(2025 - 07 - 01)), 2);
    }

    /// § 41 Abs. 4 cuts three times as fast as the others.
    #[test]
    fn the_gases_degress_one_and_a_half_percent() {
        let a = JaehrlicheAbsenkung::GASE;
        // 5,93 × 0,985 = 5,84105 → 5,84
        assert_eq!(
            a.anzulegender_wert(dec!(5.93), date!(2024 - 06 - 01)),
            dec!(5.84)
        );
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    /// §49 Satz 1 — the windows open on 1 February and 1 August.
    #[test]
    fn sect49_windows_are_semiannual_from_2024_02_01() {
        assert_eq!(degressionsfenster(date!(2023 - 12 - 31)), None);
        assert_eq!(degressionsfenster(date!(2024 - 01 - 31)), None);
        for (ibn, fenster) in [
            (date!(2024 - 02 - 01), date!(2024 - 02 - 01)),
            (date!(2024 - 07 - 31), date!(2024 - 02 - 01)),
            (date!(2024 - 08 - 01), date!(2024 - 08 - 01)),
            (date!(2025 - 01 - 31), date!(2024 - 08 - 01)),
            (date!(2025 - 02 - 01), date!(2025 - 02 - 01)),
            (date!(2025 - 08 - 15), date!(2025 - 08 - 01)),
            (date!(2026 - 02 - 01), date!(2026 - 02 - 01)),
            (date!(2026 - 08 - 09), date!(2026 - 08 - 01)),
        ] {
            assert_eq!(degressionsfenster(ibn), Some(fenster), "{ibn}");
        }
    }

    /// §49 Satz 2 — compounding on the unrounded chain, not the published figure.
    ///
    /// 8.51 × 0.99 = 8.4249 → 8.42, but the published 1 Aug 2024 value is 8.43,
    /// because the chain runs 8.60 × 0.99² = 8.42886.
    #[test]
    fn sect49_satz2_compounds_unrounded() {
        assert_eq!(abgesenkter_wert(dec!(8.60), 2), dec!(8.43));
        assert_ne!(
            abgesenkter_wert(dec!(8.51), 1),
            abgesenkter_wert(dec!(8.60), 2)
        );
    }

    /// The published BNetzA "Anzulegende Werte für Solaranlagen" series for the
    /// §48 Abs. 2 Nr. 1 (≤ 10 kW) Teileinspeisung base of 8.60 ct.
    #[test]
    fn sect49_reproduces_the_published_bnetza_series() {
        for (ibn, aw) in [
            (date!(2023 - 06 - 01), dec!(8.60)),
            (date!(2024 - 02 - 01), dec!(8.51)),
            (date!(2024 - 08 - 01), dec!(8.43)),
            (date!(2025 - 02 - 01), dec!(8.34)),
            (date!(2025 - 08 - 01), dec!(8.26)),
            (date!(2026 - 02 - 01), dec!(8.18)),
            (date!(2026 - 08 - 01), dec!(8.10)),
        ] {
            assert_eq!(
                anzulegender_wert_bei_inbetriebnahme(dec!(8.60), ibn),
                aw,
                "window of {ibn}"
            );
        }
    }

    /// The same series for the §48 Abs. 2 Nr. 2 (≤ 40 kW) and Nr. 3 (≤ 1 MW) bases.
    #[test]
    fn sect49_published_series_for_the_larger_brackets() {
        for (base, expected) in [
            (
                dec!(7.50),
                [dec!(7.43), dec!(7.35), dec!(7.28), dec!(7.20), dec!(7.13)],
            ),
            (
                dec!(6.20),
                [dec!(6.14), dec!(6.08), dec!(6.02), dec!(5.96), dec!(5.90)],
            ),
        ] {
            for (stufe, aw) in expected.iter().enumerate() {
                assert_eq!(
                    abgesenkter_wert(base, stufe as u32 + 1),
                    *aw,
                    "base {base}, step {}",
                    stufe + 1
                );
            }
        }
    }

    #[test]
    fn zero_steps_returns_the_base() {
        assert_eq!(abgesenkter_wert(dec!(8.60), 0), dec!(8.60));
        assert_eq!(degressionsstufen(date!(2023 - 01 - 01)), 0);
    }
}
