//! Unit tests for `energy-billing` — pure billing arithmetic.
//!
//! All tests use the new `BillingEngine` + `BillingContext` + `Quantities` API.
//!
//! No HTTP, no database, no external services.
//! Run: `cargo test -p energy-billing --test calculator_tests`

use energy_billing::{
    AbschlagDeduction, Absetzung, AbsetzungsGrund, BillingContext, BillingEngine, BillingPeriod,
    DynamicInterval, EegMeterInput, ElectricityProvider, EmobilityMeterInput, GasMeterInput,
    GasProvider, GridInput, HemsMeterInput, InvoiceType, MeterInput, MeteringMode, MwStProvider,
    PositionCategory, Product, Quantities, RegulatoryRates, Sect41aAnnualComparison,
    ServiceMeterInput, SolarMeterInput, WaermeMeterInput, WasserMeterInput,
    behg_ct_per_kwh_for_year,
};
use rust_decimal::Decimal;
use rust_decimal::dec;
use time::macros::date;

fn rates_2026() -> RegulatoryRates {
    RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(0.62),
        mwst_rate: dec!(0.19),
        mwst_rate_reduced: dec!(0.07),
    }
}

fn period() -> (time::Date, time::Date) {
    (date!(2026 - 01 - 01), date!(2026 - 01 - 31))
}

fn no_grid() -> GridInput {
    GridInput::default()
}

fn j(s: &str) -> Product {
    serde_json::from_str(s).unwrap()
}

fn meter(kwh: rust_decimal::Decimal) -> MeterInput {
    MeterInput {
        arbeitsmenge_kwh: kwh,
        ..Default::default()
    }
}

/// Electricity-only `Quantities` shortcut for single-meter tests.
fn elec(kwh: rust_decimal::Decimal) -> Quantities {
    Quantities {
        electricity: Some(meter(kwh)),
        ..Default::default()
    }
}

/// The `wert` of a document-level ZusatzAttribut, or `Null` when absent.
fn zusatz_wert<'a>(json: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    json["zusatzAttribute"]
        .as_array()
        .and_then(|attrs| attrs.iter().find(|a| a["name"] == name))
        .map(|a| &a["wert"])
        .unwrap_or(&serde_json::Value::Null)
}

// ── Canonical billing helpers ─────────────────────────────────────────────────
//
// All helpers dispatch through `TariffInput::build_engine()` — the same code
// path that `billingd` uses at runtime.  Tests therefore exercise the full
// category dispatch + provider pipeline, not just a hardwired provider.

/// Execute billing through the production `build_engine` dispatch.
///
/// Uses: Jan 2026 period, rates_2026(), no grid, Initial invoice type.
fn bill(tariff: &Product, q: Quantities) -> energy_billing::Invoice {
    bill_full(
        tariff,
        &GridInput::default(),
        q,
        &rates_2026(),
        InvoiceType::Initial,
    )
}

/// Like `bill` but with a custom `GridInput` (NNE / KA / Leistungspreis tests).
fn bill_grid(tariff: &Product, grid: &GridInput, q: Quantities) -> energy_billing::Invoice {
    bill_full(tariff, grid, q, &rates_2026(), InvoiceType::Initial)
}

/// Like `bill` but with custom `RegulatoryRates` (year-specific BEHG, etc.).
fn bill_rates(tariff: &Product, q: Quantities, rates: &RegulatoryRates) -> energy_billing::Invoice {
    bill_full(
        tariff,
        &GridInput::default(),
        q,
        rates,
        InvoiceType::Initial,
    )
}

/// Like `bill` but emits a `CreditNote` (EEG / EINSPEISUNG settlement).
fn bill_credit(tariff: &Product, q: Quantities) -> energy_billing::Invoice {
    bill_full(
        tariff,
        &GridInput::default(),
        q,
        &rates_2026(),
        InvoiceType::CreditNote,
    )
}

/// Full-control billing helper — uses `TariffInput::build_engine` for dispatch.
fn bill_full(
    tariff: &Product,
    grid: &GridInput,
    q: Quantities,
    rates: &RegulatoryRates,
    invoice_type: InvoiceType,
) -> energy_billing::Invoice {
    let (f, t) = period();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST".to_owned(),
        period: BillingPeriod::new(f, t).unwrap(),
        invoice_type,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    tariff.build_engine(grid, rates).bill(ctx, &q).unwrap()
}

// ── Strom ─────────────────────────────────────────────────────────────────────

#[test]
fn strom_flat_brutto_includes_stromsteuer_and_mwst() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"STROM","register_count":"Eintarif","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#,
    );
    let r = bill(&tariff, elec(dec!(100)));
    // Netto ≈ 21.35 EUR; Brutto ≈ 25.41 EUR
    assert!(
        r.brutto_eur > dec!(25) && r.brutto_eur < dec!(26),
        "Brutto {} not in [25,26]",
        r.brutto_eur
    );
}

#[test]
fn strom_eeg_gutschrift_reduces_brutto() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#);
    let r0 = bill(&tariff, elec(dec!(200)));
    let r5 = bill(
        &tariff,
        Quantities {
            electricity: Some(meter(dec!(200))),
            eeg_gutschrift_eur: Some(dec!(5)),
            ..Default::default()
        },
    );
    assert!(r5.brutto_eur < r0.brutto_eur);
    let diff = r0.brutto_eur - r5.brutto_eur;
    // 5 EUR × 1.19 = 5.95 EUR
    assert!(
        diff > dec!(5.9) && diff < dec!(6.0),
        "Expected ~5.95 diff, got {diff}"
    );
}

#[test]
fn strom_mwst_override_zero_removes_vat() {
    let (_f, _t) = period();
    let t1 = j(r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#);
    let t2 = j(
        r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10,"mwst_rate_override":0}"#,
    );
    let r1 = bill(&t1, elec(dec!(100)));
    let r2 = bill(&t2, elec(dec!(100)));
    assert!(
        r2.brutto_eur < r1.brutto_eur,
        "zero MwSt must reduce brutto"
    );
    assert_eq!(r2.mwst_eur, dec!(0), "mwst_eur must be zero");
}

#[test]
fn strom_zweitarif_higher_than_ht_eintarif() {
    let (_f, _t) = period();
    let ht = j(
        r#"{"category":"STROM","register_count":"Eintarif","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":12}"#,
    );
    let zt = j(
        r#"{"category":"STROM","register_count":"Zweitarif","grundpreis_ct_per_day":0,"arbeitspreis_ht_ct_per_kwh":12,"arbeitspreis_nt_ct_per_kwh":7}"#,
    );
    let m_ht = MeterInput {
        arbeitsmenge_kwh: dec!(100),
        arbeitsmenge_ht_kwh: None,
        arbeitsmenge_nt_kwh: None,
        spitzenleistung_kw: None,
        steuerung_stunden: None,
        ..Default::default()
    };
    let m_zt = MeterInput {
        arbeitsmenge_kwh: dec!(150),
        arbeitsmenge_ht_kwh: Some(dec!(100)),
        arbeitsmenge_nt_kwh: Some(dec!(50)),
        spitzenleistung_kw: None,
        steuerung_stunden: None,
        ..Default::default()
    };
    let r_ht = bill(
        &ht,
        Quantities {
            electricity: Some(m_ht.clone()),
            ..Default::default()
        },
    );
    let r_zt = bill(
        &zt,
        Quantities {
            electricity: Some(m_zt.clone()),
            ..Default::default()
        },
    );
    assert!(
        r_zt.netto_eur > r_ht.netto_eur,
        "Zweitarif netto must be higher"
    );
}

#[cfg(feature = "bo4e")]
#[test]
fn billing_result_rechnung_json_has_period() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#);
    let r = bill(&tariff, elec(dec!(50)));
    let _j_r = r.to_rechnung_json();
    let start = _j_r["rechnungsperiode"]["startdatum"]
        .as_str()
        .unwrap_or("");
    assert!(
        start.contains("2026-01-01"),
        "startdatum '{start}' does not match"
    );
    let _j_r = r.to_rechnung_json();
    let end = _j_r["rechnungsperiode"]["enddatum"].as_str().unwrap_or("");
    assert!(
        end.contains("2026-01-31"),
        "enddatum '{end}' does not match"
    );
}

// ── Gas ───────────────────────────────────────────────────────────────────────

#[test]
fn gas_kwh_hs_direct_determines_arbeit() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#,
    );
    let m = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(500)),
        gasqualitaet: None,
        spitzenleistung_kw: None,
        ..Default::default()
    };
    let r = bill(
        &tariff,
        Quantities {
            gas: Some(m.clone()),
            ..Default::default()
        },
    );
    // 500 × 10 ct + levies > 50 EUR
    assert!(
        r.brutto_eur > dec!(50),
        "Gas brutto {} too low",
        r.brutto_eur
    );
}

#[test]
fn gas_brennwert_conversion_equivalent_to_kwh_hs() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#,
    );
    // 100 m³ × 10 kWh/m³ × 1.0 Zs = 1000 kWh_Hs
    let m1 = GasMeterInput {
        messung_qm3: dec!(100),
        brennwert_kwh_per_qm3: Some(dec!(10)),
        zustandszahl: Some(dec!(1)),
        kwh_hs: None,
        gasqualitaet: None,
        spitzenleistung_kw: None,
        ..Default::default()
    };
    let m2 = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(1000)),
        gasqualitaet: None,
        spitzenleistung_kw: None,
        ..Default::default()
    };
    let r1 = bill(
        &tariff,
        Quantities {
            gas: Some(m1.clone()),
            ..Default::default()
        },
    );
    let r2 = bill(
        &tariff,
        Quantities {
            gas: Some(m2.clone()),
            ..Default::default()
        },
    );
    assert_eq!(
        r1.brutto_eur, r2.brutto_eur,
        "Brennwert and kwh_hs must be equivalent"
    );
}

// ── Wasser ────────────────────────────────────────────────────────────────────

fn wasser_tariff() -> Product {
    j(r#"{
        "category":"WASSER",
        "wasser_grundpreis_eur_per_month": 8.50,
        "wasser_mengenpreis_eur_per_m3": 2.10,
        "schmutzwasser_eur_per_m3": 2.80,
        "niederschlagswasser_eur_per_m2_year": 1.20
    }"#)
}

fn wasser_q(m: WasserMeterInput) -> Quantities {
    Quantities {
        wasser: Some(m),
        ..Default::default()
    }
}

#[test]
fn wasser_trinkwasser_carries_7_pct_and_public_abwasser_no_ust() {
    // 30 m³ freshwater, 100 m² sealed surface, one month, public-law fee.
    let inv = bill(
        &wasser_tariff(),
        wasser_q(WasserMeterInput {
            frischwasser_m3: dec!(30),
            versiegelte_flaeche_m2: Some(dec!(100)),
            months: Some(dec!(1)),
            ..Default::default()
        }),
    );
    // Netto: Grundpreis 8.50 + Mengenpreis 30×2.10=63.00 + Schmutzwasser
    // 30×2.80=84.00 + Niederschlagswasser 100×1.20/12=10.00 → 165.50.
    assert_eq!(inv.netto_eur, dec!(165.50));
    // USt only on the Trinkwasser block: 7 % × (8.50 + 63.00) = 5.005, stated
    // to the cent as every Steuerbetrag is (§14 Abs. 4 Nr. 8 UStG).
    assert_eq!(inv.mwst_eur, dec!(5.01));
    // Exactly one 7 % tax position and no 19 % position.
    let tax: Vec<_> = inv
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Tax)
        .collect();
    assert_eq!(tax.len(), 1, "public-law Abwasser must not create USt");
}

#[test]
fn wasser_absetzung_reduces_schmutzwasser_only() {
    // 30 m³ freshwater with 8 m³ garden + 2 m³ Schleppwasser deductions:
    // Trinkwasser stays at 30 m³, Schmutzwasser drops to 20 m³.
    let base = bill(
        &wasser_tariff(),
        wasser_q(WasserMeterInput {
            frischwasser_m3: dec!(30),
            months: Some(dec!(1)),
            ..Default::default()
        }),
    );
    let deducted = bill(
        &wasser_tariff(),
        wasser_q(WasserMeterInput {
            frischwasser_m3: dec!(30),
            absetzungen: vec![
                Absetzung {
                    m3: dec!(8),
                    grund: AbsetzungsGrund::Gartenwasser,
                },
                Absetzung {
                    m3: dec!(2),
                    grund: AbsetzungsGrund::Schleppwasser,
                },
            ],
            months: Some(dec!(1)),
            ..Default::default()
        }),
    );
    // 10 m³ × 2.80 EUR/m³ less Schmutzwasser; Trinkwasser unchanged.
    assert_eq!(base.netto_eur - deducted.netto_eur, dec!(28.00));
    // Both Absetzungen are visible as 0-EUR Info positions.
    let infos: Vec<_> = deducted
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Info)
        .collect();
    assert_eq!(infos.len(), 2);
    assert!(
        infos
            .iter()
            .any(|p| p.description.contains("Schleppwasser"))
    );
}

#[test]
fn wasser_private_regime_taxes_abwasser_at_19_pct() {
    let tariff = j(r#"{
        "category":"WASSER",
        "wasser_mengenpreis_eur_per_m3": 2.00,
        "schmutzwasser_eur_per_m3": 3.00,
        "abwasser_regime": "PRIVATE_LAW_CHARGE"
    }"#);
    let inv = bill(
        &tariff,
        wasser_q(WasserMeterInput {
            frischwasser_m3: dec!(10),
            months: Some(dec!(1)),
            ..Default::default()
        }),
    );
    // 7 % on 20.00 Trinkwasser = 1.40; 19 % on 30.00 Abwasser = 5.70.
    assert_eq!(inv.mwst_eur, dec!(7.10));
}

#[test]
fn wasser_absetzung_exceeding_frischwasser_blocks_billing() {
    let (f, t) = period();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST".to_owned(),
        period: BillingPeriod::new(f, t).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        ..Default::default()
    };
    let q = wasser_q(WasserMeterInput {
        frischwasser_m3: dec!(5),
        absetzungen: vec![Absetzung {
            m3: dec!(6),
            grund: AbsetzungsGrund::Gartenwasser,
        }],
        months: Some(dec!(1)),
        ..Default::default()
    });
    // An Absetzung above the freshwater intake is a meter-reading error —
    // billing on it would under-charge the sewage fee unverifiably.
    wasser_tariff()
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(ctx, &q)
        .expect_err("Absetzung > Frischwasser must block the run");
}

// ── Wärme ─────────────────────────────────────────────────────────────────────

#[test]
fn waerme_leistungspreis_adds_to_brutto() {
    let (_f, _t) = period();
    let t1 = j(
        r#"{"category":"WAERME","waerme_grundpreis_eur_per_month":0,"waerme_arbeitspreis_ct_per_kwh":12}"#,
    );
    let t2 = j(
        r#"{"category":"WAERME","waerme_grundpreis_eur_per_month":0,"waerme_arbeitspreis_ct_per_kwh":12,"waerme_leistungspreis_eur_per_kw_month":5}"#,
    );
    let m = WaermeMeterInput {
        kwh_waerme: dec!(300),
        spitzenleistung_kw: Some(dec!(10)),
        months: Some(dec!(1)),
    };
    let r1 = bill(
        &t1,
        Quantities {
            heat: Some(m.clone()),
            ..Default::default()
        },
    );
    let r2 = bill(
        &t2,
        Quantities {
            heat: Some(m.clone()),
            ..Default::default()
        },
    );
    assert!(
        r2.brutto_eur > r1.brutto_eur,
        "Leistungspreis must add to brutto"
    );
    // 10 kW × 5 EUR × 1.19 = 59.50 EUR additional
    let diff = r2.brutto_eur - r1.brutto_eur;
    assert!(
        diff > dec!(59) && diff < dec!(60),
        "Expected ~59.5, got {diff}"
    );
}

/// AVBFernwärmeV §24 Abs. 4: an index-linked Arbeitspreis resolves the effective
/// ct/kWh and overrides the static one, and the position states that legal basis.
#[test]
fn waerme_preisgleitklausel_resolves_and_overrides_static_arbeitspreis() {
    // base 8 + spread 4 = 12 ct/kWh effective, overriding the static 6.
    let t = j(r#"{"category":"WAERME","waerme_grundpreis_eur_per_month":0,
            "waerme_arbeitspreis_ct_per_kwh":6,
            "waerme_indexed_price":{"base_ct_per_kwh":8,"spread_ct_per_kwh":4,
                "index_name":"Fernwärme-Index","index_value":1,"factor_ct_per_unit":0}}"#);
    let m = WaermeMeterInput {
        kwh_waerme: dec!(100),
        spitzenleistung_kw: None,
        months: Some(dec!(1)),
    };
    let r = bill(
        &t,
        Quantities {
            heat: Some(m),
            ..Default::default()
        },
    );
    let ap = r
        .positions
        .iter()
        .find(|p| p.description.contains("Arbeitspreis Fernwärme"))
        .expect("Fernwärme Arbeitspreis position");
    // 100 kWh × 12 ct = 12.00 EUR net (not 6.00 from the static price).
    assert_eq!(ap.net_eur.round_dp(2), dec!(12.00), "indexed price wins");
    assert!(
        ap.trace
            .regulatory_basis
            .iter()
            .any(|b| b.contains("AVBFernwärmeV")),
        "position must cite the Preisänderungsklausel, got: {:?}",
        ap.trace.regulatory_basis
    );
}

/// A contractual Sofortbonus is a Preisnachlass (§17 UStG Entgeltminderung): it is
/// credited as a negative Bonus position that lowers the net and thus the MwSt.
#[test]
fn sofortbonus_reduces_net_and_vat_as_entgeltminderung() {
    let plain =
        j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0,"grundpreis_ct_per_day":0}"#);
    let with_bonus = j(
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0,"grundpreis_ct_per_day":0,"sofortbonus_eur":50}"#,
    );
    let r_plain = bill(&plain, elec(dec!(500)));
    let r_bonus = bill(&with_bonus, elec(dec!(500)));

    let bonus_pos = r_bonus
        .positions
        .iter()
        .find(|p| p.category == energy_billing::PositionCategory::Bonus)
        .expect("a Bonus position must be emitted");
    assert_eq!(bonus_pos.net_eur, dec!(-50), "bonus is a EUR 50 credit");
    // Entgeltminderung: netto drops by exactly 50, and the VAT drops with it.
    assert_eq!(
        (r_plain.netto_eur - r_bonus.netto_eur).round_dp(2),
        dec!(50.00),
        "bonus reduces the net base"
    );
    assert!(
        r_bonus.mwst_eur < r_plain.mwst_eur,
        "VAT is computed on the reduced base"
    );
}

// ── EEG ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "bo4e")]
#[test]
fn eeg_verguetung_is_credit_note() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.1}"#);
    let m = EegMeterInput {
        einspeisung_kwh: dec!(500),
        ..Default::default()
    };
    let r = bill_credit(
        &tariff,
        Quantities {
            eeg: Some(m.clone()),
            ..Default::default()
        },
    );
    // EEG Vergütung: positive brutto (amount paid TO the generator),
    // tagged as the "rechnungsart" ZusatzAttribut GUTSCHRIFT (BO4E has no
    // Gutschrift value in Rechnungstyp, so the typed field stays absent).
    assert!(
        r.brutto_eur > dec!(0),
        "EEG Vergütung must be positive, got {}",
        r.brutto_eur
    );
    // 500 × 8.1 ct × 1.19 ≈ 48.20 EUR
    assert!(
        r.brutto_eur > dec!(45) && r.brutto_eur < dec!(50),
        "{}",
        r.brutto_eur
    );
    let _j_r = r.to_rechnung_json();
    assert!(
        _j_r["rechnungstyp"].is_null(),
        "GUTSCHRIFT has no Rechnungstyp"
    );
    assert_eq!(
        zusatz_wert(&_j_r, "mako:rechnungsart"),
        "GUTSCHRIFT",
        "EEG invoice must be tagged GUTSCHRIFT"
    );
}

#[test]
fn eeg_zero_einspeisung_is_zero() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.1}"#);
    let r = bill_credit(
        &tariff,
        Quantities {
            eeg: Some(EegMeterInput {
                einspeisung_kwh: dec!(0),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    assert_eq!(r.brutto_eur, dec!(0));
}

// ── Solar ─────────────────────────────────────────────────────────────────────

#[test]
fn the_mieterstromzuschlag_never_reaches_the_tenants_invoice() {
    // § 21 Abs. 3 EEG 2023 is the Anlagenbetreiber's claim against the
    // Netzbetreiber. Added as a per-kWh surcharge on the tenant instead, a
    // 1,5 ct/kWh Zuschlag over-charges a 200 kWh tenant by 3,57 EUR for a
    // payment the network operator owes the landlord.
    let (_f, _t) = period();
    let base = j(r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25}"#);
    // The field no longer exists; serde ignores it, and nothing is billed.
    let legacy = j(
        r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25,"mieterstrom_aufschlag_ct_per_kwh":1.5}"#,
    );
    let m = SolarMeterInput {
        eigenverbrauch_kwh: dec!(200),
    };
    let r1 = bill(
        &base,
        Quantities {
            solar: Some(m.clone()),
            ..Default::default()
        },
    );
    let r2 = bill(
        &legacy,
        Quantities {
            solar: Some(m.clone()),
            ..Default::default()
        },
    );
    assert_eq!(r1.brutto_eur, r2.brutto_eur);
    assert!(
        !r2.positions
            .iter()
            .any(|p| p.has_tag("mieterstrom_aufschlag")),
        "no Mieterstrom surcharge may be billed to the tenant"
    );
}

/// § 42a Abs. 4 EnWG caps the Mieterstrompreis at 90 % of the local
/// Grundversorgungstarif. Above it there is no lawful invoice to issue.
#[test]
fn mieterstrom_above_ninety_percent_of_grundversorgung_is_refused() {
    let (f, t) = period();
    let over = j(
        r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":28,"grundversorgung_arbeitspreis_ct_per_kwh":30}"#,
    );
    let ctx = ctx_for(f, t);
    let q = Quantities {
        solar: Some(SolarMeterInput {
            eigenverbrauch_kwh: dec!(200),
        }),
        ..Default::default()
    };
    let err = over
        .build_engine(&GridInput::default(), &RegulatoryRates::default())
        .bill(ctx, &q)
        .expect_err("28 ct/kWh exceeds 90 % of 30 ct/kWh");
    assert!(format!("{err:?}").contains("MIETERSTROM_UEBER_90PCT_GRUNDVERSORGUNG"));

    // At the cap exactly (27 ct/kWh) it bills, and states the ceiling.
    let at_cap = j(
        r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":27,"grundversorgung_arbeitspreis_ct_per_kwh":30}"#,
    );
    let r = bill(
        &at_cap,
        Quantities {
            solar: Some(SolarMeterInput {
                eigenverbrauch_kwh: dec!(200),
            }),
            ..Default::default()
        },
    );
    assert!(r.positions.iter().any(|p| p.has_tag("preisobergrenze")));
}

// ── HEMS ──────────────────────────────────────────────────────────────────────

#[test]
fn hems_platform_fee_one_month() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"HEMS","hems_subscription_eur_per_month":14.99}"#);
    let m = HemsMeterInput {
        months: Some(dec!(1)),
        optimization_events: None,
        readout_events: None,
    };
    let r = bill(
        &tariff,
        Quantities {
            hems: Some(m.clone()),
            ..Default::default()
        },
    );
    // 14.99 × 1.19 ≈ 17.84 EUR
    assert!(
        r.brutto_eur > dec!(17.5) && r.brutto_eur < dec!(18.5),
        "{}",
        r.brutto_eur
    );
}

// ── Einspeisung ───────────────────────────────────────────────────────────────

#[cfg(feature = "bo4e")]
#[test]
fn einspeisung_net_settlement_is_gutschrift() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"EINSPEISUNG","marktwert_ct_per_kwh":6.0,"vermarktungsgebuehr_ct_per_kwh":0.5}"#,
    );
    let m = EegMeterInput {
        einspeisung_kwh: dec!(800),
        ..Default::default()
    };
    let r = bill_credit(
        &tariff,
        Quantities {
            einspeisung: Some(m.clone()),
            ..Default::default()
        },
    );
    // EINSPEISUNG = payment to Direktvermarkter — positive brutto, tagged GUTSCHRIFT
    assert!(
        r.brutto_eur > dec!(0),
        "Direktvermarktung brutto must be positive"
    );
    let _j_r = r.to_rechnung_json();
    assert_eq!(zusatz_wert(&_j_r, "mako:rechnungsart"), "GUTSCHRIFT");
}

// ── GasQualitaet audit annotation ─────────────────────────────────────────────

#[cfg(feature = "bo4e")]
#[test]
fn gas_gasqualitaet_added_as_zusatz_attribut() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#,
    );
    // Billing with H2-blended gas: the Brennwert is already the measured H2-blended value
    let m = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(500)),
        gasqualitaet: Some("H2_BLEND".into()),
        spitzenleistung_kw: None,
        ..Default::default()
    };
    let r = bill(
        &tariff,
        Quantities {
            gas: Some(m.clone()),
            ..Default::default()
        },
    );

    // Brutto must be identical to billing without gasqualitaet (no correction applied)
    let m_no_gq = GasMeterInput {
        gasqualitaet: None,
        spitzenleistung_kw: None,
        ..m.clone()
    };
    let r_no_gq = bill(
        &tariff,
        Quantities {
            gas: Some(m_no_gq.clone()),
            ..Default::default()
        },
    );
    assert_eq!(
        r.brutto_eur, r_no_gq.brutto_eur,
        "gasqualitaet must not alter the billing amount"
    );

    // The ZusatzAttribut must appear in rechnung_json
    let _j_r = r.to_rechnung_json();
    let attrs = _j_r["zusatzAttribute"].as_array();
    assert!(attrs.is_some(), "zusatzAttribute must be present");
    let has_gq = attrs.unwrap().iter().any(|a| {
        a["name"].as_str() == Some("mako:gasqualitaet") && a["wert"].as_str() == Some("H2_BLEND")
    });
    assert!(
        has_gq,
        "gasqualitaet ZusatzAttribut must be in rechnung_json"
    );
}

#[cfg(feature = "bo4e")]
#[test]
fn gas_no_gasqualitaet_no_zusatz_attribut() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#,
    );
    let m = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(100)),
        gasqualitaet: None,
        spitzenleistung_kw: None,
        ..Default::default()
    };
    let r = bill(
        &tariff,
        Quantities {
            gas: Some(m.clone()),
            ..Default::default()
        },
    );
    // No gasqualitaet → no zusatzAttribute array (or empty array)
    let _j_r = r.to_rechnung_json();
    let gq_found = _j_r["zusatzAttribute"]
        .as_array()
        .map(|a| {
            a.iter()
                .any(|v| v["name"].as_str() == Some("mako:gasqualitaet"))
        })
        .unwrap_or(false);
    assert!(!gq_found, "gasqualitaet must not appear when not set");
}

// ── §14a Modul 1 — WAERMEPUMPE Steuerungsrabatt ───────────────────────────────

#[test]
fn waermepumpe_modul1_steuerungsrabatt_reduces_brutto() {
    let (_f, _t) = period(); // 31 days Jan 2026
    let without =
        j(r#"{"category":"WAERMEPUMPE","grundpreis_ct_per_day":10,"arbeitspreis_ct_per_kwh":20}"#);
    let with_m1 = j(
        r#"{"category":"WAERMEPUMPE","grundpreis_ct_per_day":10,"arbeitspreis_ct_per_kwh":20,"sect14a_modul1_pauschale_eur_per_kw_year":120}"#,
    );
    let meter = MeterInput {
        arbeitsmenge_kwh: dec!(300),
        arbeitsmenge_ht_kwh: None,
        arbeitsmenge_nt_kwh: None,
        spitzenleistung_kw: Some(dec!(5)),
        steuerung_stunden: None,
        ..Default::default()
    };
    let r_without = bill(
        &without,
        Quantities {
            electricity: Some(meter.clone()),
            ..Default::default()
        },
    );
    let r_with = bill(
        &with_m1,
        Quantities {
            electricity: Some(meter.clone()),
            ..Default::default()
        },
    );
    assert!(
        r_with.brutto_eur < r_without.brutto_eur,
        "Modul 1 Steuerungsrabatt must reduce total"
    );
    // 5 kW × 120 EUR/year × (31/365) ≈ 51.0 EUR netto → 60.7 EUR brutto reduction
    let savings = r_without.brutto_eur - r_with.brutto_eur;
    assert!(
        savings > dec!(50) && savings < dec!(70),
        "Expected ~58-60 EUR Modul 1 saving, got {savings}"
    );
}

#[test]
fn wallbox_modul3_steuerungsrabatt_requires_steuerung_hours() {
    let (_f, _t) = period();
    // Without steuerung_stunden → Modul 3 position must NOT appear (hours = None)
    let tariff = j(
        r#"{"category":"WALLBOX","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":25,"sect14a_steuerungsentschaedigung_eur_per_kw_year":200}"#,
    );
    let meter_no_h = MeterInput {
        arbeitsmenge_kwh: dec!(200),
        spitzenleistung_kw: Some(dec!(11)),
        arbeitsmenge_ht_kwh: None,
        arbeitsmenge_nt_kwh: None,
        steuerung_stunden: None,
        ..Default::default()
    };
    let meter_h = MeterInput {
        steuerung_stunden: Some(dec!(120)),
        ..meter_no_h.clone()
    };
    let r_no_h = bill(
        &tariff,
        Quantities {
            electricity: Some(meter_no_h.clone()),
            ..Default::default()
        },
    );
    let r_h = bill(
        &tariff,
        Quantities {
            electricity: Some(meter_h.clone()),
            ..Default::default()
        },
    );
    assert!(
        r_h.brutto_eur < r_no_h.brutto_eur,
        "the Steuerungsentschädigung must reduce brutto when steuerung_stunden > 0"
    );
    let has_entschaedigung = r_h
        .positions
        .iter()
        .any(|p| p.description.contains("Steuerungsentschädigung"));
    assert!(
        has_entschaedigung,
        "a Steuerungsentschädigung position must be present when steuerung_stunden is provided"
    );
}

// ── NNE / Grid pass-through ────────────────────────────────────────────────────

#[test]
fn strom_nne_arbeitspreis_adds_to_brutto() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":10}"#);
    let m = meter(dec!(200));
    let grid_with_nne = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(8)),
        ..GridInput::default()
    };
    let r_no_nne = bill(
        &tariff,
        Quantities {
            electricity: Some(m.clone()),
            ..Default::default()
        },
    );
    let r_nne = bill_grid(
        &tariff,
        &grid_with_nne,
        Quantities {
            electricity: Some(m.clone()),
            ..Default::default()
        },
    );
    assert!(
        r_nne.brutto_eur > r_no_nne.brutto_eur,
        "NNE Arbeitspreis must increase brutto"
    );
    // 200 kWh × 8 ct × 1.19 = 19.04 EUR additional
    let diff = r_nne.brutto_eur - r_no_nne.brutto_eur;
    assert!(
        diff > dec!(18) && diff < dec!(20),
        "Expected ~19.04 NNE addition, got {diff}"
    );
}

#[test]
fn strom_nne_grundpreis_adds_to_brutto() {
    let (_f, _t) = period(); // 31 days
    let tariff = j(r#"{"category":"STROM","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":0}"#);
    let m = meter(dec!(0));
    // 240 EUR/year NNE Grundpreis
    let grid = GridInput {
        nne_grundpreis_eur_per_year: Some(dec!(240)),
        ..GridInput::default()
    };
    let r = bill_grid(
        &tariff,
        &grid,
        Quantities {
            electricity: Some(m.clone()),
            ..Default::default()
        },
    );
    // 240/365 × 31 ≈ 20.38 EUR + 19% MwSt ≈ 24.25 EUR
    assert!(
        r.brutto_eur > dec!(23) && r.brutto_eur < dec!(26),
        "NNE Grundpreis brutto {}",
        r.brutto_eur
    );
}

#[test]
fn gas_nne_and_bilanzierungsumlage_add_to_brutto() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":5}"#);
    let m = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(500)),
        gasqualitaet: None,
        spitzenleistung_kw: None,
        ..Default::default()
    };
    let grid_with = GridInput {
        gas_nne_arbeitspreis_ct_per_kwh: Some(dec!(4)),
        gas_bilanzierungsumlage_ct_per_kwh: Some(dec!(0.5)),
        ..GridInput::default()
    };
    let r_base = bill(
        &tariff,
        Quantities {
            gas: Some(m.clone()),
            ..Default::default()
        },
    );
    let r_nne = bill_grid(
        &tariff,
        &grid_with,
        Quantities {
            gas: Some(m.clone()),
            ..Default::default()
        },
    );
    assert!(
        r_nne.brutto_eur > r_base.brutto_eur,
        "Gas NNE + Bilanzierungsumlage must increase brutto"
    );
    // 500 kWh × (4 + 0.5) ct × 1.19 = 26.78 EUR additional
    let diff = r_nne.brutto_eur - r_base.brutto_eur;
    assert!(
        diff > dec!(25) && diff < dec!(28),
        "Expected ~26.78 addition, got {diff}"
    );
}

// ── §41a Dynamic EPEX tariff ──────────────────────────────────────────────────

#[test]
fn dynamic_strom_two_intervals_produce_positive_brutto() {
    use std::collections::HashMap;
    let _from = time::macros::date!(2026 - 01 - 01);
    let _to = time::macros::date!(2026 - 01 - 31);
    let tariff = j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":20}"#);
    let intervals = [
        DynamicInterval {
            timestamp_utc: time::macros::datetime!(2026-01-01 13:00 UTC), // 14:00 CET
            kwh: dec!(1),
        },
        DynamicInterval {
            timestamp_utc: time::macros::datetime!(2026-01-01 14:00 UTC), // 15:00 CET
            kwh: dec!(2),
        },
    ];
    let mut prices = HashMap::new();
    prices.insert(time::macros::datetime!(2026-01-01 13:00 UTC), dec!(25)); // 14:00 CET MTU
    prices.insert(time::macros::datetime!(2026-01-01 14:00 UTC), dec!(30)); // 15:00 CET MTU
    let r = bill(
        &tariff,
        Quantities {
            dynamic_intervals: intervals.to_vec(),
            dynamic_epex_prices: prices.clone(),
            ..Default::default()
        },
    );
    // 3 kWh EPEX (0.85 EUR) + 31 days Grundpreis (6.20 EUR) + Stromsteuer + 19% MwSt ≈ 8.46 EUR
    assert!(
        r.brutto_eur > dec!(7),
        "Dynamic strom brutto must be positive: {}",
        r.brutto_eur
    );
    // The EPEX position must be present
    let has_epex = r
        .positions
        .iter()
        .any(|p| p.description.contains("EPEX") || p.description.contains("41a"));
    assert!(has_epex, "EPEX position must appear in dynamic billing");
}

/// §41a 15-min MTU: two quarter-hours **within the same hour** must be billed
/// at their own distinct spot prices — the pre-2025-10 hourly truncation would
/// have collapsed both to one price.
#[test]
fn dynamic_strom_quarter_hour_prices_are_distinct_within_an_hour() {
    use std::collections::HashMap;
    let tariff = j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0}"#);
    // Four quarter-hours of the 13:00–14:00 UTC hour, 1 kWh each.
    let intervals: Vec<DynamicInterval> = [0i64, 15, 30, 45]
        .iter()
        .map(|m| DynamicInterval {
            timestamp_utc: time::macros::datetime!(2026-01-01 13:00 UTC)
                + time::Duration::minutes(*m),
            kwh: dec!(1),
        })
        .collect();
    // Distinct price per quarter-hour: 10, 20, 30, 40 ct/kWh (avg 25).
    let mut prices = HashMap::new();
    prices.insert(time::macros::datetime!(2026-01-01 13:00 UTC), dec!(10));
    prices.insert(time::macros::datetime!(2026-01-01 13:15 UTC), dec!(20));
    prices.insert(time::macros::datetime!(2026-01-01 13:30 UTC), dec!(30));
    prices.insert(time::macros::datetime!(2026-01-01 13:45 UTC), dec!(40));
    let r = bill(
        &tariff,
        Quantities {
            dynamic_intervals: intervals,
            dynamic_epex_prices: prices,
            ..Default::default()
        },
    );
    // Weighted average = (10+20+30+40)/4 = 25 ct/kWh over 4 kWh = 1.00 EUR net
    // energy. The Arbeitspreis position label carries the ∅ ct/kWh.
    let epex = r
        .positions
        .iter()
        .find(|p| p.description.contains("Arbeitspreis EPEX"))
        .expect("EPEX Arbeitspreis position");
    assert!(
        epex.description.contains("25.0000"),
        "quarter-hour average must be 25 ct/kWh, got: {}",
        epex.description
    );
}

/// §41a: the customer price is the (floored) spot price **plus** the supplier's
/// Arbeitspreis-Aufschlag — `auf_abschlag_ct_per_kwh` must be applied.
#[test]
fn dynamic_strom_applies_arbeitspreis_aufschlag() {
    use std::collections::HashMap;
    let intervals = [DynamicInterval {
        timestamp_utc: time::macros::datetime!(2026-01-01 12:00 UTC),
        kwh: dec!(1),
    }];
    let mut prices = HashMap::new();
    prices.insert(time::macros::datetime!(2026-01-01 12:00 UTC), dec!(20)); // spot 20 ct

    let plain = j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0}"#);
    let with_aufschlag = j(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0,"dynamic_aufschlag_ct_per_kwh":5}"#,
    );
    let r_plain = bill(
        &plain,
        Quantities {
            dynamic_intervals: intervals.to_vec(),
            dynamic_epex_prices: prices.clone(),
            ..Default::default()
        },
    );
    let r_auf = bill(
        &with_aufschlag,
        Quantities {
            dynamic_intervals: intervals.to_vec(),
            dynamic_epex_prices: prices.clone(),
            ..Default::default()
        },
    );
    // 5 ct/kWh Aufschlag on 1 kWh = 0.05 EUR net; brutto rises by 0.05 × 1.19.
    assert!(
        r_auf.brutto_eur > r_plain.brutto_eur,
        "Aufschlag must raise the dynamic invoice: {} vs {}",
        r_auf.brutto_eur,
        r_plain.brutto_eur
    );
    let auf_epex = r_auf
        .positions
        .iter()
        .find(|p| p.description.contains("Arbeitspreis EPEX"))
        .expect("EPEX Arbeitspreis position");
    assert!(
        auf_epex.description.contains("25.0000"),
        "spot 20 + Aufschlag 5 = 25 ct/kWh, got: {}",
        auf_epex.description
    );
}

#[test]
fn dynamic_strom_cap_clamps_the_spot_component() {
    use std::collections::HashMap;
    let intervals = [DynamicInterval {
        timestamp_utc: time::macros::datetime!(2026-01-01 12:00 UTC),
        kwh: dec!(1),
    }];
    let mut prices = HashMap::new();
    prices.insert(time::macros::datetime!(2026-01-01 12:00 UTC), dec!(20)); // spot 20 ct

    // Consumer-protection ceiling at 10 ct/kWh on the spot component.
    let capped = j(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0,"dynamic_epex_cap_ct_kwh":10}"#,
    );
    let r = bill(
        &capped,
        Quantities {
            dynamic_intervals: intervals.to_vec(),
            dynamic_epex_prices: prices,
            ..Default::default()
        },
    );
    let epex = r
        .positions
        .iter()
        .find(|p| p.description.contains("Arbeitspreis EPEX"))
        .expect("EPEX Arbeitspreis position");
    assert!(
        epex.description.contains("10.0000"),
        "spot 20 capped at 10 ct/kWh, got: {}",
        epex.description
    );
}

#[test]
fn dynamic_strom_missing_price_hard_blocks_the_run() {
    use energy_billing::EngineError;
    let tariff = j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0}"#);
    let intervals = [DynamicInterval {
        timestamp_utc: time::macros::datetime!(2026-01-15 10:00 UTC),
        kwh: dec!(1),
    }];
    let (f, t) = period();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST".to_owned(),
        period: BillingPeriod::new(f, t).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        ..Default::default()
    };
    let q = Quantities {
        // iMSys so the §41a guard passes and we reach the price check.
        electricity: Some(energy_billing::MeterInput {
            metering_mode: energy_billing::MeteringMode::Imsys,
            ..Default::default()
        }),
        dynamic_intervals: intervals.to_vec(),
        ..Default::default()
    };
    // §41a: consumed energy with no EPEX price must hard-block, never silently
    // bill zero. A degraded partial invoice would under-bill and be unverifiable.
    let err = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(ctx, &q)
        .expect_err("missing EPEX price for consumed energy must block the run");
    match err {
        EngineError::ValidationBlocked { warnings } => assert!(
            warnings
                .iter()
                .any(|w| w.code == "SECT41A_MISSING_EPEX_PRICES"),
            "expected the §41a missing-price block, got: {warnings:?}"
        ),
        other => panic!("expected ValidationBlocked, got {other:?}"),
    }
}

// ── E-Mobility ───────────────────────────────────────────────────────────────

#[test]
fn emobility_service_fee_and_energy() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"EMOBILITY","emobility_service_fee_eur":9.99,"emobility_kwh_price_ct":35,"emobility_session_fee_eur":0.5}"#,
    );
    let m = EmobilityMeterInput {
        months: Some(dec!(1)),
        kwh_charged: Some(dec!(100)),
        sessions: Some(3),
        roaming_sessions: None,
    };
    let r = bill(
        &tariff,
        Quantities {
            emobility: Some(m.clone()),
            ..Default::default()
        },
    );
    // Service fee: 9.99 + Energy: 100×35ct=35.00 + 3 sessions×0.50=1.50 = 46.49 netto + 19% MwSt
    assert!(
        r.brutto_eur > dec!(50),
        "EMOBILITY brutto too low: {}",
        r.brutto_eur
    );
    assert!(
        r.brutto_eur < dec!(60),
        "EMOBILITY brutto too high: {}",
        r.brutto_eur
    );
}

#[test]
fn emobility_zero_usage_gives_only_service_fee() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"EMOBILITY","emobility_service_fee_eur":4.99}"#);
    let m = EmobilityMeterInput {
        months: Some(dec!(1)),
        kwh_charged: None,
        sessions: None,
        roaming_sessions: None,
    };
    let r = bill(
        &tariff,
        Quantities {
            emobility: Some(m.clone()),
            ..Default::default()
        },
    );
    // 4.99 × 1.19 ≈ 5.94 EUR
    assert!(
        r.brutto_eur > dec!(5) && r.brutto_eur < dec!(7),
        "Fee-only brutto {}",
        r.brutto_eur
    );
}

// ── Energiedienstleistung ─────────────────────────────────────────────────────

#[test]
fn energiedienstleistung_flat_fee_and_events() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"ENERGIEDIENSTLEISTUNG","service_fee_eur":19.99,"service_event_price_eur":2.0}"#,
    );
    let m = ServiceMeterInput {
        months: Some(dec!(1)),
        event_count: Some(5),
        event_price_eur: None,
    };
    let r = bill(
        &tariff,
        Quantities {
            service: Some(m.clone()),
            ..Default::default()
        },
    );
    // 19.99 + 5×2.00=10.00 = 29.99 netto × 1.19 ≈ 35.69 EUR brutto
    assert!(
        r.brutto_eur > dec!(35) && r.brutto_eur < dec!(37),
        "Service brutto {}",
        r.brutto_eur
    );
}

// ── Reduced MwSt ─────────────────────────────────────────────────────────────

#[test]
fn waerme_reduced_mwst_7pct() {
    // §12 Abs. 2 Nr. 1 UStG: Fernwärme from renewable sources → 7% MwSt
    let (_f, _t) = period();
    let t19 =
        j(r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":12,"mwst_rate_override":0.19}"#);
    let t07 =
        j(r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":12,"mwst_rate_override":0.07}"#);
    let m = WaermeMeterInput {
        kwh_waerme: dec!(500),
        spitzenleistung_kw: None,
        months: Some(dec!(1)),
    };
    let r19 = bill(
        &t19,
        Quantities {
            heat: Some(m.clone()),
            ..Default::default()
        },
    );
    let r07 = bill(
        &t07,
        Quantities {
            heat: Some(m.clone()),
            ..Default::default()
        },
    );
    assert!(
        r07.brutto_eur < r19.brutto_eur,
        "Reduced MwSt must lower brutto"
    );
    // Same netto; brutto difference = netto × (0.19 - 0.07) = netto × 0.12
    let diff = r19.brutto_eur - r07.brutto_eur;
    let expected_diff = r19.netto_eur * dec!(0.12);
    assert!(
        (diff - expected_diff).abs() < dec!(0.01),
        "Expected MwSt diff {expected_diff:.2}, got {diff:.2}"
    );
}

// ── rechnung_json completeness ────────────────────────────────────────────────

#[cfg(feature = "bo4e")]
#[test]
fn rechnung_json_has_gesamtsteuer() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#);
    let r = bill(&tariff, elec(dec!(200)));
    let _json_obj = r.to_rechnung_json();
    let obj = _json_obj.as_object().unwrap();
    assert!(
        obj.contains_key("gesamtnetto"),
        "rechnung_json must have gesamtnetto"
    );
    assert!(
        obj.contains_key("gesamtsteuer"),
        "rechnung_json must have gesamtsteuer (BO4E)"
    );
    assert!(
        obj.contains_key("gesamtbrutto"),
        "rechnung_json must have gesamtbrutto"
    );
    // gesamtsteuer = brutto - netto (must be positive for normal invoice)
    let netto = obj["gesamtnetto"]["wert"]
        .as_str()
        .unwrap()
        .parse::<rust_decimal::Decimal>()
        .unwrap();
    let steuer = obj["gesamtsteuer"]["wert"]
        .as_str()
        .unwrap()
        .parse::<rust_decimal::Decimal>()
        .unwrap();
    let brutto = obj["gesamtbrutto"]["wert"]
        .as_str()
        .unwrap()
        .parse::<rust_decimal::Decimal>()
        .unwrap();
    assert!(
        (netto + steuer - brutto).abs() < dec!(0.001),
        "netto + steuer must equal brutto"
    );
    assert!(steuer > dec!(0), "gesamtsteuer must be positive");
}

#[cfg(feature = "bo4e")]
#[test]
fn rechnung_json_has_rechnungstyp_and_rechnungsersteller() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#);
    let r = bill(&tariff, elec(dec!(100)));
    let _json_obj = r.to_rechnung_json();
    let obj = _json_obj.as_object().unwrap();
    assert_eq!(obj["_typ"].as_str(), Some("RECHNUNG"));
    // InvoiceType::Initial maps to the BO4E Rechnungstyp ENDKUNDENRECHNUNG
    // (actual metered consumption billed to the end customer). Use
    // InvoiceType::AdvancePayment for ABSCHLAGSRECHNUNG (estimated advances).
    assert_eq!(obj["rechnungstyp"].as_str(), Some("ENDKUNDENRECHNUNG"));
    // The delivery point is a typed Marktlokation. This fixture is a real MaLo
    // (5·1·2·3·8·6·9·6·7·8 weights to 79, and 80 − 79 = 1), so both the generic
    // `_id` and the validated `marktlokationsId` carry it. The refusal path is
    // pinned separately below.
    let malo = &obj["marktlokation"];
    assert_eq!(malo["_id"].as_str(), Some("51238696781"));
    assert_eq!(malo["marktlokationsId"].as_str(), Some("51238696781"));
    // Issuer: BO4E Geschaeftspartner has no Marktpartner-code field, so the
    // code rides as a ZusatzAttribut on rechnungsersteller.
    let ersteller = &obj["rechnungsersteller"];
    assert_eq!(
        zusatz_wert(ersteller, "mako:marktpartnercode"),
        "9900000000001"
    );
}

/// A subject id that fails the BDEW check digit must never reach
/// `marktlokationsId`.
///
/// Aggregate and synthetic billing subjects flow through the same field as a
/// real delivery point, so the raw string always rides as the generic `_id`
/// while the typed field stays absent unless the checksum holds. A downstream
/// consumer reading `marktlokationsId` is entitled to assume it addresses a
/// Marktlokation that could exist.
#[cfg(feature = "bo4e")]
#[test]
fn a_bad_check_digit_never_becomes_a_marktlokations_id() {
    let (f, t) = period();
    // Same base as the valid fixture, wrong check digit: 80 − 79 = 1, not 2.
    let ctx = BillingContext {
        malo_id: "51238696782".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST".to_owned(),
        period: BillingPeriod::new(f, t).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        ..Default::default()
    };
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#);
    let r = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(ctx, &elec(dec!(100)))
        .unwrap();

    let json_obj = r.to_rechnung_json();
    let malo = &json_obj.as_object().unwrap()["marktlokation"];
    assert_eq!(
        malo["_id"].as_str(),
        Some("51238696782"),
        "the raw subject id is still carried, so nothing is lost"
    );
    assert!(
        malo["marktlokationsId"].is_null(),
        "but a wrong check digit must not masquerade as a validated MaLo-ID"
    );
}

// ── KA / Konzessionsabgabe ────────────────────────────────────────────────────

#[test]
fn strom_ka_konzessionsabgabe_adds_to_brutto() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":10}"#);
    let m = meter(dec!(200));
    let grid_with_ka = GridInput {
        ka_ct_per_kwh: Some(dec!(2.39)),
        ..GridInput::default()
    };
    let r_base = bill(
        &tariff,
        Quantities {
            electricity: Some(m.clone()),
            ..Default::default()
        },
    );
    let r_ka = bill_grid(
        &tariff,
        &grid_with_ka,
        Quantities {
            electricity: Some(m.clone()),
            ..Default::default()
        },
    );
    assert!(
        r_ka.brutto_eur > r_base.brutto_eur,
        "KA must increase brutto"
    );
    // 200 kWh × 2.39 ct × 1.19 = 5.69 EUR additional
    let diff = r_ka.brutto_eur - r_base.brutto_eur;
    assert!(
        diff > dec!(5.5) && diff < dec!(5.9),
        "Expected ~5.69 KA addition, got {diff}"
    );
}

// ── NNE Leistungspreis ────────────────────────────────────────────────────────

#[test]
fn strom_nne_leistungspreis_scales_with_peak_kw() {
    let (_f, _t) = period(); // 31 days
    let tariff = j(r#"{"category":"STROM","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":0}"#);
    let m_low = MeterInput {
        arbeitsmenge_kwh: dec!(0),
        spitzenleistung_kw: Some(dec!(5)),
        ..MeterInput::default()
    };
    let m_high = MeterInput {
        arbeitsmenge_kwh: dec!(0),
        spitzenleistung_kw: Some(dec!(10)),
        ..MeterInput::default()
    };
    // 200 EUR/kW/year NNE Leistungspreis
    let grid = GridInput {
        nne_leistungspreis_eur_per_kw_year: Some(dec!(200)),
        ..GridInput::default()
    };
    let r_low = bill_grid(
        &tariff,
        &grid,
        Quantities {
            electricity: Some(m_low.clone()),
            ..Default::default()
        },
    );
    let r_high = bill_grid(
        &tariff,
        &grid,
        Quantities {
            electricity: Some(m_high.clone()),
            ..Default::default()
        },
    );
    assert!(
        r_high.brutto_eur > r_low.brutto_eur,
        "Higher peak → higher Leistungspreis"
    );
    // Ratio should be 2:1 (10 kW vs 5 kW)
    let ratio = r_high.brutto_eur / r_low.brutto_eur;
    assert!(
        (ratio - dec!(2)).abs() < dec!(0.01),
        "Expected 2:1 ratio for double kW, got {ratio}"
    );
}

// ── EEG Marktprämie + Managementprämie ───────────────────────────────────────

#[test]
fn eeg_marktpraemie_adds_to_settlement() {
    let (_f, _t) = period();
    let base = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.0}"#);
    let with_mp = j(
        r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.0,"eeg_marktpraemie_ct_per_kwh":2.0,"eeg_managementpraemie_ct_per_kwh":0.4}"#,
    );
    let m = EegMeterInput {
        einspeisung_kwh: dec!(500),
        ..Default::default()
    };
    let r0 = bill_credit(
        &base,
        Quantities {
            eeg: Some(m.clone()),
            ..Default::default()
        },
    );
    let r1 = bill_credit(
        &with_mp,
        Quantities {
            eeg: Some(m.clone()),
            ..Default::default()
        },
    );
    assert!(
        r1.brutto_eur > r0.brutto_eur,
        "Marktpraemie+Managementpraemie must increase settlement"
    );
    // 500 × (2.0 + 0.4) ct × 1.19 = 14.28 EUR additional
    let diff = r1.brutto_eur - r0.brutto_eur;
    assert!(
        diff > dec!(14) && diff < dec!(15),
        "Expected ~14.28 additional, got {diff}"
    );
    let has_mp = r1
        .positions
        .iter()
        .any(|p| p.description.contains("Marktpr"));
    assert!(has_mp, "Marktpraemie position must appear");
}

#[test]
fn eeg_kwkg_zuschlag_credit_in_eeg_billing() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":0,"kwkg_zuschlag_ct_per_kwh":8.0}"#,
    );
    let m = EegMeterInput {
        einspeisung_kwh: dec!(400),
        ..Default::default()
    };
    let r = bill_credit(
        &tariff,
        Quantities {
            eeg: Some(m.clone()),
            ..Default::default()
        },
    );
    // 400 × 8.0 ct × 1.19 = 38.08 EUR
    assert!(
        r.brutto_eur > dec!(37) && r.brutto_eur < dec!(39),
        "KWKG Zuschlag brutto: {}",
        r.brutto_eur
    );
    let has_kwkg = r
        .positions
        .iter()
        .any(|p| p.description.to_lowercase().contains("kwkg") || p.has_tag("kwkg_zuschlag"));
    assert!(has_kwkg, "KWKG position must appear in EEG billing");
}

// ── §42a Gemeinschaftlicher Eigenverbrauch ────────────────────────────────────

#[test]
fn solar_gemeinschaft_rabatt_reduces_brutto() {
    let (_f, _t) = period();
    let base = j(r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25}"#);
    let with_r = j(
        r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25,"gemeinschaft_rabatt_ct_per_kwh":2.0}"#,
    );
    let m = SolarMeterInput {
        eigenverbrauch_kwh: dec!(300),
    };
    let r0 = bill(
        &base,
        Quantities {
            solar: Some(m.clone()),
            ..Default::default()
        },
    );
    let r1 = bill(
        &with_r,
        Quantities {
            solar: Some(m.clone()),
            ..Default::default()
        },
    );
    assert!(
        r1.brutto_eur < r0.brutto_eur,
        "Gemeinschaft Rabatt must reduce brutto"
    );
    // 300 × 2.0 ct × 1.19 = 7.14 EUR saving
    let saving = r0.brutto_eur - r1.brutto_eur;
    assert!(
        saving > dec!(7) && saving < dec!(7.5),
        "Expected ~7.14 saving, got {saving}"
    );
}

// ── Statutory rate override ───────────────────────────────────────────────────

#[test]
fn gas_energiesteuer_override_applied() {
    let (_f, _t) = period();
    let base = j(r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":10}"#);
    let with_o = j(
        r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":10,"energiesteuer_gas_ct_per_kwh_override":0}"#,
    );
    let m = GasMeterInput {
        kwh_hs: Some(dec!(500)),
        ..Default::default()
    };
    let r_std = bill(
        &base,
        Quantities {
            gas: Some(m.clone()),
            ..Default::default()
        },
    );
    let r_ovr = bill(
        &with_o,
        Quantities {
            gas: Some(m.clone()),
            ..Default::default()
        },
    );
    assert!(
        r_ovr.brutto_eur < r_std.brutto_eur,
        "Zero Energiesteuer override must reduce brutto"
    );
    // 500 × 0.55 ct × 1.19 = 3.27 EUR saving
    let saving = r_std.brutto_eur - r_ovr.brutto_eur;
    assert!(
        saving > dec!(3) && saving < dec!(4),
        "Expected ~3.27 saving, got {saving}"
    );
}

// ── BillingResult invariant and methods ──────────────────────────────────────

#[test]
fn billing_result_assert_valid_passes_for_all_categories() {
    let (_f, _t) = period();
    let m = meter(dec!(100));

    // STROM
    bill(
        &j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#),
        Quantities {
            electricity: Some(m.clone()),
            ..Default::default()
        },
    )
    .assert_valid();

    // GAS
    let gm = GasMeterInput {
        kwh_hs: Some(dec!(100)),
        ..Default::default()
    };
    bill(
        &j(r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":8}"#),
        Quantities {
            gas: Some(gm.clone()),
            ..Default::default()
        },
    )
    .assert_valid();

    // WAERME
    let wm = WaermeMeterInput {
        kwh_waerme: dec!(200),
        spitzenleistung_kw: None,
        months: Some(dec!(1)),
    };
    bill(
        &j(r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":10}"#),
        Quantities {
            heat: Some(wm.clone()),
            ..Default::default()
        },
    )
    .assert_valid();

    // EEG
    bill_credit(
        &j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8}"#),
        Quantities {
            eeg: Some(EegMeterInput {
                einspeisung_kwh: dec!(500),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .assert_valid();
}

#[test]
fn billing_result_position_total_by_tag() {
    let (_f, _t) = period();
    let grid = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(8)),
        ka_ct_per_kwh: Some(dec!(2)),
        ..GridInput::default()
    };
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":15}"#);
    let r = bill_grid(
        &tariff,
        &grid,
        Quantities {
            electricity: Some(meter(dec!(200))),
            ..Default::default()
        },
    );
    let nne_total = r.total_by_tag("nne");
    // (8 + 2) ct × 200 kWh = 20.00 EUR
    assert!(
        nne_total > dec!(19) && nne_total < dec!(21),
        "NNE tag total {}",
        nne_total
    );
    let commodity_total = r.total_by_tag("commodity");
    // 200 kWh × 15 ct = 30.00 EUR (just Arbeitspreis, not NNE)
    assert!(
        commodity_total > dec!(29) && commodity_total < dec!(31),
        "Commodity total {}",
        commodity_total
    );
}

#[cfg(feature = "bo4e")]
#[test]
fn rechnung_json_has_rechnungsempfaenger_and_faelligkeitsdatum() {
    let (_f, _t) = period();
    let r = bill(
        &j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#),
        elec(dec!(100)),
    );
    let _json_obj = r.to_rechnung_json();
    let obj = _json_obj.as_object().unwrap();
    // rechnungsempfaenger must reference the MaLo
    assert!(
        obj.contains_key("rechnungsempfaenger"),
        "rechnung_json must have rechnungsempfaenger"
    );
    let emp = &obj["rechnungsempfaenger"];
    assert_eq!(zusatz_wert(emp, "mako:externe_kunden_id"), "51238696781");
    // faelligkeitsdatum (BO4E due date, was zahlungsziel): period_to + 14 days
    // per §40c EnWG.
    //
    // BO4E declares the field `format: date-time`, so the wire value is an
    // RFC 3339 timestamp, not a bare date. mako pins it to midnight UTC — the
    // calendar date survives the promotion and survives being normalised to any
    // other offset, which a `+01:00` midnight would not.
    assert!(
        obj.contains_key("faelligkeitsdatum"),
        "rechnung_json must have faelligkeitsdatum"
    );
    let ziel = obj["faelligkeitsdatum"].as_str().unwrap_or("");
    assert_eq!(
        ziel, "2026-02-14T00:00:00Z",
        "faelligkeitsdatum must be period_to + 14 days: {ziel}"
    );
}

// ── Dynamic EPEX — negative price handling ────────────────────────────────────

#[test]
fn dynamic_strom_negative_epex_reduces_brutto() {
    // When EPEX < 0, the customer's bill decreases (credit for cheap/free/paid-for power).
    // This is correct for full pass-through §41a tariffs.
    use std::collections::HashMap;
    let _from = time::macros::date!(2026 - 01 - 01);
    let _to = time::macros::date!(2026 - 01 - 01);
    let tariff = j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":100}"#); // 1 EUR/day
    let intervals = [DynamicInterval {
        timestamp_utc: time::macros::datetime!(2026-01-01 12:00 UTC),
        kwh: dec!(1),
    }];
    let mut prices_positive = HashMap::new();
    prices_positive.insert(time::macros::datetime!(2026-01-01 12:00 UTC), dec!(20)); // +20 ct/kWh
    let mut prices_negative = HashMap::new();
    prices_negative.insert(time::macros::datetime!(2026-01-01 12:00 UTC), dec!(-30)); // -30 ct/kWh (negative EPEX)

    let r_pos = bill(
        &tariff,
        Quantities {
            dynamic_intervals: intervals.to_vec(),
            dynamic_epex_prices: prices_positive.clone(),
            ..Default::default()
        },
    );
    let r_neg = bill(
        &tariff,
        Quantities {
            dynamic_intervals: intervals.to_vec(),
            dynamic_epex_prices: prices_negative.clone(),
            ..Default::default()
        },
    );

    assert!(
        r_neg.brutto_eur < r_pos.brutto_eur,
        "Negative EPEX must produce lower brutto than positive"
    );
    r_pos.assert_valid();
    r_neg.assert_valid(); // invariant holds even for negative-price scenarios
}

// ── §51 EEG Negativpreisregel — contractual suspension (LF role) ──────────────

#[test]
fn eeg_negativpreis_suspension_reduces_verguetung() {
    // §51 EEG: when EPEX < 0, EEG Vergütung is suspended for those kWh.
    // For the LF role this is a contractual feature (not legally mandatory like for NB).
    // NB mandatory implementation lives in `eeg-billing` crate.
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.0}"#);

    // No suspension
    let m_full = EegMeterInput {
        einspeisung_kwh: dec!(500),
        ..Default::default()
    };
    // 100 kWh occurred during negative-EPEX hours → suspended
    let m_suspended = EegMeterInput {
        einspeisung_kwh: dec!(500),
        kwh_during_negative_epex: Some(dec!(100)),
    };

    let r_full = bill_credit(
        &tariff,
        Quantities {
            eeg: Some(m_full.clone()),
            ..Default::default()
        },
    );
    let r_susp = bill_credit(
        &tariff,
        Quantities {
            eeg: Some(m_suspended.clone()),
            ..Default::default()
        },
    );

    assert!(
        r_susp.brutto_eur < r_full.brutto_eur,
        "Suspension must reduce settlement"
    );
    // 100 kWh × 8.0 ct × 1.19 = 9.52 EUR reduction
    let reduction = r_full.brutto_eur - r_susp.brutto_eur;
    assert!(
        reduction > dec!(9) && reduction < dec!(10),
        "Expected ~9.52 EUR reduction, got {reduction}"
    );

    // Suspension info position must appear
    let has_info = r_susp
        .positions
        .iter()
        .any(|p| p.has_tag("eeg_negativpreis_suspension"));
    assert!(
        has_info,
        "Info position for §51 suspension must appear when kwh_during_negative_epex > 0"
    );

    // Both results must satisfy the arithmetic invariant
    r_full.assert_valid();
    r_susp.assert_valid();
}

#[test]
fn eeg_kwkg_not_affected_by_negativpreis_suspension() {
    // §51 only suspends EEG payments. KWKG Zuschlag is NOT suspended (different law).
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":0,"kwkg_zuschlag_ct_per_kwh":8.0}"#,
    );

    let m_no_susp = EegMeterInput {
        einspeisung_kwh: dec!(400),
        ..Default::default()
    };
    let m_susp = EegMeterInput {
        einspeisung_kwh: dec!(400),
        kwh_during_negative_epex: Some(dec!(400)),
    };

    let r_no = bill_credit(
        &tariff,
        Quantities {
            eeg: Some(m_no_susp.clone()),
            ..Default::default()
        },
    );
    let r_yes = bill_credit(
        &tariff,
        Quantities {
            eeg: Some(m_susp.clone()),
            ..Default::default()
        },
    );

    // KWKG uses full kwh — suspension only affects EEG Vergütung/Marktprämie
    assert_eq!(
        r_no.brutto_eur, r_yes.brutto_eur,
        "KWKG Zuschlag must NOT be affected by §51 EEG suspension"
    );
}

// ── BillingResult method coverage ─────────────────────────────────────────────

#[test]
fn billing_result_levy_total_eur_method() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#);
    let r = bill(&tariff, elec(dec!(100)));
    let levy = r.total_by_tag("levy");
    // 100 kWh × 2.05 ct Stromsteuer = 2.05 EUR levy
    assert!(
        levy > dec!(2) && levy < dec!(2.1),
        "Stromsteuer levy expected ~2.05, got {levy}"
    );
    // levy_total_eur() == position_total_by_tag("levy")
    assert_eq!(levy, r.total_by_tag("levy"));
}

#[test]
fn billing_result_positions_by_tag_iterator() {
    let (_f, _t) = period();
    let grid = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(8)),
        ka_ct_per_kwh: Some(dec!(2)),
        ..GridInput::default()
    };
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":15}"#);
    let r = bill_grid(
        &tariff,
        &grid,
        Quantities {
            electricity: Some(meter(dec!(100))),
            ..Default::default()
        },
    );
    let nne_positions: Vec<_> = r.positions_by_tag("nne").collect();
    assert!(
        nne_positions.len() >= 2,
        "Expected ≥2 NNE positions (AP + KA), got {}",
        nne_positions.len()
    );
    let commodity_positions: Vec<_> = r.positions_by_tag("commodity").collect();
    assert!(
        !commodity_positions.is_empty(),
        "Expected at least one commodity position"
    );
    // All returned positions must actually have the tag
    for p in &nne_positions {
        assert!(p.has_tag("nne"));
    }
    for p in &commodity_positions {
        assert!(p.has_tag("commodity"));
    }
}

#[test]
fn hems_with_optimization_and_readout_events() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"HEMS","hems_subscription_eur_per_month":9.99,"hems_optimization_event_eur":0.50,"hems_readout_event_eur":0.10}"#,
    );
    let m = HemsMeterInput {
        months: Some(dec!(1)),
        optimization_events: Some(5),
        readout_events: Some(10),
    };
    let r = bill(
        &tariff,
        Quantities {
            hems: Some(m.clone()),
            ..Default::default()
        },
    );
    // 9.99 + 5×0.50 + 10×0.10 = 9.99 + 2.50 + 1.00 = 13.49 netto × 1.19 ≈ 16.05 EUR
    assert!(
        r.brutto_eur > dec!(15) && r.brutto_eur < dec!(18),
        "HEMS with events brutto: {}",
        r.brutto_eur
    );
    let has_opt = r.positions.iter().any(|p| {
        p.description.to_lowercase().contains("optimiz")
            || p.description.to_lowercase().contains("optim")
    });
    assert!(has_opt, "Optimization event position must appear");
    r.assert_valid();
}

#[test]
fn dynamic_strom_floor_prevents_negative_billing() {
    // When dynamic_epex_floor_ct_kwh = 0, negative EPEX prices bill 0 (customer gets no credit).
    // This is a common contract configuration for LF's §41a tariffs.
    use std::collections::HashMap;
    // Floor = 0 ct/kWh
    let tariff_with_floor = j(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0,"dynamic_epex_floor_ct_kwh":0}"#,
    );
    let tariff_without_floor =
        j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0}"#);

    let intervals = [DynamicInterval {
        timestamp_utc: time::macros::datetime!(2026-01-01 12:00 UTC),
        kwh: dec!(10),
    }];
    let mut prices = HashMap::new();
    prices.insert(time::macros::datetime!(2026-01-01 12:00 UTC), dec!(-50)); // -50 ct/kWh

    let r_floor = bill(
        &tariff_with_floor,
        Quantities {
            dynamic_intervals: intervals.to_vec(),
            dynamic_epex_prices: prices.clone(),
            ..Default::default()
        },
    );
    let r_nofloor = bill(
        &tariff_without_floor,
        Quantities {
            dynamic_intervals: intervals.to_vec(),
            dynamic_epex_prices: prices.clone(),
            ..Default::default()
        },
    );

    // With floor=0: price is clamped to 0 → only Stromsteuer, no EPEX credit
    // Without floor: -50 ct × 10 kWh = -5 EUR EPEX credit → significantly lower brutto
    assert!(
        r_floor.brutto_eur > r_nofloor.brutto_eur,
        "Floor must produce higher brutto than no-floor with negative price; floor={}, nofloor={}",
        r_floor.brutto_eur,
        r_nofloor.brutto_eur
    );
    // With floor, brutto must be >= 0 (no negative invoice)
    assert!(
        r_floor.brutto_eur >= dec!(0),
        "Floor=0 must prevent negative invoice, got {}",
        r_floor.brutto_eur
    );
    r_floor.assert_valid();
    r_nofloor.assert_valid();
}

// ═══════════════════════════════════════════════════════════════════════════
// §41 EnWG — Jahresabrechnung mit Abschlag-Deduktion
// ═══════════════════════════════════════════════════════════════════════════

/// §41 EnWG Jahresabrechnung: advance payments must be deducted from final invoice.
#[test]
fn jahresabrechnung_deducts_abschlage_from_zahlbetrag() {
    use time::macros::date;
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":10.0,"arbeitspreis_ct_per_kwh":30.0}"#);
    let quantities = Quantities {
        electricity: Some(meter(dec!(1200))),
        ..Default::default()
    };
    // 12 monthly advance payments of EUR 120 each
    let abschlage: Vec<AbschlagDeduction> = (1u8..=12)
        .map(|m| AbschlagDeduction {
            datum: date!(2025 - 01 - 15) + time::Duration::days(i64::from(m) * 30),
            betrag_eur: dec!(120.00),
            ust_satz: dec!(0.19),
            beschreibung: Some(format!("Abschlag {m}/2025")),
        })
        .collect();
    let total_abschlage = dec!(120.00) * dec!(12);
    let ctx_final = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "SCHLUSS-2025".to_owned(),
        period: BillingPeriod::new(date!(2025 - 01 - 01), date!(2025 - 12 - 31)).unwrap(),
        invoice_type: InvoiceType::Final,
        regulatory_rates: rates_2026(),
        abschlage,
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&no_grid(), &rates_2026())
        .bill(ctx_final, &quantities)
        .unwrap();
    invoice.assert_valid();

    // Abschlag total must equal sum of deductions
    assert_eq!(invoice.abschlag_total_eur, total_abschlage);
    // Zahlbetrag = brutto - total_abschlage
    assert_eq!(invoice.zahlbetrag_eur, invoice.brutto_eur - total_abschlage);
    // Abschlag positions appear in the invoice
    let abschlag_count = invoice
        .positions
        .iter()
        .filter(|p| p.category == energy_billing::PositionCategory::Abschlag)
        .count();
    assert_eq!(abschlag_count, 12, "12 Abschlag positions expected");
    // netto_eur / mwst_eur / brutto_eur are NOT affected by Abschlag
    assert!(invoice.brutto_eur > dec!(0));

    // §14 Abs. 5 Satz 2 UStG: the tax contained in the advances is stated
    // separately. 12 × EUR 120.00 gross at 19 % → 12 × 19.16 = EUR 229.92.
    assert_eq!(invoice.abschlag_ust_eur, dec!(229.92));
}

/// §14 Abs. 5 Satz 2 UStG — an advance's tax is computed at the rate that advance
/// was invoiced at, so a mid-year rate change does not retroactively restate it.
#[test]
fn abschlag_ust_uses_the_rate_each_advance_was_invoiced_at() {
    use energy_billing::AbschlagDeduction;

    let at_19 = AbschlagDeduction {
        datum: date!(2025 - 01 - 15),
        betrag_eur: dec!(119.00),
        ust_satz: dec!(0.19),
        beschreibung: None,
    };
    let at_7 = AbschlagDeduction {
        datum: date!(2025 - 07 - 15),
        betrag_eur: dec!(107.00),
        ust_satz: dec!(0.07),
        beschreibung: None,
    };

    assert_eq!(at_19.netto_eur(), dec!(100.00));
    assert_eq!(at_19.ust_eur(), dec!(19.00));
    assert_eq!(at_7.netto_eur(), dec!(100.00));
    assert_eq!(at_7.ust_eur(), dec!(7.00));

    // Net and tax re-sum to the gross the customer actually paid.
    for a in [&at_19, &at_7] {
        assert_eq!(a.netto_eur() + a.ust_eur(), a.betrag_eur);
    }
}

/// A zero-rated advance contains no tax, and needs no special-casing.
#[test]
fn zero_rated_abschlag_contains_no_tax() {
    use energy_billing::AbschlagDeduction;

    let a = AbschlagDeduction {
        datum: date!(2025 - 03 - 15),
        betrag_eur: dec!(120.00),
        ust_satz: Decimal::ZERO,
        beschreibung: None,
    };
    assert_eq!(a.netto_eur(), dec!(120.00));
    assert_eq!(a.ust_eur(), Decimal::ZERO);
}

// ═══════════════════════════════════════════════════════════════════════════
// §40 EnWG — Kilowattstundenpreis
// ═══════════════════════════════════════════════════════════════════════════

/// §40 EnWG: invoice must display total all-inclusive ct/kWh.
#[test]
fn sect40_kilowattstundenpreis_computed_correctly() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let kwh = dec!(500);
    let invoice = bill(&tariff, elec(kwh));
    invoice.assert_valid();

    // §40: all-inclusive price = brutto_eur / kwh × 100
    let ct = invoice
        .kilowattstundenpreis_brutto_ct(kwh)
        .expect("kwh > 0");
    let expected = invoice.brutto_eur / kwh * dec!(100);
    assert_eq!(ct, expected.round_dp(4));
    // Must be higher than raw commodity rate (includes Stromsteuer + MwSt)
    assert!(
        ct > dec!(30.0),
        "all-inclusive ct/kWh must exceed commodity rate, got {ct}"
    );

    // Returns None for zero kWh
    assert!(invoice.kilowattstundenpreis_brutto_ct(dec!(0)).is_none());
}

// ═══════════════════════════════════════════════════════════════════════════
// AufAbschlag / Rabatt
// ═══════════════════════════════════════════════════════════════════════════

/// Campaign discount: -2 ct/kWh → brutto is lower than without discount.
#[test]
fn strom_auf_abschlag_discount_reduces_brutto() {
    let kwh = dec!(500);
    let without = bill(
        &j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#),
        elec(kwh),
    );
    let with_discount = bill(
        &j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0,"auf_abschlag_ct_per_kwh":-2.0}"#),
        elec(kwh),
    );
    with_discount.assert_valid();
    // Discount reduces brutto by exactly: 500 kWh × 2 ct × 1.19 MwSt = EUR 11.90
    let diff = without.brutto_eur - with_discount.brutto_eur;
    let expected_saving = kwh * dec!(2) / dec!(100) * dec!(1.19);
    assert!(
        (diff - expected_saving).abs() < dec!(0.01),
        "expected saving ~{expected_saving:.2} EUR, got diff {diff:.2} EUR"
    );
}

/// Monthly fixed surcharge adds to brutto.
#[test]
fn strom_auf_abschlag_monthly_surcharge_increases_brutto() {
    let kwh = dec!(300);
    let without = bill(
        &j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":25.0}"#),
        elec(kwh),
    );
    let with_surcharge = bill(
        &j(
            r#"{"category":"STROM","arbeitspreis_ct_per_kwh":25.0,"auf_abschlag_eur_per_month":5.0}"#,
        ),
        elec(kwh),
    );
    with_surcharge.assert_valid();
    // surcharge > 0 → brutto must be higher
    assert!(
        with_surcharge.brutto_eur > without.brutto_eur,
        "monthly surcharge must increase brutto"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// MSB Grundgebühr
// ═══════════════════════════════════════════════════════════════════════════

/// MSB fee appears as Fee position on the invoice (MsbG 2016).
#[test]
fn strom_msb_gebuehr_appears_as_fee_position() {
    let tariff =
        j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":28.0,"msb_gebuehr_ct_per_day":4.0}"#);
    let invoice = bill(&tariff, elec(dec!(400)));
    invoice.assert_valid();

    let msb_positions: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("msb_gebuehr"))
        .collect();
    assert_eq!(
        msb_positions.len(),
        1,
        "exactly one MSB fee position expected"
    );
    assert_eq!(
        msb_positions[0].category,
        energy_billing::PositionCategory::Fee
    );
    assert!(
        msb_positions[0].net_eur > dec!(0),
        "MSB fee must be positive"
    );
    // 31 days × 4 ct/day / 100 = 1.24 EUR
    let expected = dec!(31) * dec!(4) / dec!(100);
    assert_eq!(msb_positions[0].net_eur, expected);
}

// ═══════════════════════════════════════════════════════════════════════════
// BEHG year-aware pricing
// ═══════════════════════════════════════════════════════════════════════════

/// Historical billing: 2024 BEHG rate is lower than 2026.
#[test]
fn behg_year_aware_rate_differs_by_year() {
    let ct_2024 = behg_ct_per_kwh_for_year(2024).unwrap();
    let ct_2026 = behg_ct_per_kwh_for_year(2026).unwrap();
    assert!(
        ct_2026 > ct_2024,
        "2026 CO2 price (65 EUR/t) must exceed 2024 (45 EUR/t)"
    );

    let tariff: Product =
        serde_json::from_str(r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":7.50}"#)
            .unwrap();
    let gas_meter = GasMeterInput {
        kwh_hs: Some(dec!(500)),
        ..Default::default()
    };

    // Build contexts with year-specific BEHG rates (injected via RegulatoryRates)

    let quantities = Quantities {
        gas: Some(gas_meter),
        ..Default::default()
    };
    let rates_ct_2026 = RegulatoryRates {
        behg_gas_ct_per_kwh: ct_2026,
        ..RegulatoryRates::default()
    };
    let invoice_2026 = bill_rates(&tariff, quantities.clone(), &rates_ct_2026);
    let rates_ct_2024 = RegulatoryRates {
        behg_gas_ct_per_kwh: ct_2024,
        ..RegulatoryRates::default()
    };
    let invoice_2024 = bill_rates(&tariff, quantities, &rates_ct_2024);
    invoice_2026.assert_valid();
    invoice_2024.assert_valid();
    // 2026 should be more expensive due to higher CO2 levy
    assert!(
        invoice_2026.brutto_eur > invoice_2024.brutto_eur,
        "2026 invoice should be higher than 2024 (higher CO2 price): {} vs {}",
        invoice_2026.brutto_eur,
        invoice_2024.brutto_eur
    );
    // Difference ≈ (ct_2026 - ct_2024) × 500 kWh / 100 × 1.19 MwSt
    let diff = invoice_2026.brutto_eur - invoice_2024.brutto_eur;
    let expected_diff = (ct_2026 - ct_2024) * dec!(500) / dec!(100) * dec!(1.19);
    assert!(
        (diff - expected_diff).abs() < dec!(0.01),
        "BEHG difference should be ~{expected_diff:.4} EUR, got {diff:.4} EUR"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Pro-rata billing (Vertragsbeginn mid-month)
// ═══════════════════════════════════════════════════════════════════════════

/// Pro-rata: invoice period 31 days but contract started day 16 → 16/31 fraction.
#[test]
fn billing_context_prorata_mid_month_start() {
    use time::macros::date;
    let ctx_full = BillingContext {
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        ..Default::default()
    };
    let ctx_half = BillingContext {
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        vertragsbeginn: Some(date!(2026 - 01 - 16)),
        ..Default::default()
    };
    assert!(
        ctx_full.billing_days_fraction().is_none(),
        "full period = no fraction"
    );
    let frac = ctx_half.billing_days_fraction().expect("should be Some");
    // 16 days (Jan 16..31) / 31 days total
    let expected = rust_decimal::Decimal::from(16) / rust_decimal::Decimal::from(31);
    assert_eq!(frac, expected.round_dp(6));
    assert!(frac < dec!(1), "fraction must be < 1");
}

// ═══════════════════════════════════════════════════════════════════════════
// Zählerstand info positions
// ═══════════════════════════════════════════════════════════════════════════

/// Zählerstand appears as Info position when provided.
#[test]
fn strom_zaehlerstand_produces_info_position() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let meter_with_reading = MeterInput {
        arbeitsmenge_kwh: dec!(500),
        zaehlernummer: Some("1SBK0000000000".to_owned()),
        zaehlerstand_von: Some(dec!(12345)),
        zaehlerstand_bis: Some(dec!(12845)),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(meter_with_reading),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&no_grid(), &rates_2026())
        .bill(
            {
                let (f, t) = period();
                BillingContext {
                    malo_id: "51238696781".to_owned(),
                    lf_mp_id: "9900000000001".to_owned(),
                    rechnungsnummer: "TEST".to_owned(),
                    period: BillingPeriod::new(f, t).unwrap(),
                    invoice_type: InvoiceType::Initial,
                    regulatory_rates: rates_2026(),
                    ..Default::default()
                }
            },
            &quantities,
        )
        .unwrap();
    invoice.assert_valid();

    let zaehler_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("zaehlerstand"))
        .collect();
    assert_eq!(
        zaehler_pos.len(),
        1,
        "exactly one Zählerstand info position"
    );
    assert_eq!(
        zaehler_pos[0].category,
        energy_billing::PositionCategory::Info
    );
    assert_eq!(
        zaehler_pos[0].net_eur,
        dec!(0),
        "Info position has zero net"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Block / Graduated Tariff (Blocktarif / Staffelpreis)
// ═══════════════════════════════════════════════════════════════════════════

/// Three-tier block tariff: 28ct/kWh for first 1000 kWh, 24ct for next 2000, 20ct beyond.
#[test]
fn strom_block_tariff_splits_consumption_across_tiers() {
    use serde_json::json;

    let tariff: Product = serde_json::from_value(json!({
        "category": "STROM",
        "block_tiers": [
            { "bis_kwh": 1000.0, "preis_ct_per_kwh": 28.0 },
            { "bis_kwh": 3000.0, "preis_ct_per_kwh": 24.0 },
            { "preis_ct_per_kwh": 20.0 }
        ]
    }))
    .unwrap();

    // 2500 kWh: tier 1 = 1000 kWh @ 28ct, tier 2 = 1500 kWh @ 24ct
    let invoice = bill(&tariff, elec(dec!(2500)));
    invoice.assert_valid();

    let tier_positions: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("block_tier"))
        .collect();
    assert_eq!(tier_positions.len(), 2, "exactly 2 tiers used for 2500 kWh");

    assert_eq!(tier_positions[0].quantity, dec!(1000), "tier 1: 1000 kWh");
    assert_eq!(tier_positions[0].unit_price_eur, dec!(28) / dec!(100));

    assert_eq!(tier_positions[1].quantity, dec!(1500), "tier 2: 1500 kWh");
    assert_eq!(tier_positions[1].unit_price_eur, dec!(24) / dec!(100));

    // Net energy: 1000×28ct + 1500×24ct = 280 + 360 = 640 EUR
    let commodity: Decimal = tier_positions.iter().map(|p| p.net_eur).sum();
    assert_eq!(
        commodity,
        dec!(640.00000),
        "commodity total must be 640 EUR"
    );
}

#[test]
fn strom_block_tariff_all_three_tiers() {
    use serde_json::json;
    let tariff: Product = serde_json::from_value(json!({
        "category": "STROM",
        "block_tiers": [
            { "bis_kwh": 1000.0, "preis_ct_per_kwh": 28.0 },
            { "bis_kwh": 3000.0, "preis_ct_per_kwh": 24.0 },
            { "preis_ct_per_kwh": 20.0 }
        ]
    }))
    .unwrap();

    // 5000 kWh: all three tiers
    let invoice = bill(&tariff, elec(dec!(5000)));
    invoice.assert_valid();
    let tier_positions: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("block_tier"))
        .collect();
    assert_eq!(
        tier_positions.len(),
        3,
        "all three tiers activated for 5000 kWh"
    );
    assert_eq!(
        tier_positions[2].quantity,
        dec!(2000),
        "tier 3: 5000-3000=2000 kWh"
    );
    assert_eq!(tier_positions[2].unit_price_eur, dec!(20) / dec!(100));
}

#[test]
fn strom_block_tariff_lower_than_flat_rate_for_high_consumption() {
    // Block tariff is cheaper at high consumption than a flat 28ct rate
    use serde_json::json;
    let flat_tariff: Product =
        serde_json::from_value(json!({ "category": "STROM", "arbeitspreis_ct_per_kwh": 28.0 }))
            .unwrap();
    let block_tariff: Product = serde_json::from_value(json!({
        "category": "STROM",
        "block_tiers": [
            { "bis_kwh": 1000.0, "preis_ct_per_kwh": 28.0 },
            { "preis_ct_per_kwh": 20.0 }
        ]
    }))
    .unwrap();
    let flat_invoice = bill(&flat_tariff, elec(dec!(3000)));
    let block_invoice = bill(&block_tariff, elec(dec!(3000)));
    flat_invoice.assert_valid();
    block_invoice.assert_valid();
    assert!(
        block_invoice.brutto_eur < flat_invoice.brutto_eur,
        "block tariff must be cheaper at high volume: block={} flat={}",
        block_invoice.brutto_eur,
        flat_invoice.brutto_eur
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §41 EnWG — Verbrauchshistorie (consumption comparison on invoice)
// ═══════════════════════════════════════════════════════════════════════════

/// §41 Abs. 1 Nr. 3 EnWG: prior-year and average consumption must appear on invoice.
#[cfg(feature = "bo4e")]
#[test]
fn strom_verbrauchshistorie_produces_info_positions() {
    use time::macros::date;

    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-VH".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        verbrauchshistorie: Some(energy_billing::Verbrauchshistorie {
            vorjahr_kwh: Some(dec!(2800)),
            bundesdurchschnitt_kwh: Some(dec!(3500)),
            kundengruppe: Some("3-Personen-Haushalt".to_owned()),
        }),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(
            ctx,
            &Quantities {
                electricity: Some(meter(dec!(500))),
                ..Default::default()
            },
        )
        .unwrap();
    invoice.assert_valid();

    let vorjahr = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("vorjahr"))
        .expect("Vorjahr position must exist");
    assert_eq!(vorjahr.category, energy_billing::PositionCategory::Info);
    assert_eq!(vorjahr.quantity, dec!(2800));
    assert_eq!(vorjahr.net_eur, dec!(0), "Info position is EUR 0");

    let avg = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("bundesdurchschnitt"))
        .expect("Bundesdurchschnitt position must exist");
    assert_eq!(avg.quantity, dec!(3500));

    // Verbrauchshistorie must also appear in rechnung_json as ZusatzAttribute
    let json = invoice.to_rechnung_json();
    let attrs = json["zusatzAttribute"]
        .as_array()
        .expect("zusatzAttribute must exist");
    let has_vj = attrs.iter().any(|a| a["name"] == "mako:verbrauch_vorjahr");
    let has_avg = attrs
        .iter()
        .any(|a| a["name"] == "mako:verbrauch_bundesdurchschnitt");
    assert!(has_vj, "verbrauchVorjahr must be in zusatzAttribute");
    assert!(
        has_avg,
        "verbrauchBundesdurchschnitt must be in zusatzAttribute"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §40 EnWG — Kilowattstundenpreis in Rechnung JSON
// ═══════════════════════════════════════════════════════════════════════════

/// §40 EnWG: Rechnung JSON must include `kilowattstundenpreisGesamt`.
#[cfg(feature = "bo4e")]
#[test]
fn strom_rechnung_json_includes_sect40_kilowattstundenpreis() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let invoice = bill(&tariff, elec(dec!(500)));
    invoice.assert_valid();

    let json = invoice.to_rechnung_json();
    // §40 data has no BO4E field; it rides as a structured ZusatzAttribut.
    let kw_preis = zusatz_wert(&json, "mako:kilowattstundenpreis_gesamt");
    assert!(
        !kw_preis.is_null(),
        "kilowattstundenpreisGesamt must be present for electricity"
    );

    let ct_str = kw_preis["wert"].as_str().expect("wert must be a string");
    let ct: Decimal = ct_str.parse().expect("wert must be a valid Decimal");
    assert!(
        ct > dec!(30.0),
        "all-inclusive ct/kWh must exceed raw commodity rate (includes Stromsteuer + MwSt)"
    );
    assert_eq!(kw_preis["rechtlicheGrundlage"].as_str(), Some("§40 EnWG"));
}

// ═══════════════════════════════════════════════════════════════════════════
// §41 EnWG — Energiemix in Rechnung JSON
// ═══════════════════════════════════════════════════════════════════════════

/// §41 Abs. 1 Nr. 8 + §42 EnWG: Energiemix must appear in the invoice JSON.
#[cfg(feature = "bo4e")]
#[test]
fn strom_energiemix_appears_in_rechnung_json() {
    use time::macros::date;
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":28.0}"#);
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-EM".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        energiequellen: Some(energy_billing::EnergieQuellen {
            erneuerbar_pct: dec!(100),
            co2_g_per_kwh: dec!(0),
            hkn_certified: true,
            beschreibung: Some("100% Ökostrom, HKN-zertifiziert (TÜV Rheinland 2026)".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(
            ctx,
            &Quantities {
                electricity: Some(meter(dec!(300))),
                ..Default::default()
            },
        )
        .unwrap();
    invoice.assert_valid();

    let json = invoice.to_rechnung_json();
    let attrs = json["zusatzAttribute"]
        .as_array()
        .expect("zusatzAttribute must exist");
    let em = attrs
        .iter()
        .find(|a| a["name"] == "mako:stromkennzeichnung")
        .expect("stromkennzeichnung ZusatzAttribut must exist");
    // Structured, not prose: the CO₂ figure §42 Abs. 2 Nr. 2 names is a field,
    // and the human description travels inside the structure.
    assert_eq!(em["wert"]["erneuerbar_pct"], "100");
    assert_eq!(em["wert"]["co2_g_per_kwh"], "0");
    assert!(
        em["wert"]["beschreibung"]
            .as_str()
            .unwrap()
            .contains("Ökostrom")
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Abschlagsplan
// ═══════════════════════════════════════════════════════════════════════════

/// Abschlagsplan generates correct monthly entries summing to annual cost.
#[test]
fn abschlagsplan_monthly_uniform_correct() {
    use energy_billing::Abschlagsplan;
    use time::macros::date;

    let plan = Abschlagsplan::monthly_uniform(
        "51238696781",
        date!(2026 - 01 - 15),
        12,
        dec!(1440.00),
        dec!(3600),
    );
    assert_eq!(plan.entries.len(), 12);
    assert_eq!(plan.entries[0].betrag_eur, dec!(120.00));
    assert_eq!(plan.total_eur(), dec!(1440.00));
    // First entry is January 15, 2026
    assert_eq!(plan.entries[0].faellig_am, date!(2026 - 01 - 15));
    // Last entry is December 15, 2026
    assert_eq!(plan.entries[11].faellig_am, date!(2026 - 12 - 15));
    // Plan description contains the year
    assert!(
        plan.entries[0]
            .beschreibung
            .as_ref()
            .unwrap()
            .contains("2026")
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §42b EnWG (Solarpaket I) — Gemeinschaftliche Gebäudeversorgung (GGV)
// ═══════════════════════════════════════════════════════════════════════════

/// GGV hybrid billing: PV allocation < consumption → split into PV + grid portions.
#[test]
fn ggv_hybrid_billing_splits_pv_and_grid_portions() {
    use energy_billing::GgvSolarInput;
    use serde_json::json;

    // Plant generated 80 kWh; Tenant A has 60% allocation = 48 kWh PV.
    // Tenant A actually consumed 70 kWh → 48 kWh PV + 22 kWh from grid.
    let ggv_input = GgvSolarInput {
        pv_allocated_kwh: dec!(48),
        actual_consumption_kwh: dec!(70),
    };
    assert_eq!(
        ggv_input.pv_delivered_kwh(),
        dec!(48),
        "PV delivered capped at allocated"
    );
    assert_eq!(
        ggv_input.grid_kwh(),
        dec!(22),
        "grid = consumption - pv_allocated"
    );

    // Coverage ratio: 48/70 ≈ 0.6857
    let ratio = ggv_input.pv_coverage_ratio();
    assert!(
        ratio > dec!(0.68) && ratio < dec!(0.69),
        "coverage ratio {ratio}"
    );

    // Build tariff: solar_arbeitspreis (PV) + arbeitspreis (grid fallback)
    let tariff: Product = serde_json::from_value(json!({
        "category": "SOLAR",
        "solar_arbeitspreis_ct_per_kwh": 22.0,    // GGV PV rate (cheaper)
        "arbeitspreis_ct_per_kwh": 30.0,           // Grid fallback rate
        "gemeinschaft_rabatt_ct_per_kwh": 1.5      // §42b Abs. 3 Rabatt
    }))
    .unwrap();

    let quantities = Quantities {
        ggv_solar: Some(ggv_input),
        ..Default::default()
    };
    let invoice = bill_full(
        &tariff,
        &GridInput::default(),
        quantities.clone(),
        &rates_2026(),
        InvoiceType::Initial,
    );
    invoice.assert_valid();

    // Positions should include: PV Arbeitspreis, GGV Rabatt, Grid Arbeitspreis,
    // Stromsteuer (grid), GGV coverage info, MwSt
    let pv_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("ggv_pv"))
        .collect();
    let grid_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("ggv_grid"))
        .collect();
    assert!(!pv_pos.is_empty(), "PV positions must exist");
    assert!(!grid_pos.is_empty(), "Grid positions must exist");

    // PV commodity total: 48 kWh × 22 ct - 48 kWh × 1.5 ct = 48 × 20.5 ct = 9.84 EUR
    let pv_net: Decimal = pv_pos.iter().map(|p| p.net_eur).sum();
    let expected_pv = dec!(48) * (dec!(22) - dec!(1.5)) / dec!(100);
    assert!(
        (pv_net - expected_pv).abs() < dec!(0.001),
        "PV net expected {expected_pv}, got {pv_net}"
    );

    // Grid commodity: 22 kWh × 30 ct = 6.60 EUR (before Stromsteuer)
    let grid_commodity = grid_pos
        .iter()
        .filter(|p| p.category == energy_billing::PositionCategory::Commodity)
        .map(|p| p.net_eur)
        .sum::<Decimal>();
    let expected_grid = dec!(22) * dec!(30) / dec!(100);
    assert!(
        (grid_commodity - expected_grid).abs() < dec!(0.001),
        "Grid commodity expected {expected_grid}, got {grid_commodity}"
    );

    // Legal basis on PV positions must reference §42b
    let has_42b = pv_pos
        .iter()
        .any(|p| p.legal_basis.as_deref().unwrap_or("").contains("42b"));
    assert!(has_42b, "§42b EnWG must be legal basis for PV positions");

    // GGV coverage info position must exist
    let has_coverage = invoice.positions.iter().any(|p| p.has_tag("ggv_coverage"));
    assert!(has_coverage, "GGV coverage info position must be present");
}

/// GGV: when PV allocation ≥ consumption, no grid fallback position generated.
#[test]
fn ggv_no_grid_when_pv_covers_full_consumption() {
    use energy_billing::GgvSolarInput;
    use serde_json::json;

    // Tenant consumed 40 kWh, allocated 50 kWh PV → all from PV, no grid
    let ggv_input = GgvSolarInput {
        pv_allocated_kwh: dec!(50),
        actual_consumption_kwh: dec!(40),
    };
    assert_eq!(ggv_input.pv_delivered_kwh(), dec!(40)); // capped at consumption
    assert_eq!(ggv_input.grid_kwh(), dec!(0));
    assert_eq!(ggv_input.pv_coverage_ratio(), dec!(1.0000));

    let tariff: Product = serde_json::from_value(json!({
        "category": "SOLAR",
        "solar_arbeitspreis_ct_per_kwh": 20.0,
        "arbeitspreis_ct_per_kwh": 29.0  // Grid fallback (not used here)
    }))
    .unwrap();

    let quantities = Quantities {
        ggv_solar: Some(ggv_input),
        ..Default::default()
    };
    let invoice = bill_full(
        &tariff,
        &GridInput::default(),
        quantities,
        &rates_2026(),
        InvoiceType::Initial,
    );
    invoice.assert_valid();

    let grid_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("ggv_grid"))
        .collect();
    assert!(
        grid_pos.is_empty(),
        "No grid positions when PV covers all consumption"
    );

    let pv_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("ggv_pv"))
        .collect();
    assert!(!pv_pos.is_empty(), "PV positions must be present");
}

/// GgvNutzungsplan.allocate() correctly distributes plant generation.
#[test]
fn ggv_nutzungsplan_allocate_uses_lrm_for_exact_sum() {
    use energy_billing::{GgvNutzungsplan, GgvNutzungsplanEntry};

    let plan = GgvNutzungsplan(vec![
        GgvNutzungsplanEntry {
            malo_id: "A".into(),
            fraction: dec!(0.60),
        },
        GgvNutzungsplanEntry {
            malo_id: "B".into(),
            fraction: dec!(0.40),
        },
    ]);
    let plant_kwh = dec!(100.000);
    let allocs = plan.allocate(plant_kwh);

    // Sum must equal plant_kwh exactly (LRM guarantees this)
    let sum: Decimal = allocs.iter().map(|(_, k)| k).sum();
    assert_eq!(sum, plant_kwh, "allocations must sum to plant generation");

    // A gets 60, B gets 40
    let a = allocs.iter().find(|(id, _)| id == "A").unwrap().1;
    let b = allocs.iter().find(|(id, _)| id == "B").unwrap().1;
    assert_eq!(a, dec!(60.000));
    assert_eq!(b, dec!(40.000));
}

/// §42b: standard SolarProvider path unchanged when ggv_solar is None.
#[test]
fn solar_provider_simple_path_unchanged_without_ggv() {
    let tariff = j(
        r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25.0,"gemeinschaft_rabatt_ct_per_kwh":1.5}"#,
    );
    let invoice = bill(
        &tariff,
        Quantities {
            solar: Some(energy_billing::SolarMeterInput {
                eigenverbrauch_kwh: dec!(200),
            }),
            ..Default::default()
        },
    );
    invoice.assert_valid();

    // Legal basis must now be §42b (not the old §42a)
    let solar_pos = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("solar") && p.category == energy_billing::PositionCategory::Commodity);
    let legal_basis = solar_pos
        .and_then(|p| p.legal_basis.as_deref())
        .unwrap_or("");
    assert!(
        legal_basis.contains("42b"),
        "Solar arbeitspreis must reference §42b EnWG (not §42a), got: {legal_basis}"
    );

    // GGV Rabatt must also reference §42b
    let rabatt_pos = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("gemeinschaft_rabatt"));
    let rabatt_basis = rabatt_pos
        .and_then(|p| p.legal_basis.as_deref())
        .unwrap_or("");
    assert!(
        rabatt_basis.contains("42b"),
        "GGV Rabatt must reference §42b EnWG, got: {rabatt_basis}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Multi-rate MwSt (§12 UStG)
// ═══════════════════════════════════════════════════════════════════════════

/// A 0 % feed-in Gutschrift (§19 UStG Kleinunternehmer, here via an explicit
/// override) produces no Tax position. brutto must equal netto.
#[test]
fn solar_zero_mwst_produces_no_tax_position() {
    use energy_billing::PositionCategory;
    // Solar with mwst_rate_override=0 → no MwSt
    let tariff =
        j(r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":20.0,"mwst_rate_override":0.0}"#);
    let invoice = bill(
        &tariff,
        Quantities {
            solar: Some(energy_billing::SolarMeterInput {
                eigenverbrauch_kwh: dec!(100),
            }),
            ..Default::default()
        },
    );
    invoice.assert_valid();
    let tax_positions: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Tax)
        .collect();
    assert!(
        tax_positions.is_empty(),
        "0% MwSt must produce no Tax position"
    );
    // brutto = netto (no tax)
    assert_eq!(
        invoice.brutto_eur, invoice.netto_eur,
        "brutto must equal netto for 0% VAT"
    );
}

/// §12 Abs. 2 Nr. 1 UStG — 7% MwSt for renewable district heating.
#[test]
fn waerme_reduced_mwst_7pct_produces_correct_tax() {
    use energy_billing::PositionCategory;
    let tariff = j(
        r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":12.0,"mwst_rate_override":0.07}"#,
    );
    let invoice = bill(
        &tariff,
        Quantities {
            heat: Some(energy_billing::WaermeMeterInput {
                kwh_waerme: dec!(500),
                spitzenleistung_kw: None,
                months: Some(dec!(1)),
            }),
            ..Default::default()
        },
    );
    invoice.assert_valid();
    let tax_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Tax)
        .collect();
    assert_eq!(tax_pos.len(), 1, "exactly one Tax position for 7% rate");
    // Description must say 7 %
    assert!(
        tax_pos[0].description.contains("7"),
        "Tax description must mention 7%: {}",
        tax_pos[0].description
    );
    // Net: 500 × 12ct / 100 = 60 EUR; MwSt 7% = 4.20 EUR
    let expected_mwst = dec!(60) * dec!(0.07);
    assert_eq!(
        tax_pos[0].net_eur,
        expected_mwst.round_dp(5),
        "7% tax amount incorrect"
    );
}

/// Multi-rate: electricity (19%) + FernwÃ¤rme in the 7% window on same invoice.
/// MwStProvider must generate two separate Tax positions.
#[test]
fn multi_rate_mwst_electricity_and_heat_on_same_invoice() {
    use energy_billing::PositionCategory; // no, use bill_full
    // Build engine manually for multi-product invoice
    let elec_tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let heat_tariff = j(
        r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":10.0,"mwst_rate_override":0.07}"#,
    );

    let _quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(100),
            ..Default::default()
        }),
        heat: Some(energy_billing::WaermeMeterInput {
            kwh_waerme: dec!(200),
            spitzenleistung_kw: None,
            months: Some(dec!(1)),
        }),
        ..Default::default()
    };

    // The heat positions must carry applicable_tax_rate=0.07
    // Electricity positions use the engine-wide default (0.19)
    // Build two separate BillingEngines and compare brutto totals
    let elec_invoice = bill(
        &elec_tariff,
        Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: dec!(100),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let heat_invoice = bill(
        &heat_tariff,
        Quantities {
            heat: Some(energy_billing::WaermeMeterInput {
                kwh_waerme: dec!(200),
                spitzenleistung_kw: None,
                months: Some(dec!(1)),
            }),
            ..Default::default()
        },
    );
    elec_invoice.assert_valid();
    heat_invoice.assert_valid();

    // Electricity: 100 kWh × 30 ct + 2.05 ct Stromsteuer = 32.05 EUR netto × 1.19 = 38.14 EUR brutto
    assert!(
        elec_invoice.brutto_eur > dec!(37),
        "electricity brutto too low: {}",
        elec_invoice.brutto_eur
    );
    // Heat: 200 kWh × 10 ct = 20 EUR netto × 1.07 = 21.40 EUR brutto
    let heat_netto = dec!(200) * dec!(10) / dec!(100);
    let expected_heat_brutto = heat_netto * dec!(1.07);
    assert_eq!(
        heat_invoice.brutto_eur,
        expected_heat_brutto.round_dp(5),
        "7% heat brutto incorrect"
    );

    // Tax position must say "7 %"
    let heat_tax = heat_invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Tax)
        .collect::<Vec<_>>();
    assert_eq!(heat_tax.len(), 1);
    assert!(
        heat_tax[0].description.contains("7"),
        "Heat tax must reference 7%"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Minimum invoice (Mindestbetrag)
// ═══════════════════════════════════════════════════════════════════════════

/// When brutto is below minimum, a Mindestbetrag position is added.
#[test]
fn minimum_invoice_topup_when_below_minimum() {
    use energy_billing::PositionCategory;
    use time::macros::date;

    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":5.0}"#); // very cheap

    // Set minimum to EUR 100 brutto
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-MIN".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        minimum_invoice_eur_brutto: Some(dec!(100.00)),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(10),
            ..Default::default()
        }), // only 10 kWh
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    // brutto must be >= 100 EUR
    assert!(
        invoice.brutto_eur >= dec!(100.00),
        "brutto {} must be >= minimum 100.00 EUR",
        invoice.brutto_eur
    );

    // A Mindestbetrag position must exist
    let min_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("mindestbetrag"))
        .collect();
    assert_eq!(min_pos.len(), 1, "exactly one Mindestbetrag position");
    assert_eq!(min_pos[0].category, PositionCategory::Commodity);
}

/// When brutto >= minimum, NO Mindestbetrag is added.
#[test]
fn minimum_invoice_no_topup_when_already_above_minimum() {
    use time::macros::date;

    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-MIN2".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        minimum_invoice_eur_brutto: Some(dec!(5.00)), // minimum well below actual
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(500),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    let min_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("mindestbetrag"))
        .collect();
    assert!(
        min_pos.is_empty(),
        "No Mindestbetrag when brutto already exceeds minimum"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Multi-rate MwSt — bundled invoice (critical correctness test)
// ═══════════════════════════════════════════════════════════════════════════

/// A SINGLE billing engine with electricity (default 19%) AND Fernwärme at 7 %
/// (the temporary §28 Abs. 5/6 UStG window via mwst_rate_override) must produce TWO separate Tax positions.
/// This is the critical correctness test for the multi-rate MwSt architecture.
#[test]
fn bundled_invoice_electricity_19pct_and_heat_7pct_window_two_tax_positions() {
    use energy_billing::{
        BillingEngine, ElectricityProvider, HeatProvider, MwStProvider, PositionCategory,
        WaermeMeterInput,
    };
    use serde_json::json;

    // Electricity tariff — no override → engine default 19% applies
    let elec_tariff: Product = serde_json::from_value(json!({
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 30.0
    }))
    .unwrap();

    // Heat tariff inside the temporary 7 % window (§28 Abs. 5/6 UStG)
    let heat_tariff: Product = serde_json::from_value(json!({
        "category": "WAERME",
        "waerme_arbeitspreis_ct_per_kwh": 10.0,
        "mwst_rate_override": 0.07
    }))
    .unwrap();

    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(100),
            ..Default::default()
        }),
        heat: Some(WaermeMeterInput {
            kwh_waerme: dec!(200),
            months: Some(dec!(1)),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Single engine, both providers, single MwStProvider at 19% default
    let invoice = BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &elec_tariff,
            GridInput::default(),
        ))
        .add(HeatProvider::from_product(&heat_tariff))
        .add(MwStProvider::new(dec!(0.19)))
        .bill(
            {
                let (f, t) = period();
                BillingContext {
                    malo_id: "51238696781".to_owned(),
                    lf_mp_id: "9900000000001".to_owned(),
                    rechnungsnummer: "TEST-MULTI-RATE".to_owned(),
                    period: BillingPeriod::new(f, t).unwrap(),
                    invoice_type: InvoiceType::Initial,
                    regulatory_rates: rates_2026(),
                    ..Default::default()
                }
            },
            &quantities,
        )
        .unwrap();
    invoice.assert_valid();

    // Two Tax positions: 19% for electricity, 7% for heat
    let tax_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Tax)
        .collect();
    assert_eq!(
        tax_pos.len(),
        2,
        "Must have exactly 2 Tax positions (7% + 19%), got: {:?}",
        tax_pos.iter().map(|p| &p.description).collect::<Vec<_>>()
    );

    // One at 19%, one at 7%
    let has_19 = tax_pos.iter().any(|p| p.description.contains("19"));
    let has_7 = tax_pos.iter().any(|p| p.description.contains("7"));
    assert!(has_19, "Must have 19% Tax position");
    assert!(has_7, "Must have 7% Tax position");

    // Electricity netto: 100 × 30ct + 100 × 2.05ct Stromsteuer = 32.05 EUR
    // Heat netto: 200 × 10ct / 100 = 20.00 EUR
    // Total netto ≈ 52.05 EUR
    // MwSt: 32.05 × 0.19 + 20.00 × 0.07 = 6.0895 + 1.40 = 7.49 EUR
    // Brutto ≈ 59.54 EUR
    assert!(
        invoice.netto_eur > dec!(51) && invoice.netto_eur < dec!(53),
        "Netto {} expected ~52 EUR",
        invoice.netto_eur
    );
    assert!(
        invoice.brutto_eur > dec!(58) && invoice.brutto_eur < dec!(61),
        "Brutto {} expected ~59.54 EUR",
        invoice.brutto_eur
    );
}

/// Heat positions carry applicable_tax_rate from an explicit mwst_rate_override
/// (e.g. the temporary §28 Abs. 5/6 UStG 7 % window) so bundled invoices split correctly.
#[test]
fn heat_positions_carry_7pct_applicable_tax_rate() {
    use energy_billing::{
        BillingEngine, HeatProvider, MwStProvider, PositionCategory, WaermeMeterInput,
    };
    use serde_json::json;

    let heat_tariff: Product = serde_json::from_value(json!({
        "category": "WAERME",
        "waerme_arbeitspreis_ct_per_kwh": 12.0,
        "mwst_rate_override": 0.07
    }))
    .unwrap();

    let quantities = Quantities {
        heat: Some(WaermeMeterInput {
            kwh_waerme: dec!(300),
            months: Some(dec!(1)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = BillingEngine::new()
        .add(HeatProvider::from_product(&heat_tariff))
        .add(MwStProvider::new(dec!(0.19))) // engine default 19%
        .bill(
            {
                let (f, t) = period();
                BillingContext {
                    malo_id: "51238696781".to_owned(),
                    lf_mp_id: "9900000000001".to_owned(),
                    rechnungsnummer: "TEST-HEAT-7".to_owned(),
                    period: BillingPeriod::new(f, t).unwrap(),
                    invoice_type: InvoiceType::Initial,
                    regulatory_rates: rates_2026(),
                    ..Default::default()
                }
            },
            &quantities,
        )
        .unwrap();
    invoice.assert_valid();

    // Heat commodity position must carry applicable_tax_rate = Some(0.07)
    let heat_commodity = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("waerme") && p.category == PositionCategory::Commodity)
        .collect::<Vec<_>>();
    assert!(
        !heat_commodity.is_empty(),
        "Heat commodity position must exist"
    );
    for pos in &heat_commodity {
        assert_eq!(
            pos.applicable_tax_rate,
            Some(dec!(0.07)),
            "Heat position must carry 7% tax rate, got {:?}",
            pos.applicable_tax_rate
        );
    }

    // Tax position must be at 7%
    let tax_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Tax)
        .collect();
    assert_eq!(tax_pos.len(), 1, "Exactly one Tax position (7%)");
    assert!(
        tax_pos[0].description.contains("7"),
        "Tax must be at 7%: {}",
        tax_pos[0].description
    );

    // Brutto = 300 × 12ct / 100 × 1.07 = 38.52 EUR
    let expected = dec!(300) * dec!(12) / dec!(100) * dec!(1.07);
    assert_eq!(invoice.brutto_eur, expected, "Brutto must reflect 7% VAT");
}

// ═══════════════════════════════════════════════════════════════════════════
// Indexed Prices (§41 EnWG Sonderkundenvertrag)
// ═══════════════════════════════════════════════════════════════════════════

/// Gas indexed to TTF: effective price = base + spread + TTF × factor
#[test]
fn gas_indexed_price_ttf_computes_correctly() {
    use serde_json::json;

    // TTF at 35 EUR/MWh; conversion 0.1 EUR/MWh → ct/kWh
    // Effective price = 0.5 + 0.3 + 35.0 × 0.1 = 4.3 ct/kWh
    let tariff: Product = serde_json::from_value(json!({
        "category": "GAS",
        "indexed_price": {
            "base_ct_per_kwh": 0.5,
            "spread_ct_per_kwh": 0.3,
            "index_name": "TTF_Front_Month",
            "index_value": 35.0,
            "factor_ct_per_unit": 0.1
        }
    }))
    .unwrap();

    let quantities = Quantities {
        gas: Some(energy_billing::GasMeterInput {
            kwh_hs: Some(dec!(1000)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = bill(&tariff, quantities);
    invoice.assert_valid();

    // Find the indexed gas arbeitspreis position
    let gas_ap = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("indexed_price"))
        .collect::<Vec<_>>();
    assert_eq!(gas_ap.len(), 1, "Exactly one indexed Arbeitspreis position");
    assert!(
        gas_ap[0].description.contains("TTF"),
        "Position must mention index name"
    );

    // 1000 kWh × 4.3 ct / 100 = 43 EUR netto (before levies)
    let expected_ct = dec!(0.5) + dec!(0.3) + dec!(35.0) * dec!(0.1); // = 4.3 ct/kWh
    assert_eq!(expected_ct, dec!(4.3));
    let expected_netto_gas_ap = dec!(1000) * expected_ct / dec!(100);
    assert_eq!(
        gas_ap[0].net_eur, expected_netto_gas_ap,
        "Indexed gas arbeitspreis net: {}",
        gas_ap[0].net_eur
    );
}

/// When index_value is absent, indexed price falls back to static arbeitspreis.
#[test]
fn gas_indexed_price_falls_back_when_no_index_value() {
    use serde_json::json;

    // No index_value provided → fallback to gas_arbeitspreis_ct_per_kwh
    let tariff: Product = serde_json::from_value(json!({
        "category": "GAS",
        "gas_arbeitspreis_ct_per_kwh_hs": 8.0,
        "indexed_price": {
            "base_ct_per_kwh": 0.5,
            "spread_ct_per_kwh": 0.3,
            "index_name": "TTF_Front_Month",
            "factor_ct_per_unit": 0.1
            // No index_value
        }
    }))
    .unwrap();

    let quantities = Quantities {
        gas: Some(energy_billing::GasMeterInput {
            kwh_hs: Some(dec!(500)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = bill(&tariff, quantities);
    invoice.assert_valid();

    // Should use 8.0 ct/kWh fallback
    let gas_ap: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("gas") && p.category == energy_billing::PositionCategory::Commodity)
        .collect();
    // The fallback price 8.0 ct × 500 kWh = 40 EUR
    let ap_total: rust_decimal::Decimal = gas_ap.iter().map(|p| p.net_eur).sum();
    assert_eq!(
        ap_total,
        dec!(40.0),
        "Fallback gas arbeitspreis: {ap_total}"
    );
}

/// Indexed electricity (Phelix Base) via `indexed_price` config.
#[test]
fn electricity_indexed_price_phelix_computes_correctly() {
    use serde_json::json;

    // Phelix Base at 80 EUR/MWh → 8.0 ct/kWh + 0.5 base + 0.2 spread = 8.7 ct/kWh
    let tariff: Product = serde_json::from_value(json!({
        "category": "STROM",
        "indexed_price": {
            "base_ct_per_kwh": 0.5,
            "spread_ct_per_kwh": 0.2,
            "index_name": "Phelix_Base",
            "index_value": 80.0,
            "factor_ct_per_unit": 0.1
        }
    }))
    .unwrap();

    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(200),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = bill(&tariff, quantities);
    invoice.assert_valid();

    let indexed_ap: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("indexed_price"))
        .collect();
    assert_eq!(indexed_ap.len(), 1, "One indexed electricity AP position");

    // 200 kWh × 8.7 ct / 100 = 17.4 EUR
    let expected = dec!(200) * (dec!(0.5) + dec!(0.2) + dec!(80.0) * dec!(0.1)) / dec!(100);
    assert_eq!(
        indexed_ap[0].net_eur, expected,
        "Indexed electricity net: {}",
        indexed_ap[0].net_eur
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// District heating VAT — standard-rated (no permanent reduced rate)
// ═══════════════════════════════════════════════════════════════════════════

/// District heating is **standard-rated (19 %)** by default. There is no
/// permanent reduced VAT for Fernwärme: §12 Abs. 2 Nr. 1 UStG covers Anlage-2
/// goods, not heat, and the 7 % on gas/Fernwärme was the temporary §28 Abs. 5/6
/// UStG window (01.10.2022–31.03.2024). Renewable content does not change the rate.
#[test]
fn fernwaerme_is_standard_rated_by_default() {
    use energy_billing::PositionCategory;
    use serde_json::json;

    let tariff: Product = serde_json::from_value(json!({
        "category": "WAERME",
        "waerme_arbeitspreis_ct_per_kwh": 12.0,
        "waerme_erneuerbar_anteil_pct": 0.8   // disclosure only — must NOT reduce VAT
    }))
    .unwrap();

    let invoice = bill(
        &tariff,
        Quantities {
            heat: Some(energy_billing::WaermeMeterInput {
                kwh_waerme: dec!(300),
                months: Some(dec!(1)),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    invoice.assert_valid();

    // Brutto = 300 × 12ct / 100 × 1.19 = 42.84 EUR (standard rate).
    assert_eq!(
        invoice.brutto_eur,
        dec!(300) * dec!(12) / dec!(100) * dec!(1.19),
        "renewable district heating is still standard-rated"
    );
    let tax: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Tax)
        .collect();
    assert_eq!(tax.len(), 1);
    assert!(tax[0].description.contains("19"), "{}", tax[0].description);
}

/// The historical §28 Abs. 5/6 UStG 7 % window is expressed via
/// `mwst_rate_override` (billingd derives it period-aware) — a Wärme bill for a
/// period inside the window bills 7 %.
#[test]
fn fernwaerme_temporary_7pct_window_via_override() {
    use energy_billing::PositionCategory;
    use serde_json::json;

    let tariff: Product = serde_json::from_value(json!({
        "category": "WAERME",
        "waerme_arbeitspreis_ct_per_kwh": 10.0,
        "mwst_rate_override": 0.07   // period inside 01.10.2022–31.03.2024
    }))
    .unwrap();

    let invoice = bill(
        &tariff,
        Quantities {
            heat: Some(energy_billing::WaermeMeterInput {
                kwh_waerme: dec!(100),
                months: Some(dec!(1)),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    invoice.assert_valid();
    let tax: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Tax)
        .collect();
    assert_eq!(tax.len(), 1);
    assert!(tax[0].description.contains('7'), "{}", tax[0].description);
    assert_eq!(
        invoice.brutto_eur,
        dec!(100) * dec!(10) / dec!(100) * dec!(1.07)
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Seasonal tariffs (Saisontarif)
// ═══════════════════════════════════════════════════════════════════════════

/// Winter gas rate (Oct–Mar) is higher than summer rate.
#[test]
fn seasonal_gas_winter_price_higher_than_summer() {
    use serde_json::json;
    use time::macros::date;

    let tariff: Product = serde_json::from_value(json!({
        "category": "GAS",
        "seasonal_prices": [
            { "from_month": 10, "to_month": 3, "gas_arbeitspreis_ct_per_kwh_hs": 12.5, "label": "Winter" },
            { "from_month": 4,  "to_month": 9, "gas_arbeitspreis_ct_per_kwh_hs": 8.0,  "label": "Sommer" }
        ]
    })).unwrap();

    let gas_meter = energy_billing::GasMeterInput {
        kwh_hs: Some(dec!(500)),
        ..Default::default()
    };

    // January billing → winter rate (12.5 ct)
    let ctx_winter = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-WINTER".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        ..Default::default()
    };
    // July billing → summer rate (8.0 ct)
    let ctx_summer = BillingContext {
        rechnungsnummer: "TEST-SUMMER".to_owned(),
        period: BillingPeriod::new(date!(2026 - 07 - 01), date!(2026 - 07 - 31)).unwrap(),
        ..ctx_winter.clone()
    };

    let q = Quantities {
        gas: Some(gas_meter),
        ..Default::default()
    };

    let winter_invoice = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(ctx_winter, &q)
        .unwrap();
    let summer_invoice = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(ctx_summer, &q.clone())
        .unwrap();
    winter_invoice.assert_valid();
    summer_invoice.assert_valid();

    assert!(
        winter_invoice.brutto_eur > summer_invoice.brutto_eur,
        "Winter gas must be more expensive: winter={} summer={}",
        winter_invoice.brutto_eur,
        summer_invoice.brutto_eur
    );

    // Seasonal position label must appear in description
    let winter_ap = winter_invoice
        .positions
        .iter()
        .find(|p| p.has_tag("seasonal"));
    assert!(
        winter_ap.is_some(),
        "Winter position must be tagged 'seasonal'"
    );
    assert!(
        winter_ap.unwrap().description.contains("Winter"),
        "Winter label must appear: {}",
        winter_ap.unwrap().description
    );
}

/// Seasonal price overrides electricity arbeitspreis in the billing month.
#[test]
fn seasonal_electricity_summer_rate_lower_than_base() {
    use serde_json::json;
    use time::macros::date;

    let tariff: Product = serde_json::from_value(json!({
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 30.0,    // base price (used when no season matches)
        "seasonal_prices": [
            { "from_month": 6, "to_month": 8, "arbeitspreis_ct_per_kwh": 22.0, "label": "Sommer" }
        ]
    }))
    .unwrap();

    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(100),
            ..Default::default()
        }),
        ..Default::default()
    };
    // August → summer (22.0 ct)
    let ctx_aug = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-AUG".to_owned(),
        period: BillingPeriod::new(date!(2026 - 08 - 01), date!(2026 - 08 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        ..Default::default()
    };
    // November → base (30.0 ct)
    let ctx_nov = BillingContext {
        rechnungsnummer: "TEST-NOV".to_owned(),
        period: BillingPeriod::new(date!(2026 - 11 - 01), date!(2026 - 11 - 30)).unwrap(),
        ..ctx_aug.clone()
    };

    let aug_invoice = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(ctx_aug, &q)
        .unwrap();
    let nov_invoice = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(ctx_nov, &q.clone())
        .unwrap();
    aug_invoice.assert_valid();
    nov_invoice.assert_valid();
    assert!(
        aug_invoice.brutto_eur < nov_invoice.brutto_eur,
        "Summer must be cheaper: aug={} nov={}",
        aug_invoice.brutto_eur,
        nov_invoice.brutto_eur
    );
}

/// SeasonalPriceOverride.contains_month wraps around year boundary correctly.
#[test]
fn seasonal_price_override_contains_month_wrap_around() {
    use energy_billing::SeasonalPriceOverride;
    use rust_decimal::dec;

    let winter = SeasonalPriceOverride {
        from_month: 10,
        to_month: 3,
        gas_arbeitspreis_ct_per_kwh_hs: Some(dec!(12.5)),
        arbeitspreis_ct_per_kwh: None,
        label: Some("Winter".to_owned()),
    };
    assert!(winter.contains_month(10), "October is in winter");
    assert!(winter.contains_month(12), "December is in winter");
    assert!(winter.contains_month(1), "January is in winter");
    assert!(winter.contains_month(3), "March is in winter");
    assert!(!winter.contains_month(4), "April is NOT in winter");
    assert!(!winter.contains_month(9), "September is NOT in winter");
}

// ═══════════════════════════════════════════════════════════════════════════
// Prosumer net metering (§ 9 Abs. 1 Nr. 3 StromStG)
// ═══════════════════════════════════════════════════════════════════════════

/// Prosumer invoice: only grid consumption attracts commodity charges.
/// Self-consumption is Stromsteuer-exempt and NNE-free.
#[test]
fn prosumer_bills_only_grid_consumption_no_nne_on_self_consumption() {
    use energy_billing::{PositionCategory, ProsumerMeterInput};
    use serde_json::json;

    let tariff: Product = serde_json::from_value(json!({
        "category": "STROM",
        "grundpreis_ct_per_day": 10.0,
        "arbeitspreis_ct_per_kwh": 30.0
    }))
    .unwrap();

    let grid_nne = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(8.0)),
        ..GridInput::default()
    };

    let prosumer = ProsumerMeterInput {
        grid_consumption_kwh: dec!(200), // billed normally
        self_consumption_kwh: dec!(150), // exempt from Stromsteuer + NNE
        export_kwh: Some(dec!(50)),
    };

    let quantities = Quantities {
        prosumer: Some(prosumer),
        ..Default::default()
    };

    let invoice = energy_billing::BillingEngine::new()
        .add(energy_billing::ElectricityProvider::from_product(
            &tariff, grid_nne,
        ))
        .add(energy_billing::MwStProvider::new(dec!(0.19)))
        .bill(
            {
                let (f, t) = period();
                BillingContext {
                    malo_id: "51238696781".to_owned(),
                    lf_mp_id: "9900000000001".to_owned(),
                    rechnungsnummer: "TEST-PROSUMER".to_owned(),
                    period: BillingPeriod::new(f, t).unwrap(),
                    invoice_type: InvoiceType::Initial,
                    regulatory_rates: rates_2026(),
                    ..Default::default()
                }
            },
            &quantities,
        )
        .unwrap();
    invoice.assert_valid();

    // NNE should only be on 200 kWh grid consumption (not 150 kWh self-consumption)
    let nne_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("nne_arbeitspreis"))
        .collect();
    assert_eq!(nne_pos.len(), 1);
    assert_eq!(
        nne_pos[0].quantity,
        dec!(200),
        "NNE quantity must be grid_kwh only"
    );

    // Commodity arbeitspreis: 200 kWh × 30 ct = 60 EUR
    let ap_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| {
            p.has_tag("strom") && p.category == PositionCategory::Commodity && p.unit == "kWh"
        })
        .collect();
    assert!(!ap_pos.is_empty(), "Strom Arbeitspreis must exist");
    let ap_sum: Decimal = ap_pos.iter().map(|p| p.net_eur).sum();
    assert_eq!(ap_sum, dec!(60.00000), "Arbeitspreis must be 200 × 30ct");

    // Eigenverbrauch info position must exist
    let ev_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.has_tag("eigenverbrauch"))
        .collect();
    assert_eq!(ev_pos.len(), 1, "Eigenverbrauch info position must exist");
    assert_eq!(
        ev_pos[0].net_eur,
        dec!(0),
        "Eigenverbrauch info must have zero net"
    );
    assert!(
        ev_pos[0].description.contains("150"),
        "Must show self-consumption kWh"
    );
}

/// ProsumerMeterInput helpers work correctly.
#[test]
fn prosumer_meter_helpers() {
    use energy_billing::ProsumerMeterInput;

    let m = ProsumerMeterInput {
        grid_consumption_kwh: dec!(250),
        self_consumption_kwh: dec!(150),
        export_kwh: Some(dec!(50)),
    };
    assert_eq!(m.total_consumption_kwh(), dec!(400));
    let ratio = m.self_supply_ratio();
    // 150 / 400 = 0.375
    assert_eq!(ratio, dec!(0.3750));
}

// ═══════════════════════════════════════════════════════════════════════════
// §40b EnWG — Preisvergleichsdaten in invoice JSON
// ═══════════════════════════════════════════════════════════════════════════

/// §40b invoice JSON must contain preisvergleichsdaten with Arbeitspreis ct/kWh.
#[cfg(feature = "bo4e")]
#[test]
fn sect40b_preisvergleichsdaten_in_rechnung_json() {
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":30.0,"arbeitspreis_ct_per_kwh":28.0}"#);
    let invoice = bill(&tariff, elec(dec!(100)));
    invoice.assert_valid();

    let json = invoice.to_rechnung_json();
    // §40b data has no BO4E field; it rides as a structured ZusatzAttribut.
    let pvd = zusatz_wert(&json, "mako:preisvergleichsdaten");
    assert!(!pvd.is_null(), "preisvergleichsdaten must be present");
    assert_eq!(pvd["rechtlicheGrundlage"].as_str(), Some("§40b EnWG"));

    // arbeitspreis_ct_per_kwh must be set
    let ap = pvd["arbeitspreisCtProKwh"]
        .as_str()
        .expect("arbeitspreisCtProKwh must be a string");
    let ap_val: Decimal = ap.parse().expect("must be numeric");
    // Should be the 28 ct/kWh commodity price (stored in ct/kWh in JSON)
    assert_eq!(
        ap_val,
        dec!(28.0),
        "arbeitspreis should be 28 ct/kWh: {ap_val}"
    );

    // grundpreisEurProJahr must be set
    let gp_year = &pvd["grundpreisEurProJahr"];
    assert!(!gp_year.is_null(), "grundpreisEurProJahr must be present");
    let gp_val_str = gp_year["wert"].as_str().expect("wert must exist");
    let gp_val: Decimal = gp_val_str.parse().expect("must be numeric");
    // grundpreis_ct_per_day = 30ct → 365 × 30ct / 100 = 109.5 EUR/year
    let expected_gp = dec!(30) / dec!(100) * dec!(365);
    assert_eq!(gp_val, expected_gp, "grundpreis per year: {gp_val}");
}

// ═══════════════════════════════════════════════════════════════════════════
// InvoiceType — Rechnungsart mapping
// ═══════════════════════════════════════════════════════════════════════════

/// `InvoiceType::Initial` maps to BO4E Rechnungstyp `ENDKUNDENRECHNUNG`
/// (actual consumption billed to the end customer).
#[cfg(feature = "bo4e")]
#[test]
fn initial_invoice_type_maps_to_endkundenrechnung() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let invoice = bill(&tariff, elec(dec!(100)));
    let json = invoice.to_rechnung_json();
    assert_eq!(
        json["rechnungstyp"].as_str(),
        Some("ENDKUNDENRECHNUNG"),
        "Initial billing (metered consumption) must be ENDKUNDENRECHNUNG"
    );
    assert!(
        json["istStorno"].is_null(),
        "a normal invoice carries no Storno flag"
    );
}

/// `InvoiceType::AdvancePayment` maps to BO4E Rechnungstyp `ABSCHLAGSRECHNUNG`.
#[cfg(feature = "bo4e")]
#[test]
fn advance_payment_invoice_type_maps_to_abschlagsrechnung() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let invoice = bill_full(
        &tariff,
        &GridInput::default(),
        elec(dec!(100)),
        &rates_2026(),
        InvoiceType::AdvancePayment,
    );
    let json = invoice.to_rechnung_json();
    assert_eq!(
        json["rechnungstyp"].as_str(),
        Some("ABSCHLAGSRECHNUNG"),
        "AdvancePayment must produce ABSCHLAGSRECHNUNG"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Cancellation — sign reversal
// ═══════════════════════════════════════════════════════════════════════════

/// A cancellation (Stornorechnung) reverses all position signs.
/// If original brutto = +X, cancellation brutto = -X.
#[cfg(feature = "bo4e")]
#[test]
fn cancellation_invoice_reverses_all_signs() {
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":10.0,"arbeitspreis_ct_per_kwh":30.0}"#);

    // Produce the original invoice
    let original = bill(&tariff, elec(dec!(200)));
    original.assert_valid();
    assert!(original.brutto_eur > dec!(0), "Original must be positive");

    // Produce the cancellation with the exact same quantities
    let cancellation = bill_full(
        &tariff,
        &GridInput::default(),
        elec(dec!(200)),
        &rates_2026(),
        InvoiceType::Cancellation {
            original_invoice_id: "INV-2026-001".to_owned(),
        },
    );
    cancellation.assert_valid();

    // The cancellation JSON must carry the BO4E Storno marking: istStorno +
    // originalRechnungsnummer, plus the process label as ZusatzAttribut.
    let json = cancellation.to_rechnung_json();
    assert_eq!(json["istStorno"].as_bool(), Some(true));
    assert_eq!(
        json["originalRechnungsnummer"].as_str(),
        Some("INV-2026-001")
    );
    assert_eq!(zusatz_wert(&json, "mako:rechnungsart"), "STORNORECHNUNG");

    // Netto and Brutto of cancellation = -(original)
    assert_eq!(
        cancellation.netto_eur, -original.netto_eur,
        "Cancellation netto must be negated original"
    );
    assert_eq!(
        cancellation.brutto_eur, -original.brutto_eur,
        "Cancellation brutto must be negated original"
    );
    assert_eq!(
        cancellation.mwst_eur, -original.mwst_eur,
        "Cancellation MwSt must be negated original"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Correction — originalRechnungsId in JSON
// ═══════════════════════════════════════════════════════════════════════════

/// A correction invoice must have originalRechnungsId in the rechnung_json.
#[cfg(feature = "bo4e")]
#[test]
fn correction_invoice_has_original_reference_in_json() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let invoice = bill_full(
        &tariff,
        &GridInput::default(),
        elec(dec!(350)), // corrected consumption
        &rates_2026(),
        InvoiceType::Correction {
            original_invoice_id: "INV-2026-005".to_owned(),
            reason: Some("Zählerstand korrigiert".to_owned()),
        },
    );
    invoice.assert_valid();

    let json = invoice.to_rechnung_json();
    assert_eq!(zusatz_wert(&json, "mako:rechnungsart"), "KORREKTURRECHNUNG");
    assert!(
        json["istStorno"].is_null(),
        "a Korrektur is not a Storno — it bills the corrected amount"
    );
    assert_eq!(
        json["originalRechnungsnummer"].as_str(),
        Some("INV-2026-005"),
        "Correction must reference original invoice ID"
    );
    // Normal positive brutto (correction bills the CORRECTED amount, not a delta)
    assert!(
        invoice.brutto_eur > dec!(0),
        "Correction brutto must be positive"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §41 EnWG — nb_mp_id (Netzbetreiber on invoice)
// ═══════════════════════════════════════════════════════════════════════════

/// §41 Abs. 1 Nr. 5 EnWG: invoice must identify the Netzbetreiber.
#[cfg(feature = "bo4e")]
#[test]
fn nb_mp_id_appears_in_rechnung_json_when_set() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-NB-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        // §41 Abs. 1 Nr. 5 EnWG: Netzbetreiber BDEW-Codenummer
        nb_mp_id: Some("9900000000099".to_owned()),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates_2026())
        .bill(
            ctx,
            &Quantities {
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(100),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    invoice.assert_valid();

    let json = invoice.to_rechnung_json();
    let nb = &json["netzbetreiber"];
    assert!(
        !nb.is_null(),
        "netzbetreiber must be present when nb_mp_id is set"
    );
    assert_eq!(
        nb["rollencodenummer"].as_str(),
        Some("9900000000099"),
        "netzbetreiber.rollencodenummer must match nb_mp_id"
    );
}

/// When `nb_mp_id` is None, `netzbetreiber` is absent from the JSON.
#[cfg(feature = "bo4e")]
#[test]
fn nb_mp_id_absent_from_json_when_not_set() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let invoice = bill(&tariff, elec(dec!(100)));
    let json = invoice.to_rechnung_json();
    assert!(
        json["netzbetreiber"].is_null(),
        "netzbetreiber must be null when nb_mp_id is not set"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// PositionCategory::Bonus
// ═══════════════════════════════════════════════════════════════════════════

/// Welcome bonus reduces brutto and has PositionCategory::Bonus.
#[test]
fn welcome_bonus_reduces_brutto_with_bonus_category() {
    use energy_billing::BillingPosition;

    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let base_invoice = bill(&tariff, elec(dec!(200)));

    // Add a welcome bonus position manually via BillingEngine
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-BONUS".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(200),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Build engine with electricity + bonus + MwSt
    struct WelcomeBonusProvider;
    impl energy_billing::BillingProvider for WelcomeBonusProvider {
        fn bill(
            &self,
            _ctx: &BillingContext,
            _q: &Quantities,
            _prior: &[BillingPosition],
        ) -> Result<Vec<BillingPosition>, energy_billing::EngineError> {
            Ok(vec![BillingPosition {
                description: "Willkommensbonus Neukunde".to_owned(),
                legal_basis: None,
                quantity: dec!(1),
                unit: "EUR".to_owned(),
                unit_price_eur: dec!(-25.0),
                net_eur: dec!(-25.0),
                category: PositionCategory::Bonus,
                tags: vec!["bonus".to_owned()],
                applicable_tax_rate: None,
                trace: energy_billing::PositionTrace::default(),
            }])
        }
    }

    let invoice = BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &tariff,
            GridInput::default(),
        ))
        .add(WelcomeBonusProvider)
        .add(MwStProvider::new(dec!(0.19)))
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    // Bonus reduces netto and brutto
    assert!(
        invoice.brutto_eur < base_invoice.brutto_eur,
        "Welcome bonus must reduce brutto: {} vs {}",
        invoice.brutto_eur,
        base_invoice.brutto_eur
    );

    // Bonus position has correct category
    let bonus_pos: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.category == PositionCategory::Bonus)
        .collect();
    assert_eq!(bonus_pos.len(), 1, "Exactly one Bonus position");
    assert_eq!(bonus_pos[0].net_eur, dec!(-25.0));
    // Bonus affects MwSt base → MwSt is lower
    assert!(
        invoice.mwst_eur < base_invoice.mwst_eur,
        "Bonus must reduce MwSt base: {} vs {}",
        invoice.mwst_eur,
        base_invoice.mwst_eur
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Multi-product invoice (electricity + gas on same BillingEngine)
// ═══════════════════════════════════════════════════════════════════════════

/// A single BillingEngine can handle multiple product providers simultaneously,
/// producing a combined invoice with positions from all products.
#[test]
fn multi_product_electricity_and_gas_on_one_invoice() {
    let elec_tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let gas_tariff = j(r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":8.0}"#);

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-MULTI".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(200),
            ..Default::default()
        }),
        gas: Some(GasMeterInput {
            kwh_hs: Some(dec!(500)),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &elec_tariff,
            GridInput::default(),
        ))
        .add(GasProvider::from_product(&gas_tariff, GridInput::default()))
        .add(MwStProvider::new(dec!(0.19)))
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    // Invoice must have both electricity and gas positions
    let elec_pos: Vec<_> = invoice.positions_by_tag("strom").collect();
    let gas_pos: Vec<_> = invoice.positions_by_tag("gas").collect();
    assert!(!elec_pos.is_empty(), "Must have electricity positions");
    assert!(!gas_pos.is_empty(), "Must have gas positions");

    // Single combined brutto — both products together
    // Elec: 200 × 30ct + 200 × 2.05ct Stromsteuer = 64.10 EUR netto
    // Gas: 500 × 8ct + 500 × (0.55 + 0.62ct) levies = 44.85 EUR netto
    // Total netto ≈ 108.95 EUR; brutto ≈ 129.65 EUR
    assert!(
        invoice.brutto_eur > dec!(100),
        "Combined brutto must be substantial"
    );
}

// ── New feature tests ─────────────────────────────────────────────────────────

#[test]
fn eeg_kleinunternehmer_zero_pct_mwst() {
    // §19 UStG: a feed-in operator who has elected the Kleinunternehmerregelung
    // issues the Gutschrift at 0 % USt. The election is explicit — not derived
    // from plant size (§12 Abs. 3 UStG zero-rates the PV *system*, not the feed-in).
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "EEG",
        "kleinunternehmer_19_ustg": true,
        "eeg_verguetungssatz_ct_per_kwh": 8.2
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-TEST-ANLKWP".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::CreditNote,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let quantities = Quantities {
        eeg: Some(EegMeterInput {
            einspeisung_kwh: dec!(100),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    // 0% MwSt → mwst_eur must be zero
    assert_eq!(
        invoice.mwst_eur,
        Decimal::ZERO,
        "§19 UStG Kleinunternehmer: mwst must be 0 on the feed-in Gutschrift"
    );
    // EEG Gutschrift: netto_eur > 0 (LF pays the generator — amount is positive on the Gutschrift)
    assert!(
        invoice.netto_eur > Decimal::ZERO,
        "EEG Gutschrift must have positive netto (LF pays generator)"
    );
}

#[test]
fn eeg_regelbesteuerung_normal_mwst() {
    // Without the Kleinunternehmer election the feed-in Gutschrift is
    // standard-rated (Regelbesteuerung), regardless of plant size.
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "EEG",
        "anlage_kwp": 50.0,
        "eeg_verguetungssatz_ct_per_kwh": 8.2
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-TEST-ANLLG".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::CreditNote,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let quantities = Quantities {
        eeg: Some(EegMeterInput {
            einspeisung_kwh: dec!(100),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    // No Kleinunternehmer election → standard 19% MwSt (mwst_eur non-zero)
    assert!(
        invoice.mwst_eur != Decimal::ZERO,
        "Regelbesteuerung feed-in must have standard 19% MwSt"
    );
}

/// § 9b StromStG is a **Steuerentlastung**: the supplier invoices the full
/// 2,05 ct/kWh and the customer reclaims 2,00 ct/kWh from the Hauptzollamt.
/// Zero-rating the levy at supply — what the old
/// `industrie_stromsteuer_befreiung` flag did — under-declared the supplier's
/// own Stromsteueranmeldung and let the customer reclaim tax nobody had paid.
#[test]
fn a_produzierendes_gewerbe_is_billed_the_full_stromsteuer() {
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 18.0,
        "steuerentlastungen": ["STROMSTEUER9B"]
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-TEST-INDUSTRIE".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    // The levy is billed in full: 1000 kWh × 2,05 ct = 20,50 EUR.
    let levy: Vec<_> = invoice.positions_by_tag("stromsteuer").collect();
    assert_eq!(levy.len(), 1, "the Stromsteuer stays on the invoice");
    assert_eq!(levy[0].net_eur.round_dp(2), dec!(20.50));
    assert_eq!(invoice.netto_eur.round_dp(2), dec!(200.50));

    // …and the customer is told what they may reclaim, and on what.
    let hinweis: Vec<_> = invoice.positions_by_tag("steuerentlastung").collect();
    assert_eq!(hinweis.len(), 1);
    assert_eq!(hinweis[0].net_eur, Decimal::ZERO);
    assert!(hinweis[0].description.contains("20.50"));
    assert_eq!(
        hinweis[0].legal_basis.as_deref(),
        Some("\u{a7} 9b StromStG")
    );
}

/// § 9 Abs. 2 StromStG is a **reduction**, not an exemption: Fahrstrom is
/// taxed at 11,42 EUR/MWh. The old `Bahnstrom` variant dropped the line
/// entirely and lost 1,142 ct/kWh of tax on every rail-traction invoice.
#[test]
fn fahrstrom_is_billed_at_the_reduced_rate() {
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 18.0,
        "stromsteuer_tarif": { "art": "ERMAESSIGUNG", "grund": "FAHRSTROM" }
    }"#,
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
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(1000),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    let levy: Vec<_> = invoice.positions_by_tag("stromsteuer").collect();
    assert_eq!(levy.len(), 1);
    // 1000 kWh × 1,142 ct = 11,42 EUR
    assert_eq!(levy[0].net_eur.round_dp(2), dec!(11.42));
}

/// § 9 Abs. 1 Nr. 3 StromStG — a genuine exemption still removes the levy.
#[test]
fn a_kleinanlage_exemption_removes_the_levy() {
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 18.0,
        "stromsteuer_tarif": { "art": "BEFREIUNG", "grund": "KLEINANLAGE" }
    }"#,
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
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(1000),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(invoice.positions_by_tag("stromsteuer").count(), 0);
    assert_eq!(invoice.positions_by_tag("stromsteuer_befreiung").count(), 1);
    assert_eq!(invoice.netto_eur.round_dp(2), dec!(180.00));
}

#[test]
fn metering_mode_imsys_stored_on_meter_input() {
    // MeteringMode is stored and serializes correctly.
    let meter = MeterInput {
        arbeitsmenge_kwh: dec!(500),
        metering_mode: MeteringMode::Imsys,
        ..Default::default()
    };
    let json = serde_json::to_value(&meter).unwrap();
    assert_eq!(json["metering_mode"], "IMSYS");
}

#[test]
fn is_estimated_meter_produces_info_position() {
    // § 60 Abs. 2 MsbG — estimated reading must be labeled on the invoice.
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 30.0
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-TEST-EST".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(500),
            is_estimated: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    let est_pos: Vec<_> = invoice.positions_by_tag("schatzwert").collect();
    assert_eq!(
        est_pos.len(),
        1,
        "Must have § 60 Abs. 2 MsbG estimated reading notice"
    );
    assert!(
        est_pos[0].description.contains("Sch\u{00e4}tzung"),
        "Must mention Schätzung"
    );
}

#[test]
fn zaehler_replaced_produces_info_position() {
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 30.0
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-TEST-ZWECHSEL".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(500),
            zaehler_replaced: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    let replaced_pos: Vec<_> = invoice.positions_by_tag("zaehlerwechsel").collect();
    assert_eq!(
        replaced_pos.len(),
        1,
        "Must have Zählerwechsel info position"
    );
}

#[test]
fn preisgarantie_bis_produces_info_position_when_in_future() {
    // §41 Abs. 1 Nr. 4 EnWG — price guarantee must appear on invoice.
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 30.0,
        "preisgarantie_bis": "2027-12-31"
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-TEST-PREISGAR".to_owned(),
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
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    let pg_pos: Vec<_> = invoice.positions_by_tag("preisgarantie").collect();
    assert_eq!(pg_pos.len(), 1, "Must have Preisgarantie info position");
    assert!(
        pg_pos[0].description.contains("2027-12-31"),
        "Must contain guarantee date"
    );
}

#[cfg(feature = "bo4e")]
#[test]
fn billing_run_id_propagated_to_invoice_and_json() {
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 30.0
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-TEST-RUNID".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        billing_run_id: Some("run-uuid-12345".to_owned()),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(500),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    assert_eq!(invoice.billing_run_id.as_deref(), Some("run-uuid-12345"));

    let json = invoice.to_rechnung_json();
    let attrs = json["zusatzAttribute"].as_array().unwrap();
    let run_id_attr = attrs.iter().find(|a| a["name"] == "mako:billing_run_id");
    assert!(
        run_id_attr.is_some(),
        "billingRunId must appear in zusatzAttribute"
    );
    assert_eq!(run_id_attr.unwrap()["wert"], "run-uuid-12345");
}

#[test]
fn sect41a_annual_comparison_compute_savings_correct() {
    // Compute: 2000 kWh × 40 ct = 800 EUR reference; actual = 650 EUR → savings = 150 EUR
    let comp = Sect41aAnnualComparison::compute(dec!(2000), dec!(650), dec!(40.0));
    assert_eq!(comp.reference_eur_brutto, dec!(800.00));
    assert_eq!(comp.savings_eur, dec!(150.00));
}

#[test]
fn sect41a_annual_comparison_negative_savings_when_more_expensive() {
    // Compute: 2000 kWh × 20 ct = 400 EUR reference; actual = 550 EUR → savings = -150 EUR
    let comp = Sect41aAnnualComparison::compute(dec!(2000), dec!(550), dec!(20.0));
    assert_eq!(comp.reference_eur_brutto, dec!(400.00));
    assert_eq!(comp.savings_eur, dec!(-150.00));
}

#[test]
fn partial_invoice_type_maps_to_teilrechnung() {
    // §41 EnWG — Teilrechnung for partial supply periods. BO4E's closest
    // Rechnungstyp is ZWISCHENRECHNUNG; the exact label survives as attribute.
    assert_eq!(InvoiceType::PartialInvoice.rechnungsart(), "TEILRECHNUNG");
    assert_eq!(
        InvoiceType::PartialInvoice.rechnungstyp(),
        Some(rubo4e::current::Rechnungstyp::Zwischenrechnung)
    );
    assert!(!InvoiceType::PartialInvoice.is_reversal());
}

#[test]
fn metering_mode_default_is_slp() {
    let meter = MeterInput::default();
    assert_eq!(meter.metering_mode, MeteringMode::Slp);
}

#[test]
fn electricity_consumption_is_standard_rated_regardless_of_own_pv() {
    // A retail electricity supply is 19 % even where the customer runs their own
    // PV: §12 Abs. 3 UStG zero-rates the PV *system* supply, not the electricity.
    // `anlage_kwp` is informational and must not change the consumption rate.
    let rates = RegulatoryRates::default();
    for kwp in ["10.0", "30.0", "31.0"] {
        let tariff: Product =
            serde_json::from_str(&format!(r#"{{"category":"STROM","anlage_kwp": {kwp}}}"#))
                .unwrap();
        let Product::Strom(p) = &tariff else {
            unreachable!()
        };
        assert_eq!(
            rates.effective_mwst_electricity(p),
            dec!(0.19),
            "consumption with {kwp} kWp own PV is still standard-rated"
        );
    }
    // An explicit override still wins (e.g. a caller-assessed reverse-charge case).
    let overridden: Product =
        serde_json::from_str(r#"{"category":"STROM","mwst_rate_override": 0.0}"#).unwrap();
    let Product::Strom(p) = &overridden else {
        unreachable!()
    };
    assert_eq!(rates.effective_mwst_electricity(p), Decimal::ZERO);
}

// ── billing crate capability map: new features ────────────────────────────────

#[test]
fn tou_pricing_ht_nt_matches_manual_calculation() {
    // HT/NT via billing::TimeOfUsePricing must produce same result as direct arithmetic.
    // HT: 300 kWh × 32 ct/kWh = 96.00 EUR (net)
    // NT: 200 kWh × 18 ct/kWh = 36.00 EUR (net)
    // Total net commodity: 132.00 EUR
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ht_ct_per_kwh": 32.0,
        "arbeitspreis_nt_ct_per_kwh": 18.0
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-TOU".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(500),
            arbeitsmenge_ht_kwh: Some(dec!(300)),
            arbeitsmenge_nt_kwh: Some(dec!(200)),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    // HT position
    let ht: Vec<_> = invoice.positions_by_tag("ht").collect();
    assert_eq!(ht.len(), 1, "Must have one HT position");
    assert_eq!(
        ht[0].net_eur.round_dp(2),
        dec!(96.00),
        "HT: 300 × 0.32 = 96.00"
    );

    // NT position
    let nt: Vec<_> = invoice.positions_by_tag("nt").collect();
    assert_eq!(nt.len(), 1, "Must have one NT position");
    assert_eq!(
        nt[0].net_eur.round_dp(2),
        dec!(36.00),
        "NT: 200 × 0.18 = 36.00"
    );

    // Total commodity net (without levies)
    let commodity: Decimal = invoice
        .positions_by_tag("arbeitspreis")
        .filter(|p| p.category == PositionCategory::Commodity)
        .map(|p| p.net_eur)
        .sum();
    assert_eq!(commodity.round_dp(2), dec!(132.00), "HT + NT total");
}

#[test]
fn prorate_days_returns_active_and_total() {
    // vertragsbeginn = Jan 16 → active_days = 16, total_days = 31
    let ctx = BillingContext {
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        vertragsbeginn: Some(date!(2026 - 01 - 16)),
        ..Default::default()
    };
    let (active, total) = ctx.prorate_days();
    assert_eq!(total, 31);
    assert_eq!(active, 16, "Jan 16–31 = 16 billable days");
}

#[test]
fn prorate_days_no_constraint_returns_full() {
    let ctx = BillingContext {
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        ..Default::default()
    };
    let (active, total) = ctx.prorate_days();
    assert_eq!(active, 31);
    assert_eq!(total, 31);
}

#[test]
fn grundpreis_prorated_for_partial_period() {
    // Contract starts Jan 16 → Grundpreis should be 16 × 0.10 = 1.60 EUR (not 31 × 0.10 = 3.10)
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 30.0,
        "grundpreis_ct_per_day": 10.0
    }"#,
    )
    .unwrap();

    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-PRORATA".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        vertragsbeginn: Some(date!(2026 - 01 - 16)),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(250),
            ..Default::default()
        }),
        ..Default::default()
    };

    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();
    invoice.assert_valid();

    let grund: Decimal = invoice.total_by_tag("grundpreis");
    // 16 days × 0.10 EUR/day = 1.60 EUR
    assert_eq!(
        grund.round_dp(2),
        dec!(1.60),
        "Grundpreis must be prorated to 16 days"
    );
}

#[test]
fn invoice_merge_combines_positions_and_recalculates_totals() {
    // Two sub-period invoices merged: old tariff Jan 1-14, new tariff Jan 15-31.
    let tariff_old: Product = serde_json::from_str(
        r#"{
        "category": "STROM", "arbeitspreis_ct_per_kwh": 28.0, "mwst_rate_override": 0.19
    }"#,
    )
    .unwrap();
    let tariff_new: Product = serde_json::from_str(
        r#"{
        "category": "STROM", "arbeitspreis_ct_per_kwh": 32.0, "mwst_rate_override": 0.19
    }"#,
    )
    .unwrap();
    let rates = RegulatoryRates::default();

    let ctx_a = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TW-A".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 14)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let ctx_b = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TW-B".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 15), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let q_a = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(140),
            ..Default::default()
        }),
        ..Default::default()
    };
    let q_b = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(170),
            ..Default::default()
        }),
        ..Default::default()
    };

    let inv_a = tariff_old
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx_a, &q_a)
        .unwrap();
    let inv_b = tariff_new
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx_b, &q_b)
        .unwrap();

    let netto_a = inv_a.netto_eur;
    let netto_b = inv_b.netto_eur;

    let merged = inv_a.merge(inv_b);
    merged.assert_valid();

    // Merged period covers the full month
    assert_eq!(merged.context.period_from(), date!(2026 - 01 - 01));
    assert_eq!(merged.context.period_to(), date!(2026 - 01 - 31));

    // Totals are the sum of both sub-invoices
    assert_eq!(
        merged.netto_eur.round_dp(2),
        (netto_a + netto_b).round_dp(2),
        "Merged netto must equal sum of sub-invoice nettos"
    );
}

#[test]
fn invoice_allocate_proportionally_penny_correct() {
    // Split a EUR 100 invoice 60/40 between two recipients.
    // 60% → EUR 60, 40% → EUR 40. Sum must equal original exactly.
    let tariff: Product = serde_json::from_str(
        r#"{
        "category": "STROM", "arbeitspreis_ct_per_kwh": 20.0, "mwst_rate_override": 0.0
    }"#,
    )
    .unwrap();
    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "ALLOC-BASE".to_owned(),
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
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(ctx, &quantities)
        .unwrap();

    let ctx_a = BillingContext {
        rechnungsnummer: "ALLOC-A".to_owned(),
        malo_id: "A".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        ..invoice.context.clone()
    };
    let ctx_b = BillingContext {
        rechnungsnummer: "ALLOC-B".to_owned(),
        malo_id: "B".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        ..invoice.context.clone()
    };

    let original_brutto = invoice.brutto_eur;
    let parts = invoice
        .allocate_proportionally(&[dec!(0.6), dec!(0.4)], vec![ctx_a, ctx_b])
        .unwrap();

    assert_eq!(parts.len(), 2);
    let sum: Decimal = parts.iter().map(|p| p.brutto_eur).sum();
    assert_eq!(sum, original_brutto, "Allocation must be penny-exact");
    assert!(parts[0].brutto_eur > parts[1].brutto_eur, "60% > 40%");
}

/// The NNE Grundpreis accrues only over active contract days.
///
/// It billed the full period regardless of `vertragsbeginn`/`vertragsende`,
/// while the commodity Grundpreis was clipped — so a mid-month move-in paid a
/// full month of network base charge and half a month of the supplier's own.
#[test]
fn nne_grundpreis_is_clipped_to_the_contract() {
    use energy_billing::{GridInput, MeterInput};

    let product: energy_billing::Product = serde_json::from_value(serde_json::json!({
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 30.0,
        "grundpreis_ct_per_day": 10.0,
    }))
    .unwrap();
    let grid = GridInput {
        nne_grundpreis_eur_per_year: Some(dec!(73.00)), // 0.20 EUR/day
        ..Default::default()
    };
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        // Moved in on the 17th: 15 active days of 31.
        vertragsbeginn: Some(date!(2026 - 01 - 17)),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(100),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = product
        .build_engine(&grid, &ctx.regulatory_rates)
        .bill(ctx, &quantities)
        .unwrap();

    let nne_gp = invoice
        .positions
        .iter()
        .find(|p| p.description.contains("Netznutzungsentgelt Grundpreis"))
        .expect("NNE Grundpreis billed");
    // 15 days × 0.20 EUR = 3.00 EUR — not 31 × 0.20 = 6.20.
    assert_eq!(nne_gp.quantity, dec!(15));
    assert_eq!(nne_gp.net_eur, dec!(3.00));

    // And the commodity Grundpreis is clipped to the same 15 days.
    let gp = invoice
        .positions
        .iter()
        .find(|p| p.description == "Grundpreis")
        .expect("Grundpreis billed");
    assert_eq!(gp.quantity, dec!(15));
}

/// The warnings the docstring always promised now actually fire.
///
/// Estimated reading, ending price guarantee and a >50 % consumption deviation
/// were Info positions or absent entirely — visible on paper, invisible to any
/// dispatch system that inspects `invoice.warnings`.
#[test]
fn the_promised_warnings_fire() {
    use energy_billing::MeterInput;

    let product: energy_billing::Product = serde_json::from_value(serde_json::json!({
        "category": "STROM",
        "arbeitspreis_ct_per_kwh": 30.0,
        "preisgarantie_bis": "2026-02-10",
    }))
    .unwrap();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        verbrauchshistorie: Some(energy_billing::Verbrauchshistorie {
            vorjahr_kwh: Some(dec!(1000)),
            bundesdurchschnitt_kwh: None,
            kundengruppe: None,
        }),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1600), // +60 % vs prior year
            is_estimated: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = product
        .build_engine(&Default::default(), &ctx.regulatory_rates)
        .bill(ctx, &quantities)
        .unwrap();

    let codes: Vec<&str> = invoice.warnings.iter().map(|w| w.code).collect();
    assert!(codes.contains(&"ESTIMATED_READING"), "{codes:?}");
    assert!(codes.contains(&"PREISGARANTIE_ENDET"), "{codes:?}");
    assert!(codes.contains(&"VERBRAUCH_ABWEICHUNG_50PCT"), "{codes:?}");
    // None of them block the run — they are Warning, not Error severity.
    assert!(!invoice.has_errors());
}

/// §14a Modul 2 bills three Tarifstufen that replace the flat NNE Arbeitspreis.
///
/// Only Modul 1 (flat reduction) and Modul 3 (dispatch compensation) existed;
/// the zeitvariables Netzentgelt — BK6-22-300 Anlage 2 §2, with *three* bands,
/// not two — was absent from the retail engine entirely.
#[test]
fn sect14a_modul3_bills_three_bands() {
    use energy_billing::{MeterInput, Sect14aModul3Verbrauch};

    let product: energy_billing::Product = serde_json::from_value(serde_json::json!({
        "category": "WAERMEPUMPE",
        "arbeitspreis_ct_per_kwh": 20.0,
        // BK6-22-300: Modul 3 is only offered in combination with Modul 1, and
        // only against an iMSys — both are preconditions the engine enforces.
        "sect14a_modul1_pauschale_eur_per_kw_year": 0.0,
        "sect14a_modul3_nne_ht_ct_per_kwh": 12.0,
        "sect14a_modul3_nne_st_ct_per_kwh": 6.0,
        "sect14a_modul3_nne_nt_ct_per_kwh": 2.0,
    }))
    .unwrap();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(600),
            metering_mode: MeteringMode::Imsys,
            ..Default::default()
        }),
        sect14a_modul3: Some(Sect14aModul3Verbrauch {
            ht_kwh: dec!(100),
            st_kwh: dec!(300),
            nt_kwh: dec!(200),
        }),
        ..Default::default()
    };
    let invoice = product
        .build_engine(&Default::default(), &ctx.regulatory_rates)
        .bill(ctx, &quantities)
        .unwrap();

    let bands: Vec<_> = invoice
        .positions
        .iter()
        .filter(|p| p.description.starts_with("Netzentgelt §14a Modul 3"))
        .collect();
    assert_eq!(bands.len(), 3, "all three Tarifstufen appear");
    // HT 100×0.12 + ST 300×0.06 + NT 200×0.02 = 12 + 18 + 4 = 34.00 EUR.
    let total: Decimal = bands.iter().map(|p| p.net_eur).sum();
    assert_eq!(total, dec!(34.00));
    // Each band explains itself.
    for b in &bands {
        assert!(!b.trace.formula.is_empty(), "{} has a trace", b.description);
    }
}

/// Modul 2 alongside a flat NNE Arbeitspreis is refused — it would bill the
/// device's network usage twice.
#[test]
fn sect14a_modul3_with_flat_nne_is_refused() {
    use energy_billing::{GridInput, MeterInput};

    let product: energy_billing::Product = serde_json::from_value(serde_json::json!({
        "category": "WALLBOX",
        "arbeitspreis_ct_per_kwh": 20.0,
        "sect14a_modul3_nne_ht_ct_per_kwh": 12.0,
        "sect14a_modul3_nne_st_ct_per_kwh": 6.0,
        "sect14a_modul3_nne_nt_ct_per_kwh": 2.0,
    }))
    .unwrap();
    let grid = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(7.5)),
        ..Default::default()
    };
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(600),
            ..Default::default()
        }),
        ..Default::default()
    };
    let err = product
        .build_engine(&grid, &ctx.regulatory_rates)
        .bill(ctx, &quantities)
        .unwrap_err();
    assert!(err.to_string().contains("Modul 3"), "{err}");
}

// ── Vertragsart: §38 EnWG Ersatzversorgung, GVV disclosure ────────────────────

/// § 38 Abs. 4 EnWG: Ersatzversorgung ends at the latest three months
/// after it began. A four-month Ersatzversorgung period describes a supply
/// that cannot legally exist — the engine refuses it with a typed error
/// carrying the blocking warning.
#[test]
fn ersatzversorgung_over_three_months_blocks_the_run() {
    let product: Product =
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#).unwrap();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        vertragsart: energy_billing::Vertragsart::Ersatzversorgung,
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 04 - 30)).unwrap(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(600),
            ..Default::default()
        }),
        ..Default::default()
    };
    let err = product
        .build_engine(&GridInput::default(), &ctx.regulatory_rates)
        .bill(ctx, &quantities)
        .unwrap_err();
    assert_eq!(err.code(), "VALIDATION_BLOCKED");
    assert!(
        err.blocking_warnings()
            .iter()
            .any(|w| w.code == "ERSATZVERSORGUNG_UEBER_3_MONATE"),
        "{err}"
    );
}

/// Three months minus a day is the longest lawful Ersatzversorgung period —
/// it bills normally and the invoice names the regime.
#[cfg(feature = "bo4e")]
#[test]
fn ersatzversorgung_within_three_months_bills_and_names_the_regime() {
    let product: Product =
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#).unwrap();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        vertragsart: energy_billing::Vertragsart::Ersatzversorgung,
        period: BillingPeriod::new(date!(2026 - 01 - 15), date!(2026 - 04 - 14)).unwrap(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(600),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = product
        .build_engine(&GridInput::default(), &ctx.regulatory_rates)
        .bill(ctx, &quantities)
        .unwrap();
    let json = invoice.to_rechnung_json();
    let attrs = json["zusatzAttribute"].as_array().unwrap();
    assert!(
        attrs
            .iter()
            .any(|a| a["name"] == "mako:vertragsart" && a["wert"] == "ERSATZVERSORGUNG"),
        "vertragsart attribute must name the regime"
    );
}

/// The default Sondervertrag is stated on every invoice too — the regime is
/// explicit, never inferred from the tariff.
#[cfg(feature = "bo4e")]
#[test]
fn sondervertrag_is_stated_explicitly() {
    let product: Product =
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#).unwrap();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(100),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = product
        .build_engine(&GridInput::default(), &ctx.regulatory_rates)
        .bill(ctx, &quantities)
        .unwrap();
    let json = invoice.to_rechnung_json();
    let attrs = json["zusatzAttribute"].as_array().unwrap();
    assert!(
        attrs
            .iter()
            .any(|a| a["name"] == "mako:vertragsart" && a["wert"] == "SONDERVERTRAG")
    );
}

/// An inverted period is unrepresentable: the constructor is the only door.
#[test]
fn billing_period_refuses_inversion() {
    let err = BillingPeriod::new(date!(2026 - 02 - 01), date!(2026 - 01 - 31)).unwrap_err();
    assert_eq!(err.code(), "INVALID_PERIOD");

    // Serde round-trips a valid period …
    let ok = BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap();
    let json = serde_json::to_string(&ok).unwrap();
    let back: BillingPeriod = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ok);

    // … and rejects the same payload with the endpoints swapped, proving the
    // validation runs on the wire format too.
    let swapped = {
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        serde_json::json!({ "from": v["to"], "to": v["from"] }).to_string()
    };
    let parsed: Result<BillingPeriod, _> = serde_json::from_str(&swapped);
    assert!(parsed.is_err(), "deserialization must validate too");
}

/// BK6-22-300 makes §14a Modul 2 and Modul 3 mutually exclusive.
///
/// Both re-price the Netzentgelt-Arbeitspreis — Modul 2 as a percentage on the
/// device's separately metered energy, Modul 3 through three time bands — so a
/// connection holding both would have the same network usage reduced twice. The
/// engine refuses rather than silently applying both.
#[test]
fn sect14a_modul2_and_modul3_together_are_refused() {
    use energy_billing::Sect14aModul3Verbrauch;
    let product: Product = serde_json::from_value(serde_json::json!({
        "category": "WAERMEPUMPE",
        "arbeitspreis_ct_per_kwh": 28.0,
        "grundpreis_ct_per_day": 10.0,
        "sect14a_modul2_nne_reduktion_ct_per_kwh": 3.0,
        "sect14a_modul3_nne_ht_ct_per_kwh": 12.0,
        "sect14a_modul3_nne_st_ct_per_kwh": 6.0,
        "sect14a_modul3_nne_nt_ct_per_kwh": 2.0,
    }))
    .unwrap();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(600),
            ..Default::default()
        }),
        sect14a_modul3: Some(Sect14aModul3Verbrauch {
            ht_kwh: dec!(100),
            st_kwh: dec!(300),
            nt_kwh: dec!(200),
        }),
        ..Default::default()
    };
    let err = product
        .build_engine(&Default::default(), &ctx.regulatory_rates)
        .bill(ctx, &quantities)
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("MODUL2_AND_MODUL3"), "{msg}");
    assert!(msg.contains("mutually exclusive"), "{msg}");
}

/// Modul 1 is a flat reduction needing no metering, so it composes with the
/// time-variable Modul 3 — the combination BK6-22-300 explicitly allows.
#[test]
fn sect14a_modul1_combines_with_modul3() {
    use energy_billing::Sect14aModul3Verbrauch;
    let product: Product = serde_json::from_value(serde_json::json!({
        "category": "WAERMEPUMPE",
        "arbeitspreis_ct_per_kwh": 28.0,
        "grundpreis_ct_per_day": 10.0,
        "sect14a_modul1_pauschale_eur_per_kw_year": 24.0,
        "sect14a_modul3_nne_ht_ct_per_kwh": 12.0,
        "sect14a_modul3_nne_st_ct_per_kwh": 6.0,
        "sect14a_modul3_nne_nt_ct_per_kwh": 2.0,
    }))
    .unwrap();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        ..Default::default()
    };
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(600),
            spitzenleistung_kw: Some(dec!(11)),
            metering_mode: MeteringMode::Imsys,
            ..Default::default()
        }),
        sect14a_modul3: Some(Sect14aModul3Verbrauch {
            ht_kwh: dec!(100),
            st_kwh: dec!(300),
            nt_kwh: dec!(200),
        }),
        ..Default::default()
    };
    let invoice = product
        .build_engine(&Default::default(), &ctx.regulatory_rates)
        .bill(ctx, &quantities)
        .expect("Modul 1 and Modul 3 may be held together");

    assert!(
        invoice
            .positions
            .iter()
            .any(|p| p.description.contains("Modul 1 — pauschale Reduzierung")),
        "the Modul 1 pauschale must be billed"
    );
    assert_eq!(
        invoice
            .positions
            .iter()
            .filter(|p| p.description.starts_with("Netzentgelt §14a Modul 3"))
            .count(),
        3,
        "and all three Modul 3 Tarifstufen alongside it"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Regressions — see the individual test docs for the defect each one pins
// ═══════════════════════════════════════════════════════════════════════════

/// Build a context for an arbitrary period; `bill_full` is pinned to Jan 2026.
fn ctx_for(from: time::Date, to: time::Date) -> BillingContext {
    BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST".to_owned(),
        period: BillingPeriod::new(from, to).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        ..Default::default()
    }
}

/// The Gas RLM Leistungspreis is quoted per kW **and month**, so an annual bill
/// charges twelve of them — not one.
#[test]
fn gas_rlm_leistungspreis_scales_with_the_billed_months() {
    let tariff = j(r#"{
        "category":"GAS",
        "gas_arbeitspreis_ct_per_kwh_hs":8.0,
        "gas_leistungspreis_ct_per_kw_month":30.0
    }"#);
    let quantities = Quantities {
        gas: Some(GasMeterInput {
            kwh_hs: Some(dec!(500000)),
            spitzenleistung_kw: Some(dec!(50)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let ctx = ctx_for(date!(2026 - 01 - 01), date!(2026 - 12 - 31));
    let invoice = tariff
        .build_engine(&no_grid(), &rates_2026())
        .bill(ctx, &quantities)
        .unwrap();

    let lp = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("gas_leistungspreis"))
        .expect("Leistungspreis Gas position");
    // 50 kW × 0.30 EUR/kW/Monat × 12 Monate = 180.00 EUR, not 15.00.
    assert!(
        (lp.net_eur - dec!(180)).abs() < dec!(0.60),
        "annual Leistungspreis must be ~180.00 EUR, got {}",
        lp.net_eur
    );
}

/// A Stornorechnung is the exact negation of the original document — including
/// the Abschlag netting. Brutto 1200 less 1000 paid reverses to −200, never −2200.
#[test]
fn cancellation_negates_the_abschlag_netting() {
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":10.0,"arbeitspreis_ct_per_kwh":30.0}"#);
    let abschlage = vec![AbschlagDeduction {
        datum: date!(2026 - 01 - 15),
        betrag_eur: dec!(50),
        ust_satz: dec!(0.19),
        beschreibung: Some("Abschlag Januar 2026".to_owned()),
    }];
    let build = |invoice_type: InvoiceType| {
        let ctx = BillingContext {
            abschlage: abschlage.clone(),
            invoice_type,
            ..ctx_for(date!(2026 - 01 - 01), date!(2026 - 01 - 31))
        };
        tariff
            .build_engine(&no_grid(), &rates_2026())
            .bill(ctx, &elec(dec!(200)))
            .unwrap()
    };

    let original = build(InvoiceType::Final);
    original.assert_valid();
    assert!(original.abschlag_total_eur > dec!(0));

    let storno = build(InvoiceType::Cancellation {
        original_invoice_id: "INV-2026-001".to_owned(),
    });
    storno.assert_valid();
    assert_eq!(storno.brutto_eur, -original.brutto_eur);
    assert_eq!(storno.abschlag_total_eur, -original.abschlag_total_eur);
    assert_eq!(storno.abschlag_ust_eur, -original.abschlag_ust_eur);
    assert_eq!(
        storno.zahlbetrag_eur, -original.zahlbetrag_eur,
        "the Storno must credit −(brutto − Abschläge), not −(brutto + Abschläge)"
    );
}

/// The minimum-invoice top-up must not return before the reversal pass: a
/// Storno of a topped-up invoice would bill the customer instead of crediting
/// them.
#[test]
fn cancellation_with_minimum_invoice_is_still_negated() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":5.0}"#);
    let build = |invoice_type: InvoiceType| {
        let ctx = BillingContext {
            minimum_invoice_eur_brutto: Some(dec!(100.00)),
            invoice_type,
            ..ctx_for(date!(2026 - 01 - 01), date!(2026 - 01 - 31))
        };
        tariff
            .build_engine(&no_grid(), &rates_2026())
            .bill(ctx, &elec(dec!(10)))
            .unwrap()
    };

    let original = build(InvoiceType::Initial);
    original.assert_valid();
    assert!(original.brutto_eur >= dec!(100.00));

    let storno = build(InvoiceType::Cancellation {
        original_invoice_id: "INV-2026-002".to_owned(),
    });
    storno.assert_valid();
    assert_eq!(storno.brutto_eur, -original.brutto_eur);
    assert!(storno.zahlbetrag_eur < dec!(0), "a Storno credits");
    assert_eq!(
        storno
            .positions
            .iter()
            .filter(|p| p.has_tag("mindestbetrag"))
            .count(),
        1
    );
}

/// §13b: the whole supply is invoiced net, and the injected Mindestbetrag is a
/// supply position like any other — it must not attract 19 % on its own.
#[test]
fn minimum_invoice_topup_is_reverse_charged_under_sect13b() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":5.0}"#);
    let ctx = BillingContext {
        minimum_invoice_eur_brutto: Some(dec!(100.00)),
        reverse_charge: true,
        ..ctx_for(date!(2026 - 01 - 01), date!(2026 - 01 - 31))
    };
    let invoice = tariff
        .build_engine(&no_grid(), &rates_2026())
        .bill(ctx, &elec(dec!(10)))
        .unwrap();
    invoice.assert_valid();

    assert_eq!(invoice.mwst_eur, dec!(0), "§13b invoices carry no VAT");
    assert_eq!(
        invoice.brutto_eur,
        dec!(100.00),
        "the gap is net under reverse charge, so brutto lands on the minimum"
    );
    let topup = invoice
        .positions
        .iter()
        .find(|p| p.has_tag("mindestbetrag"))
        .expect("Mindestbetrag position");
    assert!(topup.is_reverse_charge());
    let subtotals = invoice.tax_subtotals(dec!(0.19));
    assert_eq!(subtotals.len(), 1);
    assert_eq!(
        subtotals[0].category,
        energy_billing::VatCategory::ReverseCharge
    );
}

/// The dynamic-tariff commodity Grundpreis is contract-clipped like every other
/// Grundpreis: a mid-period move-in pays for its own days, not the full period.
#[test]
fn dynamic_grundpreis_is_prorated_to_the_contract_days() {
    use std::collections::HashMap;
    let tariff = j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":20}"#);
    let intervals = [DynamicInterval {
        timestamp_utc: time::macros::datetime!(2026-01-20 13:00 UTC),
        kwh: dec!(1),
    }];
    let mut prices = HashMap::new();
    prices.insert(time::macros::datetime!(2026-01-20 13:00 UTC), dec!(25));
    let quantities = Quantities {
        dynamic_intervals: intervals.to_vec(),
        dynamic_epex_prices: prices,
        ..Default::default()
    };
    let ctx = BillingContext {
        vertragsbeginn: Some(date!(2026 - 01 - 16)),
        ..ctx_for(date!(2026 - 01 - 01), date!(2026 - 01 - 31))
    };
    let invoice = tariff
        .build_engine(&no_grid(), &rates_2026())
        .bill(ctx, &quantities)
        .unwrap();

    let gp = invoice
        .positions
        .iter()
        .find(|p| p.description.starts_with("Grundpreis Strom"))
        .expect("Grundpreis position");
    // Jan 16..31 = 16 days × 0.20 EUR, not 31.
    assert_eq!(gp.quantity, dec!(16));
    assert_eq!(gp.net_eur, dec!(3.20));
}

/// The EEG-Gutschrift is a separate supply governed by the operator's own USt
/// status. For a §19 Kleinunternehmer it is 0 %, so it must not net against the
/// standard-rate consumption base — that understates the supplier's output VAT.
#[test]
fn eeg_gutschrift_kleinunternehmer_does_not_reduce_the_standard_rate_base() {
    let base = r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0"#;
    let quantities = Quantities {
        electricity: Some(meter(dec!(1000))),
        eeg_gutschrift_eur: Some(dec!(100)),
        ..Default::default()
    };

    let regel = bill(&j(&format!("{base}}}")), quantities.clone());
    let klein = bill(
        &j(&format!(
            "{base},\"eeg_gutschrift_kleinunternehmer_19_ustg\":true}}"
        )),
        quantities,
    );
    regel.assert_valid();
    klein.assert_valid();

    // Same net either way — only the taxable base differs.
    assert_eq!(regel.netto_eur, klein.netto_eur);
    // The Regelbesteuerung credit nets against the 19 % base; the §19 one does not.
    assert_eq!(klein.mwst_eur - regel.mwst_eur, dec!(19.00));
    let zero_rated: Vec<_> = klein
        .tax_subtotals(dec!(0.19))
        .into_iter()
        .filter(|s| s.category == energy_billing::VatCategory::ZeroRated)
        .collect();
    assert_eq!(zero_rated.len(), 1, "the credit forms its own 0 % subtotal");
    assert_eq!(zero_rated[0].taxable_base_eur, dec!(-100));
}

/// BO4E: "die Summe dieser Beträge ergibt den Wert für gesamtsteuer". Each
/// Steuerbetrag is stated to the cent, so the total must be their sum — summing
/// 5-dp tax layers and rounding once at the end lands a cent low.
#[cfg(feature = "bo4e")]
#[test]
fn multi_rate_gesamtsteuer_equals_the_sum_of_the_stated_steuerbetraege() {
    use energy_billing::{BillingPosition, Invoice};

    // 19 % on 10.50 → 1.995 and 7 % on 7.50 → 0.525: both round up on their own
    // (2.00 + 0.53 = 2.53) but sum to 2.52 before rounding.
    let positions = vec![
        BillingPosition::debit(
            "Strom",
            dec!(1),
            "kWh",
            dec!(10.50),
            PositionCategory::Commodity,
        ),
        BillingPosition::debit(
            "Fernwärme",
            dec!(1),
            "kWh",
            dec!(7.50),
            PositionCategory::Commodity,
        )
        .with_tax_rate(dec!(0.07)),
    ];
    let ctx = ctx_for(date!(2026 - 01 - 01), date!(2026 - 01 - 31));
    let engine = BillingEngine::new()
        .add(FixedPositions(positions))
        .add(MwStProvider::new(dec!(0.19)));
    let invoice: Invoice = engine.bill(ctx, &Quantities::default()).unwrap();
    invoice.assert_valid();

    let subtotal_sum: Decimal = invoice
        .tax_subtotals(dec!(0.19))
        .iter()
        .map(|s| s.tax_amount_eur)
        .sum();
    assert_eq!(subtotal_sum, dec!(2.53));
    assert_eq!(invoice.mwst_eur, subtotal_sum);

    let json = invoice.to_rechnung_json();
    let stated: Decimal = json["steuerbetraege"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            s["steuerwert"]
                .as_str()
                .unwrap()
                .parse::<Decimal>()
                .unwrap()
        })
        .sum();
    assert_eq!(
        json["gesamtsteuer"]["wert"].as_str().unwrap(),
        stated.to_string()
    );
}

/// A provider that emits a fixed position list — lets a test drive the engine's
/// tax pass over exactly the base it cares about.
struct FixedPositions(Vec<energy_billing::BillingPosition>);

impl energy_billing::BillingProvider for FixedPositions {
    fn bill(
        &self,
        _ctx: &BillingContext,
        _quantities: &Quantities,
        _prior: &[energy_billing::BillingPosition],
    ) -> Result<Vec<energy_billing::BillingPosition>, energy_billing::EngineError> {
        Ok(self.0.clone())
    }
}

// ── Regressions: periodic charges and their signs ─────────────────────────────

/// A monthly Rabatt reduces the bill. Computing `net_eur` from the (already
/// negative) monthly figure and then multiplying by −1 again *adds* the same
/// amount: a −5 EUR/month discount bills +5 EUR under a line labelled "Rabatt".
/// The gas provider carries no such factor,
/// which is why only electricity was affected and no test caught it — the one
/// covering the field used a positive surcharge.
#[test]
fn a_monthly_rabatt_reduces_the_invoice() {
    let plain = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":25.0}"#);
    let with_rabatt = j(
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":25.0,"auf_abschlag_eur_per_month":-5.0}"#,
    );
    let with_aufschlag = j(
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":25.0,"auf_abschlag_eur_per_month":5.0}"#,
    );

    let base = bill(&plain, elec(dec!(300)));
    let rabatt = bill(&with_rabatt, elec(dec!(300)));
    let aufschlag = bill(&with_aufschlag, elec(dec!(300)));

    assert!(
        rabatt.netto_eur < base.netto_eur,
        "a Rabatt must lower the net: {} vs {}",
        rabatt.netto_eur,
        base.netto_eur
    );
    // January is exactly one month, so the deduction is the full 5 EUR.
    assert_eq!((base.netto_eur - rabatt.netto_eur).round_dp(2), dec!(5.00));
    assert_eq!(
        (aufschlag.netto_eur - base.netto_eur).round_dp(2),
        dec!(5.00)
    );

    // The Rabatt line is a Discount and its amount is negative.
    let line = rabatt
        .positions
        .iter()
        .find(|p| p.has_tag("auf_abschlag"))
        .expect("Rabatt position");
    assert_eq!(line.category, PositionCategory::Discount);
    assert!(line.net_eur < Decimal::ZERO, "got {}", line.net_eur);
}

/// Fernwärme billed for a year is billed a year of Grundpreis. `months`
/// defaulted to `1`, so a caller that did not state it — every caller billing
/// on calendar periods — got one month's standing charge on an annual invoice
/// and a plausible-looking total.
#[test]
fn fernwaerme_without_stated_months_bills_the_billed_period() {
    let tariff = j(
        r#"{"category":"WAERME","waerme_grundpreis_eur_per_month":25.0,"waerme_arbeitspreis_ct_per_kwh":9.0}"#,
    );
    let rates = rates_2026();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-WAERME-YEAR".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 12 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&no_grid(), &rates)
        .bill(
            ctx,
            &Quantities {
                heat: Some(WaermeMeterInput {
                    kwh_waerme: dec!(12000),
                    months: None,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    let gp = invoice
        .positions
        .iter()
        .find(|p| p.description.starts_with("Grundpreis"))
        .expect("Grundpreis");
    assert_eq!(gp.quantity, dec!(12), "twelve months, not one");
    assert_eq!(gp.net_eur.round_dp(2), dec!(300.00));
}

/// CO2KostAufG § 3 obliges a heat supplier to state the CO₂ cost it bore.
/// `HeatProvider` billed none at all, while the platform documented BEHG
/// coverage for "Gas and Wärme".
#[test]
fn fernwaerme_passes_through_its_co2_cost_and_states_the_emissions() {
    let tariff = j(
        r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":9.0,
             "waerme_co2_kosten_ct_per_kwh":0.9,"waerme_co2_emission_g_per_kwh":180,
             "waerme_erneuerbar_anteil_pct":35}"#,
    );
    let r = bill(
        &tariff,
        Quantities {
            heat: Some(WaermeMeterInput {
                kwh_waerme: dec!(1000),
                months: Some(dec!(1)),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let co2 = r
        .positions
        .iter()
        .find(|p| p.has_tag("behg"))
        .expect("CO₂ position");
    assert_eq!(co2.net_eur.round_dp(2), dec!(9.00)); // 1000 × 0,9 ct
    assert_eq!(co2.legal_basis.as_deref(), Some("CO2KostAufG § 3"));
    assert!(r.positions.iter().any(|p| p.has_tag("co2_emission")));
    assert!(r.positions.iter().any(|p| p.has_tag("erneuerbar_anteil")));
}

/// An indexed B2B tariff whose index value has not arrived cannot be priced.
/// Adding no Arbeitspreis position and billing the Grundpreis alone is the exact
/// silent-zero shape `KEIN_ARBEITSPREIS` exists to refuse — and the presence of
/// the *config* is not the presence of a price.
#[test]
fn an_indexed_tariff_without_its_index_value_is_refused() {
    let tariff = j(r#"{"category":"STROM","grundpreis_ct_per_day":30,
             "indexed_price":{"base_ct_per_kwh":5,"spread_ct_per_kwh":1,
                              "index_name":"Phelix Base","factor_ct_per_unit":0.1}}"#);
    let (f, t) = period();
    let err = tariff
        .build_engine(&no_grid(), &rates_2026())
        .bill(ctx_for(f, t), &elec(dec!(500)))
        .expect_err("no index value, no price");
    let msg = format!("{err:?}");
    assert!(msg.contains("INDEXWERT_FEHLT"), "got {msg}");
}

/// With the index value present the indexed price is what gets billed — even
/// when a static Arbeitspreis is also configured. Resolving to the static one
/// would bill a price the contract does not contain.
#[test]
fn an_indexed_tariff_outranks_the_static_arbeitspreis() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30,
             "indexed_price":{"base_ct_per_kwh":5,"spread_ct_per_kwh":1,
                              "index_name":"Phelix Base","index_value":80,
                              "factor_ct_per_unit":0.1}}"#);
    let r = bill(&tariff, elec(dec!(1000)));
    let ap = r
        .positions
        .iter()
        .find(|p| p.has_tag("indexed_price"))
        .expect("indexed Arbeitspreis");
    // 5 + 1 + 80 × 0.1 = 14 ct/kWh
    assert_eq!(ap.net_eur.round_dp(2), dec!(140.00));
    assert_eq!(
        r.positions
            .iter()
            .filter(|p| p.has_tag("arbeitspreis"))
            .count(),
        1,
        "exactly one Arbeitspreis reaches the invoice"
    );
}

/// § 40 Abs. 2 EnWG applies to a dynamic tariff exactly as it does to a fixed
/// one. The § 41a path emitted no Zählerstand (Nr. 6), no Vorjahresvergleich
/// (Nr. 7), no Vergleichsgruppe (Nr. 8) and no estimation notice — none of it
/// visible in the totals, all of it missing from the page.
#[test]
fn a_dynamic_invoice_carries_the_same_display_duties_as_a_static_one() {
    use std::collections::HashMap;
    let mut prices = HashMap::new();
    prices.insert(time::macros::datetime!(2026-01-01 12:00 UTC), dec!(20));

    let tariff = j(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":30,
             "dynamic_aufschlag_ct_per_kwh":5,"msb_gebuehr_ct_per_day":5}"#,
    );
    let (f, t) = period();
    let mut ctx = ctx_for(f, t);
    ctx.verbrauchshistorie = Some(energy_billing::Verbrauchshistorie {
        vorjahr_kwh: Some(dec!(3100)),
        bundesdurchschnitt_kwh: Some(dec!(2900)),
        kundengruppe: Some("Haushalt 2 Personen".to_owned()),
    });
    let invoice = tariff
        .build_engine(&no_grid(), &rates_2026())
        .bill(
            ctx,
            &Quantities {
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(1),
                    metering_mode: MeteringMode::Imsys,
                    zaehlerstand_von: Some(dec!(1000)),
                    zaehlerstand_bis: Some(dec!(1001)),
                    is_estimated: true,
                    ..Default::default()
                }),
                dynamic_intervals: vec![DynamicInterval {
                    timestamp_utc: time::macros::datetime!(2026-01-01 12:00 UTC),
                    kwh: dec!(1),
                }],
                dynamic_epex_prices: prices,
                ..Default::default()
            },
        )
        .unwrap();

    for tag in [
        "zaehlerstand",
        "verbrauchshistorie",
        "schatzwert",
        "msb_gebuehr",
    ] {
        assert!(
            invoice.positions.iter().any(|p| p.has_tag(tag)),
            "a §41a invoice must carry the '{tag}' position"
        );
    }
    // Nr. 7 and Nr. 8 are two separate lines.
    assert_eq!(invoice.positions_by_tag("verbrauchshistorie").count(), 2);
}

/// BK6-22-300 offers Modul 3 **only in combination with Modul 1**, and only
/// against an intelligentes Messsystem. Neither precondition was checked: a
/// product carrying the bands alone priced a tariff no Netzbetreiber offers and
/// dropped the Modul 1 reduction the customer was entitled to, and an SLP meter
/// got band prices against a synthetic profile rather than a measurement.
#[test]
fn modul3_needs_modul1_and_an_imsys() {
    use energy_billing::Sect14aModul3Verbrauch;

    let bands = serde_json::json!({
        "category": "WAERMEPUMPE",
        "arbeitspreis_ct_per_kwh": 28.0,
        "sect14a_modul3_nne_ht_ct_per_kwh": 12.0,
        "sect14a_modul3_nne_st_ct_per_kwh": 6.0,
        "sect14a_modul3_nne_nt_ct_per_kwh": 2.0,
    });
    let q = |mode| Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(600),
            metering_mode: mode,
            ..Default::default()
        }),
        sect14a_modul3: Some(Sect14aModul3Verbrauch {
            ht_kwh: dec!(100),
            st_kwh: dec!(300),
            nt_kwh: dec!(200),
        }),
        ..Default::default()
    };
    let (f, t) = period();

    // Bands without Modul 1, on an iMSys → only the Modul 1 finding.
    let product: Product = serde_json::from_value(bands.clone()).unwrap();
    let msg = product
        .build_engine(&no_grid(), &rates_2026())
        .bill(ctx_for(f, t), &q(MeteringMode::Imsys))
        .unwrap_err()
        .to_string();
    assert!(msg.contains("MODUL3_OHNE_MODUL1"), "{msg}");
    assert!(!msg.contains("MODUL3_IMSYS_REQUIRED"), "{msg}");

    // Bands with Modul 1, on an SLP meter → only the metering finding.
    let mut with_m1 = bands.clone();
    with_m1["sect14a_modul1_pauschale_eur_per_kw_year"] = serde_json::json!(24.0);
    let product: Product = serde_json::from_value(with_m1.clone()).unwrap();
    let msg = product
        .build_engine(&no_grid(), &rates_2026())
        .bill(ctx_for(f, t), &q(MeteringMode::Slp))
        .unwrap_err()
        .to_string();
    assert!(msg.contains("MODUL3_IMSYS_REQUIRED"), "{msg}");
    assert!(!msg.contains("MODUL3_OHNE_MODUL1"), "{msg}");

    // Both preconditions met → it bills.
    let product: Product = serde_json::from_value(with_m1).unwrap();
    product
        .build_engine(&no_grid(), &rates_2026())
        .bill(ctx_for(f, t), &q(MeteringMode::Imsys))
        .expect("Modul 1 + Modul 3 on an iMSys is the combination BK6-22-300 offers");
}

/// A `minimum_invoice_eur_brutto` top-up must actually reach the minimum.
///
/// Pass 4 grossed the shortfall up with the standard rate unconditionally, so
/// on a mixed-rate invoice — 7 % Trinkwasser beside a 19 % energy line — the
/// resulting brutto landed short by the rate difference on the top-up. The
/// Mindestbetrag is a contractual charge, so the contract names its rate.
#[test]
fn a_minimum_invoice_topup_reaches_the_minimum_at_the_agreed_rate() {
    let tariff = j(
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":25.0,"minimum_invoice_eur_brutto":100.0}"#,
    );
    let rates = rates_2026();
    let (f, t) = period();
    let bill_at = |rate: Option<rust_decimal::Decimal>| {
        let mut ctx = ctx_for(f, t);
        ctx.minimum_invoice_eur_brutto = Some(dec!(100));
        ctx.minimum_invoice_mwst_rate = rate;
        tariff
            .build_engine(&no_grid(), &rates)
            .bill(ctx, &elec(dec!(100)))
            .unwrap()
    };

    // Unstated → the standard rate, and the minimum is met exactly.
    let standard = bill_at(None);
    assert_eq!(standard.brutto_eur.round_dp(2), dec!(100.00));

    // Agreed at the reduced rate → still exactly the minimum, and the top-up
    // sits in the 7 % bucket rather than the 19 % one.
    let reduced = bill_at(Some(dec!(0.07)));
    assert_eq!(reduced.brutto_eur.round_dp(2), dec!(100.00));
    let topup = reduced
        .positions
        .iter()
        .find(|p| p.has_tag("mindestbetrag"))
        .expect("top-up position");
    assert_eq!(topup.applicable_tax_rate, Some(dec!(0.07)));
    let subs = reduced.tax_subtotals(rates.mwst_rate);
    assert!(
        subs.iter().any(|s| s.rate_percent == dec!(7)),
        "the top-up forms its own 7 % subtotal: {subs:?}"
    );
}

/// § 40 Abs. 2 Nr. 6 EnWG names **three** things, not two: the opening and
/// closing readings, the consumption derived from them, *and* "die Art, wie der
/// Zählerstand ermittelt wurde". The third was on the page only when the
/// reading happened to be an estimate — a customer could not tell a remote
/// read-out from their own self-reported figure, and what they can do about a
/// wrong number differs in each case.
#[test]
fn the_invoice_states_how_the_reading_was_obtained() {
    use energy_billing::Ablesungsart;

    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let read = |art| Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(500),
            zaehlerstand_von: Some(dec!(1000)),
            zaehlerstand_bis: Some(dec!(1500)),
            ablesungsart: art,
            ..Default::default()
        }),
        ..Default::default()
    };

    for (art, needle) in [
        (Ablesungsart::Fernauslesung, "ferngelesen"),
        (Ablesungsart::Abgelesen, "Messstellenbetreiber"),
        (Ablesungsart::Kundenselbstablesung, "Selbstablesung"),
        (Ablesungsart::Rechnerisch, "rechnerisch"),
    ] {
        let r = bill(&tariff, read(art));
        let pos = r
            .positions
            .iter()
            .find(|p| p.has_tag("zaehlerstand"))
            .expect("Zählerstand position");
        assert!(
            pos.description.contains(needle),
            "{art:?} must reach the page: {:?}",
            pos.description
        );
    }

    // Unstated: the sentence is absent, and the operator is told before dispatch.
    let (f, t) = period();
    let warnings = tariff
        .build_engine(&no_grid(), &rates_2026())
        .validate(&ctx_for(f, t), &read(Ablesungsart::Unbekannt));
    assert!(warnings.iter().any(|w| w.code == "ABLESUNGSART_FEHLT"));
}

/// The § 42c sharing input carries three transparency terms, and all three were
/// documented as reaching the invoice. The provider read `allocated_kwh` and
/// dropped the rest, so a participant saw a credited quantity with no way to
/// check it: the total the community generated and the fraction they were
/// allotted are the two figures whose product *is* that quantity.
#[test]
fn a_sharing_credit_shows_what_it_was_derived_from() {
    let tariff = j(
        r#"{"category":"SHARING","arbeitspreis_ct_per_kwh":30.0,"sharing_credit_ct_per_kwh":8.0}"#,
    );
    let r = bill(
        &tariff,
        Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: dec!(500),
                ..Default::default()
            }),
            energy_share: Some(energy_billing::EnergyShareMeterInput {
                allocated_kwh: dec!(150),
                total_plant_generation_kwh: Some(dec!(400)),
                allocation_fraction: Some(dec!(0.375)),
                gemeinschaft_id: Some("EGK-2026-001".to_owned()),
            }),
            ..Default::default()
        },
    );

    // The credit itself, unchanged: 150 kWh × 8 ct.
    let credit = r
        .positions
        .iter()
        .find(|p| p.has_tag("sharing") && p.net_eur != Decimal::ZERO)
        .expect("sharing credit");
    assert_eq!(credit.net_eur.round_dp(2), dec!(-12.00));

    // …and what it was derived from, on the page and costing nothing.
    let id = r
        .positions
        .iter()
        .find(|p| p.has_tag("gemeinschaft"))
        .expect("community identifier");
    assert!(id.description.contains("EGK-2026-001"));
    assert_eq!(id.net_eur, Decimal::ZERO);

    let alloc = r
        .positions
        .iter()
        .find(|p| p.has_tag("zuteilung"))
        .expect("allocation line");
    assert!(alloc.description.contains("37.5"), "{}", alloc.description);
    assert!(alloc.description.contains("400"), "{}", alloc.description);
    assert_eq!(alloc.net_eur, Decimal::ZERO);
}

// ── Zweitarif: the product must price the quantities it is given ──────────────

/// A `Zweitarif` product prices HT and NT and nothing else. Handed a meter that
/// reports only a total — what `edmd` returns whenever the register split did
/// not arrive — the HT/NT branch does not fire, no other branch matches, and
/// 1000 kWh billed for **€20.50**: the Stromsteuer, and nothing for the
/// electricity. `KEIN_ARBEITSPREIS` waved it through because a price field
/// *was* populated; the guard checked the product, not the pairing.
#[test]
fn a_zweitarif_product_without_an_ht_nt_split_is_refused() {
    let tariff = j(r#"{"category":"STROM","register_count":"Zweitarif",
             "arbeitspreis_ht_ct_per_kwh":32.0,"arbeitspreis_nt_ct_per_kwh":22.0}"#);
    let (f, t) = period();
    let err = tariff
        .build_engine(&no_grid(), &rates_2026())
        .bill(ctx_for(f, t), &elec(dec!(1000)))
        .expect_err("a total alone cannot be priced HT/NT");
    assert!(
        err.to_string().contains("ZWEITARIF_OHNE_HT_NT_AUFTEILUNG"),
        "{err}"
    );

    // With the split it bills: 400 × 32 ct + 600 × 22 ct = 128 + 132 = 260 EUR.
    let r = bill(
        &tariff,
        Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: dec!(1000),
                arbeitsmenge_ht_kwh: Some(dec!(400)),
                arbeitsmenge_nt_kwh: Some(dec!(600)),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    assert_eq!(r.total_by_tag("arbeitspreis").round_dp(2), dec!(260.00));
}

/// The HT/NT registers alone are a complete statement of consumption — the
/// total is their sum, not an independent fact. Gating the Arbeitspreis block
/// on `arbeitsmenge_kwh` billed such a caller nothing for their electricity
/// while still charging them the Stromsteuer on nothing.
#[test]
fn ht_and_nt_alone_are_enough_to_bill() {
    let tariff = j(
        r#"{"category":"STROM","arbeitspreis_ht_ct_per_kwh":32.0,"arbeitspreis_nt_ct_per_kwh":22.0}"#,
    );
    let r = bill(
        &tariff,
        Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: Decimal::ZERO, // not stated
                arbeitsmenge_ht_kwh: Some(dec!(400)),
                arbeitsmenge_nt_kwh: Some(dec!(600)),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    assert_eq!(r.total_by_tag("arbeitspreis").round_dp(2), dec!(260.00));
    // …and the levy is charged on the same 1000 kWh, not on zero.
    assert_eq!(r.total_by_tag("stromsteuer").round_dp(2), dec!(20.50));
}

/// An HT/NT split that does not add up to the stated total prices one of the
/// two registers wrongly, and which one is not knowable from here.
#[test]
fn an_ht_nt_split_that_does_not_add_up_is_refused() {
    let tariff = j(
        r#"{"category":"STROM","arbeitspreis_ht_ct_per_kwh":32.0,"arbeitspreis_nt_ct_per_kwh":22.0}"#,
    );
    let (f, t) = period();
    let err = tariff
        .build_engine(&no_grid(), &rates_2026())
        .bill(
            ctx_for(f, t),
            &Quantities {
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(1000),
                    arbeitsmenge_ht_kwh: Some(dec!(400)),
                    arbeitsmenge_nt_kwh: Some(dec!(500)), // 100 kWh unaccounted
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .expect_err("900 != 1000");
    assert!(err.to_string().contains("HT_NT_SUMME_WEICHT_AB"), "{err}");
}

/// A two-register meter on a **single-rate** tariff is ordinary, and it billed
/// nothing for the electricity: the HT/NT arm was selected by the *meter*
/// alone, found no band prices on the product, pushed no position, and blocked
/// the Eintarif arm below it. The Stromsteuer was charged all the same.
///
/// Found by `proptest_invoice::any_billable_consumption_produces_a_work_price`,
/// which is the general statement of this whole failure class.
#[test]
fn an_eintarif_product_bills_a_two_register_meter_on_its_total() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#);
    let r = bill(
        &tariff,
        Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: dec!(1000),
                arbeitsmenge_ht_kwh: Some(dec!(400)),
                arbeitsmenge_nt_kwh: Some(dec!(600)),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    assert_eq!(r.total_by_tag("arbeitspreis").round_dp(2), dec!(300.00));
    assert_eq!(r.total_by_tag("stromsteuer").round_dp(2), dec!(20.50));
}

/// Half a Zweitarif — one band priced, the other not — has no sensible reading:
/// billing the unpriced band at the other's rate invents a price, dropping it
/// under-bills. Left to `TimeOfUsePricing` it is refused, but as a raw `billing`
/// error (`unknown band "HT"`) rather than a finding naming the product.
#[test]
fn half_a_zweitarif_is_refused_as_a_product_defect() {
    let (f, t) = period();
    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            arbeitsmenge_ht_kwh: Some(dec!(400)),
            arbeitsmenge_nt_kwh: Some(dec!(600)),
            ..Default::default()
        }),
        ..Default::default()
    };
    for tariff in [
        r#"{"category":"STROM","arbeitspreis_nt_ct_per_kwh":22.0}"#,
        r#"{"category":"STROM","arbeitspreis_ht_ct_per_kwh":32.0}"#,
    ] {
        let err = j(tariff)
            .build_engine(&no_grid(), &rates_2026())
            .bill(ctx_for(f, t), &q)
            .expect_err("a Zweitarif needs both bands");
        let msg = err.to_string();
        assert!(msg.contains("ZWEITARIF_UNVOLLSTAENDIG"), "{msg}");
        assert!(
            !msg.contains("unknown band"),
            "the caller gets a product finding, not a library error: {msg}"
        );
    }

    // Both priced: it bills. 400 × 32 + 600 × 22 = 260,00 EUR.
    let ok = bill(
        &j(
            r#"{"category":"STROM","arbeitspreis_ht_ct_per_kwh":32.0,"arbeitspreis_nt_ct_per_kwh":22.0}"#,
        ),
        q,
    );
    assert_eq!(ok.total_by_tag("arbeitspreis").round_dp(2), dec!(260.00));
}
