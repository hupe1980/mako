//! Regulatory showcase tests for the `eeg-billing` crate.
//!
//! Each test corresponds to a specific paragraph of German energy law.
//! These tests serve as executable documentation of the regulatory requirements.
//!
//! Run: `cargo test -p eeg-billing --test regulatory_showcase`
//!
//! ## Legal sources
//!
//! - **EEG 2023**: Erneuerbare-Energien-Gesetz (BGBl. I Nr. 28, 2023)
//!   [§§20–50 feed-in settlement, §25 sanctions, §27 negative prices]
//! - **KWKG 2023**: Kraft-Wärme-Kopplungsgesetz (BGBl. I Nr. 59, 2023)
//!   [§7 KWK-Zuschlag rates, §8 Förderdauer]
//! - **BNetzA AHB / Strom**: quarterly Vergütungssätze publications
//!
//! All monetary amounts in EUR. All rates in ct/kWh. No floating-point money.

use eeg_billing::{
    SettleInput, SettlementModel, SettlementStatus, calculate_settlement, foerderendedatum_eeg,
    foerderendedatum_kwkg_years, foerderendedatum_repowering, kwk_foerderend_calendar, kwk_max_kwh,
    managementpraemie_ct, negativpreis_rule_applies,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use time::macros::date;

fn d(s: &str) -> Decimal {
    s.parse().expect("valid decimal")
}

// ═══════════════════════════════════════════════════════════════════════════
// §21 EEG 2023 — Feste Einspeisevergütung
// ═══════════════════════════════════════════════════════════════════════════

/// §21 EEG 2023 — Solar rooftop, EEG 2023 Q2, ≤10 kWp segment.
/// Rate: 8.11 ct/kWh. March: 650 kWh.
/// Payment: 650 × 8.11 / 100 = 52.715 EUR
#[test]
fn s21_solar_aufdach_q2_2023() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("650")),
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("52.715")));
    assert_eq!(out.eligible_kwh, Some(d("650")));
}

/// §21 EEG 2023 — Wind onshore 500 kW, standard rate 5.5 ct/kWh.
/// July: 95,000 kWh (average month, 26% capacity factor).
/// Payment: 95,000 × 5.5 / 100 = 5,225.00 EUR
#[test]
fn s21_wind_onshore_500kw() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("95000")),
        verguetungssatz_ct: d("5.5"),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("5225.00")));
}

/// §21 EEG 2023 — Zero kWh → EUR 0 (not an error).
#[test]
fn s21_zero_kwh_is_zero_eur() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(Decimal::ZERO),
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
}

/// §21 EEG 2023 — No meter data yet → NoData.
#[test]
fn s21_no_meter_data() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: None,
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::NoData);
    assert_eq!(out.settlement_eur, None);
}

// ═══════════════════════════════════════════════════════════════════════════
// §20 EEG 2023 — Gleitende Marktprämie (Direktvermarktung)
// ═══════════════════════════════════════════════════════════════════════════

/// §20 EEG 2023 — Direktvermarktung, positive spread.
/// Wind 750 kW: AW = 6.2 ct, EPEX July avg = 4.8 ct.
/// Marktprämie = (6.2 - 4.8) × 120,000 / 100 = 1,680 EUR
/// Managementprämie = 0.4 ct × 120,000 / 100 = 480 EUR
/// Total = 2,160 EUR
#[test]
fn s20_direktvermarktung_positive_spread_with_managementpraemie() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("120000")),
        epex_avg_ct_kwh: Some(d("4.8")),
        direktverm_aw_ct: Some(d("6.2")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("2160.00")));
    // Managementprämie tracked separately for accounting
    assert_eq!(
        out.positions
            .iter()
            .find(|p| p.legal_basis == "§20 Abs. 3 EEG 2023")
            .map(|p| p.eur),
        Some(d("480.00"))
    );
}

/// §20 EEG 2023 — Zero spread (EPEX = AW): only Managementprämie is paid.
/// AW = 5.0 ct, EPEX = 5.0 ct → Marktprämie = 0.
/// Payment = 0.4 ct × 50,000 / 100 = 200 EUR (Managementprämie only).
#[test]
fn s20_zero_spread_managementpraemie_only() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("50000")),
        epex_avg_ct_kwh: Some(d("5.0")),
        direktverm_aw_ct: Some(d("5.0")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("200.00")));
    assert_eq!(
        out.positions
            .iter()
            .find(|p| p.legal_basis == "§20 Abs. 3 EEG 2023")
            .map(|p| p.eur),
        Some(d("200.00"))
    );
}

/// §20 EEG 2023 — Negative spread (EPEX > AW): Marktprämie clamped to 0.
/// AW = 4.0 ct, EPEX Dec = 8.2 ct → max(0, -4.2) = 0.
/// Only Managementprämie (0.4 ct) is paid: 0.4 × 60,000 / 100 = 240 EUR.
#[test]
fn s20_negative_spread_clamped_managementpraemie_paid() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("60000")),
        epex_avg_ct_kwh: Some(d("8.2")),
        direktverm_aw_ct: Some(d("4.0")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("240.00")));
    // No Gleitende Marktprämie — only Managementprämie
    assert_eq!(
        out.positions
            .iter()
            .find(|p| p.legal_basis == "§20 Abs. 3 EEG 2023")
            .map(|p| p.eur),
        Some(d("240.00"))
    );
}

/// §20 EEG 2023 — EPEX price missing → PriceMissing.
#[test]
fn s20_no_epex_price_missing() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("50000")),
        direktverm_aw_ct: Some(d("6.0")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::PriceMissing);
    assert_eq!(out.settlement_eur, None);
}

/// §20 Abs. 3 Nr. 1 EEG 2023 — Large plant (>100 MW): reduced Managementprämie.
/// Plants >100 MW get 0.2 ct/kWh instead of 0.4 ct/kWh.
#[test]
fn s20_abs3_reduced_managementpraemie_large_plant() {
    // 110 MW plant (110,000 kWp): reduced rate 0.2 ct/kWh
    let mgmt_ct = managementpraemie_ct(d("110000"));
    assert_eq!(mgmt_ct, d("0.2"));

    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("2000000")), // 2 GWh for 110 MW plant
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.0")),
        managementpraemie_ct: Some(mgmt_ct), // 0.2 ct/kWh
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Marktprämie: (6.0-4.5)×2M/100 = 30,000 EUR
    // Managementprämie: 0.2×2M/100 = 4,000 EUR
    // Total: 34,000 EUR
    assert_eq!(out.settlement_eur, Some(d("34000.00")));
    assert_eq!(
        out.positions
            .iter()
            .find(|p| p.legal_basis == "§20 Abs. 3 EEG 2023")
            .map(|p| p.eur),
        Some(d("4000.00"))
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §§22a, 28 EEG 2023 — Ausschreibungsanlagen (BNetzA tender)
// ═══════════════════════════════════════════════════════════════════════════

/// §§22a, 28 EEG 2023 — BNetzA tender, 10 MWp Freifläche solar park.
/// Tendered AW = 5.82 ct/kWh. EPEX Aug avg = 4.1 ct.
/// Marktprämie = 1.72 ct × 2,500,000 kWh / 100 = 43,000 EUR
/// Managementprämie = 0.4 ct × 2,500,000 / 100 = 10,000 EUR
/// Total = 53,000 EUR
#[test]
fn s22a_ausschreibung_10mwp_august() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Ausschreibung,
        einspeisemenge_kwh: Some(d("2500000")),
        epex_avg_ct_kwh: Some(d("4.1")),
        direktverm_aw_ct: Some(d("5.82")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("53000.00")));
    assert_eq!(
        out.positions
            .iter()
            .find(|p| p.legal_basis == "§20 Abs. 3 EEG 2023")
            .map(|p| p.eur),
        Some(d("10000.00"))
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §38a EEG 2023 — Mieterstrom
// ═══════════════════════════════════════════════════════════════════════════

/// §38a EEG 2023 — 50 kWp community solar building.
/// Base rate: 7.5 ct/kWh. Mieterstrom-Zuschlag: 1.3 ct/kWh.
/// Month: 800 kWh. Payment: 800 × 8.8 / 100 = 70.40 EUR
#[test]
fn s38a_mieterstrom_building_solar() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Mieterstrom,
        einspeisemenge_kwh: Some(d("800")),
        verguetungssatz_ct: d("7.5"),
        mieter_zuschlag_ct: Some(d("1.3")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("70.40")));
}

/// §38a EEG 2023 — Zero Mieterstrom-Zuschlag equals base Vergütung.
#[test]
fn s38a_zero_zuschlag_equals_verguetung() {
    let base = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.0"),
        ..SettleInput::default()
    });
    let mieterstrom = calculate_settlement(&SettleInput {
        model: SettlementModel::Mieterstrom,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.0"),
        mieter_zuschlag_ct: Some(Decimal::ZERO),
        ..SettleInput::default()
    });
    assert_eq!(base.settlement_eur, mieterstrom.settlement_eur);
}

// ═══════════════════════════════════════════════════════════════════════════
// §50 EEG 2023 — Flexibilitätsprämie
// ═══════════════════════════════════════════════════════════════════════════

/// §50 EEG 2023 — Biomasse 500 kW flex dispatch.
/// Base: 6.5 ct/kWh. Flex premium: 1.5 ct/kWh.
/// Month: 180,000 kWh. Payment: 180,000 × 8.0 / 100 = 14,400 EUR
#[test]
fn s50_flexibilitaetspraemie_biomasse() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Flexibilitaet,
        einspeisemenge_kwh: Some(d("180000")),
        verguetungssatz_ct: d("6.5"),
        flex_praemie_ct_kwh: Some(d("1.5")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("14400.00")));
}

// ═══════════════════════════════════════════════════════════════════════════
// Post-EEG Spot (§21 post-Förderung)
// ═══════════════════════════════════════════════════════════════════════════

/// Post-EEG: 20-year-old 5 kWp plant feeds at EPEX spot.
/// June EPEX avg: 6.1 ct/kWh. 420 kWh → 25.62 EUR.
#[test]
fn post_eeg_spot_positive_epex() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::PostEegSpot,
        einspeisemenge_kwh: Some(d("420")),
        epex_avg_ct_kwh: Some(d("6.1")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("25.62")));
}

/// Post-EEG with NEGATIVE EPEX → negative settlement (plant owes money).
/// §21 post-Förderung: no price floor. Plant bears full market risk.
/// EPEX avg = -0.5 ct/kWh. 1000 kWh → -5.00 EUR.
#[test]
fn post_eeg_spot_negative_epex_plant_pays() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::PostEegSpot,
        einspeisemenge_kwh: Some(d("1000")),
        epex_avg_ct_kwh: Some(d("-0.5")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Negative settlement: plant owes EUR 5 to the NB
    assert_eq!(out.settlement_eur, Some(d("-5.00")));
    let eur = out.settlement_eur.unwrap();
    assert!(
        eur < Decimal::ZERO,
        "negative EPEX must produce negative settlement"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §25 / §47 EEG 2023 — Sanktionen bei fehlender MaStR-Registrierung
// ═══════════════════════════════════════════════════════════════════════════

/// §25 EEG 2023 — Plant not registered in MaStR → Vergütung suspended.
/// Retroactive recovery is NOT permitted (§25 Abs. 2 EEG 2023).
/// settlement_eur = 0, status = Sanctioned.
#[test]
fn s25_mastr_not_registered_zero_vergütung() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufNull), // §25 EEG sanction active
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
    // Meter data is preserved for audit trail
    assert_eq!(out.eligible_kwh, Some(d("500")));
}

/// §25 EEG 2023 — Sanktionen apply to ALL models including Direktvermarktung.
#[test]
fn s25_sanktion_overrides_direktvermarktung() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("100000")),
        epex_avg_ct_kwh: Some(d("4.8")),
        direktverm_aw_ct: Some(d("6.2")),
        managementpraemie_ct: Some(d("0.4")),
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufNull),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
    assert!(
        out.positions
            .iter()
            .all(|p| p.legal_basis != "§20 Abs. 3 EEG 2023"),
        "no Managementprämie position expected"
    );
}

/// §25 EEG 2023 — After MaStR registration: normal settlement resumes.
/// `is_sanctioned = false` (default) → calculation proceeds normally.
#[test]
fn s25_after_registration_normal_settlement() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        sanktion: None, // no §52 sanction
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("40.55")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §27 EEG 2023 (ex-§51 EEG 2021) — Negativpreisregel
// ═══════════════════════════════════════════════════════════════════════════

/// §51 EEG 2023 — Rule applies for any negative-price period (EEG 2023 removed 6h threshold).
#[test]
fn s27_negativpreis_threshold_6h() {
    assert!(!negativpreis_rule_applies(0), "0h: no negative period");
    assert!(
        negativpreis_rule_applies(1),
        "1h: EEG 2023 activates at any negative period"
    );
    assert!(negativpreis_rule_applies(6), "6h: still applies");
    assert!(negativpreis_rule_applies(24), "full day");
}

/// §27 EEG 2023 — kWh during negative-price hours are excluded from Vergütung.
/// Monthly total: 1,000 kWh. 80 kWh produced during 8h negative EPEX window.
/// Effective kWh: 920. Rate: 8.11 ct. Payment: 920 × 8.11 / 100 = 74.612 EUR.
#[test]
fn s27_verguetung_deduct_negative_price_kwh() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        kwh_during_negative_epex: Some(d("80")), // 80 kWh during negative EPEX
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.eligible_kwh, Some(d("920")));
    assert_eq!(out.settlement_eur, Some(d("74.612")));
}

/// §27 EEG 2023 — If ALL kWh were during negative hours: settlement = EUR 0.
#[test]
fn s27_all_kwh_during_negative_hours_zero_eur() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        kwh_during_negative_epex: Some(d("500")), // 100% negative period
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.eligible_kwh, Some(Decimal::ZERO));
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
}

/// §27 EEG 2023 — Mieterstrom also subject to negative-price deduction.
#[test]
fn s27_mieterstrom_negative_price_deduction() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Mieterstrom,
        einspeisemenge_kwh: Some(d("800")),
        verguetungssatz_ct: d("7.5"),
        mieter_zuschlag_ct: Some(d("1.3")),
        kwh_during_negative_epex: Some(d("100")), // 100 kWh excluded
        ..SettleInput::default()
    });
    // Effective: 700 kWh × (7.5+1.3) / 100 = 700 × 8.8 / 100 = 61.60 EUR
    assert_eq!(out.eligible_kwh, Some(d("700")));
    assert_eq!(out.settlement_eur, Some(d("61.60")));
}

/// §27 EEG 2023 — Direktvermarktung is NOT subject to the negative-price rule.
/// The Direktvermarkter bears the market price risk directly.
#[test]
fn s27_direktvermarktung_not_subject_to_negativpreis() {
    // Even if negative_kwh is supplied, it is ignored for Direktvermarktung
    let with_neg = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("10000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.0")),
        managementpraemie_ct: Some(d("0.4")),
        kwh_during_negative_epex: Some(d("500")), // supplied but ignored
        ..SettleInput::default()
    });
    let without_neg = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("10000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.0")),
        managementpraemie_ct: Some(d("0.4")),
        kwh_during_negative_epex: None,
        ..SettleInput::default()
    });
    // Direktvermarktung ignores kwh_during_negative_epex — results must be equal
    assert_eq!(
        with_neg.settlement_eur, without_neg.settlement_eur,
        "Direktvermarktung must not apply §27 negative-price deduction"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §7 KWKG 2023 — KWK-Zuschlag
// ═══════════════════════════════════════════════════════════════════════════

/// §7 KWKG 2023 — Small CHP ≤50 kW_el: KWK-Zuschlag 8.0 ct/kWh.
/// January: 7,000 kWh (70% capacity factor, 720h).
/// Payment: 7,000 × 8.0 / 100 = 560 EUR
#[test]
fn s7_kwkg_small_chp_leq50kw() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::KwkgZuschlag,
        einspeisemenge_kwh: Some(d("7000")),
        verguetungssatz_ct: d("8.0"), // §7 Abs. 1 Nr. 1 KWKG 2023 ≤50 kW rate
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("560.00")));
}

/// §7 KWKG 2023 — Large CHP >2 MW, hour-limit approaching.
/// Plant has 29,900 kWh paid. Max 30,000 kWh. This period: 400 kWh.
/// Only 100 kWh eligible (prorated last period). Status = FoerderungBeendet.
#[test]
fn s7_kwkg_large_chp_limit_reached_prorated() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::KwkgZuschlag,
        einspeisemenge_kwh: Some(d("400")),
        verguetungssatz_ct: d("3.1"), // §7 Abs. 1 Nr. 5 KWKG 2023 >2 MW rate
        kwk_strom_kwh_gesamt: Some(d("29900")),
        kwk_max_kwh: Some(d("30000")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::FoerderungBeendet);
    assert_eq!(out.eligible_kwh, Some(d("100"))); // prorated: only 100 kWh remain
    assert_eq!(out.settlement_eur, Some(d("3.1"))); // 100 × 3.1 / 100
}

/// §7 KWKG 2023 — Förderung already fully exhausted: EUR 0, FoerderungBeendet.
#[test]
fn s7_kwkg_already_exhausted() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::KwkgZuschlag,
        einspeisemenge_kwh: Some(d("5000")),
        verguetungssatz_ct: d("3.1"),
        kwk_strom_kwh_gesamt: Some(d("30001")), // already over the limit
        kwk_max_kwh: Some(d("30000")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::FoerderungBeendet);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
    assert_eq!(out.eligible_kwh, Some(Decimal::ZERO));
}

/// §7 KWKG 2023 — Year-limited plant (≤2 MW, no hour-limit): full period.
#[test]
fn s7_kwkg_year_limited_no_hour_limit() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::KwkgZuschlag,
        einspeisemenge_kwh: Some(d("50000")),
        verguetungssatz_ct: d("4.0"), // 100–250 kW rate
        kwk_strom_kwh_gesamt: None,   // no hour-limit tracking
        kwk_max_kwh: None,
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("2000.00")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §21 EEG — Eigenverbrauch (self-consumption)
// ═══════════════════════════════════════════════════════════════════════════

/// §21 EEG — Eigenverbrauch: always EUR 0, no matter the kWh or rate.
#[test]
fn eigenverbrauch_always_zero() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Eigenverbrauch,
        einspeisemenge_kwh: Some(d("9999")),
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
}

// ═══════════════════════════════════════════════════════════════════════════
// §21 EEG 2023 — Förderdauer calculation helpers
// ═══════════════════════════════════════════════════════════════════════════

/// §21 EEG 2023 — Standard 20-year Förderdauer.
///
/// §25 Abs. 1 Satz 2 EEG 2023: "Bei Anlagen, deren anzulegender Wert gesetzlich bestimmt
/// wird, verlängert sich dieser Zeitraum bis zum 31. Dezember des zwanzigsten Jahres."
/// Statutory plants ALWAYS end on December 31, never the exact anniversary date.
#[test]
fn foerderendedatum_20_years_from_inbetriebnahme() {
    // May 2010 plant → 2030-12-31, NOT 2030-05-15
    assert_eq!(
        foerderendedatum_eeg(date!(2010 - 05 - 15)).unwrap(),
        date!(2030 - 12 - 31)
    );
    // January 2000 plant → 2020-12-31
    assert_eq!(
        foerderendedatum_eeg(date!(2000 - 01 - 01)).unwrap(),
        date!(2020 - 12 - 31)
    );
    // December 31 plant → still same year-end
    assert_eq!(
        foerderendedatum_eeg(date!(2023 - 12 - 31)).unwrap(),
        date!(2043 - 12 - 31)
    );
}

/// §22 EEG 2023 — Repowering resets the 20-year Förderdauer clock.
/// The new end date is also December 31 of the 20th year (§25 Abs. 1 Satz 2).
#[test]
fn foerderendedatum_repowering_resets_clock() {
    let orig_end = foerderendedatum_eeg(date!(2010 - 06 - 01)).unwrap();
    let repowering_end = foerderendedatum_repowering(date!(2025 - 06 - 01)).unwrap();

    assert_eq!(orig_end, date!(2030 - 12 - 31)); // orig: Dec 31 of year+20
    assert_eq!(repowering_end, date!(2045 - 12 - 31)); // repowering: Dec 31 of repowering+20
    assert!(repowering_end > orig_end);
}

/// §8 KWKG 2023 — Year-based Förderdauer for ≤2 MW plants.
#[test]
fn kwkg_foerderendedatum_year_limited() {
    // 50 kW CHP: 20 years
    assert_eq!(
        foerderendedatum_kwkg_years(date!(2023 - 06 - 15), 20).unwrap(),
        date!(2043 - 06 - 15)
    );
    // 500 kW CHP: 10 years
    assert_eq!(
        foerderendedatum_kwkg_years(date!(2023 - 06 - 15), 10).unwrap(),
        date!(2033 - 06 - 15)
    );
}

/// §8 Abs. 4 KWKG 2023 — Calendar-year maximum for large CHP plants (15 years).
/// Even if the 30,000 full-load-hour limit is not reached, Förderung ends
/// after 15 calendar years.
#[test]
fn kwkg_foerderend_calendar_15yr_cap() {
    let commissioned = date!(2020 - 01 - 15);
    let calendar_end = kwk_foerderend_calendar(commissioned).unwrap();
    assert_eq!(calendar_end, date!(2035 - 01 - 15));

    // Verify: a plant running at 50% capacity uses only 15yr × 8760h × 50% = 65,700 h
    // far below the 30,000 h statutory limit, but Förderung still ends at the calendar cap.
    let half_load_h = 15 * 8760 / 2; // 65,700 hours (> 30,000 h limit!)
    // This illustrates that kwk_foerderend_calendar catches cases where the plant
    // would otherwise never hit the hour limit.
    assert!(
        i32::try_from(half_load_h).unwrap() > 30_000,
        "at 50% capacity the plant exceeds the hour limit naturally, but calendar cap applies earlier"
    );
}

/// §8 KWKG 2023 — Maximum kWh formula: rated_kW × full_load_hours.
/// Critical: NOT just full_load_hours (a common implementation bug).
#[test]
fn kwk_max_kwh_correct_formula() {
    // 2.5 MW plant, 30,000 full-load hours → 75,000,000 kWh cap
    let limit = kwk_max_kwh(d("2500"), 30_000);
    assert_eq!(limit, d("75000000"));

    // The wrong formula (hours only) would be 30,000 — off by 2500×
    assert!(
        limit > d("1000000"),
        "kwk_max_kwh must account for rated power"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Decimal precision — no float money
// ═══════════════════════════════════════════════════════════════════════════

/// All settlement arithmetic must be exact — no IEEE 754 rounding errors.
#[test]
fn decimal_precision_no_float_rounding() {
    // 333.333 kWh × 8.1 ct = 26.999973 EUR raw; rounded to 5dp = 26.99997 EUR
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("333.333")),
        verguetungssatz_ct: d("8.1"),
        ..SettleInput::default()
    });
    // 5dp precision — EuroAmount (Amount<5>) rounds to 5 decimal places
    assert_eq!(out.settlement_eur, Some(d("26.99997"))); // 5dp: 26.999973 → 26.99997

    // Classic float64 pitfall: 0.1 + 0.2 ≠ 0.3 exactly
    let float_sum = 0.1_f64 + 0.2_f64;
    // In Decimal: exact
    let decimal_sum: Decimal = d("0.1") + d("0.2");
    assert_eq!(decimal_sum, d("0.3"), "Decimal arithmetic is exact");
    // Float is not exact (0.30000000000000004)
    assert_ne!(float_sum, 0.3_f64, "f64 arithmetic is NOT exact for money");
}

/// Large settlement (1 GWh × 30 ct/kWh = 300,000 EUR) stays exact.
#[test]
fn large_settlement_exact() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(dec!(1_000_000)),
        verguetungssatz_ct: dec!(30),
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(dec!(300_000)));
}

// ═══════════════════════════════════════════════════════════════════════════
// §24 EEG 2023 — Zusammenlegung (tariff band boundary)
// ═══════════════════════════════════════════════════════════════════════════

/// §24 EEG 2023 — After Zusammenlegung the combined capacity may shift the
/// plant into a lower Vergütungssatz band.
///
/// Plant A: 8 kWp (≤10 kWp band, 7.83 ct/kWh)
/// Plant B: 5 kWp (≤10 kWp band, 7.83 ct/kWh)
/// Combined: 13 kWp → falls into 10–40 kWp band (6.79 ct/kWh)
#[test]
fn s24_zusammenlegung_crosses_tariff_band() {
    // Before Zusammenlegung: Plant A settles at ≤10 kWp rate
    let before = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("7.83"), // ≤10 kWp rate
        ..SettleInput::default()
    });

    // After Zusammenlegung: new rate for 10–40 kWp band
    let after = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("6.79"), // 10–40 kWp rate (lower)
        ..SettleInput::default()
    });

    assert!(
        after.settlement_eur < before.settlement_eur,
        "combined capacity at higher band = lower per-kWh rate"
    );

    // Difference: 500 × (7.83 - 6.79) / 100 = 500 × 1.04 / 100 = 5.20 EUR
    let diff = before.settlement_eur.unwrap() - after.settlement_eur.unwrap();
    assert_eq!(diff, d("5.20"));
}

// ═══════════════════════════════════════════════════════════════════════════
// §§20 & 23 EEG 2023 — Combined Managementprämie scenarios
// ═══════════════════════════════════════════════════════════════════════════

/// §20 EEG 2023 — Energy crisis scenario: EPEX >> AW.
/// December 2022: EPEX avg ≈ 28 ct/kWh. AW = 5.5 ct.
/// Marktprämie = max(0, 5.5-28) = 0. Only Managementprämie paid.
#[test]
fn s20_energy_crisis_epex_far_above_aw() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("80000")),
        epex_avg_ct_kwh: Some(d("28.0")),
        direktverm_aw_ct: Some(d("5.5")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Only 0.4 ct × 80,000 / 100 = 320 EUR (Managementprämie)
    assert_eq!(out.settlement_eur, Some(d("320.00")));
    assert_eq!(
        out.positions
            .iter()
            .find(|p| p.legal_basis == "§20 Abs. 3 EEG 2023")
            .map(|p| p.eur),
        Some(d("320.00"))
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// billing::LineItem bridge — settlement_to_line_items()
// ═══════════════════════════════════════════════════════════════════════════

/// `settlement_to_line_items` produces correct line item count and tags.
#[test]
fn bridge_verguetung_one_line_item() {
    use eeg_billing::bridge::settlement_to_line_items;
    let input = SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    };
    let output = calculate_settlement(&input);
    let items = settlement_to_line_items(&output);
    assert_eq!(items.len(), 1);
    assert!(
        items[0].description.contains("EEG"),
        "position description should mention EEG"
    );
    assert!(items[0].has_tag("eeg"));
    assert!(items[0].has_tag("verguetung"));
    assert_eq!(
        items[0].net_amount,
        billing::EuroAmount::checked_from_decimal(d("40.55")).unwrap()
    );
}

/// Direktvermarktung produces 2 items: Marktprämie + Managementprämie.
#[test]
fn bridge_direktvermarktung_two_line_items() {
    use eeg_billing::bridge::settlement_to_line_items;
    let input = SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("100000")),
        epex_avg_ct_kwh: Some(d("4.8")),
        direktverm_aw_ct: Some(d("6.2")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    };
    let output = calculate_settlement(&input);
    let items = settlement_to_line_items(&output);
    assert_eq!(items.len(), 2, "Marktprämie + Managementprämie");
    assert!(items[0].has_tag("marktpraemie"));
    assert!(items[1].has_tag("managementpraemie"));
}

/// §25 Sanctioned produces 1 EUR 0 item tagged §25-sanctioned.
#[test]
fn bridge_sanctioned_zero_item() {
    use eeg_billing::bridge::settlement_to_line_items;
    let input = SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufNull),
        ..SettleInput::default()
    };
    let output = calculate_settlement(&input);
    let items = settlement_to_line_items(&output);
    assert_eq!(items.len(), 1);
    assert!(items[0].has_tag("§25-sanctioned"));
    assert_eq!(items[0].net_amount, billing::EuroAmount::ZERO);
}

/// NoData → empty line items (nothing to bill yet).
#[test]
fn bridge_no_data_empty() {
    use eeg_billing::bridge::settlement_to_line_items;
    let input = SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: None, // no meter data
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    };
    let output = calculate_settlement(&input);
    let items = settlement_to_line_items(&output);
    assert!(items.is_empty(), "no meter data → no line items");
}

// ═══════════════════════════════════════════════════════════════════════════
// Billing positions — every calculation component is individually auditable
// ═══════════════════════════════════════════════════════════════════════════

/// VERGUETUNG: exactly 1 position, correct §21 EEG 2023 label.
#[test]
fn positions_verguetung_single_line() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    });
    assert_eq!(out.positions.len(), 1);
    let p = &out.positions[0];
    assert!(
        p.description.contains("EEG"),
        "description should mention EEG"
    );
    assert_eq!(p.legal_basis, "§21 EEG 2023");
    assert_eq!(p.kwh, d("1000"));
    assert_eq!(p.rate_ct_kwh, d("8.11"));
    assert_eq!(p.eur, d("81.10"));
    // settlement_eur = sum of positions
    assert_eq!(out.settlement_eur, Some(d("81.10")));
}

/// MIETERSTROM: 2 positions — base Vergütung + §38a Zuschlag.
#[test]
fn positions_mieterstrom_two_lines() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Mieterstrom,
        einspeisemenge_kwh: Some(d("800")),
        verguetungssatz_ct: d("7.5"),
        mieter_zuschlag_ct: Some(d("1.3")),
        ..SettleInput::default()
    });
    assert_eq!(out.positions.len(), 2);

    let base = &out.positions[0];
    assert_eq!(base.legal_basis, "§21 EEG 2023");
    assert_eq!(base.kwh, d("800"));
    assert_eq!(base.rate_ct_kwh, d("7.5"));
    assert_eq!(base.eur, d("60.00")); // 800 × 7.5 / 100

    let zuschlag = &out.positions[1];
    assert_eq!(zuschlag.legal_basis, "§38a EEG 2023");
    assert_eq!(zuschlag.kwh, d("800"));
    assert_eq!(zuschlag.rate_ct_kwh, d("1.3"));
    assert_eq!(zuschlag.eur, d("10.40")); // 800 × 1.3 / 100

    // Total = 60.00 + 10.40 = 70.40 EUR
    assert_eq!(out.settlement_eur, Some(d("70.40")));
}

/// DIREKTVERMARKTUNG: 2 positions — Marktprämie + Managementprämie.
/// Positions sum equals settlement_eur.
#[test]
fn positions_direktvermarktung_marktpraemie_and_managementpraemie() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("120000")),
        epex_avg_ct_kwh: Some(d("4.8")),
        direktverm_aw_ct: Some(d("6.2")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    // At positive spread: 2 positions
    assert_eq!(
        out.positions.len(),
        2,
        "should have Marktprämie + Managementprämie"
    );

    let marktpraemie = out
        .positions
        .iter()
        .find(|p| p.legal_basis == "§20 EEG 2023")
        .unwrap();
    assert_eq!(marktpraemie.rate_ct_kwh, d("1.4")); // 6.2 - 4.8 = 1.4 ct
    assert_eq!(marktpraemie.eur, d("1680.00"));

    let mgmt = out
        .positions
        .iter()
        .find(|p| p.legal_basis == "§20 Abs. 3 EEG 2023")
        .unwrap();
    assert_eq!(mgmt.rate_ct_kwh, d("0.4"));
    assert_eq!(mgmt.eur, d("480.00"));

    // Total = 1680 + 480 = 2160 EUR
    assert_eq!(out.settlement_eur, Some(d("2160.00")));
    // Positions must sum to settlement_eur
    let sum: rust_decimal::Decimal = out.positions.iter().map(|p| p.eur).sum();
    assert_eq!(
        Some(sum),
        out.settlement_eur,
        "positions must sum to settlement_eur"
    );
}

/// DIREKTVERMARKTUNG at zero spread: only Managementprämie position.
#[test]
fn positions_direktvermarktung_zero_spread_only_managementpraemie() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Direktvermarktung,
        einspeisemenge_kwh: Some(d("50000")),
        epex_avg_ct_kwh: Some(d("5.0")),
        direktverm_aw_ct: Some(d("5.0")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    // Zero spread → Marktprämie position dropped (0 ct), only Managementprämie
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].legal_basis, "§20 Abs. 3 EEG 2023");
    assert_eq!(out.positions[0].eur, d("200.00")); // 50000 × 0.4 / 100
    assert_eq!(out.settlement_eur, Some(d("200.00")));
}

/// AUSSCHREIBUNG: positions label includes "§§22a,28 EEG 2023".
#[test]
fn positions_ausschreibung_legal_basis_label() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Ausschreibung,
        einspeisemenge_kwh: Some(d("2500000")),
        epex_avg_ct_kwh: Some(d("4.1")),
        direktverm_aw_ct: Some(d("5.82")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    let praemie = out
        .positions
        .iter()
        .find(|p| p.legal_basis == "§§22a,28 EEG 2023")
        .unwrap();
    assert_eq!(praemie.rate_ct_kwh, d("1.72")); // 5.82 - 4.1
    assert_eq!(praemie.eur, d("43000.00"));
}

/// FLEXIBILITAET: 2 positions — base Vergütung + §50 Flex-Prämie.
#[test]
fn positions_flexibilitaet_two_lines() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Flexibilitaet,
        einspeisemenge_kwh: Some(d("180000")),
        verguetungssatz_ct: d("6.5"),
        flex_praemie_ct_kwh: Some(d("1.5")),
        ..SettleInput::default()
    });
    assert_eq!(out.positions.len(), 2);
    let base = out
        .positions
        .iter()
        .find(|p| p.legal_basis == "§21 EEG 2023")
        .unwrap();
    assert_eq!(base.eur, d("11700.00")); // 180000 × 6.5 / 100
    let flex = out
        .positions
        .iter()
        .find(|p| p.legal_basis == "§50b EEG 2023")
        .unwrap();
    assert_eq!(flex.eur, d("2700.00")); // 180000 × 1.5 / 100
    assert_eq!(out.settlement_eur, Some(d("14400.00")));
}

/// KWKG: single position with §7 KWKG 2023 legal basis.
#[test]
fn positions_kwkg_single_line() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::KwkgZuschlag,
        einspeisemenge_kwh: Some(d("7000")),
        verguetungssatz_ct: d("8.0"),
        ..SettleInput::default()
    });
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].legal_basis, "§7 KWKG 2023");
    assert_eq!(out.positions[0].kwh, d("7000"));
    assert_eq!(out.positions[0].eur, d("560.00"));
}

/// KWKG prorated: description includes "Förderdauer-Endabrechnung".
#[test]
fn positions_kwkg_prorated_description_contains_endabrechnung() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::KwkgZuschlag,
        einspeisemenge_kwh: Some(d("400")),
        verguetungssatz_ct: d("3.1"),
        kwk_strom_kwh_gesamt: Some(d("29900")),
        kwk_max_kwh: Some(d("30000")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::FoerderungBeendet);
    assert_eq!(out.positions.len(), 1);
    assert!(
        out.positions[0]
            .description
            .contains("Förderdauer-Endabrechnung"),
        "description must indicate final prorated period"
    );
    assert_eq!(out.positions[0].kwh, d("100")); // prorated
    assert_eq!(out.positions[0].eur, d("3.1")); // 100 × 3.1 / 100
}

/// §27 EEG negative-price rule: description mentions §27.
#[test]
fn positions_negativpreis_description_mentions_s27() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        kwh_during_negative_epex: Some(d("80")),
        ..SettleInput::default()
    });
    assert!(
        out.positions[0].description.contains("§27"),
        "description must reference §27 EEG 2023 when negative-price rule applied"
    );
    assert_eq!(out.positions[0].kwh, d("920")); // 1000 - 80
}

/// Eigenverbrauch: zero positions (EUR 0, no charge document).
#[test]
fn positions_eigenverbrauch_zero_positions() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Eigenverbrauch,
        einspeisemenge_kwh: Some(d("500")),
        ..SettleInput::default()
    });
    assert!(
        out.positions.is_empty(),
        "Eigenverbrauch must produce no billing positions"
    );
    assert_eq!(out.settlement_eur, Some(d("0")));
}

/// Sanctioned: zero positions (EUR 0, no charge document, status = Sanctioned).
#[test]
fn positions_sanctioned_zero_positions() {
    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufNull),
        ..SettleInput::default()
    });
    assert!(
        out.positions.is_empty(),
        "Sanctioned must produce no billing positions"
    );
    assert_eq!(out.settlement_eur, Some(d("0")));
    assert_eq!(out.status, SettlementStatus::Sanctioned);
}

/// POST_EEG_SPOT negative: position eur is negative, to_line_item uses Sign::Credit.
#[test]
fn positions_post_eeg_negative_eur_credit_line_item() {
    use eeg_billing::bridge::settlement_to_line_items;

    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::PostEegSpot,
        einspeisemenge_kwh: Some(d("1000")),
        epex_avg_ct_kwh: Some(d("-0.5")),
        ..SettleInput::default()
    });
    assert_eq!(out.positions.len(), 1);
    let p = &out.positions[0];
    assert!(
        p.eur < rust_decimal::Decimal::ZERO,
        "negative EPEX → negative EUR position"
    );
    assert_eq!(p.eur, d("-5.00")); // 1000 × (-0.5) / 100

    // Bridge converts negative position to Sign::Credit LineItem
    let items = settlement_to_line_items(&out);
    assert_eq!(items.len(), 1);
    // The net_amount is negative (credit = negative in billing convention)
    assert!(
        items[0].net_amount < billing::Amount::<5>::ZERO,
        "negative EPEX must produce credit (negative) LineItem"
    );
}

/// Positions sum invariant: settlement_eur always equals sum(positions[*].eur).
#[test]
fn positions_sum_equals_settlement_eur_invariant() {
    use rust_decimal::Decimal;

    // Test for all multi-component models
    let cases = vec![
        SettleInput {
            model: SettlementModel::Mieterstrom,
            einspeisemenge_kwh: Some(d("500")),
            verguetungssatz_ct: d("7.5"),
            mieter_zuschlag_ct: Some(d("1.3")),
            ..SettleInput::default()
        },
        SettleInput {
            model: SettlementModel::Flexibilitaet,
            einspeisemenge_kwh: Some(d("200000")),
            verguetungssatz_ct: d("6.5"),
            flex_praemie_ct_kwh: Some(d("1.5")),
            ..SettleInput::default()
        },
        SettleInput {
            model: SettlementModel::Direktvermarktung,
            einspeisemenge_kwh: Some(d("100000")),
            epex_avg_ct_kwh: Some(d("4.5")),
            direktverm_aw_ct: Some(d("6.0")),
            managementpraemie_ct: Some(d("0.4")),
            ..SettleInput::default()
        },
    ];
    for input in &cases {
        let out = calculate_settlement(input);
        let pos_sum: Decimal = out.positions.iter().map(|p| p.eur).sum();
        assert_eq!(
            out.settlement_eur,
            Some(pos_sum),
            "settlement_eur must equal sum of position EUR amounts for {:?}",
            input.model
        );
    }
}

/// to_line_item bridge: VERGUETUNG produces a debit LineItem with correct quantity and rate.
#[test]
fn to_line_item_verguetung_debit_with_qty_and_rate() {
    use eeg_billing::bridge::settlement_to_line_items;

    let out = calculate_settlement(&SettleInput {
        model: SettlementModel::Verguetung,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    });
    let items = settlement_to_line_items(&out);
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert!(
        item.description.contains("EEG"),
        "description should mention EEG"
    );
    // Quantity = 1000 kWh
    assert_eq!(item.quantity_value(), Some(d("1000")));
    assert_eq!(item.unit_label(), Some("kWh"));
    // Net amount = 81.10 EUR
    use billing::Amount;
    assert_eq!(
        item.net_amount,
        Amount::<5>::from_decimal(d("81.10")).unwrap()
    );
    // Tagged with legal_basis
    assert_eq!(
        item.metadata.get("legal_basis").map(String::as_str),
        Some("§21 EEG 2023")
    );
}
