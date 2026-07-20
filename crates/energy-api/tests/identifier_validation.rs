//! Identifiers on the API boundary must be validated, not merely typed.
//!
//! MaLo-Ident is the first binding API process in German MaKo (mandatory since
//! 06.06.2025) and a hard precondition for every supplier switch. An unvalidated
//! identifier accepted here propagates into the identification path, so the
//! check digit has to be enforced at deserialization.

use energy_api::models::electricity::{IdentificationParameterId, LocationId, MaloId, MeloId};

/// A MaLo-ID whose BDEW check digit does not match must be refused when the
/// request body is deserialized — not accepted and carried onward.
#[test]
fn a_bad_malo_check_digit_is_rejected_at_deserialization() {
    // 51238696780 is valid; flipping the last digit breaks the check digit.
    let ok: Result<MaloId, _> = serde_json::from_str(r#""51238696780""#);
    assert!(ok.is_ok(), "valid MaLo-ID must deserialize: {ok:?}");

    let bad: Result<MaloId, _> = serde_json::from_str(r#""51238696781""#);
    assert!(
        bad.is_err(),
        "a wrong check digit must not deserialize into a MaloId"
    );
}

#[test]
fn malformed_malo_ids_are_rejected() {
    for raw in [
        r#""""#,
        r#""5123869678""#,
        r#""512386967800""#,
        r#""5123869678A""#,
    ] {
        let parsed: Result<MaloId, _> = serde_json::from_str(raw);
        assert!(parsed.is_err(), "{raw} must not deserialize into a MaloId");
    }
}

/// A MeLo-ID is 33 characters: ISO 3166-1 alpha-2 plus a 31-character body.
#[test]
fn melo_ids_enforce_their_shape() {
    let ok: Result<MeloId, _> = serde_json::from_str(r#""DE0123456789012345678901234567890""#);
    assert!(ok.is_ok(), "33-char MeLo-ID must deserialize: {ok:?}");

    let short: Result<MeloId, _> = serde_json::from_str(r#""DE012345678901234567890""#);
    assert!(short.is_err(), "a 23-character MeLo-ID must be rejected");
}

/// The whole request wrapper must fail, not just the leaf — otherwise a bad ID
/// still reaches the handler inside an otherwise-valid body.
#[test]
fn an_enclosing_request_body_fails_when_the_identifier_is_bad() {
    let good = r#"{"maloId":"51238696780"}"#;
    let parsed: Result<IdentificationParameterId, _> = serde_json::from_str(good);
    assert!(parsed.is_ok(), "valid body must parse: {parsed:?}");

    let bad = r#"{"maloId":"51238696781"}"#;
    let parsed: Result<IdentificationParameterId, _> = serde_json::from_str(bad);
    assert!(
        parsed.is_err(),
        "a bad check digit must fail the enclosing body, not just the field"
    );
}

/// Pin the wire property names of `identificationParameterId` against
/// `maloIdentV1.yaml` (tag `1.0.0`, the binding version).
///
/// Serde's `rename_all = "camelCase"` derives these, so a field rename in Rust
/// silently changes the wire contract. Unknown properties are ignored on
/// deserialization, which means a mismatch does not error — it just drops the
/// value. For MaLo-Ident that would mean an identification request whose
/// MaLo-ID is silently absent.
///
/// Note `tranchenIds`: the spec is mixed German/English, so a "tidier"
/// `tranche_ids` in Rust would produce `trancheIds` and stop matching.
#[test]
fn identification_parameter_id_matches_the_v1_wire_contract() {
    let json = serde_json::to_value(IdentificationParameterId {
        malo_id: Some(MaloId::new("51238696780").unwrap()),
        tranchen_ids: Some(vec!["12345678901".to_owned()]),
        melo_ids: Some(vec![
            MeloId::new("DE0123456789012345678901234567890").unwrap(),
        ]),
        meter_numbers: Some(vec!["METER-1".to_owned()]),
        customer_number: Some("CUST-1".to_owned()),
    })
    .expect("serialises");

    let obj = json.as_object().expect("an object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "customerNumber",
            "maloId",
            "meloIds",
            "meterNumbers",
            "tranchenIds"
        ],
        "wire property names must match maloIdentV1.yaml"
    );
}

/// `LocationId` is untagged, so an invalid identifier must not silently fall
/// through to the other variant.
#[test]
fn an_invalid_location_id_matches_neither_variant() {
    let nelo: Result<LocationId, _> = serde_json::from_str(r#""E1234848431""#);
    assert!(nelo.is_ok(), "valid NeLo-ID must parse: {nelo:?}");

    let bogus: Result<LocationId, _> = serde_json::from_str(r#""not-an-id""#);
    assert!(
        bogus.is_err(),
        "an untagged enum must not accept a malformed identifier"
    );
}
