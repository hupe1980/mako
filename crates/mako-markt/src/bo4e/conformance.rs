//! The BO4E rules `rubo4e`'s own `.validate()` does not check.
//!
//! BO4E's machine-readable schema constrains almost nothing: of the 35
//! Geschäftsobjekte at v202607.1.0, exactly **two** declare a `required` field
//! and **none** declares a `oneOf`, `anyOf` or `not`. `Marktlokation.json`
//! accepts `{}`. Every rule the standard has lives in prose, so someone has to
//! run it. `rubo4e` runs most; these two are the rest:
//!
//! | Rule | Source |
//! |---|---|
//! | `rechnung.gesamtnetto` | `gesamtnetto`: „Die Summe der Nettobeträge der Rechnungsteile" |
//! | `rechnung.storno` | `istStorno`: „im Falle 'true' findet sich im Attribut 'originalrechnungsnummer' die Nummer der Originalrechnung" |
//!
//! A rule earns a place here **only if BO4E asserts it**, quoting the sentence.
//! Requirements of mako's own live in the per-endpoint profile.
//!
//! # Inbound and outbound are not the same bar
//!
//! [`Bo4eConformance::residual_rules`] runs on everything, in or out: they are
//! BO4E's rules, and a conformant counterparty must not be refused for missing
//! something the standard does not ask for.
//!
//! [`Bo4eConformance::emission_rules`] runs **only on what mako sends**. BO4E
//! marks none of the three invoice totals required, so an invoice stating just
//! a gross total must be accepted — but mako emits all three, and pinning that
//! costs nothing. `rubo4e` keeps that rule in `validation::current::quality`,
//! unwired from `.validate()` for the same reason, and mako calls it by name on
//! the outbound path.

use rubo4e::validation::ValidationFailure;
use rust_decimal::Decimal;
use rust_decimal::RoundingStrategy;

/// Build a failure at `path`.
fn failure(path: &str, message: impl Into<String>) -> ValidationFailure {
    ValidationFailure {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// The BO4E-stated rules `rubo4e`'s own `.validate()` does not cover.
///
/// The default is empty, which is the answer for every type but `Rechnung`.
/// Listed explicitly rather than blanket-implemented: a blanket
/// `impl<T> Bo4eConformance for T` would silently absorb a type that *does*
/// gain a rule, which is the failure this module exists to prevent.
pub trait Bo4eConformance {
    /// Rules BO4E states about this type that `rubo4e` does not check.
    ///
    /// Returns every failure rather than the first, so a caller sees the whole
    /// picture in one `422` — the same shape
    /// [`rubo4e::validation::report_errors`] produces for the derived rules.
    fn residual_rules(&self) -> Vec<ValidationFailure> {
        Vec::new()
    }

    /// Rules mako holds itself to on the way **out**, which BO4E does not state.
    ///
    /// Never applied to a received document: refusing a counterparty for
    /// missing something the standard does not ask for is exactly the error
    /// this crate's rule bar exists to prevent. On mako's own output the
    /// calculation is different — mako controls the document, and a
    /// merely-sensible rule costs nothing to satisfy.
    fn emission_rules(&self) -> Vec<ValidationFailure> {
        Vec::new()
    }
}

/// Types with no residual rule — everything `rubo4e`'s validators fully cover.
macro_rules! no_residual_rules {
    ($($t:ty),* $(,)?) => { $(
        impl Bo4eConformance for $t {}
    )* };
}

no_residual_rules![
    rubo4e::current::Angebot,
    rubo4e::current::Bilanzierung,
    rubo4e::current::Energiemenge,
    rubo4e::current::Energiemix,
    rubo4e::current::Fremdkosten,
    rubo4e::current::Geraet,
    rubo4e::current::Geschaeftspartner,
    rubo4e::current::Kosten,
    rubo4e::current::Lastgang,
    rubo4e::current::LastvariablePreisposition,
    rubo4e::current::Marktlokation,
    rubo4e::current::Messlokation,
    rubo4e::current::Netzlokation,
    rubo4e::current::Person,
    rubo4e::current::Preisgarantie,
    rubo4e::current::PreisblattMessung,
    rubo4e::current::PreisblattNetznutzung,
    rubo4e::current::Standorteigenschaften,
    rubo4e::current::SteuerbareRessource,
    rubo4e::current::Tarifinfo,
    rubo4e::current::Tarifpreisblatt,
    rubo4e::current::TechnischeRessource,
    rubo4e::current::Vertrag,
    rubo4e::current::Vorauszahlung,
    rubo4e::current::Zaehler,
    rubo4e::current::Zahlungsinformation,
    rubo4e::current::Zaehlzeitdefinition,
    rubo4e::current::Zeitreihe,
    rubo4e::current::ZeitvariablePreisposition,
];

/// Do two amounts agree at the scale the total is stated in?
///
/// Amounts are compared at the scale of the **stated total**, with the cent as
/// the floor: a position vector carried at six decimals sums to a figure whose
/// last four digits no invoice states, so demanding exact equality would reject
/// every document in circulation. Two decimals is the unit invoices settle in.
fn agrees(computed: Decimal, stated: Decimal) -> bool {
    let scale = stated.scale().max(2);
    // Kaufmännisch, not `round_dp`'s banker's rounding: the producer states
    // 12.35 for a raw 12.345, and rounding the recomputed figure half-to-even
    // would answer 12.34 and refuse a document whose own arithmetic is right.
    let kfm = |d: Decimal| d.round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero);
    kfm(computed) == kfm(stated)
}

impl Bo4eConformance for rubo4e::current::Rechnung {
    /// **`rechnung.gesamtnetto`** — the positions sum to the net total.
    ///
    /// > `gesamtnetto`: „Die Summe der Nettobeträge der Rechnungsteile."
    ///
    /// Skipped when the document states a `rabattNetto` (a discount the
    /// positions do not carry), summarises `teilrechnungen` (whose positions
    /// live on the child invoices), or has a position that states no
    /// `gesamtpreis` — BO4E makes that field optional, and a position claiming
    /// no amount makes the sum *unknowable*, not smaller.
    ///
    /// **`rechnung.storno`** — a reversal names what it reverses.
    ///
    /// > `istStorno`: „im Falle 'true' findet sich im Attribut
    /// > 'originalrechnungsnummer' die Nummer der Originalrechnung."
    ///
    /// `rubo4e` covers the rest, including everything nested below.
    ///
    /// **`zuZahlen` is not checked**: its description reads „(gesamtbrutto -
    /// vorausbezahlt - rabattBrutto)" and v202607 ships no `rabattBrutto` —
    /// only `rabattNetto`, a net discount that cannot be subtracted from a
    /// gross total. The equation is not reconstructible from the payload.
    fn residual_rules(&self) -> Vec<ValidationFailure> {
        let mut out = Vec::new();

        // ── rechnung.gesamtnetto ─────────────────────────────────────────────
        if let (Some(stated), Some(positions)) = (
            self.gesamtnetto.as_ref().and_then(|b| b.wert),
            self.rechnungspositionen.as_ref(),
        ) {
            let summarises_children = self.teilrechnungen.as_ref().is_some_and(|t| !t.is_empty());
            // `collect::<Option<Vec<_>>>()`: one position with no stated amount
            // makes the whole sum unknowable, so the rule suspends rather than
            // comparing a partial sum against the stated whole.
            let amounts: Option<Vec<Decimal>> = positions
                .iter()
                .map(|p| p.gesamtpreis.as_ref().and_then(|b| b.wert))
                .collect();

            if let Some(amounts) = amounts
                && !amounts.is_empty()
                && self.rabatt_netto.is_none()
                && !summarises_children
            {
                // `checked_add`, not `sum`: `Decimal`'s `Add` panics on
                // overflow and every figure here arrives in a payload a
                // counterparty sent. A sum that cannot be represented is not
                // one this rule can contradict, so it suspends.
                if let Some(sum) = amounts
                    .iter()
                    .copied()
                    .try_fold(Decimal::ZERO, Decimal::checked_add)
                    && !agrees(sum, stated)
                {
                    out.push(failure(
                        "gesamtnetto",
                        format!(
                            "the {} position{} sum to {sum}, not the stated \
                             gesamtnetto ({stated}) — BO4E: \"Die Summe der \
                             Nettobeträge der Rechnungsteile\"",
                            positions.len(),
                            if positions.len() == 1 { "" } else { "s" }
                        ),
                    ));
                }
            }
        }

        // ── rechnung.storno ──────────────────────────────────────────────────
        if self.ist_storno == Some(true) && self.original_rechnungsnummer.is_none() {
            out.push(failure(
                "originalRechnungsnummer",
                "istStorno is true but originalRechnungsnummer is absent; a \
                 reversal that does not name the invoice it reverses is one no \
                 receiver can book",
            ));
        }

        out
    }

    /// `rubo4e`'s `quality::rechnung_totals_are_complete`: state all three
    /// totals or none.
    ///
    /// `gesamtbrutto = gesamtnetto + gesamtsteuer`, so any two determine the
    /// third and stating exactly two makes the reader do arithmetic the sender
    /// already did. BO4E marks none of them required, which is why this is an
    /// emission rule and not a conformance one — and why `rubo4e` keeps it out
    /// of `.validate()`.
    fn emission_rules(&self) -> Vec<ValidationFailure> {
        rubo4e::validation::current::quality::rechnung_totals_are_complete(self)
            .err()
            .map(|e| failure("gesamtbrutto", e.to_string()))
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Bo4eConformance as _;
    use rubo4e::current::{Betrag, Rechnung, Rechnungsposition, Steuerbetrag, Waehrungscode};
    use rust_decimal::{Decimal, dec};

    fn eur(wert: Decimal) -> Betrag {
        Betrag {
            wert: Some(wert),
            waehrung: Some(Waehrungscode::Eur),
            ..Default::default()
        }
    }

    fn balanced_invoice() -> Rechnung {
        Rechnung {
            gesamtnetto: Some(eur(dec!(300.00))),
            gesamtsteuer: Some(eur(dec!(57.00))),
            gesamtbrutto: Some(eur(dec!(357.00))),
            steuerbetraege: Some(vec![Steuerbetrag {
                steuerwert: Some(dec!(57.00)),
                ..Default::default()
            }]),
            rechnungspositionen: Some(vec![Rechnungsposition {
                gesamtpreis: Some(eur(dec!(300.00))),
                ..Default::default()
            }]),
            ..Default::default()
        }
    }

    #[test]
    fn a_balanced_invoice_has_no_residual_failure() {
        assert!(balanced_invoice().residual_rules().is_empty());
    }

    #[test]
    fn positions_that_miss_the_net_total_are_reported() {
        let mut r = balanced_invoice();
        r.rechnungspositionen = Some(vec![Rechnungsposition {
            gesamtpreis: Some(eur(dec!(299.00))),
            ..Default::default()
        }]);
        let f = r.residual_rules();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "gesamtnetto");
    }

    /// A discount the positions do not carry breaks the identity by design.
    #[test]
    fn a_rabatt_suspends_the_position_sum() {
        let mut r = balanced_invoice();
        r.rabatt_netto = Some(eur(dec!(10.00)));
        r.rechnungspositionen = Some(vec![Rechnungsposition {
            gesamtpreis: Some(eur(dec!(310.00))),
            ..Default::default()
        }]);
        assert!(r.residual_rules().is_empty());
    }

    /// A position stating no amount makes the sum unknowable, not smaller.
    #[test]
    fn an_unpriced_position_suspends_the_sum() {
        let mut r = balanced_invoice();
        r.rechnungspositionen = Some(vec![
            Rechnungsposition {
                gesamtpreis: Some(eur(dec!(300.00))),
                ..Default::default()
            },
            Rechnungsposition::default(),
        ]);
        assert!(r.residual_rules().is_empty());
    }

    /// Positions carried at six decimals sum to a figure no invoice states; the
    /// comparison happens at the scale of the total.
    #[test]
    fn position_rounding_does_not_break_the_sum() {
        let mut r = balanced_invoice();
        r.rechnungspositionen = Some(vec![
            Rechnungsposition {
                gesamtpreis: Some(eur(dec!(100.004999))),
                ..Default::default()
            },
            Rechnungsposition {
                gesamtpreis: Some(eur(dec!(199.995001))),
                ..Default::default()
            },
        ]);
        assert!(r.residual_rules().is_empty());
    }

    /// `Decimal`'s `Add` panics on overflow, and these figures are untrusted.
    #[test]
    fn an_overflowing_position_sum_does_not_panic() {
        let mut r = balanced_invoice();
        r.rechnungspositionen = Some(vec![
            Rechnungsposition {
                gesamtpreis: Some(eur(Decimal::MAX)),
                ..Default::default()
            },
            Rechnungsposition {
                gesamtpreis: Some(eur(Decimal::MAX)),
                ..Default::default()
            },
        ]);
        assert!(r.residual_rules().is_empty());
    }

    #[test]
    fn a_reversal_must_name_the_invoice_it_reverses() {
        let mut r = balanced_invoice();
        r.ist_storno = Some(true);
        let f = r.residual_rules();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "originalRechnungsnummer");

        r.original_rechnungsnummer = Some("2026-000123".to_owned());
        assert!(r.residual_rules().is_empty());
    }

    /// Both residual rules report together rather than short-circuiting.
    #[test]
    fn every_residual_failure_is_reported_at_once() {
        let mut r = balanced_invoice();
        r.ist_storno = Some(true);
        r.rechnungspositionen = Some(vec![Rechnungsposition {
            gesamtpreis: Some(eur(dec!(299.00))),
            ..Default::default()
        }]);
        assert_eq!(r.residual_rules().len(), 2);
    }

    /// The rules `rubo4e` owns are not restated here. This pins the division of
    /// labour, so a release that drops one is caught rather than silently
    /// leaving a gap mako believes is covered.
    #[test]
    fn rubo4e_owns_the_rules_this_module_no_longer_states() {
        use rubo4e::prelude::Validate as _;

        // gesamtnetto + gesamtsteuer != gesamtbrutto
        let mut r = balanced_invoice();
        r.gesamtbrutto = Some(eur(dec!(358.00)));
        assert!(r.validate().is_err(), "rubo4e checks gesamtbrutto");
        assert!(
            r.residual_rules().is_empty(),
            "and mako does not restate it"
        );

        // steuerbetraege do not sum to gesamtsteuer
        let mut r = balanced_invoice();
        r.steuerbetraege = Some(vec![Steuerbetrag {
            steuerwert: Some(dec!(56.00)),
            ..Default::default()
        }]);
        assert!(r.validate().is_err(), "rubo4e checks the tax breakdown");

        // mixed currency
        let mut r = balanced_invoice();
        r.gesamtbrutto = Some(Betrag {
            wert: Some(dec!(357.00)),
            waehrung: Some(Waehrungscode::Chf),
            ..Default::default()
        });
        assert!(r.validate().is_err(), "rubo4e checks currency agreement");
    }

    /// A location with no Ortsangabe is conformant — the case `rubo4e` rejected
    /// before 0.11, and the shape mako emits most often.
    #[test]
    fn a_location_reference_carries_no_ortsangabe() {
        use rubo4e::prelude::Validate as _;
        assert!(rubo4e::current::Marktlokation::default().validate().is_ok());
        assert!(rubo4e::current::Messlokation::default().validate().is_ok());
    }

    /// `.validate()` descends since 0.11, so a violation two levels down is
    /// caught without mako recursing by hand.
    #[test]
    fn rubo4e_descends_into_nested_values() {
        use rubo4e::prelude::Validate as _;
        let r = Rechnung {
            rechnungspositionen: Some(vec![Rechnungsposition {
                lieferungszeitraum: Some(rubo4e::current::Zeitraum {
                    startdatum: Some(time::macros::date!(2026 - 02 - 01)),
                    enddatum: Some(time::macros::date!(2026 - 01 - 01)),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        assert!(
            r.validate().is_err(),
            "an inverted Zeitraum on a position is reported by rubo4e's own dive"
        );
    }

    /// The positions sum to an exact half-cent and the document states the
    /// kaufmännisch total. Comparing with `Decimal::round_dp` would round the
    /// recomputed sum half-to-even, answer 12.34 against a stated 12.35, and
    /// refuse a document whose own arithmetic is right.
    #[test]
    fn a_half_cent_sum_agrees_with_the_kaufmaennisch_total() {
        let r = Rechnung {
            gesamtnetto: Some(eur(dec!(12.35))),
            rechnungspositionen: Some(vec![
                Rechnungsposition {
                    gesamtpreis: Some(eur(dec!(6.1725))),
                    ..Default::default()
                },
                Rechnungsposition {
                    gesamtpreis: Some(eur(dec!(6.1725))),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        assert!(r.residual_rules().is_empty(), "{:?}", r.residual_rules());
    }

    /// The same guard still catches a total that is actually wrong.
    #[test]
    fn a_total_off_by_a_cent_is_still_refused() {
        let r = Rechnung {
            gesamtnetto: Some(eur(dec!(12.36))),
            rechnungspositionen: Some(vec![Rechnungsposition {
                gesamtpreis: Some(eur(dec!(12.345))),
                ..Default::default()
            }]),
            ..Default::default()
        };
        assert_eq!(r.residual_rules().len(), 1);
    }
}
