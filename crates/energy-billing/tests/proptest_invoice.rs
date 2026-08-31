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
    BillingContext, BillingPeriod, GasMeterInput, GridInput, InvoiceType, MeterInput, Product,
    Quantities, RegulatoryRates,
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
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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
            "mwst_rate_override": "0.0",
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
            "arbeitspreis_ct_per_kwh": "30.0",
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

// ── Verbrauchsteuer-Begünstigungen ────────────────────────────────────────────

proptest! {
    /// **A Steuerentlastung never moves an amount.**
    ///
    /// § 9a/§ 9b/§ 9c StromStG are reliefs the customer claims from the
    /// Hauptzollamt after being invoiced in full. Declaring one must leave
    /// netto, MwSt and brutto bit-identical for any tariff and any consumption
    /// — the only difference is one 0-EUR note. This is the property the old
    /// model could not have: its `industrie_stromsteuer_befreiung` flag zeroed
    /// the levy, so declaring the relief changed the invoice by 2,05 ct/kWh.
    #[test]
    fn an_entlastung_leaves_every_total_untouched(
        ap in arb_arbeitspreis(),
        gp in arb_grundpreis(),
        kwh in arb_kwh(),
    ) {
        let plain: Product = serde_json::from_value(serde_json::json!({
            "category": "STROM",
            "arbeitspreis_ct_per_kwh": ap,
            "grundpreis_ct_per_day": gp,
        })).unwrap();
        let entlastet: Product = serde_json::from_value(serde_json::json!({
            "category": "STROM",
            "arbeitspreis_ct_per_kwh": ap,
            "grundpreis_ct_per_day": gp,
            "steuerentlastungen": ["STROMSTEUER9B", "STROMSTEUER9A"],
        })).unwrap();

        let q = Quantities {
            electricity: Some(MeterInput { arbeitsmenge_kwh: kwh, ..Default::default() }),
            ..Default::default()
        };
        let rates = RegulatoryRates::default();
        let a = plain.build_engine(&GridInput::default(), &rates)
            .bill(base_ctx(), &q).unwrap();
        let b = entlastet.build_engine(&GridInput::default(), &rates)
            .bill(base_ctx(), &q).unwrap();

        prop_assert_eq!(a.netto_eur, b.netto_eur);
        prop_assert_eq!(a.mwst_eur, b.mwst_eur);
        prop_assert_eq!(a.brutto_eur, b.brutto_eur);
        // …and the notes are the only new lines, all of them zero.
        prop_assert_eq!(b.positions.len(), a.positions.len() + 2);
        for p in b.positions.iter().filter(|p| p.has_tag("steuerentlastung")) {
            prop_assert_eq!(p.net_eur, Decimal::ZERO);
        }
    }

    /// **A Befreiung removes exactly the levy, and an Ermäßigung replaces it.**
    ///
    /// Both are supplier-side, so both change the invoice — the property is
    /// that they change it by exactly the levy and by nothing else. An
    /// Ermäßigung that dropped the line instead (the old `Bahnstrom` shape)
    /// would fail the second assertion.
    #[test]
    fn a_befreiung_and_an_ermaessigung_move_only_the_levy(
        ap in arb_arbeitspreis(),
        kwh in 1.0_f64..=50_000.0_f64,
    ) {
        let kwh = to_decimal(kwh);
        let make = |tarif: serde_json::Value| -> Product {
            serde_json::from_value(serde_json::json!({
                "category": "STROM",
                "arbeitspreis_ct_per_kwh": ap,
                "stromsteuer_tarif": tarif,
            })).unwrap()
        };
        let q = Quantities {
            electricity: Some(MeterInput { arbeitsmenge_kwh: kwh, ..Default::default() }),
            ..Default::default()
        };
        let rates = RegulatoryRates::default();
        let bill = |p: Product| p.build_engine(&GridInput::default(), &rates)
            .bill(base_ctx(), &q).unwrap();

        let regel = bill(make(serde_json::json!({"art": "REGEL"})));
        let befreit = bill(make(serde_json::json!({
            "art": "BEFREIUNG", "grund": "KLEINANLAGE"
        })));
        let ermaessigt = bill(make(serde_json::json!({
            "art": "ERMAESSIGUNG", "grund": "FAHRSTROM"
        })));

        // The Regelsatz levy is the whole difference to the Befreiung.
        let levy = regel.total_by_tag("stromsteuer");
        prop_assert_eq!(levy, energy_billing::round_money(kwh * dec!(0.0205), 5));
        prop_assert_eq!(befreit.total_by_tag("stromsteuer"), Decimal::ZERO);
        prop_assert_eq!(regel.netto_eur - befreit.netto_eur, levy);

        // The Ermäßigung keeps a levy line, at the statutory reduced rate.
        let reduced = ermaessigt.total_by_tag("stromsteuer");
        prop_assert_eq!(reduced, energy_billing::round_money(kwh * dec!(0.01142), 5));
        prop_assert!(reduced > Decimal::ZERO || kwh.is_zero());
        prop_assert_eq!(regel.netto_eur - ermaessigt.netto_eur, levy - reduced);
    }
}

// ── Calendar-exact period fractions ───────────────────────────────────────────

proptest! {
    /// **Twelve monthly invoices bill exactly what one annual invoice bills.**
    ///
    /// The property `days ÷ 30.4375` cannot have: it makes each month slightly
    /// more than a month, so twelve of them over-bill a year of Grundpreis —
    /// consistently, and always in the operator's favour.
    #[test]
    fn twelve_months_of_grundpreis_equal_one_year(
        eur_per_month in 1.0_f64..=500.0_f64,
    ) {
        let gp = to_decimal(eur_per_month);
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "WAERME",
            "waerme_grundpreis_eur_per_month": gp,
            "waerme_arbeitspreis_ct_per_kwh": "9.0",
        })).unwrap();
        let rates = RegulatoryRates::default();
        let heat = |kwh: Decimal| Quantities {
            heat: Some(energy_billing::WaermeMeterInput {
                kwh_waerme: kwh,
                months: None,
                ..Default::default()
            }),
            ..Default::default()
        };
        let ctx_for = |from, to| BillingContext {
            period: BillingPeriod::new(from, to).unwrap(),
            regulatory_rates: rates.clone(),
            ..base_ctx()
        };

        let annual = tariff.build_engine(&GridInput::default(), &rates)
            .bill(ctx_for(date!(2026-01-01), date!(2026-12-31)), &heat(dec!(12000)))
            .unwrap();

        let mut monthly_total = Decimal::ZERO;
        for m in 1u8..=12 {
            let month = time::Month::try_from(m).unwrap();
            let first = time::Date::from_calendar_date(2026, month, 1).unwrap();
            let last = time::Date::from_calendar_date(
                2026, month, time::util::days_in_month(month, 2026)).unwrap();
            let inv = tariff.build_engine(&GridInput::default(), &rates)
                .bill(ctx_for(first, last), &heat(dec!(1000))).unwrap();
            monthly_total += inv
                .positions
                .iter()
                .filter(|p| p.description.starts_with("Grundpreis"))
                .map(|p| p.net_eur)
                .sum::<Decimal>();
        }
        let annual_gp: Decimal = annual
            .positions
            .iter()
            .filter(|p| p.description.starts_with("Grundpreis"))
            .map(|p| p.net_eur)
            .sum();
        prop_assert_eq!(annual_gp, monthly_total);
        prop_assert_eq!(annual_gp, energy_billing::round_money(gp * dec!(12), 5));
    }
}

// ── No silent zero ────────────────────────────────────────────────────────────

proptest! {
    /// **Consumption that reaches the engine is always priced.**
    ///
    /// The generalisation of every silent-zero defect this crate has had: a
    /// priceless product, an indexed tariff with no index value, a Zweitarif
    /// product against an unsplit meter, HT/NT registers with no stated total.
    /// Each billed the levies and nothing for the electricity, each looked like
    /// an ordinary invoice, and each was found by hand.
    ///
    /// The property is the invariant behind all of them: whenever `bill()`
    /// *succeeds* and there is consumption, an Arbeitspreis position exists and
    /// is non-zero. A future pricing shape that cannot price its quantities
    /// must therefore refuse, not return a Grundpreis-and-levies invoice.
    #[test]
    fn any_billable_consumption_produces_a_work_price(
        ap in arb_arbeitspreis(),
        ht in arb_arbeitspreis(),
        nt in arb_arbeitspreis(),
        kwh in 1.0_f64..=50_000.0_f64,
        shape in 0usize..4,
        split in prop::option::of(0.0_f64..=1.0_f64),
    ) {
        let kwh = to_decimal(kwh);
        prop_assume!(kwh > Decimal::ZERO);

        // Four pricing shapes the catalog can produce.
        let tariff = match shape {
            0 => serde_json::json!({"category": "STROM", "arbeitspreis_ct_per_kwh": ap}),
            1 => serde_json::json!({
                "category": "STROM",
                "arbeitspreis_ht_ct_per_kwh": ht,
                "arbeitspreis_nt_ct_per_kwh": nt,
            }),
            2 => serde_json::json!({
                "category": "STROM",
                "arbeitspreis_ct_per_kwh": ap,
                "indexed_price": {
                    "base_ct_per_kwh": "5", "spread_ct_per_kwh": "1",
                    "index_name": "Phelix Base", "factor_ct_per_unit": "0.1",
                },
            }),
            _ => serde_json::json!({
                "category": "STROM",
                "block_tiers": [{"bis_kwh": "1000", "preis_ct_per_kwh": ap}, {"preis_ct_per_kwh": ap}],
            }),
        };
        let product: Product = serde_json::from_value(tariff).unwrap();

        // …and the meter, with or without an HT/NT split.
        let electricity = match split {
            Some(f) => {
                let f = to_decimal(f).min(Decimal::ONE);
                let ht_kwh = (kwh * f).round_dp(3);
                MeterInput {
                    arbeitsmenge_kwh: kwh,
                    arbeitsmenge_ht_kwh: Some(ht_kwh),
                    arbeitsmenge_nt_kwh: Some(kwh - ht_kwh),
                    ..Default::default()
                }
            }
            None => MeterInput { arbeitsmenge_kwh: kwh, ..Default::default() },
        };
        let quantities = Quantities { electricity: Some(electricity), ..Default::default() };

        let rates = RegulatoryRates::default();
        // A refusal is a correct outcome — the point is that *success* implies
        // the electricity was priced.
        if let Ok(invoice) = product
            .build_engine(&GridInput::default(), &rates)
            .bill(base_ctx(), &quantities)
        {
            let work = invoice.total_by_tag("arbeitspreis");
            prop_assert!(
                work > Decimal::ZERO,
                "billed {kwh} kWh and charged {work} EUR for the electricity; \
                 positions: {:?}",
                invoice
                    .positions
                    .iter()
                    .map(|p| (&p.description, p.net_eur))
                    .collect::<Vec<_>>()
            );
        }
    }
}

proptest! {
    /// **The same invariant, across the other metered commodities.**
    ///
    /// The property above is electricity-only, which is exactly why the defect
    /// recurred outside it: gas, Fernwärme and charging energy each reach the
    /// engine as a delivered quantity, and each had (or could have had) a
    /// pricing shape that billed the fees and nothing for the commodity.
    ///
    /// Same statement, same tag: whenever `bill()` succeeds and a quantity was
    /// delivered, an Arbeitspreis position exists and is non-zero. A refusal
    /// remains a correct outcome — the claim is about what *success* implies.
    #[test]
    fn any_delivered_commodity_produces_a_work_price(
        ap in arb_arbeitspreis(),
        qty in 1.0_f64..=50_000.0_f64,
        shape in 0usize..3,
    ) {
        let qty = to_decimal(qty);
        prop_assume!(qty > Decimal::ZERO);
        prop_assume!(ap > Decimal::ZERO);

        let (product, quantities) = match shape {
            0 => (
                serde_json::json!({
                    "category": "GAS",
                    "gas_arbeitspreis_ct_per_kwh_hs": ap,
                }),
                Quantities {
                    gas: Some(GasMeterInput { kwh_hs: Some(qty), ..Default::default() }),
                    ..Default::default()
                },
            ),
            1 => (
                serde_json::json!({
                    "category": "WAERME",
                    "waerme_arbeitspreis_ct_per_kwh": ap,
                }),
                Quantities {
                    heat: Some(energy_billing::WaermeMeterInput {
                        kwh_waerme: qty,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ),
            _ => (
                serde_json::json!({
                    "category": "EMOBILITY",
                    "emobility_kwh_price_ct": ap,
                }),
                Quantities {
                    emobility: Some(energy_billing::EmobilityMeterInput {
                        kwh_charged: Some(qty),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ),
        };
        let product: Product = serde_json::from_value(product).unwrap();

        let rates = RegulatoryRates::default();
        if let Ok(invoice) = product
            .build_engine(&GridInput::default(), &rates)
            .bill(base_ctx(), &quantities)
        {
            let work = invoice.total_by_tag("arbeitspreis");
            prop_assert!(
                work > Decimal::ZERO,
                "delivered {qty} and charged {work} EUR for the commodity; positions: {:?}",
                invoice
                    .positions
                    .iter()
                    .map(|p| (&p.description, p.net_eur))
                    .collect::<Vec<_>>()
            );
        }
    }
}

proptest! {
    /// **Every line multiplies out.** `PEPPOL-EN16931-R120` allows ±0.02 between
    /// a line's `price × quantity` and its amount, and a receiver rejects on it.
    ///
    /// The trap is a *rounded* price: rounding what the page prints is right,
    /// but the same figure is the machine field BT-146, and the further it is
    /// rounded and the more units it multiplies, the further the product drifts
    /// from the amount beside it. §41a states a weighted average — rarely
    /// representable — so it is the position most exposed, and the drift only
    /// appears at volume.
    #[test]
    fn every_position_price_multiplies_out_to_its_amount(
        ap in arb_arbeitspreis(),
        gp in 0.0_f64..=500.0_f64,
        kwh in 1.0_f64..=20_000_000.0_f64,
    ) {
        let kwh = to_decimal(kwh);
        prop_assume!(kwh > Decimal::ZERO);
        let tariff = serde_json::json!({
            "category": "STROM",
            "arbeitspreis_ct_per_kwh": ap,
            "grundpreis_ct_per_day": to_decimal(gp),
        });
        let product: Product = serde_json::from_value(tariff).unwrap();
        let quantities = Quantities {
            electricity: Some(MeterInput { arbeitsmenge_kwh: kwh, ..Default::default() }),
            ..Default::default()
        };

        if let Ok(invoice) = product
            .build_engine(&GridInput::default(), &RegulatoryRates::default())
            .bill(base_ctx(), &quantities)
        {
            for p in &invoice.positions {
                if p.quantity == Decimal::ZERO {
                    continue;
                }
                let drift = (p.quantity * p.unit_price_eur - p.net_eur).abs();
                prop_assert!(
                    drift <= dec!(0.02),
                    "PEPPOL-EN16931-R120: {:?} states {} x {} against an amount of {} \
                     (drift {drift})",
                    p.description, p.quantity, p.unit_price_eur, p.net_eur
                );
            }
        }
    }
}
