//! Guard: the request bodies `demos/o2c` posts to `productd` are ones
//! `productd` accepts.
//!
//! See `services/vertragd/tests/demo_payloads.rs` for why: a demo payload is
//! shipped API surface, and one whose fields the endpoint does not have would
//! return `2xx` with the values going nowhere.

use productd::pg::ProductUpsertRequest;

/// The demo's Tarifpreisblatt, priced in **cents**.
///
/// `billingd` reads `grundpreis_ct_per_day` and `arbeitspreis_ct_per_kwh` by
/// traversing `data.tarifpreise` keyed on `preistyp`, and a `preis` is read
/// verbatim as cents — writing `"0.20"` for 20 ct/Tag prices the tariff at a
/// fifth of a cent a day, which bills without complaint.
#[test]
fn the_o2c_product_payload_deserialises() {
    let req: ProductUpsertRequest = serde_json::from_value(serde_json::json!({
        "category": "STROM",
        "name": "Strom Zuhause Demo",
        "sparte": "STROM",
        "register_count": "Eintarif",
        "kundentyp": "Haushalt",
        "valid_from": "2026-01-01",
        "product_status": "PUBLISHED",
        "data": {
            "_typ": "TARIFPREISBLATT",
            "bezeichnung": "Strom Zuhause Demo 2026",
            "zeitlicheGueltigkeit": { "startdatum": "2026-01-01" },
            "tarifpreise": [
                { "preistyp": "GRUNDPREIS",            "preisstaffeln": [{ "preis": "20" }] },
                { "preistyp": "ARBEITSPREIS_EINTARIF", "preisstaffeln": [{ "preis": "32" }] }
            ]
        }
    }))
    .expect("productd would refuse this demo payload — fix the demo, or the endpoint");
    assert_eq!(req.category, "STROM");
    assert_eq!(
        req.data["tarifpreise"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default(),
        2,
        "the demo prices a Grundpreis and an Arbeitspreis"
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
