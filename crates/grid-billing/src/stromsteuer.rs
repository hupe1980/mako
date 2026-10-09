//! The § 3 StromStG rate series — the one source of the Stromsteuer rate.
//!
//! The rate lives here, in the lowest crate of the billing stack, because two
//! crates read it: `energy-billing` bills it on a retail invoice, and
//! `eeg-billing` caps a § 53c EEG 2023 reduction at it. `energy-billing`
//! re-exports this module's items.

use rust_decimal::Decimal;
use rust_decimal::dec;

/// One row of a dated statutory rate series: a run of calendar years, the
/// rate, and where it was read.
///
/// A series ends at the last year somebody read the statute for. A year past
/// the end has no row and the lookup answers `None` — never the last row's
/// rate, because a rate nobody has checked for that year reads exactly like a
/// correct one on the invoice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SatzZeile {
    /// First calendar year the row covers.
    pub von_jahr: i32,
    /// Last calendar year the row covers — the last year the statute was read for.
    pub bis_jahr: i32,
    /// The rate, ct/kWh (ct/kWh_Hs for Erdgas).
    pub ct_per_kwh: Decimal,
    /// The provision and Fassung read, with the date it was read.
    pub quelle: &'static str,
}

impl SatzZeile {
    /// The row of `series` covering `year`, if any.
    #[must_use]
    pub fn fuer(series: &'static [SatzZeile], year: i32) -> Option<&'static SatzZeile> {
        series
            .iter()
            .find(|z| (z.von_jahr..=z.bis_jahr).contains(&year))
    }
}

/// The standard § 3 StromStG rate: **2,05 ct/kWh** — „Die Steuer beträgt
/// 20,50 Euro für eine Megawattstunde."
pub const STROMSTEUER_REGELSATZ_CT_PER_KWH: Decimal = dec!(2.05);

/// The § 3 StromStG series.
///
/// Starts in 2003, the first year of the 20,50 EUR/MWh stage (BGBl. I 2002
/// S. 4602); ends with the last year the statute was read for. A year is added
/// once § 3 StromStG has been read with no promulgated change for it — the
/// rate carries no time limit — and re-read before that year begins.
pub const STROMSTEUER_SERIE: &[SatzZeile] = &[SatzZeile {
    von_jahr: 2003,
    bis_jahr: 2027,
    ct_per_kwh: STROMSTEUER_REGELSATZ_CT_PER_KWH,
    quelle: "§ 3 StromStG (20,50 EUR/MWh), gelesen auf gesetze-im-internet.de am 2026-10-05; \
             ohne Befristung und ohne verkündete Änderung für 2027",
}];

/// The standard § 3 StromStG rate (ct/kWh) for a calendar year.
///
/// `None` outside [`STROMSTEUER_SERIE`]: before 2003, when the rate was still
/// climbing through the Ökosteuer stages, and after the last year the statute
/// was read for. Either way the caller refuses the period rather than inherit
/// another year's rate.
///
/// ```rust
/// use grid_billing::stromsteuer::stromsteuer_for_year;
/// assert_eq!(stromsteuer_for_year(2024), Some(rust_decimal::dec!(2.05)));
/// assert_eq!(stromsteuer_for_year(1999), None); // before the standard rate
/// assert_eq!(stromsteuer_for_year(2028), None); // not yet read for 2028
/// ```
#[must_use]
pub fn stromsteuer_for_year(year: i32) -> Option<Decimal> {
    SatzZeile::fuer(STROMSTEUER_SERIE, year).map(|z| z.ct_per_kwh)
}
