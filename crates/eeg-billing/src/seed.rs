//! The statutory Einspeisevergütung series, as rows a service can seed a table
//! with.
//!
//! This is the same statute the rest of the crate reads, projected into one flat
//! list so a service's reference table cannot drift away from it. Each row is a
//! (Erzeugungsart, Vergütungsform, capacity band, commissioning window) and the
//! **net** Vergütungssatz that combination is paid.
//!
//! ## Net, not gross
//!
//! Every rate here is the anzulegender Wert **less the § 53 Abs. 1 deduction**:
//! 0,4 ct/kWh for solar (Nr. 2) and 0,2 ct/kWh for Wasserkraft, Biomasse,
//! Geothermie und die Gase (Nr. 1). The Einspeisevergütung is what these rows
//! describe, so they carry what is actually paid.
//!
//! ## Windows are commissioning windows
//!
//! Every §§ 40–49 Absenkung applies „für die nach diesem Zeitpunkt in Betrieb
//! genommenen Anlagen", so a row's window bounds the **Inbetriebnahmedatum**, not
//! the settled month. A plant keeps the value of its window for its whole
//! Förderdauer.
//!
//! ## What is not here
//!
//! - **Wind**, on- and offshore: § 22 Abs. 2 makes the claim depend on a BNetzA
//!   Zuschlag and § 36h derives the value from it, so there is nothing statutory
//!   to tabulate.
//! - **KWKG**: § 7 prices per Leistungsanteil, so a plant's rate is a blend
//!   across the bands its capacity spans and no single-rate row can state it.
//!   [`crate::kwkg::zuschlag_ct_kwh`] computes it.
//! - Plants commissioned **before 2023**, whose values come from their own EEG
//!   version and are imported per plant.

use crate::rounding::RoundMoney;
use rust_decimal::Decimal;
use rust_decimal::dec;
use time::{Date, Month};

/// The last Inbetriebnahmedatum [`verguetungssatz_rows`] covers.
///
/// Beyond it the windows are still statutory but no longer enumerated here;
/// [`crate::rates::aw_ct_bei_inbetriebnahme`] computes any of them on demand.
pub const HORIZONT: Date = time::macros::date!(2030 - 12 - 31);

/// The first Inbetriebnahmedatum the EEG 2023 values apply to.
const EEG2023_START: Date = time::macros::date!(2023 - 01 - 01);

/// One row of the statutory series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerguetungssatzRow {
    /// Erzeugungsart, in the spelling [`crate::ErzeugungsArt::from_db_str`] reads.
    pub erzeugungsart: &'static str,
    /// Lower bound of the capacity band, exclusive of the band below it.
    pub leistung_min_kwp: Decimal,
    /// Upper bound of the capacity band, inclusive. `None` for an open top band.
    pub leistung_max_kwp: Option<Decimal>,
    /// `UEBERSCHUSS` or `VOLLEINSPEISUNG`.
    pub verguetungsform: &'static str,
    /// The **net** Vergütungssatz in ct/kWh — anzulegender Wert less § 53 Abs. 1.
    pub verguetungssatz_ct: Decimal,
    /// First Inbetriebnahmedatum this row applies to.
    pub billing_start: Date,
    /// Last Inbetriebnahmedatum this row applies to.
    pub billing_end: Date,
    /// EEG version year.
    pub eeg_gesetz: i16,
    /// The Nummer the value rests on.
    pub notes: String,
}

/// One capacity band of a Staffel: upper bound (inclusive, `None` = open) and
/// the Startwert in ct/kWh.
struct Band {
    max: Option<Decimal>,
    startwert_ct: Decimal,
    nummer: &'static str,
}

const fn band(max: Option<Decimal>, startwert_ct: Decimal, nummer: &'static str) -> Band {
    Band {
        max,
        startwert_ct,
        nummer,
    }
}

/// How a Staffel's values step down over time.
#[derive(Clone, Copy)]
enum Absenkung {
    /// § 49 — 1 % every six months from 1 February 2024.
    Halbjaehrlich,
    /// §§ 40 Abs. 5, 41 Abs. 4, 44a, 45 Abs. 2 — once a year.
    Jaehrlich(crate::degression::JaehrlicheAbsenkung),
}

impl Absenkung {
    /// The first day of each window, in order, up to [`HORIZONT`].
    fn fensterstarts(self) -> Vec<Date> {
        let mut starts = vec![EEG2023_START];
        let mut next = match self {
            Self::Halbjaehrlich => crate::degression::DEGRESSIONSBEGINN,
            Self::Jaehrlich(a) => a.beginn,
        };
        while next <= HORIZONT {
            starts.push(next);
            next = match self {
                Self::Halbjaehrlich => plus_monate(next, 6),
                Self::Jaehrlich(_) => plus_monate(next, 12),
            };
        }
        starts
    }

    /// The value in force for a plant commissioned on `ibn`, rounded as the
    /// statute rounds it.
    fn wert(self, startwert_ct: Decimal, ibn: Date) -> Decimal {
        match self {
            Self::Halbjaehrlich => {
                crate::degression::anzulegender_wert_bei_inbetriebnahme(startwert_ct, ibn)
            }
            Self::Jaehrlich(a) => a.anzulegender_wert(startwert_ct, ibn),
        }
    }
}

fn plus_monate(von: Date, monate: u32) -> Date {
    let total = i32::from(von.month() as u8 - 1) + i32::try_from(monate).unwrap_or(0);
    let jahr = von.year() + total / 12;
    let monat =
        Month::try_from(u8::try_from(total % 12 + 1).unwrap_or(1)).unwrap_or(Month::January);
    Date::from_calendar_date(jahr, monat, 1).unwrap_or(von)
}

fn tag_davor(d: Date) -> Date {
    d.previous_day().unwrap_or(d)
}

/// § 53 Abs. 1 EEG 2023 — the deduction that turns the anzulegender Wert into the
/// Einspeisevergütung, by Erzeugungsart.
fn sect53_ct(erzeugungsart: &str) -> Decimal {
    if erzeugungsart.starts_with("SOLAR") || erzeugungsart.starts_with("WIND") {
        // Nr. 2 — Solaranlagen und Windenergieanlagen.
        dec!(0.4)
    } else {
        // Nr. 1 — Wasserkraft, Biomasse, Geothermie, Deponie-, Klär- und Grubengas.
        dec!(0.2)
    }
}

fn staffel(
    erzeugungsart: &'static str,
    verguetungsform: &'static str,
    bands: &[Band],
    absenkung: Absenkung,
    out: &mut Vec<VerguetungssatzRow>,
) {
    let abzug = sect53_ct(erzeugungsart);
    let starts = absenkung.fensterstarts();
    for (i, start) in starts.iter().copied().enumerate() {
        let ende = starts.get(i + 1).map_or(HORIZONT, |n| tag_davor(*n));
        if ende < start {
            continue;
        }
        let mut min = Decimal::ZERO;
        for b in bands {
            out.push(VerguetungssatzRow {
                erzeugungsart,
                leistung_min_kwp: min,
                leistung_max_kwp: b.max,
                verguetungsform,
                verguetungssatz_ct: absenkung.wert(b.startwert_ct, start) - abzug,
                billing_start: start,
                billing_end: ende,
                eeg_gesetz: 2023,
                notes: b.nummer.to_owned(),
            });
            min = b.max.unwrap_or(min);
        }
    }
}

/// The full statutory series for plants commissioned from 1 January 2023 up to
/// [`HORIZONT`].
///
/// Rows are ordered by Erzeugungsart, then Vergütungsform, then window, then
/// capacity band.
///
/// ```rust
/// use eeg_billing::seed::verguetungssatz_rows;
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// let rows = verguetungssatz_rows();
/// // § 48 Abs. 2 Nr. 1 base 8,60 ct, one § 49 step, less the § 53 Abs. 1 Nr. 2
/// // deduction of 0,4 ct.
/// let r = rows
///     .iter()
///     .find(|r| {
///         r.erzeugungsart == "SOLAR_AUFDACH"
///             && r.verguetungsform == "UEBERSCHUSS"
///             && r.leistung_min_kwp == dec!(0)
///             && r.billing_start == date!(2024 - 02 - 01)
///     })
///     .expect("the 1 February 2024 window");
/// assert_eq!(r.verguetungssatz_ct, dec!(8.11));
/// assert_eq!(r.billing_end, date!(2024 - 07 - 31));
/// ```
#[must_use]
pub fn verguetungssatz_rows() -> Vec<VerguetungssatzRow> {
    use crate::degression::JaehrlicheAbsenkung as A;
    let mut out = Vec::new();

    // § 48 Abs. 2 EEG 2023 in the version § 101 Abs. 1 Satz 2 keeps in force —
    // the one as at 15 May 2024 — degressed by § 49.
    staffel(
        "SOLAR_AUFDACH",
        "UEBERSCHUSS",
        &[
            band(Some(dec!(10)), dec!(8.60), "§48 Abs. 2 Nr. 1 EEG 2023"),
            band(Some(dec!(40)), dec!(7.50), "§48 Abs. 2 Nr. 2 EEG 2023"),
            band(Some(dec!(1000)), dec!(6.20), "§48 Abs. 2 Nr. 3 EEG 2023"),
        ],
        Absenkung::Halbjaehrlich,
        &mut out,
    );

    // § 48 Abs. 2 + Abs. 2a. The Abs. 2a uplift has bands of its own, so the
    // Volleinspeisung ladder is split at 10 / 40 / 100 / 400 / 1 000 kW and each
    // step takes the uplift its own size falls in — Nr. 3's 5,1 ct is larger
    // than Nr. 2's 3,8, and the 40–100 kW band gets 5,1. § 49 names Absatz 2a
    // expressly, so base and uplift degress together.
    staffel(
        "SOLAR_AUFDACH",
        "VOLLEINSPEISUNG",
        &[
            band(
                Some(dec!(10)),
                dec!(13.40),
                "§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023",
            ),
            band(
                Some(dec!(40)),
                dec!(11.30),
                "§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023",
            ),
            band(
                Some(dec!(100)),
                dec!(11.30),
                "§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023",
            ),
            band(
                Some(dec!(400)),
                dec!(9.40),
                "§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023",
            ),
            band(
                Some(dec!(1000)),
                dec!(8.10),
                "§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023",
            ),
        ],
        Absenkung::Halbjaehrlich,
        &mut out,
    );

    // § 40 Abs. 1 EEG 2023, degressed by Abs. 5.
    staffel(
        "WASSERKRAFT",
        "UEBERSCHUSS",
        &[
            band(Some(dec!(500)), dec!(12.03), "§40 Abs. 1 Nr. 1 EEG 2023"),
            band(Some(dec!(2000)), dec!(7.93), "§40 Abs. 1 Nr. 2 EEG 2023"),
            band(Some(dec!(5000)), dec!(6.07), "§40 Abs. 1 Nr. 3 EEG 2023"),
            band(Some(dec!(10000)), dec!(5.32), "§40 Abs. 1 Nr. 4 EEG 2023"),
            band(Some(dec!(20000)), dec!(5.13), "§40 Abs. 1 Nr. 5 EEG 2023"),
            band(Some(dec!(50000)), dec!(4.12), "§40 Abs. 1 Nr. 6 EEG 2023"),
            band(None, dec!(3.37), "§40 Abs. 1 Nr. 7 EEG 2023"),
        ],
        Absenkung::Jaehrlich(A::WASSERKRAFT),
        &mut out,
    );

    // § 41 EEG 2023 — one ladder per gas, all degressed by Abs. 4. Abs. 1 and
    // Abs. 2 have no „mehr als" Nummer, so both close at 5 MW.
    staffel(
        "DEPONIEGAS",
        "UEBERSCHUSS",
        &[
            band(Some(dec!(500)), dec!(7.46), "§41 Abs. 1 Nr. 1 EEG 2023"),
            band(Some(dec!(5000)), dec!(5.17), "§41 Abs. 1 Nr. 2 EEG 2023"),
        ],
        Absenkung::Jaehrlich(A::GASE),
        &mut out,
    );
    staffel(
        "KLAERGAS",
        "UEBERSCHUSS",
        &[
            band(Some(dec!(500)), dec!(5.93), "§41 Abs. 2 Nr. 1 EEG 2023"),
            band(Some(dec!(5000)), dec!(5.17), "§41 Abs. 2 Nr. 2 EEG 2023"),
        ],
        Absenkung::Jaehrlich(A::GASE),
        &mut out,
    );
    staffel(
        "GRUBENGAS",
        "UEBERSCHUSS",
        &[
            band(Some(dec!(1000)), dec!(5.98), "§41 Abs. 3 Nr. 1 EEG 2023"),
            band(Some(dec!(5000)), dec!(3.81), "§41 Abs. 3 Nr. 2 EEG 2023"),
            band(None, dec!(3.37), "§41 Abs. 3 Nr. 3 EEG 2023"),
        ],
        Absenkung::Jaehrlich(A::GASE),
        &mut out,
    );

    // § 42 Satz 1 EEG 2023 — one tier; above 150 kW the value comes from a
    // tender (§ 22 Abs. 4). Degressed by § 44a, which steps on 1 July.
    staffel(
        "BIOMASSE",
        "UEBERSCHUSS",
        &[band(Some(dec!(150)), dec!(12.67), "§42 Satz 1 EEG 2023")],
        Absenkung::Jaehrlich(A::BIOMASSE),
        &mut out,
    );

    // § 43 Abs. 1 EEG 2023 — Vergärung von Bioabfällen, a separate and higher
    // claim than § 42, likewise degressed by § 44a.
    staffel(
        "BIOGAS",
        "UEBERSCHUSS",
        &[
            band(Some(dec!(500)), dec!(14.16), "§43 Abs. 1 Nr. 1 EEG 2023"),
            band(Some(dec!(20000)), dec!(12.41), "§43 Abs. 1 Nr. 2 EEG 2023"),
        ],
        Absenkung::Jaehrlich(A::BIOMASSE),
        &mut out,
    );

    // § 45 Abs. 1 EEG 2023 — flat, degressed by Abs. 2.
    staffel(
        "GEOTHERMIE",
        "UEBERSCHUSS",
        &[band(None, dec!(25.20), "§45 Abs. 1 EEG 2023")],
        Absenkung::Jaehrlich(A::GEOTHERMIE),
        &mut out,
    );

    for row in &mut out {
        row.verguetungssatz_ct = row.verguetungssatz_ct.round_kfm(4);
    }
    out
}

#[cfg(test)]
mod statutory_seed_tests {
    use super::*;

    fn finde(art: &str, form: &str, min: Decimal, start: Date) -> VerguetungssatzRow {
        verguetungssatz_rows()
            .into_iter()
            .find(|r| {
                r.erzeugungsart == art
                    && r.verguetungsform == form
                    && r.leistung_min_kwp == min
                    && r.billing_start == start
            })
            .unwrap_or_else(|| panic!("{art}/{form} band from {min} kW, window of {start}"))
    }

    /// **§ 48 Abs. 2 EEG 2023 has three bands**, the last running to 1 MW.
    ///
    /// There is no tier between 100 kWp and 1 MWp, and none below 6,20 ct.
    #[test]
    fn solar_ueberschuss_has_exactly_the_three_sect48_abs2_bands() {
        let start = time::macros::date!(2024 - 02 - 01);
        let bands: Vec<_> = verguetungssatz_rows()
            .into_iter()
            .filter(|r| {
                r.erzeugungsart == "SOLAR_AUFDACH"
                    && r.verguetungsform == "UEBERSCHUSS"
                    && r.billing_start == start
            })
            .map(|r| (r.leistung_min_kwp, r.leistung_max_kwp, r.verguetungssatz_ct))
            .collect();
        assert_eq!(
            bands,
            vec![
                (dec!(0), Some(dec!(10)), dec!(8.11)),
                (dec!(10), Some(dec!(40)), dec!(7.03)),
                (dec!(40), Some(dec!(1000)), dec!(5.74)),
            ]
        );
    }

    /// **§ 49 opens the first window on 1 February 2024** and every six months
    /// after, and each window is closed — a plant commissioned in 2026 is not
    /// paid a 2024 rate.
    #[test]
    fn sect49_windows_run_on_and_close() {
        for (start, ende, ct) in [
            (
                time::macros::date!(2023 - 01 - 01),
                time::macros::date!(2024 - 01 - 31),
                dec!(8.20),
            ),
            (
                time::macros::date!(2024 - 02 - 01),
                time::macros::date!(2024 - 07 - 31),
                dec!(8.11),
            ),
            (
                time::macros::date!(2026 - 02 - 01),
                time::macros::date!(2026 - 07 - 31),
                dec!(7.78),
            ),
        ] {
            let r = finde("SOLAR_AUFDACH", "UEBERSCHUSS", dec!(0), start);
            assert_eq!(r.billing_end, ende, "window of {start}");
            assert_eq!(r.verguetungssatz_ct, ct, "window of {start}");
        }
    }

    /// **§ 48 Abs. 2a Nr. 3 is 5,1 ct**, so the 40–100 kW Volleinspeisung band
    /// takes a larger uplift than the 10–40 kW one, and § 49 degresses the sum.
    #[test]
    fn volleinspeisung_takes_the_uplift_of_its_own_band() {
        let start = time::macros::date!(2024 - 02 - 01);
        // (6,20 + 5,10) × 0,99 = 11,187 → 11,19, less 0,4 = 10,79.
        assert_eq!(
            finde("SOLAR_AUFDACH", "VOLLEINSPEISUNG", dec!(40), start).verguetungssatz_ct,
            dec!(10.79)
        );
        // (6,20 + 1,90) × 0,99 = 8,019 → 8,02, less 0,4 = 7,62.
        assert_eq!(
            finde("SOLAR_AUFDACH", "VOLLEINSPEISUNG", dec!(400), start).verguetungssatz_ct,
            dec!(7.62)
        );
    }

    /// **§ 53 Abs. 1 Nr. 1 takes 0,2 ct from every non-solar Einspeisevergütung**,
    /// and §§ 40 Abs. 5 / 44a step the anzulegender Wert down first.
    #[test]
    fn non_solar_rows_are_net_and_degressed() {
        // § 40 Abs. 1 Nr. 1: 12,03 × 0,995³ = 11,85, less 0,2 = 11,65.
        assert_eq!(
            finde(
                "WASSERKRAFT",
                "UEBERSCHUSS",
                dec!(0),
                time::macros::date!(2026 - 01 - 01)
            )
            .verguetungssatz_ct,
            dec!(11.65)
        );
        // § 44a steps on 1 July, so the 2024 biomass window opens then.
        let bio = finde(
            "BIOMASSE",
            "UEBERSCHUSS",
            dec!(0),
            time::macros::date!(2024 - 07 - 01),
        );
        assert_eq!(bio.billing_end, time::macros::date!(2025 - 06 - 30));
        // 12,67 × 0,995 = 12,60665 → 12,61, less 0,2 = 12,41.
        assert_eq!(bio.verguetungssatz_ct, dec!(12.41));
    }

    /// Wind and KWK are absent: neither has a statutory value a capacity band
    /// can state.
    #[test]
    fn wind_and_kwk_are_not_tabulated() {
        for row in verguetungssatz_rows() {
            assert!(
                !row.erzeugungsart.starts_with("WIND") && row.erzeugungsart != "KWKG",
                "{} must not be seeded",
                row.erzeugungsart
            );
        }
    }

    /// Every window is closed and the windows of one band tile the range without
    /// a gap, so no commissioning date resolves to two rows or to none.
    #[test]
    fn windows_tile_without_gaps() {
        let mut rows = verguetungssatz_rows();
        rows.sort_by_key(|r| {
            (
                r.erzeugungsart,
                r.verguetungsform,
                r.leistung_min_kwp,
                r.billing_start,
            )
        });
        for pair in rows.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.erzeugungsart == b.erzeugungsart
                && a.verguetungsform == b.verguetungsform
                && a.leistung_min_kwp == b.leistung_min_kwp
            {
                assert_eq!(
                    a.billing_end.next_day(),
                    Some(b.billing_start),
                    "gap between {} and {}",
                    a.billing_end,
                    b.billing_start
                );
            }
            assert!(a.billing_end >= a.billing_start);
        }
    }
}
