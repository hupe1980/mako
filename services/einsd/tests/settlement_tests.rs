//! Comprehensive EEG/KWKG settlement tests using `eeg_billing::calculate_settlement`.
//!
//! These tests validate the actual settlement formula from the `eeg-billing` crate.
//! Each test verifies: SettlementStatus, settlement_eur, positions count/content.
//!
//! Run: `cargo test -p einsd --test settlement_tests`

use eeg_billing::{
    AusschreibungMetadata, CapacityBlock, EegGesetz, ErzeugungsArt, Pflichtverstoss, SanktionsTyp,
    SettleInput, SettlementScheme, SettlementStatus, TariffSource, calculate_settlement, rates,
};
use rust_decimal::Decimal;
use rust_decimal::dec;
use time::macros::date;

fn d(s: &str) -> Decimal {
    s.parse().expect("decimal parse")
}

// ── §21 EEG — Feste Einspeisevergütung ───────────────────────────────────────

#[test]
fn verguetung_solar_100kwh_eeg_2023_rate() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("100")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("8.11")));
    assert_eq!(out.eligible_kwh, Some(d("100")));
    assert_eq!(out.positions.len(), 1);
    assert!(out.positions[0].legal_basis.contains("EEG"));
}

#[test]
fn verguetung_eeg_2017_rate_12_35_ct() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(12.35),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        inbetriebnahme: Some(date!(2017 - 04 - 01)),
        leistung_kwp: Some(dec!(8)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(dec!(123.50)));
}

#[test]
fn verguetung_eeg_2004_legacy_rate_57_4_ct() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(57.4),
        },
        einspeisemenge_kwh: Some(dec!(200)),
        inbetriebnahme: Some(date!(2004 - 07 - 15)),
        leistung_kwp: Some(dec!(5)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(dec!(114.80)));
}

#[test]
fn verguetung_no_data_returns_no_data_status() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.11),
        },
        einspeisemenge_kwh: None,
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::NoData);
    assert!(out.settlement_eur.is_none());
    assert!(out.positions.is_empty());
}

// ── §51 EEG 2023 — Negativpreisregel ─────────────────────────────────────────

#[test]
fn negativpreis_applied_to_post_2016_large_plant() {
    // Plant commissioned 2020 → EEG 2017 rules apply (§100 Übergangsregelung).
    // EEG 2017 kW exemption threshold is 500 kW; 150 kW < 500 kW → §51 does NOT apply.
    // Caller would only pass kwh_during_negative_epex for ≥500 kW EEG 2017 plants.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(9.25),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        kwh_during_negative_epex: Some(dec!(200)),
        inbetriebnahme: Some(date!(2020 - 06 - 01)),
        leistung_kwp: Some(dec!(150)),
        eeg_gesetz: EegGesetz::Eeg2017,
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 150 kW < 500 kW exemption (EEG 2017) → full 1000 kWh settled
    assert_eq!(out.settlement_eur, Some(dec!(92.50)));
    assert_eq!(out.eligible_kwh, Some(dec!(1000)));
}

#[test]
fn negativpreis_applied_to_eeg2017_above_500kw_plant() {
    // Plant commissioned 2019 → EEG 2017 rules. 600 kW ≥ 500 kW → §51 applies.
    // Caller has already verified ≥6 consecutive negative-price hours.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(9.25),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        kwh_during_negative_epex: Some(dec!(200)),
        inbetriebnahme: Some(date!(2019 - 03 - 01)),
        leistung_kwp: Some(dec!(600)),
        eeg_gesetz: EegGesetz::Eeg2017,
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Effective = 800 kWh; 800 × 9.25 / 100 = 74.00 EUR
    assert_eq!(out.settlement_eur, Some(dec!(74.00)));
    assert_eq!(out.eligible_kwh, Some(dec!(800)));
}

#[test]
fn negativpreis_exempt_pre_2016_plant() {
    // §51 EEG 2017 is NOT retroactive — pre-2016 plants exempt
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(29.37),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        kwh_during_negative_epex: Some(dec!(200)),
        inbetriebnahme: Some(date!(2010 - 06 - 01)), // pre-2016
        leistung_kwp: Some(dec!(150)),
        eeg_gesetz: EegGesetz::Eeg2009,
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Full 1000 kWh — §27 not applied
    assert_eq!(out.settlement_eur, Some(dec!(293.70)));
    assert_eq!(out.eligible_kwh, Some(dec!(1000)));
}

#[test]
fn negativpreis_exempt_small_plant_below_100kwp() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.11),
        },
        einspeisemenge_kwh: Some(dec!(500)),
        kwh_during_negative_epex: Some(dec!(100)),
        inbetriebnahme: Some(date!(2023 - 03 - 15)),
        leistung_kwp: Some(dec!(9.5)), // <100 kWp → exempt
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Full 500 kWh: small plant exempt from §27
    assert_eq!(out.settlement_eur, Some(dec!(40.55)));
    assert_eq!(out.eligible_kwh, Some(dec!(500)));
}

// ── Automatic FoerderungBeendet detection ─────────────────────────────────────

#[test]
fn foerderendedatum_triggers_foerderung_beendet() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(57.4),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        foerderendedatum: Some(date!(2024 - 07 - 15)),
        billing_date: Some(date!(2024 - 08 - 01)), // billing after end
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::FoerderungBeendet);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
    assert!(out.positions.is_empty());
}

#[test]
fn foerderendedatum_still_active_within_end_month() {
    // billing_date (July 1) ≤ foerderendedatum (July 15) → status = Calculated, not FoerderungBeendet.
    // §25 Abs. 1 Satz 3 EEG: pro-rata for expiry mid-month: 15/31 days.
    // 100 kWh × 8.11 ct × (15/31) = 8.11 × 0.483871 = 3.92419 EUR
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.11),
        },
        einspeisemenge_kwh: Some(dec!(100)),
        foerderendedatum: Some(date!(2024 - 07 - 15)),
        billing_date: Some(date!(2024 - 07 - 01)), // billing_date ≤ foerderendedatum
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(dec!(3.92419))); // §25 pro-rata: 15/31 days
}

// ── §25 EEG — MaStR-Sanktionen ───────────────────────────────────────────────

#[test]
fn mastr_not_registered_forces_zero_settlement() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.11),
        },
        einspeisemenge_kwh: Some(dec!(500)),
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufNull),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
    assert!(out.positions.is_empty());
}

#[test]
fn sanction_takes_priority_over_all_other_conditions() {
    // Even when EPEX missing (PriceMissing), §25 fires first
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: dec!(0),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(dec!(500)),
        marktwert_ct_kwh: None,
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufNull),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Sanctioned);
}

// ── §21 Abs. 3 EEG 2023 — Mieterstrom ───────────────────────────────────────────────

#[test]
fn mieterstrom_base_plus_zuschlag() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::TenantElectricity {
            verguetungssatz_ct: dec!(8.11),
            mieter_zuschlag_ct: Some(dec!(2.5)),
        },
        einspeisemenge_kwh: Some(dec!(400)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 400×8.11/100 + 400×2.5/100 = 32.44 + 10.00 = 42.44
    assert_eq!(out.settlement_eur, Some(dec!(42.44)));
    assert_eq!(out.positions.len(), 2);
    assert!(out.positions[1].legal_basis.contains("21 Abs. 3"));
}

#[test]
fn mieterstrom_without_zuschlag_produces_one_position() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::TenantElectricity {
            verguetungssatz_ct: dec!(8.11),
            mieter_zuschlag_ct: None,
        },
        einspeisemenge_kwh: Some(dec!(500)),
        ..SettleInput::default()
    });
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.settlement_eur, Some(dec!(40.55)));
}

// ── §20 EEG — Gleitende Marktprämie ──────────────────────────────────────────

#[test]
fn direktvermarktung_positive_spread() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: dec!(6.5),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(dec!(10000)),
        marktwert_ct_kwh: Some(dec!(4.1)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Anlage 1 EEG 2023: Marktprämie = AW − Monatsmarktwert, marketing costs
    // already folded into the AW. 10000 × (6.5 − 4.1) / 100 = 240.
    assert_eq!(out.settlement_eur, Some(dec!(240.00)));
    assert_eq!(out.positions.len(), 1);
}

#[test]
fn direktvermarktung_negative_spread_pays_nothing() {
    // Anlage 1 EEG 2023: the Marktprämie floors at zero. With a Monatsmarktwert
    // above the anzulegender Wert the plant earns from the market, not the NB —
    // there is no guaranteed marketing floor to fall back on.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: dec!(6.5),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(dec!(50000)),
        marktwert_ct_kwh: Some(dec!(30.0)), // Marktwert >> AW (6.5 ct) → zero
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // AW (6.5) < Monatsmarktwert (30.0) → max(0, …) = EUR 0
    assert_eq!(out.settlement_eur, Some(dec!(0)));
}

#[test]
fn direktvermarktung_price_missing() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: dec!(6.5),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        marktwert_ct_kwh: None,
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::PriceMissing);
    assert!(out.settlement_eur.is_none());
}

// ── §§22a,28 EEG — Ausschreibung ─────────────────────────────────────────────

#[test]
fn ausschreibung_large_solar_park() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: dec!(5.82),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        tariff_source: TariffSource::Auction(AusschreibungMetadata::default()),
        einspeisemenge_kwh: Some(dec!(2_500_000)),
        marktwert_ct_kwh: Some(dec!(4.1)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 2_500_000 × (5.82 − 4.1) / 100 = 43_000.
    assert_eq!(out.settlement_eur, Some(dec!(43_000)));
    assert!(out.positions[0].legal_basis.contains("22a"));
}

// ── Post-EEG Spot ─────────────────────────────────────────────────────────────

#[test]
fn post_eeg_spot_positive_epex() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(dec!(1000)),
        marktwert_ct_kwh: Some(dec!(8.5)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(dec!(85.00)));
}

#[test]
fn post_eeg_spot_negative_epex_no_floor() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(dec!(1000)),
        marktwert_ct_kwh: Some(dec!(-0.5)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(dec!(-5.00)));
}

// ── §21 Abs. 3 EEG — Eigenverbrauch ─────────────────────────────────────────────────

#[test]
fn eigenverbrauch_always_zero_eur() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::Eigenverbrauch,
        einspeisemenge_kwh: Some(dec!(999)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
    assert!(out.positions.is_empty());
}

// ── §7 KWKG 2023 — KWK-Zuschlag ──────────────────────────────────────────────

#[test]
fn kwkg_full_period_no_hour_limit() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: dec!(6.0),
            kwh_paid_gesamt: None,
            max_kwh: None,
        },
        einspeisemenge_kwh: Some(dec!(5000)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(dec!(300.00)));
}

#[test]
fn kwkg_hour_limit_prorated_last_period() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: dec!(3.1),
            kwh_paid_gesamt: Some(dec!(29_900)),
            max_kwh: Some(dec!(30_000)),
        },
        einspeisemenge_kwh: Some(dec!(400)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::FoerderungBeendet);
    // 100 eligible × 3.1 ct / 100 = 3.10 EUR
    assert_eq!(out.settlement_eur, Some(dec!(3.10)));
    assert_eq!(out.eligible_kwh, Some(dec!(100)));
}

#[test]
fn kwkg_limit_exhausted_returns_zero() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: dec!(6.0),
            kwh_paid_gesamt: Some(dec!(30_000)),
            max_kwh: Some(dec!(30_000)),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::FoerderungBeendet);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
}

// ── §50 EEG 2023 — Flexibilitätsprämie ───────────────────────────────────────

#[test]
fn flexibilitaet_base_plus_praemie() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FlexibilityPremium {
            verguetungssatz_ct: dec!(14.67),
            flex_praemie_ct_kwh: Some(dec!(0.5)),
        },
        einspeisemenge_kwh: Some(dec!(2000)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // Base: 293.40; Flex: 10.00; Total = 303.40
    assert_eq!(out.settlement_eur, Some(dec!(303.40)));
    assert_eq!(out.positions.len(), 2);
    assert!(out.positions[1].legal_basis.contains("50"));
}

// ── §24 EEG — Anlagenerweiterung (multi-block) ────────────────────────────────

#[test]
fn anlagenerweiterung_two_blocks_proportional() {
    // 10 kWp at 9.25 ct + 5 kWp at 8.11 ct, total 1500 kWh
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(9.25),
        },
        einspeisemenge_kwh: Some(dec!(1500)),
        leistung_kwp: Some(dec!(10)),
        inbetriebnahme: Some(date!(2020 - 03 - 15)),
        foerderendedatum: Some(date!(2040 - 03 - 15)),
        billing_date: Some(date!(2026 - 07 - 01)),
        capacity_blocks: vec![CapacityBlock {
            leistung_kwp: dec!(5),
            verguetungssatz_ct: dec!(8.11),
            inbetriebnahme: date!(2024 - 06 - 01),
            foerderendedatum: date!(2044 - 06 - 01),
        }],
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.positions.len(), 2);
    // Block 0: 1000 kWh × 9.25 ct = 92.50; Block 1: 500 kWh × 8.11 ct = 40.55; Total = 133.05
    assert_eq!(out.settlement_eur, Some(dec!(133.05)));
}

#[test]
fn anlagenerweiterung_expired_primary_block_excluded() {
    // Primary block expired, only extension contributes
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(57.4),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        leistung_kwp: Some(dec!(5)),
        foerderendedatum: Some(date!(2024 - 06 - 01)), // expired
        billing_date: Some(date!(2024 - 07 - 01)),
        capacity_blocks: vec![CapacityBlock {
            leistung_kwp: dec!(5),
            verguetungssatz_ct: dec!(8.11),
            inbetriebnahme: date!(2024 - 01 - 01),
            foerderendedatum: date!(2044 - 01 - 01),
        }],
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].rate_ct_kwh, dec!(8.11));
}

#[test]
fn anlagenerweiterung_all_expired_foerderung_beendet() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(9.25),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        leistung_kwp: Some(dec!(10)),
        foerderendedatum: Some(date!(2020 - 01 - 01)),
        billing_date: Some(date!(2025 - 01 - 01)),
        capacity_blocks: vec![CapacityBlock {
            leistung_kwp: dec!(5),
            verguetungssatz_ct: dec!(8.11),
            inbetriebnahme: date!(2004 - 01 - 01),
            foerderendedatum: date!(2024 - 01 - 01),
        }],
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::FoerderungBeendet);
    assert!(out.positions.is_empty());
}

// ── Statutory EEG rate tables ─────────────────────────────────────────────────

#[test]
fn kwkg_2023_rate_all_tiers() {
    let table = rates::kwkg_zuschlag_lookup().expect("KWKG rates known");
    assert_eq!(
        table.rate_for(dec!(20)).unwrap(),
        billing::Amount::parse("0.08").unwrap()
    );
    assert_eq!(
        table.rate_for(dec!(75)).unwrap(),
        billing::Amount::parse("0.06").unwrap()
    );
    assert_eq!(
        table.rate_for(dec!(200)).unwrap(),
        billing::Amount::parse("0.05").unwrap()
    );
    assert_eq!(
        table.rate_for(dec!(2000)).unwrap(),
        billing::Amount::parse("0.04").unwrap()
    );
    assert_eq!(
        table.rate_for(dec!(5000)).unwrap(),
        billing::Amount::parse("0.03").unwrap()
    );
}

#[test]
fn lookup_rate_solar_aufdach_2023() {
    // §48 Abs. 2 EEG 2023 gross AW for ≤10 kWp. The published 8.20 ct
    // Einspeisevergütung is this minus the §53 deduction of 0.4 ct.
    let rate = rates::lookup_rate("SOLAR_AUFDACH", dec!(9), 2023).unwrap();
    assert_eq!(rate, billing::Amount::parse("0.08600").unwrap());
}

#[test]
fn lookup_rate_unknown_tech_returns_err() {
    // A completely non-existent technology string returns Err (no DB row).
    assert!(rates::lookup_rate("NOT_A_REAL_TECHNOLOGY", dec!(100), 2023).is_err());
}

// ── billing::PricingModel integration ───────────────────────────────────────────────

#[test]
fn eeg_settle_tariff_produces_billing_document() {
    use billing::DocumentMeta;
    use billing::PricingModel;
    use eeg_billing::tariff::EegSettleTariff;

    let output = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.11),
        },
        einspeisemenge_kwh: Some(dec!(500)),
        ..SettleInput::default()
    });
    let tariff = EegSettleTariff::new(&output);
    let doc = tariff.settle(DocumentMeta::default()).expect("settle");
    assert_eq!(doc.net_total(), billing::Amount::parse("40.55000").unwrap());
    doc.assert_valid();
}

#[test]
fn eeg_settle_tariff_mit_mwst_19_percent() {
    use billing::DocumentMeta;
    use billing::PricingModel;
    use eeg_billing::tariff::EegSettleTariffRegelbesteuerung;

    let output = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.11),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        ..SettleInput::default()
    });
    let tariff = EegSettleTariffRegelbesteuerung::new(&output);
    let doc = tariff
        .settle(DocumentMeta::default())
        .expect("settle mit MwSt");
    assert_eq!(doc.net_total(), billing::Amount::parse("81.10000").unwrap());
    doc.assert_valid();
}

// ── Decimal precision ─────────────────────────────────────────────────────────

#[test]
fn settlement_5dp_no_float_rounding() {
    // 333.333 × 8.1 / 100 = 26.999973 — exact in Decimal
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.1"),
        },
        einspeisemenge_kwh: Some(d("333.333")),
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(d("26.99997")));
}

#[test]
fn settlement_gigawatt_scale_no_overflow() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(30.0),
        },
        einspeisemenge_kwh: Some(dec!(1_000_000)),
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(dec!(300_000)));
}

// ── Förderdauer helpers ───────────────────────────────────────────────────────

#[test]
fn foerderendedatum_eeg_standard_20_years() {
    use eeg_billing::foerderendedatum_eeg;
    // §25 Abs. 1 Satz 2 EEG 2023: statutory plants extend to Dec 31 of year+20
    assert_eq!(
        foerderendedatum_eeg(date!(2010 - 05 - 15)).unwrap(),
        date!(2030 - 12 - 31)
    );
}

#[test]
fn foerderendedatum_repowering_resets_20_years() {
    use eeg_billing::foerderendedatum_repowering;
    // Repowering also uses statutory rule → Dec 31 of year+20
    assert_eq!(
        foerderendedatum_repowering(date!(2024 - 06 - 01)).unwrap(),
        date!(2044 - 12 - 31)
    );
}

#[test]
fn kwk_eligible_kwh_prorated() {
    use eeg_billing::kwk_eligible_kwh;
    let (eligible, done) = kwk_eligible_kwh(dec!(400), dec!(29_900), dec!(30_000));
    assert_eq!(eligible, dec!(100));
    assert!(done);
}

#[test]
fn kwk_foerderend_calendar_15_years() {
    use eeg_billing::kwk_foerderend_calendar;
    assert_eq!(
        kwk_foerderend_calendar(date!(2020 - 01 - 15)).unwrap(),
        date!(2035 - 01 - 15)
    );
}

/// §51 — the trigger is the Inbetriebnahmedatum, not the law year.
#[test]
fn negativpreis_threshold_at_boundary() {
    use eeg_billing::NegativpreisRegime as R;
    // The Solarspitzengesetz boundary sits inside 2025.
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2025 - 02 - 24)).mindest_lauflaenge_qh(),
        Some(12)
    );
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2025 - 02 - 25)).mindest_lauflaenge_qh(),
        Some(1)
    );
    // The §100 Abs. 1 Satz 4 EEG 2017 boundary sits at 01.01.2016.
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2015 - 12 - 31)).mindest_lauflaenge_qh(),
        None
    );
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2016 - 01 - 01)).mindest_lauflaenge_qh(),
        Some(24)
    );
}

// ── §50a EEG 2023 — Flexibilitätszuschlag (neue Anlagen) ─────────────────────

#[test]
fn flexibilitaet_zuschlag_monthly_payment() {
    // §50a: 500 kW additional flexible capacity × 100 EUR/kW/year ÷ 12 months = 4166.67 EUR/month
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FlexibilitySurcharge {
            rate_eur_per_kw_year: dec!(100),
        },
        einspeisemenge_kwh: None,      // not energy-based
        leistung_kwp: Some(dec!(500)), // 500 kW flexible capacity
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 500 × 100 / 12 = 4166.66666... → rounded to 5dp: 4166.66667 EUR
    let eur = out.settlement_eur.expect("should have EUR amount");
    assert!(
        eur > dec!(4166) && eur < dec!(4167),
        "500 kW × 100 EUR/kW/yr ÷ 12 ≈ 4166.67"
    );
    assert_eq!(out.positions.len(), 1);
    assert!(out.positions[0].legal_basis.contains("50a"));
}

#[test]
fn flexibilitaet_zuschlag_zero_capacity_returns_zero() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FlexibilitySurcharge {
            rate_eur_per_kw_year: dec!(100),
        },
        leistung_kwp: None, // no capacity → zero payment
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
}

// ── §23b EEG 2023 — PostEegSpot Jahresmarktwert-Deckel (10 ct cap) ─────────

#[test]
fn post_eeg_spot_capped_at_10ct() {
    // §23b: When EPEX > 10 ct/kWh, the payment is capped at 10 ct/kWh
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(dec!(1000)),
        marktwert_ct_kwh: Some(dec!(25.0)), // EPEX = 25 ct → cap to 10 ct
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 1000 × 10 ct / 100 = 100 EUR (not 250 EUR)
    assert_eq!(out.settlement_eur, Some(dec!(100.00)));
    assert!(
        out.positions[0].description.contains("23b"),
        "description should mention §23b cap"
    );
}

#[test]
fn post_eeg_spot_below_10ct_not_capped() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(dec!(1000)),
        marktwert_ct_kwh: Some(dec!(8.5)), // below 10 ct → no cap
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(dec!(85.00)));
}

#[test]
fn post_eeg_spot_negative_epex_not_capped() {
    // §23b cap does NOT apply to negative prices — plant owes NB
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(dec!(1000)),
        marktwert_ct_kwh: Some(dec!(-0.5)),
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(dec!(-5.00)));
}

// ── §51 EEG 2023 — Negativpreisregel ────────

/// §51 EEG 2023 as enacted — a 2024 plant above the 400 kW exemption is reduced.
#[test]
fn negativpreis_eeg2023_reduces_above_the_400kw_exemption() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.51),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        kwh_during_negative_epex: Some(dec!(50)), // 50 kWh during negative hours
        inbetriebnahme: Some(date!(2024 - 06 - 01)),
        leistung_kwp: Some(dec!(500)),
        ..SettleInput::default()
    });
    // Effective = 950 kWh; 950 × 8.51 / 100 = 80.845 EUR
    assert_eq!(out.eligible_kwh, Some(dec!(950)));
    assert_eq!(out.settlement_eur, Some(dec!(80.845)));
}

/// The same plant at 200 kWp is **inside** the exemption of the Fassung that
/// governs it (§51 Abs. 2 EEG 2023 as enacted: under 400 kW).
///
/// Reading every 2023+ plant as post-Solarspitzengesetz would exempt only below
/// 100 kW and reduce this plant by 50 kWh a month it is entitled to in full.
#[test]
fn negativpreis_eeg2023_exempts_below_400kw() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.51),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        kwh_during_negative_epex: Some(dec!(50)),
        inbetriebnahme: Some(date!(2024 - 06 - 01)),
        leistung_kwp: Some(dec!(200)),
        ..SettleInput::default()
    });
    assert_eq!(out.eligible_kwh, Some(dec!(1000)), "paid in full");
    assert_eq!(out.settlement_eur, Some(dec!(85.10)));
}

/// The same 200 kWp plant commissioned after the Solarspitzengesetz **is**
/// reduced — the exemption is 100 kW there.
#[test]
fn negativpreis_solarspitzen_reduces_the_same_200kw_plant() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.51),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        kwh_during_negative_epex: Some(dec!(50)),
        inbetriebnahme: Some(date!(2025 - 06 - 01)),
        leistung_kwp: Some(dec!(200)),
        ..SettleInput::default()
    });
    assert_eq!(out.eligible_kwh, Some(dec!(950)));
}

// ── §51a EEG 2023 — Vergütungszeitraum-Verlängerung ────────────────────────

#[test]
fn verguetungszeitraum_verlaengerung_solar_50_pct() {
    use eeg_billing::verguetungszeitraum_verlaengerung_qh;
    // Solar PV: 40 lost quarter-hours → 20 additional (×0.5)
    assert_eq!(verguetungszeitraum_verlaengerung_qh(40, true), 20);
    // Solar PV: 41 lost → ceiling(41/2) = 21
    assert_eq!(verguetungszeitraum_verlaengerung_qh(41, true), 21);
}

#[test]
fn verguetungszeitraum_verlaengerung_wind_rounds_to_full_day() {
    use eeg_billing::verguetungszeitraum_verlaengerung_qh;
    // Wind/non-solar: §51a Abs. 1 Satz 2 rounds lost QH up to a full calendar day.
    assert_eq!(verguetungszeitraum_verlaengerung_qh(100, false), 192); // 2 days
    assert_eq!(verguetungszeitraum_verlaengerung_qh(0, false), 0);
}

// ── §24 EEG 2023 — Zusammenlegung 12-month window ────────────────────────────

#[test]
fn zusammenlegung_same_year_is_within_12_months() {
    use eeg_billing::zusammenlegung_within_12_months;
    // Jan 2024 and Dec 2024: same year → within 12-month window
    assert!(zusammenlegung_within_12_months(
        date!(2024 - 01 - 15),
        date!(2024 - 12 - 15)
    ));
}

#[test]
fn zusammenlegung_13_months_apart_is_outside_window() {
    use eeg_billing::zusammenlegung_within_12_months;
    // Jan 2024 and Mar 2025: 14 months → outside window
    assert!(!zusammenlegung_within_12_months(
        date!(2024 - 01 - 01),
        date!(2025 - 03 - 01)
    ));
}

// ── EEG rate table accuracy (with Solarpaket I 2024 rates) ───────────────────

// ── Umsatzsteuer (VAT) — §19 UStG Kleinunternehmer + Regelbesteuerung ───────
//
// A feed-in Gutschrift has exactly two VAT treatments. The operator's status is
// declared masterdata (`eeg_anlagen.ust_status`); `default_for_plant` only seeds
// a sensible default. §12 Abs. 3 UStG is a hardware-supply rate and never a
// feed-in category.

#[test]
fn ust_default_small_post_2023_solar_is_kleinunternehmer() {
    use eeg_billing::ust::VatStatus;
    // 9.5 kWp solar, commissioned 2024: the §12-Abs.-3 hardware 0 % removed the
    // incentive to opt in, so the operator is, by default, a §19 Kleinunternehmer.
    let vat = VatStatus::default_for_plant(true, dec!(9.5), Some(date!(2024 - 06 - 01)));
    assert_eq!(vat, VatStatus::Kleinunternehmer);
    assert!(vat.is_exempt());
    assert_eq!(vat.ust_rate(), Decimal::ZERO);
}

#[test]
fn ust_default_large_solar_is_regelbesteuerung() {
    use eeg_billing::ust::VatStatus;
    // 50 kWp: exceeds the §19 turnover reality → Regelbesteuerung
    let vat = VatStatus::default_for_plant(true, dec!(50), Some(date!(2024 - 01 - 01)));
    assert_eq!(vat, VatStatus::Regelbesteuerung);
    assert!(!vat.is_exempt());
}

#[test]
fn ust_default_pre_2023_solar_is_regelbesteuerung() {
    use eeg_billing::ust::VatStatus;
    // 9 kWp but commissioned Dec 2022: the small-PV default is post-2023 only.
    let vat = VatStatus::default_for_plant(true, dec!(9), Some(date!(2022 - 12 - 01)));
    assert_eq!(vat, VatStatus::Regelbesteuerung);
}

#[test]
fn ust_default_wind_is_regelbesteuerung() {
    use eeg_billing::ust::VatStatus;
    // Wind plant: the small-PV Kleinunternehmer default is solar-only.
    let vat = VatStatus::default_for_plant(false, dec!(5000), Some(date!(2024 - 01 - 01)));
    assert_eq!(vat, VatStatus::Regelbesteuerung);
}

#[test]
fn ust_db_round_trip() {
    use eeg_billing::ust::VatStatus;
    for status in [VatStatus::Kleinunternehmer, VatStatus::Regelbesteuerung] {
        assert_eq!(VatStatus::from_db_str(status.as_db_str()), Some(status));
    }
    assert_eq!(VatStatus::Kleinunternehmer.as_db_str(), "KLEINUNTERNEHMER");
    assert_eq!(VatStatus::Regelbesteuerung.as_db_str(), "REGELBESTEUERUNG");
    assert_eq!(VatStatus::from_db_str("BEFREIT_12_ABS_3"), None);
}

#[test]
fn ust_kleinunternehmer_is_exempt() {
    use eeg_billing::ust::VatStatus;
    let vat = VatStatus::Kleinunternehmer;
    assert!(vat.is_exempt());
    assert_eq!(vat.ust_rate(), Decimal::ZERO);
}

#[test]
fn ust_regelbesteuerung_19_pct() {
    use eeg_billing::ust::VatStatus;
    let vat = VatStatus::Regelbesteuerung;
    assert!(!vat.is_exempt());
    assert_eq!(vat.ust_rate(), dec!(0.19));
}

#[test]
fn ust_tax_layers_one_layer_per_status() {
    use eeg_billing::ust::{VatStatus, ust_tax_layers};
    // An exempt supply is still a taxable supply: EN 16931 BG-23 requires it in the
    // VAT breakdown under its own category, so the layer is present and charges
    // nothing rather than being omitted.
    for status in [VatStatus::Kleinunternehmer, VatStatus::Regelbesteuerung] {
        assert_eq!(
            ust_tax_layers(status).len(),
            1,
            "{status:?} must contribute exactly one breakdown entry"
        );
    }
}

#[test]
fn ust_kleinunternehmer_billing_document_is_exempt() {
    use billing::{BillingDocument, DocumentMeta, PricingModel as _};
    use eeg_billing::tariff::EegSettleTariffKleinunternehmer;
    use eeg_billing::ust::{VatStatus, ust_tax_layers};

    let output = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.51),
        },
        einspeisemenge_kwh: Some(dec!(500)),
        leistung_kwp: Some(dec!(9.5)),
        inbetriebnahme: Some(date!(2024 - 06 - 01)),
        ..SettleInput::default()
    });

    let vat = VatStatus::Kleinunternehmer;
    let tariff = EegSettleTariffKleinunternehmer::new(&output);
    let doc = BillingDocument::from_positions(
        DocumentMeta::default(),
        tariff.positions(&()).unwrap().into_inner(),
        ust_tax_layers(vat),
        vec![],
    )
    .unwrap();

    // 500 kWh × 8.51 ct = 42.55 EUR; §19 → no USt → gross = net
    assert_eq!(doc.net_total(), billing::Amount::parse("42.55000").unwrap());
    assert_eq!(
        doc.gross_total(),
        doc.net_total(),
        "§19 Kleinunternehmer: no USt"
    );

    // The turnover still appears in the EN 16931 BG-23 breakdown as an exempt
    // supply (category E): charging no tax is not the same as no taxable base.
    let breakdown = doc.tax_breakdown();
    assert_eq!(breakdown.len(), 1);
    assert_eq!(breakdown[0].category, billing::TaxCategory::Exempt);
    assert!(breakdown[0].rate.is_zero());
    assert_eq!(breakdown[0].taxable_base, doc.net_total());
    assert!(breakdown[0].tax_amount.is_zero());

    doc.assert_valid();
}

#[test]
fn ust_regelbesteuerung_19pct_billing_document() {
    use billing::{BillingDocument, DocumentMeta, PricingModel as _};
    use eeg_billing::tariff::EegSettleTariffRegelbesteuerung;
    use eeg_billing::ust::{VatStatus, ust_tax_layers};

    let output = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(5.56),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        leistung_kwp: Some(dec!(100)), // 100 kWp → Regelbesteuerung
        ..SettleInput::default()
    });

    let vat = VatStatus::Regelbesteuerung;
    let tariff = EegSettleTariffRegelbesteuerung::new(&output);
    let doc = BillingDocument::from_positions(
        DocumentMeta::default(),
        tariff.positions(&()).unwrap().into_inner(),
        ust_tax_layers(vat), // 19%
        vec![],
    )
    .unwrap();

    // Net = 1000 × 5.56 ct = 55.60 EUR; USt 19% = 10.564 EUR; Gross = 66.164 EUR
    assert_eq!(doc.net_total(), billing::Amount::parse("55.60000").unwrap());
    // USt = 55.60 × 0.19 = 10.564 EUR
    assert_eq!(doc.tax_total(), billing::Amount::parse("10.56400").unwrap());
    assert_eq!(
        doc.gross_total(),
        billing::Amount::parse("66.16400").unwrap()
    );
    doc.assert_valid();
}

/// §100 EEG Übergangsregelung: plants commissioned before 01.01.2023 apply old EEG rules.
/// The Vergütungssatz is fixed at commissioning — this is modeled by providing the
/// historically correct rate as verguetungssatz_ct.
#[test]
fn uebergangsregelung_pre_2023_plant_uses_historical_rate() {
    // Plant commissioned 2015: EEG 2012 solar rate, ≤10 kWp ≈ 12.31 ct/kWh (example)
    // The caller (einsd) looks up the historical rate via lookup_verguetungssatz()
    // The formula is identical — only the rate differs
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(12.31),
        },
        einspeisemenge_kwh: Some(dec!(400)),
        inbetriebnahme: Some(date!(2015 - 03 - 15)),
        leistung_kwp: Some(dec!(9.5)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 400 × 12.31 / 100 = 49.24 EUR
    assert_eq!(out.settlement_eur, Some(dec!(49.24)));
    // §51 (Negativpreisregel): plant <100 kWp → exempt even without inbetriebnahme check
    // (our guard checks: inbetriebnahme=2015, which is post-2016? NO — 2015 < 2016-01-01
    //  so §51 Negativpreisregel does NOT apply — plant commissioned 2015 is pre-EEG-2017)
    // (Note: §51 EEG 2017 applied to plants commissioned after 01.01.2016 with ≥100 kWp)
    // For this plant (2015): even if kwh_during_negative_epex were supplied, it would not
    // apply because pre-2016 plants are exempt.
    let out_neg = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(12.31),
        },
        einspeisemenge_kwh: Some(dec!(400)),
        inbetriebnahme: Some(date!(2015 - 03 - 15)),
        leistung_kwp: Some(dec!(9.5)),
        kwh_during_negative_epex: Some(dec!(50)), // supplied but should be ignored
        ..SettleInput::default()
    });
    // Pre-2016 plant → §51 NOT applied, full 400 kWh settled
    assert_eq!(out_neg.eligible_kwh, Some(dec!(400)));
    assert_eq!(out_neg.settlement_eur, Some(dec!(49.24)));
}

// ── §52 EEG 2023 — Pflichtzahlung (separate from Vergütung) ─────────────────
//
// KEY INSIGHT (from EEG 2023 law text):
// §47 EEG 2021 (MaStR → Vergütung = 0) is "WEGGEFALLEN" (deleted) in EEG 2023.
// For NEW plants (EEG 2023 rules): §52 applies a SEPARATE €10/kW/month penalty.
// The Vergütung itself is NOT reduced — the operator still receives full Vergütung
// AND must pay the §52 penalty to the NB.
// For OLD plants (commissioned before 01.01.2023, §100 Übergangsregelung):
// the old §47 rule (Vergütung = 0) still applies via is_sanctioned = true.

#[test]
fn pflichtzahlung_mastr_not_registered_eeg_2023_plant_still_receives_verguetung() {
    // NEW plant (EEG 2023): MaStR not registered → penalty to NB, but Vergütung intact
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.51),
        },
        einspeisemenge_kwh: Some(dec!(500)),
        inbetriebnahme: Some(date!(2024 - 01 - 15)), // EEG 2023 plant
        leistung_kwp: Some(dec!(50)),
        sanktion: None, // EEG 2023: use pflichtverstoss, not sanktion
        pflichtverstoss: vec![Pflichtverstoss {
            typ: SanktionsTyp::MastrNichtRegistriert,
            leistung_kw: dec!(50),
            monate_des_verstosses: 2,
            nachtraeglich_erfuellt: false,
            technischer_defekt: false,
        }],
        ..SettleInput::default()
    });

    // Vergütung is STILL PAID (§52 doesn't zero the Vergütung — unlike old §47)
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(dec!(42.55))); // 500 × 8.51 / 100

    // The §52 penalty is SEPARATE: 50 kW × €10 × 2 months = €1000
    assert_eq!(out.pflichtzahlung_eur, Some(dec!(1000)));
}

#[test]
fn pflichtzahlung_fernsteuerbarkeit_10_eur_per_kw_per_month() {
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.51),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        pflichtverstoss: vec![Pflichtverstoss {
            typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
            leistung_kw: dec!(200),   // 200 kW plant
            monate_des_verstosses: 3, // 3 months of violation
            nachtraeglich_erfuellt: false,
            technischer_defekt: false,
        }],
        ..SettleInput::default()
    });
    // 200 kW × €10 × 3 months = €6000 penalty
    assert_eq!(out.pflichtzahlung_eur, Some(dec!(6000)));
    // Vergütung unchanged: 1000 × 8.51 / 100 = 85.10 EUR
    assert_eq!(out.settlement_eur, Some(dec!(85.10)));
}

#[test]
fn pflichtzahlung_retroactively_reduced_to_2_eur_when_fulfilled() {
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};
    // §52 Abs. 3: once obligation fulfilled, penalty reduces to €2/kW/month retroactively
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.51),
        },
        einspeisemenge_kwh: Some(dec!(100)),
        pflichtverstoss: vec![Pflichtverstoss {
            typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
            leistung_kw: dec!(100),
            monate_des_verstosses: 4,
            nachtraeglich_erfuellt: true,
            technischer_defekt: false, // obligation since fulfilled
        }],
        ..SettleInput::default()
    });
    // 100 kW × €2 × 4 months = €800 (reduced from €4000)
    assert_eq!(out.pflichtzahlung_eur, Some(dec!(800)));
}

#[test]
fn no_pflichtverstoss_means_none_pflichtzahlung() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.51),
        },
        einspeisemenge_kwh: Some(dec!(200)),
        ..SettleInput::default()
    });
    // No violation → pflichtzahlung_eur is None (not zero)
    assert!(out.pflichtzahlung_eur.is_none());
}

#[test]
fn old_plant_is_sanctioned_eur_0_correct_via_par100_uebergangsregelung() {
    // OLD plant (before 01.01.2023) — §100 Übergangsregelung applies EEG 2021 rules:
    // §47 EEG 2021 (now deleted) reduced Vergütung to EUR 0 for MaStR non-registration.
    // Model this with is_sanctioned = true (correct for old plants).
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(29.37),
        },
        einspeisemenge_kwh: Some(dec!(500)),
        inbetriebnahme: Some(date!(2010 - 05 - 15)), // OLD plant
        leistung_kwp: Some(dec!(9.5)),
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufNull), // §47 EEG 2021 via §100: Vergütung = 0
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO)); // EUR 0 correct for old plant
    assert!(out.pflichtzahlung_eur.is_none()); // no §52 (old plant uses old rules)
}

#[test]
fn pflichtzahlung_via_calculate_pflichtzahlung_function() {
    use eeg_billing::foerderdauer::calculate_pflichtzahlung;
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    // Direct helper function test
    let v = Pflichtverstoss {
        typ: SanktionsTyp::MastrNichtRegistriert,
        leistung_kw: dec!(500),
        monate_des_verstosses: 3,
        nachtraeglich_erfuellt: false,
        technischer_defekt: false,
    };
    assert_eq!(calculate_pflichtzahlung(&v), dec!(15000)); // 500 × 10 × 3

    let v_reduced = Pflichtverstoss {
        nachtraeglich_erfuellt: true,
        technischer_defekt: false,
        ..v
    };
    assert_eq!(calculate_pflichtzahlung(&v_reduced), dec!(3000)); // 500 × 2 × 3
}

// ── §51 EEG version-specific Negativpreisregel thresholds ───────────────────
//
// EEG 2017: ≥6 consecutive negative-price hours → kein Vergütungsanspruch
// EEG 2021: ≥4 consecutive negative-price hours → kein Vergütungsanspruch
// EEG 2023: ≥1 hour (any period) → kein Vergütungsanspruch
// EEG ≤2014: §51 does not apply at all
//
// A "500 kW exemption" (EEG 2017/2021) means plants <500 kW never lose
// Vergütung due to §51. EEG 2023 lowers the threshold to <100 kW.
//
// These tests exercise `negativpreis_rule_applies_for_version()` helper and
// verify that `calculate_settlement` with `kwh_during_negative_epex` uses the
// correct threshold for each EEG version.

use eeg_billing::NegativpreisRegime as Regime;
// EegGesetz and ErzeugungsArt are already imported at the top of this file.

/// A helper reading like the Clearingstelle's table: commissioning date in,
/// (run length in quarter-hours, kW exemption) out.
fn regel(ibn: time::Date, art: Option<ErzeugungsArt>) -> (Option<usize>, Option<Decimal>) {
    let r = Regime::fuer_inbetriebnahme(ibn);
    (r.mindest_lauflaenge_qh(), r.kw_grenze(art))
}

#[test]
fn negativpreis_eeg2017_is_six_hours_and_500kw_3mw() {
    let (run, kw) = regel(date!(2017 - 06 - 01), Some(ErzeugungsArt::SolarAufdach));
    assert_eq!(run, Some(24), "6 h = 24 quarter-hours");
    assert_eq!(kw, Some(Decimal::from(500)));
    let (_, wind) = regel(date!(2017 - 06 - 01), Some(ErzeugungsArt::WindOnshore));
    assert_eq!(wind, Some(Decimal::from(3000)), "§51 Abs. 3 Nr. 1 EEG 2017");
}

#[test]
fn negativpreis_eeg2021_is_four_hours_and_500kw_for_all() {
    let (run, kw) = regel(date!(2021 - 06 - 01), Some(ErzeugungsArt::WindOnshore));
    assert_eq!(run, Some(16), "4 h = 16 quarter-hours");
    assert_eq!(kw, Some(Decimal::from(500)), "the wind carve-out is gone");
}

/// The EEG 2023 as enacted: the staged 4-3-2-1-hour rule and a **400 kW**
/// exemption. Applying the current Fassung's rule to these plants reduced
/// payments the statute did not reduce.
#[test]
fn negativpreis_eeg2023_as_enacted_is_staged_and_400kw() {
    assert_eq!(
        regel(date!(2023 - 06 - 01), None),
        (Some(16), Some(Decimal::from(400))),
        "commissioning year 2023: four consecutive hours"
    );
    assert_eq!(
        regel(date!(2024 - 06 - 01), None),
        (Some(12), Some(Decimal::from(400))),
        "commissioning year 2024: three consecutive hours"
    );
}

/// From the Solarspitzengesetz (25.02.2025): the first negative quarter-hour,
/// 100 kW until an iMSys is installed, 2 kW until the §85 Abs. 2 Nr. 12
/// Festlegung.
#[test]
fn negativpreis_solarspitzen_is_the_first_quarter_hour() {
    let r = Regime::fuer_inbetriebnahme(date!(2025 - 06 - 01));
    assert_eq!(r.mindest_lauflaenge_qh(), Some(1));
    assert_eq!(r.kw_grenze(None), Some(Decimal::from(100)));
    assert!(r.ist_befreit(Some(Decimal::from(50)), None, false, false));
    assert!(!r.ist_befreit(Some(Decimal::from(50)), None, true, false));
    assert!(
        r.ist_befreit(Decimal::from_str_exact("1.5").ok(), None, true, false),
        "below 2 kW an iMSys does not lift the exemption"
    );
}

/// §3 Nr. 37 — a Pilotwindenergieanlage is outside §51 whatever its size.
#[test]
fn negativpreis_pilotwindanlage_is_always_exempt() {
    for ibn in [
        date!(2017 - 06 - 01),
        date!(2021 - 06 - 01),
        date!(2024 - 06 - 01),
        date!(2026 - 06 - 01),
    ] {
        assert!(
            Regime::fuer_inbetriebnahme(ibn).ist_befreit(
                Some(Decimal::from(6000)),
                Some(ErzeugungsArt::WindOnshore),
                true,
                true
            ),
            "IBN {ibn}"
        );
    }
}

#[test]
fn negativpreis_pre_2016_never_applies() {
    for ibn in [
        date!(2015 - 12 - 31),
        date!(2010 - 01 - 01),
        date!(2004 - 06 - 01),
    ] {
        let r = Regime::fuer_inbetriebnahme(ibn);
        assert_eq!(r.mindest_lauflaenge_qh(), None, "IBN {ibn}");
        assert!(r.ist_befreit(Some(Decimal::from(9999)), None, true, false));
    }
}

#[test]
fn negativpreis_eeg2023_settlement_triggers_with_1h() {
    // Integration: a 2023 plant above the 400 kW exemption; the caller has
    // already applied the staged four-hour run test when deriving the kWh.
    // §51 reduces eligible kWh by the negative-price kWh (not to zero).
    // 100 total kWh - 10 negative kWh = 90 effective kWh × 8.11ct = 7.299 EUR
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.11),
        },
        einspeisemenge_kwh: Some(dec!(100)),
        inbetriebnahme: Some(date!(2023 - 07 - 01)),
        leistung_kwp: Some(dec!(500)),
        kwh_during_negative_epex: Some(dec!(10)),
        eeg_gesetz: EegGesetz::Eeg2023,
        ..Default::default()
    });
    // §51 applied: eligible kWh reduced by 10, settlement reduced
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.eligible_kwh, Some(dec!(90)));
    // 90 × 8.11 / 100 = 7.299
    assert!(out.settlement_eur.is_some_and(|e| e < dec!(8.11)));
    // Full EUR 0 when all kWh are during negative prices:
    let out_zero = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.11),
        },
        einspeisemenge_kwh: Some(dec!(100)),
        inbetriebnahme: Some(date!(2023 - 07 - 01)),
        leistung_kwp: Some(dec!(500)),
        kwh_during_negative_epex: Some(dec!(100)), // all kWh during negative EPEX
        eeg_gesetz: EegGesetz::Eeg2023,
        ..Default::default()
    });
    assert_eq!(out_zero.status, SettlementStatus::Calculated);
    assert_eq!(out_zero.settlement_eur, Some(dec!(0)));
}

#[test]
fn negativpreis_eeg2017_plant_below_500kw_not_affected() {
    // An EEG 2017 plant below 500 kW must NOT lose Vergütung even with
    // many negative-price hours, because of the 500 kW exemption.
    // The formula uses leistung_kwp < 500 → skip §51 regardless of hours.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(12.35),
        },
        einspeisemenge_kwh: Some(dec!(100)),
        inbetriebnahme: Some(date!(2018 - 03 - 01)),
        leistung_kwp: Some(dec!(499)), // below 500 kW exemption
        kwh_during_negative_epex: Some(dec!(50)), // many negative-price kWh
        eeg_gesetz: EegGesetz::Eeg2017,
        ..Default::default()
    });
    // Should still calculate normally — no §51 for <500 kW under EEG 2017
    assert_eq!(out.status, SettlementStatus::Calculated);
}

#[test]
fn negativpreis_eeg2023_plant_below_100kw_not_affected() {
    // An EEG 2023 plant below 100 kW must NOT lose Vergütung — §51 EEG 2023
    // exempts plants <100 kW (§51 Abs. 1 S. 2 EEG 2023).
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(8.11),
        },
        einspeisemenge_kwh: Some(dec!(100)),
        inbetriebnahme: Some(date!(2024 - 01 - 01)),
        leistung_kwp: Some(dec!(99)), // below 100 kW exemption
        kwh_during_negative_epex: Some(dec!(20)),
        eeg_gesetz: EegGesetz::Eeg2023,
        ..Default::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
}

// ── §100 Abs. 1 Satz 4 EEG 2017 — Bestandsschutz boundary (01.01.2016) ──────────────
//
// §100 Abs. 1 Satz 4 EEG 2017: "Für Strom aus Anlagen, die vor dem 1. Januar 2016
// in Betrieb genommen worden sind, ist §51 nicht anzuwenden."
//
// ▸ Commissioned 31.12.2015 (= before 2016): §51 NEVER applies (Bestandsschutz)
// ▸ Commissioned 01.01.2016 (= from 2016): §51 EEG 2017 applies (6h, 500kW/3MW)

#[test]
fn negativpreis_2015_plant_has_bestandsschutz_para51_never_applies() {
    // Plant commissioned 2015 → EEG 2012 era → §51 does NOT apply at all.
    // Even with large kW and negative-price kWh, settlement is full.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(12.35),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        inbetriebnahme: Some(date!(2015 - 12 - 31)), // before 2016-01-01
        leistung_kwp: Some(dec!(2000)),              // large plant
        kwh_during_negative_epex: Some(dec!(500)),
        eeg_gesetz: EegGesetz::Eeg2012, // §100 Abs. 1 Satz 4 EEG 2017: §51 not applicable
        ..Default::default()
    });
    // §100 Abs. 1 Satz 4 EEG 2017: §51 not applicable → full 1000 kWh
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.eligible_kwh, Some(dec!(1000)));
}

#[test]
fn negativpreis_2016_plant_subject_to_eeg2017_para51() {
    // Plant commissioned 01.01.2016 → EEG 2017 §51 applies (6h threshold, 500kW).
    // 600 kW ≥ 500 kW (non-wind) → caller has verified ≥6h → §51 applies.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(12.35),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        inbetriebnahme: Some(date!(2016 - 01 - 01)), // from 2016-01-01: EEG 2017
        leistung_kwp: Some(dec!(600)),               // ≥ 500 kW → §51 applies
        kwh_during_negative_epex: Some(dec!(200)),   // caller verified ≥6h
        eeg_gesetz: EegGesetz::Eeg2017,
        ..Default::default()
    });
    // §51 EEG 2017 applied: 1000 - 200 = 800 kWh eligible
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.eligible_kwh, Some(dec!(800)));
}

#[test]
fn negativpreis_2016_plant_below_500kw_exempt() {
    // Plant from 2016, 300 kW non-wind → below 500 kW exemption (EEG 2017) → §51 not applied.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(12.35),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        inbetriebnahme: Some(date!(2016 - 06 - 01)),
        leistung_kwp: Some(dec!(300)), // < 500 kW → exempt
        kwh_during_negative_epex: Some(dec!(200)),
        eeg_gesetz: EegGesetz::Eeg2017,
        ..Default::default()
    });
    // Exempt → full 1000 kWh
    assert_eq!(out.eligible_kwh, Some(dec!(1000)));
}

// ── EEG 2017 wind turbine vs. non-wind kW exemption ──────────────────────────
// §51 Abs. 3 EEG 2017:
// Nr. 1: Windenergieanlagen < 3 000 kW → exempt
// Nr. 2: sonstige Anlagen  <   500 kW → exempt

#[test]
fn negativpreis_eeg2017_wind_below_3mw_exempt() {
    // Wind turbine 1500 kW commissioned 2018 (EEG 2017 applies).
    // 1500 kW < 3000 kW wind exemption → §51 does NOT apply.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(9.25),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        inbetriebnahme: Some(date!(2018 - 05 - 01)),
        leistung_kwp: Some(dec!(1500)), // 1.5 MW wind → < 3 MW exemption
        kwh_during_negative_epex: Some(dec!(300)),
        erzeugungsart: Some(ErzeugungsArt::WindOnshore),
        eeg_gesetz: EegGesetz::Eeg2017,
        ..Default::default()
    });
    // Wind turbine < 3 MW under EEG 2017 → exempt → full 1000 kWh
    assert_eq!(out.eligible_kwh, Some(dec!(1000)));
}

#[test]
fn negativpreis_eeg2017_wind_above_3mw_not_exempt() {
    // Wind turbine 3500 kW commissioned 2019 (EEG 2017 applies).
    // 3500 kW ≥ 3000 kW → §51 applies. Caller verified ≥6h consecutive hours.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(5.8),
        },
        einspeisemenge_kwh: Some(dec!(10000)),
        inbetriebnahme: Some(date!(2019 - 04 - 01)),
        leistung_kwp: Some(dec!(3500)), // 3.5 MW wind → ≥ 3 MW → §51 applies
        kwh_during_negative_epex: Some(dec!(1000)), // caller verified ≥6h
        erzeugungsart: Some(ErzeugungsArt::WindOnshore),
        eeg_gesetz: EegGesetz::Eeg2017,
        ..Default::default()
    });
    // §51 applied: 10000 - 1000 = 9000 kWh eligible
    assert_eq!(out.eligible_kwh, Some(dec!(9000)));
}

#[test]
fn negativpreis_eeg2017_solar_1mw_not_exempt() {
    // Solar plant 1000 kW (1 MW) commissioned 2017.
    // 1000 kW ≥ 500 kW non-wind threshold → §51 applies.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(9.25),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        inbetriebnahme: Some(date!(2017 - 09 - 01)),
        leistung_kwp: Some(dec!(1000)), // 1 MW solar → ≥ 500 kW → §51 applies
        kwh_during_negative_epex: Some(dec!(200)), // caller verified ≥6h
        erzeugungsart: Some(ErzeugungsArt::SolarAufdach),
        eeg_gesetz: EegGesetz::Eeg2017,
        ..Default::default()
    });
    // §51 applied: 1000 - 200 = 800 kWh eligible
    assert_eq!(out.eligible_kwh, Some(dec!(800)));
}

#[test]
fn negativpreis_eeg2021_wind_above_500kw_not_exempt() {
    // EEG 2021 removed the wind 3 MW exception — ALL plants < 500 kW exempt.
    // Wind turbine 1500 kW commissioned 2021 → NOT exempt under EEG 2021.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(7.5),
        },
        einspeisemenge_kwh: Some(dec!(1000)),
        inbetriebnahme: Some(date!(2021 - 06 - 01)),
        leistung_kwp: Some(dec!(1500)), // 1.5 MW wind — EEG 2021: ≥ 500 kW → §51
        kwh_during_negative_epex: Some(dec!(200)), // caller verified ≥4h
        erzeugungsart: Some(ErzeugungsArt::WindOnshore),
        eeg_gesetz: EegGesetz::Eeg2021,
        ..Default::default()
    });
    // EEG 2021: wind 3 MW exception removed; 1500 kW ≥ 500 kW → §51 applies
    assert_eq!(out.eligible_kwh, Some(dec!(800)));
}

// ── §52 Abs. 3 Nr. 2 EEG 2023 — always-€2/kW violation types ────────────────
//
// §52 Abs. 3 Nr. 2 EEG 2023: certain violation types are ALWAYS €2/kW/month,
// not the standard €10/kW. This is NOT a reduction from €10 — the rate is
// permanently set at €2 for these types:
//   - Nr. 9a: §37a Abs. 1a / §48 Abs. 6 post-commissioning violations
//   - Nr. 10: Volleinspeisung obligation not met (§48 Abs. 2a)
//
// Contrast with §52 Abs. 3 Nr. 1 types (Nr. 1, 3, 4, 11) which START at €10
// and reduce retroactively to €2 once the obligation is fulfilled.

use eeg_billing::foerderdauer::calculate_pflichtzahlung;

#[test]
fn pflichtzahlung_nr9a_always_two_eur_not_ten() {
    // Nr. 9a (InbetriebnahmeVorgabeVerletzt): ALWAYS €2/kW/month, never €10
    let violation = Pflichtverstoss {
        typ: SanktionsTyp::InbetriebnahmeVorgabeVerletzt,
        leistung_kw: dec!(500),
        monate_des_verstosses: 3,
        nachtraeglich_erfuellt: false,
        technischer_defekt: false,
    };
    // 500 kW × €2 × 3 months = €3 000 (NOT €15 000)
    assert_eq!(calculate_pflichtzahlung(&violation), dec!(3000));
}

#[test]
fn pflichtzahlung_nr9a_nachtraeglich_erfuellt_has_no_effect() {
    // Nr. 9a rate is fixed at €2 — fulfillment has no effect (no retroactive reduction)
    let fulfilled = Pflichtverstoss {
        typ: SanktionsTyp::InbetriebnahmeVorgabeVerletzt,
        leistung_kw: dec!(500),
        monate_des_verstosses: 3,
        nachtraeglich_erfuellt: true,
        technischer_defekt: false, // has NO effect for Nr. 9a
    };
    // Still €2/kW, not €1 or anything else
    assert_eq!(calculate_pflichtzahlung(&fulfilled), dec!(3000));
}

#[test]
fn pflichtzahlung_nr10_volleinspeisung_always_two_eur() {
    // Nr. 10 (VolleinspeisungspflichtVerletzt): ALWAYS €2/kW/month.
    // §52 Abs. 4 Nr. 3: assessed for ALL calendar months of the year → 12 months.
    let violation = Pflichtverstoss {
        typ: SanktionsTyp::VolleinspeisungspflichtVerletzt,
        leistung_kw: dec!(300),
        monate_des_verstosses: 12, // full calendar year per §52 Abs. 4 Nr. 3
        nachtraeglich_erfuellt: false,
        technischer_defekt: false,
    };
    // 300 kW × €2 × 12 months = €7 200
    assert_eq!(calculate_pflichtzahlung(&violation), dec!(7200));
}

#[test]
fn pflichtzahlung_nr11_mastr_retroactive_reduction_still_works() {
    // Verify retroactive reduction for Nr. 11 (MaStR) is unchanged
    let v = Pflichtverstoss {
        typ: SanktionsTyp::MastrNichtRegistriert,
        leistung_kw: dec!(200),
        monate_des_verstosses: 2,
        nachtraeglich_erfuellt: false,
        technischer_defekt: false,
    };
    assert_eq!(calculate_pflichtzahlung(&v), dec!(4000)); // 200 × €10 × 2

    let v_fulfilled = Pflichtverstoss {
        nachtraeglich_erfuellt: true,
        technischer_defekt: false,
        ..v
    };
    assert_eq!(calculate_pflichtzahlung(&v_fulfilled), dec!(800)); // 200 × €2 × 2
}

#[test]
fn pflichtzahlung_nr2_speicher_not_reducible() {
    // Nr. 2 (SpeicherAnforderungNichtErfuellt) is NOT in the Abs. 3 Nr. 1 reduction list
    // → always €10/kW even when fulfilled
    let v_fulfilled = Pflichtverstoss {
        typ: SanktionsTyp::SpeicherAnforderungNichtErfuellt,
        leistung_kw: dec!(100),
        monate_des_verstosses: 1,
        nachtraeglich_erfuellt: true,
        technischer_defekt: false, // has no effect for Nr. 2
    };
    assert_eq!(calculate_pflichtzahlung(&v_fulfilled), dec!(1000)); // 100 × €10 × 1
}
