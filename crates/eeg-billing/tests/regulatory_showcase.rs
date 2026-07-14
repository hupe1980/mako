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
//!   [§§20–50 feed-in settlement, §52 sanctions, §51 negative prices]
//! - **KWKG 2023**: Kraft-Wärme-Kopplungsgesetz (BGBl. I Nr. 59, 2023)
//!   [§7 KWK-Zuschlag rates, §8 Förderdauer]
//! - **BNetzA AHB / Strom**: quarterly Vergütungssätze publications
//!
//! All monetary amounts in EUR. All rates in ct/kWh. No floating-point money.

use eeg_billing::{
    SettleInput, SettlementScheme, SettlementStatus, TariffSource, calculate_settlement,
    foerderendedatum_eeg, foerderendedatum_kwkg_years, foerderendedatum_repowering,
    kwk_foerderend_calendar, kwk_max_kwh, managementpraemie_ct, negativpreis_rule_applies,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::MarketPremium,
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
        scheme: SettlementScheme::MarketPremium,
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

/// §20 EEG 2023 — High EPEX (above AW + Managementprämie): total payment is ZERO.
///
/// **EEG 2023 §20 Abs. 3 correction**: the Managementprämie is NOT a guaranteed
/// floor payment. §20 Abs. 3 says the AW is _increased by_ 0.4 ct for the
/// calculation — so the Marktprämie formula is max(0, AW+0.4 − EPEX).
/// When EPEX > AW + Managementprämie, the total is 0 — the plant receives nothing.
///
/// Old EEG ≤2012 model (legally wrong for EEG 2023):
///   Marktprämie = max(0, AW − EPEX)        → 0 when EPEX > AW
///   Managementprämie = flat 0.4 ct/kWh     → always paid (wrong for EEG 2023!)
///
/// Correct EEG 2023 model:
///   eff_AW = 4.0 + 0.4 = 4.4 ct/kWh
///   total = max(0, 4.4 − 8.2) = 0 EUR     → zero, no floor
#[test]
fn s20_negative_spread_clamped_to_zero_eeg2023() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("60000")),
        epex_avg_ct_kwh: Some(d("8.2")),
        direktverm_aw_ct: Some(d("4.0")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // eff_AW = 4.4 ct; EPEX = 8.2 ct >> eff_AW → total = 0
    assert_eq!(out.settlement_eur, Some(d("0")));
    // Zero-spread audit position shown (no Managementprämie position — both are 0)
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].eur, d("0"));
}

/// §20 EEG 2023 — EPEX price missing → PriceMissing.
#[test]
fn s20_no_epex_price_missing() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
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
        scheme: SettlementScheme::MarketPremium,
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
        scheme: SettlementScheme::MarketPremium,
        tariff_source: TariffSource::Auction(eeg_billing::AusschreibungMetadata::default()),
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
        scheme: SettlementScheme::TenantElectricity,
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
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.0"),
        ..SettleInput::default()
    });
    let mieterstrom = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::TenantElectricity,
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
        scheme: SettlementScheme::FlexibilityPremium,
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
        scheme: SettlementScheme::PostEeg,
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
        scheme: SettlementScheme::PostEeg,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::MarketPremium,
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
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        sanktion: None, // no §52 sanction
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("40.55")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §51 EEG 2023 — Negativpreisregel (negative EPEX price rule)
// ═══════════════════════════════════════════════════════════════════════════

/// §51 EEG 2023 — Rule applies for ANY negative-price period.
/// (EEG 2023 removed the 6-consecutive-hours threshold that existed in EEG 2017/2021.)
#[test]
fn s51_negativpreis_threshold_applies_any_duration() {
    assert!(!negativpreis_rule_applies(0), "0h: no negative period");
    assert!(
        negativpreis_rule_applies(1),
        "1h: EEG 2023 activates at any negative period"
    );
    assert!(negativpreis_rule_applies(6), "6h: still applies");
    assert!(negativpreis_rule_applies(24), "full day");
}

/// §51 EEG 2023 — kWh during negative-price hours are excluded from Vergütung.
/// Monthly total: 1,000 kWh. 80 kWh produced during 8h negative EPEX window.
/// Effective kWh: 920. Rate: 8.11 ct. Payment: 920 × 8.11 / 100 = 74.612 EUR.
#[test]
fn s51_verguetung_deduct_negative_price_kwh() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        kwh_during_negative_epex: Some(d("80")), // 80 kWh during negative EPEX
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.eligible_kwh, Some(d("920")));
    assert_eq!(out.settlement_eur, Some(d("74.612")));
}

/// §51 EEG 2023 — If ALL kWh were during negative hours: settlement = EUR 0.
#[test]
fn s51_all_kwh_during_negative_hours_zero_eur() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        kwh_during_negative_epex: Some(d("500")), // 100% negative period
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.eligible_kwh, Some(Decimal::ZERO));
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
}

/// §51 EEG 2023 — Mieterstrom also subject to negative-price deduction.
#[test]
fn s51_mieterstrom_negative_price_deduction() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::TenantElectricity,
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

/// §51 EEG 2023 — Direktvermarktung is NOT subject to the negative-price rule.
/// The Direktvermarkter bears the market price risk directly.
#[test]
fn s51_direktvermarktung_not_subject_to_negativpreis() {
    // Even if negative_kwh is supplied, it is ignored for Direktvermarktung
    let with_neg = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("10000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.0")),
        managementpraemie_ct: Some(d("0.4")),
        kwh_during_negative_epex: Some(d("500")), // supplied but ignored
        ..SettleInput::default()
    });
    let without_neg = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
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
        "Direktvermarktung must not apply §51 negative-price deduction"
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
        scheme: SettlementScheme::KwkSurcharge,
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
        scheme: SettlementScheme::KwkSurcharge,
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
        scheme: SettlementScheme::KwkSurcharge,
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
        scheme: SettlementScheme::KwkSurcharge,
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
        scheme: SettlementScheme::Eigenverbrauch,
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
        half_load_h > 30_000,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("7.83"), // ≤10 kWp rate
        ..SettleInput::default()
    });

    // After Zusammenlegung: new rate for 10–40 kWp band
    let after = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
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

/// §20 EEG 2023 — Energy crisis scenario: EPEX >> AW. Plant receives ZERO.
///
/// **EEG 2023 §20 Abs. 3 correction**: Managementprämie is NOT a guaranteed
/// floor. eff_AW = 5.5 + 0.4 = 5.9 ct; EPEX = 28.0 ct >> 5.9 ct → total = 0.
///
/// Under the old (incorrect) EEG ≤2012 model this would have been:
///   0.4 ct × 80,000 kWh / 100 = 320 EUR (Managementprämie flat payment)
/// Under the correct EEG 2023 model:
///   total = max(0, 5.9 − 28.0) = 0 EUR
///
/// The plant operator bears full market price risk. No payment from the NB.
/// The Direktvermarkter sells at 28 ct EPEX — operator benefits through revenue
/// sharing in their Direktvermarktungsvertrag.
#[test]
fn s20_energy_crisis_epex_far_above_aw() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("80000")),
        epex_avg_ct_kwh: Some(d("28.0")),
        direktverm_aw_ct: Some(d("5.5")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // eff_AW = 5.9 ct; EPEX = 28.0 ct >> eff_AW → total = 0 EUR
    assert_eq!(out.settlement_eur, Some(d("0")));
    // Audit position present but zero
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].eur, d("0"));
}

// ═══════════════════════════════════════════════════════════════════════════
// billing::LineItem bridge — settlement_to_line_items()
// ═══════════════════════════════════════════════════════════════════════════

/// `settlement_to_line_items` produces correct line item count and tags.
#[test]
fn bridge_verguetung_one_line_item() {
    use eeg_billing::bridge::settlement_to_line_items;
    let input = SettleInput {
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::MarketPremium,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::TenantElectricity,
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
        scheme: SettlementScheme::MarketPremium,
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
        scheme: SettlementScheme::MarketPremium,
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
        scheme: SettlementScheme::MarketPremium,
        tariff_source: TariffSource::Auction(eeg_billing::AusschreibungMetadata::default()),
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
        scheme: SettlementScheme::FlexibilityPremium,
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
        scheme: SettlementScheme::KwkSurcharge,
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
        scheme: SettlementScheme::KwkSurcharge,
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

/// §51 EEG negative-price rule: description mentions §51.
#[test]
fn positions_negativpreis_description_mentions_s51() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        kwh_during_negative_epex: Some(d("80")),
        ..SettleInput::default()
    });
    assert!(
        out.positions[0].description.contains("§51"),
        "description must reference §51 EEG 2023 when negative-price rule applied"
    );
    assert_eq!(out.positions[0].kwh, d("920")); // 1000 - 80
}

/// Eigenverbrauch: zero positions (EUR 0, no charge document).
#[test]
fn positions_eigenverbrauch_zero_positions() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::Eigenverbrauch,
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
        scheme: SettlementScheme::FeedInTariff,
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
        scheme: SettlementScheme::PostEeg,
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
            scheme: SettlementScheme::TenantElectricity,
            einspeisemenge_kwh: Some(d("500")),
            verguetungssatz_ct: d("7.5"),
            mieter_zuschlag_ct: Some(d("1.3")),
            ..SettleInput::default()
        },
        SettleInput {
            scheme: SettlementScheme::FlexibilityPremium,
            einspeisemenge_kwh: Some(d("200000")),
            verguetungssatz_ct: d("6.5"),
            flex_praemie_ct_kwh: Some(d("1.5")),
            ..SettleInput::default()
        },
        SettleInput {
            scheme: SettlementScheme::MarketPremium,
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
            input.scheme
        );
    }
}

/// to_line_item bridge: VERGUETUNG produces a debit LineItem with correct quantity and rate.
#[test]
fn to_line_item_verguetung_debit_with_qty_and_rate() {
    use eeg_billing::bridge::settlement_to_line_items;

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
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

// ═══════════════════════════════════════════════════════════════════════════
// §51 EEG — Version-specific Negativpreisregel (6h / 4h / 1h thresholds)
// ═══════════════════════════════════════════════════════════════════════════

/// §51 EEG 2017 — Bestandsschutz: threshold is 6 consecutive hours.
/// 5h negative: rule does NOT trigger (< 6h threshold).
/// Caller must pass only kWh for which the 6h threshold was met.
/// Passing kwh_during_negative_epex=None for 5h ensures no deduction.
#[test]
fn s51_eeg2017_requires_6_consecutive_hours() {
    use eeg_billing::{EegGesetz, ErzeugungsArt};
    // EEG 2017 solar plant 600 kWp: above 500 kW non-wind threshold.
    // < 6h negative: no deduction (caller passes None for sub-threshold runs)
    let out_5h = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("10000")),
        verguetungssatz_ct: d("5.5"),
        kwh_during_negative_epex: None, // 5h run not met → caller passes None → no deduction
        eeg_gesetz: EegGesetz::Eeg2017,
        leistung_kwp: Some(d("600")), // 600 kWp solar
        erzeugungsart: Some(ErzeugungsArt::SolarFreiflaeche),
        ..SettleInput::default()
    });
    assert_eq!(
        out_5h.eligible_kwh,
        Some(d("10000")),
        "< 6h: no §51 deduction"
    );
    assert_eq!(out_5h.settlement_eur, Some(d("550.00")));

    // EEG 2017 solar 600 kWp: 6h threshold met → caller passes kwh_during_negative_epex
    let out_6h = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("10000")),
        verguetungssatz_ct: d("5.5"),
        kwh_during_negative_epex: Some(d("600")), // 600 kWh during 6h negative run
        eeg_gesetz: EegGesetz::Eeg2017,
        leistung_kwp: Some(d("600")), // 600 kWp: > 500 kW solar threshold
        erzeugungsart: Some(ErzeugungsArt::SolarFreiflaeche),
        ..SettleInput::default()
    });
    assert_eq!(
        out_6h.eligible_kwh,
        Some(d("9400")),
        "6h: §51 applied to 600 kWp solar"
    );
    assert_eq!(out_6h.settlement_eur, Some(d("517.00")));
}

/// §51 EEG 2017 — Wind onshore <3 MW exempt; other technologies <500 kW exempt.
#[test]
fn s51_eeg2017_wind_3mw_exemption() {
    use eeg_billing::{EegGesetz, ErzeugungsArt};

    // Wind 2.9 MW: below 3 MW threshold → exempt under EEG 2017 §51 Abs. 3 Nr. 1
    let wind_exempt = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500000")),
        verguetungssatz_ct: d("5.5"),
        kwh_during_negative_epex: Some(d("10000")),
        eeg_gesetz: EegGesetz::Eeg2017,
        leistung_kwp: Some(d("2900")), // 2.9 MW: < 3 MW wind exemption
        erzeugungsart: Some(ErzeugungsArt::WindOnshore),
        ..SettleInput::default()
    });
    // Wind <3 MW is exempt under EEG 2017 → no deduction
    assert_eq!(wind_exempt.eligible_kwh, Some(d("500000")));

    // Solar 600 kWp: above 500 kW non-wind threshold → §51 applies
    let solar_not_exempt = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500000")),
        verguetungssatz_ct: d("5.5"),
        kwh_during_negative_epex: Some(d("10000")),
        eeg_gesetz: EegGesetz::Eeg2017,
        leistung_kwp: Some(d("600")), // 600 kWp: > 500 kW non-wind threshold
        erzeugungsart: Some(ErzeugungsArt::SolarFreiflaeche),
        ..SettleInput::default()
    });
    assert_eq!(
        solar_not_exempt.eligible_kwh,
        Some(d("490000")),
        "§51 applied to 600 kWp solar"
    );
}

/// §51 EEG ≤2012 — Bestandsschutz: §51 NEVER applies (§66 EEG 2017 Satz 4).
/// Pre-2016 plants are always exempt, regardless of capacity or technology.
#[test]
fn s51_pre_2016_plants_always_exempt() {
    use eeg_billing::{EegGesetz, ErzeugungsArt};

    // 5 MWp solar from 2012: §51 must not apply even with 24h negative prices
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000000")),
        verguetungssatz_ct: d("18.5"),              // EEG 2012 rate
        kwh_during_negative_epex: Some(d("50000")), // 50 MWh during negative hours
        eeg_gesetz: EegGesetz::Eeg2012,
        leistung_kwp: Some(d("5000")), // 5 MWp: would otherwise trigger §51
        erzeugungsart: Some(ErzeugungsArt::SolarFreiflaeche),
        ..SettleInput::default()
    });
    // Bestandsschutz: §51 NEVER applies for EEG ≤2012 (§66 EEG 2017 Satz 4)
    assert_eq!(
        out.eligible_kwh,
        Some(d("1000000")),
        "EEG 2012 plant: §51 Bestandsschutz — no deduction"
    );
    assert_eq!(out.settlement_eur, Some(d("185000.00")));
}

/// §51 EEG 2021 — Threshold is 4 consecutive hours (changed from 6h in EEG 2017).
/// Wind exception removed: all plants <500 kW exempt.
#[test]
fn s51_eeg2021_4h_threshold() {
    use eeg_billing::{EegGesetz, ErzeugungsArt};

    // EEG 2021 wind plant 500 kW: NOT exempt (EEG 2021 removed wind exception)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("100000")),
        verguetungssatz_ct: d("8.0"),
        kwh_during_negative_epex: Some(d("5000")), // 4h threshold met
        eeg_gesetz: EegGesetz::Eeg2021,
        leistung_kwp: Some(d("500")), // exactly 500 kW — at EEG 2021 exemption boundary
        erzeugungsart: Some(ErzeugungsArt::WindOnshore),
        ..SettleInput::default()
    });
    // 500 kW is AT the threshold — EEG 2021: < 500 kW exempt; ≥500 kW not
    // The kw_grenze returns Some(500) for Eeg2021, meaning ≥500 kW triggers §51
    assert_eq!(
        out.eligible_kwh,
        Some(d("95000")),
        "EEG 2021 500 kW: §51 applies"
    );
}

/// §51 EEG 2023 — Any negative period triggers the rule (threshold = 1h).
#[test]
fn s51_eeg2023_any_negative_period() {
    use eeg_billing::EegGesetz;

    // EEG 2023 plant ≥100 kWp: even 1 hour of negative EPEX triggers §51
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        kwh_during_negative_epex: Some(d("30")), // 1h of negative EPEX
        eeg_gesetz: EegGesetz::Eeg2023,
        leistung_kwp: Some(d("150")), // ≥100 kWp: not exempt under EEG 2023
        ..SettleInput::default()
    });
    assert_eq!(
        out.eligible_kwh,
        Some(d("970")),
        "EEG 2023: any negative period → §51"
    );
    assert_eq!(out.settlement_eur, Some(d("78.667")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §52 EEG ≤2021 — SanktionAlt (alt regime via §100 Übergangsregelung)
// ═══════════════════════════════════════════════════════════════════════════

/// §52 Abs. 2 EEG ≤2021 — VerguetungAufMarktwert: Vergütung → EPEX Marktwert.
/// Missing Fernsteuerbarkeit (§9 Abs. 1/2). §52 Abs. 2 Nr. 1.
/// EPEX July avg = 5.2 ct/kWh. Plant receives market value instead of tariff.
#[test]
fn s52_alt_verguetung_auf_marktwert() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("50000")),
        verguetungssatz_ct: d("14.5"),   // old EEG 2012 wind rate
        epex_avg_ct_kwh: Some(d("5.2")), // EPEX monthly avg needed for Marktwert
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufMarktwert),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    // Plant receives EPEX Marktwert instead of EEG tariff
    // 50,000 kWh × 5.2 ct / 100 = 2,600 EUR (vs 7,250 EUR at tariff)
    assert_eq!(out.settlement_eur, Some(d("2600.00")));
    assert_eq!(out.eligible_kwh, Some(d("50000")));
}

/// §52 Abs. 2 EEG ≤2021 — VerguetungAufMarktwert: no EPEX price → PriceMissing.
#[test]
fn s52_alt_marktwert_requires_epex_price() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("14.5"),
        epex_avg_ct_kwh: None, // EPEX missing → cannot compute Marktwert
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufMarktwert),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::PriceMissing);
    assert_eq!(out.settlement_eur, None);
}

/// §52 Abs. 3 EEG ≤2021 — VerguetungReduziert20Prozent: Vergütung × 0.80.
/// MaStR partially registered (§71 Nr. 1 done but incomplete data).
/// Plant receives 80% of normal tariff.
#[test]
fn s52_alt_verguetung_reduziert_20prozent() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("10000")),
        verguetungssatz_ct: d("10.0"),
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungReduziert20Prozent),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    // 10,000 kWh × 10.0 ct × 0.80 / 100 = 800 EUR (not 1,000 EUR)
    assert_eq!(out.settlement_eur, Some(d("800.00")));
}

/// §52 Abs. 3 — Rounding: result rounded to 2 decimal places per §52 Abs. 3.
#[test]
fn s52_alt_reduziert_rounding() {
    // 7,777 kWh × 11.11 ct × 0.80 / 100 = 691.17776 raw → rounded to 2dp
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("7777")),
        verguetungssatz_ct: d("11.11"),
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungReduziert20Prozent),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    // 7777 × 11.11 / 100 = 864.0247. × 0.80 = 691.21976 → rounded to 5dp internally
    // Then §52 Abs. 3: "wobei das Ergebnis auf zwei Stellen nach dem Komma gerundet wird"
    // The formula engine uses EuroAmount (5dp); §52 Abs. 3 external rounding is the operator's responsibility
    let eur = out.settlement_eur.unwrap();
    assert!(eur > d("0"), "non-zero sanction amount");
    // Verify it's 80% of unsanctioned (approximate due to precision)
    assert_eq!(
        eur,
        d("691.22"),
        "§52 Abs. 3 requires 2dp rounding of the 20% reduction"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §52 EEG 2023 — Pflichtzahlung (new regime, separate from Vergütung)
// ═══════════════════════════════════════════════════════════════════════════

/// §52 EEG 2023 — FernsteuerbarkeitmFehlend: €10/kW × 3 months.
/// Plant still receives full Vergütung; penalty is separate obligation.
#[test]
fn s52_2023_pflichtzahlung_fernsteuerbarkeit_fehlend() {
    use eeg_billing::foerderdauer::calculate_pflichtzahlung;
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    let violation = Pflichtverstoss {
        typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
        leistung_kw: d("500"),
        monate_des_verstosses: 3,
        nachtraeglich_erfuellt: false,
    };
    let penalty = calculate_pflichtzahlung(&violation);
    assert_eq!(penalty, d("15000")); // 500 × €10 × 3 = €15,000
}

/// §52 EEG 2023 — Retroactive reduction to €2/kW after fulfillment (§52 Abs. 3 Nr. 1).
#[test]
fn s52_2023_nachtraegliche_erfuellung_reduziert_auf_2eur() {
    use eeg_billing::foerderdauer::calculate_pflichtzahlung;
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    // Before fulfillment: €10/kW/month
    let before = calculate_pflichtzahlung(&Pflichtverstoss {
        typ: SanktionsTyp::MastrNichtRegistriert,
        leistung_kw: d("100"),
        monate_des_verstosses: 6,
        nachtraeglich_erfuellt: false,
    });
    assert_eq!(before, d("6000")); // 100 × €10 × 6

    // After fulfillment: retroactively reduced to €2/kW/month
    let after = calculate_pflichtzahlung(&Pflichtverstoss {
        typ: SanktionsTyp::MastrNichtRegistriert,
        leistung_kw: d("100"),
        monate_des_verstosses: 6,
        nachtraeglich_erfuellt: true,
    });
    assert_eq!(after, d("1200")); // 100 × €2 × 6
    assert!(
        after < before,
        "fulfilled obligation should have lower penalty"
    );
}

/// §52 EEG 2023 — SpeicherAnforderungNichtErfuellt: always €10/kW (no reduction).
/// §52 Abs. 3 Nr. 2 does NOT cover this type — fulfillment has no effect.
#[test]
fn s52_2023_speicher_always_10eur_no_reduction() {
    use eeg_billing::foerderdauer::calculate_pflichtzahlung;
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    let without = calculate_pflichtzahlung(&Pflichtverstoss {
        typ: SanktionsTyp::SpeicherAnforderungNichtErfuellt,
        leistung_kw: d("200"),
        monate_des_verstosses: 2,
        nachtraeglich_erfuellt: false,
    });
    let with_fulfilment = calculate_pflichtzahlung(&Pflichtverstoss {
        typ: SanktionsTyp::SpeicherAnforderungNichtErfuellt,
        leistung_kw: d("200"),
        monate_des_verstosses: 2,
        nachtraeglich_erfuellt: true, // Has NO effect for Speicher type
    });
    assert_eq!(without, d("4000")); // 200 × €10 × 2
    assert_eq!(with_fulfilment, d("4000")); // Same — no reduction
}

/// §52 EEG 2023 — VolleinspeisungspflichtVerletzt: always €2/kW (§52 Abs. 3 Nr. 2).
/// §48 Abs. 2a violation: plant declared Volleinspeisung but didn't deliver 100%.
#[test]
fn s52_2023_volleinspeisung_always_2eur() {
    use eeg_billing::foerderdauer::calculate_pflichtzahlung;
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    let penalty = calculate_pflichtzahlung(&Pflichtverstoss {
        typ: SanktionsTyp::VolleinspeisungspflichtVerletzt,
        leistung_kw: d("50"),
        monate_des_verstosses: 12, // All 12 months of the calendar year (§52 Abs. 4 Nr. 3)
        nachtraeglich_erfuellt: false,
    });
    assert_eq!(penalty, d("1200")); // 50 × €2 × 12 (always €2/kW for this type)
}

/// §52 EEG 2023 — Vergütung continues during penalty period (separate from old §52).
/// Plant receives full Vergütung AND owes the penalty separately.
#[test]
fn s52_2023_vergütung_plus_pflichtzahlung_independent() {
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        pflichtverstoss: vec![Pflichtverstoss {
            typ: SanktionsTyp::MastrNichtRegistriert,
            leistung_kw: d("10"),
            monate_des_verstosses: 1,
            nachtraeglich_erfuellt: false,
        }],
        ..SettleInput::default()
    });
    // Vergütung: 500 × 8.11 / 100 = 40.55 EUR (paid to plant)
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("40.55")));
    // Pflichtzahlung: 10 kW × €10 × 1 month = €100 (plant owes to NB separately)
    assert_eq!(out.pflichtzahlung_eur, Some(d("100")));
    // These are INDEPENDENT — Vergütung is NOT reduced by the §52 penalty
}

// ═══════════════════════════════════════════════════════════════════════════
// §50a EEG 2023 — FlexibilitaetZuschlag (new biomass plants)
// ═══════════════════════════════════════════════════════════════════════════

/// §50a EEG 2023 — capacity-based monthly payment (€100/kW/year ÷ 12).
/// 200 kW flex capacity, €100/kW/year statutory rate.
/// Monthly: 200 × 100 / 12 = 1,666.67 EUR
#[test]
fn s50a_flexibilitaetszuschlag_monthly_capacity_payment() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FlexibilitySurcharge,
        leistung_kwp: Some(d("200")), // flex capacity in kW
        verguetungssatz_ct: d("100"), // statutory €100/kW/year
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 200 kW × 100 EUR/kW/year ÷ 12 months = 1,666.667 EUR/month
    let expected = (d("200") * d("100") / d("12")).round_dp(5);
    assert_eq!(out.settlement_eur, Some(expected));
    assert_eq!(out.positions.len(), 1);
    assert!(out.positions[0].legal_basis.contains("50a"));
}

/// §50a is distinct from §50b: it's for NEW plants (neue Anlagen).
/// §50b is for EXISTING plants (bestehende Anlagen) + kWh-based.
/// §50a is purely capacity-based (kW × rate / 12), independent of kWh produced.
#[test]
fn s50a_independent_of_kwh_produced() {
    let with_kwh = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FlexibilitySurcharge,
        leistung_kwp: Some(d("300")),
        verguetungssatz_ct: d("100"),
        einspeisemenge_kwh: Some(d("200000")), // kWh supplied but irrelevant
        ..SettleInput::default()
    });
    let without_kwh = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FlexibilitySurcharge,
        leistung_kwp: Some(d("300")),
        verguetungssatz_ct: d("100"),
        einspeisemenge_kwh: None, // no kWh data
        ..SettleInput::default()
    });
    // Both should produce the same payment (300 × 100 / 12)
    assert_eq!(with_kwh.settlement_eur, without_kwh.settlement_eur);
    assert_eq!(with_kwh.status, SettlementStatus::Calculated);
}

// ═══════════════════════════════════════════════════════════════════════════
// §19 EEG — Einspeisemanagement (curtailment) compensation
// ═══════════════════════════════════════════════════════════════════════════

/// §19 EEG 2023 — NB curtails plant: compensation for lost generation.
/// Plant produces 1,000 kWh but 150 kWh were curtailed by NB.
/// Einspeisemenge = 850 kWh. EinsMan compensation = 150 × 8.11 / 100 = 12.165 EUR.
/// Total = 850 × 8.11 / 100 + 150 × 8.11 / 100 = 81.10 EUR (as if 1,000 kWh).
#[test]
fn s19_einspeisemanagement_compensation() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("850")), // measured feed-in (after curtailment)
        verguetungssatz_ct: d("8.11"),
        einspeisemanagement_kwh: Some(d("150")), // 150 kWh curtailed by NB
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Regular settlement: 850 × 8.11 / 100 = 68.935 EUR
    // EinsMan compensation: 150 × 8.11 / 100 = 12.165 EUR
    // Total: 81.100 EUR
    assert_eq!(out.settlement_eur, Some(d("81.100")));
    // eligible_kwh includes both measured + EinsMan
    assert_eq!(out.eligible_kwh, Some(d("1000")));
    // Separate §19 position
    assert!(
        out.positions.iter().any(|p| p.legal_basis.contains("§19")),
        "§19 EinsMan position expected"
    );
    let einsman_pos = out
        .positions
        .iter()
        .find(|p| p.legal_basis.contains("§19"))
        .unwrap();
    assert_eq!(einsman_pos.kwh, d("150"));
    assert_eq!(einsman_pos.eur, d("12.165"));
}

/// §19 EEG — EinsMan also applies to Direktvermarktung plants (uses AW as rate).
#[test]
fn s19_einspeisemanagement_direktvermarktung_uses_aw_rate() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("100000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.0")),
        managementpraemie_ct: Some(d("0.4")),
        einspeisemanagement_kwh: Some(d("5000")), // 5,000 kWh curtailed
        ..SettleInput::default()
    });
    // Regular: (6.0-4.5)×100k/100 + 0.4×100k/100 = 1,500 + 400 = 1,900 EUR
    // EinsMan: 5,000 × 6.0 / 100 = 300 EUR (uses AW as compensation rate)
    // Total: 2,200 EUR
    assert_eq!(out.settlement_eur, Some(d("2200.00")));
    let einsman = out
        .positions
        .iter()
        .find(|p| p.legal_basis.contains("§19"))
        .unwrap();
    assert_eq!(einsman.rate_ct_kwh, d("6.0")); // AW used as rate
    assert_eq!(einsman.eur, d("300.00"));
}

/// §19 EEG — No curtailment in billing period → no §19 position.
#[test]
fn s19_no_curtailment_no_einsman_position() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        einspeisemanagement_kwh: None, // no curtailment
        ..SettleInput::default()
    });
    assert!(
        out.positions.iter().all(|p| !p.legal_basis.contains("§19")),
        "no §19 position when einspeisemanagement_kwh is None"
    );
    assert_eq!(out.settlement_eur, Some(d("81.10")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §36k EEG — Wind Korrekturfaktor (location correction)
// ═══════════════════════════════════════════════════════════════════════════

/// §36k EEG 2023 — Low-wind site: Korrekturfaktor > 1.0 → higher effective AW.
/// Base AW = 6.5 ct/kWh. Korrekturfaktor = 1.12 (poor wind site, Gütegrad ~85%).
/// Effective AW = 6.5 × 1.12 = 7.28 ct/kWh.
#[test]
fn s36k_korrekturfaktor_increases_aw_for_low_wind_site() {
    use eeg_billing::wind_onshore_korrekturfaktor_corrected_aw;

    let corrected_aw = wind_onshore_korrekturfaktor_corrected_aw(d("6.5"), d("1.12"));
    assert_eq!(corrected_aw, d("7.28000"));

    // Use in settlement
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("200000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.5")),      // base AW
        wind_korrekturfaktor: Some(d("1.12")), // §36k low-wind correction
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    // Effective AW = 6.5 × 1.12 = 7.28 ct
    // Spread = 7.28 - 4.5 = 2.78 ct/kWh
    // Marktprämie = 200,000 × 2.78 / 100 = 5,560 EUR
    // Managementprämie = 200,000 × 0.4 / 100 = 800 EUR
    // Total = 6,360 EUR
    assert_eq!(out.settlement_eur, Some(d("6360.00")));
}

/// §36k EEG 2023 — High-wind site: Korrekturfaktor < 1.0 → lower effective AW.
/// Base AW = 6.5 ct/kWh. Korrekturfaktor = 0.78 (high-wind site, Gütegrad ~130%).
/// Effective AW = 6.5 × 0.78 = 5.07 ct/kWh.
#[test]
fn s36k_korrekturfaktor_decreases_aw_for_high_wind_site() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("300000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.5")),
        wind_korrekturfaktor: Some(d("0.78")), // high-wind site correction
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    // Effective AW = 6.5 × 0.78 = 5.07 ct
    // Spread = 5.07 - 4.5 = 0.57 ct/kWh
    // Marktprämie = 300,000 × 0.57 / 100 = 1,710 EUR
    // Managementprämie = 300,000 × 0.4 / 100 = 1,200 EUR
    // Total = 2,910 EUR
    assert_eq!(out.settlement_eur, Some(d("2910.00")));
}

/// §36k — Korrekturfaktor 1.0 = no change (reference yield site).
#[test]
fn s36k_korrekturfaktor_1_0_no_change() {
    let with_k = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("100000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.0")),
        wind_korrekturfaktor: Some(d("1.0")), // reference site → no change
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    let without_k = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("100000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.0")),
        wind_korrekturfaktor: None, // no correction factor
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(with_k.settlement_eur, without_k.settlement_eur);
}

/// §36k — Pre-2016 plants: Korrekturfaktor not applicable (Bestandsschutz §100).
/// Setting wind_korrekturfaktor = None for old EEG2012 plants is mandatory.
/// This test verifies no correction is applied when None.
#[test]
fn s36k_bestandsschutz_no_correction_for_pre_2016() {
    use eeg_billing::EegGesetz;
    // EEG 2012 wind plant: §36k Bestandsschutz → no Korrekturfaktor
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("100000")),
        verguetungssatz_ct: d("8.9"), // EEG 2012 wind rate (net after §53)
        wind_korrekturfaktor: None,   // correct for EEG ≤2012
        eeg_gesetz: EegGesetz::Eeg2012,
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(d("8900.00")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §24 EEG — Anlagenerweiterung (CapacityBlock multi-block settlement)
// ═══════════════════════════════════════════════════════════════════════════

/// §24 EEG 2023 — Plant extension: original 10 kWp + 5 kWp extension.
/// Different rates per block; total kWh allocated proportionally (10:5 = 2:1).
/// 900 kWh total: original 600 kWh × 9.25 ct + extension 300 kWh × 8.11 ct.
#[test]
fn s24_capacity_block_proportional_allocation() {
    use eeg_billing::CapacityBlock;

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("900")),
        verguetungssatz_ct: d("9.25"), // original block rate
        leistung_kwp: Some(d("10")),   // original capacity
        inbetriebnahme: Some(date!(2020 - 03 - 15)),
        foerderendedatum: Some(date!(2040 - 12 - 31)),
        capacity_blocks: vec![CapacityBlock {
            leistung_kwp: d("5"),
            verguetungssatz_ct: d("8.11"), // extension block rate (lower due to degression)
            inbetriebnahme: date!(2024 - 06 - 01),
            foerderendedatum: date!(2044 - 12 - 31),
        }],
        billing_date: Some(date!(2026 - 07 - 01)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.positions.len(), 2, "one position per active block");
    // Block 1: 10/15 × 900 = 600 kWh × 9.25 ct = 55.500 EUR
    // Block 2:  5/15 × 900 = 300 kWh × 8.11 ct = 24.330 EUR
    // Total: 79.830 EUR
    assert_eq!(out.settlement_eur, Some(d("79.830")));
    assert_eq!(out.eligible_kwh, Some(d("900")));
}

/// §24 EEG — Expired block contributes EUR 0; active block gets its proportional share.
/// Proportional allocation is FIXED by capacity ratios (10 kWp : 5 kWp = 2:1).
/// When primary block expires, active block still receives only its 1/3 share.
#[test]
fn s24_expired_block_contributes_zero() {
    use eeg_billing::CapacityBlock;

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("300")),
        verguetungssatz_ct: d("24.43"), // EEG 2012 rate (old)
        leistung_kwp: Some(d("10")),
        inbetriebnahme: Some(date!(2012 - 06 - 01)),
        foerderendedatum: Some(date!(2032 - 12 - 31)), // expired before billing_date
        capacity_blocks: vec![CapacityBlock {
            leistung_kwp: d("5"),
            verguetungssatz_ct: d("8.11"),
            inbetriebnahme: date!(2024 - 06 - 01),
            foerderendedatum: date!(2044 - 12 - 31),
        }],
        billing_date: Some(date!(2033 - 07 - 01)), // original block expired
        ..SettleInput::default()
    });
    // §24 proportional allocation: 5/(10+5) = 1/3 share of 300 kWh = 100 kWh
    // Extension block payment: 100 × 8.11 ct / 100 = 8.11 EUR
    // Primary block is expired → 0 EUR contribution
    assert_eq!(out.settlement_eur, Some(d("8.11000")));
    assert_eq!(out.positions.len(), 1, "only active block has a position");
}

// ═══════════════════════════════════════════════════════════════════════════
// Rate lookup tables — eeg_billing::rates
// ═══════════════════════════════════════════════════════════════════════════

/// Solar PV Solarpaket I (2024): Gebäudeanlage ≤10 kWp → 8.51 ct/kWh.
#[test]
fn rates_solar_pv_solarpaket_i_2024_ueberschuss() {
    use eeg_billing::rates;
    let table = rates::solar_pv_ueberschuss_lookup(2024).expect("EEG 2024 rates known");
    let rate = table.rate_for(d("9")).expect("9 kWp in table");
    assert_eq!(rate, billing::Amount::parse("0.08510").unwrap());
}

/// Solar PV Volleinspeisung 2024 (Solarpaket I): higher rate for 100% grid feed-in.
#[test]
fn rates_solar_pv_volleinspeisung_2024_higher_than_ueberschuss() {
    use eeg_billing::rates;
    let u = rates::solar_pv_ueberschuss_lookup(2024).unwrap();
    let v = rates::solar_pv_volleinspeisung_lookup(2024).unwrap();
    let rate_u = u.rate_for(d("9")).unwrap();
    let rate_v = v.rate_for(d("9")).unwrap();
    assert!(
        rate_v > rate_u,
        "Volleinspeisung rate must exceed Überschuss rate"
    );
}

/// §53 deduction: solar PV = −0.4 ct/kWh; biomass = −0.2 ct/kWh.
#[test]
fn rates_sect53_deduction_by_technology() {
    use eeg_billing::{ErzeugungsArt, rates};
    // Solar PV: −0.4 ct/kWh
    assert_eq!(
        rates::sect53_deduction(ErzeugungsArt::SolarAufdach),
        d("0.4")
    );
    // Wind: −0.4 ct/kWh
    assert_eq!(
        rates::sect53_deduction(ErzeugungsArt::WindOnshore),
        d("0.4")
    );
    // Biomasse: −0.2 ct/kWh
    assert_eq!(rates::sect53_deduction(ErzeugungsArt::Biomasse), d("0.2"));
    // Wasserkraft: −0.2 ct/kWh
    assert_eq!(
        rates::sect53_deduction(ErzeugungsArt::Wasserkraft),
        d("0.2")
    );
    // KWKG: no §53 deduction (0.0 ct/kWh)
    assert_eq!(rates::sect53_deduction(ErzeugungsArt::Kwk), d("0.0"));
}

// ═══════════════════════════════════════════════════════════════════════════
// §52 Abs. 5 EEG 2023 — Multiple simultaneous violations (monthly cap)
// ═══════════════════════════════════════════════════════════════════════════

/// §52 Abs. 5 — Multiple violations sum; cap at €10/kW/month.
/// Plant has both MaStR violation AND Fernsteuerbarkeit missing.
/// Without cap: €10/kW × 2 violations = €20/kW. Cap: €10/kW.
#[test]
fn s52_abs5_multiple_violations_capped_at_10eur_per_kw() {
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        pflichtverstoss: vec![
            Pflichtverstoss {
                typ: SanktionsTyp::MastrNichtRegistriert,
                leistung_kw: d("100"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
            },
            Pflichtverstoss {
                typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
                leistung_kw: d("100"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
            },
        ],
        ..SettleInput::default()
    });
    // Without cap: 100 × €10 + 100 × €10 = €2,000
    // With §52 Abs. 5 cap: max = 100 × €10 × 1 month = €1,000
    assert_eq!(out.pflichtzahlung_eur, Some(d("1000")));
    // Vergütung is unaffected: 1,000 × 8.11 / 100 = 81.10 EUR
    assert_eq!(out.settlement_eur, Some(d("81.10")));
    assert_eq!(out.status, SettlementStatus::Calculated);
}

/// §52 — Multiple violations, some fulfilled: each computed independently.
#[test]
fn s52_multiple_violations_independent_computation() {
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        pflichtverstoss: vec![
            Pflichtverstoss {
                typ: SanktionsTyp::MastrNichtRegistriert,
                leistung_kw: d("50"),
                monate_des_verstosses: 3,
                nachtraeglich_erfuellt: true, // retroactive reduction
            },
            Pflichtverstoss {
                typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
                leistung_kw: d("50"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
            },
        ],
        ..SettleInput::default()
    });
    // MaStR: 50 × €2 × 3 = €300 (retroactively reduced to €2)
    // Fernsteuerbarkeit: 50 × €10 × 1 = €500
    // Sum: €800; cap = 50 × €10 × 3 (max months) = €1500
    // €800 < €1500 cap → not capped
    assert_eq!(out.pflichtzahlung_eur, Some(d("800")));
}

/// §52 — Empty violations list → no pflichtzahlung.
#[test]
fn s52_empty_vec_no_pflichtzahlung() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        pflichtverstoss: vec![],
        ..SettleInput::default()
    });
    assert_eq!(out.pflichtzahlung_eur, None);
    assert_eq!(out.settlement_eur, Some(d("40.55")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §51a EEG 2023 — Verlängerungsanspruch (payment period extension)
// ═══════════════════════════════════════════════════════════════════════════

/// §51a Abs. 1 EEG 2023 — Wind plant: 1:1 extension.
/// 240 quarter-hours with negative prices → Förderdauer extended by 240 QH.
#[test]
fn s51a_wind_one_to_one_extension() {
    use eeg_billing::foerderdauer::verguetungszeitraum_verlaengerung_qh;
    use eeg_billing::{EegGesetz, ErzeugungsArt};

    // Verify the helper function
    assert_eq!(verguetungszeitraum_verlaengerung_qh(240, false), 240);
    assert_eq!(verguetungszeitraum_verlaengerung_qh(100, false), 100);

    // Settlement with §51 applied + QH tracking
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("50000")),
        verguetungssatz_ct: d("5.5"),
        kwh_during_negative_epex: Some(d("2000")), // §51 reduced kWh
        negative_price_quarter_hours: Some(240),   // 60h = 240 QH at negative EPEX
        eeg_gesetz: EegGesetz::Eeg2023,
        leistung_kwp: Some(d("500")),
        erzeugungsart: Some(ErzeugungsArt::WindOnshore),
        ..SettleInput::default()
    });
    // §51a wind: 1:1 extension → 240 QH
    assert_eq!(out.verlaengerungsanspruch_qh, 240);
    // §51 applied: eligible = 50000 - 2000 = 48000 kWh
    assert_eq!(out.eligible_kwh, Some(d("48000")));
}

/// §51a Abs. 2 EEG 2023 — Solar PV plant: ceil(lost_qh / 2) extension.
/// 100 quarter-hours → ceil(100/2) = 50 QH extension.
#[test]
fn s51a_solar_half_extension_factor() {
    use eeg_billing::foerderdauer::verguetungszeitraum_verlaengerung_qh;
    use eeg_billing::{EegGesetz, ErzeugungsArt};

    // Verify the helper: solar uses 0.5 factor (rounded up)
    assert_eq!(verguetungszeitraum_verlaengerung_qh(100, true), 50);
    assert_eq!(verguetungszeitraum_verlaengerung_qh(101, true), 51); // ceil(101/2)

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        kwh_during_negative_epex: Some(d("50")),
        negative_price_quarter_hours: Some(100), // 25h negative
        eeg_gesetz: EegGesetz::Eeg2023,
        leistung_kwp: Some(d("200")),
        erzeugungsart: Some(ErzeugungsArt::SolarAufdach),
        ..SettleInput::default()
    });
    // §51a solar: ceil(100/2) = 50 QH extension
    assert_eq!(out.verlaengerungsanspruch_qh, 50);
}

/// §51a — No QH tracking when negative_price_quarter_hours not supplied.
#[test]
fn s51a_no_qh_input_no_extension() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        kwh_during_negative_epex: Some(d("50")),
        negative_price_quarter_hours: None, // not provided
        ..SettleInput::default()
    });
    assert_eq!(
        out.verlaengerungsanspruch_qh, 0,
        "no QH tracking when input not set"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §53b EEG 2023 — Regionale Grünstromkennzeichnung reduction
// ═══════════════════════════════════════════════════════════════════════════

/// §53b EEG 2023 — BNetzA regional reduction reduces Vergütung.
/// Plant in renewable-saturated area: 0.3 ct/kWh reduction.
/// 1,000 kWh × 8.11 ct = 81.10 EUR gross → 81.10 − 3.00 = 78.10 EUR net.
#[test]
fn s53b_regional_reduction_applied() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("8.11"),
        sect53b_regional_reduction_ct: Some(d("0.3")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Gross: 1000 × 8.11 / 100 = 81.10 EUR
    // §53b reduction: 1000 × 0.3 / 100 = 3.00 EUR
    // Net: 78.10 EUR
    assert_eq!(out.settlement_eur, Some(d("78.10")));
    assert_eq!(out.positions.len(), 2, "§21 Vergütung + §53b reduction");
    assert!(
        out.positions.iter().any(|p| p.legal_basis.contains("53b")),
        "§53b position expected"
    );
    let r53b = out
        .positions
        .iter()
        .find(|p| p.legal_basis.contains("53b"))
        .unwrap();
    assert_eq!(r53b.eur, d("-3.00")); // negative (reduces settlement)
}

/// §53b — Does NOT apply to Direktvermarktung (market pricing governs directly).
#[test]
fn s53b_not_applied_to_direktvermarktung() {
    let with_r53b = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("100000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.0")),
        managementpraemie_ct: Some(d("0.4")),
        sect53b_regional_reduction_ct: Some(d("0.5")),
        ..SettleInput::default()
    });
    let without_r53b = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("100000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("6.0")),
        managementpraemie_ct: Some(d("0.4")),
        sect53b_regional_reduction_ct: None,
        ..SettleInput::default()
    });
    // §53b must NOT affect Direktvermarktung
    assert_eq!(with_r53b.settlement_eur, without_r53b.settlement_eur);
}

/// §53b = None → no reduction.
#[test]
fn s53b_none_no_reduction() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        sect53b_regional_reduction_ct: None,
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(d("40.55")));
    assert_eq!(out.positions.len(), 1, "no §53b position when None");
}

// ═══════════════════════════════════════════════════════════════════════════
// InbetriebnahmeTyp — Plant lifecycle enum
// ═══════════════════════════════════════════════════════════════════════════

/// InbetriebnahmeTyp: Repowering resets the Förderdauer clock.
#[test]
fn inbetriebnahmetyp_repowering_resets_foerderdauer() {
    use eeg_billing::InbetriebnahmeTyp;

    assert!(InbetriebnahmeTyp::Repowering.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Erstinbetriebnahme.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Wiederinbetriebnahme.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Modernisierung.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Zusammenlegung.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Erweiterung.resets_foerderdauer());
}

/// InbetriebnahmeTyp DB roundtrip.
#[test]
fn inbetriebnahmetyp_db_roundtrip() {
    use eeg_billing::InbetriebnahmeTyp;

    let all = [
        InbetriebnahmeTyp::Erstinbetriebnahme,
        InbetriebnahmeTyp::Wiederinbetriebnahme,
        InbetriebnahmeTyp::Modernisierung,
        InbetriebnahmeTyp::Repowering,
        InbetriebnahmeTyp::Zusammenlegung,
        InbetriebnahmeTyp::Erweiterung,
    ];
    for t in all {
        let db_str = t.to_db_str();
        let parsed = InbetriebnahmeTyp::from_db_str(db_str).unwrap();
        assert_eq!(t, parsed, "DB roundtrip failed for {:?}", t);
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// §23a EEG 2023 — Degression
// ══════════════════════════════════════════════════════════════════════════════

/// §23a EEG 2023 — reference quarter uses unchanged rate.
#[test]
fn sect23a_reference_quarter_no_change() {
    use eeg_billing::degression::{
        DegressionTier, SOLARPAKET_I_REFERENCE_QUARTER, solar_ueberschuss_rate_for_quarter,
    };
    let rate = solar_ueberschuss_rate_for_quarter(
        SOLARPAKET_I_REFERENCE_QUARTER,
        d("9"),
        DegressionTier::None,
    );
    assert_eq!(rate, Some(d("8.51")));
}

/// §23a — 1 % Standard tier over 4 quarters reduces rate by ~3.9 %.
#[test]
fn sect23a_standard_4_quarters_below_reference() {
    use eeg_billing::degression::{DegressionTier, Quarter, solar_ueberschuss_rate_for_quarter};
    let rate = solar_ueberschuss_rate_for_quarter(
        Quarter {
            year: 2025,
            quarter: 2,
        }, // 4 quarters after Q2 2024
        d("9"),
        DegressionTier::Standard,
    );
    // 8.51 × 0.99^4 ≈ 8.17 ct/kWh
    assert_eq!(rate, Some(d("8.17")));
}

/// §23a — Volleinspeisung rate correctly degresssed.
#[test]
fn sect23a_volleinspeisung_degresssed() {
    use eeg_billing::degression::{
        DegressionTier, Quarter, solar_volleinspeisung_rate_for_quarter,
    };
    // Q2 2024 reference: 13.31 ct, Standard tier, 2 quarters → 13.31 × 0.99^2 ≈ 13.05
    let rate = solar_volleinspeisung_rate_for_quarter(
        Quarter {
            year: 2024,
            quarter: 4,
        },
        d("9"),
        DegressionTier::Standard,
    );
    assert_eq!(rate, Some(d("13.05")));
}

/// §23a — Plants > 1 MWp must use Ausschreibung — no statutory rate.
#[test]
fn sect23a_above_1mwp_no_statutory_rate() {
    use eeg_billing::degression::{
        DegressionTier, SOLARPAKET_I_REFERENCE_QUARTER, solar_ueberschuss_rate_for_quarter,
    };
    let rate = solar_ueberschuss_rate_for_quarter(
        SOLARPAKET_I_REFERENCE_QUARTER,
        d("2000"), // 2 MWp
        DegressionTier::None,
    );
    assert!(
        rate.is_none(),
        "No statutory rate above Ausschreibungspflicht threshold"
    );
}

/// §23a — historical plant (before Solarpaket I) returns None (use DB lookup).
#[test]
fn sect23a_historical_plant_use_db_lookup() {
    use eeg_billing::degression::{DegressionTier, Quarter, solar_ueberschuss_rate_for_quarter};
    let rate = solar_ueberschuss_rate_for_quarter(
        Quarter {
            year: 2021,
            quarter: 3,
        },
        d("9"),
        DegressionTier::Standard,
    );
    assert!(
        rate.is_none(),
        "Pre-Solarpaket I plants must use einsd DB lookup"
    );
}

/// §23a — Degression tier from GW expansion table.
#[test]
fn sect23a_tier_from_annual_gw_expansion() {
    use eeg_billing::degression::DegressionTier;
    // Germany installed ~16.5 GW in 2024 → Maximum tier
    assert_eq!(
        DegressionTier::from_gw_expansion(d("16.5")),
        DegressionTier::Maximum
    );
    // 2022 was ~7.9 GW → None tier
    assert_eq!(
        DegressionTier::from_gw_expansion(d("7.9")),
        DegressionTier::None
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// §§20–22 EEG 2023 — Direktvermarktung rules
// ══════════════════════════════════════════════════════════════════════════════

/// §20 EEG — mandatory Direktvermarktung for plants > 100 kW.
#[test]
fn sect20_mandatory_above_100kw() {
    use eeg_billing::EegGesetz;
    use eeg_billing::direktverm::is_direktvermarktung_mandatory;

    // Exactly 100 kW: NOT mandatory (§20 says "> 100 kW")
    assert!(!is_direktvermarktung_mandatory(
        d("100"),
        EegGesetz::Eeg2023
    ));

    // 100.001 kW: mandatory
    assert!(is_direktvermarktung_mandatory(
        d("100.001"),
        EegGesetz::Eeg2023
    ));

    // 750 kW wind: definitely mandatory
    assert!(is_direktvermarktung_mandatory(d("750"), EegGesetz::Eeg2023));
}

/// §20 — EEG 2009 plants are exempt from mandatory Direktvermarktung (§100 Übergangsregelung).
#[test]
fn sect20_eeg2009_plants_exempt_from_mandatory() {
    use eeg_billing::EegGesetz;
    use eeg_billing::direktverm::is_direktvermarktung_mandatory;

    // Even a large EEG 2009 plant may stay on Einspeisevergütung forever
    assert!(!is_direktvermarktung_mandatory(
        d("500"),
        EegGesetz::Eeg2009
    ));
    assert!(!is_direktvermarktung_mandatory(
        d("500"),
        EegGesetz::Eeg2000
    ));
}

/// §22 EEG — Ausschreibungspflicht thresholds.
#[test]
fn sect22_ausschreibung_thresholds() {
    use eeg_billing::ErzeugungsArt;
    use eeg_billing::direktverm::requires_ausschreibung;

    // Solar >1 MWp: tendering mandatory
    assert!(requires_ausschreibung(
        d("1001"),
        ErzeugungsArt::SolarFreiflaeche
    ));
    assert!(!requires_ausschreibung(
        d("999"),
        ErzeugungsArt::SolarAufdach
    ));

    // Wind onshore >750 kW: tendering mandatory
    assert!(requires_ausschreibung(d("751"), ErzeugungsArt::WindOnshore));
    assert!(!requires_ausschreibung(
        d("750"),
        ErzeugungsArt::WindOnshore
    ));

    // Wind offshore: always tendering (§23 EEG 2023)
    assert!(requires_ausschreibung(d("1"), ErzeugungsArt::WindOffshore));

    // Biomasse >150 kW: tendering mandatory
    assert!(requires_ausschreibung(d("151"), ErzeugungsArt::Biomasse));
    assert!(!requires_ausschreibung(d("150"), ErzeugungsArt::Biogas));
}

/// §21 Abs. 3 — monthly switch validation: mandatory plant cannot switch.
#[test]
fn sect21_mandatory_plant_cannot_switch_back() {
    use eeg_billing::EegGesetz;
    use eeg_billing::direktverm::{SwitchBlockedReason, validate_switch_to_vergütung};
    use time::macros::date;

    let result =
        validate_switch_to_vergütung(d("200"), EegGesetz::Eeg2023, date!(2025 - 07 - 01), None);
    assert_eq!(
        result,
        Err(SwitchBlockedReason::PflichtgemasseDirektvermarktung)
    );
}

/// §21 Abs. 3 — voluntary plant can switch once per month.
#[test]
fn sect21_voluntary_switch_once_per_month() {
    use eeg_billing::EegGesetz;
    use eeg_billing::direktverm::{SwitchBlockedReason, validate_switch_to_vergütung};
    use time::macros::date;

    // Different month → OK
    let ok = validate_switch_to_vergütung(
        d("80"),
        EegGesetz::Eeg2023,
        date!(2025 - 08 - 01),
        Some(date!(2025 - 07 - 01)),
    );
    assert!(ok.is_ok());

    // Same month → blocked
    let blocked = validate_switch_to_vergütung(
        d("80"),
        EegGesetz::Eeg2023,
        date!(2025 - 07 - 15),
        Some(date!(2025 - 07 - 01)),
    );
    assert!(matches!(
        blocked,
        Err(SwitchBlockedReason::AlreadySwitchedThisMonth { .. })
    ));
}

// ══════════════════════════════════════════════════════════════════════════════
// §36k EEG 2023 — Wind Standort (structured site model)
// ══════════════════════════════════════════════════════════════════════════════

/// §36k — WindStandort struct directly wired into SettleInput via wind_standort field.
#[test]
fn sect36k_wind_standort_auto_derives_korrekturfaktor() {
    use eeg_billing::wind::{WindStandort, WindStandortklasse};
    use eeg_billing::{SettleInput, SettlementScheme, SettlementStatus};

    let standort = WindStandort {
        guetegrad: dec!(0.85),
        korrekturfaktor: dec!(1.08),
        grundverguetungsperiode_aktiv: true,
        standortklasse: WindStandortklasse::BelowReference,
    };

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("1000")),
        direktverm_aw_ct: Some(d("6.28")), // base AW
        epex_avg_ct_kwh: Some(d("4.00")),
        wind_standort: Some(standort), // korrekturfaktor = 1.08
        // wind_korrekturfaktor intentionally NOT set → derived from standort
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);

    // Effective AW = 6.28 × 1.08 = 6.7824 ct → Prämie = 6.7824 − 4.00 = 2.7824 ct
    // settlement = 1000 kWh × 2.7824 ct / 100 = 27.824 EUR
    assert!(out.settlement_eur.is_some());
    let eur = out.settlement_eur.unwrap();
    // Should be > 27.00 EUR (corrected AW > base AW)
    assert!(eur > d("27.00") && eur < d("29.00"), "unexpected: {eur}");
}

/// §36k — Explicit wind_korrekturfaktor takes precedence over wind_standort.
#[test]
fn sect36k_explicit_korrekturfaktor_wins_over_standort() {
    use eeg_billing::wind::{WindStandort, WindStandortklasse};
    use eeg_billing::{SettleInput, SettlementScheme};

    let standort = WindStandort {
        guetegrad: dec!(0.85),
        korrekturfaktor: dec!(1.08), // standort says 1.08
        grundverguetungsperiode_aktiv: true,
        standortklasse: WindStandortklasse::BelowReference,
    };

    let out_explicit = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("1000")),
        direktverm_aw_ct: Some(d("6.28")),
        epex_avg_ct_kwh: Some(d("4.00")),
        wind_korrekturfaktor: Some(d("1.05")), // explicit override: 1.05
        wind_standort: Some(standort),
        ..SettleInput::default()
    });

    // Explicit 1.05 applies: AW = 6.28 × 1.05 = 6.594 → Prämie = 2.594 ct
    let out_implicit = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("1000")),
        direktverm_aw_ct: Some(d("6.28")),
        epex_avg_ct_kwh: Some(d("4.00")),
        wind_korrekturfaktor: Some(d("1.08")), // same as standort.korrekturfaktor
        ..SettleInput::default()
    });

    // settlement with 1.05 should be less than with 1.08
    assert!(out_explicit.settlement_eur < out_implicit.settlement_eur);
}

// ══════════════════════════════════════════════════════════════════════════════
// §100 EEG 2023 Übergangsregelung
// ══════════════════════════════════════════════════════════════════════════════

/// §100 — EEG 2017 plant commissioned before 2023 continues under its original rules.
///
/// The settlement FORMULA is identical. What changes:
/// - The Vergütungssatz is the one fixed at EEG 2017 commissioning date
/// - §51 applies EEG 2017 rules (≥6 consecutive hours, wind <3MW/other <500kW)
/// - §52 sanction uses SanktionAlt (old regime), not Pflichtverstoss (new EEG 2023)
#[test]
fn sect100_uebergangsregelung_eeg2017_plant_in_2025() {
    use eeg_billing::{EegGesetz, SettleInput, SettlementScheme};

    // EEG 2017 solar plant, 50 kWp, commissioned 2019
    // Still billable in 2025 — rate was fixed at 2019 commissioning (e.g. 10.02 ct/kWh)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("10.02"), // EEG 2017 rate from 2019
        eeg_gesetz: EegGesetz::Eeg2017,
        leistung_kwp: Some(d("50")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 500 kWh × 10.02 ct = 50.10 EUR
    assert_eq!(out.settlement_eur, Some(d("50.10")));
}

/// §100 — Old §52 EEG ≤2021 MaStR sanction: Vergütung auf Null.
#[test]
fn sect100_old_mastr_sanction_vergütung_null() {
    use eeg_billing::{EegGesetz, SanktionAlt, SettleInput, SettlementScheme, SettlementStatus};

    // EEG 2017 plant not registered in MaStR — §52 Abs. 1 old regime → Vergütung = 0 (old regime §52 Abs. 1 Nr. 1)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("10.02"),
        eeg_gesetz: EegGesetz::Eeg2017,
        sanktion: Some(SanktionAlt::VerguetungAufNull),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    assert_eq!(out.settlement_eur, Some(d("0")));
}

/// §100 — Old §52 Abs. 2: missing Fernsteuerbarkeit under EEG 2017/2021 → EPEX Marktwert.
#[test]
fn sect100_fernsteuerbarkeit_missing_eeg2017_marktwert() {
    use eeg_billing::{EegGesetz, SanktionAlt, SettleInput, SettlementScheme, SettlementStatus};

    // EEG 2017 plant, Fernsteuerbarkeit not installed → Vergütung = EPEX Marktwert
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("1000")),
        verguetungssatz_ct: d("12.35"), // EEG 2017 rate
        eeg_gesetz: EegGesetz::Eeg2017,
        epex_avg_ct_kwh: Some(d("5.50")), // EPEX monthly average
        sanktion: Some(SanktionAlt::VerguetungAufMarktwert),
        ..SettleInput::default()
    });
    // §52 Abs. 2 EEG ≤2021 uses Sanctioned status (not Calculated)
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    // Vergütung = 1000 kWh × 5.50 ct = 55.00 EUR
    assert_eq!(out.settlement_eur, Some(d("55.00")));
}

// ══════════════════════════════════════════════════════════════════════════════
// §24 EEG 2023 — Multi-block Anlagenerweiterung
// ══════════════════════════════════════════════════════════════════════════════

/// §24 EEG — multi-block settlement allocates kWh proportionally by kWp.
#[test]
fn sect24_multi_block_proportional_allocation() {
    use eeg_billing::{CapacityBlock, SettleInput, SettlementScheme, SettlementStatus};
    use time::macros::date;

    // Primary block: 10 kWp at 9.25 ct/kWh (EEG 2020 rate)
    // Extension block: 5 kWp at 8.11 ct/kWh (EEG 2023 rate)
    // Total: 15 kWp, 300 kWh input → 200 kWh to primary, 100 kWh to extension
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("300")),
        verguetungssatz_ct: d("9.25"), // primary block rate
        leistung_kwp: Some(d("10")),
        inbetriebnahme: Some(date!(2020 - 06 - 01)),
        foerderendedatum: Some(date!(2040 - 12 - 31)),
        capacity_blocks: vec![CapacityBlock {
            leistung_kwp: d("5"),
            verguetungssatz_ct: d("8.11"),
            inbetriebnahme: date!(2024 - 03 - 01),
            foerderendedatum: date!(2044 - 03 - 01),
        }],
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);

    // Primary: 300 × (10/15) = 200 kWh × 9.25 ct = 18.50 EUR
    // Extension: 300 × (5/15) = 100 kWh × 8.11 ct = 8.11 EUR
    // Total: 26.61 EUR
    let total = out.settlement_eur.unwrap();
    assert!(
        (total - d("26.61")).abs() < d("0.01"),
        "unexpected: {total}"
    );
    assert_eq!(out.positions.len(), 2);
}

/// §24 EEG — expired block contributes EUR 0, active block continues normally.
#[test]
fn sect24_expired_block_excluded() {
    use eeg_billing::{CapacityBlock, SettleInput, SettlementScheme, SettlementStatus};
    use time::macros::date;

    // Primary: 10 kWp — still active (expires 2043)
    // Extension: 5 kWp — expired in 2025 (foerderendedatum 2025-01-01, billing_date 2026-01-01)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("300")),
        verguetungssatz_ct: d("9.25"),
        leistung_kwp: Some(d("10")),
        inbetriebnahme: Some(date!(2023 - 06 - 01)),
        foerderendedatum: Some(date!(2043 - 12 - 31)),
        billing_date: Some(date!(2026 - 01 - 01)),
        capacity_blocks: vec![CapacityBlock {
            leistung_kwp: d("5"),
            verguetungssatz_ct: d("8.11"),
            inbetriebnahme: date!(2005 - 01 - 01),
            foerderendedatum: date!(2025 - 01 - 01), // already expired
        }],
        ..SettleInput::default()
    });
    // Primary block still active, extension block expired
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Only the primary position should appear
    assert_eq!(
        out.positions.len(),
        1,
        "expired block should not produce a position"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// §52 EEG 2023 — Multiple simultaneous violations
// ══════════════════════════════════════════════════════════════════════════════

/// §52 — two simultaneous violations: MaStR + Fernsteuerbarkeit.
#[test]
fn sect52_two_simultaneous_violations_pflichtzahlung() {
    use eeg_billing::{Pflichtverstoss, SanktionsTyp, SettleInput, SettlementScheme};

    // Plant with both violations: MaStR not registered + Fernsteuerbarkeit missing
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.51"),
        pflichtverstoss: vec![
            Pflichtverstoss {
                typ: SanktionsTyp::MastrNichtRegistriert,
                leistung_kw: d("50"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
            },
            Pflichtverstoss {
                typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
                leistung_kw: d("50"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
            },
        ],
        ..SettleInput::default()
    });

    // Both violations: MaStR = €10/kW/month + Fernsteuerbarkeit = €10/kW/month
    // But §52 Abs. 5 cap = €10/kW × 1 month = €500
    // Sum = 50 × 10 + 50 × 10 = €1000 → capped at €500
    assert!(out.pflichtzahlung_eur.is_some());
    let pz = out.pflichtzahlung_eur.unwrap();
    assert_eq!(pz, d("500.00"), "§52 Abs. 5 cap should limit to 500 EUR");

    // Vergütung still calculated (EEG 2023 §52 does NOT reduce Vergütung)
    assert_eq!(out.settlement_eur, Some(d("42.55")));
}

// ══════════════════════════════════════════════════════════════════════════════
// KWKG 2023 — Hour limit enforcement
// ══════════════════════════════════════════════════════════════════════════════

/// KWKG §8 — year limit: when cumulative kWh approaches max, eligible kWh is capped.
#[test]
fn kwkg_hour_limit_caps_eligible_kwh() {
    use eeg_billing::{SettleInput, SettlementScheme, SettlementStatus};

    // 100 kW CHP; 60,000 h × 100 kW = 6,000,000 kWh max
    // Already used: 5,990,000 kWh → only 10,000 kWh remaining
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::KwkSurcharge,
        einspeisemenge_kwh: Some(d("50000")), // 50,000 kWh this month — exceeds remaining
        verguetungssatz_ct: d("8.00"),        // KWKG rate for ≤50 kW bracket used here
        kwk_strom_kwh_gesamt: Some(d("5990000")),
        kwk_max_kwh: Some(d("6000000")),
        ..SettleInput::default()
    });
    // When limit is reached mid-period, status = FoerderungBeendet (final billing)
    assert_eq!(out.status, SettlementStatus::FoerderungBeendet);

    // Eligible kWh = min(50000, 6000000 - 5990000) = 10000
    assert_eq!(out.eligible_kwh, Some(d("10000")));

    // Settlement = 10000 × 8.00 ct / 100 = 800 EUR
    assert_eq!(out.settlement_eur, Some(d("800.00")));
}

// ══════════════════════════════════════════════════════════════════════════════
// §50b EEG 2023 — Flexibilitätsprämie (bestehende Biomasseanlagen)
// ══════════════════════════════════════════════════════════════════════════════

/// §50b — Flexibilitätsprämie: Vergütung + Flexibilitätsprämie ct/kWh.
#[test]
fn sect50b_flexibilitaetspraemie_biomasse() {
    use eeg_billing::{SettleInput, SettlementScheme, SettlementStatus};

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FlexibilityPremium,
        einspeisemenge_kwh: Some(d("2000")),
        verguetungssatz_ct: d("14.47"), // Biomasse net rate (14.67 − 0.20 §53)
        flex_praemie_ct_kwh: Some(d("1.30")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);

    // Vergütung: 2000 × 14.47 / 100 = 289.40 EUR
    // Flexprämie: 2000 × 1.30 / 100 = 26.00 EUR
    // Total: 315.40 EUR
    assert!(out.settlement_eur.is_some());
    let total = out.settlement_eur.unwrap();
    assert!(
        (total - d("315.40")).abs() < d("0.01"),
        "unexpected: {total}"
    );
    assert_eq!(out.positions.len(), 2);
}

// ══════════════════════════════════════════════════════════════════════════════
// §51a EEG 2023 — Verlängerungsanspruch for Agri-PV
// ══════════════════════════════════════════════════════════════════════════════

/// §51a — Agri-PV gets 0.5 factor on Verlängerungsanspruch (§51a Abs. 2).
#[test]
fn sect51a_agripv_half_factor() {
    use eeg_billing::{ErzeugungsArt, SettleInput, SettlementScheme};

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.51"),
        leistung_kwp: Some(d("100")),
        erzeugungsart: Some(ErzeugungsArt::SolarAgriPv),
        kwh_during_negative_epex: Some(d("50")), // some negative-price kWh
        negative_price_quarter_hours: Some(40),  // 40 quarter-hours
        ..SettleInput::default()
    });

    // Agri-PV: Verlängerungsanspruch = ceil(40 / 2) = 20 QH (§51a Abs. 2, factor 0.5)
    assert_eq!(out.verlaengerungsanspruch_qh, 20);
}

/// §51a — non-solar plant gets 1:1 Verlängerungsanspruch.
#[test]
fn sect51a_non_solar_full_factor() {
    use eeg_billing::{EegGesetz, ErzeugungsArt, SettleInput, SettlementScheme};

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("2000")),
        verguetungssatz_ct: d("14.47"),
        leistung_kwp: Some(d("500")),
        erzeugungsart: Some(ErzeugungsArt::Biomasse),
        eeg_gesetz: EegGesetz::Eeg2023,
        kwh_during_negative_epex: Some(d("200")),
        negative_price_quarter_hours: Some(24),
        ..SettleInput::default()
    });

    // Biomasse (not solar): Verlängerungsanspruch = 24 QH (1:1 factor)
    assert_eq!(out.verlaengerungsanspruch_qh, 24);
}

// ══════════════════════════════════════════════════════════════════════════════
// Multi-Messkonzept — metering module
// ══════════════════════════════════════════════════════════════════════════════

/// Bidirectional meter: Überschusseinspeisung billing uses direct injection measurement.
#[test]
fn messkonzept_ueberschuss_bidirectional_meter() {
    use eeg_billing::Messkonzept;
    use eeg_billing::metering::{EinspeisemengeInput, compute_einspeisemenge};

    // 400 kWh generated, 150 kWh self-consumed, 250 kWh fed into grid
    let input = EinspeisemengeInput {
        einspeisemessung_kwh: Some(d("250")), // what the grid meter measured
        erzeugungsmessung_kwh: Some(d("400")), // inverter meter
        bezugsmessung_kwh: None,
        teilnehmer_kwh: vec![],
    };
    // Überschuss: billing basis = Einspeisemessung (250 kWh), NOT Erzeugungsmessung
    let kwh = compute_einspeisemenge(&input, Messkonzept::Ueberschusseinspeisung).unwrap();
    assert_eq!(kwh, d("250"));
}

/// Volleinspeisung: billing uses generation meter (all output), not feed-in meter.
#[test]
fn messkonzept_volleinspeisung_uses_generation_meter() {
    use eeg_billing::Messkonzept;
    use eeg_billing::metering::{EinspeisemengeInput, compute_einspeisemenge};

    // 400 kWh generated, all goes into grid (Volleinspeisung)
    let input = EinspeisemengeInput {
        einspeisemessung_kwh: Some(d("398")), // slight measurement difference from loss
        erzeugungsmessung_kwh: Some(d("400")), // generation meter = billing basis
        bezugsmessung_kwh: None,
        teilnehmer_kwh: vec![],
    };
    let kwh = compute_einspeisemenge(&input, Messkonzept::Volleinspeisung).unwrap();
    assert_eq!(kwh, d("400")); // generation meter wins
}

/// §42b GGV: generation covers all tenant consumption — excess goes to grid.
#[test]
fn messkonzept_ggv_full_coverage_surplus_to_grid() {
    use eeg_billing::metering::{EinspeisemengeInput, compute_tenant_allocation};

    // Building PV generates 1000 kWh; tenants consume 600 kWh total
    let input = EinspeisemengeInput {
        einspeisemessung_kwh: None,
        erzeugungsmessung_kwh: Some(d("1000")),
        bezugsmessung_kwh: None,
        teilnehmer_kwh: vec![d("250"), d("200"), d("150")], // 3 tenants, 600 kWh total
    };
    let (allocs, feed_in) = compute_tenant_allocation(&input).unwrap();
    assert_eq!(allocs.len(), 3);
    // Each tenant gets their full consumption (generation > consumption)
    assert_eq!(allocs[0].1, d("250"));
    assert_eq!(allocs[1].1, d("200"));
    assert_eq!(allocs[2].1, d("150"));
    // Grid feed-in = 1000 - 600 = 400 kWh
    assert_eq!(feed_in, d("400"));
}

/// §42b GGV: generation < total tenant consumption — proportional allocation.
#[test]
fn messkonzept_ggv_shortage_proportional_allocation() {
    use eeg_billing::metering::{EinspeisemengeInput, compute_tenant_allocation};

    // Building generates only 300 kWh for 500 kWh demand → 60% coverage
    let input = EinspeisemengeInput {
        einspeisemessung_kwh: None,
        erzeugungsmessung_kwh: Some(d("300")),
        bezugsmessung_kwh: None,
        teilnehmer_kwh: vec![d("300"), d("200")], // 500 kWh total demand
    };
    let (allocs, feed_in) = compute_tenant_allocation(&input).unwrap();
    // Tenant A: 300 × 300/500 = 180 kWh
    assert_eq!(allocs[0].1, d("180"));
    // Tenant B: 300 × 200/500 = 120 kWh
    assert_eq!(allocs[1].1, d("120"));
    assert_eq!(feed_in, d("0"));
}

/// Eigenverbrauch computed from generation and feed-in meters.
#[test]
fn eigenverbrauch_from_generation_minus_einspeisung() {
    use eeg_billing::metering::compute_eigenverbrauch;

    // 500 kWh generated, 320 kWh fed in → 180 kWh self-consumed
    let ev = compute_eigenverbrauch(Some(d("500")), Some(d("320")));
    assert_eq!(ev, Some(d("180")));
}

/// §14a Modul 2: HT/NT split for demand flexibility reporting.
#[test]
fn sect14a_ht_nt_split_measurement() {
    use eeg_billing::metering::Sect14aModul2Measurement;

    let m = Sect14aModul2Measurement {
        eigenverbrauch_ht_kwh: d("150"),
        eigenverbrauch_nt_kwh: d("350"),
        steuerungsmassnahme_kwh: Some(d("30")),
    };
    assert_eq!(m.total_kwh(), d("500"));
    // HT ratio: 150/500 = 0.30
    assert_eq!(m.ht_ratio(), Some(d("0.3000")));
}

// ══════════════════════════════════════════════════════════════════════════════
// §§52–54 Reduction pipeline
// ══════════════════════════════════════════════════════════════════════════════

/// §52 Abs. 6 netting: Pflichtzahlung deducted from Vergütung.
#[test]
fn sect52_abs6_netting_deducts_from_vergutung() {
    use eeg_billing::reductions::apply_sect52_netting;

    // Vergütung 42.55 EUR, Pflichtzahlung 10.00 EUR → operator receives 32.55 EUR
    let result = apply_sect52_netting(d("42.55"), d("10.00"));
    assert_eq!(result.net_vergütung_eur, d("32.55"));
    assert_eq!(result.residual_pflichtzahlung_eur, d("0"));
    assert!(result.netting_applied);
}

/// §52 Abs. 6: when penalty exceeds Vergütung — residual owed separately.
#[test]
fn sect52_abs6_netting_penalty_exceeds_vergutung() {
    use eeg_billing::reductions::apply_sect52_netting;

    // Vergütung 30 EUR < Pflichtzahlung 500 EUR (e.g. small plant, many violations)
    let result = apply_sect52_netting(d("30.00"), d("500.00"));
    assert_eq!(result.net_vergütung_eur, d("0"));
    assert_eq!(result.residual_pflichtzahlung_eur, d("470.00"));
}

/// §54 Ausschreibungsreduzierung: effective AW reduced by deduction, floored at 0.
#[test]
fn sect54_ausschreibungsreduzierung_reduces_aw() {
    use eeg_billing::reductions::Sect54Reduction;
    use time::macros::date;

    let r = Sect54Reduction {
        deduction_ct_kwh: d("0.5"),
        bnetza_notification_ref: "BNetzA-54-2026-WIND-001".into(),
        effective_from: date!(2026 - 01 - 01),
    };
    // Awarded AW = 6.28 ct → effective = 5.78 ct
    assert_eq!(r.effective_aw(d("6.28")), d("5.78"));
    // AW < deduction → floor at 0
    assert_eq!(r.effective_aw(d("0.3")), d("0"));
}

/// §53b + §52 Abs. 6 combined: both reductions applied in correct order.
#[test]
fn sect53b_and_sect52_netting_combined_pipeline() {
    use eeg_billing::reductions::ReductionPipeline;

    let pipeline = ReductionPipeline {
        pflichtzahlung_eur: Some(d("10.00")),
        apply_sect52_netting: true,
        sect53b_ct_kwh: Some(d("0.5")), // 0.5 ct/kWh regional reduction
        sect53c: None,
        sect54: None,
    };
    // 1000 kWh × 0.5 ct/kWh / 100 = 5.00 EUR §53b
    // gross = 42.55 → after §53b = 37.55 → after §52 netting = 27.55
    let result = pipeline.apply_with_kwh(d("42.55"), d("1000"));
    assert_eq!(result.net_vergütung_eur, d("27.55"));
    assert_eq!(result.residual_pflichtzahlung_eur, d("0"));
    assert!(result.total_reductions_eur > d("0"));
}

// ══════════════════════════════════════════════════════════════════════════════
// Settlement state machine
// ══════════════════════════════════════════════════════════════════════════════

/// Healthy plant: MaStR registered + Fernsteuerbarkeit installed → Active.
#[test]
fn settlement_state_healthy_plant_is_active() {
    use eeg_billing::settlement_state::{SettlementPeriodState, derive_settlement_state};
    use time::macros::date;

    let state = derive_settlement_state(
        true,                        // mastr_registriert
        Some(date!(2024 - 01 - 01)), // fernsteuerbarkeit installed
        d("50"),                     // leistung_kwp
        Some(date!(2040 - 12 - 31)), // foerderendedatum
        date!(2026 - 07 - 01),       // billing_date
        2023,
    );
    assert_eq!(state, SettlementPeriodState::Active);
}

/// EEG 2023, MaStR missing → Reduced (Pflichtzahlung, Vergütung still flows).
#[test]
fn settlement_state_eeg2023_mastr_missing_reduced() {
    use eeg_billing::settlement_state::{SettlementPeriodState, derive_settlement_state};
    use time::macros::date;

    let state = derive_settlement_state(
        false,
        None,
        d("50"),
        Some(date!(2040 - 12 - 31)),
        date!(2026 - 07 - 01),
        2023,
    );
    assert_eq!(state, SettlementPeriodState::Reduced);
}

/// EEG 2017, MaStR missing → Suspended (VerguetungAufNull, old regime).
#[test]
fn settlement_state_eeg2017_mastr_missing_suspended() {
    use eeg_billing::settlement_state::{SettlementPeriodState, derive_settlement_state};
    use time::macros::date;

    let state = derive_settlement_state(
        false,
        None,
        d("50"),
        Some(date!(2040 - 12 - 31)),
        date!(2026 - 07 - 01),
        2017,
    );
    assert_eq!(state, SettlementPeriodState::Suspended);
}

/// Förderdauer expired → PostEeg state.
#[test]
fn settlement_state_foerderdauer_expired_post_eeg() {
    use eeg_billing::settlement_state::{SettlementPeriodState, derive_settlement_state};
    use time::macros::date;

    let state = derive_settlement_state(
        true,
        Some(date!(2020 - 01 - 01)),
        d("10"),
        Some(date!(2024 - 12 - 31)), // expired
        date!(2025 - 01 - 01),       // billing after expiry
        2023,
    );
    assert_eq!(state, SettlementPeriodState::PostEeg);
}

/// State is_payable and is_terminal semantics.
#[test]
fn settlement_state_payable_and_terminal_semantics() {
    use eeg_billing::settlement_state::SettlementPeriodState;

    assert!(SettlementPeriodState::Active.is_payable());
    assert!(SettlementPeriodState::Reduced.is_payable());
    assert!(SettlementPeriodState::PostEeg.is_payable());
    assert!(!SettlementPeriodState::Suspended.is_payable());
    assert!(!SettlementPeriodState::Ended.is_payable());
    assert!(SettlementPeriodState::Ended.is_terminal());
    assert!(!SettlementPeriodState::Active.is_terminal());
}

// ══════════════════════════════════════════════════════════════════════════════
// §20 Abs. 3 EEG 2023 — Managementprämie boundary cases
// ══════════════════════════════════════════════════════════════════════════════

/// §20 Abs. 3 — Partial Managementprämie when EPEX is between AW and AW+Mgmt.
///
/// When AW ≤ EPEX < AW + Managementprämie, the plant receives only the residual
/// management component (not the full 0.4 ct). This is the key difference from
/// the old EEG ≤2012 model.
#[test]
fn s20_partial_managementpraemie_when_epex_between_aw_and_eff_aw() {
    // AW = 5.0, Managementprämie = 0.4, EPEX = 5.2
    // eff_AW = 5.4; EPEX (5.2) is between AW (5.0) and eff_AW (5.4)
    // pure_praemie = max(0, 5.0 - 5.2) = 0
    // effective_mgmt = max(0, 5.4 - 5.2) = 0.2 ct (partial)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("100000")),
        epex_avg_ct_kwh: Some(d("5.2")),
        direktverm_aw_ct: Some(d("5.0")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Partial mgmt: 0.2 ct × 100,000 / 100 = 200 EUR
    assert_eq!(out.settlement_eur, Some(d("200.00")));
    // Only management component position (pure praemie = 0)
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].legal_basis, "§20 Abs. 3 EEG 2023");
    assert_eq!(out.positions[0].rate_ct_kwh, d("0.2")); // partial, not full 0.4
}

/// §20 Abs. 3 — Correct total when positive spread: both positions present.
#[test]
fn s20_full_praemie_plus_full_mgmt_when_epex_below_aw() {
    // AW = 6.2, Mgmt = 0.4, EPEX = 4.8
    // pure_praemie = 6.2 - 4.8 = 1.4 ct; effective_mgmt = 0.4 ct (full)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        einspeisemenge_kwh: Some(d("100000")),
        epex_avg_ct_kwh: Some(d("4.8")),
        direktverm_aw_ct: Some(d("6.2")),
        managementpraemie_ct: Some(d("0.4")),
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(d("1800.00")));
    // Two positions: Marktprämie 1.4 ct + Managementprämie 0.4 ct
    assert_eq!(out.positions.len(), 2);
    assert_eq!(out.positions[0].rate_ct_kwh, d("1.4")); // pure praemie
    assert_eq!(out.positions[1].rate_ct_kwh, d("0.4")); // full mgmt
}

// ══════════════════════════════════════════════════════════════════════════════
// §§22a, 28 EEG 2023 — Ausschreibung Förderdauer expired
// ══════════════════════════════════════════════════════════════════════════════

/// Ausschreibung award expired: FoerderungBeendet detected automatically.
#[test]
fn ausschreibung_foerderdauer_expired_post_eeg() {
    use time::macros::date;

    // BNetzA tender plant, award expires end of 2025
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium,
        tariff_source: TariffSource::Auction(eeg_billing::AusschreibungMetadata::default()),
        einspeisemenge_kwh: Some(d("500000")),
        epex_avg_ct_kwh: Some(d("4.5")),
        direktverm_aw_ct: Some(d("5.80")),
        foerderendedatum: Some(date!(2025 - 12 - 31)),
        billing_date: Some(date!(2026 - 01 - 01)), // billing AFTER award expiry
        ..SettleInput::default()
    });
    // Förderdauer expired → FoerderungBeendet (not PriceMissing or Calculated)
    assert_eq!(out.status, SettlementStatus::FoerderungBeendet);
    assert_eq!(out.settlement_eur, Some(d("0")));
}

// ══════════════════════════════════════════════════════════════════════════════
// SettlementType — Correction and Reversal
// ══════════════════════════════════════════════════════════════════════════════

/// Correction settlement: carries the original_id and reason in settlement_type.
#[test]
fn settlement_type_correction_carries_metadata() {
    use eeg_billing::scheme::{CorrectionReason, SettlementType};

    let correction_type = SettlementType::Correction {
        original_id: "ORIG-2026-06-001".to_string(),
        reason: CorrectionReason::MeterDataCorrected,
    };

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        settlement_type: correction_type.clone(),
        einspeisemenge_kwh: Some(d("520")), // revised: 20 kWh more than original
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    });
    // Settlement is calculated normally — the type metadata is for caller use
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 520 × 8.11 / 100 = 42.172 EUR (corrected amount)
    assert_eq!(out.settlement_eur, Some(d("42.172")));
}

/// Reversal settlement: same formula but settlement_type = Reversal.
#[test]
fn settlement_type_reversal_carries_original_id() {
    use eeg_billing::scheme::SettlementType;

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        settlement_type: SettlementType::Reversal {
            original_id: "ORIG-2026-06-001".to_string(),
        },
        einspeisemenge_kwh: Some(d("-500")), // negative kWh for reversal
        verguetungssatz_ct: d("8.11"),
        ..SettleInput::default()
    });
    // Reversal: negative amount (refund of original settlement)
    assert_eq!(out.status, SettlementStatus::Calculated);
    let eur = out.settlement_eur.unwrap();
    assert!(eur < d("0"), "reversal should produce negative settlement");
}

// ══════════════════════════════════════════════════════════════════════════════
// Post-EEG — configurable negative price floor
// ══════════════════════════════════════════════════════════════════════════════

/// Post-EEG with zero floor: plant not exposed to negative EPEX (contract protection).
#[test]
fn post_eeg_negative_epex_with_zero_floor() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::PostEeg,
        einspeisemenge_kwh: Some(d("1000")),
        epex_avg_ct_kwh: Some(d("-2.0")),   // negative EPEX
        post_eeg_price_floor: Some(d("0")), // contract floor: 0 ct/kWh
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Floor applied: 0 ct × 1000 kWh / 100 = 0 EUR (not negative)
    assert_eq!(out.settlement_eur, Some(d("0")));
}

/// Post-EEG without floor (default): plant pays for negative EPEX.
#[test]
fn post_eeg_negative_epex_no_floor_plant_pays() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::PostEeg,
        einspeisemenge_kwh: Some(d("1000")),
        epex_avg_ct_kwh: Some(d("-2.0")),
        post_eeg_price_floor: None, // no floor = full market exposure
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // -2.0 ct × 1000 kWh / 100 = -20 EUR (plant owes NB)
    assert_eq!(out.settlement_eur, Some(d("-20.00")));
}

// ══════════════════════════════════════════════════════════════════════════════
// Repowering — scope distinctions
// ══════════════════════════════════════════════════════════════════════════════

/// Full repowering resets the Förderdauer; partial repowering does not.
#[test]
fn repowering_scope_foerderdauer_reset_semantics() {
    use eeg_billing::RepoweringScope;

    // Full replacement: Förderdauer resets
    assert!(RepoweringScope::Full.resets_foerderdauer_definitely());
    assert!(RepoweringScope::FullWithCapacityIncrease.resets_foerderdauer_definitely());

    // Partial: Förderdauer does NOT reset (original date governs)
    assert!(!RepoweringScope::RotorOnly.resets_foerderdauer_definitely());
    assert!(!RepoweringScope::NacelleAndRotor.resets_foerderdauer_definitely());
    assert!(!RepoweringScope::TurbineUnit.resets_foerderdauer_definitely());
}

/// Partial repowering (rotor only): original Förderdauer continues.
#[test]
fn partial_repowering_keeps_original_foerderdauer() {
    use time::macros::date;

    // Wind turbine commissioned 2010 — Förderdauer until 2030-12-31
    let original_end = foerderendedatum_eeg(date!(2010 - 06 - 01)).unwrap();
    assert_eq!(original_end, date!(2030 - 12 - 31));

    // Rotor replacement in 2025: Förderdauer does NOT reset
    // The original 2030-12-31 end date continues
    use eeg_billing::RepoweringScope;
    let scope = RepoweringScope::RotorOnly;
    assert!(
        !scope.resets_foerderdauer_definitely(),
        "Rotor-only repowering keeps original Förderdauer"
    );

    // Full repowering in 2025 WOULD reset to 2045
    let full_reset_end = foerderendedatum_repowering(date!(2025 - 06 - 01)).unwrap();
    assert_eq!(full_reset_end, date!(2045 - 12 - 31));
    assert!(
        full_reset_end > original_end,
        "full repowering extends beyond original"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// §52 Abs. 6 — Netting in settlement context
// ══════════════════════════════════════════════════════════════════════════════

/// §52 Abs. 6 full pipeline: calculate settlement + apply netting = net disbursement.
#[test]
fn sect52_abs6_full_netting_pipeline() {
    use eeg_billing::reductions::ReductionPipeline;
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    // 1. Calculate settlement (Vergütung independent of §52)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("500")),
        verguetungssatz_ct: d("8.11"),
        pflichtverstoss: vec![Pflichtverstoss {
            typ: SanktionsTyp::MastrNichtRegistriert,
            leistung_kw: d("10"),
            monate_des_verstosses: 1,
            nachtraeglich_erfuellt: false,
        }],
        ..SettleInput::default()
    });

    let gross = out.settlement_eur.unwrap(); // 40.55 EUR Vergütung
    let penalty = out.pflichtzahlung_eur.unwrap(); // 100 EUR Pflichtzahlung

    // 2. Apply §52 Abs. 6 netting (NB deducts penalty from Vergütung disbursement)
    let pipeline = ReductionPipeline {
        pflichtzahlung_eur: Some(penalty),
        apply_sect52_netting: true,
        ..ReductionPipeline::none()
    };
    let result = pipeline.apply(gross);

    // Penalty (100) > Vergütung (40.55): net disbursement = 0, residual = 59.45 EUR
    assert_eq!(result.net_vergütung_eur, d("0"));
    assert_eq!(result.residual_pflichtzahlung_eur, d("59.45"));
    assert!(
        result.total_reductions_eur > d("0"),
        "netting applied: reductions > 0"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// EEG ≤2009 grandfathering — no §51, no Direktvermarktungspflicht
// ══════════════════════════════════════════════════════════════════════════════

/// EEG 2000 / 2004 plant: no §51 Negativpreisregel, stays on Einspeisevergütung forever.
#[test]
fn eeg2000_grandfathering_no_negativpreis_no_direktverm() {
    use eeg_billing::direktverm::is_direktvermarktung_mandatory;
    use eeg_billing::{EegGesetz, ErzeugungsArt};

    // EEG 2000 plant: §51 does not apply (Bestandsschutz §66 EEG 2017)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff,
        einspeisemenge_kwh: Some(d("10000")),
        verguetungssatz_ct: d("50.62"), // EEG 2000 solar rate ≤30 kWp
        kwh_during_negative_epex: Some(d("2000")), // 2000 kWh during negative EPEX
        eeg_gesetz: EegGesetz::Eeg2000,
        leistung_kwp: Some(d("5000")), // 5 MW solar: would trigger §51 in EEG 2023
        erzeugungsart: Some(ErzeugungsArt::SolarFreiflaeche),
        ..SettleInput::default()
    });
    // No §51 for EEG 2000 (Bestandsschutz)
    assert_eq!(out.eligible_kwh, Some(d("10000")));
    assert_eq!(out.settlement_eur, Some(d("5062.00")));

    // EEG 2000 plants are also exempt from mandatory Direktvermarktung
    assert!(!is_direktvermarktung_mandatory(
        d("500"),
        EegGesetz::Eeg2000
    ));
    assert!(!is_direktvermarktung_mandatory(
        d("500"),
        EegGesetz::Eeg2004
    ));
}
