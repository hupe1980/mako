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
//! 6. **Historic rates 2022** — heating gas stayed 0.55; the relief was 7 % USt
//! 7. **§41a enforcement** — dynamic tariff rejects non-iMSys metering mode
//! 8. **§40 Kilowattstundenpreis** — mandatory all-inclusive price per kWh
//! 9. **§41 mandatory fields** — rechnung_json contains all §41 EnWG fields
//! 10. **§42c Energy Sharing** — sharing credit reduces effective customer cost
//! 11. **Industrie §9b StromStG** — an Entlastung is billed in full and noted
//! 12. **§41a dynamic tariff** — a priced day to the cent, the weighted-average
//!     price, both DST days, and what does and does not block the run
//! 13. **Mieterstrom / GGV** — the §42a Abs. 4 ceiling, the §9 Abs. 1 Nr. 3
//!     exemption stated on the page, and the GGV PV/grid tax split
//!
//! ## Updating golden values
//!
//! If the calculation is intentionally changed (e.g., new BEHG rate), update
//! the expected values in this file. Each test documents the full calculation
//! path so the expected values can be verified by hand.

use energy_billing::{
    BillingContext, BillingPeriod, GasMeterInput, GridInput, InvoiceType, MeterInput,
    PositionCategory, Product, Quantities, RegulatoryRates,
};
use rust_decimal::dec;
use time::macros::date;

// ── Scenario 7 (here ordered first as a regression guard): §41a enforcement ──

/// **Golden: §41a Abs. 1 EnWG — dynamic tariff must be rejected for non-iMSys meter**
///
/// §41a Abs. 1 EnWG prohibits offering §41a dynamic tariffs to customers who
/// do not have an intelligent metering system (iMSys / Smart Meter Gateway).
///
/// When `dynamic_epex = true` AND `electricity.metering_mode = Slp`, the engine
/// must return `Err(BillingError::InvalidInput)` — not produce a partial invoice.
#[test]
fn sect41a_dynamic_tariff_rejects_non_imsys_metering_mode() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696012".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R41B-TEST-001".into(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };

    let tariff: Product = serde_json::from_str(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":"20"}"#,
    )
    .unwrap();

    // SLP metering mode — §41a violation
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
        "§41a: dynamic_epex + Slp must return Err, got Ok(invoice)"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("41a") || err_msg.contains("iMSys") || err_msg.contains("IMSYS"),
        "§41a error message must reference §41a or iMSys, got: {err_msg}"
    );

    // Validate also returns the error
    let warnings = tariff
        .build_engine(&GridInput::default(), &rates)
        .validate(&ctx, &quantities_slp);
    assert!(
        !warnings.is_empty(),
        "§41a: validate() must return at least one warning for SLP + dynamic_epex"
    );
    let has_error = warnings
        .iter()
        .any(|w| w.severity == WarningSeverity::Error);
    assert!(
        has_error,
        "§41a: at least one Error-severity warning expected"
    );

    // iMSys mode — must succeed. The interval series is what a dynamic tariff
    // is billed from, so a well-formed input carries one that matches the
    // meter: 300 kWh stated, 300 kWh delivered, priced.
    let mut prices = std::collections::HashMap::new();
    prices.insert(time::macros::datetime!(2026-01-01 13:00 UTC), dec!(25));
    let quantities_imsys = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(300),
            metering_mode: MeteringMode::Imsys,
            ..Default::default()
        }),
        dynamic_intervals: vec![energy_billing::DynamicInterval {
            timestamp_utc: time::macros::datetime!(2026-01-01 13:00 UTC),
            kwh: dec!(300),
        }],
        dynamic_epex_prices: prices,
        ..Default::default()
    };
    let result_imsys = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities_imsys);
    assert!(
        result_imsys.is_ok(),
        "§41a: dynamic_epex + Imsys must succeed, got: {:?}",
        result_imsys.err()
    );
    assert!(
        result_imsys.unwrap().warnings.is_empty(),
        "§41a: no warnings for valid iMSys + dynamic_epex combination"
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
        "arbeitspreis_ct_per_kwh": "28.50",
        "grundpreis_ct_per_day": "8.00"
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.17906516),
        mwst_rate: dec!(0.19),
        mwst_rate_reduced: dec!(0.07),
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-STROM-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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

/// **Golden: §13b reverse charge — supply to a Stromwiederverkäufer**
///
/// Same tariff and quantities as `golden_strom_slp_eintarif_jan_2026`, but the
/// customer is an electricity reseller (`reverse_charge = true`, §13b Abs. 2 Nr. 5
/// lit. b UStG). The net base is identical (100.24 EUR), but the supplier charges
/// **no** VAT — the recipient owes it — so `mwst_eur == 0`, `brutto == netto`, and
/// the supply positions carry the reverse-charge marker (EN 16931 `AE`).
#[test]
fn golden_strom_reverse_charge_13b_charges_no_vat() {
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": "28.50",
        "grundpreis_ct_per_day": "8.00"
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.17906516),
        mwst_rate: dec!(0.19),
        mwst_rate_reduced: dec!(0.07),
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-STROM-13B-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        reverse_charge: true, // §13b Abs. 2 Nr. 5 lit. b UStG — Stromwiederverkäufer
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

    // Net base is unchanged from the standard-rated golden (91.20 + 2.48 + 6.56).
    assert_eq!(
        invoice.netto_eur.round_dp(2),
        dec!(100.24),
        "Netto unchanged"
    );
    // But the supplier charges no VAT under §13b — the recipient owes it.
    assert_eq!(invoice.mwst_eur, dec!(0), "§13b: supplier charges no VAT");
    assert_eq!(
        invoice.brutto_eur.round_dp(2),
        dec!(100.24),
        "§13b: brutto == netto (no VAT added)"
    );
    // The supply positions carry the reverse-charge marker (EN 16931 `AE`).
    assert!(
        invoice.positions.iter().any(|p| p.is_reverse_charge()),
        "at least one supply position must be reverse-charge"
    );
}

// ── Scenario 2: Gas with levies ───────────────────────────────────────────────

/// **Golden: Gas invoice with Brennwert, Energiesteuer, BEHG CO₂, January 2026**
///
/// ## Tariff
/// - Arbeitspreis: 7.50 ct/kWh_Hs
/// - Grundpreis: 5.00 ct/day
/// - Energiesteuer: 0.55 ct/kWh (§2 EnergieStG)
/// - BEHG CO₂ (2026): 65 EUR/t × 0.18139464 kg/kWh_Hs ÷ 10 = 1.17906516 ct/kWh
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
/// BEHG CO₂:           500 × 1.17906516 ct = 5.8953258 EUR
/// Netto:              37.50 + 1.55 + 2.75 + 5.8953258 = 47.6953258 EUR
/// MwSt 19%:           47.6953258 × 0.19 = 9.0621119 EUR
/// Brutto:             ≈ 56.76 EUR
/// ```
#[test]
fn golden_gas_with_levies_jan_2026() {
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "GAS",
        "gas_arbeitspreis_ct_per_kwh_hs": "7.50",
        "gas_grundpreis_ct_per_day": "5.00"
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.17906516),
        mwst_rate: dec!(0.19),
        mwst_rate_reduced: dec!(0.07),
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-GAS-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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

    // BEHG: 500 × 0.0117906516 = 5.8953258
    let behg = invoice.total_by_tag("behg");
    assert!(
        (behg - dec!(5.8953258)).abs() < dec!(0.01),
        "BEHG golden: expected ~5.8953, got {}",
        behg
    );

    // Netto: ~47.6953
    assert!(
        (invoice.netto_eur - dec!(47.6953258)).abs() < dec!(0.05),
        "Gas netto golden: expected ~47.70, got {}",
        invoice.netto_eur
    );

    // Brutto: ~56.76
    assert!(
        (invoice.brutto_eur - dec!(56.7574377)).abs() < dec!(0.05),
        "Gas brutto golden: expected ~56.76, got {}",
        invoice.brutto_eur
    );
}

// ── Scenario 3: EEG Gutschrift ────────────────────────────────────────────────

/// **Golden: EEG feed-in Gutschrift (credit note), Kleinunternehmer, January 2026**
///
/// ## Context
/// LF issues a monthly Gutschrift to a PV plant operator who has elected the
/// Kleinunternehmerregelung (§19 UStG), so the Gutschrift carries 0 % USt. The
/// 0 % follows the operator's tax election — not the plant size (§12 Abs. 3 UStG
/// zero-rates the PV *system* supply, not the feed-in remuneration).
///
/// ## Tariff
/// - Einspeisevergütung: 8.20 ct/kWh (EEG 2023, ≤10 kWp)
/// - kleinunternehmer_19_ustg: true → 0 % MwSt (§19 UStG)
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
#[cfg(feature = "bo4e")]
#[test]
fn golden_eeg_gutschrift_kleinunternehmer_jan_2026() {
    use energy_billing::EegMeterInput;

    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "EEG",
        "eeg_verguetungssatz_ct_per_kwh": "8.20",
        "kleinunternehmer_19_ustg": true
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-EEG-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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

    // §19 UStG Kleinunternehmer → 0% on the feed-in Gutschrift
    assert_eq!(invoice.mwst_eur, dec!(0), "EEG ≤30 kWp: MwSt must be 0");

    // Brutto equals netto for 0% MwSt
    assert_eq!(
        invoice.brutto_eur.round_dp(2),
        dec!(22.96),
        "EEG brutto golden"
    );

    // Verify JSON includes the correct process label. BO4E Rechnungstyp has no
    // Gutschrift value, so the typed field stays absent and the label rides as
    // the "rechnungsart" ZusatzAttribut.
    let json = invoice.to_rechnung_json();
    assert!(
        json["rechnungstyp"].is_null(),
        "GUTSCHRIFT has no Rechnungstyp"
    );
    let rechnungsart = json["zusatzAttribute"]
        .as_array()
        .and_then(|attrs| attrs.iter().find(|a| a["name"] == "mako:rechnungsart"))
        .map(|a| a["wert"].clone())
        .unwrap_or(serde_json::Value::Null);
    assert_eq!(
        rechnungsart, "GUTSCHRIFT",
        "EEG credit note must be tagged rechnungsart GUTSCHRIFT"
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
        "arbeitspreis_ct_per_kwh": "24.00",
        "leistungspreis_strom_ct_per_kw_month": "4.50"
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.17906516),
        mwst_rate: dec!(0.19),
        mwst_rate_reduced: dec!(0.07),
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-RLM-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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
/// A CHP (KWK) plant operator buys gas. **§ 53a EnergieStG is a
/// Steuerentlastung**, applied for at the Hauptzollamt after the fact — the
/// supplier invoices the full 0,55 ct/kWh and says so. The old scenario
/// zero-rated it at supply under "§54 EnergieStG", which is a relief too, and
/// the invoice lost 275 EUR of Energiesteuer that the operator would then
/// reclaim a second time.
///
/// ## Tariff
/// - Gas Arbeitspreis: 8.00 ct/kWh_Hs
/// - Energiesteuer: Regelsatz 0.55 ct/kWh_Hs + § 53a Entlastungshinweis
/// - BEHG: 1.17906516 ct/kWh_Hs
/// - MwSt: 19%
///
/// ## Consumption
/// - 50 000 kWh_Hs (gas consumed in KWK plant, January 2026)
///
/// ## Expected
/// - Energiesteuer billed in full: 50 000 × 0.55 ct = 275.00 EUR
/// - One § 53a Entlastungshinweis naming that figure
/// - BEHG applies: 50 000 × 1.17906516 ct / 100 = 589.53 EUR
#[test]
fn golden_gas_kwk_is_billed_the_full_energiesteuer_with_a_53a_note() {
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "GAS",
        "gas_arbeitspreis_ct_per_kwh_hs": "8.00",
        "steuerentlastungen": ["ENERGIESTEUER53A"]
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.17906516),
        mwst_rate: dec!(0.19),
        mwst_rate_reduced: dec!(0.07),
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-GAS-KWK-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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

    // The levy is billed in full: 50 000 × 0.55 ct = 275.00 EUR
    let energiesteuer = invoice.total_by_tag("energiesteuer_gas");
    assert_eq!(
        energiesteuer.round_dp(2),
        dec!(275.00),
        "a § 53a Entlastung does not reduce what the supplier invoices"
    );
    assert!(
        !invoice
            .positions
            .iter()
            .any(|p| p.tags.iter().any(|t| t == "energiesteuer_gas_befreiung")),
        "§ 53a is not a Befreiung — no exemption notice may appear"
    );

    // …and the operator is told what to file, and on what.
    let hinweis: Vec<_> = invoice.positions_by_tag("steuerentlastung").collect();
    assert_eq!(hinweis.len(), 1);
    assert!(hinweis[0].description.contains("275.00"));
    assert_eq!(
        hinweis[0].legal_basis.as_deref(),
        Some("\u{a7} 53a EnergieStG")
    );

    // BEHG still applies: 50 000 × 1.17906516 ct / 100 = 589.53 EUR
    let behg = invoice
        .positions
        .iter()
        .find(|p| p.tags.iter().any(|t| t == "behg"))
        .expect("BEHG position must exist even for KWK gas");
    let expected_behg = dec!(50000) * dec!(1.17906516) / dec!(100);
    let diff = (behg.net_eur - expected_behg).abs();
    assert!(
        diff < dec!(0.01),
        "BEHG: expected {expected_behg}, got {}",
        behg.net_eur
    );
}

// ── Scenario 6: Historic rates — heating gas was never zero-rated ─────────────

/// **Golden: heating-gas Energiesteuer stayed 0.55 ct/kWh through 2022.**
///
/// The 2022 Energiesteuersenkungsgesetz (BGBl. I 2022 S. 810) reduced
/// **motor-fuel** rates (§2 Abs. 1 EnergieStG) for June–August 2022 only —
/// the "Tankrabatt". Heating gas (§2 Abs. 3 Nr. 4) was never reduced; the
/// actual gas reliefs were the Dezember-Soforthilfe (EWSG) and the USt cut
/// to 7 % from 01.10.2022 to 31.03.2024 (§28 Abs. 5/6 UStG).
///
/// A retroactive correction of a 2022 gas invoice therefore uses 0.55 ct/kWh
/// Energiesteuer and — for periods wholly inside the window — 7 % USt.
#[test]
fn golden_2022_heating_gas_energiesteuer_stays_055() {
    use energy_billing::energiesteuer_gas_for_year;

    // The heating-gas rate is constant through the crisis years.
    for year in [2021, 2022, 2023, 2024] {
        let rate = energiesteuer_gas_for_year(year).expect("rate must be known");
        assert_eq!(
            rate,
            dec!(0.55),
            "EnergieStG heating gas {year}: 0.55 ct/kWh — the 2022 Tankrabatt was fuels-only"
        );
    }

    // The real 2022/23 relief: 7 % USt on gas/Wärme (§28 Abs. 5/6 UStG).
    use energy_billing::mwst_rate_for_gas_waerme_period;
    use time::macros::date;
    assert_eq!(
        mwst_rate_for_gas_waerme_period(date!(2023 - 01 - 01), date!(2023 - 12 - 31)),
        Some(dec!(0.07)),
        "calendar year 2023 gas bills carry 7 % USt"
    );

    // Stromsteuer has been 2.05 ct since 2003
    use energy_billing::stromsteuer_for_year;
    for year in [2010, 2015, 2020, 2024, 2026] {
        let rate = stromsteuer_for_year(year).expect("StromStG rate known");
        assert_eq!(rate, dec!(2.05), "StromStG {year}: must be 2.05 ct/kWh");
    }
}

// ── §40 EnWG — Kilowattstundenpreis completeness ─────────────────────────────

/// §40 EnWG: electricity invoices must show the all-inclusive price per kWh.
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
        malo_id: "51238696012".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R40A-TEST-001".into(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"30.0","grundpreis_ct_per_day":"8.22"}"#,
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

    // §40: kilowattstundenpreis must be computable
    let kwh_preis = invoice
        .kilowattstundenpreis_brutto_ct(dec!(500))
        .expect("§40 kilowattstundenpreis must be Some for non-zero kWh");

    // With 30ct Arbeit + Stromsteuer + MwSt the all-in price must be > 30 ct
    assert!(
        kwh_preis > dec!(30.0),
        "§40 kilowattstundenpreis must include all charges, got {kwh_preis:.4} ct/kWh"
    );
    // And below 50 ct as a sanity bound
    assert!(
        kwh_preis < dec!(50.0),
        "§40 kilowattstundenpreis seems too high: {kwh_preis:.4} ct/kWh"
    );

    // §40 — must return None for zero kWh (avoid division by zero)
    assert!(
        invoice
            .kilowattstundenpreis_brutto_ct(Decimal::ZERO)
            .is_none(),
        "§40: kilowattstundenpreis must be None when kWh = 0"
    );
}

// ── §41 EnWG — Mandatory invoice fields ───────────────────────────────────────

/// §41 Abs. 1 EnWG requires specific mandatory fields on every energy invoice.
/// This test verifies that `to_rechnung_json()` includes all required fields.
#[cfg(feature = "bo4e")]
#[test]
fn sect41_rechnung_json_contains_mandatory_fields() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696012".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R41-TEST-001".into(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        zaehler_id: Some("1EFW1234567".into()), // §41 Abs. 1 Nr. 6 — Zählernummer
        nb_mp_id: Some("9900357000004".into()), // §41 Abs. 1 Nr. 5 — Netzbetreiber
        // §42 EnWG — structured, with the CO₂ figure Abs. 2 Nr. 2 requires.
        energiequellen: Some(energy_billing::EnergieQuellen {
            erneuerbar_pct: dec!(100),
            co2_g_per_kwh: dec!(0),
            hkn_certified: true,
            beschreibung: Some("100% Ökostrom (EE-Strom HKN-zertifiziert)".into()),
            ..Default::default()
        }),
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
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"30.0"}"#).unwrap();
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
        has_attr("mako:verbrauch_vorjahr"),
        "§41 Abs. 1 Nr. 3: verbrauchVorjahr ZusatzAttribut required when Verbrauchshistorie set"
    );

    // §42 EnWG — Energiemix
    assert!(
        has_attr("mako:stromkennzeichnung"),
        "§42 EnWG: Stromkennzeichnung ZusatzAttribut required"
    );

    // CustomerKategorie for ERP routing
    assert!(
        has_attr("mako:kundenkategorie"),
        "kundenkategorie ZusatzAttribut required for ERP routing"
    );

    // BillingRunId for audit trail
    assert!(
        has_attr("mako:billing_run_id"),
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
        malo_id: "51238696012".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R42C-TEST-001".into(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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
        r#"{"category":"SHARING","arbeitspreis_ct_per_kwh":"32.0","sharing_credit_ct_per_kwh":"20.0"}"#,
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
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"32.0"}"#).unwrap();
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

// ── Steuerbegünstigungen — Entlastung vs. Befreiung ───────────────────────────

/// A large industrial customer is **not** exempt from Stromsteuer. § 9b StromStG
/// is a Steuerentlastung it claims from the Hauptzollamt after being invoiced in
/// full (permanent at the EU minimum rate since 01.01.2026: 20,00 of the
/// 20,50 EUR/MWh, from 12 500 kWh a year).
///
/// Zero-rating the levy on a product boolean instead would leave the supplier's
/// own Stromsteueranmeldung short by 1 025 EUR on this one invoice — the
/// customer's later Entlastungsantrag does not repair that, it duplicates it.
#[test]
fn an_industrial_customer_is_billed_the_full_stromsteuer_and_told_about_9b() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696012".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "R-INDUSTRIE-001".into(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"18.0","steuerentlastungen":["STROMSTEUER9B"]}"#,
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

    // 50 000 kWh × 2,05 ct = 1 025,00 EUR, invoiced.
    assert_eq!(
        invoice.total_by_tag("stromsteuer").round_dp(2),
        dec!(1025.00)
    );
    assert!(
        !invoice
            .positions
            .iter()
            .any(|p| p.has_tag("stromsteuer_befreiung")),
        "§ 9b is not a Befreiung — no exemption notice may appear"
    );

    let hinweis: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Info && p.has_tag("steuerentlastung"))
        .collect();
    assert_eq!(hinweis.len(), 1);
    assert!(hinweis[0].description.contains("1025.00"));
}

// ── Outbound BO4E conformance ────────────────────────────────────────────────
//
// mako strict-decodes every BO4E document it *receives* (`ensure_known_enums`
// at each ingest boundary), and until now checked nothing about what it
// *emits*. That asymmetry mattered here more than anywhere: `to_rechnung()` is
// what reaches a counterparty.
//
// The catch-all is `rubo4e`'s own forward-compatibility choice, not the
// market's. go-bo4e's generated `UnmarshalJSON` returns `invalid <Enum> %q` for
// an unlisted value and has no catch-all variant; BO4E-python's enums are
// pydantic `StrEnum`s and raise a `ValidationError`. Both reject the **whole
// document**, so an out-of-schema enum in an emitted `Rechnung` is not an
// invoice the recipient reads imperfectly — it is one a Go or Python
// counterparty cannot parse at all.
//
// The convention this pins is already the crate's own: a concept BO4E does not
// model rides in a `ZusatzAttribut` rather than being forced into a typed
// field. `Rechnungstyp` has no Gutschrift value, so a credit note leaves
// `rechnungstyp` absent and labels itself via the `rechnungsart` attribute
// (see `golden_eeg_gutschrift_kleinunternehmer_jan_2026`).

/// Every `Rechnung` this crate emits must round-trip with no `Unknown` enum
/// anywhere in the tree.
#[test]
fn every_emitted_rechnung_is_valid_bo4e() {
    for (label, invoice) in emitted_invoices() {
        // The outbound gate: out-of-schema enums *and* the BO4E-stated rules —
        // net plus tax is gross, the Steuerbetrag breakdown sums to the tax
        // total, the positions sum to the net. mako refuses a received document
        // that breaks these, so it must not emit one either.
        let rechnung = invoice.to_rechnung();
        mako_markt::bo4e::ensure_conformant(&rechnung)
            .unwrap_or_else(|e| panic!("{label}: emitted a Rechnung mako would refuse: {e}"));

        // …and the JSON a caller stores is the same document, not a null.
        let json = invoice.to_rechnung_json();
        assert!(
            json.is_object(),
            "{label}: to_rechnung_json must yield the document, not {json}"
        );
        let round_tripped: rubo4e::current::Rechnung =
            serde_json::from_value(json).unwrap_or_else(|e| panic!("{label}: not a Rechnung: {e}"));
        mako_markt::bo4e::ensure_conformant(&round_tripped)
            .unwrap_or_else(|e| panic!("{label}: the stored JSON form would be refused: {e}"));
    }
}

/// One invoice per shape the crate can emit, so the guard above covers the
/// branches that differ in which BO4E enums they set: the commodity, and the
/// VAT category (`Steuerart::Ust` vs `Rcv`, the two `to_rechnung` can produce).
fn emitted_invoices() -> Vec<(&'static str, energy_billing::Invoice)> {
    let rates = RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(1.17906516),
        mwst_rate: dec!(0.19),
        mwst_rate_reduced: dec!(0.07),
    };

    let ctx = |nr: &str| BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: nr.to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };

    let strom: Product = serde_json::from_str(
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"28.50","grundpreis_ct_per_day":"8.00"}"#,
    )
    .expect("strom fixture");
    let gas: Product = serde_json::from_str(
        r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":"7.50","gas_grundpreis_ct_per_day":"5.00"}"#,
    )
    .expect("gas fixture");

    let strom_q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(320),
            ..Default::default()
        }),
        ..Default::default()
    };
    let gas_q = Quantities {
        gas: Some(GasMeterInput {
            kwh_hs: Some(dec!(1200)),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Reverse charge exercises the other `Steuerart` branch (§13b UStG).
    let mut rc_ctx = ctx("CONF-STROM-RCV");
    rc_ctx.reverse_charge = true;

    vec![
        (
            "strom",
            strom
                .build_engine(&GridInput::default(), &rates)
                .bill(ctx("CONF-STROM-001"), &strom_q)
                .expect("strom invoice"),
        ),
        (
            "strom reverse charge",
            strom
                .build_engine(&GridInput::default(), &rates)
                .bill(rc_ctx, &strom_q)
                .expect("reverse-charge invoice"),
        ),
        (
            "gas",
            gas.build_engine(&GridInput::default(), &rates)
                .bill(ctx("CONF-GAS-001"), &gas_q)
                .expect("gas invoice"),
        ),
    ]
}

// ── Scenario 12: §41a EnWG — a dynamic tariff, end to end ────────────────────

/// The full §41a invoice, computed by hand.
///
/// A ZVT customer on an iMSys, four quarter-hours of a January day:
///
/// | MTU (UTC) | kWh | EPEX ct/kWh | clamped into [0, 40] | + 3,0 Aufschlag | EUR |
/// |---|---|---|---|---|---|
/// | 11:00 | 0,500 | 8,00 | 8,00 | 11,00 | 0,05500 |
/// | 11:15 | 0,250 | −2,00 | **0,00** (floor) | 3,00 | 0,00750 |
/// | 11:30 | 1,000 | 45,00 | **40,00** (cap) | 43,00 | 0,43000 |
/// | 11:45 | 0,750 | 12,00 | 12,00 | 15,00 | 0,11250 |
/// | | **2,500** | | | | **0,60500** |
///
/// Then, on 2,5 kWh:
/// - NNE Arbeitspreis 7,50 ct → 0,18750 EUR
/// - Konzessionsabgabe 1,32 ct → 0,03300 EUR
/// - Stromsteuer 2,05 ct → 0,05125 EUR
/// - Grundpreis 30 ct/day × 31 days → 9,30 EUR
///
/// netto = 0,605 + 0,1875 + 0,033 + 0,05125 + 9,30 = **10,17675 EUR**
/// MwSt 19 % on that = 1,93 EUR (rounded per rate) → brutto **12,10675 EUR**
#[test]
fn golden_sect41a_dynamic_day_reconciles_to_the_cent() {
    use energy_billing::{DynamicInterval, MeteringMode};
    use std::collections::HashMap;

    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "dynamic_epex": true,
        "grundpreis_ct_per_day": "30.0",
        "dynamic_aufschlag_ct_per_kwh": "3.0",
        "dynamic_epex_floor_ct_kwh": "0.0",
        "dynamic_epex_cap_ct_kwh": "40.0"
    }"#,
    )
    .unwrap();

    let mtu = |m: u8| {
        time::macros::datetime!(2026-01-15 11:00 UTC) + time::Duration::minutes(i64::from(m))
    };
    let mut prices = HashMap::new();
    prices.insert(mtu(0), dec!(8.00));
    prices.insert(mtu(15), dec!(-2.00));
    prices.insert(mtu(30), dec!(45.00));
    prices.insert(mtu(45), dec!(12.00));

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-41A-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let grid = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(7.50)),
        ka_ct_per_kwh: Some(dec!(1.32)),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(2.5),
            metering_mode: MeteringMode::Imsys,
            ..Default::default()
        }),
        dynamic_intervals: vec![
            DynamicInterval {
                timestamp_utc: mtu(0),
                kwh: dec!(0.5),
            },
            DynamicInterval {
                timestamp_utc: mtu(15),
                kwh: dec!(0.25),
            },
            DynamicInterval {
                timestamp_utc: mtu(30),
                kwh: dec!(1.0),
            },
            DynamicInterval {
                timestamp_utc: mtu(45),
                kwh: dec!(0.75),
            },
        ],
        dynamic_epex_prices: prices,
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&grid, &rates)
        .bill(ctx, &quantities)
        .expect("a fully priced §41a day bills");
    invoice.assert_valid();

    // The energy leg, to the 10⁻⁵ EUR the engine works in.
    assert_eq!(invoice.total_by_tag("§41a").round_dp(5), dec!(0.60500));
    // …and each pass-through on the same 2,5 kWh.
    assert_eq!(
        invoice.total_by_tag("nne_arbeitspreis").round_dp(5),
        dec!(0.18750)
    );
    assert_eq!(
        invoice.total_by_tag("konzessionsabgabe").round_dp(5),
        dec!(0.03300)
    );
    assert_eq!(
        invoice.total_by_tag("stromsteuer").round_dp(5),
        dec!(0.05125)
    );
    assert_eq!(invoice.total_by_tag("grundpreis").round_dp(2), dec!(9.30));

    assert_eq!(invoice.netto_eur.round_dp(5), dec!(10.17675));
    assert_eq!(invoice.mwst_eur.round_dp(2), dec!(1.93));
    assert_eq!(invoice.brutto_eur.round_dp(5), dec!(12.10675));
}

/// The Arbeitspreis line states a **weighted-average** ct/kWh, and it has to be
/// the one the amount actually implies — a customer checking the invoice
/// divides the total by the kWh and expects the printed figure.
#[test]
fn golden_sect41a_average_price_matches_the_amount() {
    use energy_billing::{DynamicInterval, MeteringMode};
    use std::collections::HashMap;

    let tariff: Product = serde_json::from_str(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":"0",
             "dynamic_aufschlag_ct_per_kwh":"2.0"}"#,
    )
    .unwrap();
    // Deliberately uneven: 3 kWh at 10 ct and 1 kWh at 30 ct average to 15 ct,
    // not to the 20 ct an unweighted mean would give.
    let t0 = time::macros::datetime!(2026-01-15 08:00 UTC);
    let t1 = t0 + time::Duration::minutes(15);
    let mut prices = HashMap::new();
    prices.insert(t0, dec!(10.00));
    prices.insert(t1, dec!(30.00));

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(
            ctx,
            &Quantities {
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(4),
                    metering_mode: MeteringMode::Imsys,
                    ..Default::default()
                }),
                dynamic_intervals: vec![
                    DynamicInterval {
                        timestamp_utc: t0,
                        kwh: dec!(3),
                    },
                    DynamicInterval {
                        timestamp_utc: t1,
                        kwh: dec!(1),
                    },
                ],
                dynamic_epex_prices: prices,
                ..Default::default()
            },
        )
        .unwrap();

    let ap = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("§41a"))
        .expect("Arbeitspreis position");
    // (3 × 12 + 1 × 32) ct = 68 ct over 4 kWh = 17 ct/kWh.
    assert_eq!(ap.net_eur.round_dp(5), dec!(0.68000));
    assert_eq!(ap.quantity, dec!(4));
    assert_eq!(
        (ap.net_eur / ap.quantity * dec!(100)).round_dp(4),
        dec!(17.0000)
    );
    assert!(
        ap.description.contains("17.0000"),
        "the printed average must be the one the amount implies: {:?}",
        ap.description
    );
}

/// **DST.** CET and CEST are whole-hour offsets, so a local quarter-hour is
/// always a UTC quarter-hour and flooring in UTC needs no timezone conversion.
///
/// The days that prove it are the switches: 25 hours (100 MTUs) on the last
/// Sunday in October and 23 hours (92 MTUs) on the last Sunday in March. Both
/// are ordinary UTC sequences, and every interval must find its price.
#[test]
fn golden_sect41a_prices_every_mtu_of_a_dst_day() {
    use energy_billing::{DynamicInterval, MeteringMode, mtu_start};
    use std::collections::HashMap;

    // 25.10.2026 is the last Sunday in October — the 25-hour day.
    // 00:00 CEST = 22:00 UTC on the 24th; the day runs 100 quarter-hours.
    for (label, start, mtus) in [
        (
            "Oct (25 h)",
            time::macros::datetime!(2026-10-24 22:00 UTC),
            100_i64,
        ),
        (
            "Mar (23 h)",
            time::macros::datetime!(2026-03-28 23:00 UTC),
            92_i64,
        ),
    ] {
        let mut prices = HashMap::new();
        let mut intervals = Vec::new();
        for i in 0..mtus {
            let t = start + time::Duration::minutes(15 * i);
            prices.insert(t, dec!(10.00));
            // Offset the consumption timestamp inside its quarter-hour: the
            // lookup must floor to the MTU, not match the instant.
            intervals.push(DynamicInterval {
                timestamp_utc: t + time::Duration::seconds(437),
                kwh: dec!(1),
            });
            assert_eq!(mtu_start(t + time::Duration::seconds(437)), t, "{label}");
        }

        let tariff: Product = serde_json::from_str(
            r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":"0"}"#,
        )
        .unwrap();
        let rates = RegulatoryRates::default();
        let ctx = BillingContext {
            period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 12 - 31)).unwrap(),
            regulatory_rates: rates.clone(),
            ..Default::default()
        };
        let invoice = tariff
            .build_engine(&GridInput::default(), &rates)
            .bill(
                ctx,
                &Quantities {
                    electricity: Some(MeterInput {
                        arbeitsmenge_kwh: dec!(1) * rust_decimal::Decimal::from(mtus),
                        metering_mode: MeteringMode::Imsys,
                        ..Default::default()
                    }),
                    dynamic_intervals: intervals,
                    dynamic_epex_prices: prices,
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{label}: every MTU must be priced: {e}"));

        // Every quarter-hour reached the bill: mtus kWh × 10 ct.
        let expected = rust_decimal::Decimal::from(mtus) / dec!(10);
        assert_eq!(
            invoice.total_by_tag("§41a").round_dp(5),
            expected,
            "{label}"
        );
    }
}

/// A missing price in a **zero-consumption** quarter-hour costs nothing and is
/// documented as harmless — so it must not block the run, while the very same
/// gap in an interval that carries energy must.
#[test]
fn golden_sect41a_only_an_unpriced_kwh_blocks_the_run() {
    use energy_billing::{DynamicInterval, MeteringMode};
    use std::collections::HashMap;

    let tariff: Product = serde_json::from_str(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":"0"}"#,
    )
    .unwrap();
    let t0 = time::macros::datetime!(2026-01-15 08:00 UTC);
    let t1 = t0 + time::Duration::minutes(15);
    let mut prices = HashMap::new();
    prices.insert(t0, dec!(10.00)); // t1 deliberately unpriced

    let rates = RegulatoryRates::default();
    let run = |kwh_at_t1| {
        let ctx = BillingContext {
            period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
            regulatory_rates: rates.clone(),
            ..Default::default()
        };
        tariff.build_engine(&GridInput::default(), &rates).bill(
            ctx,
            &Quantities {
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(5),
                    metering_mode: MeteringMode::Imsys,
                    ..Default::default()
                }),
                dynamic_intervals: vec![
                    DynamicInterval {
                        timestamp_utc: t0,
                        kwh: dec!(5),
                    },
                    DynamicInterval {
                        timestamp_utc: t1,
                        kwh: kwh_at_t1,
                    },
                ],
                dynamic_epex_prices: prices.clone(),
                ..Default::default()
            },
        )
    };

    // Nothing consumed in the unpriced quarter-hour: bill the rest.
    let ok = run(dec!(0)).expect("an unpriced empty interval is harmless");
    assert_eq!(ok.total_by_tag("§41a").round_dp(5), dec!(0.50000));

    // One watt-hour in it, and the invoice would silently under-bill.
    let err = run(dec!(0.001)).expect_err("consumption without a price cannot be billed");
    assert!(
        err.to_string().contains("SECT41A_MISSING_EPEX_PRICES"),
        "{err}"
    );
}

// ── Scenario 13: Mieterstrom (§ 42a EnWG) ────────────────────────────────────

/// The full Mieterstrom invoice, computed by hand.
///
/// A tenant in a building with a 60 kWp rooftop plant draws 250 kWh of solar in
/// the month. The Mieterstrompreis is 24 ct/kWh; the local Grundversorgung
/// Arbeitspreis is 30 ct/kWh, so **§ 42a Abs. 4 EnWG** caps the price at 27 ct
/// and 24 ct is lawful.
///
/// - Arbeitspreis  250 kWh × 24 ct = **60,00 EUR**
/// - Stromsteuer   **none** — § 9 Abs. 1 Nr. 3 lit. b StromStG: the plant is
///   under 2 MW and the operator delivers to Letztverbraucher drawing the
///   electricity im räumlichen Zusammenhang. The invoice *states* the ground.
/// - MwSt 19 % on 60,00 = **11,40 EUR** → brutto **71,40 EUR**
///
/// The Mieterstromzuschlag (§ 21 Abs. 3 EEG 2023) appears nowhere: it is the
/// Anlagenbetreiber's claim against the Netzbetreiber, settled by `einsd`.
#[test]
fn golden_mieterstrom_invoice_reconciles_and_states_its_ceiling() {
    use energy_billing::SolarMeterInput;

    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "SOLAR",
        "solar_arbeitspreis_ct_per_kwh": "24.0",
        "grundversorgung_arbeitspreis_ct_per_kwh": "30.0",
        "anlage_kwp": "60.0"
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLDEN-MS-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(
            ctx,
            &Quantities {
                solar: Some(SolarMeterInput {
                    eigenverbrauch_kwh: dec!(250),
                }),
                ..Default::default()
            },
        )
        .expect("24 ct is inside the § 42a Abs. 4 ceiling");
    invoice.assert_valid();

    assert_eq!(
        invoice.total_by_tag("arbeitspreis").round_dp(2),
        dec!(60.00)
    );
    assert_eq!(invoice.netto_eur.round_dp(2), dec!(60.00));
    assert_eq!(invoice.mwst_eur.round_dp(2), dec!(11.40));
    assert_eq!(invoice.brutto_eur.round_dp(2), dec!(71.40));

    // No Stromsteuer — and the ground is on the page, not merely absent.
    assert_eq!(invoice.total_by_tag("stromsteuer"), dec!(0));
    let exemption = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("stromsteuer_befreiung"))
        .expect("§ 9 Abs. 1 Nr. 3 StromStG must be stated");
    assert_eq!(
        exemption.legal_basis.as_deref(),
        Some("§ 9 Abs. 1 Nr. 3 StromStG")
    );
    assert_eq!(exemption.net_eur, dec!(0));

    // The § 42a Abs. 4 ceiling is stated too, so a tenant can check it.
    let cap = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("preisobergrenze"))
        .expect("the 90 % ceiling belongs on the page");
    assert_eq!(cap.legal_basis.as_deref(), Some("§ 42a Abs. 4 EnWG"));
    assert!(cap.description.contains("27.0000"), "{}", cap.description);

    // The § 21 Abs. 3 EEG Zuschlag is somebody else's claim.
    assert!(
        !invoice
            .positions
            .iter()
            .any(|p| p.description.contains("Zuschlag")),
        "the Mieterstromzuschlag is settled by einsd, never billed to the tenant"
    );
}

/// **§ 42a Abs. 4 EnWG is a ceiling, and the boundary is exact.**
///
/// 90 % of 30 ct is 27 ct. At 27,00 the invoice bills; one hundredth of a cent
/// above it there is no lawful invoice to issue, so the run is refused rather
/// than producing one an operator would have to unwind.
#[test]
fn golden_mieterstrom_ceiling_is_exact_at_ninety_percent() {
    use energy_billing::SolarMeterInput;

    let rates = RegulatoryRates::default();
    let bill_at = |ms_ct: &str| {
        let tariff: Product = serde_json::from_str(&format!(
            r#"{{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":"{ms_ct}",
                 "grundversorgung_arbeitspreis_ct_per_kwh":"30.0"}}"#
        ))
        .unwrap();
        let ctx = BillingContext {
            period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
            regulatory_rates: rates.clone(),
            ..Default::default()
        };
        tariff.build_engine(&GridInput::default(), &rates).bill(
            ctx,
            &Quantities {
                solar: Some(SolarMeterInput {
                    eigenverbrauch_kwh: dec!(100),
                }),
                ..Default::default()
            },
        )
    };

    assert!(bill_at("26.99").is_ok(), "below the ceiling");
    assert!(bill_at("27.00").is_ok(), "exactly at the ceiling is lawful");
    let err = bill_at("27.01").expect_err("above the ceiling there is no lawful invoice");
    assert!(
        err.to_string()
            .contains("MIETERSTROM_UEBER_90PCT_GRUNDVERSORGUNG"),
        "{err}"
    );
}

/// Without a stated Grundversorgungstarif there is nothing to cap against, so
/// the engine bills what it was told — the ceiling is a check on a comparison
/// the operator supplies, not a figure the engine can invent.
#[test]
fn golden_mieterstrom_without_a_reference_tariff_bills_unchecked() {
    use energy_billing::SolarMeterInput;

    let tariff: Product =
        serde_json::from_str(r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":"35.0"}"#)
            .unwrap();
    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(
            ctx,
            &Quantities {
                solar: Some(SolarMeterInput {
                    eigenverbrauch_kwh: dec!(100),
                }),
                ..Default::default()
            },
        )
        .expect("no reference tariff, no comparison");
    assert_eq!(invoice.netto_eur.round_dp(2), dec!(35.00));
    assert!(
        !invoice
            .positions
            .iter()
            .any(|p| p.has_tag("preisobergrenze")),
        "nothing to state without a Grundversorgungstarif"
    );
}

/// A Mieterstrom supply that does **not** qualify for § 9 Abs. 1 Nr. 3 — a
/// plant over 2 MW, or delivery beyond the räumlicher Zusammenhang — is taxed
/// like any other supply, and the product says so.
#[test]
fn golden_mieterstrom_outside_the_exemption_is_taxed() {
    use energy_billing::SolarMeterInput;

    let tariff: Product = serde_json::from_str(
        r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":"24.0",
             "stromsteuer_tarif":{"art":"REGEL"}}"#,
    )
    .unwrap();
    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(
            ctx,
            &Quantities {
                solar: Some(SolarMeterInput {
                    eigenverbrauch_kwh: dec!(250),
                }),
                ..Default::default()
            },
        )
        .unwrap();
    // 250 kWh × 2,05 ct = 5,125 EUR of Stromsteuer, on top of the 60,00 supply.
    assert_eq!(
        invoice.total_by_tag("stromsteuer").round_dp(5),
        dec!(5.12500)
    );
    assert_eq!(invoice.netto_eur.round_dp(5), dec!(65.12500));
    assert_eq!(invoice.positions_by_tag("stromsteuer_befreiung").count(), 0);
}

/// **§ 42b EnWG GGV** — the hybrid split, computed by hand.
///
/// A participant consumes 400 kWh in the month and is allocated 300 kWh of the
/// building's PV. The remaining 100 kWh comes off the grid.
///
/// - PV portion    300 kWh × 24 ct = 72,00 EUR, less the 3 ct GGV-Rabatt
///   (300 × 3 ct = 9,00) = **63,00 EUR**, no Stromsteuer (§ 9 Abs. 1 Nr. 3)
/// - Grid portion  100 kWh × 32 ct = **32,00 EUR**, plus Stromsteuer
///   100 × 2,05 ct = **2,05 EUR** — grid electricity is taxed
///
/// netto = 63,00 + 32,00 + 2,05 = **97,05 EUR**
#[test]
fn golden_ggv_splits_pv_and_grid_and_taxes_only_the_grid() {
    use energy_billing::GgvSolarInput;

    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "SOLAR",
        "solar_arbeitspreis_ct_per_kwh": "24.0",
        "arbeitspreis_ct_per_kwh": "32.0",
        "gemeinschaft_rabatt_ct_per_kwh": "3.0"
    }"#,
    )
    .unwrap();
    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        rechnungsnummer: "GOLDEN-GGV-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(
            ctx,
            &Quantities {
                ggv_solar: Some(GgvSolarInput {
                    pv_allocated_kwh: dec!(300),
                    actual_consumption_kwh: dec!(400),
                }),
                ..Default::default()
            },
        )
        .unwrap();
    invoice.assert_valid();

    assert_eq!(invoice.total_by_tag("ggv_pv").round_dp(2), dec!(63.00));
    assert_eq!(invoice.total_by_tag("ggv_grid").round_dp(2), dec!(34.05));
    assert_eq!(invoice.netto_eur.round_dp(2), dec!(97.05));

    // The PV portion is exempt and says so; the grid portion is taxed.
    assert_eq!(
        invoice.total_by_tag("stromsteuer").round_dp(5),
        dec!(2.05000)
    );
    assert_eq!(invoice.positions_by_tag("stromsteuer_befreiung").count(), 1);

    // …and the coverage ratio a participant checks their allocation against.
    let cov = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("ggv_coverage"))
        .expect("coverage line");
    assert!(cov.description.contains("75"), "{}", cov.description);
}

/// A **year** of hourly intervals still reconciles exactly.
///
/// `billing::DynamicPricing` accumulates the interval products as exact
/// `Decimal` and reduces once at the end. Reducing each product to five
/// decimals before adding instead accumulates a bias that a single billed day
/// cannot show — the golden day above is 96 quarter-hours, and the drift only
/// becomes visible over thousands of intervals.
///
/// So this bills 8 760 hourly intervals against prices that do not divide
/// cleanly, and asserts the total against the sum computed the exact way. A
/// re-introduced per-product rounding fails here and nowhere else.
#[test]
fn golden_sect41a_a_year_of_intervals_does_not_drift() {
    use energy_billing::{DynamicInterval, MeteringMode};
    use rust_decimal::Decimal;
    use std::collections::HashMap;

    let tariff: Product = serde_json::from_str(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":"0.0"}"#,
    )
    .unwrap();

    let start = time::macros::datetime!(2026-01-01 00:00 UTC);
    let hours = 8_760_i64;

    // Quantities and prices chosen so neither the product nor the average is
    // representable in five decimals: 1/3 kWh at a price that cycles through
    // thirds of a cent.
    let mut intervals = Vec::with_capacity(hours as usize);
    let mut prices = HashMap::new();
    let mut expected_net = Decimal::ZERO;
    for h in 0..hours {
        let at = start + time::Duration::hours(h);
        let kwh = dec!(0.333);
        let price_ct = dec!(7.777) + Decimal::from(h % 13) * dec!(0.011);
        intervals.push(DynamicInterval {
            timestamp_utc: at,
            kwh,
        });
        prices.insert(at, price_ct);
        expected_net += kwh * price_ct / dec!(100);
    }

    let (f, t) = (date!(2026 - 01 - 01), date!(2026 - 12 - 31));
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLD-41A-YEAR".to_owned(),
        period: BillingPeriod::new(f, t).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: RegulatoryRates::default(),
        ..Default::default()
    };
    let q = Quantities {
        dynamic_intervals: intervals,
        dynamic_epex_prices: prices,
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(0.333) * Decimal::from(hours),
            metering_mode: MeteringMode::Imsys,
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &RegulatoryRates::default())
        .bill(ctx, &q)
        .expect("a full year of priced intervals bills");

    // The energy line, against the exact sum. Two cents of tolerance covers the
    // single documented rounding to the cent, and nothing else.
    let billed = invoice.total_by_tag("§41a");
    let drift = (billed - expected_net).abs();
    assert!(
        drift < dec!(0.02),
        "a year of intervals drifted by {drift} EUR (billed {billed}, exact {expected_net}) \
         — the per-interval products are being rounded before they are summed"
    );
}

/// **PEPPOL-EN16931-R120**: a line's stated unit price must multiply out to its
/// amount, within ±0.02.
///
/// The §41a Arbeitspreis states a *weighted average* price, which is a rounded
/// figure, against the exact sum of the priced intervals. Rounding the displayed
/// price is right — a customer reads it — but the further it is rounded and the
/// more energy it multiplies, the further the product drifts from the amount
/// beside it. At a year's consumption that drift is what a PEPPOL receiver
/// rejects on.
#[test]
fn golden_sect41a_price_times_quantity_matches_the_amount() {
    use energy_billing::{DynamicInterval, MeteringMode, PositionCategory};
    use rust_decimal::Decimal;
    use std::collections::HashMap;

    let tariff: Product = serde_json::from_str(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":"0.0"}"#,
    )
    .unwrap();

    let start = time::macros::datetime!(2026-01-01 00:00 UTC);
    let hours = 8_760_i64;
    let mut intervals = Vec::with_capacity(hours as usize);
    let mut prices = HashMap::new();
    for h in 0..hours {
        let at = start + time::Duration::hours(h);
        intervals.push(DynamicInterval {
            timestamp_utc: at,
            kwh: dec!(2000.0),
        });
        prices.insert(at, dec!(7.777) + Decimal::from(h % 13) * dec!(0.011));
    }

    let (f, t) = (date!(2026 - 01 - 01), date!(2026 - 12 - 31));
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "GOLD-R120".to_owned(),
        period: BillingPeriod::new(f, t).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: RegulatoryRates::default(),
        ..Default::default()
    };
    let q = Quantities {
        dynamic_intervals: intervals,
        dynamic_epex_prices: prices,
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(2000.0) * Decimal::from(hours),
            metering_mode: MeteringMode::Imsys,
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &RegulatoryRates::default())
        .bill(ctx, &q)
        .expect("bills");

    for p in invoice
        .positions
        .iter()
        .filter(|p| p.category != PositionCategory::Info && p.quantity != Decimal::ZERO)
    {
        let product = p.quantity * p.unit_price_eur;
        let drift = (product - p.net_eur).abs();
        assert!(
            drift <= dec!(0.02),
            "PEPPOL-EN16931-R120: {:?} states {} x {} = {} but its amount is {} \
             (drift {drift})",
            p.description,
            p.quantity,
            p.unit_price_eur,
            product,
            p.net_eur
        );
    }
}
