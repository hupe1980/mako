//! Guard: the request bodies `demos/eeg-billing` posts to `einsd` are ones
//! `einsd` accepts.
//!
//! See `services/vertragd/tests/demo_payloads.rs` for why. A demo payload is
//! shipped API surface, and one whose fields the endpoint does not have would
//! return `2xx` with the values going nowhere.

use einsd::pg::AnlageUpsertRequest;
use einsd::pg_einspeiser::UpsertEinspeiser;

fn fixture(rel: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    serde_json::from_str(&src)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

fn accepts<T: serde::de::DeserializeOwned>(what: &str, body: serde_json::Value) -> T {
    serde_json::from_value(body).unwrap_or_else(|e| {
        panic!(
            "{what}: einsd would refuse this demo payload — {e}\n\
             A demo body is shipped API surface: fix the demo, or the endpoint."
        )
    })
}

/// The EEG plant and its Einspeiser.
#[test]
fn the_eeg_billing_payloads_deserialise() {
    let anlage: AnlageUpsertRequest = accepts(
        "PUT /api/v1/anlagen/{tr_id}",
        fixture("demos/eeg-billing/fixtures/anlage.json"),
    );
    // § 51 follows the **Inbetriebnahmedatum**, not the law year, so a demo
    // plant with no commissioning date settles under the wrong regime.
    assert!(
        !anlage.inbetriebnahme.to_string().is_empty(),
        "the plant states when it went into service"
    );
    // § 9 Abs. 2 is expressed as **two booleans**, not one enum. A 9.8 kWp
    // geförderte Anlage sits below 25 kW, so Satz 1 Nr. 3 asks it for the 60 %
    // Wirkleistungsbegrenzung and not for the Fernsteuerbarkeit — and a plant
    // that does not carry what it owes settles `Reduced`, at which point the
    // demo's expected amounts are wrong. The fixture said
    // `"sect9_erfuellung": "LEISTUNGSBEGRENZUNG_60"`, which is no field of this
    // request at all.
    assert!(
        anlage.sect9_begrenzung_60,
        "the demo plant carries the 60 % cap § 9 Abs. 2 Satz 1 Nr. 3 asks of it"
    );

    let _: UpsertEinspeiser = accepts(
        "PUT /api/v1/einspeiser/{id}",
        fixture("demos/eeg-billing/fixtures/einspeiser.json"),
    );
}

/// A field the endpoint does not have is a refusal, not a discarded value.
#[test]
fn a_field_the_endpoint_does_not_have_is_refused() {
    let mut body = fixture("demos/eeg-billing/fixtures/anlage.json");
    body.as_object_mut()
        .expect("an object")
        .insert("verguetung_ct".into(), serde_json::json!("8.2"));
    let err = serde_json::from_value::<AnlageUpsertRequest>(body)
        .expect_err("the field is `verguetungssatz_ct`");
    assert!(err.to_string().contains("unknown field"), "{err}");

    // And a `Decimal` is a JSON **string**. A float cannot represent a decimal
    // exactly, so accepting one would settle a Vergütung against the nearest
    // binary double; `rust_decimal` under `serde-str` refuses it. An unquoted
    // `"leistung_kwp": 9.8` in the fixture makes this endpoint a 422.
    let mut body = fixture("demos/eeg-billing/fixtures/anlage.json");
    body.as_object_mut()
        .expect("an object")
        .insert("leistung_kwp".into(), serde_json::json!(9.8));
    let err = serde_json::from_value::<AnlageUpsertRequest>(body)
        .expect_err("a JSON float is not a Decimal");
    assert!(err.to_string().contains("Decimal"), "{err}");
}
