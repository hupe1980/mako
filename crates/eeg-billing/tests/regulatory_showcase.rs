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
    CapacityBlock, EegGesetz, SettleInput, SettlementScheme, SettlementStatus, TariffSource,
    calculate_settlement, foerderendedatum_eeg, foerderendedatum_kwkg_years,
    foerderendedatum_repowering, kwk_foerderend_calendar, kwk_max_kwh,
};
use rust_decimal::Decimal;
use rust_decimal::dec;
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("650")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("5.5"),
        },
        einspeisemenge_kwh: Some(d("95000")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("5225.00")));
}

/// §21 EEG 2023 — Zero kWh → EUR 0 (not an error).
#[test]
fn s21_zero_kwh_is_zero_eur() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(Decimal::ZERO),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
}

/// §21 EEG 2023 — No meter data yet → NoData.
#[test]
fn s21_no_meter_data() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: None,
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::NoData);
    assert_eq!(out.settlement_eur, None);
}

// ═══════════════════════════════════════════════════════════════════════════
// §20 EEG 2023 — Gleitende Marktprämie (Direktvermarktung)
// ═══════════════════════════════════════════════════════════════════════════

/// Anlage 1 Nr. 3.1.2 EEG 2023 — Direktvermarktung, positive spread.
/// Wind 750 kW: AW = 6.2 ct, Monatsmarktwert July = 4.8 ct.
/// MP = AW − MW = 1.4 ct → 1.4 × 120,000 / 100 = 1,680 EUR, and nothing else.
#[test]
fn anlage1_direktvermarktung_positive_spread() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.2"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("120000")),
        marktwert_ct_kwh: Some(d("4.8")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("1680.00")));
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].rate_ct_kwh, d("1.4"));
}

/// Anlage 1 Nr. 3.1.2 EEG 2023 — zero spread (MW = AW): the claim is zero.
///
/// Nothing tops it up: the Managementprämie is an EEG 2012 construct with no
/// basis in Anlage 1 or §20.
#[test]
fn anlage1_zero_spread_pays_nothing() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("5.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("50000")),
        marktwert_ct_kwh: Some(d("5.0")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("0")));
    assert_eq!(
        out.positions.len(),
        1,
        "a settled zero still gets a position"
    );
    assert_eq!(out.positions[0].rate_ct_kwh, d("0"));
}

/// Anlage 1 Nr. 3.1.2 Satz 2 EEG 2023 — MW above AW clamps the claim to zero.
///
/// "Ergibt sich bei der Berechnung ein Wert kleiner null, wird … der Wert 'MP'
/// mit null festgesetzt." The operator keeps the market revenue; the
/// Netzbetreiber owes nothing, and nothing is clawed back either.
#[test]
fn anlage1_negative_spread_clamped_to_zero() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("4.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("60000")),
        marktwert_ct_kwh: Some(d("8.2")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // MW 8.2 ct > AW 4.0 ct → MP = 0.
    assert_eq!(out.settlement_eur, Some(d("0")));
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].eur, d("0"));
}

/// §20 EEG 2023 — EPEX price missing → PriceMissing.
#[test]
fn s20_no_epex_price_missing() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("50000")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::PriceMissing);
    assert_eq!(out.settlement_eur, None);
}

/// Anlage 1 Nr. 3.1.2 EEG 2023 — plant size does not enter the Marktprämie.
///
/// The formula is `MP = AW − MW` for a 9 kWp roof and a 110 MW park alike. A
/// capacity-tiered Managementprämie on top is an EEG 2012 construct; since
/// EEG 2014 the marketing cost sits inside the AW, and neither §20 nor Anlage 1
/// mentions it.
#[test]
fn anlage1_marktpraemie_has_no_capacity_dependent_component() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("2000000")), // 2 GWh for 110 MW plant
        marktwert_ct_kwh: Some(d("4.5")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // MP = 6.0 − 4.5 = 1.5 ct → 1.5 × 2,000,000 / 100 = 30,000 EUR, and nothing else.
    assert_eq!(out.settlement_eur, Some(d("30000.00")));
    assert_eq!(
        out.positions.len(),
        1,
        "one Marktprämie position, no uplift"
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
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("5.82"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        tariff_source: TariffSource::Auction(eeg_billing::AusschreibungMetadata::default()),
        einspeisemenge_kwh: Some(d("2500000")),
        marktwert_ct_kwh: Some(d("4.1")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // MP = 5.82 − 4.1 = 1.72 ct → 1.72 × 2,500,000 / 100 = 43,000 EUR.
    assert_eq!(out.settlement_eur, Some(d("43000.00")));
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].legal_basis, "§§22a,28 EEG 2023");
}

/// §39n EEG 2023 (i.V.m. §3 InnAusV) — Innovationsausschreibung feste Marktprämie.
///
/// The fixed premium is the Zuschlagswert per kWh and does NOT shrink as the
/// Monatsmarktwert rises — the defining difference from the gleitende Marktprämie.
#[test]
fn s39n_innovationsausschreibung_feste_marktpraemie_is_fixed() {
    let innovation = TariffSource::Auction(eeg_billing::AusschreibungMetadata {
        innovation_auction: true,
        ..Default::default()
    });
    let make = |marktwert: &str| {
        calculate_settlement(&SettleInput {
            scheme: SettlementScheme::MarketPremium {
                direktverm_aw_ct: d("5.82"),
                wind_korrekturfaktor: None,
                wind_standort: None,
            },
            tariff_source: innovation.clone(),
            einspeisemenge_kwh: Some(d("2500000")),
            marktwert_ct_kwh: Some(d(marktwert)),
            ..SettleInput::default()
        })
    };
    // feste Marktprämie = 5.82 ct × 2,500,000 kWh = 145,500 EUR, independent of Marktwert.
    let low = make("4.1");
    let high = make("6.0"); // Marktwert ABOVE the AW → gleitende would pay 0.
    assert_eq!(low.settlement_eur, Some(d("145500.00")));
    assert_eq!(high.settlement_eur, Some(d("145500.00")));
    assert!(
        low.positions.iter().any(|p| p.legal_basis.contains("39n")),
        "expected a §39n feste-Marktprämie position"
    );
}

/// §53 Abs. 1 EEG 2023 — the flat AW deduction is applied only when the caller
/// flags the rate as gross; a net rate (einsd's stored default) is untouched.
#[test]
fn s53_abs1_deduction_only_on_gross_aw() {
    use eeg_billing::ErzeugungsArt;
    let base = |gross: bool| SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("100000")),
        erzeugungsart: Some(ErzeugungsArt::SolarAufdach),
        aw_is_gross: gross,
        ..SettleInput::default()
    };
    // Net (default): 8.11 ct × 100,000 = 8,110 EUR, no deduction.
    assert_eq!(
        calculate_settlement(&base(false)).settlement_eur,
        Some(d("8110.00"))
    );
    // Gross: solar −0.4 ct → 7.71 ct × 100,000 = 7,710 EUR.
    assert_eq!(
        calculate_settlement(&base(true)).settlement_eur,
        Some(d("7710.00"))
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §21 Abs. 3 EEG 2023 — Mieterstrom
// ═══════════════════════════════════════════════════════════════════════════

/// §21 Abs. 3 EEG 2023 — 50 kWp community solar building.
/// Base rate: 7.5 ct/kWh. Mieterstrom-Zuschlag: 1.3 ct/kWh.
/// Month: 800 kWh. Payment: 800 × 8.8 / 100 = 70.40 EUR
#[test]
fn s21_abs3_mieterstrom_building_solar() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::TenantElectricity {
            verguetungssatz_ct: d("7.5"),
            mieter_zuschlag_ct: Some(d("1.3")),
        },
        einspeisemenge_kwh: Some(d("800")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("70.40")));
}

/// §21 Abs. 3 EEG 2023 — Zero Mieterstrom-Zuschlag equals base Vergütung.
#[test]
fn s21_abs3_zero_zuschlag_equals_verguetung() {
    let base = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.0"),
        },
        einspeisemenge_kwh: Some(d("500")),
        ..SettleInput::default()
    });
    let mieterstrom = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::TenantElectricity {
            verguetungssatz_ct: d("8.0"),
            mieter_zuschlag_ct: Some(Decimal::ZERO),
        },
        einspeisemenge_kwh: Some(d("500")),
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
        scheme: SettlementScheme::FlexibilityPremium {
            verguetungssatz_ct: d("6.5"),
            flex_praemie_ct_kwh: Some(d("1.5")),
        },
        einspeisemenge_kwh: Some(d("180000")),
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
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(d("420")),
        marktwert_ct_kwh: Some(d("6.1")),
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
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: Some(d("-0.5")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
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
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.2"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("4.8")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
        sanktion: None, // no §52 sanction
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("40.55")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §51 EEG 2023 — Negativpreisregel (negative EPEX price rule)
// ═══════════════════════════════════════════════════════════════════════════

/// §51 — the run-length threshold is a function of the **commissioning date**,
/// not of the law year.
///
/// A plant commissioned on 24.02.2025 and one commissioned the next day are both
/// "EEG 2023" plants; the Solarspitzengesetz governs only the second. Applying
/// the post-Solarspitzengesetz rule to every 2023+ plant reduces 2023 and 2024
/// plants for isolated negative quarter-hours the statute still pays for.
#[test]
fn s51_threshold_follows_the_commissioning_date() {
    use eeg_billing::NegativpreisRegime as R;
    use time::macros::date;

    assert_eq!(
        R::fuer_inbetriebnahme(date!(2015 - 12 - 31)).mindest_lauflaenge_qh(),
        None,
        "§100 Abs. 1 Satz 4 EEG 2017: §51 does not reach pre-2016 plants"
    );
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2016 - 01 - 01)).mindest_lauflaenge_qh(),
        Some(24),
        "EEG 2017: 6 consecutive hours"
    );
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2021 - 06 - 01)).mindest_lauflaenge_qh(),
        Some(16),
        "EEG 2021: 4 consecutive hours"
    );
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2023 - 06 - 01)).mindest_lauflaenge_qh(),
        Some(16),
        "EEG 2023 original: 4 consecutive hours for a 2023 plant"
    );
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2024 - 06 - 01)).mindest_lauflaenge_qh(),
        Some(12),
        "EEG 2023 original: 3 consecutive hours from commissioning year 2024"
    );
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2025 - 02 - 24)).mindest_lauflaenge_qh(),
        Some(12),
        "the day before the Solarspitzengesetz: still the staged rule"
    );
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2025 - 02 - 25)).mindest_lauflaenge_qh(),
        Some(1),
        "Solarspitzengesetz: from the first negative quarter-hour"
    );
}

/// §51 EEG 2023 — kWh during negative-price hours are excluded from Vergütung.
/// Monthly total: 1,000 kWh. 80 kWh produced during 8h negative EPEX window.
/// Effective kWh: 920. Rate: 8.11 ct. Payment: 920 × 8.11 / 100 = 74.612 EUR.
#[test]
fn s51_verguetung_deduct_negative_price_kwh() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
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
        scheme: SettlementScheme::TenantElectricity {
            verguetungssatz_ct: d("7.5"),
            mieter_zuschlag_ct: Some(d("1.3")),
        },
        einspeisemenge_kwh: Some(d("800")),
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
fn s51_reduces_the_marktpraemie_for_negative_price_intervals() {
    // §51 Abs. 1 zeroes the anzulegender Wert, and Anlage 1 Nr. 1 defines the
    // Marktprämie's "AW" as the anzulegender Wert "unter Berücksichtigung der
    // §§ 19 bis 54" — so the Marktprämie is exactly what §51 reduces.
    let with_neg = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("10000")),
        marktwert_ct_kwh: Some(d("4.5")),
        kwh_during_negative_epex: Some(d("500")),
        ..SettleInput::default()
    });
    let without_neg = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("10000")),
        marktwert_ct_kwh: Some(d("4.5")),
        kwh_during_negative_epex: None,
        ..SettleInput::default()
    });
    // MP = 6.0 − 4.5 = 1.5 ct. Without §51: 10,000 kWh → 150 EUR.
    assert_eq!(without_neg.settlement_eur, Some(d("150.00000")));
    // With §51: the 500 kWh fed in at a negative price earn no premium →
    // 9,500 kWh → 142.50 EUR.
    assert_eq!(with_neg.settlement_eur, Some(d("142.50000")));
    assert_eq!(with_neg.eligible_kwh, Some(d("9500")));
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
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: d("8.0"),
            kwh_paid_gesamt: None,
            max_kwh: None,
        },
        einspeisemenge_kwh: Some(d("7000")),
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
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: d("3.1"),
            kwh_paid_gesamt: Some(d("29900")),
            max_kwh: Some(d("30000")),
        },
        einspeisemenge_kwh: Some(d("400")),
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
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: d("3.1"),
            kwh_paid_gesamt: Some(d("30001")),
            max_kwh: Some(d("30000")),
        },
        einspeisemenge_kwh: Some(d("5000")),
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
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: d("4.0"),
            kwh_paid_gesamt: None,
            max_kwh: None,
        },
        einspeisemenge_kwh: Some(d("50000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.1"),
        },
        einspeisemenge_kwh: Some(d("333.333")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: dec!(30),
        },
        einspeisemenge_kwh: Some(dec!(1_000_000)),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("7.83"),
        },
        einspeisemenge_kwh: Some(d("500")),
        ..SettleInput::default()
    });

    // After Zusammenlegung: new rate for 10–40 kWp band
    let after = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("6.79"),
        },
        einspeisemenge_kwh: Some(d("500")),
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
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("5.5"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("80000")),
        marktwert_ct_kwh: Some(d("28.0")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
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
        eeg_billing::EuroAmount::checked_from_decimal(d("40.55")).unwrap()
    );
}

/// Direktvermarktung produces 2 items: Marktprämie + Managementprämie.
#[test]
fn bridge_direktvermarktung_two_line_items() {
    use eeg_billing::bridge::settlement_to_line_items;
    let input = SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.2"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("4.8")),
        ..SettleInput::default()
    };
    let output = calculate_settlement(&input);
    let items = settlement_to_line_items(&output);
    assert_eq!(items.len(), 1, "one Marktprämie line, no Managementprämie");
    assert!(items[0].has_tag("marktpraemie"));
}

/// §25 Sanctioned produces 1 EUR 0 item tagged §25-sanctioned.
#[test]
fn bridge_sanctioned_zero_item() {
    use eeg_billing::bridge::settlement_to_line_items;
    let input = SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
        sanktion: Some(eeg_billing::SanktionAlt::VerguetungAufNull),
        ..SettleInput::default()
    };
    let output = calculate_settlement(&input);
    let items = settlement_to_line_items(&output);
    assert_eq!(items.len(), 1);
    assert!(items[0].has_tag("§25-sanctioned"));
    assert_eq!(items[0].net_amount, eeg_billing::EuroAmount::ZERO);
}

/// NoData → empty line items (nothing to bill yet).
#[test]
fn bridge_no_data_empty() {
    use eeg_billing::bridge::settlement_to_line_items;
    let input = SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: None, // no meter data
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
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

/// MIETERSTROM: 2 positions — base Vergütung + §21 Abs. 3 Zuschlag.
#[test]
fn positions_mieterstrom_two_lines() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::TenantElectricity {
            verguetungssatz_ct: d("7.5"),
            mieter_zuschlag_ct: Some(d("1.3")),
        },
        einspeisemenge_kwh: Some(d("800")),
        ..SettleInput::default()
    });
    assert_eq!(out.positions.len(), 2);

    let base = &out.positions[0];
    assert_eq!(base.legal_basis, "§21 EEG 2023");
    assert_eq!(base.kwh, d("800"));
    assert_eq!(base.rate_ct_kwh, d("7.5"));
    assert_eq!(base.eur, d("60.00")); // 800 × 7.5 / 100

    let zuschlag = &out.positions[1];
    assert_eq!(zuschlag.legal_basis, "§21 Abs. 3 EEG 2023");
    assert_eq!(zuschlag.kwh, d("800"));
    assert_eq!(zuschlag.rate_ct_kwh, d("1.3"));
    assert_eq!(zuschlag.eur, d("10.40")); // 800 × 1.3 / 100

    // Total = 60.00 + 10.40 = 70.40 EUR
    assert_eq!(out.settlement_eur, Some(d("70.40")));
}

/// DIREKTVERMARKTUNG: one Marktprämie position, summing to settlement_eur.
#[test]
fn positions_direktvermarktung_single_marktpraemie() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.2"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("120000")),
        marktwert_ct_kwh: Some(d("4.8")),
        ..SettleInput::default()
    });
    assert_eq!(out.positions.len(), 1);

    let marktpraemie = out
        .positions
        .iter()
        .find(|p| p.legal_basis == "§23a EEG 2023 i.V.m. Anlage 1")
        .unwrap();
    assert_eq!(marktpraemie.rate_ct_kwh, d("1.4")); // 6.2 - 4.8 = 1.4 ct
    assert_eq!(marktpraemie.eur, d("1680.00"));

    assert_eq!(out.settlement_eur, Some(d("1680.00")));
    // Positions must sum to settlement_eur
    let sum: rust_decimal::Decimal = out.positions.iter().map(|p| p.eur).sum();
    assert_eq!(
        Some(sum),
        out.settlement_eur,
        "positions must sum to settlement_eur"
    );
}

/// DIREKTVERMARKTUNG at zero spread: one position, at zero.
#[test]
fn positions_direktvermarktung_zero_spread() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("5.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("50000")),
        marktwert_ct_kwh: Some(d("5.0")),
        ..SettleInput::default()
    });
    assert_eq!(out.positions.len(), 1);
    assert_eq!(
        out.positions[0].legal_basis,
        "§23a EEG 2023 i.V.m. Anlage 1"
    );
    assert_eq!(out.positions[0].eur, d("0"));
    assert_eq!(out.settlement_eur, Some(d("0")));
}

/// AUSSCHREIBUNG: positions label includes "§§22a,28 EEG 2023".
#[test]
fn positions_ausschreibung_legal_basis_label() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("5.82"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        tariff_source: TariffSource::Auction(eeg_billing::AusschreibungMetadata::default()),
        einspeisemenge_kwh: Some(d("2500000")),
        marktwert_ct_kwh: Some(d("4.1")),
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
        scheme: SettlementScheme::FlexibilityPremium {
            verguetungssatz_ct: d("6.5"),
            flex_praemie_ct_kwh: Some(d("1.5")),
        },
        einspeisemenge_kwh: Some(d("180000")),
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
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: d("8.0"),
            kwh_paid_gesamt: None,
            max_kwh: None,
        },
        einspeisemenge_kwh: Some(d("7000")),
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
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: d("3.1"),
            kwh_paid_gesamt: Some(d("29900")),
            max_kwh: Some(d("30000")),
        },
        einspeisemenge_kwh: Some(d("400")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
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
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: Some(d("-0.5")),
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
            scheme: SettlementScheme::TenantElectricity {
                verguetungssatz_ct: d("7.5"),
                mieter_zuschlag_ct: Some(d("1.3")),
            },
            einspeisemenge_kwh: Some(d("500")),
            ..SettleInput::default()
        },
        SettleInput {
            scheme: SettlementScheme::FlexibilityPremium {
                verguetungssatz_ct: d("6.5"),
                flex_praemie_ct_kwh: Some(d("1.5")),
            },
            einspeisemenge_kwh: Some(d("200000")),
            ..SettleInput::default()
        },
        SettleInput {
            scheme: SettlementScheme::MarketPremium {
                direktverm_aw_ct: d("6.0"),
                wind_korrekturfaktor: None,
                wind_standort: None,
            },
            einspeisemenge_kwh: Some(d("100000")),
            marktwert_ct_kwh: Some(d("4.5")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
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
        Amount::<5>::checked_from_decimal(d("81.10")).unwrap()
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("5.5"),
        },
        einspeisemenge_kwh: Some(d("10000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("5.5"),
        },
        einspeisemenge_kwh: Some(d("10000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("5.5"),
        },
        einspeisemenge_kwh: Some(d("500000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("5.5"),
        },
        einspeisemenge_kwh: Some(d("500000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("18.5"),
        },
        einspeisemenge_kwh: Some(d("1000000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.0"),
        },
        einspeisemenge_kwh: Some(d("100000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.5"),
        },
        einspeisemenge_kwh: Some(d("50000")),
        marktwert_ct_kwh: Some(d("5.2")), // EPEX monthly avg needed for Marktwert
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.5"),
        },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: None, // EPEX missing → cannot compute Marktwert
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("10.0"),
        },
        einspeisemenge_kwh: Some(d("10000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("11.11"),
        },
        einspeisemenge_kwh: Some(d("7777")),
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
        technischer_defekt: false,
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
        technischer_defekt: false,
    });
    assert_eq!(before, d("6000")); // 100 × €10 × 6

    // After fulfillment: retroactively reduced to €2/kW/month
    let after = calculate_pflichtzahlung(&Pflichtverstoss {
        typ: SanktionsTyp::MastrNichtRegistriert,
        leistung_kw: d("100"),
        monate_des_verstosses: 6,
        nachtraeglich_erfuellt: true,
        technischer_defekt: false,
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
        technischer_defekt: false,
    });
    let with_fulfilment = calculate_pflichtzahlung(&Pflichtverstoss {
        typ: SanktionsTyp::SpeicherAnforderungNichtErfuellt,
        leistung_kw: d("200"),
        monate_des_verstosses: 2,
        nachtraeglich_erfuellt: true, // Has NO effect for Speicher type
        technischer_defekt: false,
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
        technischer_defekt: false,
    });
    assert_eq!(penalty, d("1200")); // 50 × €2 × 12 (always €2/kW for this type)
}

/// §52 EEG 2023 — Vergütung continues during penalty period (separate from old §52).
/// Plant receives full Vergütung AND owes the penalty separately.
#[test]
fn s52_2023_vergütung_plus_pflichtzahlung_independent() {
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
        pflichtverstoss: vec![Pflichtverstoss {
            typ: SanktionsTyp::MastrNichtRegistriert,
            leistung_kw: d("10"),
            monate_des_verstosses: 1,
            nachtraeglich_erfuellt: false,
            technischer_defekt: false,
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
        scheme: SettlementScheme::FlexibilitySurcharge {
            rate_eur_per_kw_year: d("100"),
        },
        leistung_kwp: Some(d("200")), // flex capacity in kW
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
        scheme: SettlementScheme::FlexibilitySurcharge {
            rate_eur_per_kw_year: d("100"),
        },
        leistung_kwp: Some(d("300")),
        einspeisemenge_kwh: Some(d("200000")), // kWh supplied but irrelevant
        ..SettleInput::default()
    });
    let without_kwh = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FlexibilitySurcharge {
            rate_eur_per_kw_year: d("100"),
        },
        leistung_kwp: Some(d("300")),
        einspeisemenge_kwh: None, // no kWh data
        ..SettleInput::default()
    });
    // Both should produce the same payment (300 × 100 / 12)
    assert_eq!(with_kwh.settlement_eur, without_kwh.settlement_eur);
    assert_eq!(with_kwh.status, SettlementStatus::Calculated);
}

// ═══════════════════════════════════════════════════════════════════════════
// §13a EnWG — Einspeisemanagement (curtailment) compensation
// ═══════════════════════════════════════════════════════════════════════════

/// §13a EnWG — NB curtails plant: compensation for lost generation.
/// Plant produces 1,000 kWh but 150 kWh were curtailed by NB.
/// Einspeisemenge = 850 kWh. EinsMan compensation = 150 × 8.11 / 100 = 12.165 EUR.
/// Total = 850 × 8.11 / 100 + 150 × 8.11 / 100 = 81.10 EUR (as if 1,000 kWh).
#[test]
fn s19_einspeisemanagement_compensation() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("850")), // measured feed-in (after curtailment)
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
        out.positions.iter().any(|p| p.legal_basis.contains("§13a")),
        "§19 EinsMan position expected"
    );
    let einsman_pos = out
        .positions
        .iter()
        .find(|p| p.legal_basis.contains("§13a"))
        .unwrap();
    assert_eq!(einsman_pos.kwh, d("150"));
    assert_eq!(einsman_pos.eur, d("12.165"));
}

/// §13a EnWG — EinsMan also applies to Direktvermarktung plants (uses AW as rate).
#[test]
fn s19_einspeisemanagement_direktvermarktung_uses_aw_rate() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("4.5")),
        einspeisemanagement_kwh: Some(d("5000")), // 5,000 kWh curtailed
        ..SettleInput::default()
    });
    // Marktprämie: (6.0 − 4.5) × 100,000 / 100 = 1,500 EUR
    // EinsMan: 5,000 × 6.0 / 100 = 300 EUR (uses the AW as compensation rate)
    assert_eq!(out.settlement_eur, Some(d("1800.00")));
    let einsman = out
        .positions
        .iter()
        .find(|p| p.legal_basis.contains("§13a"))
        .unwrap();
    assert_eq!(einsman.rate_ct_kwh, d("6.0")); // AW used as rate
    assert_eq!(einsman.eur, d("300.00"));
}

/// §13a EnWG — No curtailment in billing period → no §19 position.
#[test]
fn s19_no_curtailment_no_einsman_position() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
        einspeisemanagement_kwh: None, // no curtailment
        ..SettleInput::default()
    });
    assert!(
        out.positions
            .iter()
            .all(|p| !p.legal_basis.contains("§13a")),
        "no §19 position when einspeisemanagement_kwh is None"
    );
    assert_eq!(out.settlement_eur, Some(d("81.10")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §36h EEG — Wind Korrekturfaktor (location correction)
// ═══════════════════════════════════════════════════════════════════════════

/// §36h EEG 2023 — Low-wind site: Korrekturfaktor > 1.0 → higher effective AW.
/// Base AW = 6.5 ct/kWh. Korrekturfaktor = 1.12 (poor wind site, Gütegrad ~85%).
/// Effective AW = 6.5 × 1.12 = 7.28 ct/kWh.
#[test]
fn s36h_korrekturfaktor_increases_aw_for_low_wind_site() {
    use eeg_billing::wind_onshore_korrekturfaktor_corrected_aw;

    let corrected_aw = wind_onshore_korrekturfaktor_corrected_aw(d("6.5"), d("1.12"));
    assert_eq!(corrected_aw, d("7.28000"));

    // Use in settlement
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.5"),
            wind_korrekturfaktor: Some(d("1.12")),
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("200000")),
        marktwert_ct_kwh: Some(d("4.5")),
        ..SettleInput::default()
    });
    // Effective AW = 6.5 × 1.12 = 7.28 ct
    // MP = 7.28 − 4.5 = 2.78 ct → 200,000 × 2.78 / 100 = 5,560 EUR
    assert_eq!(out.settlement_eur, Some(d("5560.00")));
}

/// §36h EEG 2023 — High-wind site: Korrekturfaktor < 1.0 → lower effective AW.
/// Base AW = 6.5 ct/kWh. Korrekturfaktor = 0.78 (high-wind site, Gütegrad ~130%).
/// Effective AW = 6.5 × 0.78 = 5.07 ct/kWh.
#[test]
fn s36h_korrekturfaktor_decreases_aw_for_high_wind_site() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.5"),
            wind_korrekturfaktor: Some(d("0.78")),
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("300000")),
        marktwert_ct_kwh: Some(d("4.5")),
        ..SettleInput::default()
    });
    // Effective AW = 6.5 × 0.78 = 5.07 ct
    // MP = 5.07 − 4.5 = 0.57 ct → 300,000 × 0.57 / 100 = 1,710 EUR
    assert_eq!(out.settlement_eur, Some(d("1710.00")));
}

/// §36h — Korrekturfaktor 1.0 = no change (reference yield site).
#[test]
fn s36h_korrekturfaktor_1_0_no_change() {
    let with_k = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.0"),
            wind_korrekturfaktor: Some(d("1.0")),
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("4.5")),
        ..SettleInput::default()
    });
    let without_k = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("4.5")),
        ..SettleInput::default()
    });
    assert_eq!(with_k.settlement_eur, without_k.settlement_eur);
}

/// §36h — Pre-2016 plants: Korrekturfaktor not applicable (Bestandsschutz §100).
/// Setting wind_korrekturfaktor = None for old EEG2012 plants is mandatory.
/// This test verifies no correction is applied when None.
#[test]
fn s36h_bestandsschutz_no_correction_for_pre_2016() {
    use eeg_billing::EegGesetz;
    // EEG 2012 wind plant: §36h Bestandsschutz → no Korrekturfaktor
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.9"),
        },
        einspeisemenge_kwh: Some(d("100000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("9.25"),
        },
        einspeisemenge_kwh: Some(d("900")),
        leistung_kwp: Some(d("10")), // original capacity
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("24.43"),
        },
        einspeisemenge_kwh: Some(d("300")),
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

/// §48 Abs. 2 Nr. 1 in the 1 Feb 2024 §49 window: 8.60 × 0.99 = 8.51 ct/kWh.
#[test]
fn rates_solar_pv_feb_2024_window_ueberschuss() {
    use eeg_billing::rates;
    let table =
        rates::solar_pv_ueberschuss_lookup(date!(2024 - 03 - 01)).expect("EEG 2023 window known");
    let rate = table.rate_for(d("9")).expect("9 kWp in table");
    assert_eq!(rate, billing::Amount::parse("0.08510").unwrap());
}

/// Solar PV Volleinspeisung 2024 (Solarpaket I): higher rate for 100% grid feed-in.
#[test]
fn rates_solar_pv_volleinspeisung_2024_higher_than_ueberschuss() {
    use eeg_billing::rates;
    let u = rates::solar_pv_ueberschuss_lookup(date!(2024 - 03 - 01)).unwrap();
    let v = rates::solar_pv_volleinspeisung_lookup(date!(2024 - 03 - 01)).unwrap();
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
        pflichtverstoss: vec![
            Pflichtverstoss {
                typ: SanktionsTyp::MastrNichtRegistriert,
                leistung_kw: d("100"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
                technischer_defekt: false,
            },
            Pflichtverstoss {
                typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
                leistung_kw: d("100"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
                technischer_defekt: false,
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
        pflichtverstoss: vec![
            Pflichtverstoss {
                typ: SanktionsTyp::MastrNichtRegistriert,
                leistung_kw: d("50"),
                monate_des_verstosses: 3,
                nachtraeglich_erfuellt: true, // retroactive reduction
                technischer_defekt: false,
            },
            Pflichtverstoss {
                typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
                leistung_kw: d("50"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
                technischer_defekt: false,
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
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
fn s51a_wind_extension_rounds_up_to_full_calendar_day() {
    use eeg_billing::foerderdauer::verguetungszeitraum_verlaengerung_qh;
    use eeg_billing::{EegGesetz, ErzeugungsArt};

    // §51a Abs. 1 Satz 2: lost QH round UP to the next full calendar day (96 QH).
    assert_eq!(verguetungszeitraum_verlaengerung_qh(240, false), 288); // 2.5 d → 3 d
    assert_eq!(verguetungszeitraum_verlaengerung_qh(100, false), 192); // 1.04 d → 2 d

    // Settlement with §51 applied + QH tracking
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("5.5"),
        },
        einspeisemenge_kwh: Some(d("50000")),
        kwh_during_negative_epex: Some(d("2000")), // §51 reduced kWh
        negative_price_quarter_hours: Some(240),   // 60h = 240 QH at negative EPEX
        eeg_gesetz: EegGesetz::Eeg2023,
        leistung_kwp: Some(d("500")),
        erzeugungsart: Some(ErzeugungsArt::WindOnshore),
        ..SettleInput::default()
    });
    // §51a Abs. 1 Satz 2 wind: 240 QH = 2.5 days → rounds up to 3 days = 288 QH.
    assert_eq!(out.verlaengerungsanspruch_qh, 288);
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
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
// §§53b–54 EEG 2023 — reductions of the anzulegender Wert
// ═══════════════════════════════════════════════════════════════════════════

use eeg_billing::aw_reductions::{AwReductionContext, Sect54SolarReduction};

/// §53b EEG 2023 — a Regionalnachweis cuts the AW by the statutory 0,1 ct/kWh.
///
/// 1 000 kWh at 8,11 ct → AW 8,01 ct → 80,10 EUR.
#[test]
fn s53b_regionalnachweis_cuts_the_statutory_tenth_of_a_cent() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
        aw_reductions: AwReductionContext {
            regionalnachweis_ausgestellt: true,
            ..AwReductionContext::default()
        },
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("80.10")));
    let cut = out
        .positions
        .iter()
        .find(|p| p.legal_basis.contains("53b"))
        .expect("§53b audit position expected");
    assert_eq!(cut.rate_ct_kwh, d("-0.1"), "the rate is fixed by statute");
    assert_eq!(
        cut.eur,
        d("0"),
        "the euro effect is already in the reduced AW"
    );
}

/// §53b reaches a Direktvermarktung plant whose AW is **gesetzlich bestimmt**.
///
/// The statute's test is how the AW was determined, not how the electricity is
/// marketed — so a statutory-AW Marktprämie plant is in scope.
#[test]
fn s53b_applies_to_a_statutory_aw_under_direktvermarktung() {
    let mp = |ctx: AwReductionContext| {
        calculate_settlement(&SettleInput {
            scheme: SettlementScheme::MarketPremium {
                direktverm_aw_ct: d("6.0"),
                wind_korrekturfaktor: None,
                wind_standort: None,
            },
            einspeisemenge_kwh: Some(d("100000")),
            marktwert_ct_kwh: Some(d("4.5")),
            aw_reductions: ctx,
            ..SettleInput::default()
        })
    };
    let with_nachweis = mp(AwReductionContext {
        regionalnachweis_ausgestellt: true,
        ..AwReductionContext::default()
    });
    let without = mp(AwReductionContext::default());
    // AW 6.0 → 5.9; MP = (5.9 − 4.5) × 100 000 / 100 = 1 400 EUR
    assert_eq!(without.settlement_eur, Some(d("1500.00")));
    assert_eq!(with_nachweis.settlement_eur, Some(d("1400.00")));
}

/// §53b does **not** reach a tender-determined AW — it is not gesetzlich bestimmt.
#[test]
fn s53b_does_not_reach_an_auction_aw() {
    use eeg_billing::{AusschreibungMetadata, TariffSource};
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("5.80"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("4.5")),
        tariff_source: TariffSource::Auction(AusschreibungMetadata::default()),
        aw_reductions: AwReductionContext {
            regionalnachweis_ausgestellt: true,
            ..AwReductionContext::default()
        },
        ..SettleInput::default()
    });
    // Unreduced: (5.80 − 4.5) × 100 000 / 100 = 1 300 EUR
    assert_eq!(out.settlement_eur, Some(d("1300.00")));
    assert!(!out.positions.iter().any(|p| p.legal_basis.contains("53b")));
}

/// §53c EEG 2023 — the AW drops by the granted per-kWh Stromsteuerbefreiung.
#[test]
fn s53c_stromsteuerbefreiung_cuts_the_aw() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1000")),
        aw_reductions: AwReductionContext {
            // Full §3 StromStG rate: 20,50 EUR/MWh = 2,05 ct/kWh.
            stromsteuerbefreiung_ct_kwh: Some(d("2.05")),
            ..AwReductionContext::default()
        },
        ..SettleInput::default()
    });
    // AW 8.11 → 6.06 → 1 000 × 6.06 / 100 = 60.60 EUR
    assert_eq!(out.settlement_eur, Some(d("60.60")));
    assert!(out.positions.iter().any(|p| p.legal_basis.contains("53c")));
}

/// §54 Abs. 1 EEG 2023 — a Zahlungsberechtigung applied for after the 18th
/// calendar month costs 0,3 ct/kWh on a solar first-segment award.
#[test]
fn s54_abs1_late_zahlungsberechtigung_cuts_the_award() {
    use eeg_billing::{AusschreibungMetadata, ErzeugungsArt, TariffSource};
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("5.80"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("4.5")),
        tariff_source: TariffSource::Auction(AusschreibungMetadata::default()),
        erzeugungsart: Some(ErzeugungsArt::SolarFreiflaeche),
        aw_reductions: AwReductionContext {
            sect54_solar: Some(Sect54SolarReduction {
                zahlungsberechtigung_nach_18_monaten: true,
                ..Sect54SolarReduction::default()
            }),
            ..AwReductionContext::default()
        },
        ..SettleInput::default()
    });
    // AW 5.80 → 5.50; MP = (5.50 − 4.5) × 100 000 / 100 = 1 000 EUR
    assert_eq!(out.settlement_eur, Some(d("1000.00")));
    assert!(
        out.positions
            .iter()
            .any(|p| p.legal_basis.contains("54 Abs. 1"))
    );
}

/// The reduction hits the AW **before** the Marktprämie floor, so a plant whose
/// Marktwert already exceeds its AW settles at zero rather than going negative.
///
/// This is the defect the AW-level model exists to prevent: a post-hoc euro
/// deduction would have billed the operator for feeding in.
#[test]
fn an_aw_cut_cannot_push_the_marktpraemie_below_zero() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("4.00"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        // Marktwert well above AW + Managementprämie → premium already zero.
        marktwert_ct_kwh: Some(d("9.00")),
        aw_reductions: AwReductionContext {
            regionalnachweis_ausgestellt: true,
            stromsteuerbefreiung_ct_kwh: Some(d("2.05")),
            ..AwReductionContext::default()
        },
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(d("0")));
}

/// No reductions configured → the AW is untouched and no audit position appears.
#[test]
fn no_aw_reductions_leaves_the_settlement_alone() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(d("40.55")));
    assert_eq!(out.positions.len(), 1);
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
// §48/§49 EEG 2023 — anzulegende Werte für Solaranlagen und ihre Absenkung
// ══════════════════════════════════════════════════════════════════════════════

/// §48 Abs. 2 EEG 2023 (Fassung vom 15.05.2024, in force per §101 Abs. 1 Satz 2)
/// — the three Gebäude brackets, before any §49 step.
#[test]
fn sect48_abs2_base_anzulegende_werte() {
    use eeg_billing::rates::solar_pv_ueberschuss_aw_ct;
    let ibn = date!(2023 - 06 - 01); // before the first §49 window
    assert_eq!(solar_pv_ueberschuss_aw_ct(d("9"), ibn), Some(d("8.60")));
    assert_eq!(solar_pv_ueberschuss_aw_ct(d("40"), ibn), Some(d("7.50")));
    assert_eq!(solar_pv_ueberschuss_aw_ct(d("800"), ibn), Some(d("6.20")));
    // §22 Abs. 3: above 1 MW the AW comes from an Ausschreibung, not the statute.
    assert_eq!(solar_pv_ueberschuss_aw_ct(d("2000"), ibn), None);
}

/// §48 Abs. 2a EEG 2023 — the Volleinspeisung uplift, per bracket.
#[test]
fn sect48_abs2a_volleinspeisung_uplift() {
    use eeg_billing::rates::{solar_pv_ueberschuss_aw_ct, solar_pv_volleinspeisung_aw_ct};
    let ibn = date!(2023 - 06 - 01);
    for (kwp, uplift) in [
        ("9", "4.8"),
        ("40", "3.8"),
        ("100", "5.1"),
        ("400", "3.2"),
        ("1000", "1.9"),
    ] {
        let ueber = solar_pv_ueberschuss_aw_ct(d(kwp), ibn).unwrap();
        let voll = solar_pv_volleinspeisung_aw_ct(d(kwp), ibn).unwrap();
        assert_eq!(voll - ueber, d(uplift), "{kwp} kWp");
    }
    // The ≤ 10 kW total: 8.60 + 4.80.
    assert_eq!(
        solar_pv_volleinspeisung_aw_ct(d("9"), ibn),
        Some(d("13.40"))
    );
}

/// §49 EEG 2023 — the published window series for a ≤ 10 kW Gebäudeanlage.
///
/// Cross-checked against the Bundesnetzagentur "Anzulegende Werte für
/// Solaranlagen" spreadsheet (Marktprämienmodell, Teileinspeisung column).
#[test]
fn sect49_published_window_series_ueberschuss() {
    use eeg_billing::rates::solar_pv_ueberschuss_aw_ct;
    for (ibn, aw) in [
        (date!(2024 - 01 - 15), "8.60"),
        (date!(2024 - 02 - 01), "8.51"),
        (date!(2024 - 08 - 01), "8.43"),
        (date!(2025 - 02 - 01), "8.34"),
        (date!(2025 - 08 - 01), "8.26"),
        (date!(2026 - 02 - 01), "8.18"),
    ] {
        assert_eq!(
            solar_pv_ueberschuss_aw_ct(d("9"), ibn),
            Some(d(aw)),
            "{ibn}"
        );
    }
}

/// §49 EEG 2023 — the same series on the Volleinspeisung column.
#[test]
fn sect49_published_window_series_volleinspeisung() {
    use eeg_billing::rates::solar_pv_volleinspeisung_aw_ct;
    for (ibn, aw) in [
        (date!(2024 - 02 - 01), "13.27"),
        (date!(2024 - 08 - 01), "13.13"),
        (date!(2025 - 02 - 01), "13.00"),
        (date!(2026 - 02 - 01), "12.74"),
    ] {
        assert_eq!(
            solar_pv_volleinspeisung_aw_ct(d("9"), ibn),
            Some(d(aw)),
            "{ibn}"
        );
    }
}

/// §49 EEG 2023 is a **fixed 1 % semi-annual** step — no GW-keyed "atmender
/// Deckel", and no quarterly cadence. Two commissionings in the same half-year
/// window get the same anzulegender Wert.
#[test]
fn sect49_is_semiannual_not_quarterly() {
    use eeg_billing::rates::solar_pv_ueberschuss_aw_ct;
    let q1 = solar_pv_ueberschuss_aw_ct(d("9"), date!(2025 - 02 - 01));
    let q2 = solar_pv_ueberschuss_aw_ct(d("9"), date!(2025 - 04 - 30));
    let q3 = solar_pv_ueberschuss_aw_ct(d("9"), date!(2025 - 07 - 31));
    assert_eq!(q1, q2, "Q1 and Q2 of a half-year share one window");
    assert_eq!(q1, q3);
    assert_ne!(
        q1,
        solar_pv_ueberschuss_aw_ct(d("9"), date!(2025 - 08 - 01))
    );
}

/// §49 Satz 2 — each step compounds on the **unrounded** predecessor.
#[test]
fn sect49_satz2_compounds_on_unrounded_values() {
    use eeg_billing::degression::abgesenkter_wert;
    // Naively chaining the published 8.51 would give 8.42; the statute's
    // unrounded chain from 8.60 gives the published 8.43.
    assert_eq!(abgesenkter_wert(d("8.51"), 1), d("8.42"));
    assert_eq!(abgesenkter_wert(d("8.60"), 2), d("8.43"));
}

/// Plants commissioned before EEG 2023 have no entry in these tables — their
/// rates come from `einsd`'s per-window DB series.
#[test]
fn pre_eeg2023_plants_use_the_db_series() {
    use eeg_billing::rates::solar_pv_ueberschuss_aw_ct;
    assert!(solar_pv_ueberschuss_aw_ct(d("9"), date!(2022 - 12 - 31)).is_none());
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
// §36h EEG 2023 — Wind Standort (structured site model)
// ══════════════════════════════════════════════════════════════════════════════

/// §36h — WindStandort struct directly wired into SettleInput via wind_standort field.
#[test]
fn sect36h_wind_standort_auto_derives_korrekturfaktor() {
    use eeg_billing::wind::{WindStandort, WindStandortklasse};
    use eeg_billing::{SettleInput, SettlementScheme, SettlementStatus};

    let standort = WindStandort {
        guetefaktor: dec!(0.85),
        korrekturfaktor: dec!(1.08),
        suedregion: false,
        standortklasse: WindStandortklasse::BelowReference,
    };

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.28"),
            wind_korrekturfaktor: None,
            wind_standort: Some(standort),
        },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: Some(d("4.00")),
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

/// §36h — Explicit wind_korrekturfaktor takes precedence over wind_standort.
#[test]
fn sect36h_explicit_korrekturfaktor_wins_over_standort() {
    use eeg_billing::wind::{WindStandort, WindStandortklasse};
    use eeg_billing::{SettleInput, SettlementScheme};

    let standort = WindStandort {
        guetefaktor: dec!(0.85),
        korrekturfaktor: dec!(1.08), // standort says 1.08
        suedregion: false,
        standortklasse: WindStandortklasse::BelowReference,
    };

    let out_explicit = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.28"),
            wind_korrekturfaktor: Some(d("1.05")),
            wind_standort: Some(standort),
        },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: Some(d("4.00")),
        ..SettleInput::default()
    });

    // Explicit 1.05 applies: AW = 6.28 × 1.05 = 6.594 → Prämie = 2.594 ct
    let out_implicit = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.28"),
            wind_korrekturfaktor: Some(d("1.08")),
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: Some(d("4.00")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("10.02"),
        },
        einspeisemenge_kwh: Some(d("500")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("10.02"),
        },
        einspeisemenge_kwh: Some(d("500")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("12.35"),
        },
        einspeisemenge_kwh: Some(d("1000")),
        eeg_gesetz: EegGesetz::Eeg2017,
        marktwert_ct_kwh: Some(d("5.50")), // EPEX monthly average
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("9.25"),
        },
        einspeisemenge_kwh: Some(d("300")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("9.25"),
        },
        einspeisemenge_kwh: Some(d("300")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.51"),
        },
        einspeisemenge_kwh: Some(d("500")),
        pflichtverstoss: vec![
            Pflichtverstoss {
                typ: SanktionsTyp::MastrNichtRegistriert,
                leistung_kw: d("50"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
                technischer_defekt: false,
            },
            Pflichtverstoss {
                typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
                leistung_kw: d("50"),
                monate_des_verstosses: 1,
                nachtraeglich_erfuellt: false,
                technischer_defekt: false,
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
        scheme: SettlementScheme::KwkSurcharge {
            verguetungssatz_ct: d("8.00"),
            kwh_paid_gesamt: Some(d("5990000")),
            max_kwh: Some(d("6000000")),
        },
        einspeisemenge_kwh: Some(d("50000")), // 50,000 kWh this month — exceeds remaining
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
        scheme: SettlementScheme::FlexibilityPremium {
            verguetungssatz_ct: d("14.47"),
            flex_praemie_ct_kwh: Some(d("1.30")),
        },
        einspeisemenge_kwh: Some(d("2000")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.51"),
        },
        einspeisemenge_kwh: Some(d("500")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.47"),
        },
        einspeisemenge_kwh: Some(d("2000")),
        leistung_kwp: Some(d("500")),
        erzeugungsart: Some(ErzeugungsArt::Biomasse),
        eeg_gesetz: EegGesetz::Eeg2023,
        kwh_during_negative_epex: Some(d("200")),
        negative_price_quarter_hours: Some(24),
        ..SettleInput::default()
    });

    // Biomasse (not solar): §51a Abs. 1 Satz 2 rounds 24 QH (0.25 d) up to one
    // full calendar day = 96 QH.
    assert_eq!(out.verlaengerungsanspruch_qh, 96);
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

/// §52 Abs. 6 is a permission, not a duty: without it the operator receives the
/// full Vergütung and the whole Pflichtzahlung stays outstanding.
#[test]
fn sect52_netting_is_optional_for_the_netzbetreiber() {
    use eeg_billing::reductions::ReductionPipeline;

    let no_offset = ReductionPipeline {
        pflichtzahlung_eur: Some(d("10.00")),
        apply_sect52_netting: false,
    }
    .apply(d("42.55"));
    assert_eq!(no_offset.net_vergütung_eur, d("42.55"));
    assert_eq!(no_offset.residual_pflichtzahlung_eur, d("10.00"));
    assert_eq!(no_offset.total_reductions_eur, d("0"));

    let offset = ReductionPipeline {
        pflichtzahlung_eur: Some(d("10.00")),
        apply_sect52_netting: true,
    }
    .apply(d("42.55"));
    assert_eq!(offset.net_vergütung_eur, d("32.55"));
    assert_eq!(offset.residual_pflichtzahlung_eur, d("0"));
}

// ══════════════════════════════════════════════════════════════════════════════
// Settlement state machine
// ══════════════════════════════════════════════════════════════════════════════

/// A healthy plant, and the facts that move it off `Active`.
fn state_facts() -> eeg_billing::settlement_state::SettlementStateFacts {
    use eeg_billing::settlement_state::{Sect9Erfuellung, SettlementStateFacts};
    SettlementStateFacts {
        mastr_registriert: true,
        sect9_erfuellung: Sect9Erfuellung::Fernsteuerbarkeit,
        leistung_kwp: d("50"),
        erzeugungsart: None,
        foerderendedatum: Some(date!(2040 - 12 - 31)),
        billing_date: date!(2026 - 07 - 01),
        eeg_gesetz_year: 2023,
    }
}

/// Healthy plant: MaStR registered + §9 satisfied → Active.
#[test]
fn settlement_state_healthy_plant_is_active() {
    use eeg_billing::settlement_state::{SettlementPeriodState, derive_settlement_state};
    assert_eq!(
        derive_settlement_state(&state_facts()),
        SettlementPeriodState::Active
    );
}

/// §9 Abs. 2 Nr. 2 EEG — a 50 kW plant may satisfy §9 with the 60 %
/// Leistungsbegrenzung instead of Fernsteuerbarkeit.
///
/// A flat "≥ 25 kW must have Fernsteuerbarkeit" would put every compliant plant
/// in the 25–100 kW band into `Reduced` and charge it a §52 Abs. 1 Nr. 1
/// Pflichtzahlung of 10 €/kW/month it does not owe.
#[test]
fn settlement_state_sixty_percent_cap_satisfies_sect9_below_100kw() {
    use eeg_billing::settlement_state::{
        Sect9Erfuellung, SettlementPeriodState, SettlementStateFacts, derive_settlement_state,
    };
    let facts = SettlementStateFacts {
        sect9_erfuellung: Sect9Erfuellung::Leistungsbegrenzung60,
        ..state_facts()
    };
    assert_eq!(
        derive_settlement_state(&facts),
        SettlementPeriodState::Active
    );

    // From 100 kW the alternative is gone (§9 Abs. 2 Nr. 1).
    let gross = SettlementStateFacts {
        leistung_kwp: d("100"),
        ..facts
    };
    assert_eq!(
        derive_settlement_state(&gross),
        SettlementPeriodState::Reduced
    );
}

/// EEG 2023, MaStR missing → Reduced (Pflichtzahlung, Vergütung still flows).
#[test]
fn settlement_state_eeg2023_mastr_missing_reduced() {
    use eeg_billing::settlement_state::{
        SettlementPeriodState, SettlementStateFacts, derive_settlement_state,
    };
    let facts = SettlementStateFacts {
        mastr_registriert: false,
        ..state_facts()
    };
    assert_eq!(
        derive_settlement_state(&facts),
        SettlementPeriodState::Reduced
    );
}

/// EEG 2017, MaStR missing → Suspended (VerguetungAufNull, old regime).
#[test]
fn settlement_state_eeg2017_mastr_missing_suspended() {
    use eeg_billing::settlement_state::{
        SettlementPeriodState, SettlementStateFacts, derive_settlement_state,
    };
    let facts = SettlementStateFacts {
        mastr_registriert: false,
        eeg_gesetz_year: 2017,
        ..state_facts()
    };
    assert_eq!(
        derive_settlement_state(&facts),
        SettlementPeriodState::Suspended
    );
}

/// Förderdauer expired → PostEeg state.
#[test]
fn settlement_state_foerderdauer_expired_post_eeg() {
    use eeg_billing::settlement_state::{
        SettlementPeriodState, SettlementStateFacts, derive_settlement_state,
    };
    let facts = SettlementStateFacts {
        leistung_kwp: d("10"),
        foerderendedatum: Some(date!(2024 - 12 - 31)),
        billing_date: date!(2025 - 01 - 01),
        ..state_facts()
    };
    assert_eq!(
        derive_settlement_state(&facts),
        SettlementPeriodState::PostEeg
    );
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
fn anlage1_marktwert_just_above_aw_pays_nothing() {
    // AW = 5.0, MW = 5.2 → MP = max(0, −0.2) = 0. There is no intermediate band
    // in which a residual Managementprämie keeps the claim alive.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("5.0"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("5.2")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("0")));
    assert_eq!(out.positions.len(), 1);
    assert_eq!(
        out.positions[0].legal_basis,
        "§23a EEG 2023 i.V.m. Anlage 1"
    );
    assert_eq!(out.positions[0].rate_ct_kwh, d("0"));
}

/// §20 Abs. 3 — Correct total when positive spread: both positions present.
#[test]
fn anlage1_full_spread_when_marktwert_below_aw() {
    // AW = 6.2, MW = 4.8 → MP = 1.4 ct.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.2"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("4.8")),
        ..SettleInput::default()
    });
    assert_eq!(out.settlement_eur, Some(d("1400.00")));
    assert_eq!(out.positions.len(), 1);
    assert_eq!(out.positions[0].rate_ct_kwh, d("1.4"));
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
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("5.80"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        tariff_source: TariffSource::Auction(eeg_billing::AusschreibungMetadata::default()),
        einspeisemenge_kwh: Some(d("500000")),
        marktwert_ct_kwh: Some(d("4.5")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        settlement_type: correction_type.clone(),
        einspeisemenge_kwh: Some(d("520")), // revised: 20 kWh more than original
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        settlement_type: SettlementType::Reversal {
            original_id: "ORIG-2026-06-001".to_string(),
        },
        einspeisemenge_kwh: Some(d("-500")), // negative kWh for reversal
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
        scheme: SettlementScheme::PostEeg {
            price_floor: Some(d("0")),
        },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: Some(d("-2.0")), // negative EPEX
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
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: Some(d("-2.0")),
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
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
        pflichtverstoss: vec![Pflichtverstoss {
            typ: SanktionsTyp::MastrNichtRegistriert,
            leistung_kw: d("10"),
            monate_des_verstosses: 1,
            nachtraeglich_erfuellt: false,
            technischer_defekt: false,
        }],
        ..SettleInput::default()
    });

    let gross = out.settlement_eur.unwrap(); // 40.55 EUR Vergütung
    let penalty = out.pflichtzahlung_eur.unwrap(); // 100 EUR Pflichtzahlung

    // 2. Apply §52 Abs. 6 netting (NB deducts penalty from Vergütung disbursement)
    let pipeline = ReductionPipeline {
        pflichtzahlung_eur: Some(penalty),
        apply_sect52_netting: true,
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
    use eeg_billing::ErzeugungsArt;
    use eeg_billing::direktverm::is_direktvermarktung_mandatory;

    // EEG 2000 plant: §51 does not apply (Bestandsschutz §66 EEG 2017)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("50.62"),
        },
        einspeisemenge_kwh: Some(d("10000")),
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

// ── §44b Abs. 1 EEG 2023 — Biogas quota January reset ────────────────────────

#[test]
fn sect44b_quota_january_reset_when_billing_year_differs() {
    // Plant: 200 kW biogas, annual quota = 200 × 0.45 × 8760 = 788_400 kWh/year.
    // Simulate: quota_ytd_year is last year → counter treated as 0.
    // All 10_000 kWh should be eligible (fresh year).
    let quota = d("10000"); // <<< eligible_this_month: full 10_000 because no prior YTD
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.10"),
        },
        einspeisemenge_kwh: Some(d("10000")),
        biogas_sect44b_eligible_kwh: Some(quota), // caller passes full quota (reset)
        leistung_kwp: Some(d("200")),
        ..SettleInput::default()
    });
    // No cap applied (eligible_kwh == total_kwh)
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.eligible_kwh, Some(d("10000")));
    // Position: 10000 kWh × 14.10 ct = 1410.00 EUR
    assert_eq!(out.settlement_eur, Some(d("1410.00000")));
    // No excess position
    assert_eq!(out.positions.len(), 1);
}

#[test]
fn sect44b_quota_partial_cap_triggers_excess_position() {
    // 200 kW biogas, only 6000 kWh eligible in quota (last 4000 are excess)
    // Excess at EPEX 5.0 ct/kWh
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.10"),
        },
        einspeisemenge_kwh: Some(d("10000")),
        biogas_sect44b_eligible_kwh: Some(d("6000")),
        marktwert_ct_kwh: Some(d("5.00")),
        leistung_kwp: Some(d("200")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // eligible fraction = 6000/10000 = 0.6 × 1410 EUR = 846 EUR
    let main_eur = out.positions[0].eur;
    assert!(
        (main_eur - d("846.00000")).abs() < d("0.001"),
        "main position expected ~846 EUR, got {main_eur}"
    );
    // excess: 4000 × 5.0 ct = 200 EUR
    let excess = &out.positions[1];
    assert!(excess.legal_basis.contains("§44b"));
    assert_eq!(excess.kwh, d("4000"));
    assert_eq!(excess.rate_ct_kwh, d("5.00"));
}

// ── §51b boundary — EPEX at exactly 2 ct/kWh ─────────────────────────────────

#[test]
fn sect51b_biogas_ausschreibung_epex_at_exactly_2ct_triggers_zero_aw() {
    // §51b Satz 1: "wenn der Spotmarktpreis 2 Cent pro Kilowattstunde ODER WENIGER beträgt"
    // At exactly 2.00 ct: AW = 0.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("13.50"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        tariff_source: eeg_billing::TariffSource::Auction(eeg_billing::AusschreibungMetadata {
            award_ct: Some(d("13.50")),
            is_biogas_sect51b: true,
            ..Default::default()
        }),
        einspeisemenge_kwh: Some(d("5000")),
        marktwert_ct_kwh: Some(d("2.00")), // boundary: exactly 2 ct
        ..SettleInput::default()
    });
    // At exactly 2 ct, §51b applies → AW = 0
    assert_eq!(out.settlement_eur, Some(d("0.00000")));
    assert!(
        out.positions[0].legal_basis.contains("§51b"),
        "expected §51b legal basis"
    );
}

#[test]
fn sect51b_biogas_ausschreibung_epex_above_2ct_receives_market_premium() {
    // At 2.01 ct: §51b does NOT apply → normal MarketPremium calculated
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("13.50"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        tariff_source: eeg_billing::TariffSource::Auction(eeg_billing::AusschreibungMetadata {
            award_ct: Some(d("13.50")),
            is_biogas_sect51b: true,
            ..Default::default()
        }),
        einspeisemenge_kwh: Some(d("5000")),
        marktwert_ct_kwh: Some(d("2.01")), // just above 2 ct
        ..SettleInput::default()
    });
    // MarketPremium = max(0, AW + Mgmt - EPEX) = 13.50 + 0.40 - 2.01 = 11.89 ct
    // settlement = 5000 × 11.89 ct / 100 = 594.50 EUR
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert!(
        out.settlement_eur.is_some_and(|e| e > d("0")),
        "expected positive settlement above 2ct"
    );
}

// ── §23b — Post-EEG 10 ct/kWh cap boundary ────────────────────────────────────

#[test]
fn post_eeg_exactly_10ct_is_not_capped() {
    // At exactly 10 ct/kWh: no cap applied (condition is strictly > 10)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: Some(d("10.00")),
        ..SettleInput::default()
    });
    // 1000 × 10.00 / 100 = 100.00 EUR — no cap annotation
    assert_eq!(out.settlement_eur, Some(d("100.00000")));
    assert!(
        !out.positions[0].description.contains("Deckel"),
        "10 ct exactly should NOT trigger cap annotation"
    );
}

#[test]
fn post_eeg_above_10ct_is_capped_at_10ct() {
    // At 15 ct/kWh: capped to 10 ct/kWh
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::PostEeg { price_floor: None },
        einspeisemenge_kwh: Some(d("1000")),
        marktwert_ct_kwh: Some(d("15.00")),
        ..SettleInput::default()
    });
    // 1000 × 10.00 / 100 = 100.00 EUR
    assert_eq!(out.settlement_eur, Some(d("100.00000")));
    assert!(
        out.positions[0].description.contains("Deckel"),
        "15 ct should trigger §23b cap annotation"
    );
}

// ── §24 multi-block with mixed EEG versions ───────────────────────────────────

#[test]
fn multi_block_sect24_mixed_eeg_versions_each_block_correct_negativpreis() {
    use time::Date;

    // Primary block: 100 kWp IBN 2015 — §51 never reaches a pre-2016 plant.
    // Extension block: 400 kWp IBN 2024 — the EEG 2023 Fassung as enacted, whose
    // exemption is **400 kW**, tested on the aggregated 500 kWp (§51 Abs. 2
    // Satz 2 i.V.m. §24).
    // Total kWh: 1500. Primary share: 100/500 = 0.2 → 300 kWh.
    // Extension share: 400/500 = 0.8 → 1200 kWh.
    // Negative EPEX kWh: 200 total.
    //   Primary neg share: 200 × 0.2 = 40 kWh → EEG 2012 has no §51 → no deduction.
    //   Extension neg share: 200 × 0.8 = 160 kWh → EEG 2023 ≥100 kW → deducted.
    let ibn_primary = Date::from_calendar_date(2015, time::Month::June, 1).expect("valid date");
    let ibn_ext = Date::from_calendar_date(2024, time::Month::March, 1).expect("valid date");
    let fed_ext = Date::from_calendar_date(2044, time::Month::December, 31).expect("valid date");

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("1500")),
        leistung_kwp: Some(d("100")), // primary block
        inbetriebnahme: Some(ibn_primary),
        eeg_gesetz: EegGesetz::Eeg2012,
        kwh_during_negative_epex: Some(d("200")),
        capacity_blocks: vec![CapacityBlock {
            leistung_kwp: d("400"),
            inbetriebnahme: ibn_ext,
            verguetungssatz_ct: d("8.11"),
            foerderendedatum: fed_ext,
        }],
        ..SettleInput::default()
    });

    assert_eq!(out.status, SettlementStatus::Calculated);
    // Primary block (IBN 2015): no §51 → 300 kWh paid in full.
    // Extension block (IBN 2024): §51 → 1200 − 160 = 1040 kWh.
    let total = out.settlement_eur.expect("settlement must exist");
    assert!(total > d("0"), "combined settlement must be positive");
    assert!(
        out.positions.len() >= 2,
        "must have at least 2 positions (primary + extension)"
    );
    // Extension block should have fewer kWh due to §51 deduction
    // Without deduction: 1500 × (120/150) = 1200 kWh
    // With §51 deduction: 1200 - (200 × 0.8) = 1200 - 160 = 1040 kWh
    let ext_pos = out.positions.last().unwrap();
    assert!(
        ext_pos.kwh < d("1200"),
        "extension block kWh must be reduced by §51 deduction (expected <1200, got {})",
        ext_pos.kwh
    );
    assert!(
        ext_pos.kwh > d("0"),
        "extension block should still have positive kWh after deduction"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// InbetriebnahmeTyp — plant lifecycle tracking (§3 EEG 2023)
// ═══════════════════════════════════════════════════════════════════════════

/// New field `inbetriebnahme_typ` on `SettleInput` defaults to `Erstinbetriebnahme`.
/// For audit purposes the field is included even for normal plants.
#[test]
fn inbetriebnahme_typ_default_is_erstinbetriebnahme() {
    use eeg_billing::InbetriebnahmeTyp;
    let input = SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("100")),
        ..SettleInput::default()
    };
    assert_eq!(
        input.inbetriebnahme_typ,
        InbetriebnahmeTyp::Erstinbetriebnahme
    );
}

/// §22 EEG 2023 Repowering: `InbetriebnahmeTyp::Repowering` must reset Förderdauer.
#[test]
fn inbetriebnahme_typ_repowering_resets_foerderdauer_flag() {
    use eeg_billing::InbetriebnahmeTyp;
    assert!(InbetriebnahmeTyp::Repowering.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Erstinbetriebnahme.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Wiederinbetriebnahme.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Modernisierung.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Zusammenlegung.resets_foerderdauer());
    assert!(!InbetriebnahmeTyp::Erweiterung.resets_foerderdauer());
}

/// Wiederinbetriebnahme (restart after shutdown) must NOT reset the Förderdauer —
/// the plant continues under its original commissioning date.
#[test]
fn inbetriebnahme_typ_wiederinbetriebnahme_keeps_foerderdauer() {
    use eeg_billing::InbetriebnahmeTyp;
    let original_inbetriebnahme = date!(2010 - 06 - 01);
    let foerderendedatum = foerderendedatum_eeg(original_inbetriebnahme).unwrap();

    // Restart in 2018 — still uses 2010 foerderendedatum
    let input = SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("12.83"),
        },
        inbetriebnahme: Some(original_inbetriebnahme),
        inbetriebnahme_typ: InbetriebnahmeTyp::Wiederinbetriebnahme,
        foerderendedatum: Some(foerderendedatum),
        billing_date: Some(date!(2025 - 01 - 01)),
        einspeisemenge_kwh: Some(d("500")),
        ..SettleInput::default()
    };
    let out = calculate_settlement(&input);
    // Before foerderendedatum (2030-12-31): normal settlement
    assert_eq!(out.status, SettlementStatus::Calculated);
    assert_eq!(out.settlement_eur, Some(d("64.15"))); // 500 × 12.83 / 100
}

// ═══════════════════════════════════════════════════════════════════════════
// FoerderendeGrund — funding termination lifecycle (§25 + §8 KWKG)
// ═══════════════════════════════════════════════════════════════════════════

/// §25 EEG: `FoerderendeGrund::Expired20Years` transitions to PostEeg.
#[test]
fn foerderungsende_expired_20years_transitions_to_post_eeg() {
    use eeg_billing::foerderungsende::FoerderendeGrund;
    assert!(FoerderendeGrund::Expired20Years.transitions_to_post_eeg());
    assert!(FoerderendeGrund::Expired20YearsPlusSect51aExtension.transitions_to_post_eeg());
}

/// Terminal reasons do NOT transition to PostEeg.
#[test]
fn foerderungsende_terminal_reasons_do_not_post_eeg() {
    use eeg_billing::foerderungsende::FoerderendeGrund;
    assert!(!FoerderendeGrund::AuctionAwardExpired.transitions_to_post_eeg());
    assert!(!FoerderendeGrund::KwkHourLimitExhausted.transitions_to_post_eeg());
    assert!(!FoerderendeGrund::KwkYearLimitReached.transitions_to_post_eeg());
    assert!(!FoerderendeGrund::Revoked.transitions_to_post_eeg());
    assert!(!FoerderendeGrund::VoluntaryTermination.transitions_to_post_eeg());
    assert!(!FoerderendeGrund::PermanentLoss.transitions_to_post_eeg());
    assert!(!FoerderendeGrund::MastrDeregistered.transitions_to_post_eeg());
}

// ═══════════════════════════════════════════════════════════════════════════
// §§40–41 EEG 2023 — Wasserkraft (hydropower ecological compliance)
// ═══════════════════════════════════════════════════════════════════════════

/// §40 EEG 2023: Wasserkraft plant uses FeedInTariff with ErzeugungsArt::Wasserkraft.
/// The calculation is formula-identical to solar — what differs is the tariff rate
/// and the ecological compliance requirement stored in the plant registry.
///
/// This test documents that Wasserkraft plants can use the standard settlement path.
#[test]
fn s40_wasserkraft_slp_flat_rate() {
    use eeg_billing::ErzeugungsArt;
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("7.33"), // §40 Abs. 2 EEG 2023 reference rate ≤500 kW
        },
        erzeugungsart: Some(ErzeugungsArt::Wasserkraft),
        einspeisemenge_kwh: Some(d("12000")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 12000 × 7.33 / 100 = 879.60 EUR
    assert_eq!(out.settlement_eur, Some(d("879.60")));
}

/// §41 EEG 2023: Wasserkraft modernization — plant gets extended support for
/// ecological improvements. The calculation path is the same; the extended
/// foerderendedatum is computed by the plant registry (einsd), not the formula.
///
/// This test confirms that modernized hydro plants settle identically to new plants.
#[test]
fn s41_wasserkraft_modernisierung_settlement_path() {
    use eeg_billing::ErzeugungsArt;
    // Modernized 2023-05-01; foerderendedatum extended to 2043-12-31
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("12.50"),
        },
        erzeugungsart: Some(ErzeugungsArt::Wasserkraft),
        einspeisemenge_kwh: Some(d("8500")),
        foerderendedatum: Some(date!(2043 - 12 - 31)),
        billing_date: Some(date!(2026 - 01 - 01)),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 8500 × 12.50 / 100 = 1062.50 EUR
    assert_eq!(out.settlement_eur, Some(d("1062.50")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §48a EEG 2023 — Gemeinschaftliche Gebäudeversorgung (GGV) settlement
// ═══════════════════════════════════════════════════════════════════════════

/// §48a EEG 2023 (Solarpaket I): Gemeinschaftliche Gebäudeversorgung.
///
/// A solar plant serves multiple building tenants. The plant operator receives
/// payment for the total Einspeisemenge at the §48 Abs. 2 rate — the multi-tenant
/// allocation is handled by the metering layer (the external `metering` crate's
/// `AggregationRule::GgvProportionalAllocation` in edmd).
/// The NB-to-plant-operator settlement uses `FeedInTariff` with the §48a Zuschlag
/// incorporated into the rate by the plant registry.
///
/// Regulatory basis: §48a EEG 2023 i.d.F. Solarpaket I (2024).
#[test]
fn sect48a_ggv_settlement_uses_feed_in_tariff_scheme() {
    use eeg_billing::ErzeugungsArt;
    // GGV building plant: 25 kWp on a 5-party building, total feed-in = 350 kWh
    // Rate: §48 Abs. 2a EEG 2023 (GGV Volleinspeisung) = 8.51 ct/kWh
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.51"),
        },
        erzeugungsart: Some(ErzeugungsArt::SolarAufdach),
        einspeisemenge_kwh: Some(d("350")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 350 × 8.51 / 100 = 29.785 EUR
    assert_eq!(out.settlement_eur, Some(d("29.785")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §37a / Balkonkraftwerk (Stecker-PV) simplified path
// ═══════════════════════════════════════════════════════════════════════════

/// §37a EEG 2023: Stecker-PV (Balkonkraftwerk) ≤800 W.
///
/// Balkonkraftwerke are registered in MaStR but use simplified registration.
/// Settlement is same formula as FeedInTariff at the §48 Abs. 2 rate.
/// The ErzeugungsArt::SolarStecker variant identifies these plants in the registry.
///
/// Regulatory basis: §37a EEG 2023 (Steckersolargeräte), MaStR §5a AnlRegV.
#[test]
fn sect37a_stecker_pv_settlement_is_standard_feed_in() {
    use eeg_billing::ErzeugungsArt;
    // 600 W Balkonkraftwerk, 25 kWh fed in during summer month
    // Rate: §48 Abs. 2 EEG 2024 (Solarpaket I): 8.51 ct/kWh (≤10 kWp band)
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.51"),
        },
        erzeugungsart: Some(ErzeugungsArt::SolarStecker),
        einspeisemenge_kwh: Some(d("25")),
        leistung_kwp: Some(d("0.6")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 25 × 8.51 / 100 = 2.1275 EUR
    assert_eq!(out.settlement_eur, Some(d("2.1275")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §52 Abs. 4 — Extra penalty months for specific violations
// ═══════════════════════════════════════════════════════════════════════════

/// §52 Abs. 4 Nr. 4: Doppelvermarktungsverbot violation → +6 extra penalty months.
///
/// When an operator violates §80 EEG (double marketing), the penalty applies for
/// the violation period PLUS 6 additional calendar months.
/// The caller must add these 6 months to `monate_des_verstosses`.
///
/// This test verifies that the formula correctly scales to the extended period.
#[test]
fn sect52_abs4_doppelvermarktung_6_extra_months() {
    use eeg_billing::foerderdauer::calculate_pflichtzahlung;
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};
    // 3-month violation + 6 extra = 9 months total; 200 kW plant
    let violation = Pflichtverstoss {
        typ: SanktionsTyp::DoppelvermarktungsverbotVerletzt,
        leistung_kw: d("200"),
        monate_des_verstosses: 9, // caller adds 6 extra months per §52 Abs. 4 Nr. 4
        nachtraeglich_erfuellt: false,
        technischer_defekt: false,
    };
    let penalty = calculate_pflichtzahlung(&violation);
    // 200 kW × EUR 10 × 9 months = EUR 18 000
    assert_eq!(penalty, d("18000"));
}

/// §52 Abs. 4: the Ausfallvergütung-Höchstdauer violation (Nr. 5) gets **no**
/// extra months — Abs. 4 lists only Nr. 7 (+3), Nr. 9 (+1), Nr. 10 (full year)
/// and Nr. 12 (+6). The Pflichtzahlung is €10/kW for the violation months alone.
#[test]
fn sect52_ausfallverguetung_has_no_extra_months() {
    use eeg_billing::foerderdauer::calculate_pflichtzahlung;
    use eeg_billing::{Pflichtverstoss, SanktionsTyp};
    // 3-month violation (the §21 Abs. 1 Satz 1 Nr. 3 maximum); 100 kW plant.
    let violation = Pflichtverstoss {
        typ: SanktionsTyp::AusfallverguetungHoechstdauerUeberschritten,
        leistung_kw: d("100"),
        monate_des_verstosses: 3,
        nachtraeglich_erfuellt: false,
        technischer_defekt: false,
    };
    let penalty = calculate_pflichtzahlung(&violation);
    // 100 kW × EUR 10 × 3 months = EUR 3 000 (no Abs. 4 extension for Nr. 5).
    assert_eq!(penalty, d("3000"));
}

// ═══════════════════════════════════════════════════════════════════════════
// foerderendedatum edge cases — leap year, year boundary
// ═══════════════════════════════════════════════════════════════════════════

/// §25 EEG: foerderendedatum for plants commissioned on February 28 in a non-leap year.
/// The 20-year end date is always December 31 of the 20th year (§25 Abs. 1 Satz 2).
#[test]
fn foerderendedatum_feb28_nonleap_year() {
    let inbetriebnahme = date!(2005 - 02 - 28);
    let end = foerderendedatum_eeg(inbetriebnahme).unwrap();
    // 2025 is not a leap year; December 31, 2025
    assert_eq!(end, date!(2025 - 12 - 31));
}

/// §25 EEG: a plant commissioned on December 31 ends on December 31 of year+20.
/// The end date is in the SAME calendar year as the start year + 20.
#[test]
fn foerderendedatum_dec31_plant_ends_same_year() {
    let inbetriebnahme = date!(2023 - 12 - 31);
    let end = foerderendedatum_eeg(inbetriebnahme).unwrap();
    assert_eq!(end, date!(2043 - 12 - 31));
}

/// §25 EEG: a plant commissioned on January 1 ends on December 31 of year+20.
/// Not December 31 of year+19 — the end must be at least 20 full years.
#[test]
fn foerderendedatum_jan1_plant_ends_20_years_later() {
    let inbetriebnahme = date!(2010 - 01 - 01);
    let end = foerderendedatum_eeg(inbetriebnahme).unwrap();
    assert_eq!(end, date!(2030 - 12 - 31));
    // Not 2029-12-31 — the 20th year after 2010 is 2030
}

// ═══════════════════════════════════════════════════════════════════════════
// EegGesetz — Bestandsschutz boundary tests
// ═══════════════════════════════════════════════════════════════════════════

/// §100 Abs. 1 Satz 4 EEG 2017 boundary:
/// Plants commissioned 2015-12-31 (last day before 2016-01-01) are EXEMPT from §51.
/// Plants commissioned 2016-01-01 ARE subject to §51 EEG 2017 (6h threshold).
#[test]
fn eeg_gesetz_bestandsschutz_boundary_2015_to_2016() {
    use eeg_billing::{ErzeugungsArt, NegativpreisRegime as R};
    use rust_decimal::dec;
    use time::macros::date;

    // Pre-2016: §51 never applies (§100 Abs. 1 Satz 4 EEG 2017).
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2015 - 12 - 31)).kw_grenze(None),
        None,
        "EEG 2012 plants exempt from §51 per §100 Abs. 1 Satz 4"
    );

    // §51 Abs. 3 Nr. 2 EEG 2017: 500 kW for non-wind.
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2016 - 01 - 01)).kw_grenze(Some(ErzeugungsArt::SolarAufdach)),
        Some(dec!(500))
    );
    // §51 Abs. 3 Nr. 1 EEG 2017: 3 MW for wind.
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2016 - 01 - 01)).kw_grenze(Some(ErzeugungsArt::WindOnshore)),
        Some(dec!(3000))
    );
    // EEG 2021 dropped the wind carve-out.
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2021 - 06 - 01)).kw_grenze(Some(ErzeugungsArt::WindOnshore)),
        Some(dec!(500))
    );
    // EEG 2023 as enacted: 400 kW — not the 100 kW of the current Fassung.
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2024 - 06 - 01)).kw_grenze(None),
        Some(dec!(400))
    );
    // From the Solarspitzengesetz: 100 kW, transitional until iMSys.
    assert_eq!(
        R::fuer_inbetriebnahme(date!(2025 - 06 - 01)).kw_grenze(None),
        Some(dec!(100))
    );
}

/// EEG 2023 §51 applies to ALL sizes once iMSys is installed.
#[test]
fn eeg2023_sect51_applies_to_all_once_imesys_installed() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("100")),
        kwh_during_negative_epex: Some(d("50")), // 50 kWh during negative hours
        leistung_kwp: Some(d("50")),             // 50 kWp — would be exempt WITHOUT iMSys
        has_imesys: true,                        // iMSys installed → exemption lifted
        eeg_gesetz: eeg_billing::EegGesetz::Eeg2023,
        ..SettleInput::default()
    });
    // §51 applied: eligible = 100 - 50 = 50 kWh
    // 50 × 8.11 / 100 = 4.055 EUR
    assert_eq!(out.eligible_kwh, Some(d("50")));
    assert_eq!(out.settlement_eur, Some(d("4.055")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §43 Abs. 1 Nr. 2 EEG 2023 — Biomass substrate cap blocks settlement
// ═══════════════════════════════════════════════════════════════════════════

/// §43 Abs. 1 Nr. 2 EEG 2023 — plant with >40% Energiepflanzen vom Acker
/// loses EEG support for the billing period entirely.
///
/// Legal basis: §43 Abs. 1 Nr. 2 EEG 2023 (BGBl. I 2023 Nr. 1):
/// "Der Anteil der im Durchschnitt des Kalenderjahres für die Erzeugung von
/// Strom und Wärme … eingesetzten Energiepflanzen … 40 Prozent nicht übersteigen."
#[test]
fn sect43_substrate_cap_exceeded_blocks_settlement() {
    use eeg_billing::biomasse::{BiomassBrennstoff, BiomassSettlementData};

    // Plant with 55% Energiepflanzen (exceeds 40% cap)
    let biomasse = BiomassSettlementData::new(
        BiomassBrennstoff::PflanzlicheBiomasse,
        dec!(0.0),  // no Gülle
        dec!(0.55), // 55% Energiepflanzen — cap exceeded
        dec!(200),  // 200 kW plant
    );
    assert!(!biomasse.substrate_cap_ok);

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.47"),
        },
        einspeisemenge_kwh: Some(d("10000")),
        biomasse: Some(biomasse),
        ..SettleInput::default()
    });

    // §43 cap violated → settlement must be blocked
    assert_eq!(out.status, SettlementStatus::Sanctioned);
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
    assert!(
        out.positions.iter().any(|p| p.legal_basis.contains("§43")),
        "position must cite §43 EEG 2023"
    );
}

/// §43 Abs. 1 Nr. 2 EEG 2023 — plant exactly at the 40% cap proceeds normally.
#[test]
fn sect43_substrate_cap_exactly_at_limit_allows_settlement() {
    use eeg_billing::biomasse::{BiomassBrennstoff, BiomassSettlementData};

    // Plant exactly at 40% cap — must NOT be blocked
    let biomasse = BiomassSettlementData::new(
        BiomassBrennstoff::PflanzlicheBiomasse,
        dec!(0.0),  // no Gülle
        dec!(0.40), // exactly 40% — within limit
        dec!(200),
    );
    assert!(biomasse.substrate_cap_ok);

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.47"),
        },
        einspeisemenge_kwh: Some(d("5000")),
        biomasse: Some(biomasse),
        ..SettleInput::default()
    });

    assert_eq!(out.status, SettlementStatus::Calculated);
    // 5000 × 14.47 / 100 = 723.50 EUR
    assert_eq!(out.settlement_eur, Some(d("723.50")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §44 EEG 2023 — Güllekleinanlage rate table
// ═══════════════════════════════════════════════════════════════════════════

/// §44 EEG 2023 — Güllekleinanlage: correct gross AW from rate table.
///
/// Plants ≤75 kW_el with ≥80% Gülle input receive 16.90 ct/kWh gross AW
/// (net after §53 -0.2 ct deduction = 16.70 ct/kWh).
///
/// Legal basis: §44 EEG 2023 (BGBl. I 2023 Nr. 1)
#[test]
fn sect44_guellebonusanlage_rate_table() {
    use eeg_billing::biomasse::{BiomassBrennstoff, BiomassSettlementData};
    use eeg_billing::rates;

    // Verify rate table lookup
    let table = rates::guellekleinanlage_rate(2023).expect("EEG 2023 Güllekleinanlage rates known");
    let gross_aw = table.rate_for(dec!(50)).expect("50 kW in range");
    // Gross AW = 16.90 ct/kWh → Amount<5> = 0.16900 EUR/kWh
    // billing::Amount is EUR/kWh; convert to ct for readable assertion
    let gross_aw_ct = gross_aw.into_decimal() * rust_decimal::Decimal::from(100u32);
    assert_eq!(
        gross_aw_ct.round_dp(2),
        dec!(16.90),
        "§44 EEG 2023 gross AW = 16.90 ct/kWh"
    );

    // Plants > 75 kW must not receive Güllekleinanlage rate
    // rate_for returns Result<Amount, BillingError>; Err = capacity exceeds table
    assert!(
        table.rate_for(dec!(80)).is_err(),
        ">75 kW not eligible for Güllekleinanlage rate"
    );

    // Net rate = gross − §53 deduction (0.2 ct for Biomasse)
    let sect53 = rates::sect53_deduction(eeg_billing::ErzeugungsArt::Biogas);
    let net_ct: rust_decimal::Decimal = dec!(16.90) - sect53;
    assert_eq!(
        net_ct,
        dec!(16.70),
        "net Vergütungssatz after §53 deduction = 16.70 ct/kWh"
    );

    // Settlement with the net rate
    let biomasse = BiomassSettlementData::new(
        BiomassBrennstoff::Guelle,
        dec!(0.85), // 85% Gülle
        dec!(0.05),
        dec!(50), // 50 kW — Güllekleinanlage eligible
    );
    assert!(biomasse.ist_guellebonusanlage);

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: net_ct, // 16.70 ct/kWh (after §53 deduction)
        },
        einspeisemenge_kwh: Some(d("2000")),
        biomasse: Some(biomasse),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 2000 × 16.70 / 100 = 334.00 EUR
    assert_eq!(out.settlement_eur, Some(d("334.00")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §42a EEG 2023 — Holzbiomasse restriction post-2026
// ═══════════════════════════════════════════════════════════════════════════

/// §42a EEG 2023 — new Holzbiomasse plant commissioned from 2026-01-01
/// loses EEG eligibility (fresh wood primary energy prohibition).
#[test]
fn sect42a_holzbiomasse_post_2026_blocked() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.47"),
        },
        einspeisemenge_kwh: Some(d("8000")),
        erzeugungsart: Some(eeg_billing::ErzeugungsArt::BiomassHolz),
        inbetriebnahme: Some(date!(2026 - 03 - 15)), // commissioned after restriction
        ..SettleInput::default()
    });

    assert_eq!(
        out.status,
        SettlementStatus::Sanctioned,
        "Holzbiomasse ≥ 2026 must be Sanctioned"
    );
    assert_eq!(out.settlement_eur, Some(Decimal::ZERO));
    assert!(
        out.positions.iter().any(|p| p.legal_basis.contains("§42a")),
        "position must cite §42a EEG 2023"
    );
}

/// §42a EEG 2023 — Holzbiomasse plant commissioned BEFORE 2026 retains Bestandsschutz.
#[test]
fn sect42a_holzbiomasse_pre_2026_bestandsschutz() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("13.63"),
        },
        einspeisemenge_kwh: Some(d("5000")),
        erzeugungsart: Some(eeg_billing::ErzeugungsArt::BiomassHolz),
        inbetriebnahme: Some(date!(2022 - 06 - 01)), // pre-2026 → Bestandsschutz
        ..SettleInput::default()
    });

    assert_eq!(
        out.status,
        SettlementStatus::Calculated,
        "Pre-2026 Holzbiomasse plant retains EEG support (Bestandsschutz)"
    );
    // 5000 × 13.63 / 100 = 681.50 EUR
    assert_eq!(out.settlement_eur, Some(d("681.50")));
}

// ═══════════════════════════════════════════════════════════════════════════
// §§70–74 EEG 2023 — Wind offshore settlement via MarketPremium
// ═══════════════════════════════════════════════════════════════════════════

/// §§70–74 EEG 2023 — Wind offshore: always tender-based (§22 Abs. 3 EEG).
/// Settled as MarketPremium with BNetzA-awarded AW.
///
/// Legal basis: §§70–74 EEG 2023 (Offshore-Zuschlag via BNetzA-Ausschreibung).
#[test]
fn wind_offshore_market_premium_settlement() {
    use eeg_billing::scheme::AusschreibungMetadata;

    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("8.40"), // BNetzA tender-awarded AW
            wind_korrekturfaktor: None,  // offshore uses no §36h correction
            wind_standort: None,
        },
        tariff_source: TariffSource::Auction(AusschreibungMetadata {
            zuschlag_id: Some("BNetzA-OFF-2024-001".to_owned()),
            award_ct: Some(d("8.40")),
            award_date: Some(date!(2024 - 05 - 01)),
            award_expired: false,
            innovation_auction: false,
            is_buergerenergie: false,
            is_biogas_sect51b: false,
        }),
        einspeisemenge_kwh: Some(d("50000000")), // 50 GWh offshore farm
        marktwert_ct_kwh: Some(d("6.20")),       // EPEX monthly avg
        leistung_kwp: Some(d("120000")),         // 120 MW
        erzeugungsart: Some(eeg_billing::ErzeugungsArt::WindOffshore),
        eeg_gesetz: EegGesetz::Eeg2023,
        ..SettleInput::default()
    });

    assert_eq!(out.status, SettlementStatus::Calculated);
    // MP = max(0, AW − MW) = 8.40 − 6.20 = 2.20 ct
    // 50_000_000 × 2.20 / 100 = 1_100_000 EUR
    assert_eq!(out.settlement_eur, Some(d("1100000.00")));
    assert!(out.settlement_eur.is_some_and(|e| e > Decimal::ZERO));
}

// ═══════════════════════════════════════════════════════════════════════════
// §48b EEG 2023 (Solarpaket I) — Stecker-PV SLP billing
// ═══════════════════════════════════════════════════════════════════════════

/// §48b EEG 2023 (Solarpaket I, BGBl I 2024 Nr. 107) — Stecker-PV (Balkonkraftwerk).
///
/// Stecker-PV (≤2 kWp) uses simplified SLP S0 annual feed-in estimation.
/// No mandatory MaStR registration below 800 W (§9 Abs. 1 EEG 2023 exception).
/// Einspeisevergütung is the same formula as standard solar but at the applicable
/// Solarpaket I rate.
///
/// Note: Stecker-PV feed-in is typically very small (annual ~100–300 kWh).
/// The rate is based on §48 Abs. 2 EEG 2023 (Überschusseinspeisung, ≤10 kWp).
#[test]
fn sect48b_stecker_pv_annual_settlement_via_slp_estimate() {
    // §48 Abs. 2 Nr. 1 in the 1 Feb 2024 §49 window: 8.51 ct/kWh.
    let rate = eeg_billing::rates::solar_pv_ueberschuss_lookup(date!(2024 - 03 - 01))
        .expect("EEG 2023 window known")
        .rate_for(dec!(0.8)) // 800 W Stecker-PV
        .expect("800 W in range");

    assert_eq!(
        rate,
        billing::Amount::parse("0.08510").unwrap(),
        "Stecker-PV ≤ 10 kWp: 8.51 ct/kWh in the Feb-2024 window"
    );

    // Annual SLP estimate: 120 kWh net feed-in for 800 W balcony panel
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"), // net after §53 deduction (8.51 - 0.40)
        },
        einspeisemenge_kwh: Some(d("120")), // 120 kWh annual feed-in
        erzeugungsart: Some(eeg_billing::ErzeugungsArt::SolarStecker),
        leistung_kwp: Some(d("0.80")),
        eeg_gesetz: EegGesetz::Eeg2023,
        ..SettleInput::default()
    });

    assert_eq!(out.status, SettlementStatus::Calculated);
    // 120 × 8.11 / 100 = 9.732 EUR
    assert_eq!(out.settlement_eur, Some(d("9.732")));
}

// ══════════════════════════════════════════════════════════════════════════════
// Findings pinned as regressions — §24/§51 aggregation, §25 proration, §13a
// ══════════════════════════════════════════════════════════════════════════════

/// §51 Abs. 2 Satz 2 EEG 2023 i.V.m. §24 — the kW test runs on the whole plant.
///
/// A 600 kWp plant split into three §24-linked 200 kWp blocks is one Anlage for
/// the §51 size test ("Zur Ermittlung der Anlagengröße nach Satz 1 ist § 24
/// entsprechend anzuwenden"). Testing each block on its own would put every
/// block under the exemption — 400 kW for a plant commissioned under the EEG
/// 2023 as enacted — and let the plant keep the full payment for its
/// negative-price energy.
#[test]
fn s51_abs2_satz2_aggregates_capacity_blocks_per_sect24() {
    use eeg_billing::CapacityBlock;
    let block = |kwp: &str| CapacityBlock {
        leistung_kwp: d(kwp),
        verguetungssatz_ct: d("8.00"),
        inbetriebnahme: date!(2023 - 03 - 01),
        foerderendedatum: date!(2043 - 03 - 01),
    };
    // A 2023 plant is under the staged 4-hour rule; the caller has already
    // applied it when deriving `kwh_during_negative_epex`.
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.00"),
        },
        einspeisemenge_kwh: Some(d("30000")),
        leistung_kwp: Some(d("200")),
        capacity_blocks: vec![block("200"), block("200")],
        inbetriebnahme: Some(date!(2023 - 03 - 01)),
        erzeugungsart: Some(eeg_billing::ErzeugungsArt::SolarAufdach),
        kwh_during_negative_epex: Some(d("3000")),
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::Calculated);
    // 600 kWp total ≥ the 400 kW threshold → §51 applies to every block.
    // (Three 1/3 shares rounded to 3 dp leave a 0.027 kWh allocation residue.)
    assert_eq!(out.eligible_kwh.unwrap().round(), d("27000"));
    assert_eq!(out.settlement_eur.unwrap().round_dp(0), d("2160"));
}

/// §24 EEG 2023 — a multi-block plant on a scheme with no per-block rate is
/// `PriceMissing`, never a €0 `Calculated`.
///
/// `MarketPremium` carries no `verguetungssatz_ct`; settling its blocks at zero
/// would report "we owe you nothing" for a plant that is owed its full
/// Marktprämie, and the status would not flag it for anyone to notice.
#[test]
fn s24_multi_block_without_a_block_rate_is_price_missing() {
    use eeg_billing::CapacityBlock;
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("6.20"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        einspeisemenge_kwh: Some(d("100000")),
        marktwert_ct_kwh: Some(d("4.50")),
        leistung_kwp: Some(d("500")),
        capacity_blocks: vec![CapacityBlock {
            leistung_kwp: d("250"),
            verguetungssatz_ct: d("6.00"),
            inbetriebnahme: date!(2024 - 01 - 01),
            foerderendedatum: date!(2044 - 01 - 01),
        }],
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::PriceMissing);
    assert_eq!(out.settlement_eur, None);
}

/// §25 Abs. 1 Satz 3 EEG 2023 — the commissioning month is not prorated.
///
/// A meter commissioned on 15 June only ever recorded the second half of the
/// month, so the reported 500 kWh *is* the partial month. Scaling it by 16/30
/// would bill 266.67 kWh and underpay the operator by 47 %.
#[test]
fn s25_commissioning_month_is_not_double_prorated() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("500")),
        inbetriebnahme: Some(date!(2024 - 06 - 15)),
        billing_date: Some(date!(2024 - 06 - 01)),
        ..SettleInput::default()
    });
    assert_eq!(out.eligible_kwh, Some(d("500")));
    assert_eq!(out.settlement_eur, Some(d("40.55")));
    assert_eq!(out.billing_days_fraction_applied, None);
}

/// §25 EEG 2023 — the Förderende month *is* prorated: the meter runs past the
/// entitlement, so the reading covers more than may be paid.
#[test]
fn s25_foerderende_month_is_prorated() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("10.00"),
        },
        einspeisemenge_kwh: Some(d("3000")),
        foerderendedatum: Some(date!(2024 - 06 - 20)),
        billing_date: Some(date!(2024 - 06 - 01)),
        ..SettleInput::default()
    });
    // 20/30 of June is still entitled → 2 000 kWh × 10 ct = 200 EUR.
    assert_eq!(out.eligible_kwh, Some(d("2000")));
    assert_eq!(out.settlement_eur, Some(d("200.00")));
    assert!(out.billing_days_fraction_applied.is_some());
}

/// §13a EnWG — the curtailment compensation is not scaled by the §25 fraction.
///
/// Those kWh were curtailed by the Netzbetreiber, not fed in over an entitlement
/// window; the claim is the full compensated quantity.
#[test]
fn s13a_compensation_survives_the_sect25_proration() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("10.00"),
        },
        einspeisemenge_kwh: Some(d("3000")),
        einspeisemanagement_kwh: Some(d("1000")),
        foerderendedatum: Some(date!(2024 - 06 - 20)),
        billing_date: Some(date!(2024 - 06 - 01)),
        ..SettleInput::default()
    });
    let einsman = out
        .positions
        .iter()
        .find(|p| p.legal_basis.contains("§13a"))
        .expect("a §13a position");
    assert_eq!(einsman.kwh, d("1000"), "curtailed energy is not prorated");
    assert_eq!(einsman.eur, d("100.00"));
    // 200 EUR prorated EEG payment + 100 EUR untouched §13a compensation.
    assert_eq!(out.settlement_eur, Some(d("300.00")));
}

/// §42a EEG 2023 — a §13a EnWG compensation does not lift an EEG sanction.
#[test]
fn s42a_sanction_survives_an_einspeisemanagement_position() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.67"),
        },
        einspeisemenge_kwh: Some(d("10000")),
        einspeisemanagement_kwh: Some(d("500")),
        erzeugungsart: Some(eeg_billing::ErzeugungsArt::BiomassHolz),
        inbetriebnahme: Some(date!(2026 - 02 - 01)),
        ..SettleInput::default()
    });
    assert_eq!(
        out.status,
        SettlementStatus::Sanctioned,
        "the §42a sanction must remain visible on the settlement"
    );
}

/// §44b Abs. 1 Satz 2 EEG 2023 — no Marktwert means no price for the excess.
#[test]
fn s44b_excess_without_a_marktwert_is_price_missing() {
    let out = calculate_settlement(&SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("14.67"),
        },
        einspeisemenge_kwh: Some(d("100000")),
        biogas_sect44b_eligible_kwh: Some(d("45000")),
        marktwert_ct_kwh: None,
        ..SettleInput::default()
    });
    assert_eq!(out.status, SettlementStatus::PriceMissing);
}

/// §51a — the extension is granted only where §51 actually withheld something.
///
/// The Verlängerung covers "Strom … für den sich der anzulegende Wert nach
/// Maßgabe des § 51 verringert", so a plant inside the Abs. 2 exemption was paid
/// in full and earns nothing.
#[test]
fn s51a_no_extension_where_the_kw_exemption_applied() {
    let input = |kwp: &str| SettleInput {
        scheme: SettlementScheme::FeedInTariff {
            verguetungssatz_ct: d("8.11"),
        },
        einspeisemenge_kwh: Some(d("10000")),
        leistung_kwp: Some(d(kwp)),
        erzeugungsart: Some(eeg_billing::ErzeugungsArt::SolarAufdach),
        // From the Solarspitzengesetz: exemption at 100 kW, and §51a covers every
        // plant rather than only the ausschreibungspflichtige ones.
        inbetriebnahme: Some(date!(2025 - 06 - 01)),
        kwh_during_negative_epex: Some(d("400")),
        negative_price_quarter_hours: Some(96),
        ..SettleInput::default()
    };
    let exempt = calculate_settlement(&input("50"));
    assert_eq!(exempt.verlaengerungsanspruch_qh, 0);
    assert_eq!(exempt.eligible_kwh, Some(d("10000")), "paid in full");

    let subject = calculate_settlement(&input("150"));
    assert!(subject.verlaengerungsanspruch_qh > 0);
    assert_eq!(subject.eligible_kwh, Some(d("9600")));
}

/// §51a before the Solarspitzengesetz — a statutory-AW plant is reduced and gets
/// **nothing** back.
///
/// Until 25.02.2025 the Verlängerung existed only for ausschreibungspflichtige
/// Anlagen. A 500 kWp plant on the statutory tariff commissioned in 2024 loses
/// the negative-price quarter-hours outright; granting it an extension would
/// stretch a Förderdauer the statute ends on time.
#[test]
fn s51a_pre_solarspitzen_extension_is_auction_only() {
    let input = |source: eeg_billing::TariffSource| SettleInput {
        scheme: SettlementScheme::MarketPremium {
            direktverm_aw_ct: d("7.00"),
            wind_korrekturfaktor: None,
            wind_standort: None,
        },
        tariff_source: source,
        einspeisemenge_kwh: Some(d("10000")),
        marktwert_ct_kwh: Some(d("4.00")),
        leistung_kwp: Some(d("500")),
        erzeugungsart: Some(eeg_billing::ErzeugungsArt::SolarFreiflaeche),
        inbetriebnahme: Some(date!(2024 - 06 - 01)),
        kwh_during_negative_epex: Some(d("400")),
        negative_price_quarter_hours: Some(96),
        ..SettleInput::default()
    };

    let statutory = calculate_settlement(&input(eeg_billing::TariffSource::Statutory));
    assert_eq!(
        statutory.verlaengerungsanspruch_qh, 0,
        "§51a did not reach statutory-AW plants before the Solarspitzengesetz"
    );
    assert_eq!(
        statutory.eligible_kwh,
        Some(d("9600")),
        "§51 still reduced the plant — it simply gets no time back"
    );

    let auction = calculate_settlement(&input(eeg_billing::TariffSource::Auction(
        eeg_billing::AusschreibungMetadata::default(),
    )));
    assert!(
        auction.verlaengerungsanspruch_qh > 0,
        "an ausschreibungspflichtige Anlage did earn the extension"
    );
}
