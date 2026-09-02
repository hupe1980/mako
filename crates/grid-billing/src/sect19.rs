//! Individuelle Netzentgelte — §19 Abs. 2 StromNEV.
//!
//! Two forms, both agreed between Netzbetreiber and Letztverbraucher and both
//! subject to BNetzA oversight (BK4-22-089, as amended):
//!
//! - **Atypische Netznutzung** (Satz 1): the customer's annual peak predictably
//!   falls in the network's low-load windows. The individual charge must not be
//!   less than **20 %** of the published charge.
//! - **Intensive Netznutzung / Bandlast** (Satz 2): qualification requires a
//!   Benutzungsstundenzahl that *„mindestens 7 000 Stunden im Jahr erreicht"*
//!   and a Stromverbrauch that *„zehn Gigawattstunden **übersteigt**"* — the
//!   hours are inclusive, the energy is not. The floor then falls with
//!   utilisation — 20 % from 7 000 h, **15 %** from 7 500 h, **10 %** from
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sect19Art {
    /// Satz 1 — annual peak predictably in the network's low-load windows.
    AtypischeNetznutzung,
    /// Satz 2 — Bandlast: ≥ 7 000 Benutzungsstunden and > 10 GWh a year.
    IntensiveNetznutzung,
}

/// The Satz 2 Stromverbrauch threshold, in kWh.
///
/// Satz 2 asks whether the consumption *übersteigt* ten Gigawattstunden, so this
/// is the value that has to be **exceeded**, not reached: an Abnahmestelle at
/// exactly 10 GWh does not qualify.
pub const BANDLAST_ARBEITSSCHWELLE_KWH: Decimal = dec!(10_000_000);

/// The Satz 2 Benutzungsstundenzahl threshold, in hours.
///
/// Satz 2 asks whether the Benutzungsstundenzahl *erreicht* mindestens 7 000
/// Stunden, so this value qualifies when it is reached.
pub const BANDLAST_MINDESTBENUTZUNGSSTUNDEN: Decimal = dec!(7000);

/// The Satz 2 floor for a given utilisation, as a fraction of the published
/// charge.
///
/// Returns `None` below the qualification threshold — at least 7 000 h *and*
/// more than 10 GWh. `None` means "no Satz 2 agreement is available at all",
/// not "no floor".
#[must_use]
pub fn bandlast_mindestentgelt(
    benutzungsstunden: Decimal,
    jahresarbeit_kwh: Decimal,
) -> Option<Decimal> {
    if jahresarbeit_kwh <= BANDLAST_ARBEITSSCHWELLE_KWH
        || benutzungsstunden < BANDLAST_MINDESTBENUTZUNGSSTUNDEN
    {
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Sect19Vereinbarung {
    /// Which form the agreement takes.
    pub art: Sect19Art,
    /// The agreed fraction of the published Netzentgelt — `0.20` pays 20 %.
    ///
    /// Bounded to `(0, 1]` by [`Sect19Vereinbarung::pruefe_prozentsatz`], which
    /// the settlement runs before the fraction reaches any arithmetic.
    pub vereinbarter_prozentsatz: Decimal,
    /// The regulatory act the agreement rests on, for the trace.
    ///
    /// §19 Abs. 2 Satz 5 makes a Vereinbarung subject to a **Genehmigung** of
    /// the Regulierungsbehörde; Satz 7 reduces that to a written **Anzeige**
    /// where a Festlegung under §29 Abs. 1 EnWG has concretised the criteria,
    /// which BK4-22-089 does. One or the other always exists, so an agreement
    /// carrying neither reference is one the settlement reports.
    pub genehmigung: Option<String>,
}

impl Sect19Vereinbarung {
    /// Reject a fraction that is not an individual *Netzentgelt* at all.
    ///
    /// §19 Abs. 2 grants a **reduced** charge, so the fraction lives in
    /// `(0, 1]`: at or above 1 there is no reduction to grant, and at or below 0
    /// the Letztverbraucher would take network access free or be paid for it.
    /// Both are outside anything the ordinance authorises, so neither is a
    /// warning — the floors below are the graduated check, this is the domain.
    ///
    /// The fraction reaches the engine from a settlement request, so nothing
    /// upstream has constrained it.
    ///
    /// # Errors
    ///
    /// [`BillingError::InvalidInput`](crate::error::BillingError::InvalidInput)
    /// when the fraction is outside `(0, 1]`.
    pub fn pruefe_prozentsatz(&self) -> Result<(), crate::error::BillingError> {
        if self.vereinbarter_prozentsatz > Decimal::ZERO
            && self.vereinbarter_prozentsatz <= Decimal::ONE
        {
            return Ok(());
        }
        Err(crate::error::BillingError::InvalidInput {
            reason: format!(
                "§19 Abs. 2 StromNEV: the agreed fraction of the published Netzentgelt \
                 is {}, which is outside (0, 1] — an individuelles Netzentgelt reduces \
                 the published charge, it does not remove or raise it",
                self.vereinbarter_prozentsatz.normalize()
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A consumption that clears the Satz 2 Arbeitsschwelle.
    const UEBER_10_GWH: Decimal = dec!(10_000_001);

    /// The statutory staircase, at each Benutzungsstunden boundary.
    #[test]
    fn the_statutory_staircase() {
        let a = UEBER_10_GWH;
        assert_eq!(bandlast_mindestentgelt(dec!(7000), a), Some(dec!(0.20)));
        assert_eq!(bandlast_mindestentgelt(dec!(7499), a), Some(dec!(0.20)));
        assert_eq!(bandlast_mindestentgelt(dec!(7500), a), Some(dec!(0.15)));
        assert_eq!(bandlast_mindestentgelt(dec!(7999), a), Some(dec!(0.15)));
        assert_eq!(bandlast_mindestentgelt(dec!(8000), a), Some(dec!(0.10)));
        assert_eq!(bandlast_mindestentgelt(dec!(8760), a), Some(dec!(0.10)));
    }

    /// Both qualification conditions are required, not either.
    #[test]
    fn qualification_needs_hours_and_energy() {
        assert_eq!(bandlast_mindestentgelt(dec!(6999), UEBER_10_GWH), None);
        assert_eq!(
            bandlast_mindestentgelt(dec!(8000), dec!(9_999_999)),
            None,
            "9.999999 GWh is below the threshold however high the utilisation"
        );
    }

    /// The two thresholds read differently: the hours are *erreicht*
    /// („mindestens 7 000"), the energy is *übersteigt* („zehn Gigawattstunden").
    #[test]
    fn the_energy_threshold_must_be_exceeded_and_the_hours_only_reached() {
        assert_eq!(
            bandlast_mindestentgelt(BANDLAST_MINDESTBENUTZUNGSSTUNDEN, UEBER_10_GWH),
            Some(dec!(0.20)),
            "exactly 7 000 Benutzungsstunden qualifies — Satz 2 says `erreicht`"
        );
        assert_eq!(
            bandlast_mindestentgelt(dec!(8000), BANDLAST_ARBEITSSCHWELLE_KWH),
            None,
            "exactly 10 GWh does not qualify — Satz 2 says `übersteigt`"
        );
    }
}
