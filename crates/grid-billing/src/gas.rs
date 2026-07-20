//! Gas network structure — Druckstufen and Kapazitätsprodukte, §15 GasNEV.
//!
//! Gas network charges differ structurally from Strom. §15 GasNEV splits the
//! costs into Einspeise- and Ausspeiseentgelte (Abs. 2), lets the exit charge
//! reflect "die Druckstufe im Ausspeisepunkt" (Abs. 3), permits differentiation
//! "nach der Druckstufe oder dem Leitungsdurchmesser" (Abs. 6), and requires
//! the charge system to account for "unterbrechbarer und unterjähriger
//! Kapazitätsprodukte" (Abs. 5).
//!
//! What the ordinance does **not** fix is any discount for interruptible
//! capacity — that is a property of the published price sheet. So, as with the
//! Strom Netzebene, the structure here is recorded and billed at supplied
//! rates; nothing is derived from a schedule this crate would have to invent.

use rust_decimal::Decimal;

/// The pressure level a metering point takes gas from.
///
/// The gas analogue of the Strom [`crate::netzebene::Netzebene`]: charges are
/// published per level, so the level is what makes a rate checkable against a
/// price sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum Druckstufe {
    /// Hochdruck — above 1 bar, the transmission-adjacent networks.
    Hochdruck,
    /// Mitteldruck — 0.1 to 1 bar.
    Mitteldruck,
    /// Niederdruck — up to 0.1 bar, the distribution networks households sit on.
    Niederdruck,
}

impl Druckstufe {
    /// A short label for position text and trace.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hochdruck => "Hochdruck",
            Self::Mitteldruck => "Mitteldruck",
            Self::Niederdruck => "Niederdruck",
        }
    }
}

/// Whether capacity is firm or interruptible.
///
/// §15 Abs. 5 GasNEV requires the charge system to account for interruptible
/// products; the discount itself comes from the price sheet, which is why the
/// rate on [`GasKapazitaet`] is per product rather than derived here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Kapazitaetsprodukt {
    /// Feste Kapazität — firm, not curtailable by the network operator.
    Fest,
    /// Unterbrechbare Kapazität — curtailable, priced below firm on every
    /// published sheet precisely because it can be interrupted.
    Unterbrechbar,
}

impl Kapazitaetsprodukt {
    /// A short label for position text and trace.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fest => "feste Kapazität",
            Self::Unterbrechbar => "unterbrechbare Kapazität",
        }
    }
}

/// A booked gas capacity and its annual rate.
///
/// The rate is annual (EUR per kWh/h per year, the standard price-sheet unit);
/// the engine pro-rates it over the settlement period by calendar days, and
/// records that convention in the trace where an auditor can see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct GasKapazitaet {
    /// Booked capacity in kWh/h.
    pub bestellte_kapazitaet_kwh_h: Decimal,
    /// Annual rate in EUR per kWh/h.
    pub entgelt_eur_per_kwh_h_a: Decimal,
    /// Firm or interruptible.
    pub produkt: Kapazitaetsprodukt,
    /// The pressure level the rate was published for, where known.
    pub druckstufe: Option<Druckstufe>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Levels order from high pressure down, and label for the trace.
    #[test]
    fn druckstufen_order_and_label() {
        assert!(Druckstufe::Hochdruck < Druckstufe::Niederdruck);
        assert_eq!(Druckstufe::Mitteldruck.label(), "Mitteldruck");
        assert_eq!(
            Kapazitaetsprodukt::Unterbrechbar.label(),
            "unterbrechbare Kapazität"
        );
    }
}
