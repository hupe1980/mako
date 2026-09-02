//! Preisobergrenzen für den Messstellenbetrieb — §30 MsbG.
//!
//! What a Messstellenbetreiber may charge for an intelligentes Messsystem is
//! capped, and the cap is split: part falls to the Netzbetreiber, the remainder
//! to the Letztverbraucher. The bands are set by annual consumption **or** by
//! installed generating capacity, whichever puts the metering point in the
//! higher band.
//!
//! ## The band is derived, never asserted
//!
//! §30 Abs. 1 states five Nummern, each a disjunction of criteria over facts the
//! settlement already knows about the metering point. [`PflichtEinstufung`]
//! carries those facts and [`PflichtEinstufung::band`] walks the Nummern, so the
//! ceiling a charge is measured against follows from the metering point rather
//! than from whichever band the caller named. A settlement request cannot pick
//! its own ceiling.
//!
//! ## Why this is checked rather than assumed
//!
//! These are Höchstbeträge in the same sense as the KAV §2 ceilings, and the
//! crate already refuses to let a Konzessionsabgabe exceed its ceiling silently.
//! A metering charge above the POG is the same class of defect — an amount the
//! customer is entitled to have refunded — so the settlement checks the ceiling
//! and not merely that the fee is non-negative.

use rust_decimal::Decimal;
use rust_decimal::dec;

/// Which §30 MsbG case a metering point falls under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MessstellenKategorie {
    /// **§30 Abs. 1** — Pflichteinbaufall, described by the facts that classify it.
    Pflichteinbau(PflichtEinstufung),
    /// **§30 Abs. 3** — optionaler Einbau, at the Anschlussnutzer's request.
    ///
    /// A single ceiling regardless of consumption.
    OptionalerEinbau,
}

/// The facts §30 Abs. 1 classifies a Pflichteinbau metering point by.
///
/// Every field is optional because a metering point need not exhibit every
/// criterion: a Letztverbraucher has a Jahresverbrauch and no installierte
/// Leistung, an Erzeugungsanlage the other way round, and a §14a
/// Vereinbarung is a fact about the Zählpunkt on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct PflichtEinstufung {
    /// Jahresstromverbrauch in kWh at this Abnahmestelle.
    #[serde(default)]
    pub jahresverbrauch_kwh: Option<Decimal>,
    /// Installierte Leistung in kW of the Anlage at this Zählpunkt.
    #[serde(default)]
    pub installierte_leistung_kw: Option<Decimal>,
    /// A §14a EnWG Vereinbarung exists over a steuerbare Verbrauchseinrichtung
    /// at this Zählpunkt — a Nummer-4 criterion in its own right.
    #[serde(default)]
    pub steuerbare_verbrauchseinrichtung: bool,
}

impl PflichtEinstufung {
    /// The §30 Abs. 1 band these facts put the metering point in.
    ///
    /// The Nummern are walked from the top down, so a metering point that meets
    /// several takes the highest — which is what "**oder**" inside each Nummer
    /// and the descending order of the catalogue together mean.
    ///
    /// With no fact at all the result is [`PflichtBand::Bis10000`], the tightest
    /// ceiling §30 Abs. 1 sets. A Pflichteinbaufall exists at all only above
    /// 6 000 kWh (§29 Abs. 1), so that is the floor of the catalogue rather than
    /// a guess, and an unclassifiable point is measured against the strictest
    /// cap rather than escaping the check.
    #[must_use]
    pub fn band(&self) -> PflichtBand {
        let kwh = self.jahresverbrauch_kwh.unwrap_or(Decimal::ZERO);
        let kw = self.installierte_leistung_kw.unwrap_or(Decimal::ZERO);

        // Nr. 1 — > 100 000 kWh oder > 100 kW.
        if kwh > dec!(100_000) || kw > dec!(100) {
            PflichtBand::Ueber100000
        // Nr. 2 — > 50 000 bis einschließlich 100 000 kWh oder > 25 bis einschließlich 100 kW.
        } else if kwh > dec!(50_000) || kw > dec!(25) {
            PflichtBand::Bis100000
        // Nr. 3 — > 20 000 bis einschließlich 50 000 kWh oder > 15 bis einschließlich 25 kW.
        } else if kwh > dec!(20_000) || kw > dec!(15) {
            PflichtBand::Bis50000
        // Nr. 4 — > 10 000 bis einschließlich 20 000 kWh, eine steuerbare
        // Verbrauchseinrichtung mit §14a-Vereinbarung, oder > 7 bis
        // einschließlich 15 kW.
        } else if kwh > dec!(10_000) || kw > dec!(7) || self.steuerbare_verbrauchseinrichtung {
            PflichtBand::Bis20000
        // Nr. 5 — > 6 000 bis einschließlich 10 000 kWh.
        } else {
            PflichtBand::Bis10000
        }
    }
}

/// The §30 Abs. 1 bands.
///
/// `Ueber100000` has no fixed total: §30 Abs. 1 allows an "angemessenes
/// jährliches Entgelt", so only the Netzbetreiber's share is capped.
///
/// Reached through [`PflichtEinstufung::band`] rather than named directly, so a
/// band is always the consequence of the metering point's facts.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum PflichtBand {
    /// Nr. 5 — > 6 000 – ≤ 10 000 kWh.
    Bis10000,
    /// Nr. 4 — > 10 000 – ≤ 20 000 kWh, a steuerbare Verbrauchseinrichtung, or > 7 – ≤ 15 kW.
    Bis20000,
    /// Nr. 3 — > 20 000 – ≤ 50 000 kWh, or > 15 – ≤ 25 kW.
    Bis50000,
    /// Nr. 2 — > 50 000 – ≤ 100 000 kWh, or > 25 – ≤ 100 kW.
    Bis100000,
    /// Nr. 1 — > 100 000 kWh or > 100 kW.
    Ueber100000,
}

/// Who owes the charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Entgeltschuldner {
    /// The Netzbetreiber's share.
    Netzbetreiber,
    /// The Letztverbraucher's share.
    Letztverbraucher,
}

/// The §30 MsbG ceiling in EUR per year, or `None` where the statute sets none.
///
/// `None` means "no fixed ceiling" — the >100 000 kWh band, where §30 Abs. 1
/// allows an angemessenes Entgelt for the Letztverbraucher's share. It does not
/// mean "unchecked": the Netzbetreiber's share is capped in every band.
#[must_use]
pub fn preisobergrenze_eur_per_jahr(
    kategorie: MessstellenKategorie,
    schuldner: Entgeltschuldner,
) -> Option<Decimal> {
    use Entgeltschuldner as E;
    use MessstellenKategorie as K;
    use PflichtBand as B;

    match (kategorie, schuldner) {
        // §30 Abs. 1: the Netzbetreiber's share is 80 EUR in every band.
        (K::Pflichteinbau(_), E::Netzbetreiber) => Some(dec!(80)),
        (K::Pflichteinbau(einstufung), E::Letztverbraucher) => match einstufung.band() {
            B::Bis10000 => Some(dec!(40)),
            B::Bis20000 => Some(dec!(50)),
            B::Bis50000 => Some(dec!(110)),
            B::Bis100000 => Some(dec!(140)),
            // "angemessenes jährliches Entgelt" — no fixed figure.
            B::Ueber100000 => None,
        },
        // §30 Abs. 3: 60 EUR in total, 30 EUR each.
        (K::OptionalerEinbau, _) => Some(dec!(30)),
    }
}

/// The combined §30 Abs. 1 ceiling across both parties, where one is fixed.
#[must_use]
pub fn gesamtobergrenze_eur_per_jahr(kategorie: MessstellenKategorie) -> Option<Decimal> {
    let nb = preisobergrenze_eur_per_jahr(kategorie, Entgeltschuldner::Netzbetreiber)?;
    let lv = preisobergrenze_eur_per_jahr(kategorie, Entgeltschuldner::Letztverbraucher)?;
    Some(nb + lv)
}

/// **§30 Abs. 2** — the additional yearly ceiling per party for installing and
/// operating a Steuereinrichtung at the Netzanschlusspunkt.
pub const STEUEREINRICHTUNG_OBERGRENZE_EUR_PER_JAHR: Decimal = dec!(50);

#[cfg(test)]
mod tests {
    use super::*;
    use Entgeltschuldner as E;
    use MessstellenKategorie as K;
    use PflichtBand as B;

    /// A Pflichteinbau point known only by its Jahresverbrauch.
    fn verbrauch(kwh: Decimal) -> MessstellenKategorie {
        K::Pflichteinbau(PflichtEinstufung {
            jahresverbrauch_kwh: Some(kwh),
            ..PflichtEinstufung::default()
        })
    }

    /// A Pflichteinbau point known only by its installierte Leistung.
    fn leistung(kw: Decimal) -> PflichtEinstufung {
        PflichtEinstufung {
            installierte_leistung_kw: Some(kw),
            ..PflichtEinstufung::default()
        }
    }

    /// The §30 Abs. 1 schedule, as published.
    #[test]
    fn the_pflichteinbau_schedule() {
        for (kwh, lv, total) in [
            (dec!(9_000), dec!(40), dec!(120)),
            (dec!(15_000), dec!(50), dec!(130)),
            (dec!(30_000), dec!(110), dec!(190)),
            (dec!(80_000), dec!(140), dec!(220)),
        ] {
            let k = verbrauch(kwh);
            assert_eq!(
                preisobergrenze_eur_per_jahr(k, E::Netzbetreiber),
                Some(dec!(80))
            );
            assert_eq!(
                preisobergrenze_eur_per_jahr(k, E::Letztverbraucher),
                Some(lv)
            );
            assert_eq!(gesamtobergrenze_eur_per_jahr(k), Some(total));
        }
    }

    /// Each Nummer's Jahresverbrauch bound, at the boundary. Every bound reads
    /// „über X bis einschließlich Y", so X belongs to the band below and Y to
    /// this one.
    #[test]
    fn the_consumption_bounds_are_exclusive_below_and_inclusive_above() {
        for (kwh, band) in [
            (dec!(6_001), B::Bis10000),
            (dec!(10_000), B::Bis10000),
            (dec!(10_001), B::Bis20000),
            (dec!(20_000), B::Bis20000),
            (dec!(20_001), B::Bis50000),
            (dec!(50_000), B::Bis50000),
            (dec!(50_001), B::Bis100000),
            (dec!(100_000), B::Bis100000),
            (dec!(100_001), B::Ueber100000),
        ] {
            let einstufung = PflichtEinstufung {
                jahresverbrauch_kwh: Some(kwh),
                ..PflichtEinstufung::default()
            };
            assert_eq!(einstufung.band(), band, "{kwh} kWh");
        }
    }

    /// The installierte-Leistung criteria run on their own scale — 7/15/25/100 kW.
    #[test]
    fn the_capacity_bounds_classify_a_generating_point() {
        for (kw, band) in [
            (dec!(7), B::Bis10000),
            (dec!(7.5), B::Bis20000),
            (dec!(15), B::Bis20000),
            (dec!(15.1), B::Bis50000),
            (dec!(25), B::Bis50000),
            (dec!(25.1), B::Bis100000),
            (dec!(100), B::Bis100000),
            (dec!(100.1), B::Ueber100000),
        ] {
            assert_eq!(leistung(kw).band(), band, "{kw} kW");
        }
    }

    /// Nr. 4 lists the §14a Zählpunkt beside the 10 000–20 000 kWh band, so a
    /// steuerbare Verbrauchseinrichtung reaches that band on consumption alone
    /// that would otherwise sit in Nr. 5.
    #[test]
    fn a_sect14a_zaehlpunkt_reaches_the_nummer_four_band() {
        let einstufung = PflichtEinstufung {
            jahresverbrauch_kwh: Some(dec!(7_000)),
            steuerbare_verbrauchseinrichtung: true,
            ..PflichtEinstufung::default()
        };
        assert_eq!(einstufung.band(), B::Bis20000);
        assert_eq!(
            preisobergrenze_eur_per_jahr(K::Pflichteinbau(einstufung), E::Letztverbraucher),
            Some(dec!(50))
        );
    }

    /// Each Nummer is a disjunction: the criterion that classifies highest wins.
    #[test]
    fn the_highest_matching_nummer_wins() {
        let einstufung = PflichtEinstufung {
            jahresverbrauch_kwh: Some(dec!(7_000)),
            installierte_leistung_kw: Some(dec!(60)),
            steuerbare_verbrauchseinrichtung: true,
        };
        assert_eq!(
            einstufung.band(),
            B::Bis100000,
            "60 kW is Nr. 2 — the consumption and the §14a fact classify lower"
        );
    }

    /// With no fact at all the tightest ceiling applies, so an unclassifiable
    /// point is still measured rather than let through.
    #[test]
    fn an_unclassifiable_point_takes_the_tightest_ceiling() {
        let k = K::Pflichteinbau(PflichtEinstufung::default());
        assert_eq!(
            preisobergrenze_eur_per_jahr(k, E::Letztverbraucher),
            Some(dec!(40))
        );
    }

    /// Above 100 000 kWh the Letztverbraucher's share is an angemessenes
    /// Entgelt, but the Netzbetreiber's share is capped like every other band.
    #[test]
    fn the_top_band_caps_only_the_grid_operators_share() {
        let k = verbrauch(dec!(250_000));
        assert_eq!(
            preisobergrenze_eur_per_jahr(k, E::Netzbetreiber),
            Some(dec!(80))
        );
        assert_eq!(preisobergrenze_eur_per_jahr(k, E::Letztverbraucher), None);
        assert_eq!(
            gesamtobergrenze_eur_per_jahr(k),
            None,
            "no total where one share is open"
        );
    }

    /// §30 Abs. 3 is one ceiling regardless of consumption.
    #[test]
    fn an_optional_installation_is_capped_at_thirty_each() {
        for schuldner in [E::Netzbetreiber, E::Letztverbraucher] {
            assert_eq!(
                preisobergrenze_eur_per_jahr(K::OptionalerEinbau, schuldner),
                Some(dec!(30))
            );
        }
        assert_eq!(
            gesamtobergrenze_eur_per_jahr(K::OptionalerEinbau),
            Some(dec!(60))
        );
    }

    /// The bands rise monotonically — a higher band never caps lower.
    #[test]
    fn the_bands_rise_monotonically() {
        let mut previous = Decimal::ZERO;
        for kwh in [dec!(9_000), dec!(15_000), dec!(30_000), dec!(80_000)] {
            let ceiling = preisobergrenze_eur_per_jahr(verbrauch(kwh), E::Letztverbraucher)
                .expect("a fixed ceiling");
            assert!(
                ceiling > previous,
                "{kwh} kWh must exceed the band below it"
            );
            previous = ceiling;
        }
    }
}
