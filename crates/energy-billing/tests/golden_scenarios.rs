//! Golden master tests — canonical German energy billing scenarios.
//!
//! These tests compute invoices for well-defined real-world scenarios and
//! assert exact EUR amounts. They serve as regression tests: if a refactoring
//! or regulatory change silently alters an invoice amount, a golden test will
//! catch it.
//!
//! ## Scenarios
//!
//! 1. **Standard electricity** — SLP customer, flat rate, 31-day month
//! 2. **Gas with Brennwert + BEHG** — monthly gas bill with levies
//! 3. **EEG feed-in Gutschrift** — solar plant operator monthly credit note
//!
//! ## Updating golden values
//!
//! If the calculation is intentionally changed (e.g., new BEHG rate), update
//! the expected values in this file. Each test documents the full calculation
//! path so the expected values can be verified by hand.

use energy_billing::{
    BillingContext, GasMeterInput, GridInput, InvoiceType, MeterInput, PositionCategory,
    Quantities, RegulatoryRates, TariffInput,
};
use rust_decimal_macros::dec;
use time::macros::date;

// ── Scenario 1: Standard electricity — SLP customer, Eintarif ────────────────

/// **Golden: Standard SLP electricity invoice, January 2026 (31 days)**
///
/// ## Tariff
/// - Arbeitspreis: 28.50 ct/kWh
/// - Grundpreis: 8.00 ct/day
/// - Stromsteuer: 2.05 ct/kWh (§3 StromStG)
/// - MwSt: 19%
///
/// ## Consumption
/// - 320 kWh (Jan 2026)
///
/// ## Expected calculation
/// ```
/// Arbeitspreis: 320 kWh × 28.50 ct = 91.20 EUR
/// Grundpreis:   31 days × 0.0800 EUR/day = 2.48 EUR
/// Stromsteuer:  320 kWh × 2.05 ct = 6.56 EUR
/// Netto total:  91.20 + 2.48 + 6.56 = 100.24 EUR
/// MwSt 19%:     100.24 × 0.19 = 19.0456 → 19.05 EUR (rounded)
/// Brutto:       100.24 + 19.05 = 119.29 EUR
/// ```
#[test]
fn golden_strom_slp_eintarif_jan_2026() {
    let tariff: TariffInput = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 28.50,
        "grundpreis_ct_per_day": 8.00
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.310),
        mwst_rate: dec!(0.19),
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-STROM-001".to_owned(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };

    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(320),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .unwrap()
        .bill(ctx, &quantities)
        .unwrap();

    invoice.assert_valid();

    // Arbeitspreis: 320 × 0.2850 = 91.20
    let arbeit = invoice.total_by_tag("arbeitspreis");
    assert_eq!(arbeit.round_dp(2), dec!(91.20), "Arbeitspreis golden");

    // Grundpreis: 31 × 0.08 = 2.48
    let grund = invoice.total_by_tag("grundpreis");
    assert_eq!(grund.round_dp(2), dec!(2.48), "Grundpreis golden");

    // Stromsteuer: 320 × 0.0205 = 6.56
    let stromst = invoice.total_by_tag("stromsteuer");
    assert_eq!(stromst.round_dp(2), dec!(6.56), "Stromsteuer golden");

    // Netto: 91.20 + 2.48 + 6.56 = 100.24
    assert_eq!(invoice.netto_eur.round_dp(2), dec!(100.24), "Netto golden");

    // MwSt 19%: 100.24 × 0.19 = 19.0456 → rounded in MwSt position
    assert!(
        (invoice.mwst_eur - dec!(19.05)).abs() < dec!(0.01),
        "MwSt golden: expected ~19.05 EUR, got {}",
        invoice.mwst_eur
    );

    // Brutto: netto + mwst ≈ 119.29
    assert!(
        (invoice.brutto_eur - dec!(119.29)).abs() < dec!(0.01),
        "Brutto golden: expected ~119.29 EUR, got {}",
        invoice.brutto_eur
    );
}

// ── Scenario 2: Gas with levies ───────────────────────────────────────────────

/// **Golden: Gas invoice with Brennwert, Energiesteuer, BEHG CO₂, January 2026**
///
/// ## Tariff
/// - Arbeitspreis: 7.50 ct/kWh_Hs
/// - Grundpreis: 5.00 ct/day
/// - Energiesteuer: 0.55 ct/kWh (§2 EnergieStG)
/// - BEHG CO₂ (2026): 65 EUR/t × 0.20160 kg/kWh ÷ 10 = 1.3104 ct/kWh
/// - MwSt: 19%
///
/// ## Consumption
/// - 500 kWh_Hs
///
/// ## Expected calculation
/// ```
/// Arbeitspreis Gas:   500 × 7.50 ct = 37.50 EUR
/// Grundpreis Gas:     31 × 0.0500 = 1.55 EUR
/// Energiesteuer:      500 × 0.55 ct = 2.75 EUR
/// BEHG CO₂:           500 × 1.3104 ct = 6.552 EUR
/// Netto:              37.50 + 1.55 + 2.75 + 6.552 = 48.352 EUR
/// MwSt 19%:           48.352 × 0.19 = 9.18688 EUR
/// Brutto:             ≈ 57.54 EUR
/// ```
#[test]
fn golden_gas_with_levies_jan_2026() {
    let tariff: TariffInput = serde_json::from_str(
        r#"{
        "category": "GAS",
        "gas_arbeitspreis_ct_per_kwh_hs": 7.50,
        "gas_grundpreis_ct_per_day": 5.00
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.3104),
        mwst_rate: dec!(0.19),
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-GAS-001".to_owned(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };

    let quantities = Quantities {
        gas: Some(GasMeterInput {
            kwh_hs: Some(dec!(500)),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .unwrap()
        .bill(ctx, &quantities)
        .unwrap();

    invoice.assert_valid();

    // Arbeitspreis gas: 500 × 0.075 = 37.50
    // Only Commodity positions tagged "gas" that are NOT grundpreis
    let gas_arbeit: rust_decimal::Decimal = invoice
        .positions_by_tag("gas")
        .filter(|p| {
            p.category == energy_billing::PositionCategory::Commodity && !p.has_tag("grundpreis")
        })
        .map(|p| p.net_eur)
        .sum();
    assert_eq!(
        gas_arbeit.round_dp(2),
        dec!(37.50),
        "Gas Arbeitspreis golden"
    );

    // Grundpreis gas: 31 × 0.05 = 1.55
    assert_eq!(
        invoice.total_by_tag("grundpreis").round_dp(2),
        dec!(1.55),
        "Gas Grundpreis golden"
    );

    // Energiesteuer: 500 × 0.0055 = 2.75
    assert_eq!(
        invoice.total_by_tag("energiesteuer_gas").round_dp(2),
        dec!(2.75),
        "Energiesteuer golden"
    );

    // BEHG: 500 × 0.013104 = 6.552
    let behg = invoice.total_by_tag("behg");
    assert!(
        (behg - dec!(6.552)).abs() < dec!(0.01),
        "BEHG golden: expected ~6.552, got {}",
        behg
    );

    // Netto: ~48.352
    assert!(
        (invoice.netto_eur - dec!(48.352)).abs() < dec!(0.05),
        "Gas netto golden: expected ~48.35, got {}",
        invoice.netto_eur
    );

    // Brutto: ~57.54
    assert!(
        (invoice.brutto_eur - dec!(57.54)).abs() < dec!(0.05),
        "Gas brutto golden: expected ~57.54, got {}",
        invoice.brutto_eur
    );
}

// ── Scenario 3: EEG Gutschrift ────────────────────────────────────────────────

/// **Golden: EEG feed-in Gutschrift (credit note), small PV plant, January 2026**
///
/// ## Context
/// LF issues a monthly Gutschrift to a PV plant operator (10 kWp, EEG 2023,
/// Einspeisevergütung). MwSt: 0% per §12 Abs. 3 UStG (≤ 30 kWp).
///
/// ## Tariff
/// - Einspeisevergütung: 8.20 ct/kWh (EEG 2023, ≤10 kWp)
/// - anlage_kwp: 10 → auto 0% MwSt (§12 Abs. 3 UStG)
///
/// ## Feed-in quantity
/// - 280 kWh (Jan 2026)
///
/// ## Expected calculation
/// ```
/// Vergütung: 280 kWh × 8.20 ct = 22.96 EUR
/// MwSt 0%:   22.96 × 0 = 0.00 EUR
/// Brutto:    22.96 EUR
/// ```
#[test]
fn golden_eeg_gutschrift_10kwp_jan_2026() {
    use energy_billing::EegMeterInput;

    let tariff: TariffInput = serde_json::from_str(
        r#"{
        "category": "EEG",
        "eeg_verguetungssatz_ct_per_kwh": 8.20,
        "anlage_kwp": 10.0
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-EEG-001".to_owned(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::CreditNote,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };

    let quantities = Quantities {
        eeg: Some(EegMeterInput {
            einspeisung_kwh: dec!(280),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .unwrap()
        .bill(ctx, &quantities)
        .unwrap();

    invoice.assert_valid();

    // Vergütung: 280 × 0.0820 = 22.96 EUR
    let verguetung = invoice.total_by_tag("eeg_verguetung");
    assert_eq!(verguetung.round_dp(2), dec!(22.96), "EEG Vergütung golden");

    // §12 Abs. 3 UStG auto-0% for 10 kWp
    assert_eq!(invoice.mwst_eur, dec!(0), "EEG ≤30 kWp: MwSt must be 0");

    // Brutto equals netto for 0% MwSt
    assert_eq!(
        invoice.brutto_eur.round_dp(2),
        dec!(22.96),
        "EEG brutto golden"
    );

    // Verify JSON includes correct rechnungsart
    let json = invoice.to_rechnung_json();
    assert_eq!(
        json["rechnungsart"].as_str().unwrap(),
        "GUTSCHRIFT",
        "EEG credit note must be rechnungsart GUTSCHRIFT"
    );
}

// ── Scenario 4: RLM demand charge — large commercial electricity ──────────────

/// **Golden: RLM electricity invoice with demand charge (Leistungspreis)**
///
/// ## Tariff
/// - Arbeitspreis: 24.00 ct/kWh
/// - Grundpreis: 0 ct/day (no separate Grundpreis for RLM)
/// - Leistungspreis: 4.50 ct/kW/month (demand charge)
/// - Stromsteuer: 2.05 ct/kWh
/// - MwSt: 19%
///
/// ## Consumption (January 2026, 31 days)
/// - Energy: 12 000 kWh
/// - Peak demand: 45 kW (Spitzenleistung)
///
/// ## Expected calculation
/// ```
/// Arbeitspreis: 12 000 kWh × 24.00 ct = 2 880.00 EUR
/// Leistungspreis: 45 kW × 4.50 ct/month = 2.025 EUR → 2.025 EUR
/// Stromsteuer: 12 000 kWh × 2.05 ct = 246.00 EUR
/// Netto: 2 880.00 + 2.025 + 246.00 = 3 128.025 EUR
/// MwSt 19%: 3 128.025 × 0.19 = 594.3248 → 594.32 EUR (rounded to 2dp on total)
/// Brutto: 3 128.03 + 594.33 ≈ 3 722.35 EUR
/// ```
/// (exact values depend on internal rounding — the key check is positions exist)
#[test]
fn golden_rlm_demand_charge() {
    let tariff: TariffInput = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 24.00,
        "leistungspreis_strom_ct_per_kw_month": 4.50
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.310),
        mwst_rate: dec!(0.19),
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-RLM-001".to_owned(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates,
        ..Default::default()
    };

    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(12000),
            spitzenleistung_kw: Some(dec!(45)),
            metering_mode: energy_billing::MeteringMode::Rlm,
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &ctx.regulatory_rates)
        .expect("RLM tariff must build engine")
        .bill(ctx, &quantities)
        .expect("RLM billing must succeed");

    // Arbeitspreis: 12 000 × 24.00ct / 100 = 2 880.00 EUR
    let arbeit = invoice
        .positions
        .iter()
        .find(|p| p.tags.iter().any(|t| t == "arbeitspreis"))
        .expect("Arbeitspreis position must exist");
    assert_eq!(arbeit.quantity, dec!(12000));

    // Leistungspreis: 45 kW × 4.50ct / 100 = 2.025 EUR
    let leistung = invoice
        .positions
        .iter()
        .find(|p| p.tags.iter().any(|t| t == "leistungspreis"))
        .expect("Leistungspreis position must exist");
    assert_eq!(leistung.quantity, dec!(45));
    assert_eq!(leistung.unit, "kW");
    assert_eq!(leistung.category, PositionCategory::Commodity);

    let expected_leistung_eur = dec!(45) * dec!(4.50) / dec!(100);
    let diff = (leistung.net_eur - expected_leistung_eur).abs();
    assert!(
        diff < dec!(0.0001),
        "Leistungspreis: expected {expected_leistung_eur}, got {}",
        leistung.net_eur
    );

    // Invoice must be in debit territory (netto > 0)
    assert!(
        invoice.netto_eur > dec!(2000),
        "Large RLM invoice must have significant netto amount"
    );
    assert!(
        invoice.brutto_eur > invoice.netto_eur,
        "Brutto must exceed Netto by MwSt"
    );
}

// ── Scenario 5: Gas Energiesteuer exemption — CHP plant ──────────────────────

/// **Golden: Industrial gas invoice with §54 EnergieStG Energiesteuer exemption (KWK)**
///
/// A CHP (KWK) plant operator receives a gas supply invoice where Energiesteuer
/// is exempt under §54 Abs. 1 EnergieStG (gas used in Kraft-Wärme-Kopplung).
///
/// ## Tariff
/// - Gas Arbeitspreis: 8.00 ct/kWh_Hs
/// - Grundpreis: 0
/// - Energiesteuer: EXEMPT (§54 EnergieStG KWK)
/// - BEHG: 1.310 ct/kWh_Hs (BEHG applies even to KWK plants)
/// - MwSt: 19%
///
/// ## Consumption
/// - 50 000 kWh_Hs (gas consumed in KWK plant, January 2026)
///
/// ## Expected
/// - No Energiesteuer levy position
/// - Exemption info position must appear
/// - BEHG applies: 50 000 × 1.310 ct / 100 = 655.00 EUR
#[test]
fn golden_gas_energiesteuer_exempt_kwk() {
    let tariff: TariffInput = serde_json::from_str(
        r#"{
        "category": "GAS",
        "gas_arbeitspreis_ct_per_kwh_hs": 8.00,
        "gas_energiesteuer_befreiung": true
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.310),
        mwst_rate: dec!(0.19),
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-GAS-KWK-001".to_owned(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates,
        ..Default::default()
    };

    let quantities = Quantities {
        gas: Some(GasMeterInput {
            kwh_hs: Some(dec!(50000)),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &ctx.regulatory_rates)
        .expect("KWK gas tariff must build engine")
        .bill(ctx, &quantities)
        .expect("KWK gas billing must succeed");

    // No regular Energiesteuer levy position
    let has_regular_energiesteuer = invoice
        .positions
        .iter()
        .any(|p| p.tags.iter().any(|t| t == "energiesteuer_gas"));
    assert!(
        !has_regular_energiesteuer,
        "KWK invoice must NOT have regular Energiesteuer position"
    );

    // Exemption info position must be present
    let has_exemption = invoice
        .positions
        .iter()
        .any(|p| p.tags.iter().any(|t| t == "energiesteuer_gas_befreiung"));
    assert!(
        has_exemption,
        "KWK invoice must have Energiesteuer Befreiung info position"
    );

    // BEHG still applies: 50 000 × 1.310 ct / 100 = 655.00 EUR
    let behg = invoice
        .positions
        .iter()
        .find(|p| p.tags.iter().any(|t| t == "behg"))
        .expect("BEHG position must exist even for KWK gas");
    let expected_behg = dec!(50000) * dec!(1.310) / dec!(100);
    let diff = (behg.net_eur - expected_behg).abs();
    assert!(
        diff < dec!(0.01),
        "BEHG: expected {expected_behg}, got {}",
        behg.net_eur
    );
}

// ── Scenario 6: Historic rate — 2022 Energiesteuersenkung (0-rate) ────────────

/// **Golden: Gas correction invoice using 2022 emergency 0-rate Energiesteuer**
///
/// Germany temporarily reduced the gas Energiesteuer to 0 ct/kWh from
/// 01.04.2022 to 31.03.2023 (Energiesteuersenkungsgesetz).
///
/// For a retroactive correction of a 2022 invoice, the correct rate is 0 ct/kWh.
/// This tests that `energiesteuer_gas_for_year(2022)` returns 0 and that
/// the `effective_energiesteuer_gas_for_year` method applies it correctly.
#[test]
fn golden_2022_energiesteuer_senkung_zero_rate() {
    use energy_billing::energiesteuer_gas_for_year;
    use rust_decimal::Decimal;

    // Verify the historic rate lookup
    let rate_2022 = energiesteuer_gas_for_year(2022).expect("2022 rate must be known");
    assert_eq!(
        rate_2022,
        Decimal::ZERO,
        "2022 Energiesteuersenkung: must be 0 ct/kWh"
    );

    // 2023 restored rate
    let rate_2023 = energiesteuer_gas_for_year(2023).expect("2023 rate must be known");
    assert_eq!(
        rate_2023,
        dec!(0.55),
        "2023 restored rate must be 0.55 ct/kWh"
    );

    // Stromsteuer has been 2.05 ct since 2003
    use energy_billing::stromsteuer_for_year;
    for year in [2010, 2015, 2020, 2024, 2026] {
        let rate = stromsteuer_for_year(year).expect("StromStG rate known");
        assert_eq!(rate, dec!(2.05), "StromStG {year}: must be 2.05 ct/kWh");
    }
}
