//! Netzseitige Umlagen — the levies the ÜNB collect through the Netzentgelt.
//!
//! Three levies ride on the network charge rather than on the commodity:
//!
//! | Levy | Basis | Funds |
//! |---|---|---|
//! | Aufschlag für besondere Netznutzung (§19 StromNEV-Umlage) | §19 Abs. 2 StromNEV, EnFG | reduced individual network charges |
//! | Offshore-Netzumlage | §17f EnWG, EnFG | offshore connection cost and compensation |
//! | KWKG-Umlage | §26 KWKG, EnFG | the KWK-Zuschlag paid to CHP operators |
//!
//! All three are set annually by the Übertragungsnetzbetreiber and published by
//! 25 October for the following calendar year.
//!
//! ## Why the rates are tabled by year rather than configured once
//!
//! A correction reopens an earlier delivery period and has to bill it at the
//! rate that applied *then*. A single configured scalar cannot express two
//! years at once, so the statutory series is held here and a configured value
//! overrides it only where an operator genuinely needs to depart from it. This
//! matches how Stromsteuer, Energiesteuer and the BEHG price are handled on the
//! supply side.
//!
//! ## Privileged consumption (EnFG)
//!
//! The Energiefinanzierungsgesetz replaced the older per-levy privilege rules
//! with one scheme of Letztverbrauchergruppen. The §19 StromNEV-Umlage is
//! published as an explicit A′/B′/C′ schedule; the Offshore- and KWKG-Umlage are
//! published as the non-privileged rate, with privileges granted per
//! Entnahmestelle under §§ 21 ff. EnFG. [`Letztverbrauchergruppe`] therefore
//! carries the group and the caller supplies the privileged rate where one has
//! been granted.

use rust_decimal::Decimal;
use rust_decimal::dec;

/// Letztverbrauchergruppe for the network levies (EnFG §§ 21 ff.).
///
/// The bands are per Entnahmestelle and per calendar year, not per contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Letztverbrauchergruppe {
    /// **A′** — the full levy. Applies to the first 1 GWh at an Entnahmestelle,
    /// and to every Entnahmestelle that does not exceed it.
    #[default]
    A,
    /// **B′** — consumption above 1 GWh/a at the same Entnahmestelle.
    B,
    /// **C′** — consumption above 1 GWh/a at an energy-intensive undertaking
    /// meeting the EnFG criteria.
    C,
    /// Exempt under §21 EnFG — the levy is zero rather than reduced.
    Befreit,
}

/// The first delivery year the tabled series covers.
///
/// Below this the tables claim nothing, so a caller billing an earlier period is
/// expected to supply the rate rather than be warned about a gap this crate
/// never undertook to fill.
pub const ERSTES_ERFASSTES_JAHR: i32 = 2026;

/// The threshold separating A′ from B′/C′, per Entnahmestelle and year.
pub const ENFG_SCHWELLE_KWH: Decimal = dec!(1_000_000);

/// §19 StromNEV-Umlage by year and Letztverbrauchergruppe, in ct/kWh.
///
/// Published as an explicit A′/B′/C′ schedule, which is why this levy is tabled
/// per group while the other two are not.
const SECT19_STROMNEV: &[(i32, Decimal, Decimal, Decimal)] = &[
    // year, A′, B′, C′
    (2026, dec!(1.559), dec!(0.050), dec!(0.025)),
];

/// Offshore-Netzumlage (§17f EnWG) by year, non-privileged, in ct/kWh.
const OFFSHORE_NETZUMLAGE: &[(i32, Decimal)] = &[(2026, dec!(0.941))];

/// KWKG-Umlage (§26 KWKG) by year, non-privileged, in ct/kWh.
const KWKG_UMLAGE: &[(i32, Decimal)] = &[(2026, dec!(0.446))];

/// The §19 StromNEV-Umlage for a year and group, in ct/kWh.
///
/// Returns `None` for a year the series does not cover — the caller must then
/// supply the rate explicitly rather than be billed at a neighbouring year's.
#[must_use]
pub fn sect19_stromnev_ct_per_kwh(year: i32, gruppe: Letztverbrauchergruppe) -> Option<Decimal> {
    if gruppe == Letztverbrauchergruppe::Befreit {
        return Some(Decimal::ZERO);
    }
    SECT19_STROMNEV
        .iter()
        .find(|(y, ..)| *y == year)
        .map(|(_, a, b, c)| match gruppe {
            Letztverbrauchergruppe::A => *a,
            Letztverbrauchergruppe::B => *b,
            Letztverbrauchergruppe::C => *c,
            Letztverbrauchergruppe::Befreit => Decimal::ZERO,
        })
}

/// The Offshore-Netzumlage for a year, in ct/kWh.
///
/// The published figure is the non-privileged rate; a privileged Entnahmestelle
/// pays what its EnFG decision grants, which the caller supplies.
#[must_use]
pub fn offshore_netzumlage_ct_per_kwh(
    year: i32,
    gruppe: Letztverbrauchergruppe,
) -> Option<Decimal> {
    if gruppe == Letztverbrauchergruppe::Befreit {
        return Some(Decimal::ZERO);
    }
    OFFSHORE_NETZUMLAGE
        .iter()
        .find(|(y, _)| *y == year)
        .map(|(_, r)| *r)
}

/// The KWKG-Umlage for a year, in ct/kWh.
///
/// As for the Offshore-Netzumlage, the published figure is the non-privileged
/// rate.
#[must_use]
pub fn kwkg_umlage_ct_per_kwh(year: i32, gruppe: Letztverbrauchergruppe) -> Option<Decimal> {
    if gruppe == Letztverbrauchergruppe::Befreit {
        return Some(Decimal::ZERO);
    }
    KWKG_UMLAGE
        .iter()
        .find(|(y, _)| *y == year)
        .map(|(_, r)| *r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 2026 schedule as published by the ÜNB.
    #[test]
    fn published_2026_rates() {
        use Letztverbrauchergruppe as G;
        assert_eq!(sect19_stromnev_ct_per_kwh(2026, G::A), Some(dec!(1.559)));
        assert_eq!(sect19_stromnev_ct_per_kwh(2026, G::B), Some(dec!(0.050)));
        assert_eq!(sect19_stromnev_ct_per_kwh(2026, G::C), Some(dec!(0.025)));
        assert_eq!(
            offshore_netzumlage_ct_per_kwh(2026, G::A),
            Some(dec!(0.941))
        );
        assert_eq!(kwkg_umlage_ct_per_kwh(2026, G::A), Some(dec!(0.446)));
    }

    /// §21 EnFG exempts rather than reduces, for every levy and every year.
    #[test]
    fn exemption_is_zero_not_missing() {
        use Letztverbrauchergruppe::Befreit;
        for year in [2020, 2026, 2099] {
            assert_eq!(
                sect19_stromnev_ct_per_kwh(year, Befreit),
                Some(Decimal::ZERO)
            );
            assert_eq!(
                offshore_netzumlage_ct_per_kwh(year, Befreit),
                Some(Decimal::ZERO)
            );
            assert_eq!(kwkg_umlage_ct_per_kwh(year, Befreit), Some(Decimal::ZERO));
        }
    }

    /// An uncovered year yields nothing rather than a neighbouring year's rate.
    ///
    /// Billing 2027 at the 2026 rate would be wrong by an amount nobody would
    /// notice until the ÜNB reconciliation.
    #[test]
    fn an_uncovered_year_has_no_rate() {
        use Letztverbrauchergruppe as G;
        assert_eq!(sect19_stromnev_ct_per_kwh(2025, G::A), None);
        assert_eq!(offshore_netzumlage_ct_per_kwh(2027, G::A), None);
        assert_eq!(kwkg_umlage_ct_per_kwh(1999, G::A), None);
    }

    /// A′ is the full levy; the privileged bands are strictly smaller.
    #[test]
    fn privileged_bands_are_lower_than_the_full_rate() {
        use Letztverbrauchergruppe as G;
        let a = sect19_stromnev_ct_per_kwh(2026, G::A).unwrap();
        let b = sect19_stromnev_ct_per_kwh(2026, G::B).unwrap();
        let c = sect19_stromnev_ct_per_kwh(2026, G::C).unwrap();
        assert!(b < a, "B′ must be below A′");
        assert!(c < b, "C′ must be below B′");
    }
}
