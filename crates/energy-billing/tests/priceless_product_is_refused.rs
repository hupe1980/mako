//! A commodity product must be able to price its commodity.
//!
//! The price fields of a `Product` are populated by mapping `productd`'s
//! `preistyp` strings onto struct fields. A renamed position, a typo in the
//! mapper, or a catalog row saved without its price maps to `None` — in
//! silence. Unguarded, the resulting invoice is not an error: it bills 1000 kWh
//! of electricity for €20.50 — the Stromsteuer and nothing else — and looks
//! entirely ordinary on paper.

use energy_billing::{
    BillingContext, BillingPeriod, EngineError, GridInput, MeterInput, Product, Quantities,
    RegulatoryRates, WarningSeverity,
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
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#,
        r#"{"category":"STROM","arbeitspreis_ht_ct_per_kwh":32.0,"arbeitspreis_nt_ct_per_kwh":24.0}"#,
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
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":0.0}"#).unwrap();
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
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":20}"#,
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
        serde_json::from_str(r#"{"category":"WASSER","schmutzwasser_eur_per_m3":2.5}"#).unwrap();
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
        serde_json::from_str(r#"{"category":"EMOBILITY","emobility_service_fee_eur":4.99}"#)
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
        r#"{"category":"EMOBILITY","emobility_service_fee_eur":49.0,"emobility_kwh_price_ct":0.0}"#,
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
        serde_json::from_str(r#"{"category":"ENERGIEDIENSTLEISTUNG","service_fee_eur":9.9}"#)
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
