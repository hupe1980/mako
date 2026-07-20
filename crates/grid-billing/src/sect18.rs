//! Entgelte für dezentrale Erzeugung — §18 StromNEV, under Abschmelzung.
//!
//! §18 StromNEV pays the operator of a decentral generating plant for the
//! upstream network costs its feed-in avoids. The payment is being phased out by
//! Festlegung **GBK-25-02-1#1** (Große Beschlusskammer Energie, 17.02.2026):
//! the underlying vermiedene Kosten are cut in three steps and the payments end
//! entirely with 2028.
//!
//! ## The schedule, from the Tenor
//!
//! Tenorziffer 2 Satz 2 cuts the costs "in drei Stufen um a) 50 vom Hundert
//! beginnend am 01. Juli 2026, b) 50 von Hundert beginnend am 01. Januar 2027
//! c) und 75 vom Hundert beginnend am 01. Januar 2028"; Satz 3 states the
//! effect: "Die Kürzungen entsprechen einer jährlichen Abschmelzung von 25 %."
//!
//! | Period | Remaining factor | Annual average |
//! |---|---|---|
//! | to 30.06.2026 | 1.00 | 2026: 0.75 |
//! | 01.07.2026 – 31.12.2026 | 0.50 | |
//! | 2027 | 0.50 | 0.50 |
//! | 2028 | 0.25 | 0.25 |
//! | from 2029 | 0.00 | — |
//!
//! The two 50 % steps are not cumulative: the 2027 cut restates the same level
//! the July 2026 cut reached, which is what makes the *annual averages* fall by
//! 25 percentage points a year.
//!
//! ## EEG plants are excluded
//!
//! §18 Abs. 1 Satz 4 Nr. 1 StromNEV: a plant funded under the EEG receives no
//! Entgelt für dezentrale Erzeugung. Settling one is refused as an error, not
//! warned — the payment would be unlawful, and unlike a ceiling breach there is
//! no legitimate reading under which it goes out anyway.

use rust_decimal::Decimal;
use rust_decimal::dec;
use time::Date;

use crate::error::BillingError;
use crate::types::{
    BillingPositionKind, CalculationTrace, LegalReference, SettlementPeriod, SettlementPosition,
    SettlementResult, SettlementStatus, SettlementType, Sparte, TariffSource,
};

/// First day of the first cut (Tenorziffer 2 Satz 2 lit. a).
const STUFE_A: Date = time::macros::date!(2026 - 07 - 01);
/// First day of the second cut (lit. b) — restates the 50 % level for 2027.
const STUFE_B: Date = time::macros::date!(2027 - 01 - 01);
/// First day of the third cut (lit. c).
const STUFE_C: Date = time::macros::date!(2028 - 01 - 01);
/// First day with no payment at all.
const ENDE: Date = time::macros::date!(2029 - 01 - 01);

/// The fraction of the vermiedene Kosten still payable on a given day.
///
/// GBK-25-02-1#1 Tenorziffer 2. The factor is a property of the *day the energy
/// was fed in*, so a settlement over a period that crosses a step must split the
/// period — which [`settle_dezentrale_einspeisung`] enforces rather than
/// averaging across the step.
#[must_use]
pub fn abschmelzfaktor(tag: Date) -> Decimal {
    if tag >= ENDE {
        Decimal::ZERO
    } else if tag >= STUFE_C {
        dec!(0.25)
    } else if tag >= STUFE_B || tag >= STUFE_A {
        dec!(0.50)
    } else {
        Decimal::ONE
    }
}

/// `true` when the factor changes inside the period.
#[must_use]
pub fn period_crosses_a_step(period: SettlementPeriod) -> bool {
    abschmelzfaktor(period.from()) != abschmelzfaktor(period.to())
}

/// Input for a §18 settlement — the DSO's payment to a decentral generator.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DezentraleEinspeisungInput {
    /// The generating plant's metering location.
    pub malo_id: String,
    /// The paying Netzbetreiber.
    pub nb_mp_id: String,
    /// The plant operator being paid.
    pub anlagenbetreiber_mp_id: String,
    /// The delivery period.
    pub period: SettlementPeriod,
    /// Energy fed in during the period, in kWh.
    pub einspeisung_kwh: Decimal,
    /// The vermiedene Kosten of the upstream level, in ct/kWh, **before** the
    /// Abschmelzung — the engine applies the factor for the period.
    pub vermiedene_kosten_ct_per_kwh: Decimal,
    /// `true` when the plant is funded under the EEG.
    ///
    /// §18 Abs. 1 Satz 4 Nr. 1 StromNEV excludes such plants from the payment
    /// entirely; settling one is refused.
    pub ist_eeg_gefoerdert: bool,
    /// Price-sheet identifier for the trace, where one exists.
    pub tariff_sheet_id: Option<String>,
}

/// Settle the §18 payment for one plant and period.
///
/// The result is a payment *from* the Netzbetreiber *to* the plant operator, so
/// the position is negative from the NB's books — consistent with how a
/// Mehrmengen credit is signed elsewhere in this crate.
///
/// # Errors
///
/// - [`BillingError::InvalidInput`] for an EEG-funded plant (§18 Abs. 1 S. 4
///   Nr. 1 — the payment would be unlawful).
/// - [`BillingError::InvalidInput`] when the period crosses an Abschmelzung
///   step: the factor differs at its start and its end, so one settlement over
///   it would pay part of the energy at the wrong level. Split the period at
///   the step date.
/// - [`BillingError::InvalidInput`] for negative energy or a negative rate.
pub fn settle_dezentrale_einspeisung(
    input: &DezentraleEinspeisungInput,
) -> Result<SettlementResult, BillingError> {
    if input.ist_eeg_gefoerdert {
        return Err(BillingError::InvalidInput {
            reason: "an EEG-funded plant receives no Entgelt für dezentrale Erzeugung \
                     (§18 Abs. 1 Satz 4 Nr. 1 StromNEV)"
                .to_owned(),
        });
    }
    if input.einspeisung_kwh < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "einspeisung_kwh must be non-negative".to_owned(),
        });
    }
    if input.vermiedene_kosten_ct_per_kwh < Decimal::ZERO {
        return Err(BillingError::InvalidInput {
            reason: "vermiedene_kosten_ct_per_kwh must be non-negative".to_owned(),
        });
    }
    if period_crosses_a_step(input.period) {
        return Err(BillingError::InvalidInput {
            reason: format!(
                "the period {} – {} crosses a GBK-25-02-1#1 Abschmelzung step; \
                 split it at the step date so each part is paid at its factor",
                input.period.from(),
                input.period.to()
            ),
        });
    }

    let faktor = abschmelzfaktor(input.period.from());
    let base_eur = input.vermiedene_kosten_ct_per_kwh / dec!(100);
    let reduced_eur = (base_eur * faktor).round_dp(6);
    // Negative: the NB pays out.
    let net_eur = -(input.einspeisung_kwh * reduced_eur).round_dp(5);

    let mut positions = Vec::new();
    let mut warnings = Vec::new();
    if faktor.is_zero() {
        warnings.push(crate::types::SettlementWarning {
            severity: crate::types::WarningSeverity::Info,
            code: "SECT18_ABGESCHMOLZEN",
            message: "the Entgelt für dezentrale Erzeugung is fully phased out for this \
                      period (GBK-25-02-1#1); nothing is payable"
                .to_owned(),
        });
    } else {
        positions.push(SettlementPosition {
            text: format!(
                "Entgelt für dezentrale Erzeugung ({} % nach Abschmelzung)",
                (faktor * dec!(100)).normalize()
            ),
            kind: BillingPositionKind::DezentraleEinspeisung,
            quantity: input.einspeisung_kwh.round_dp(3),
            unit: crate::types::QuantityUnit::Kwh,
            unit_price_eur: reduced_eur,
            net_eur,
            spot_price_formula: None,
            trace: CalculationTrace {
                explanation: format!(
                    "{:.3} kWh × {:.6} EUR/kWh (= {:.6} × {faktor} Abschmelzung) = {:.5} EUR \
                     payable to the plant operator",
                    input.einspeisung_kwh,
                    reduced_eur,
                    base_eur,
                    net_eur.abs()
                ),
                input_quantity: input.einspeisung_kwh,
                input_unit_price_eur: reduced_eur,
                gross_eur: net_eur,
                legal_refs: vec![
                    LegalReference::StromNev { paragraph: "§18" },
                    LegalReference::BnetzaDecision {
                        reference: "GBK-25-02-1#1",
                    },
                ],
                tariff_source: input
                    .tariff_sheet_id
                    .clone()
                    .map(|sheet_id| TariffSource::PublishedTariffSheet { sheet_id }),
                regulatory_reduction_factor: Some(faktor),
                rounding_note: Some("unit price to 6 dp; net to 5 dp"),
            },
        });
    }

    Ok(SettlementResult {
        malo_id: input.malo_id.clone(),
        sparte: Sparte::Strom,
        regime: crate::regulatory::RegulatoryRegime::for_period(
            input.period.from(),
            input.period.to(),
        ),
        settlement_type: SettlementType::DezentraleEinspeisung,
        status: SettlementStatus::Initial,
        period: input.period,
        nb_mp_id: input.nb_mp_id.clone(),
        counterparty_mp_id: input.anlagenbetreiber_mp_id.clone(),
        total_eur: positions
            .iter()
            .map(|p| p.net_eur)
            .sum::<Decimal>()
            .round_dp(2),
        positions,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn base(period: SettlementPeriod) -> DezentraleEinspeisungInput {
        DezentraleEinspeisungInput {
            malo_id: "51238696780".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            anlagenbetreiber_mp_id: "9900012345678".to_owned(),
            period,
            einspeisung_kwh: dec!(10_000),
            vermiedene_kosten_ct_per_kwh: dec!(0.60),
            ist_eeg_gefoerdert: false,
            tariff_sheet_id: None,
        }
    }

    fn p(from: Date, to: Date) -> SettlementPeriod {
        SettlementPeriod::new(from, to).expect("valid period")
    }

    /// The Tenor's schedule, day by day at each boundary.
    #[test]
    fn the_tenor_schedule() {
        assert_eq!(abschmelzfaktor(date!(2026 - 06 - 30)), Decimal::ONE);
        assert_eq!(abschmelzfaktor(date!(2026 - 07 - 01)), dec!(0.50));
        assert_eq!(abschmelzfaktor(date!(2027 - 06 - 15)), dec!(0.50));
        assert_eq!(abschmelzfaktor(date!(2028 - 01 - 01)), dec!(0.25));
        assert_eq!(abschmelzfaktor(date!(2028 - 12 - 31)), dec!(0.25));
        assert_eq!(abschmelzfaktor(date!(2029 - 01 - 01)), Decimal::ZERO);
    }

    /// The annual averages fall by 25 points a year — the Tenor's own
    /// cross-check ("Die Kürzungen entsprechen einer jährlichen Abschmelzung
    /// von 25 %").
    #[test]
    fn the_annual_averages_fall_by_a_quarter() {
        // 2026: half the year at 1.00, half at 0.50.
        let h1 = abschmelzfaktor(date!(2026 - 03 - 01));
        let h2 = abschmelzfaktor(date!(2026 - 09 - 01));
        assert_eq!((h1 + h2) / dec!(2), dec!(0.75));
        assert_eq!(abschmelzfaktor(date!(2027 - 07 - 01)), dec!(0.50));
        assert_eq!(abschmelzfaktor(date!(2028 - 07 - 01)), dec!(0.25));
    }

    /// A pre-cut month pays the full rate; a 2028 month a quarter of it.
    #[test]
    fn the_factor_reaches_the_payment() {
        let full =
            settle_dezentrale_einspeisung(&base(p(date!(2026 - 01 - 01), date!(2026 - 01 - 31))))
                .expect("settles");
        // 10 000 kWh × 0.006 EUR = 60 EUR, paid out → negative.
        assert_eq!(full.total_eur, dec!(-60.00));

        let quarter =
            settle_dezentrale_einspeisung(&base(p(date!(2028 - 03 - 01), date!(2028 - 03 - 31))))
                .expect("settles");
        assert_eq!(quarter.total_eur, dec!(-15.00));
        assert_eq!(
            quarter.positions[0].trace.regulatory_reduction_factor,
            Some(dec!(0.25))
        );
    }

    /// June–July 2026 crosses the first step and must be split, not averaged.
    #[test]
    fn a_period_across_a_step_is_refused() {
        let r =
            settle_dezentrale_einspeisung(&base(p(date!(2026 - 06 - 15), date!(2026 - 07 - 15))));
        assert!(matches!(r, Err(BillingError::InvalidInput { .. })));
    }

    /// §18 Abs. 1 Satz 4 Nr. 1: an EEG plant gets nothing, and settling one is
    /// an error rather than a zero — the payment would be unlawful.
    #[test]
    fn an_eeg_plant_is_refused() {
        let mut i = base(p(date!(2026 - 01 - 01), date!(2026 - 01 - 31)));
        i.ist_eeg_gefoerdert = true;
        assert!(matches!(
            settle_dezentrale_einspeisung(&i),
            Err(BillingError::InvalidInput { .. })
        ));
    }

    /// From 2029 nothing is payable: no position, an Info saying why.
    #[test]
    fn from_2029_nothing_is_payable() {
        let r =
            settle_dezentrale_einspeisung(&base(p(date!(2029 - 02 - 01), date!(2029 - 02 - 28))))
                .expect("settles to zero");
        assert!(r.positions.is_empty());
        assert_eq!(r.total_eur, Decimal::ZERO);
        assert!(r.warnings.iter().any(|w| w.code == "SECT18_ABGESCHMOLZEN"));
    }
}
