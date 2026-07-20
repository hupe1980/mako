//! Netzebenen und Entgeltsystematik — §17 StromNEV.
//!
//! ## What §17 actually says
//!
//! §17 Abs. 1–2 establishes one system for metered consumption: a
//! **Jahresleistungspreis** in EUR per kW of annual peak demand, plus an
//! **Arbeitspreis** in ct per kWh.
//!
//! §17 Abs. 6 allows an Arbeitspreis-only alternative for consumption up to
//! 100 000 kWh a year in the Niederspannungsnetz — which is what an SLP customer
//! is billed on.
//!
//! §17 Abs. 8's Monatsleistungspreis applies only to Landstrom for seagoing
//! vessels; it is not a general alternative and is not modelled here.
//!
//! ## Benutzungsstundenzahl
//!
//! The Benutzungsstundenzahl — annual energy divided by annual peak demand —
//! does not appear in §17 as a threshold. It is the convention by which a
//! Preisblatt publishes two price pairs, one for high-utilisation and one for
//! low-utilisation offtake, and the boundary is a property of the price sheet
//! rather than of the ordinance.
//!
//! So it is **computed and recorded, not applied**: [`benutzungsstundenzahl`]
//! puts the figure in the calculation trace, where an auditor can check that the
//! rates supplied were the ones the price sheet meant for that utilisation. The
//! crate does not pick the rate — it never resolves a Preisblatt.

use rust_decimal::Decimal;

/// A voltage level, and the transformation between two of them.
///
/// Netzentgelte are published per level, and a metering point is billed at the
/// level it takes supply from. Transformation levels exist because a customer
/// supplied out of a transformer pays for that transformer as well as for the
/// network above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum Netzebene {
    /// Höchstspannung — 380/220 kV.
    Hoechstspannung,
    /// Umspannung Höchst- zu Hochspannung.
    UmspannungHoechstHoch,
    /// Hochspannung — 110 kV.
    Hochspannung,
    /// Umspannung Hoch- zu Mittelspannung.
    UmspannungHochMittel,
    /// Mittelspannung — 1 to 60 kV.
    Mittelspannung,
    /// Umspannung Mittel- zu Niederspannung.
    UmspannungMittelNieder,
    /// Niederspannung — below 1 kV.
    Niederspannung,
}

impl Netzebene {
    /// A short label for the position text and the trace.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hoechstspannung => "Höchstspannung",
            Self::UmspannungHoechstHoch => "Umspannung HöS/HS",
            Self::Hochspannung => "Hochspannung",
            Self::UmspannungHochMittel => "Umspannung HS/MS",
            Self::Mittelspannung => "Mittelspannung",
            Self::UmspannungMittelNieder => "Umspannung MS/NS",
            Self::Niederspannung => "Niederspannung",
        }
    }

    /// `true` for a transformation level rather than a network level.
    #[must_use]
    pub const fn ist_umspannung(self) -> bool {
        matches!(
            self,
            Self::UmspannungHoechstHoch | Self::UmspannungHochMittel | Self::UmspannungMittelNieder
        )
    }
}

/// The §17 StromNEV threshold below which Abs. 6 permits an Arbeitspreis-only
/// tariff in the Niederspannungsnetz.
pub const ARBEITSPREIS_NUR_GRENZE_KWH: Decimal = rust_decimal::dec!(100_000);

/// The Benutzungsstundenzahl — annual energy over annual peak demand, in hours.
///
/// Returns `None` when the peak demand is zero: the ratio is undefined, and
/// reporting a number there would invite a comparison against a price-sheet
/// threshold that cannot be meaningful.
///
/// The figure belongs in the trace rather than in the calculation. Which of a
/// price sheet's two rate pairs applies at a given utilisation is a property of
/// that sheet; this crate is given the rates and records what they should have
/// been chosen against.
#[must_use]
pub fn benutzungsstundenzahl(
    jahresarbeit_kwh: Decimal,
    jahreshoechstleistung_kw: Decimal,
) -> Option<Decimal> {
    if jahreshoechstleistung_kw.is_zero() {
        return None;
    }
    Some((jahresarbeit_kwh / jahreshoechstleistung_kw).round_dp(1))
}

/// Whether §17 Abs. 6 permits billing this metering point on an Arbeitspreis
/// alone.
///
/// Both conditions must hold: supply out of the Niederspannungsnetz, and annual
/// consumption at or below 100 000 kWh.
#[must_use]
pub fn arbeitspreis_nur_zulaessig(ebene: Netzebene, jahresarbeit_kwh: Decimal) -> bool {
    ebene == Netzebene::Niederspannung && jahresarbeit_kwh <= ARBEITSPREIS_NUR_GRENZE_KWH
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    /// 8 760 h is a full year at constant load — the theoretical maximum.
    #[test]
    fn benutzungsstunden_is_energy_over_peak() {
        // 1 000 000 kWh at a 200 kW peak = 5 000 h.
        assert_eq!(
            benutzungsstundenzahl(dec!(1_000_000), dec!(200)),
            Some(dec!(5000))
        );
        // A flat 100 kW load all year.
        assert_eq!(
            benutzungsstundenzahl(dec!(876_000), dec!(100)),
            Some(dec!(8760))
        );
    }

    /// Without a peak the ratio is undefined, not zero.
    #[test]
    fn benutzungsstunden_is_undefined_without_a_peak() {
        assert_eq!(benutzungsstundenzahl(dec!(1000), Decimal::ZERO), None);
    }

    /// §17 Abs. 6 needs both conditions: Niederspannung *and* ≤100 000 kWh.
    #[test]
    fn the_arbeitspreis_only_option_needs_both_conditions() {
        assert!(arbeitspreis_nur_zulaessig(
            Netzebene::Niederspannung,
            dec!(3500)
        ));
        assert!(arbeitspreis_nur_zulaessig(
            Netzebene::Niederspannung,
            dec!(100_000)
        ));
        assert!(
            !arbeitspreis_nur_zulaessig(Netzebene::Niederspannung, dec!(100_001)),
            "above the threshold"
        );
        assert!(
            !arbeitspreis_nur_zulaessig(Netzebene::Mittelspannung, dec!(3500)),
            "the option is Niederspannung only"
        );
    }

    /// The levels order from highest voltage to lowest, and transformation
    /// levels are distinguishable from network levels.
    #[test]
    fn the_levels_order_and_classify() {
        assert!(Netzebene::Hoechstspannung < Netzebene::Niederspannung);
        assert!(Netzebene::UmspannungHochMittel.ist_umspannung());
        assert!(!Netzebene::Mittelspannung.ist_umspannung());
        assert_eq!(
            Netzebene::UmspannungMittelNieder.label(),
            "Umspannung MS/NS"
        );
    }
}
