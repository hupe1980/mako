//! Guard: the request bodies the seeder posts to `productd`
//! (`tools/mako-seed/world/`) deserialise into `productd`'s request type.
//!
//! See `services/vertragd/tests/it/seed_payloads.rs` for why: a seeded body is
//! shipped API surface, and one whose fields the endpoint does not have would
//! return `2xx` with the values going nowhere.

use productd::pg::ProductUpsertRequest;

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

/// The seeded Tarifpreisblatt, priced in **cents**.
///
/// `billingd` reads `grundpreis_ct_per_day` and `arbeitspreis_ct_per_kwh` by
/// traversing `data.tarifpreise` keyed on `preistyp`, and a `preis` is read
/// verbatim as cents — writing `"0.20"` for 20 ct/Tag prices the tariff at a
/// fifth of a cent a day, which bills without complaint.
#[test]
fn the_seeded_product_payload_deserialises() {
    let req: ProductUpsertRequest = serde_json::from_value(world("productd-product-strom.json"))
        .expect(
            "productd would refuse this seeded body — fix tools/mako-seed/world, or the endpoint",
        );
    assert_eq!(req.category, "STROM");
    assert_eq!(
        req.data["tarifpreise"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default(),
        2,
        "the seed prices a Grundpreis and an Arbeitspreis"
    );
}

/// The seeded Gas product prices the same two positions, in cents, under
/// category `GAS`.
#[test]
fn the_seeded_gas_product_payload_deserialises() {
    let req: ProductUpsertRequest = serde_json::from_value(world("productd-product-gas.json"))
        .expect(
            "productd would refuse this seeded body — fix tools/mako-seed/world, or the endpoint",
        );
    assert_eq!(req.category, "GAS");
    assert_eq!(
        req.data["tarifpreise"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default(),
        2,
        "the seed prices a Grundpreis and an Arbeitspreis"
    );
}

/// A field the endpoint does not have is a refusal, not a discarded value.
///
/// `gueltigkeit` is the near-miss that matters here: BO4E spells it
/// `zeitlicheGueltigkeit`, and a `Tarifpreisblatt` carrying the short form
/// stores its validity in `_additional` where nothing reads it.
#[test]
fn a_field_the_endpoint_does_not_have_is_refused() {
    let err = serde_json::from_value::<ProductUpsertRequest>(serde_json::json!({
        "category": "STROM",
        "name": "Strom Zuhause Demo",
        "valid_from": "2026-01-01",
        "data": {},
        "grundpreis_ct_per_day": "20"
    }))
    .expect_err("prices live inside the Tarifpreisblatt, not beside it");
    assert!(err.to_string().contains("unknown field"), "{err}");
}

/// The `energiemix` on the product `PUT` crosses the BO4E gate, exactly as the
/// dedicated sub-resource does, so the same document is checked the same way
/// whichever route writes it.
#[test]
fn the_product_energiemix_is_gated() {
    let err = serde_json::from_value::<ProductUpsertRequest>(serde_json::json!({
        "category": "STROM",
        "name": "Strom Zuhause Demo",
        "valid_from": "2026-01-01",
        "data": {},
        "energiemix": { "anteil": [{ "erzeugungsart": "SONNENSCHEIN", "anteilProzent": "100" }] }
    }))
    .expect_err("SONNENSCHEIN is not a BO4E Erzeugungsart");
    let (_, detail) = mako_markt::bo4e::recover_rejection(&err.to_string())
        .expect("the gate's rejection survives serde");
    assert_eq!(detail["code"], "bo4e.unknown_enum");
}
