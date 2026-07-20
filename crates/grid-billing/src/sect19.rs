//! Individuelle Netzentgelte — §19 Abs. 2 StromNEV.
//!
//! Two forms, both agreed between Netzbetreiber and Letztverbraucher and both
//! subject to BNetzA oversight (BK4-22-089, as amended):
//!
//! - **Atypische Netznutzung** (Satz 1): the customer's annual peak predictably
//!   falls in the network's low-load windows. The individual charge must not be
//!   less than **20 %** of the published charge.
//! - **Intensive Netznutzung / Bandlast** (Satz 2): qualification requires at
//!   least **7 000 Benutzungsstunden and 10 GWh** a year; the floor then falls
//!   with utilisation — 20 % from 7 000 h, **15 %** from 7 500 h, **10 %** from
//!   8 000 h.
//!
//! The floors are statutory — they are in the ordinance text itself, not only
//! in the Beschlusskammer's methodology. What BK4-22-089 adds is *how* the
//! reduced charge is derived (the physikalischer Pfad); this crate does not
//! derive it — the agreed percentage arrives as an input, and the engine's job
//! is to apply it to the right positions and to refuse to let it silently fall
//! below the floor.
//!
//! ## What the reduction applies to
//!
//! The individual charge replaces the **Netzentgelt** — Arbeits- and
//! Leistungspreis. It does not touch the Konzessionsabgabe or the network
//! levies: the revenue the Netzbetreiber loses is compensated through the
//! §19 StromNEV-Umlage, which this crate bills separately.

use rust_decimal::Decimal;
use rust_decimal::dec;

/// The two §19 Abs. 2 forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Sect19Art {
    /// Satz 1 — annual peak predictably in the network's low-load windows.
    AtypischeNetznutzung,
    /// Satz 2 — Bandlast: ≥ 7 000 Benutzungsstunden and ≥ 10 GWh a year.
    IntensiveNetznutzung,
}

/// Qualification threshold for Satz 2, in kWh.
pub const BANDLAST_MINDESTARBEIT_KWH: Decimal = dec!(10_000_000);

/// The Satz 2 floor for a given utilisation, as a fraction of the published
/// charge.
///
/// Returns `None` below the qualification threshold — 7 000 h *and* 10 GWh.
/// `None` means "no Satz 2 agreement is available at all", not "no floor".
#[must_use]
pub fn bandlast_mindestentgelt(
    benutzungsstunden: Decimal,
    jahresarbeit_kwh: Decimal,
) -> Option<Decimal> {
    if jahresarbeit_kwh < BANDLAST_MINDESTARBEIT_KWH || benutzungsstunden < dec!(7000) {
        return None;
    }
    Some(if benutzungsstunden >= dec!(8000) {
        dec!(0.10)
    } else if benutzungsstunden >= dec!(7500) {
        dec!(0.15)
    } else {
        dec!(0.20)
    })
}

/// The Satz 1 floor — 20 % of the published charge, unconditionally.
///
/// Whether the peak really falls in the low-load windows is what the BNetzA
/// approval establishes; by the time this crate is asked to settle, that
/// question is decided.
pub const ATYPISCH_MINDESTENTGELT: Decimal = dec!(0.20);

/// An agreed §19 Abs. 2 individual charge, as a fraction of the published one.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Sect19Vereinbarung {
    /// Which form the agreement takes.
    pub art: Sect19Art,
    /// The agreed fraction of the published Netzentgelt — `0.20` pays 20 %.
    pub vereinbarter_prozentsatz: Decimal,
    /// The BNetzA approval or notification reference, for the trace.
    pub genehmigung: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The statutory staircase, at each boundary.
    #[test]
    fn the_statutory_staircase() {
        let gwh10 = dec!(10_000_000);
        assert_eq!(bandlast_mindestentgelt(dec!(7000), gwh10), Some(dec!(0.20)));
        assert_eq!(bandlast_mindestentgelt(dec!(7499), gwh10), Some(dec!(0.20)));
        assert_eq!(bandlast_mindestentgelt(dec!(7500), gwh10), Some(dec!(0.15)));
        assert_eq!(bandlast_mindestentgelt(dec!(7999), gwh10), Some(dec!(0.15)));
        assert_eq!(bandlast_mindestentgelt(dec!(8000), gwh10), Some(dec!(0.10)));
        assert_eq!(bandlast_mindestentgelt(dec!(8760), gwh10), Some(dec!(0.10)));
    }

    /// Both qualification conditions are required, not either.
    #[test]
    fn qualification_needs_hours_and_energy() {
        assert_eq!(bandlast_mindestentgelt(dec!(6999), dec!(10_000_000)), None);
        assert_eq!(
            bandlast_mindestentgelt(dec!(8000), dec!(9_999_999)),
            None,
            "9.999999 GWh is below the threshold however high the utilisation"
        );
    }
}
