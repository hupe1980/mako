//! A commodity product must be able to price its commodity.
//!
//! The price fields of a `Product` are populated by mapping `productd`'s
//! `preistyp` strings onto struct fields. A renamed position, a typo in the
//! mapper, or a catalog row saved without its price maps to `None` — in
//! silence. Unguarded, the resulting invoice is not an error: it bills 1000 kWh
//! of electricity for €20.50 — the Stromsteuer and nothing else — and looks
//! entirely ordinary on paper.

use energy_billing::{
    BillingContext, BillingPeriod, EegMeterInput, EnergyShareMeterInput, EngineError, GridInput,
    HemsMeterInput, MeterInput, MeteringMode, Product, Quantities, RegulatoryRates,
    Sect14aModul3Verbrauch, WarningSeverity,
};
use rust_decimal::dec;
use time::macros::date;

fn ctx(rates: &RegulatoryRates) -> BillingContext {
    BillingContext {
        malo_id: "51238696781".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "GUARD-1".into(),
        period: BillingPeriod::new(date!(2026 - 06 - 01), date!(2026 - 06 - 30)).unwrap(),
        regulatory_rates: rates.clone(),
        ..Default::default()
    }
}

/// The defect, exactly as it reached production shape: a STROM product whose
/// price fields are all absent.
#[test]
fn electricity_without_any_work_price_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(r#"{"category":"STROM"}"#).unwrap();
    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);

    let warnings = engine.validate(&ctx(&rates), &q);
    let finding = warnings
        .iter()
        .find(|w| w.code == "KEIN_ARBEITSPREIS")
        .expect("a product that cannot price its commodity must be a finding");
    assert_eq!(
        finding.severity,
        WarningSeverity::Error,
        "Warning severity would let the invoice out; only Error blocks it"
    );

    match engine.bill(ctx(&rates), &q) {
        Err(EngineError::ValidationBlocked { warnings }) => {
            assert!(warnings.iter().any(|w| w.code == "KEIN_ARBEITSPREIS"));
        }
        Err(other) => panic!("expected ValidationBlocked, got {other}"),
        Ok(inv) => panic!(
            "a priceless product billed {} EUR instead of being refused — this is the \
             Stromsteuer-only invoice the guard exists to prevent",
            inv.netto_eur
        ),
    }
}

/// Every way of pricing electricity satisfies the guard — it asks whether the
/// product can price the commodity at all, not whether it uses one nominated
/// field. A tariff priced only by HT/NT, or only dynamically, is legitimate.
#[test]
fn any_form_of_work_price_satisfies_the_guard() {
    let rates = RegulatoryRates::default();
    for json in [
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"30.0"}"#,
        r#"{"category":"STROM","arbeitspreis_ht_ct_per_kwh":"32.0","arbeitspreis_nt_ct_per_kwh":"24.0"}"#,
        r#"{"category":"STROM","dynamic_epex":true}"#,
    ] {
        let product: Product = serde_json::from_str(json).unwrap();
        let engine = product.build_engine(&GridInput::default(), &rates);
        let warnings = engine.validate(&ctx(&rates), &Quantities::default());
        assert!(
            !warnings.iter().any(|w| w.code == "KEIN_ARBEITSPREIS"),
            "{json} prices its commodity and must not be flagged: {warnings:?}"
        );
    }
}

/// An operator who genuinely charges nothing per kWh says so with a zero. The
/// guard distinguishes "priced at zero" from "no price on file", which is the
/// whole point — one is a decision, the other is missing data.
#[test]
fn an_explicit_zero_price_is_a_decision_and_is_allowed() {
    let rates = RegulatoryRates::default();
    let product: Product =
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"0.0"}"#).unwrap();
    let engine = product.build_engine(&GridInput::default(), &rates);
    let warnings = engine.validate(&ctx(&rates), &Quantities::default());
    assert!(!warnings.iter().any(|w| w.code == "KEIN_ARBEITSPREIS"));
}

/// Gas has the same failure mode: without a work price the invoice charges the
/// Energiesteuer and the BEHG levy and nothing for the gas.
#[test]
fn gas_without_a_work_price_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(r#"{"category":"GAS"}"#).unwrap();
    let engine = product.build_engine(&GridInput::default(), &rates);
    let warnings = engine.validate(&ctx(&rates), &Quantities::default());
    assert!(
        warnings
            .iter()
            .any(|w| w.code == "KEIN_ARBEITSPREIS" && w.severity == WarningSeverity::Error),
        "{warnings:?}"
    );
}

/// And Fernwärme.
#[test]
fn heat_without_a_work_price_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(r#"{"category":"WAERME"}"#).unwrap();
    let engine = product.build_engine(&GridInput::default(), &rates);
    let warnings = engine.validate(&ctx(&rates), &Quantities::default());
    assert!(
        warnings
            .iter()
            .any(|w| w.code == "KEIN_ARBEITSPREIS" && w.severity == WarningSeverity::Error),
        "{warnings:?}"
    );
}

// ── The same defect, reached from the quantity side ──────────────────────────
//
// The guards above ask whether the *product* can price its commodity. These ask
// the mirror question: whether the provider can price the *quantity it was
// handed*. Both end at the same invoice — base fees, levies on nothing, and a
// plausible-looking total.

/// On the § 41a path the quarter-hour series **is** the billed quantity: the
/// Arbeitspreis, the Netzentgelt, the Konzessionsabgabe and the Stromsteuer are
/// all charged on the sum of the priced intervals, and nothing reads the meter
/// total. An absent series therefore does not bill zero energy honestly — it
/// issues a Grundpreis-only invoice for a customer who consumed 1000 kWh.
#[test]
fn a_dynamic_tariff_without_its_interval_series_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":"20"}"#,
    )
    .unwrap();
    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            metering_mode: energy_billing::MeteringMode::Imsys,
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    match engine.bill(ctx(&rates), &q) {
        Err(EngineError::ValidationBlocked { warnings }) => {
            assert!(
                warnings.iter().any(|w| w.code == "SECT41A_KEINE_INTERVALLE"
                    && w.severity == WarningSeverity::Error),
                "{warnings:?}"
            )
        }
        Err(other) => panic!("expected ValidationBlocked, got {other}"),
        Ok(inv) => panic!(
            "1000 kWh billed as {} EUR — the Grundpreis and not one kWh of energy",
            inv.netto_eur
        ),
    }
}

/// A short series is the same defect scaled: it bills the energy *and every
/// levy* on the quantity that happened to arrive, while the meter says
/// otherwise. The meter total is the independent witness.
#[test]
fn a_dynamic_interval_series_must_agree_with_the_meter_total() {
    let rates = RegulatoryRates::default();
    let product: Product =
        serde_json::from_str(r#"{"category":"STROM","dynamic_epex":true}"#).unwrap();
    let mut prices = std::collections::HashMap::new();
    prices.insert(time::macros::datetime!(2026-06-01 10:00 UTC), dec!(25));
    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            metering_mode: energy_billing::MeteringMode::Imsys,
            ..Default::default()
        }),
        dynamic_intervals: vec![energy_billing::DynamicInterval {
            timestamp_utc: time::macros::datetime!(2026-06-01 10:00 UTC),
            kwh: dec!(400),
        }],
        dynamic_epex_prices: prices,
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    match engine.bill(ctx(&rates), &q) {
        Err(EngineError::ValidationBlocked { warnings }) => assert!(
            warnings
                .iter()
                .any(|w| w.code == "SECT41A_INTERVALLSUMME_WEICHT_AB"),
            "{warnings:?}"
        ),
        Err(other) => panic!("expected ValidationBlocked, got {other}"),
        Ok(inv) => panic!("600 kWh silently dropped; invoice netto {}", inv.netto_eur),
    }
}

/// Interval sums and register differences never agree to the last digit — the
/// series is per-quarter-hour rounded, the total is a difference of two
/// readings. Normal measurement noise must not block a run.
#[test]
fn a_dynamic_series_within_tolerance_still_bills() {
    let rates = RegulatoryRates::default();
    let product: Product =
        serde_json::from_str(r#"{"category":"STROM","dynamic_epex":true}"#).unwrap();
    let mut prices = std::collections::HashMap::new();
    prices.insert(time::macros::datetime!(2026-06-01 10:00 UTC), dec!(25));
    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            metering_mode: energy_billing::MeteringMode::Imsys,
            ..Default::default()
        }),
        dynamic_intervals: vec![energy_billing::DynamicInterval {
            timestamp_utc: time::macros::datetime!(2026-06-01 10:00 UTC),
            // 0.3 % out — inside the 0.5 % tolerance.
            kwh: dec!(997),
        }],
        dynamic_epex_prices: prices,
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    engine
        .bill(ctx(&rates), &q)
        .expect("measurement noise must not block a billing run");
}

/// Water has the same failure mode from the product side, and it is easy to
/// miss because the invoice is not empty: the Schmutzwassergebühr rides the
/// Frischwassermaßstab, so a tariff that prices only the Abwasser side bills a
/// full, plausible Gebühr and nothing for the drinking water delivered.
#[test]
fn water_without_any_trinkwasser_price_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product =
        serde_json::from_str(r#"{"category":"WASSER","schmutzwasser_eur_per_m3":"2.5"}"#).unwrap();
    let q = Quantities {
        wasser: Some(energy_billing::WasserMeterInput {
            frischwasser_m3: dec!(120),
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    match engine.bill(ctx(&rates), &q) {
        Err(EngineError::ValidationBlocked { warnings }) => assert!(
            warnings
                .iter()
                .any(|w| w.code == "KEIN_TRINKWASSERPREIS" && w.severity == WarningSeverity::Error),
            "{warnings:?}"
        ),
        Err(other) => panic!("expected ValidationBlocked, got {other}"),
        Ok(inv) => panic!(
            "120 m³ of drinking water billed as {} EUR of Abwassergebühr alone",
            inv.netto_eur
        ),
    }
}

/// Charging energy is measured at the charge point, so `kwh_charged` is a
/// delivered quantity like any other. Without a per-kWh price the invoice
/// carries the monthly Servicegebühr and nothing for the electricity.
#[test]
fn emobility_charging_without_a_kwh_price_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product =
        serde_json::from_str(r#"{"category":"EMOBILITY","emobility_service_fee_eur":"4.99"}"#)
            .unwrap();
    let q = Quantities {
        emobility: Some(energy_billing::EmobilityMeterInput {
            kwh_charged: Some(dec!(500)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    match engine.bill(ctx(&rates), &q) {
        Err(EngineError::ValidationBlocked { warnings }) => assert!(
            warnings
                .iter()
                .any(|w| w.code == "KEIN_LADEPREIS" && w.severity == WarningSeverity::Error),
            "{warnings:?}"
        ),
        Err(other) => panic!("expected ValidationBlocked, got {other}"),
        Ok(inv) => panic!(
            "500 kWh charged, billed {} EUR — the service fee and no energy",
            inv.netto_eur
        ),
    }
}

/// A tariff that bundles charging into the flat fee says so with a zero, and is
/// billed.
#[test]
fn emobility_bundled_charging_states_a_zero_and_is_billed() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(
        r#"{"category":"EMOBILITY","emobility_service_fee_eur":"49.0","emobility_kwh_price_ct":"0.0"}"#,
    )
    .unwrap();
    let q = Quantities {
        emobility: Some(energy_billing::EmobilityMeterInput {
            kwh_charged: Some(dec!(500)),
            ..Default::default()
        }),
        ..Default::default()
    };
    product
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx(&rates), &q)
        .expect("a bundled charging tariff is a decision, not missing data");
}

/// Counted events with no price anywhere fall off the invoice. Unlike delivered
/// energy the count is also a legitimate informational figure, so this names the
/// ambiguity rather than refusing the run.
#[test]
fn service_events_without_a_price_are_flagged_but_do_not_block() {
    let rates = RegulatoryRates::default();
    let product: Product =
        serde_json::from_str(r#"{"category":"ENERGIEDIENSTLEISTUNG","service_fee_eur":"9.9"}"#)
            .unwrap();
    let q = Quantities {
        service: Some(energy_billing::ServiceMeterInput {
            event_count: Some(12),
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    let warnings = engine.validate(&ctx(&rates), &q);
    assert!(
        warnings
            .iter()
            .any(|w| w.code == "KEIN_EREIGNISPREIS" && w.severity == WarningSeverity::Warning),
        "{warnings:?}"
    );
    engine
        .bill(ctx(&rates), &q)
        .expect("an ambiguous event count must not block the run");
}

// ── The same defect on the side that PAYS ────────────────────────────────────
//
// A Gutschrift settles a measured Einspeisung. The rate it is settled at is
// mapped from the catalogue the same way an Arbeitspreis is, so it goes missing
// the same way — and the resulting document is a Rechnung over €0,00 that reads
// like a month with no production. The consumption-side twin of the same defect
// blocks the run; underpaying a generator is not the milder case.

/// A Direktvermarktungs-Gutschrift without a Marktwert settles nothing and
/// refuses rather than crediting €0,00.
#[test]
fn einspeisung_without_a_marktwert_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(
        r#"{"category":"EINSPEISUNG","vermarktungsgebuehr_ct_per_kwh":"0.3"}"#,
    )
    .unwrap();
    let q = Quantities {
        einspeisung: Some(EegMeterInput {
            einspeisung_kwh: dec!(12000),
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    assert!(
        engine
            .validate(&ctx(&rates), &q)
            .iter()
            .any(|w| w.code == "KEIN_MARKTWERT" && w.severity == WarningSeverity::Error),
        "a settlement that pays a generator nothing for measured energy must be refused"
    );
    match engine.bill(ctx(&rates), &q) {
        Err(EngineError::ValidationBlocked { .. }) => {}
        Err(other) => panic!("expected ValidationBlocked, got {other}"),
        Ok(inv) => panic!(
            "the Gutschrift settled {} EUR for 12000 kWh instead of being refused",
            inv.netto_eur
        ),
    }
}

/// A priced Marktwert satisfies the guard, and an explicit zero is a decision.
#[test]
fn a_priced_marktwert_satisfies_the_einspeisung_guard() {
    let rates = RegulatoryRates::default();
    for json in [
        r#"{"category":"EINSPEISUNG","marktwert_ct_per_kwh":"7.4"}"#,
        r#"{"category":"EINSPEISUNG","marktwert_ct_per_kwh":"0.0"}"#,
    ] {
        let product: Product = serde_json::from_str(json).unwrap();
        let q = Quantities {
            einspeisung: Some(EegMeterInput {
                einspeisung_kwh: dec!(12000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let engine = product.build_engine(&GridInput::default(), &rates);
        assert!(
            !engine
                .validate(&ctx(&rates), &q)
                .iter()
                .any(|w| w.code == "KEIN_MARKTWERT"),
            "{json} prices the feed-in and must not be flagged"
        );
    }
}

/// An EEG Gutschrift without any Vergütungssatz refuses. The Managementprämie
/// is the contractual Direktvermarktungsentgelt and settles no energy on its
/// own, so it does not satisfy the guard.
#[test]
fn eeg_without_any_verguetungssatz_is_refused() {
    let rates = RegulatoryRates::default();
    for json in [
        r#"{"category":"EEG"}"#,
        r#"{"category":"EEG","eeg_managementpraemie_ct_per_kwh":"0.4"}"#,
    ] {
        let product: Product = serde_json::from_str(json).unwrap();
        let q = Quantities {
            eeg: Some(EegMeterInput {
                einspeisung_kwh: dec!(8000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let engine = product.build_engine(&GridInput::default(), &rates);
        assert!(
            engine
                .validate(&ctx(&rates), &q)
                .iter()
                .any(|w| w.code == "KEIN_VERGUETUNGSSATZ" && w.severity == WarningSeverity::Error),
            "{json} settles no energy and must be refused"
        );
        assert!(matches!(
            engine.bill(ctx(&rates), &q),
            Err(EngineError::ValidationBlocked { .. })
        ));
    }
}

/// Any of the three Vergütungsarten satisfies the EEG guard.
#[test]
fn any_verguetungssatz_satisfies_the_eeg_guard() {
    let rates = RegulatoryRates::default();
    for json in [
        r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":"8.11"}"#,
        r#"{"category":"EEG","eeg_marktpraemie_ct_per_kwh":"2.4"}"#,
        r#"{"category":"EEG","kwkg_zuschlag_ct_per_kwh":"4.0"}"#,
        r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":"0.0"}"#,
    ] {
        let product: Product = serde_json::from_str(json).unwrap();
        let q = Quantities {
            eeg: Some(EegMeterInput {
                einspeisung_kwh: dec!(8000),
                ..Default::default()
            }),
            ..Default::default()
        };
        let engine = product.build_engine(&GridInput::default(), &rates);
        assert!(
            !engine
                .validate(&ctx(&rates), &q)
                .iter()
                .any(|w| w.code == "KEIN_VERGUETUNGSSATZ"),
            "{json} settles the energy and must not be flagged"
        );
    }
}

/// §42c: allocated community kWh with no Gutschriftsatz would bill the full grid
/// consumption with the credit missing entirely, leaving no trace on the paper.
#[test]
fn energy_sharing_without_a_credit_rate_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product =
        serde_json::from_str(r#"{"category":"SHARING","arbeitspreis_ct_per_kwh":"30.0"}"#).unwrap();
    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            ..Default::default()
        }),
        energy_share: Some(EnergyShareMeterInput {
            allocated_kwh: dec!(400),
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    assert!(
        engine.validate(&ctx(&rates), &q).iter().any(
            |w| w.code == "KEIN_SHARING_GUTSCHRIFTSATZ" && w.severity == WarningSeverity::Error
        ),
        "a dropped §42c credit overcharges the participant and must be refused"
    );
    match engine.bill(ctx(&rates), &q) {
        Err(EngineError::ValidationBlocked { .. }) => {}
        Err(other) => panic!("expected ValidationBlocked, got {other}"),
        Ok(inv) => panic!(
            "the invoice billed {} EUR with the §42c credit silently absent",
            inv.netto_eur
        ),
    }
}

/// HEMS events are counted off the system. Priced at nothing they drop out of
/// the invoice without a trace.
#[test]
fn hems_events_without_a_price_are_refused() {
    let rates = RegulatoryRates::default();
    let product: Product =
        serde_json::from_str(r#"{"category":"HEMS","hems_subscription_eur_per_month":"9.90"}"#)
            .unwrap();
    let q = Quantities {
        hems: Some(HemsMeterInput {
            months: Some(dec!(1)),
            optimization_events: Some(42),
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    assert!(
        engine
            .validate(&ctx(&rates), &q)
            .iter()
            .any(|w| w.code == "KEIN_HEMS_EREIGNISPREIS" && w.severity == WarningSeverity::Error),
        "42 recorded events billed at nothing must be refused"
    );
    assert!(matches!(
        engine.bill(ctx(&rates), &q),
        Err(EngineError::ValidationBlocked { .. })
    ));
}

/// A HEMS product carrying no price at all bills an empty document.
#[test]
fn hems_without_any_price_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(r#"{"category":"HEMS"}"#).unwrap();
    let engine = product.build_engine(&GridInput::default(), &rates);
    assert!(
        engine
            .validate(&ctx(&rates), &Quantities::default())
            .iter()
            .any(|w| w.code == "KEIN_HEMS_PREIS" && w.severity == WarningSeverity::Error)
    );
}

// ── §14a EnWG Modul 3 — the bands are one tariff ─────────────────────────────

fn waermepumpe_modul3(bands: &str) -> Product {
    serde_json::from_str(&format!(
        r#"{{"category":"WAERMEPUMPE","arbeitspreis_ct_per_kwh":"30.0",
             "sect14a_modul1_pauschale_eur_per_year":"110.0",{bands}}}"#
    ))
    .unwrap()
}

fn modul3_quantities() -> Quantities {
    Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(3000),
            metering_mode: MeteringMode::Imsys,
            ..Default::default()
        }),
        sect14a_modul3: Some(Sect14aModul3Verbrauch {
            ht_kwh: dec!(1000),
            st_kwh: dec!(1000),
            nt_kwh: dec!(1000),
        }),
        ..Default::default()
    }
}

/// A partially priced Modul 3 band triple is a refusal. The bands replace the
/// flat NNE Arbeitspreis, so an unpriced band's kWh carry no network charge at
/// all — and a rate band silently omitted from the invoice is indistinguishable
/// from one that was never priced.
#[test]
fn an_incomplete_modul3_band_triple_is_refused() {
    let rates = RegulatoryRates::default();
    for (bands, missing) in [
        (
            r#""sect14a_modul3_nne_ht_ct_per_kwh":"9.0","sect14a_modul3_nne_nt_ct_per_kwh":"3.0""#,
            "ST",
        ),
        (
            r#""sect14a_modul3_nne_ht_ct_per_kwh":"9.0","sect14a_modul3_nne_st_ct_per_kwh":"6.0""#,
            "NT",
        ),
        (
            r#""sect14a_modul3_nne_st_ct_per_kwh":"6.0","sect14a_modul3_nne_nt_ct_per_kwh":"3.0""#,
            "HT",
        ),
    ] {
        let engine = waermepumpe_modul3(bands).build_engine(&GridInput::default(), &rates);
        let warnings = engine.validate(&ctx(&rates), &modul3_quantities());
        let finding = warnings
            .iter()
            .find(|w| w.code == "MODUL3_BAND_UNVOLLSTAENDIG")
            .unwrap_or_else(|| {
                panic!("a triple missing {missing} must be a finding: {warnings:?}")
            });
        assert_eq!(finding.severity, WarningSeverity::Error);
        assert!(
            finding.message.contains(missing),
            "the finding must name the unpriced band: {}",
            finding.message
        );
        assert!(matches!(
            engine.bill(ctx(&rates), &modul3_quantities()),
            Err(EngineError::ValidationBlocked { .. })
        ));
    }
}

/// Every Modul 3 precondition keys on the whole band triple: a product that
/// prices ST and NT only is on Modul 3 and must not escape the guard that
/// forbids a flat NNE Arbeitspreis beside the bands.
#[test]
fn modul3_preconditions_key_on_the_whole_band_triple() {
    let rates = RegulatoryRates::default();
    let grid = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(7.5)),
        ..Default::default()
    };
    let product = waermepumpe_modul3(
        r#""sect14a_modul3_nne_st_ct_per_kwh":"6.0","sect14a_modul3_nne_nt_ct_per_kwh":"3.0""#,
    );
    let warnings = product
        .build_engine(&grid, &rates)
        .validate(&ctx(&rates), &modul3_quantities());
    assert!(
        warnings.iter().any(|w| w.code == "MODUL3_AND_FLAT_NNE"),
        "{warnings:?}"
    );
}

/// A complete triple bills all three bands and raises no incompleteness finding.
#[test]
fn a_complete_modul3_band_triple_bills() {
    let rates = RegulatoryRates::default();
    let product = waermepumpe_modul3(
        r#""sect14a_modul3_nne_ht_ct_per_kwh":"9.0","sect14a_modul3_nne_st_ct_per_kwh":"6.0",
           "sect14a_modul3_nne_nt_ct_per_kwh":"0.0""#,
    );
    let engine = product.build_engine(&GridInput::default(), &rates);
    assert!(
        !engine
            .validate(&ctx(&rates), &modul3_quantities())
            .iter()
            .any(|w| w.code == "MODUL3_BAND_UNVOLLSTAENDIG")
    );
    let inv = engine
        .bill(ctx(&rates), &modul3_quantities())
        .expect("a completely priced triple bills");
    assert_eq!(
        inv.positions
            .iter()
            .filter(|p| p.tags.iter().any(|t| t == "modul3"))
            .count(),
        3
    );
}

/// A measured dimming with no Spitzenleistung prices the §14a
/// Steuerungsentschädigung at nothing: both rate bases need the capacity, so the
/// compensation would leave no trace on the invoice.
#[test]
fn steuerungsentschaedigung_without_a_spitzenleistung_is_refused() {
    let rates = RegulatoryRates::default();
    for rate in [
        r#""sect14a_steuerungsentschaedigung_ct_per_kwh":"5.0""#,
        r#""sect14a_steuerungsentschaedigung_eur_per_kw_year":"40.0""#,
    ] {
        let product: Product = serde_json::from_str(&format!(
            r#"{{"category":"WAERMEPUMPE","arbeitspreis_ct_per_kwh":"30.0",{rate}}}"#
        ))
        .unwrap();
        let q = Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: dec!(3000),
                steuerung_stunden: Some(dec!(120)),
                spitzenleistung_kw: None,
                ..Default::default()
            }),
            ..Default::default()
        };
        let engine = product.build_engine(&GridInput::default(), &rates);
        assert!(
            engine.validate(&ctx(&rates), &q).iter().any(|w| w.code
                == "STEUERUNGSENTSCHAEDIGUNG_OHNE_SPITZENLEISTUNG"
                && w.severity == WarningSeverity::Error),
            "{rate}: a dimming that is compensated with nothing must be refused"
        );
        assert!(matches!(
            engine.bill(ctx(&rates), &q),
            Err(EngineError::ValidationBlocked { .. })
        ));
    }
}

/// A known Spitzenleistung credits the Steuerungsentschädigung.
#[test]
fn steuerungsentschaedigung_with_a_spitzenleistung_is_credited() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(
        r#"{"category":"WAERMEPUMPE","arbeitspreis_ct_per_kwh":"30.0",
            "sect14a_steuerungsentschaedigung_ct_per_kwh":"5.0"}"#,
    )
    .unwrap();
    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(3000),
            steuerung_stunden: Some(dec!(120)),
            spitzenleistung_kw: Some(dec!(4)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);
    assert!(
        !engine
            .validate(&ctx(&rates), &q)
            .iter()
            .any(|w| w.code == "STEUERUNGSENTSCHAEDIGUNG_OHNE_SPITZENLEISTUNG")
    );
    let inv = engine.bill(ctx(&rates), &q).expect("bills");
    assert!(inv.positions.iter().any(|p| {
        p.tags
            .iter()
            .any(|t| t == "sect14a_steuerungsentschaedigung")
    }));
}
