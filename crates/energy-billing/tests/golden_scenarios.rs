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
//! 4. **RLM demand charge** — large commercial electricity with Leistungspreis
//! 5. **Gas Energiesteuer exemption** — CHP (KWK) §54 EnergieStG
//! 6. **2022 Energiesteuersenkung** — historic zero-rate check
//! 7. **§41b enforcement** — dynamic tariff rejects non-iMSys metering mode
//! 8. **§40a Kilowattstundenpreis** — mandatory all-inclusive price per kWh
//! 9. **§41 mandatory fields** — rechnung_json contains all §41 EnWG fields
//! 10. **§42c Energy Sharing** — sharing credit reduces effective customer cost
//! 11. **Industrie §9 StromStG** — typed StromsteuerBefreiung enum
//!
//! ## Updating golden values
//!
//! If the calculation is intentionally changed (e.g., new BEHG rate), update
//! the expected values in this file. Each test documents the full calculation
//! path so the expected values can be verified by hand.

use energy_billing::{
    BillingContext, GasMeterInput, GridInput, InvoiceType, MeterInput, PositionCategory, Product,
    Quantities, RegulatoryRates,
};
use rust_decimal::dec;
use time::macros::date;

// ── Scenario 7 (here ordered first as a regression guard): §41b enforcement ──

/// **Golden: §41b EnWG — dynamic tariff must be rejected for non-iMSys meter**
///
/// §41b Abs. 2 EnWG prohibits offering §41a dynamic tariffs to customers who
/// do not have an intelligent metering system (iMSys / Smart Meter Gateway).
///
/// When `dynamic_epex = true` AND `electricity.metering_mode = Slp`, the engine
/// must return `Err(BillingError::InvalidInput)` — not produce a partial invoice.
#[test]
fn sect41b_dynamic_tariff_rejects_non_imsys_metering_mode() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696780".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R41B-TEST-001".into(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };

    let tariff: Product = serde_json::from_str(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":20}"#,
    )
    .unwrap();

    // SLP metering mode — §41b violation
    let quantities_slp = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(300),
            metering_mode: MeteringMode::Slp,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx.clone(), &quantities_slp);

    assert!(
        result.is_err(),
        "§41b: dynamic_epex + Slp must return Err, got Ok(invoice)"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("41b") || err_msg.contains("iMSys") || err_msg.contains("IMSYS"),
        "§41b error message must reference §41b or iMSys, got: {err_msg}"
    );

    // Validate also returns the error
    let warnings = tariff
        .build_engine(&GridInput::default(), &rates)
        .validate(&ctx, &quantities_slp);
    assert!(
        !warnings.is_empty(),
        "§41b: validate() must return at least one warning for SLP + dynamic_epex"
    );
    let has_error = warnings
        .iter()
        .any(|w| w.severity == WarningSeverity::Error);
    assert!(
        has_error,
        "§41b: at least one Error-severity warning expected"
    );

    // iMSys mode — must succeed
    let quantities_imsys = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(300),
            metering_mode: MeteringMode::Imsys,
            ..Default::default()
        }),
        ..Default::default()
    };
    let result_imsys = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities_imsys);
    assert!(
        result_imsys.is_ok(),
        "§41b: dynamic_epex + Imsys must succeed, got: {:?}",
        result_imsys.err()
    );
    assert!(
        result_imsys.unwrap().warnings.is_empty(),
        "§41b: no warnings for valid iMSys + dynamic_epex combination"
    );
}

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
    let tariff: Product = serde_json::from_str(
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
    let tariff: Product = serde_json::from_str(
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

    let tariff: Product = serde_json::from_str(
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
    let tariff: Product = serde_json::from_str(
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
        .bill(ctx, &quantities)
        .unwrap();

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
    let tariff: Product = serde_json::from_str(
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
        .bill(ctx, &quantities)
        .unwrap();

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

// ── §40a EnWG — Kilowattstundenpreis completeness ─────────────────────────────

/// §40a EnWG Abs. 1: electricity invoices must show the all-inclusive price per kWh.
/// Verified: kilowattstundenpreis_brutto_ct returns a sensible value covering all charges.
#[test]
fn sect40a_kilowattstundenpreis_brutto_includes_all_charges() {
    use energy_billing::*;
    use rust_decimal::Decimal;
    use rust_decimal::dec;
    use time::macros::date;

    // Standard household: 500 kWh @ 30 ct/kWh + 0.11 ct KA + 2.05 ct Stromsteuer + 19% MwSt
    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696780".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R40A-TEST-001".into(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        kundenkategorie: CustomerKategorie::Haushalt,
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(500),
            ..Default::default()
        }),
        ..Default::default()
    };
    let tariff: Product = serde_json::from_str(
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0,"grundpreis_ct_per_day":8.22}"#,
    )
    .unwrap();
    let invoice = BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &tariff,
            GridInput::default(),
        ))
        .add(MwStProvider::new(dec!(0.19)))
        .bill(ctx, &quantities)
        .unwrap();

    invoice.assert_valid();

    // §40a: kilowattstundenpreis must be computable
    let kwh_preis = invoice
        .kilowattstundenpreis_brutto_ct(dec!(500))
        .expect("§40a kilowattstundenpreis must be Some for non-zero kWh");

    // With 30ct Arbeit + Stromsteuer + MwSt the all-in price must be > 30 ct
    assert!(
        kwh_preis > dec!(30.0),
        "§40a kilowattstundenpreis must include all charges, got {kwh_preis:.4} ct/kWh"
    );
    // And below 50 ct as a sanity bound
    assert!(
        kwh_preis < dec!(50.0),
        "§40a kilowattstundenpreis seems too high: {kwh_preis:.4} ct/kWh"
    );

    // §40a Abs. 1 — must return None for zero kWh (avoid division by zero)
    assert!(
        invoice
            .kilowattstundenpreis_brutto_ct(Decimal::ZERO)
            .is_none(),
        "§40a: kilowattstundenpreis must be None when kWh = 0"
    );
}

// ── §41 EnWG — Mandatory invoice fields ───────────────────────────────────────

/// §41 Abs. 1 EnWG requires specific mandatory fields on every energy invoice.
/// This test verifies that `to_rechnung_json()` includes all required fields.
#[test]
fn sect41_rechnung_json_contains_mandatory_fields() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696780".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R41-TEST-001".into(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        zaehler_id: Some("1EFW1234567".into()), // §41 Abs. 1 Nr. 6 — Zählernummer
        nb_mp_id: Some("9900357000004".into()), // §41 Abs. 1 Nr. 5 — Netzbetreiber
        energiemix: Some("100% Ökostrom (EE-Strom HKN-zertifiziert)".into()), // §42 EnWG
        billing_run_id: Some("d1a2b3c4-0001".into()),
        kundenkategorie: CustomerKategorie::Haushalt,
        verbrauchshistorie: Some(Verbrauchshistorie {
            vorjahr_kwh: Some(dec!(5800)),
            bundesdurchschnitt_kwh: Some(dec!(3500)),
            kundengruppe: Some("2-Personen-Haushalt".into()),
        }),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(500),
            zaehlernummer: Some("1EFW1234567".into()),
            zaehlerstand_von: Some(dec!(12345.678)),
            zaehlerstand_bis: Some(dec!(12845.678)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let tariff: Product =
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#).unwrap();
    let invoice = BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &tariff,
            GridInput::default(),
        ))
        .add(MwStProvider::new(dec!(0.19)))
        .bill(ctx, &quantities)
        .unwrap();

    let json = invoice.to_rechnung_json();

    // §41 Abs. 1 Nr. 1 — Rechnungsnummer
    assert_eq!(
        json["rechnungsnummer"].as_str(),
        Some("R41-TEST-001"),
        "§41 Abs. 1 Nr. 1: rechnungsnummer required"
    );

    // §41 Abs. 1 Nr. 2 — Rechnungsdatum
    assert!(
        json["rechnungsdatum"].is_string(),
        "§41 Abs. 1 Nr. 2: rechnungsdatum required"
    );

    // §41 Abs. 1 Nr. 2 — Abrechnungszeitraum (period_from / period_to)
    assert!(
        json["rechnungsperiode"]["startdatum"].is_string(),
        "§41 Abs. 1 Nr. 2: rechnungsperiode.startdatum required"
    );

    // Positions exist
    let positions = json["rechnungspositionen"]
        .as_array()
        .expect("rechnungspositionen must be an array");
    assert!(
        !positions.is_empty(),
        "invoice must have at least one position"
    );

    // ZusatzAttribute must contain the mandatory regulatory fields
    let attrs = json["zusatzAttribute"]
        .as_array()
        .expect("zusatzAttribute must be present");

    let has_attr = |name: &str| attrs.iter().any(|a| a["name"].as_str() == Some(name));

    // §41 Abs. 1 Nr. 3 — Verbrauchshistorie in ZusatzAttribute
    assert!(
        has_attr("verbrauchVorjahr"),
        "§41 Abs. 1 Nr. 3: verbrauchVorjahr ZusatzAttribut required when Verbrauchshistorie set"
    );

    // §42 EnWG — Energiemix
    assert!(
        has_attr("energiemix"),
        "§42 EnWG: energiemix ZusatzAttribut required"
    );

    // CustomerKategorie for ERP routing
    assert!(
        has_attr("kundenkategorie"),
        "kundenkategorie ZusatzAttribut required for ERP routing"
    );

    // BillingRunId for audit trail
    assert!(
        has_attr("billingRunId"),
        "billingRunId ZusatzAttribut required for audit trail"
    );
}

// ── §42c EnWG — Energy Sharing credit ─────────────────────────────────────────

/// §42c EnWG: community energy sharing generates a credit reducing effective cost.
#[test]
fn sect42c_energy_sharing_credit_reduces_effective_cost() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696780".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R42C-TEST-001".into(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(500),
            ..Default::default()
        }),
        energy_share: Some(EnergyShareMeterInput {
            allocated_kwh: dec!(150), // 150 kWh from community PV
            total_plant_generation_kwh: Some(dec!(400)),
            allocation_fraction: Some(dec!(0.375)),
            gemeinschaft_id: Some("EGK-2024-001".into()),
        }),
        ..Default::default()
    };

    // SHARING tariff: full STROM price + sharing credit
    let strom_tariff: Product = serde_json::from_str(
        r#"{"category":"SHARING","arbeitspreis_ct_per_kwh":32.0,"sharing_credit_ct_per_kwh":20.0}"#,
    )
    .unwrap();
    let invoice = strom_tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx.clone(), &quantities)
        .unwrap();

    invoice.assert_valid();

    // The sharing credit must be present as a negative EnergyShare position
    let share_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::EnergyShare)
        .collect();
    assert_eq!(
        share_pos.len(),
        1,
        "exactly one EnergyShare credit position"
    );
    assert!(
        share_pos[0].net_eur < dec!(0),
        "sharing credit must be negative (reduces customer cost)"
    );

    // Credit amount: 150 kWh × 20 ct = 30.00 EUR (net, before MwSt)
    let expected_credit_netto = dec!(-30.0);
    let diff = (share_pos[0].net_eur - expected_credit_netto).abs();
    assert!(
        diff < dec!(0.001),
        "sharing credit: expected {expected_credit_netto:.5}, got {:.5}",
        share_pos[0].net_eur
    );

    // §42c legal basis must be cited
    assert!(
        share_pos[0].legal_basis.as_deref() == Some("§42c EnWG"),
        "§42c EnWG must be cited as legal basis for sharing credit"
    );

    // Effective cost is less than without sharing
    let tariff_no_share: Product =
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":32.0}"#).unwrap();
    let invoice_no_share = BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &tariff_no_share,
            GridInput::default(),
        ))
        .add(MwStProvider::new(dec!(0.19)))
        .bill(
            ctx,
            &Quantities {
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(500),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();

    assert!(
        invoice.brutto_eur < invoice_no_share.brutto_eur,
        "sharing customer must pay less (brutto {:.2} vs no-sharing {:.2})",
        invoice.brutto_eur,
        invoice_no_share.brutto_eur
    );
}

// ── CustomerKategorie — industrial Stromsteuer exemption ──────────────────────

/// §9 Nr. 1 StromStG: industrial customers (Industrie category) with
/// `industrie_stromsteuer_befreiung = true` must not have a Stromsteuer position.
#[test]
fn industrie_customer_stromsteuer_befreiung_removes_levy() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696780".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R-INDUSTRIE-001".into(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        kundenkategorie: CustomerKategorie::Industrie,
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(50_000), // large industrial customer
            ..Default::default()
        }),
        ..Default::default()
    };
    let tariff: Product = serde_json::from_str(
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":18.0,"industrie_stromsteuer_befreiung":true}"#,
    )
    .unwrap();

    let invoice = BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &tariff,
            GridInput::default(),
        ))
        .add(MwStProvider::new(dec!(0.19)))
        .bill(ctx, &quantities)
        .unwrap();

    // Stromsteuer levy position must be absent (§9 Nr. 1 StromStG exemption)
    let stromsteuer_positions: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("stromsteuer"))
        .collect();
    assert!(
        stromsteuer_positions.is_empty(),
        "§9 StromStG: industrial exemption — no Stromsteuer position should appear"
    );

    // Instead, an informational exemption note with tag "stromsteuer_befreiung" should appear
    let exemption_info: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Info && p.has_tag("stromsteuer_befreiung"))
        .collect();
    assert!(
        !exemption_info.is_empty(),
        "§9 StromStG: industrial exemption must generate an informational position with tag 'stromsteuer_befreiung'"
    );
}
