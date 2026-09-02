//! Umsatzsteuer on grid settlements.
//!
//! A settlement that states only its net amount is not an invoice. §14 Abs. 4
//! Nr. 8 UStG requires "den anzuwendenden Steuersatz sowie den auf das Entgelt
//! entfallenden Steuerbetrag oder … einen Hinweis darauf, dass eine
//! Steuerbefreiung gilt", and without it the recipient has no Vorsteuerabzug.
//!
//! # Two kinds of supply, taxed differently
//!
//! Everything this crate settles falls into one of two boxes, and which box
//! decides both the rate and who owes the tax:
//!
//! - **Netznutzung, Messstellenbetrieb, abrechnungswürdige Handlungen** are
//!   *sonstige Leistungen*. UStAE 13b.3a excludes them from §13b by name — the
//!   provision reaches the energy itself, not the provision and maintenance of
//!   the network — so the issuer always owes the tax at the Regelsteuersatz.
//! - **Mehr-/Mindermengen are a Lieferung**, of electricity or of gas through
//!   the Erdgasnetz. That brings §13b Abs. 2 Nr. 5 Buchst. b into play, and with
//!   it the reverse charge.
//!
//! # The §13b condition is asymmetric between the Sparten
//!
//! §13b Abs. 5 states it twice, differently, and the difference is not a
//! drafting accident:
//!
//! - **Elektrizität** — the recipient owes the tax where "der liefernde
//!   Unternehmer *und* der Leistungsempfänger Wiederverkäufer von Elektrizität
//!   im Sinne des § 3g sind". **Both** parties.
//! - **Gas über das Erdgasnetz** — the recipient owes it "wenn er ein
//!   Wiederverkäufer von Erdgas im Sinne des § 3g ist". The **recipient** alone.
//!
//! Status is evidenced by a valid *USt 1 TH* (UStAE 13b.3a); absent it, the
//! supply is taxed normally. Getting this backwards is not a rounding error:
//! tax shown on a reverse-charge invoice is owed under §14c Abs. 1 UStG *and*
//! gives the recipient no Vorsteuerabzug, because the recipient still owes it
//! under §13b.

use crate::rounding::RoundMoney;
use rust_decimal::{Decimal, dec};

use crate::error::BillingError;

pub use ::billing::TaxCategory;

/// What is being supplied, for tax purposes.
///
/// Not the same axis as [`crate::Sparte`]: a Netznutzung Gas settlement and a
/// Mehrmengen Gas settlement are both "gas", and are taxed under different
/// rules because one is a service and the other is a supply of the commodity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Leistungsart {
    /// A service — Netznutzung, Messstellenbetrieb, AWH.
    ///
    /// Never reverse-charged (UStAE 13b.3a), and never covered by the gas and
    /// Fernwärme rate reduction, which reached *Lieferungen* only.
    SonstigeLeistung,
    /// A supply of electricity (§3g Abs. 1 Satz 1 UStG).
    LieferungStrom,
    /// A supply of gas through the Erdgasnetz (§3g Abs. 1 Satz 1 UStG).
    LieferungGas,
}

/// Who holds §3g Wiederverkäufer status, as evidenced by a *USt 1 TH*.
///
/// Two fields rather than one because the statute asks two different questions
/// depending on the Sparte, and a single "reverse charge: yes/no" flag would put
/// that judgement in the caller — which is where it was getting made wrongly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Wiederverkaeuferstatus {
    /// The issuing party holds it. Relevant for electricity only.
    pub leistender: bool,
    /// The billed party holds it. Relevant for both Sparten.
    pub empfaenger: bool,
}

impl Wiederverkaeuferstatus {
    /// Neither party holds §3g status — the ordinary case, taxed normally.
    pub const KEINER: Self = Self {
        leistender: false,
        empfaenger: false,
    };

    /// Both parties hold it, which is what an electricity supply needs.
    pub const BEIDE: Self = Self {
        leistender: true,
        empfaenger: true,
    };

    /// Whether §13b Abs. 2 Nr. 5 Buchst. b shifts the liability for this supply.
    #[must_use]
    pub const fn verlagert(self, art: Leistungsart) -> bool {
        match art {
            // UStAE 13b.3a: the provision reaches the energy, not the network.
            Leistungsart::SonstigeLeistung => false,
            Leistungsart::LieferungStrom => self.leistender && self.empfaenger,
            Leistungsart::LieferungGas => self.empfaenger,
        }
    }
}

/// The §14a Abs. 5 Satz 2 UStG wording a reverse-charge invoice must carry.
pub const HINWEIS_REVERSE_CHARGE: &str = "Steuerschuldnerschaft des Leistungsempfängers";

/// The Regelsteuersatz, as a percentage.
pub const REGELSTEUERSATZ: Decimal = dec!(19);

/// The Umsatzsteuer stated on a settlement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Steuerausweis {
    /// UNCL 5305 category — `S` for a taxed supply, `AE` for a reverse charge.
    pub kategorie: TaxCategory,
    /// The rate in percent. Zero under a reverse charge.
    pub satz_prozent: Decimal,
    /// The net amount the rate applies to.
    pub bemessungsgrundlage_eur: Decimal,
    /// The tax itself, rounded commercially to the cent.
    pub steuer_eur: Decimal,
    /// The note the invoice must carry, where one is required.
    pub hinweis: Option<&'static str>,
    /// The paragraph this treatment rests on, for the audit trail.
    pub rechtsgrundlage: &'static str,
}

impl Steuerausweis {
    /// The gross amount — net plus tax.
    #[must_use]
    pub fn brutto_eur(&self) -> Decimal {
        self.bemessungsgrundlage_eur + self.steuer_eur
    }
}

/// The Umsatzsteuer rate in force for a supply over a delivery period.
///
/// Returns `None` when the period **straddles a rate change**: no single rate is
/// right for such a period, and picking one would misbill part of it. The caller
/// splits the period at the Stichtag and settles each part.
///
/// The departures from 19 % this crate can meet:
///
/// | Window | Rate | Applies to | Basis |
/// |---|---|---|---|
/// | 01.07.2020 – 31.12.2020 | 16 % | every supply | §28 Abs. 1–3 UStG a. F. |
/// | 01.10.2022 – 31.03.2024 | 7 % | gas through the Erdgasnetz | §28 Abs. 5 UStG |
///
/// The 7 % window reached the **Lieferung von Gas**, not the operation of the
/// network: a Netznutzung Gas invoice for that period is 19 %, and a Gas
/// Mehrmengen invoice for the same period is 7 %.
#[must_use]
pub fn regelsatz_prozent(art: Leistungsart, from: time::Date, to: time::Date) -> Option<Decimal> {
    /// `(von, bis, satz)`, newest first; a period inside one takes its rate.
    type Fenster = (time::Date, time::Date, Decimal);

    const COVID: Fenster = (
        time::macros::date!(2020 - 07 - 01),
        time::macros::date!(2020 - 12 - 31),
        dec!(16),
    );
    const GAS: Fenster = (
        time::macros::date!(2022 - 10 - 01),
        time::macros::date!(2024 - 03 - 31),
        dec!(7),
    );

    let fenster: &[Fenster] = match art {
        Leistungsart::LieferungGas => &[COVID, GAS],
        Leistungsart::SonstigeLeistung | Leistungsart::LieferungStrom => &[COVID],
    };

    for (von, bis, satz) in fenster {
        if from >= *von && to <= *bis {
            return Some(*satz);
        }
        if from <= *bis && to >= *von {
            // Overlaps without being contained — the period spans a Stichtag.
            return None;
        }
    }
    Some(REGELSTEUERSATZ)
}

/// State the Umsatzsteuer on a net amount.
///
/// # Errors
///
/// Returns [`BillingError::InvalidInput`] when the delivery period straddles a
/// rate change, because no single rate describes it.
pub fn steuerausweis(
    netto_eur: Decimal,
    art: Leistungsart,
    status: Wiederverkaeuferstatus,
    period: crate::SettlementPeriod,
) -> Result<Steuerausweis, BillingError> {
    if status.verlagert(art) {
        return Ok(Steuerausweis {
            kategorie: TaxCategory::ReverseCharge,
            satz_prozent: Decimal::ZERO,
            bemessungsgrundlage_eur: netto_eur,
            // BR-AE-09: a reverse-charge invoice states no tax amount. Showing
            // one anyway is owed under §14c Abs. 1 and still not deductible.
            steuer_eur: Decimal::ZERO,
            hinweis: Some(HINWEIS_REVERSE_CHARGE),
            rechtsgrundlage: match art {
                Leistungsart::LieferungStrom | Leistungsart::LieferungGas => {
                    "§13b Abs. 2 Nr. 5 Buchst. b UStG"
                }
                Leistungsart::SonstigeLeistung => unreachable!("never shifted"),
            },
        });
    }

    let satz = regelsatz_prozent(art, period.from(), period.to()).ok_or_else(|| {
        BillingError::InvalidInput {
            reason: format!(
                "the delivery period {} – {} straddles an Umsatzsteuer rate change; split it \
                 at the Stichtag and settle each part, rather than billing the whole period \
                 at one rate",
                period.from(),
                period.to()
            ),
        }
    })?;

    Ok(Steuerausweis {
        kategorie: TaxCategory::Standard,
        satz_prozent: satz,
        bemessungsgrundlage_eur: netto_eur,
        // Kaufmännisch to the cent: the tax is a monetary amount on the
        // invoice, not an intermediate.
        steuer_eur: (netto_eur * satz / dec!(100))
            .round_kfm(2),
        hinweis: None,
        rechtsgrundlage: "§12 Abs. 1 UStG",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SettlementPeriod;
    use time::macros::date;

    fn period(from: time::Date, to: time::Date) -> SettlementPeriod {
        SettlementPeriod::new(from, to).expect("ordered")
    }

    /// Network services are never reverse-charged, whatever either party is.
    ///
    /// UStAE 13b.3a excludes them by name: the provision reaches the energy, not
    /// the provision and maintenance of the network.
    #[test]
    fn a_network_service_is_never_reverse_charged() {
        for status in [
            Wiederverkaeuferstatus::KEINER,
            Wiederverkaeuferstatus::BEIDE,
        ] {
            assert!(!status.verlagert(Leistungsart::SonstigeLeistung));
        }
    }

    /// Electricity needs **both** parties; gas needs the recipient alone.
    ///
    /// §13b Abs. 5 states the condition twice, differently. Collapsing the two
    /// into one flag is how an invoice ends up reverse-charged that should not
    /// have been — tax owed under §14c and no Vorsteuerabzug for the recipient.
    #[test]
    fn the_sect13b_condition_is_asymmetric_between_the_sparten() {
        let nur_empfaenger = Wiederverkaeuferstatus {
            leistender: false,
            empfaenger: true,
        };
        let nur_leistender = Wiederverkaeuferstatus {
            leistender: true,
            empfaenger: false,
        };

        // Strom: both, or nothing.
        assert!(!nur_empfaenger.verlagert(Leistungsart::LieferungStrom));
        assert!(!nur_leistender.verlagert(Leistungsart::LieferungStrom));
        assert!(Wiederverkaeuferstatus::BEIDE.verlagert(Leistungsart::LieferungStrom));

        // Gas: the recipient alone decides it.
        assert!(nur_empfaenger.verlagert(Leistungsart::LieferungGas));
        assert!(!nur_leistender.verlagert(Leistungsart::LieferungGas));
        assert!(Wiederverkaeuferstatus::BEIDE.verlagert(Leistungsart::LieferungGas));
    }

    /// A reverse-charge invoice states no tax and carries the §14a wording.
    #[test]
    fn a_reverse_charge_states_no_tax_and_says_why() {
        let s = steuerausweis(
            dec!(1000),
            Leistungsart::LieferungStrom,
            Wiederverkaeuferstatus::BEIDE,
            period(date!(2026 - 01 - 01), date!(2026 - 01 - 31)),
        )
        .expect("computable");
        assert_eq!(s.kategorie, TaxCategory::ReverseCharge);
        assert_eq!(s.steuer_eur, Decimal::ZERO);
        assert_eq!(s.satz_prozent, Decimal::ZERO);
        assert_eq!(s.hinweis, Some(HINWEIS_REVERSE_CHARGE));
        assert_eq!(s.brutto_eur(), dec!(1000));
    }

    /// The ordinary case: 19 %, stated, and added to the gross.
    #[test]
    fn an_ordinary_supply_is_taxed_at_the_regelsteuersatz() {
        let s = steuerausweis(
            dec!(1234.56),
            Leistungsart::SonstigeLeistung,
            Wiederverkaeuferstatus::KEINER,
            period(date!(2026 - 01 - 01), date!(2026 - 01 - 31)),
        )
        .expect("computable");
        assert_eq!(s.kategorie, TaxCategory::Standard);
        assert_eq!(s.satz_prozent, dec!(19));
        assert_eq!(s.steuer_eur, dec!(234.57), "1234.56 × 19 % = 234.5664");
        assert_eq!(s.brutto_eur(), dec!(1469.13));
        assert_eq!(s.hinweis, None);
    }

    /// The gas reduction reached the commodity, not the network.
    ///
    /// A Netznutzung Gas invoice for a period inside the window is 19 %; a Gas
    /// Mehrmengen invoice for the same period is 7 %.
    #[test]
    fn the_gas_reduction_reached_the_commodity_not_the_network() {
        let im_fenster = (date!(2023 - 01 - 01), date!(2023 - 01 - 31));
        assert_eq!(
            regelsatz_prozent(Leistungsart::LieferungGas, im_fenster.0, im_fenster.1),
            Some(dec!(7))
        );
        assert_eq!(
            regelsatz_prozent(Leistungsart::SonstigeLeistung, im_fenster.0, im_fenster.1),
            Some(dec!(19)),
            "Netznutzung Gas is a service — §28 Abs. 5 UStG reduced Lieferungen"
        );
        assert_eq!(
            regelsatz_prozent(Leistungsart::LieferungStrom, im_fenster.0, im_fenster.1),
            Some(dec!(19)),
            "the reduction was for gas and Fernwärme, never for electricity"
        );
    }

    /// The 2020 reduction reached everything, services included.
    #[test]
    fn the_covid_reduction_reached_every_supply() {
        let im_fenster = (date!(2020 - 08 - 01), date!(2020 - 08 - 31));
        for art in [
            Leistungsart::SonstigeLeistung,
            Leistungsart::LieferungStrom,
            Leistungsart::LieferungGas,
        ] {
            assert_eq!(
                regelsatz_prozent(art, im_fenster.0, im_fenster.1),
                Some(dec!(16))
            );
        }
    }

    /// A period spanning a Stichtag has no single rate, and is refused.
    ///
    /// Billing it at either rate misstates one part of it, and the misstatement
    /// is invisible: the invoice adds up.
    #[test]
    fn a_period_spanning_a_rate_change_is_refused() {
        // The gas reduction ended 31.03.2024; this period runs across it.
        assert_eq!(
            regelsatz_prozent(
                Leistungsart::LieferungGas,
                date!(2024 - 03 - 01),
                date!(2024 - 04 - 30)
            ),
            None
        );
        let refused = steuerausweis(
            dec!(1000),
            Leistungsart::LieferungGas,
            Wiederverkaeuferstatus::KEINER,
            period(date!(2024 - 03 - 01), date!(2024 - 04 - 30)),
        );
        assert!(refused.is_err(), "{refused:?}");

        // Either side of it, cleanly.
        assert_eq!(
            regelsatz_prozent(
                Leistungsart::LieferungGas,
                date!(2024 - 03 - 01),
                date!(2024 - 03 - 31)
            ),
            Some(dec!(7))
        );
        assert_eq!(
            regelsatz_prozent(
                Leistungsart::LieferungGas,
                date!(2024 - 04 - 01),
                date!(2024 - 04 - 30)
            ),
            Some(dec!(19))
        );
    }

    /// A credit is taxed like the charge it reverses — the sign carries through.
    #[test]
    fn a_credit_carries_its_tax_with_the_same_sign() {
        let s = steuerausweis(
            dec!(-500),
            Leistungsart::LieferungStrom,
            Wiederverkaeuferstatus::KEINER,
            period(date!(2026 - 01 - 01), date!(2026 - 01 - 31)),
        )
        .expect("computable");
        assert_eq!(s.steuer_eur, dec!(-95.00));
        assert_eq!(s.brutto_eur(), dec!(-595.00));
    }
}
