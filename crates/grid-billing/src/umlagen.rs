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
//! All three are set annually by the Übertragungsnetzbetreiber and published
//! „bis zum 25. Oktober eines Kalenderjahres" for the following one (§ 11
//! EnFG). Every tabled row names the publication it is read from.
//!
//! A year with no row has no rate: [`crate::settle_nne`] refuses a Strom
//! period whose levy is neither tabled nor overridden, and prices a period
//! crossing 1 January at each year's own rate.
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
//!
//! ## The privilege is a tranche, not a rate
//!
//! B′ and C′ are published „für Strommengen **über** 1 000 000 kWh" at one
//! Entnahmestelle. The first Gigawattstunde of the calendar year is billed at
//! A′ whatever the group, and only the excess carries the reduced rate.
//! [`aufteilung`] splits a settlement period's quantity across that boundary,
//! which is why it needs the Jahresvorverbrauch: the threshold is annual and
//! per Entnahmestelle, while a settlement covers one period of the year.
//!
//! Applying B′ to the whole quantity would understate a B′ Entnahmestelle's
//! §19-Aufschlag by the first Gigawattstunde times the difference between the
//! two rates — roughly 15 000 EUR a year at the 2026 schedule.

use rust_decimal::Decimal;
use rust_decimal::dec;

/// Letztverbrauchergruppe for the network levies (EnFG §§ 21 ff.).
///
/// The bands are per Entnahmestelle and per calendar year, not per contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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

/// The threshold separating A′ from B′/C′, per Entnahmestelle and year.
///
/// The reduced rate is published for quantities *über* this figure, so the
/// Gigawattstunde itself is billed at A′.
pub const ENFG_SCHWELLE_KWH: Decimal = dec!(1_000_000);

/// How one settlement period's quantity splits across the EnFG 1-GWh boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Umlagenaufteilung {
    /// kWh of this period that fall in the year's first Gigawattstunde and so
    /// carry the A′ rate.
    pub voller_satz_kwh: Decimal,
    /// kWh of this period above the year's first Gigawattstunde, carrying the
    /// group's reduced rate.
    pub privilegiert_kwh: Decimal,
}

/// Split a period's quantity across the EnFG 1-GWh boundary.
///
/// `jahresvorverbrauch_kwh` is what this Entnahmestelle has already consumed in
/// the same calendar year *before* this period. The threshold is annual, so a
/// monthly settlement cannot tell which side of it the period falls on without
/// that figure.
///
/// Groups A′ and Befreit have no boundary — one rate covers the whole quantity,
/// and for Befreit that rate is zero. Their split puts everything on the
/// `voller_satz_kwh` side.
#[must_use]
pub fn aufteilung(
    gruppe: Letztverbrauchergruppe,
    menge_kwh: Decimal,
    jahresvorverbrauch_kwh: Decimal,
) -> Umlagenaufteilung {
    use Letztverbrauchergruppe as G;
    if matches!(gruppe, G::A | G::Befreit) {
        return Umlagenaufteilung {
            voller_satz_kwh: menge_kwh,
            privilegiert_kwh: Decimal::ZERO,
        };
    }
    let rest = (ENFG_SCHWELLE_KWH - jahresvorverbrauch_kwh).max(Decimal::ZERO);
    let voller_satz_kwh = menge_kwh.min(rest);
    Umlagenaufteilung {
        voller_satz_kwh,
        privilegiert_kwh: menge_kwh - voller_satz_kwh,
    }
}

/// The publication the 2026 rows are read from.
///
/// The Übertragungsnetzbetreiber publish the following year's Umlagen on their
/// joint platform „bis zum 25. Oktober eines Kalenderjahres" (§ 11 EnFG).
const UENB_2026: &str = "Übertragungsnetzbetreiber, Netzentgeltumlagen 2026 \
                         (netztransparenz.de, Veröffentlichung nach § 11 EnFG im Oktober 2025)";

/// One year of the §19 StromNEV-Umlage, A′/B′/C′ in ct/kWh, with its source.
#[derive(Debug, Clone, Copy)]
struct Sect19Zeile {
    jahr: i32,
    a: Decimal,
    b: Decimal,
    c: Decimal,
    quelle: &'static str,
}

/// One year of a levy published only at the non-privileged rate, in ct/kWh,
/// with its source.
#[derive(Debug, Clone, Copy)]
struct UmlageZeile {
    jahr: i32,
    satz: Decimal,
    quelle: &'static str,
}

/// §19 StromNEV-Umlage by year and Letztverbrauchergruppe, in ct/kWh.
///
/// Published as an explicit A′/B′/C′ schedule, which is why this levy is tabled
/// per group while the other two are not.
const SECT19_STROMNEV: &[Sect19Zeile] = &[Sect19Zeile {
    jahr: 2026,
    a: dec!(1.559),
    b: dec!(0.050),
    c: dec!(0.025),
    quelle: UENB_2026,
}];

/// Offshore-Netzumlage (§17f EnWG) by year, non-privileged, in ct/kWh.
const OFFSHORE_NETZUMLAGE: &[UmlageZeile] = &[UmlageZeile {
    jahr: 2026,
    satz: dec!(0.941),
    quelle: UENB_2026,
}];

/// KWKG-Umlage (§26 KWKG) by year, non-privileged, in ct/kWh.
const KWKG_UMLAGE: &[UmlageZeile] = &[UmlageZeile {
    jahr: 2026,
    satz: dec!(0.446),
    quelle: UENB_2026,
}];

/// The publication a tabled year of the §19 StromNEV-Umlage is read from.
///
/// `None` for a year the series does not cover.
#[must_use]
pub fn sect19_stromnev_quelle(year: i32) -> Option<&'static str> {
    SECT19_STROMNEV
        .iter()
        .find(|z| z.jahr == year)
        .map(|z| z.quelle)
}

/// The publication a tabled year of the Offshore-Netzumlage is read from.
#[must_use]
pub fn offshore_netzumlage_quelle(year: i32) -> Option<&'static str> {
    OFFSHORE_NETZUMLAGE
        .iter()
        .find(|z| z.jahr == year)
        .map(|z| z.quelle)
}

/// The publication a tabled year of the KWKG-Umlage is read from.
#[must_use]
pub fn kwkg_umlage_quelle(year: i32) -> Option<&'static str> {
    KWKG_UMLAGE
        .iter()
        .find(|z| z.jahr == year)
        .map(|z| z.quelle)
}

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
        .find(|z| z.jahr == year)
        .map(|z| match gruppe {
            Letztverbrauchergruppe::A => z.a,
            Letztverbrauchergruppe::B => z.b,
            Letztverbrauchergruppe::C => z.c,
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
        .find(|z| z.jahr == year)
        .map(|z| z.satz)
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
    KWKG_UMLAGE.iter().find(|z| z.jahr == year).map(|z| z.satz)
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

    /// Every tabled row names the publication it is read from: a rate without a
    /// source cannot be checked against the ÜNB figure, and a mistyped one would
    /// bill every Strom period of its year wrong.
    #[test]
    fn every_tabled_row_names_its_publication() {
        let jahre = SECT19_STROMNEV
            .iter()
            .map(|z| (z.jahr, z.quelle))
            .chain(OFFSHORE_NETZUMLAGE.iter().map(|z| (z.jahr, z.quelle)))
            .chain(KWKG_UMLAGE.iter().map(|z| (z.jahr, z.quelle)));
        for (jahr, quelle) in jahre {
            assert!(
                !quelle.trim().is_empty(),
                "the {jahr} Umlage row names no publication"
            );
        }
        assert!(sect19_stromnev_quelle(2026).is_some());
        assert!(offshore_netzumlage_quelle(2026).is_some());
        assert!(kwkg_umlage_quelle(2026).is_some());
        assert_eq!(kwkg_umlage_quelle(2027), None);
    }

    /// Each levy is tabled for the same years, so a period never finds one
    /// levy's year and misses another's.
    #[test]
    fn the_three_tables_cover_the_same_years() {
        let s19: Vec<i32> = SECT19_STROMNEV.iter().map(|z| z.jahr).collect();
        let off: Vec<i32> = OFFSHORE_NETZUMLAGE.iter().map(|z| z.jahr).collect();
        let kwk: Vec<i32> = KWKG_UMLAGE.iter().map(|z| z.jahr).collect();
        assert_eq!(s19, off);
        assert_eq!(s19, kwk);
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

    /// A′ and Befreit have no boundary — everything sits on the full-rate side.
    #[test]
    fn an_unprivileged_group_is_never_split() {
        use Letztverbrauchergruppe as G;
        for g in [G::A, G::Befreit] {
            let a = aufteilung(g, dec!(5_000_000), dec!(3_000_000));
            assert_eq!(a.voller_satz_kwh, dec!(5_000_000), "{g:?}");
            assert_eq!(a.privilegiert_kwh, Decimal::ZERO, "{g:?}");
        }
    }

    /// The reduced rate is published „für Strommengen über 1 000 000 kWh", so
    /// the first Gigawattstunde of the year carries A′ even for a B′ customer.
    #[test]
    fn the_first_gigawatt_hour_of_the_year_is_never_privileged() {
        let a = aufteilung(Letztverbrauchergruppe::B, dec!(2_500_000), Decimal::ZERO);
        assert_eq!(a.voller_satz_kwh, ENFG_SCHWELLE_KWH);
        assert_eq!(a.privilegiert_kwh, dec!(1_500_000));
    }

    /// Once the year's allowance is used up, the whole period is privileged.
    #[test]
    fn a_period_past_the_threshold_is_wholly_privileged() {
        let a = aufteilung(Letztverbrauchergruppe::C, dec!(400_000), dec!(1_000_000));
        assert_eq!(a.voller_satz_kwh, Decimal::ZERO);
        assert_eq!(a.privilegiert_kwh, dec!(400_000));
    }

    /// A period straddling the boundary splits at it, not before or after.
    #[test]
    fn a_straddling_period_splits_at_the_boundary() {
        let a = aufteilung(Letztverbrauchergruppe::B, dec!(300_000), dec!(900_000));
        assert_eq!(a.voller_satz_kwh, dec!(100_000));
        assert_eq!(a.privilegiert_kwh, dec!(200_000));
    }

    /// The split conserves the quantity, wherever the year stands.
    #[test]
    fn the_split_conserves_the_quantity() {
        use Letztverbrauchergruppe as G;
        for g in [G::A, G::B, G::C, G::Befreit] {
            for vor in [dec!(0), dec!(999_999), dec!(1_000_000), dec!(9_000_000)] {
                let menge = dec!(750_000);
                let a = aufteilung(g, menge, vor);
                assert_eq!(
                    a.voller_satz_kwh + a.privilegiert_kwh,
                    menge,
                    "{g:?} after {vor}"
                );
                assert!(a.voller_satz_kwh >= Decimal::ZERO);
                assert!(a.privilegiert_kwh >= Decimal::ZERO);
            }
        }
    }
}
