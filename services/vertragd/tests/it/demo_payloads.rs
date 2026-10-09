//! Guard: every request body `demos/o2c` posts to `vertragd` is one `vertragd`
//! accepts — and carries what the invoice needs.
//!
//! ## Why this test exists
//!
//! `demos/o2c` posted its customer as
//!
//! ```json
//! { "kundentyp": "B2C", "haushaltskunde": true,
//!   "anrede": "Frau", "vorname": "Erika", "nachname": "Mustermann",
//!   "email": "erika.mustermann@example.org",
//!   "strasse": "Musterstr. 1", "plz": "10115", "ort": "Berlin", "land": "DE",
//!   "iban": "DE02120300000000202051", "zahlungsart": "SEPA_LASTSCHRIFT" }
//! ```
//!
//! Nine of those twelve keys are not fields of `CreateKundeInput`. It takes a
//! BO4E `geschaeftspartner`, and the SEPA mandate belongs on
//! `PUT /kunden/{id}/zahlungsinformation` as a BO4E `Zahlungsinformation`.
//! `serde` ignores a key no field declares, so the request returned `201`, the
//! customer was created **with no name and no address**, the invoice that
//! followed named nobody, and the demo asserted nothing about it and passed.
//!
//! Three changes make that impossible; this pins the demo against all three.

use vertragd::pg::{CreateKundeInput, CreateVersorgungsvertragInput};

/// Deserialise `body` as `T`, or fail naming the payload.
fn accepts<T: serde::de::DeserializeOwned>(what: &str, body: serde_json::Value) -> T {
    serde_json::from_value(body).unwrap_or_else(|e| {
        panic!(
            "{what}: vertragd would refuse this demo payload — {e}\n\
             A demo body is shipped API surface: fix the demo, or the endpoint."
        )
    })
}

/// The demo's customer, as it is posted now.
#[test]
fn the_o2c_kunde_payload_deserialises_and_names_somebody() {
    let input: CreateKundeInput = accepts(
        "POST /api/v1/kunden",
        serde_json::json!({
            "kundentyp": "B2C",
            "haushaltskunde": true,
            "email": "erika.mustermann@example.org",
            "erp_kunde_id": "DEMO-KUNDE-51238696781",
            "geschaeftspartner": {
                "anrede": "FRAU",
                "vorname": "Erika",
                "nachname": "Mustermann",
                "adresse": {
                    "strasse": "Musterstr.",
                    "hausnummer": "1",
                    "postleitzahl": "10115",
                    "ort": "Berlin",
                    "landescode": "DE"
                },
                "kontaktwege": [{
                    "kontaktart": "E_MAIL",
                    "kontaktwert": "erika.mustermann@example.org",
                    "istBevorzugterKontaktweg": true
                }]
            }
        }),
    );

    let gp = input
        .geschaeftspartner
        .as_ref()
        .expect("the demo posts a Geschaeftspartner");
    // § 14 Abs. 4 Nr. 1 UStG names the Leistungsempfänger, EN 16931 makes BT-44
    // mandatory. A demo whose customer has no name issues an invoice to nobody.
    assert_eq!(gp.nachname.as_deref(), Some("Mustermann"));
    let adresse = gp.adresse.as_ref().expect("with a full address");
    assert_eq!(adresse.postleitzahl.as_deref(), Some("10115"));
    assert_eq!(adresse.ort.as_deref(), Some("Berlin"));

    // And what gets stored is the gate's canonical round-trip: the `_typ` the
    // request omitted is present, because a BO4E consumer needs it.
    let stored = gp.canonical_json().expect("serialisable");
    assert_eq!(stored["_typ"], "GESCHAEFTSPARTNER");
}

/// The shape that shipped is a refusal now, not a silent `201`.
#[test]
fn the_flat_customer_payload_is_refused_by_name() {
    let err = serde_json::from_value::<CreateKundeInput>(serde_json::json!({
        "kundentyp": "B2C",
        "haushaltskunde": true,
        "vorname": "Erika",
        "nachname": "Mustermann",
        "strasse": "Musterstr. 1",
        "plz": "10115",
        "ort": "Berlin",
        "iban": "DE02120300000000202051"
    }))
    .expect_err("none of those are fields of this endpoint");
    assert!(
        err.to_string().contains("unknown field"),
        "the refusal must name the field rather than dropping it: {err}"
    );
}

/// The BO4E gate runs while the body deserialises, so a handler cannot forget
/// it — and the refusal keeps the stage that refused.
#[test]
fn an_out_of_schema_enum_in_the_customer_is_refused_at_the_gate() {
    let err = serde_json::from_value::<CreateKundeInput>(serde_json::json!({
        "kundentyp": "B2C",
        "geschaeftspartner": { "anrede": "FRAUU" }
    }))
    .expect_err("FRAUU is not a BO4E Anrede");
    let (sentence, detail) = mako_markt::bo4e::recover_rejection(&err.to_string())
        .expect("the rejection survives serde so the 422 keeps its keys");
    assert_eq!(detail["code"], "bo4e.unknown_enum");
    assert_eq!(detail["paths"][0], "anrede");
    assert!(sentence.contains("GESCHAEFTSPARTNER"), "{sentence}");

    // A `_typ` naming another BO is refused too — nothing downstream catches
    // one, because the strict-enum walk visits a value's fields and never
    // reaches `typ`.
    let err = serde_json::from_value::<CreateKundeInput>(serde_json::json!({
        "kundentyp": "B2C",
        "geschaeftspartner": { "_typ": "MARKTLOKATION" }
    }))
    .expect_err("a Marktlokation is not a Geschaeftspartner");
    let (_, detail) = mako_markt::bo4e::recover_rejection(&err.to_string()).expect("recoverable");
    assert_eq!(detail["code"], "bo4e.discriminator");
}

/// The demo's supply contract.
#[test]
fn the_o2c_vertrag_payload_deserialises() {
    let input: CreateVersorgungsvertragInput = accepts(
        "POST /api/v1/kunden/{id}/vertraege",
        serde_json::json!({
            "kundentyp": "B2C",
            "vertragsart": "SONDERVERTRAG",
            "vertragsbeginn": "2026-01-01",
            "kuendigungsfrist_monate": 1,
            "abrechnungszyklus": "MONATLICH",
            "auto_renewal": true,
            "renewal_monate": 0,
            "zahlungsziel_tage": 14,
            "erp_contract_id": "DEMO-51238696781",
            "standort_bezeichnung": "Musterstr. 1, 10115 Berlin",
            "komponenten": [{
                "sparte": "STROM",
                "malo_id": "51238696781",
                "nb_mp_id": "9900357000004",
                "product_code": "STROM-H0-DEMO",
                "lieferbeginn": "2026-01-01",
                "jahresverbrauch_kwh": "3000"
            }]
        }),
    );
    assert_eq!(input.komponenten.len(), 1);
    // § 309 Nr. 9 lit. b BGB: the only lawful tacit extension of a consumer
    // contract is into an unbefristeten Vertrag, which `0` is.
    assert_eq!(input.renewal_monate, Some(0));
}

/// A `standort_adresse` on the contract is a BO4E `Adresse` and crosses the
/// same gate.
#[test]
fn the_supply_address_is_gated() {
    let err = serde_json::from_value::<CreateVersorgungsvertragInput>(serde_json::json!({
        "kundentyp": "B2C",
        "vertragsbeginn": "2026-01-01",
        "komponenten": [],
        "standort_adresse": { "_typ": "GESCHAEFTSPARTNER" }
    }))
    .expect_err("a Geschaeftspartner is not an Adresse");
    let (_, detail) = mako_markt::bo4e::recover_rejection(&err.to_string()).expect("recoverable");
    assert_eq!(detail["code"], "bo4e.discriminator");
    assert_eq!(detail["expected_typ"], "ADRESSE");
}
