//! Guard: every request body the seeder posts to `vertragd`
//! (`tools/mako-seed/world/`) deserialises into `vertragd`'s request type —
//! and carries what the invoice needs.
//!
//! A seeded body is shipped API surface: it is run by whoever evaluates the
//! platform and copied into tickets as "this is what a request looks like".
//! The customer is a BO4E `geschaeftspartner`, and the SEPA mandate a BO4E
//! `Zahlungsinformation` on `PUT /kunden/{id}/zahlungsinformation`. Every body
//! denies unknown fields, so a flat `vorname`/`strasse`/`iban` payload is a
//! `422` naming the field rather than a `201` for a customer with no name — and
//! an invoice addressed to nobody (§ 14 Abs. 4 Nr. 1 UStG).

use vertragd::pg::{CreateKundeInput, CreateVersorgungsvertragInput};

/// A body from the seeder's world, `tools/mako-seed/world/<file>`.
fn world(file: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/mako-seed/world")
        .join(file);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    serde_json::from_str(&src)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

/// Deserialise `body` as `T`, or fail naming the payload.
fn accepts<T: serde::de::DeserializeOwned>(what: &str, body: serde_json::Value) -> T {
    serde_json::from_value(body).unwrap_or_else(|e| {
        panic!(
            "{what}: vertragd would refuse this seeded body — {e}\n\
             A seeded body is shipped API surface: fix tools/mako-seed/world, or the endpoint."
        )
    })
}

/// The seeded customer.
#[test]
fn the_seeded_kunde_payload_deserialises_and_names_somebody() {
    let input: CreateKundeInput = accepts("POST /api/v1/kunden", world("vertragd-kunde.json"));

    let gp = input
        .geschaeftspartner
        .as_ref()
        .expect("the seed posts a Geschaeftspartner");
    // § 14 Abs. 4 Nr. 1 UStG names the Leistungsempfänger, EN 16931 makes BT-44
    // mandatory. A customer with no name issues an invoice to nobody.
    assert_eq!(gp.nachname.as_deref(), Some("Mustermann"));
    let adresse = gp.adresse.as_ref().expect("with a full address");
    assert_eq!(adresse.postleitzahl.as_deref(), Some("12345"));
    assert_eq!(adresse.ort.as_deref(), Some("Musterstadt"));

    // And what gets stored is the gate's canonical round-trip: the `_typ` the
    // request omitted is present, because a BO4E consumer needs it.
    let stored = gp.canonical_json().expect("serialisable");
    assert_eq!(stored["_typ"], "GESCHAEFTSPARTNER");
}

/// A flat customer payload is a refusal, not a silent `201`.
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

/// The seeded supply contract.
#[test]
fn the_seeded_vertrag_payload_deserialises() {
    let input: CreateVersorgungsvertragInput = accepts(
        "POST /api/v1/kunden/{id}/vertraege",
        world("vertragd-vertrag.json"),
    );
    assert_eq!(input.komponenten.len(), 1);
    // § 309 Nr. 9 lit. b BGB: the only lawful tacit extension of a consumer
    // contract is into an unbefristeten Vertrag, which `0` is.
    assert_eq!(input.renewal_monate, Some(0));
}

/// The seeded Gas contract states every fact the GeLi Gas Anmeldung 44001
/// carries, so registering it at the NB is not refused for a missing one.
#[test]
fn the_seeded_gas_vertrag_payload_deserialises_with_its_anmeldung_facts() {
    let input: CreateVersorgungsvertragInput = accepts(
        "POST /api/v1/kunden/{id}/vertraege",
        world("vertragd-vertrag-gas.json"),
    );
    assert_eq!(input.komponenten.len(), 1);
    let k = &input.komponenten[0];
    assert_eq!(k.sparte, "GAS");
    assert!(
        k.wird_angemeldet(),
        "the Gas component is registered at its NB"
    );
    assert!(k.lieferbeginn_anlass.is_some());
    assert!(k.netznutzungsvertrag.is_some());
    assert!(k.netznutzung_zahler.is_some());
    assert!(k.gemeinderabatt_prozent.is_some());
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

/// The seeded SEPA mandate is a BO4E `Zahlungsinformation`.
#[test]
fn the_seeded_zahlungsinformation_deserialises() {
    let _: mako_markt::bo4e::Bo4e<rubo4e::current::Zahlungsinformation> = accepts(
        "PUT /api/v1/kunden/{id}/zahlungsinformation",
        world("vertragd-zahlungsinformation.json"),
    );
}

/// Clara Sommer's online order. The seeder fills in the frozen offer's id and
/// hash and the documents' hashes; every other field is the body as sent.
#[test]
fn the_seeded_online_order_deserialises() {
    let order: vertragd::bestellung::BestellungInput = accepts(
        "POST /api/v1/bestellungen",
        world("vertragd-bestellung.json"),
    );
    assert!(
        order.verbraucher,
        "the order is a consumer's, so it carries the Widerrufsrecht"
    );
    assert_eq!(order.button_beschriftung, "zahlungspflichtig bestellen");
}
