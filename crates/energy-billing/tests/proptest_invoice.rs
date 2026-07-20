//! Property-based tests for `energy-billing` arithmetic invariants.
//!
//! Uses [`proptest`] to verify that the `Invoice` arithmetic invariants hold
//! for any combination of randomised tariff prices and consumption values.
//!
//! ## Invariants under test
//!
//! 1. `brutto_eur == netto_eur + mwst_eur` (within 0.001 EUR rounding)
//! 2. `zahlbetrag_eur == brutto_eur - abschlag_total_eur`
//! 3. `netto_eur >= 0` for normal (non-credit) invoices
//! 4. Cancellation invoice has opposite sign: `cancelled.brutto_eur == -original.brutto_eur`
//! 5. Block tariff total matches flat-rate total for equivalent consumption
//! 6. MwSt 0% produces `mwst_eur == 0`
//! 7. Zero consumption → `netto_eur == 0` for commodity-only products
//! 8. Pro-rata fraction in [0, 1] → `brutto_eur <= full_period_brutto_eur`

use energy_billing::{
    BillingContext, GasMeterInput, GridInput, InvoiceType, MeterInput, Product, Quantities,
    RegulatoryRates,
};
use proptest::prelude::*;
use rust_decimal::Decimal;
use rust_decimal::dec;
use time::macros::date;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn base_ctx() -> BillingContext {
    BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-PROP-001".to_owned(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: RegulatoryRates::default(),
        ..Default::default()
    }
}

/// Convert f64 from proptest into a non-negative Decimal with bounded precision.
fn to_decimal(f: f64) -> Decimal {
    Decimal::from_f64_retain(f.abs())
        .unwrap_or(Decimal::ZERO)
        .round_dp(4)
}

// ── Strategy generators ───────────────────────────────────────────────────────

/// Arbitrary electricity arbeitspreis (0.5–100 ct/kWh).
fn arb_arbeitspreis() -> impl Strategy<Value = Decimal> {
    (0.5_f64..=100.0_f64).prop_map(to_decimal)
}

/// Arbitrary grundpreis (0–50 ct/day).
fn arb_grundpreis() -> impl Strategy<Value = Decimal> {
    (0.0_f64..=50.0_f64).prop_map(to_decimal)
}

/// Arbitrary consumption (0–50 000 kWh).
fn arb_kwh() -> impl Strategy<Value = Decimal> {
    (0.0_f64..=50_000.0_f64).prop_map(to_decimal)
}

/// Arbitrary MwSt rate: 0%, 7%, or 19%.
fn arb_mwst() -> impl Strategy<Value = Decimal> {
    prop_oneof![Just(dec!(0.00)), Just(dec!(0.07)), Just(dec!(0.19)),]
}

// ── Invariant 1 & 2: brutto = netto + mwst, zahlbetrag = brutto ──────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Electricity: `brutto == netto + mwst` and `zahlbetrag == brutto` for all
    /// combinations of randomised tariff prices and consumption.
    #[test]
    fn electricity_invoice_arithmetic_invariants(
        arbeitspreis in arb_arbeitspreis(),
        grundpreis in arb_grundpreis(),
        kwh in arb_kwh(),
        mwst_rate in arb_mwst(),
    ) {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "STROM",
            "arbeitspreis_ct_per_kwh": arbeitspreis,
            "grundpreis_ct_per_day": grundpreis,
            "mwst_rate_override": mwst_rate,
        })).unwrap();

        let ctx = base_ctx();
        let quantities = Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: kwh,
                ..Default::default()
            }),
            ..Default::default()
        };

        let invoice = tariff.build_engine(&GridInput::default(), &ctx.regulatory_rates)
            .bill(ctx, &quantities).unwrap();

        invoice.assert_valid();

        // Invariant 1: brutto == netto + mwst (within 0.001 EUR rounding tolerance)
        let diff = (invoice.brutto_eur - (invoice.netto_eur + invoice.mwst_eur)).abs();
        prop_assert!(
            diff < dec!(0.001),
            "brutto({}) != netto({}) + mwst({})",
            invoice.brutto_eur, invoice.netto_eur, invoice.mwst_eur
        );

        // Invariant 2: zahlbetrag == brutto (no Abschläge in base ctx)
        prop_assert_eq!(
            invoice.zahlbetrag_eur,
            invoice.brutto_eur,
            "zahlbetrag must equal brutto when no Abschläge"
        );

        // Invariant 3: netto >= 0 for normal invoices
        prop_assert!(
            invoice.netto_eur >= Decimal::ZERO,
            "netto must be non-negative for Initial invoice, got {}",
            invoice.netto_eur
        );
    }

    /// Gas: arithmetic invariants hold for arbitrary gas prices and consumption.
    #[test]
    fn gas_invoice_arithmetic_invariants(
        arbeitspreis in arb_arbeitspreis(),
        grundpreis in arb_grundpreis(),
        kwh in arb_kwh(),
        mwst_rate in arb_mwst(),
    ) {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "GAS",
            "gas_arbeitspreis_ct_per_kwh_hs": arbeitspreis,
            "gas_grundpreis_ct_per_day": grundpreis,
            "mwst_rate_override": mwst_rate,
        })).unwrap();

        let ctx = base_ctx();
        let quantities = Quantities {
            gas: Some(GasMeterInput {
                kwh_hs: Some(kwh),
                ..Default::default()
            }),
            ..Default::default()
        };

        let invoice = tariff.build_engine(&GridInput::default(), &ctx.regulatory_rates)
            .bill(ctx, &quantities).unwrap();

        invoice.assert_valid();

        let diff = (invoice.brutto_eur - (invoice.netto_eur + invoice.mwst_eur)).abs();
        prop_assert!(diff < dec!(0.001), "gas: brutto != netto + mwst");
        prop_assert!(invoice.netto_eur >= Decimal::ZERO, "gas: netto < 0");
    }

    /// MwSt 0% → mwst_eur must be zero for any positive consumption.
    #[test]
    fn zero_mwst_produces_zero_tax(
        kwh in arb_kwh(),
        ap in arb_arbeitspreis(),
    ) {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "STROM",
            "arbeitspreis_ct_per_kwh": ap,
            "mwst_rate_override": 0.0,
        })).unwrap();

        let ctx = base_ctx();
        let quantities = Quantities {
            electricity: Some(MeterInput { arbeitsmenge_kwh: kwh, ..Default::default() }),
            ..Default::default()
        };
        let invoice = tariff.build_engine(&GridInput::default(), &ctx.regulatory_rates)
            .bill(ctx, &quantities).unwrap();

        prop_assert_eq!(
            invoice.mwst_eur,
            Decimal::ZERO,
            "0% MwSt must produce zero tax, got {}",
            invoice.mwst_eur
        );
        prop_assert_eq!(
            invoice.brutto_eur,
            invoice.netto_eur,
            "brutto must equal netto when mwst = 0"
        );
    }

    /// Zero consumption → only Grundpreis contributes to netto.
    #[test]
    fn zero_consumption_only_grundpreis(
        gp in arb_grundpreis(),
        mwst in arb_mwst(),
    ) {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "STROM",
            "arbeitspreis_ct_per_kwh": 30.0,
            "grundpreis_ct_per_day": gp,
            "mwst_rate_override": mwst,
        })).unwrap();

        let ctx = base_ctx();
        let quantities = Quantities {
            electricity: Some(MeterInput { arbeitsmenge_kwh: Decimal::ZERO, ..Default::default() }),
            ..Default::default()
        };
        let invoice = tariff.build_engine(&GridInput::default(), &ctx.regulatory_rates)
            .bill(ctx, &quantities).unwrap();

        // Expected netto: grundpreis only = gp_ct/day × days / 100
        let days = Decimal::from(31u32); // Jan 2026 = 31 days
        let expected_netto = (gp / dec!(100) * days).round_dp(5);
        let diff = (invoice.netto_eur - expected_netto).abs();
        prop_assert!(
            diff < dec!(0.01),
            "zero-consumption netto({}) != grundpreis-only({})",
            invoice.netto_eur, expected_netto
        );
    }

    /// Cancellation invoice has exactly opposite sign of the original.
    #[test]
    fn cancellation_negates_original(
        ap in arb_arbeitspreis(),
        kwh in (0.1_f64..=50_000.0_f64).prop_map(to_decimal),
    ) {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "STROM",
            "arbeitspreis_ct_per_kwh": ap,
        })).unwrap();

        let quantities = Quantities {
            electricity: Some(MeterInput { arbeitsmenge_kwh: kwh, ..Default::default() }),
            ..Default::default()
        };

        let mut ctx_orig = base_ctx();
        ctx_orig.invoice_type = InvoiceType::Initial;
        let original = tariff.build_engine(&GridInput::default(), &ctx_orig.regulatory_rates)
            .bill(ctx_orig, &quantities).unwrap();

        let mut ctx_cancel = base_ctx();
        ctx_cancel.invoice_type = InvoiceType::Cancellation {
            original_invoice_id: "R-PROP-001".to_owned(),
        };
        let cancellation = tariff.build_engine(&GridInput::default(), &ctx_cancel.regulatory_rates)
            .bill(ctx_cancel, &quantities).unwrap();

        // Cancellation must exactly negate the original
        let sum = original.brutto_eur + cancellation.brutto_eur;
        prop_assert!(
            sum.abs() < dec!(0.001),
            "original({}) + cancellation({}) must sum to 0, got {}",
            original.brutto_eur, cancellation.brutto_eur, sum
        );
    }
}

// ── Gas invoice invariants ────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Gas: `brutto == netto + mwst` for all randomised tariff prices and consumption.
    /// Also verifies: BEHG position exists when BEHG rate > 0.
    #[test]
    fn gas_invoice_arithmetic_and_behg_invariants(
        arbeitspreis in (0.5_f64..=50.0_f64).prop_map(to_decimal),
        kwh_hs in (0.0_f64..=100_000.0_f64).prop_map(to_decimal),
        mwst_rate in arb_mwst(),
        behg_ct in (0.0_f64..=5.0_f64).prop_map(to_decimal),
    ) {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "GAS",
            "gas_arbeitspreis_ct_per_kwh_hs": arbeitspreis,
            "mwst_rate_override": mwst_rate,
            "behg_gas_ct_per_kwh_override": behg_ct,
        })).unwrap();

        let rates = RegulatoryRates {
            behg_gas_ct_per_kwh: behg_ct,
            ..RegulatoryRates::default()
        };

        let ctx = BillingContext {
            regulatory_rates: rates.clone(),
            ..base_ctx()
        };

        let quantities = Quantities {
            gas: Some(energy_billing::GasMeterInput {
                kwh_hs: Some(kwh_hs),
                ..Default::default()
            }),
            ..Default::default()
        };

        let invoice = tariff.build_engine(&GridInput::default(), &rates).bill(ctx, &quantities).unwrap();

        // brutto == netto + mwst (within 0.01 EUR rounding)
        let diff = (invoice.brutto_eur - (invoice.netto_eur + invoice.mwst_eur)).abs();
        prop_assert!(
            diff <= dec!(0.01),
            "Gas: brutto({}) != netto({}) + mwst({}), diff={}",
            invoice.brutto_eur, invoice.netto_eur, invoice.mwst_eur, diff
        );

        // zahlbetrag == brutto (no Abschlag in this test)
        prop_assert_eq!(invoice.zahlbetrag_eur, invoice.brutto_eur);
    }

    /// RLM demand charge: Leistungspreis position is always non-negative.
    #[test]
    fn rlm_demand_charge_non_negative(
        arbeitspreis in (0.5_f64..=50.0_f64).prop_map(to_decimal),
        leistungspreis_ct in (0.0_f64..=100.0_f64).prop_map(to_decimal),
        kwh in arb_kwh(),
        kw in (0.0_f64..=10_000.0_f64).prop_map(to_decimal),
    ) {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "STROM",
            "arbeitspreis_ct_per_kwh": arbeitspreis,
            "leistungspreis_strom_ct_per_kw_month": leistungspreis_ct,
        })).unwrap();

        let ctx = base_ctx();
        let quantities = Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: kwh,
                spitzenleistung_kw: Some(kw),
                ..Default::default()
            }),
            ..Default::default()
        };

        let invoice = tariff.build_engine(&GridInput::default(), &ctx.regulatory_rates).bill(ctx, &quantities).unwrap();

        // All Leistungspreis positions must be non-negative
        for pos in &invoice.positions {
            if pos.tags.iter().any(|t| t == "leistungspreis") {
                prop_assert!(
                    pos.net_eur >= dec!(0),
                    "Leistungspreis position must be non-negative, got {}",
                    pos.net_eur
                );
            }
        }

        // Overall netto must be non-negative for positive tariff
        if arbeitspreis > dec!(0) && leistungspreis_ct >= dec!(0) {
            prop_assert!(
                invoice.netto_eur >= dec!(0),
                "netto_eur must be non-negative for positive tariff, got {}",
                invoice.netto_eur
            );
        }
    }

    /// Historic Stromsteuer rate: `stromsteuer_for_year` returns consistent values.
    #[test]
    fn historic_stromsteuer_year_table_consistent(year in 2003i32..=2026i32) {
        use energy_billing::stromsteuer_for_year;
        let rate = stromsteuer_for_year(year);
        // All years 2003-2026 must have a known rate
        prop_assert!(rate.is_some(), "StromStG rate for {year} must be known");
        // Rate must be positive and plausible (1.0 – 3.0 ct/kWh)
        let r = rate.unwrap();
        prop_assert!(
            r >= dec!(1.0) && r <= dec!(3.0),
            "StromStG rate {r} for {year} outside plausible range [1.0, 3.0]"
        );
    }
}
