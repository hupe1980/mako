//! Preisobergrenzen für den Messstellenbetrieb — §30 MsbG.
//!
//! What a Messstellenbetreiber may charge for an intelligentes Messsystem is
//! capped, and the cap is split: part falls to the Netzbetreiber, the remainder
//! to the Letztverbraucher. The bands are set by annual consumption **or** by
//! installed generating capacity, whichever puts the metering point in the
//! higher band.
//!
//! ## Why this is checked rather than assumed
//!
//! These are Höchstbeträge in the same sense as the KAV §2 ceilings, and the
//! crate already refuses to let a Konzessionsabgabe exceed its ceiling silently.
//! A metering charge above the POG is the same class of defect — an amount the
//! customer is entitled to have refunded — and was previously unchecked: the MSB
//! settlement validated only that the fee was non-negative.

use rust_decimal::Decimal;
use rust_decimal::dec;

/// Which §30 MsbG case a metering point falls under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum MessstellenKategorie {
    /// **§30 Abs. 1** — Pflichteinbaufall.
    Pflichteinbau(PflichtBand),
    /// **§30 Abs. 3** — optionaler Einbau, at the Anschlussnutzer's request.
    ///
    /// A single ceiling regardless of consumption.
    OptionalerEinbau,
}

/// The §30 Abs. 1 bands.
///
/// A metering point falls in a band by annual consumption **or** by installed
/// capacity — whichever is higher. `Ueber100000` has no fixed total: §30 Abs. 1
/// allows an "angemessenes jährliches Entgelt", so only the Netzbetreiber's
/// share is capped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum PflichtBand {
    /// > 6 000 – ≤ 10 000 kWh.
    Bis10000,
    /// > 10 000 – ≤ 20 000 kWh, a steuerbare Verbrauchseinrichtung, or > 7 – ≤ 15 kW.
    Bis20000,
    /// > 20 000 – ≤ 50 000 kWh, or > 15 – ≤ 25 kW.
    Bis50000,
    /// > 50 000 – ≤ 100 000 kWh, or > 25 – ≤ 100 kW.
    Bis100000,
    /// > 100 000 kWh or > 100 kW.
    Ueber100000,
}

/// Who owes the charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
        (K::Pflichteinbau(B::Bis10000), E::Letztverbraucher) => Some(dec!(40)),
        (K::Pflichteinbau(B::Bis20000), E::Letztverbraucher) => Some(dec!(50)),
        (K::Pflichteinbau(B::Bis50000), E::Letztverbraucher) => Some(dec!(110)),
        (K::Pflichteinbau(B::Bis100000), E::Letztverbraucher) => Some(dec!(140)),
        // "angemessenes jährliches Entgelt" — no fixed figure.
        (K::Pflichteinbau(B::Ueber100000), E::Letztverbraucher) => None,
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

    /// The §30 Abs. 1 schedule, as published.
    #[test]
    fn the_pflichteinbau_schedule() {
        for (band, lv, total) in [
            (B::Bis10000, dec!(40), dec!(120)),
            (B::Bis20000, dec!(50), dec!(130)),
            (B::Bis50000, dec!(110), dec!(190)),
            (B::Bis100000, dec!(140), dec!(220)),
        ] {
            let k = K::Pflichteinbau(band);
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

    /// Above 100 000 kWh the Letztverbraucher's share is an angemessenes
    /// Entgelt, but the Netzbetreiber's share is capped like every other band.
    #[test]
    fn the_top_band_caps_only_the_grid_operators_share() {
        let k = K::Pflichteinbau(B::Ueber100000);
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
        let bands = [B::Bis10000, B::Bis20000, B::Bis50000, B::Bis100000];
        let mut previous = Decimal::ZERO;
        for band in bands {
            let ceiling = preisobergrenze_eur_per_jahr(K::Pflichteinbau(band), E::Letztverbraucher)
                .expect("a fixed ceiling");
            assert!(ceiling > previous, "{band:?} must exceed the band below it");
            previous = ceiling;
        }
    }
}
