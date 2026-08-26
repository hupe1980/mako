//! A stored `Tarifpreisblatt` must be a *valid* BO4E document.
//!
//! mako's price-type vocabulary is a superset of BO4E's ten `Preistyp` values —
//! it prices an EEG-Marktprämie, a HEMS optimisation event, an E-Mobility
//! roaming fee, none of which the standard models. Written straight into
//! `tarifpreispositionen[*].preistyp`, they make a document stamped
//! `_typ: "TARIFPREISBLATT"` carry price types that are not BO4E values.
//!
//! What that costs depends on the reader, and the lenient case is the *mild*
//! one. `rubo4e` decodes an unlisted value to its `Unknown` catch-all and says
//! nothing. go-bo4e's generated `UnmarshalJSON` returns `invalid Preistyp %q`
//! and has no catch-all at all; BO4E-python's enums are pydantic `StrEnum`s,
//! which raise a `ValidationError`. Both reject the **whole document** — so a
//! Go or Python counterparty does not misread the price sheet, it fails to
//! read it.
//!
//! They travel instead in the `mako:preistyp` `ZusatzAttribut`, which is BO4E's
//! own answer to carrying something the schema does not define. These tests pin
//! that: whatever `normalize_tarifpreisblatt` returns must round-trip through
//! `rubo4e` with **no** enum anywhere in the tree falling through to `Unknown`.

use mako_markt::bo4e::{MAKO_PREISTYP_ATTRIBUT, position_preistyp};
use productd::handlers::normalize_tarifpreisblatt;
use rubo4e::current::Tarifpreisblatt;

fn product(preistyp: &str) -> serde_json::Value {
    serde_json::json!({
        "_typ": "TARIFPREISBLATT",
        "bezeichnung": "Testtarif",
        "sparte": "STROM",
        "tarifpreispositionen": [{
            "preistyp": preistyp,
            "preisstaffeln": [{ "preis": "8.20" }]
        }]
    })
}

/// The whole point: no `Unknown` survives anywhere in a stored document.
#[test]
fn a_stored_tarifpreisblatt_has_no_unknown_enums() {
    for preistyp in [
        // BO4E's own
        "GRUNDPREIS",
        "ARBEITSPREIS_HT",
        "LEISTUNGSPREIS",
        // mako extensions, which BO4E does not define
        "EEG_MARKTPRAEMIE",
        "HEMS_OPTIMIERUNGSEVENT",
        "EMOBILITY_ROAMING",
        "SERVICE_GEBUEHR",
    ] {
        let stored = normalize_tarifpreisblatt("STROM", product(preistyp))
            .unwrap_or_else(|e| panic!("{preistyp} rejected: {e:?}"));

        let typed: Tarifpreisblatt = serde_json::from_value(stored.clone())
            .unwrap_or_else(|e| panic!("{preistyp} is not a Tarifpreisblatt: {e}"));

        mako_markt::bo4e::ensure_conformant(&typed)
            .unwrap_or_else(|e| panic!("{preistyp} produced a document mako would refuse: {e}"));
    }
}

/// A BO4E-defined type stays in the BO4E field; a mako one moves aside.
#[test]
fn the_vocabulary_split_is_where_it_should_be() {
    let bo4e = normalize_tarifpreisblatt("STROM", product("GRUNDPREIS")).expect("accepted");
    let pos = &bo4e["tarifpreispositionen"][0];
    assert_eq!(pos["preistyp"], "GRUNDPREIS");
    assert_eq!(position_preistyp(pos), "GRUNDPREIS");

    let mako = normalize_tarifpreisblatt("STROM", product("EEG_MARKTPRAEMIE")).expect("accepted");
    let pos = &mako["tarifpreispositionen"][0];
    assert!(
        pos.get("preistyp").is_none(),
        "a mako price type must not occupy BO4E's enum field: {pos}"
    );
    assert_eq!(
        pos["zusatzAttribute"][0]["name"], MAKO_PREISTYP_ATTRIBUT,
        "it belongs in the sanctioned extension slot"
    );
    // Readers see the same answer either way.
    assert_eq!(position_preistyp(pos), "EEG_MARKTPRAEMIE");
}

/// Re-normalising a stored document must not append a second attribute.
#[test]
fn normalisation_is_idempotent() {
    let once = normalize_tarifpreisblatt("STROM", product("KWKG_ZUSCHLAG")).expect("accepted");
    let twice = normalize_tarifpreisblatt("STROM", once.clone()).expect("accepted");
    assert_eq!(once, twice, "normalisation must be a fixpoint");

    let attrs = twice["tarifpreispositionen"][0]["zusatzAttribute"]
        .as_array()
        .expect("attribute list");
    assert_eq!(attrs.len(), 1, "the mako attribute must not accumulate");
}

/// A value in neither vocabulary is still refused outright.
#[test]
fn an_unknown_price_type_is_still_rejected() {
    let bad = normalize_tarifpreisblatt("STROM", product("NOT_A_PRICE_TYPE"));
    assert!(bad.is_err(), "an unlisted preistyp must be a 422");
}
