//! The typed Strom- and Energiesteuer lines of an issued invoice.
//!
//! The Versorger owes the Stromsteuer (§ 5 Abs. 2 StromStG) and the Lieferer
//! the Energiesteuer on Erdgas (§ 38 Abs. 2 EnergieStG), and both declare it to
//! the Hauptzollamt (§ 8 StromStG, § 39 EnergieStG). A declaration is a sum over
//! the tax lines of the invoices issued, grouped by ground — so each line is
//! stored typed, beside the `billing_records` row it belongs to and in the
//! statement that writes that row, rather than left inside the invoice JSON.
//!
//! | Position tag | [`Steuer`] | [`Art`] |
//! |---|---|---|
//! | `stromsteuer` | `STROMSTEUER` | `REGELSATZ` |
//! | `stromsteuer` + `stromsteuer_ermaessigt` | `STROMSTEUER` | `ERMAESSIGUNG` |
//! | `stromsteuer_befreiung` | `STROMSTEUER` | `BEFREIUNG` |
//! | `energiesteuer_gas` | `ENERGIESTEUER` | `REGELSATZ` |
//! | `energiesteuer_gas_befreiung` | `ENERGIESTEUER` | `BEFREIUNG` |
//!
//! A Befreiung is a line too: its quantity is supplied tax-free under a named
//! ground, which a declaration states beside the taxed quantity. A
//! Stornorechnung's lines are its original's, negated
//! ([`crate::pg::insert_correction_record`]), so a Storno nets them to zero.

use energy_billing::{BillingPosition, Invoice};
use rust_decimal::Decimal;

/// Which tax a line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Steuer {
    /// § 3 StromStG.
    Stromsteuer,
    /// § 2 Abs. 3 Satz 1 Nr. 4 EnergieStG (Erdgas).
    Energiesteuer,
}

impl Steuer {
    /// Every value, in the order of the `steuerzeilen.steuer` CHECK list.
    pub const ALL: [Self; 2] = [Self::Stromsteuer, Self::Energiesteuer];

    /// The stored value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stromsteuer => "STROMSTEUER",
            Self::Energiesteuer => "ENERGIESTEUER",
        }
    }
}

/// On which ground a quantity is taxed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Art {
    /// The Regelsatz.
    Regelsatz,
    /// A reduced rate, its ground in [`Steuerzeile::rechtsgrundlage`].
    Ermaessigung,
    /// Supplied tax-free, its ground in [`Steuerzeile::rechtsgrundlage`].
    Befreiung,
}

impl Art {
    /// Every value, in the order of the `steuerzeilen.art` CHECK list.
    pub const ALL: [Self; 3] = [Self::Regelsatz, Self::Ermaessigung, Self::Befreiung];

    /// The stored value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Regelsatz => "REGELSATZ",
            Self::Ermaessigung => "ERMAESSIGUNG",
            Self::Befreiung => "BEFREIUNG",
        }
    }
}

/// One tax line of an invoice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Steuerzeile {
    /// The tax.
    pub steuer: Steuer,
    /// The ground.
    pub art: Art,
    /// The provision the line is taxed (or exempted) under, as the invoice
    /// cites it.
    pub rechtsgrundlage: String,
    /// The quantity — kWh for Strom, kWh_Hs for Erdgas.
    pub menge: Decimal,
    /// `kWh` or `kWh_Hs`.
    pub einheit: String,
    /// The rate in ct per unit; `0` on a Befreiung.
    pub satz_ct: Decimal,
    /// The amount invoiced, in EUR; `0` on a Befreiung.
    pub betrag_eur: Decimal,
}

/// The tax lines of `invoice`, in position order.
#[must_use]
pub fn aus_rechnung(invoice: &Invoice) -> Vec<Steuerzeile> {
    invoice.positions.iter().filter_map(zeile).collect()
}

fn zeile(p: &BillingPosition) -> Option<Steuerzeile> {
    let tagged = |t: &str| p.tags.iter().any(|x| x == t);
    let (steuer, art, fallback) = if tagged("stromsteuer_befreiung") {
        (Steuer::Stromsteuer, Art::Befreiung, "StromStG")
    } else if tagged("stromsteuer") && tagged("stromsteuer_ermaessigt") {
        (Steuer::Stromsteuer, Art::Ermaessigung, "StromStG")
    } else if tagged("stromsteuer") {
        (Steuer::Stromsteuer, Art::Regelsatz, "§ 3 StromStG")
    } else if tagged("energiesteuer_gas_befreiung") {
        (Steuer::Energiesteuer, Art::Befreiung, "EnergieStG")
    } else if tagged("energiesteuer_gas") {
        (
            Steuer::Energiesteuer,
            Art::Regelsatz,
            "§ 2 Abs. 3 Satz 1 Nr. 4 EnergieStG",
        )
    } else {
        return None;
    };
    let (satz_ct, betrag_eur) = if art == Art::Befreiung {
        (Decimal::ZERO, Decimal::ZERO)
    } else {
        (p.unit_price_eur * Decimal::ONE_HUNDRED, p.net_eur)
    };
    Some(Steuerzeile {
        steuer,
        art,
        rechtsgrundlage: p.legal_basis.clone().unwrap_or_else(|| fallback.to_owned()),
        menge: p.quantity,
        einheit: p.unit.clone(),
        satz_ct,
        betrag_eur,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use energy_billing::PositionCategory;
    use rust_decimal::dec;

    fn levy(tags: &[&str], kwh: Decimal, eur_per_kwh: Decimal, basis: &str) -> BillingPosition {
        let mut p =
            BillingPosition::debit("Steuer", kwh, "kWh", eur_per_kwh, PositionCategory::Levy)
                .with_legal_basis(basis.to_owned());
        for t in tags {
            p = p.with_tag(*t);
        }
        p
    }

    /// A Regelsatz line and an Ermäßigung line are two lines, each with its
    /// own ground and rate; a non-tax position is none.
    #[test]
    fn each_ground_is_its_own_line() {
        let positions = [
            levy(
                &["levy", "stromsteuer"],
                dec!(1000),
                dec!(0.0205),
                "§ 3 StromStG",
            ),
            levy(
                &["levy", "stromsteuer", "stromsteuer_ermaessigt"],
                dec!(200),
                dec!(0.01142),
                "§ 9 Abs. 2 Nr. 1 StromStG",
            ),
            levy(&["nne"], dec!(1200), dec!(0.08), "StromNEV"),
        ];
        let zeilen: Vec<Steuerzeile> = positions.iter().filter_map(zeile).collect();
        assert_eq!(zeilen.len(), 2, "{zeilen:?}");
        assert_eq!(
            (zeilen[0].art, zeilen[0].satz_ct, zeilen[0].betrag_eur),
            (Art::Regelsatz, dec!(2.0500), dec!(20.50))
        );
        assert_eq!(zeilen[1].art, Art::Ermaessigung);
        assert_eq!(zeilen[1].rechtsgrundlage, "§ 9 Abs. 2 Nr. 1 StromStG");
        assert_eq!(zeilen[1].menge, dec!(200));
    }

    /// A Befreiung is a line with its quantity and no amount.
    #[test]
    fn an_exemption_is_a_line_without_an_amount() {
        let mut p = levy(
            &["stromsteuer_befreiung"],
            dec!(500),
            Decimal::ZERO,
            "§ 9 Abs. 1 Nr. 1 StromStG",
        );
        p.category = PositionCategory::Info;
        let z = zeile(&p).expect("a line");
        assert_eq!((z.steuer, z.art), (Steuer::Stromsteuer, Art::Befreiung));
        assert_eq!((z.menge, z.betrag_eur), (dec!(500), Decimal::ZERO));
    }
}
