//! The request bodies the README and the operator guide print must parse.
//!
//! A documented example that does not deserialise is worse than none: it is
//! read as authoritative and costs an operator an afternoon.

use netzbilanzd::request::{BillingRunRequest, SettlementRequest};

fn parse(body: serde_json::Value) -> BillingRunRequest {
    serde_json::from_value(body).expect("the documented body must parse")
}

/// The README quick-start and the operator guide's fuller NNE example.
#[test]
fn the_documented_nne_examples_parse() {
    let readme = parse(serde_json::json!({
        "invoice_date": "2026-02-01",
        "due_date": "2026-03-03",
        "rechnungskreis": "NNE",
        "positions": [{
            "malo_id": "51238696012",
            "period_from": "2026-01-01",
            "period_to": "2026-01-31",
            "settlement": {
                "billing_type": "nne",
                "nb_mp_id": "9900357000004",
                "lf_mp_id": "9900012345678",
                "sparte": "Strom",
                "arbeitspreis": { "Einheitlich": { "menge_kwh": "1500", "preis_ct_per_kwh": "3.5" } },
                "konzessionsabgabe": { "satz_ct_per_kwh": "0.11", "klasse": "Sondervertragskunde" },
                "netzebene": "Niederspannung"
            }
        }]
    }));
    assert_eq!(readme.positions.len(), 1);

    let guide = parse(serde_json::json!({
        "invoice_date": "2026-02-01",
        "due_date": "2026-03-03",
        "rechnungskreis": "NNE",
        "positions": [{
            "malo_id": "51238696012",
            "period_from": "2026-01-01",
            "period_to": "2026-01-31",
            "settlement": {
                "billing_type": "nne",
                "nb_mp_id": "9900357000004",
                "lf_mp_id": "9900012345678",
                "sparte": "Strom",
                "arbeitspreis": { "Einheitlich": { "menge_kwh": "1500", "preis_ct_per_kwh": "3.5" } },
                // A monthly NN-Rechnung bills the Preisblatt's
                // **Monatsleistungspreis**: EUR/kW·Monat against the month's
                // Höchstleistung (§ 17 Abs. 8 StromNEV names the
                // Monatsleistungspreissystem alongside the annual one).
                //
                // Omitting `system` defaults to `JAHR`, which is § 17 Abs. 2
                // Satz 2's Jahresleistungspreis × Jahreshöchstleistung — the
                // whole Abrechnungsjahr's demand charge, correct only on a
                // yearly settlement. The engine bills it and warns
                // (`JAHRESLEISTUNGSPREIS_UNTERJAEHRIG`) rather than inventing a
                // pro-rating no Preisblatt publishes, so the documented example
                // has to say which system it is on.
                "leistungspreis": {
                    "spitzenleistung_kw": "40",
                    "preis_eur_per_kw": "12.50",
                    "system": { "MONAT": { "monate": "1" } }
                },
                "konzessionsabgabe": { "satz_ct_per_kwh": "0.11", "klasse": "Sondervertragskunde" },
                "netzebene": "Niederspannung",
                "jahresarbeit_kwh": "18000",
                "tariff_sheet_id": "Preisblatt-NNE-2026-Q1"
            }
        }]
    }));
    let SettlementRequest::Nne(nne) = &guide.positions[0].settlement else {
        panic!("expected an NNE settlement");
    };
    let lp = nne.leistungspreis.expect("the documented Leistungspreis");
    assert!(nne.netzebene.is_some());
    // The documented example is a **monthly** settlement, so it states the
    // Monatsleistungspreissystem rather than inheriting the `JAHR` default.
    assert_eq!(
        lp.system,
        grid_billing::LeistungspreisSystem::Monat {
            monate: rust_decimal::Decimal::ONE
        },
        "the documented January settlement bills one month of Leistungspreis"
    );

    // …and it settles to the figures the guide prints. A documented response
    // nobody recomputes drifts silently the moment the engine changes: this
    // example's demand charge moved from a pro-rated 42.47 € to the full
    // 500.00 € when `LeistungspreisSystem` arrived, and the printed totals
    // stayed where they were.
    let settled = netzbilanzd::billing::settle(&guide.positions[0]).expect("the guide settles");
    assert_eq!(settled.total_eur.to_string(), "598.34", "netto_eur");
    assert_eq!(
        settled.steuer.steuer_eur.to_string(),
        "113.68",
        "steuer_eur"
    );
    assert_eq!(
        settled.steuer.brutto_eur().to_string(),
        "712.02",
        "brutto_eur"
    );
    assert!(
        settled.warnings.is_empty(),
        "a monthly Leistungspreis over a monthly period warns about nothing: {:?}",
        settled.warnings
    );
}

/// Both Leistungspreissysteme reach the engine off the wire.
///
/// `system` rides in on `grid_billing::Leistungspreis` and has a serde default,
/// so a body that omits it silently bills § 17 Abs. 2 Satz 2's annual
/// Jahresleistungsentgelt — the whole Abrechnungsjahr's demand charge — on a
/// monthly invoice. That default is a real answer, not a placeholder, so the
/// caller has to be able to say the other one and be believed.
#[test]
fn the_leistungspreissystem_is_stateable_on_the_api() {
    use grid_billing::LeistungspreisSystem;
    use rust_decimal::Decimal;

    let with = |system: serde_json::Value| -> LeistungspreisSystem {
        let body = serde_json::json!({
            "invoice_date": "2026-02-01",
            "due_date": "2026-03-03",
            "positions": [{
                "malo_id": "51238696012",
                "period_from": "2026-01-01",
                "period_to": "2026-01-31",
                "settlement": {
                    "billing_type": "nne",
                    "nb_mp_id": "9900357000004",
                    "lf_mp_id": "9900012345678",
                    "sparte": "Strom",
                    "arbeitspreis": { "Einheitlich": { "menge_kwh": "1500", "preis_ct_per_kwh": "3.5" } },
                    "leistungspreis": {
                        "spitzenleistung_kw": "40",
                        "preis_eur_per_kw": "12.50",
                        "system": system,
                    },
                }
            }]
        });
        let SettlementRequest::Nne(nne) = parse(body).positions.remove(0).settlement else {
            panic!("expected an NNE settlement");
        };
        nne.leistungspreis.expect("leistungspreis").system
    };

    assert_eq!(with(serde_json::json!("JAHR")), LeistungspreisSystem::Jahr);
    assert_eq!(
        with(serde_json::json!({ "MONAT": { "monate": "3" } })),
        LeistungspreisSystem::Monat {
            monate: Decimal::from(3)
        }
    );
}

/// All three §14a module bodies the docs print.
#[test]
fn the_documented_sect14a_modules_parse() {
    for arbeitspreis in [
        serde_json::json!({ "Modul1Pauschal": {
            "basis": { "menge_kwh": "1500", "preis_ct_per_kwh": "3.5" },
            "pauschale_eur_pro_jahr": "120.00", "jahresanteil": "0.0849"
        }}),
        serde_json::json!({ "Modul2ProzentualeReduzierung": {
            "basis": { "menge_kwh": "800", "preis_ct_per_kwh": "3.5" }, "reduktion": "0.85"
        }}),
        serde_json::json!({ "Modul3ZeitVariabel": {
            "ht": { "menge_kwh": "600", "preis_ct_per_kwh": "4.20" },
            "st": { "menge_kwh": "100", "preis_ct_per_kwh": "3.00" },
            "nt": { "menge_kwh": "400", "preis_ct_per_kwh": "1.50" }
        }}),
    ] {
        parse(serde_json::json!({
            "invoice_date": "2026-02-01",
            "due_date": "2026-03-03",
            "positions": [{
                "malo_id": "51238696012",
                "period_from": "2026-01-01",
                "period_to": "2026-01-31",
                "settlement": {
                    "billing_type": "nne",
                    "nb_mp_id": "9900357000004",
                    "lf_mp_id": "9900012345678",
                    "sparte": "Strom",
                    "arbeitspreis": arbeitspreis
                }
            }]
        }));
    }
}

/// The demo smoke test's body, which is the one an operator runs first.
#[test]
fn the_smoke_test_body_parses() {
    parse(serde_json::json!({
        "invoice_date": "2026-02-01",
        "due_date": "2026-03-03",
        "rechnungskreis": "SMOKE-1",
        "positions": [{
            "malo_id": "51238696012",
            "period_from": "2026-01-01",
            "period_to": "2026-01-28",
            "settlement": {
                "billing_type": "nne",
                "nb_mp_id": "9900357000004",
                "lf_mp_id": "9900012345678",
                "sparte": "Strom",
                "arbeitspreis": { "Einheitlich": { "menge_kwh": "1500.000", "preis_ct_per_kwh": "3.500" } },
                "konzessionsabgabe": { "satz_ct_per_kwh": "0.110", "klasse": "Sondervertragskunde" }
            }
        }]
    }));
}

/// The §13b reverse-charge shape the README and the operator guide print.
///
/// A Mehr-/Mindermenge is a Lieferung, so the two §3g facts travel on the
/// settlement; both documents show them in exactly this form.
#[test]
fn the_documented_wiederverkaeufer_shape_parses() {
    let run = parse(serde_json::json!({
        "invoice_date": "2026-02-01",
        "due_date": "2026-03-03",
        "positions": [{
            "malo_id": "51238696012",
            "period_from": "2026-01-01",
            "period_to": "2026-01-31",
            "settlement": {
                "billing_type": "mmm",
                "nb_mp_id": "9900357000004",
                "lf_mp_id": "9900012345678",
                "sparte": "Strom",
                "gemessen_kwh": "1200.000",
                "bilanziert_kwh": "1000.000",
                "wiederverkaeufer": { "leistender": true, "empfaenger": true }
            }
        }]
    }));
    let SettlementRequest::Mmm(mmm) = &run.positions[0].settlement else {
        panic!("expected an MMM settlement");
    };
    assert!(mmm.wiederverkaeufer.leistender && mmm.wiederverkaeufer.empfaenger);
    // Electricity needs both; this pair shifts the liability.
    assert!(
        mmm.wiederverkaeufer
            .verlagert(grid_billing::Leistungsart::LieferungStrom)
    );
}

/// An MSB-Rechnung names its recipient's market role in full —
/// `Netzbetreiber`, `Lieferant`, `Energieserviceanbieter`, the three the PID
/// overview 4.0 lists for 31009.
#[test]
fn the_msb_recipient_roles_parse() {
    for rolle in ["Netzbetreiber", "Lieferant", "Energieserviceanbieter"] {
        let run = parse(serde_json::json!({
            "invoice_date": "2026-02-01",
            "due_date": "2026-03-03",
            "positions": [{
                "malo_id": "51238696012",
                "period_from": "2026-01-01",
                "period_to": "2026-12-31",
                "settlement": {
                    "billing_type": "msb",
                    "msb_mp_id": "4012345000023",
                    "empfaenger_rolle": rolle,
                    "empfaenger_mp_id": "9900357000004",
                    "sparte": "Strom",
                    "grundgebuehr_eur_per_month": "9.50",
                    "billing_months": 12,
                    "messdienstleistung_eur": "24.00"
                }
            }]
        }));
        let SettlementRequest::Msb(msb) = &run.positions[0].settlement else {
            panic!("expected an MSB settlement");
        };
        assert_eq!(msb.billing_months, 12);
    }
}

/// A Gas Mehr-/Mindermengen position, with the prices left to auto-fetch.
#[test]
fn a_gas_mmm_position_parses_without_prices() {
    let run = parse(serde_json::json!({
        "invoice_date": "2026-02-01",
        "due_date": "2026-03-03",
        "positions": [{
            "malo_id": "51238696012",
            "period_from": "2026-01-01",
            "period_to": "2026-01-31",
            "settlement": {
                "billing_type": "mmm",
                "nb_mp_id": "9900357000004",
                "lf_mp_id": "9900012345678",
                "sparte": "Gas",
                "gemessen_kwh": "1200.000",
                "bilanziert_kwh": "1000.000"
            }
        }]
    }));
    let SettlementRequest::Mmm(mmm) = &run.positions[0].settlement else {
        panic!("expected an MMM settlement");
    };
    assert!(
        mmm.mehr_preis_ct_per_kwh.is_none(),
        "prices are resolved from marktd before the engine runs"
    );
}

/// The Abschlagsrechnung and the deduction shape both documents print.
#[test]
fn the_documented_abschlag_shapes_parse() {
    // The Abschlag itself: an amount and how it was arrived at.
    let abschlag = parse(serde_json::json!({
        "invoice_date": "2026-01-05",
        "due_date": "2026-02-04",
        "rechnungskreis": "ABS",
        "positions": [{
            "malo_id": "51238696012",
            "period_from": "2026-01-01",
            "period_to": "2026-12-31",
            "settlement": {
                "billing_type": "abschlag",
                "nb_mp_id": "9900357000004",
                "lf_mp_id": "9900012345678",
                "sparte": "Strom",
                "betrag_netto_eur": "1000.00",
                "grundlage": "Vorjahresverbrauch"
            }
        }]
    }));
    let SettlementRequest::Abschlag(a) = &abschlag.positions[0].settlement else {
        panic!("expected an Abschlag settlement");
    };
    assert_eq!(
        a.grundlage,
        grid_billing::AbschlagGrundlage::Vorjahresverbrauch
    );

    // Every documented `grundlage` value parses.
    for grundlage in ["Vorjahresverbrauch", "Prognose", "Vereinbarung"] {
        parse(serde_json::json!({
            "invoice_date": "2026-01-05",
            "due_date": "2026-02-04",
            "positions": [{
                "malo_id": "51238696012",
                "period_from": "2026-01-01",
                "period_to": "2026-12-31",
                "settlement": {
                    "billing_type": "abschlag",
                    "nb_mp_id": "9900357000004",
                    "lf_mp_id": "9900012345678",
                    "sparte": "Strom",
                    "betrag_netto_eur": "1000.00",
                    "grundlage": grundlage
                }
            }]
        }));
    }

    // The invoice that settles them, with the cadence and the deductions.
    let settling = parse(serde_json::json!({
        "invoice_date": "2027-01-15",
        "due_date": "2027-02-14",
        "positions": [{
            "malo_id": "51238696012",
            "period_from": "2026-01-01",
            "period_to": "2026-12-31",
            "cadence": "Abschlussrechnung",
            "abschlaege": [
                "550e8400-e29b-41d4-a716-446655440000",
                "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
            ],
            "settlement": {
                "billing_type": "nne",
                "nb_mp_id": "9900357000004",
                "lf_mp_id": "9900012345678",
                "sparte": "Strom",
                "arbeitspreis": { "Einheitlich": { "menge_kwh": "18000", "preis_ct_per_kwh": "3.5" } }
            }
        }]
    }));
    assert_eq!(settling.positions[0].abschlaege.len(), 2);
    assert_eq!(
        settling.positions[0].cadence,
        Some(grid_billing::Rechnungscharakter::Abschlussrechnung)
    );
}

/// Every documented cadence value parses.
#[test]
fn the_documented_cadences_parse() {
    for cadence in [
        "Abschlagsrechnung",
        "Abschlussrechnung",
        "Turnusrechnung",
        "Monatsrechnung",
        "Zwischenrechnung",
    ] {
        let run = parse(serde_json::json!({
            "invoice_date": "2026-02-01",
            "due_date": "2026-03-03",
            "positions": [{
                "malo_id": "51238696012",
                "period_from": "2026-01-01",
                "period_to": "2026-01-31",
                "cadence": cadence,
                "settlement": {
                    "billing_type": "nne",
                    "nb_mp_id": "9900357000004",
                    "lf_mp_id": "9900012345678",
                    "sparte": "Strom",
                    "arbeitspreis": { "Einheitlich": { "menge_kwh": "1500", "preis_ct_per_kwh": "3.5" } }
                }
            }]
        }));
        assert!(run.positions[0].cadence.is_some(), "{cadence} must parse");
    }
}
