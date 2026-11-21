//! The bundled sample messages: `edi-energy`'s committed fixtures, which carry
//! synthetic MP-IDs, Lokations-IDs, names and dates only.

use serde_json::{Value, json};

macro_rules! fixture {
    ($path:literal) => {
        include_bytes!(concat!("../../../crates/edi-energy/tests/fixtures/", $path))
    };
}

/// `(name, title, text)`; the name is what a share link carries.
const SAMPLES: &[(&str, &str, &[u8])] = &[
    (
        "utilmd-55001",
        "UTILMD 55001 Anmeldung — fehlerfrei",
        fixture!("utilmd/valid/beispiel_55001_lieferbeginn.edi"),
    ),
    (
        "utilmd-lokations-id",
        "UTILMD mit falscher Lokations-ID",
        fixture!("utilmd/invalid/bad_lokations_id.edi"),
    ),
    (
        "utilmd-55036-sg12",
        "UTILMD 55036 ohne Altlieferant",
        fixture!("utilmd/invalid/55036_ohne_sg12_altlieferant.edi"),
    ),
    (
        "utilmd-44001-gas",
        "UTILMD Gas 44001 Lieferbeginn — fehlerfrei",
        fixture!("utilmd/valid/beispiel_44001_lieferbeginn_gas.edi"),
    ),
    (
        "mscons-13002",
        "MSCONS 13002 Zählerstand Gas — fehlerfrei",
        fixture!("mscons/valid/beispiel_13002_gas_release_2_5.edi"),
    ),
    (
        "mscons-melo",
        "MSCONS 13003 mit falscher Messlokation",
        fixture!("mscons/invalid/bad_melo.edi"),
    ),
    (
        "aperak-29001",
        "APERAK 29001 Verarbeitbarkeitsfehler — fehlerfrei",
        fixture!("aperak/valid/beispiel_29001_verarbeitbarkeitsfehler.edi"),
    ),
    (
        "aperak-ohne-referenz",
        "APERAK ohne Bezug auf die Ursprungsnachricht",
        fixture!("aperak/invalid/missing_acw_ref.edi"),
    ),
    (
        "invoic-31001",
        "INVOIC 31001 Abschlagsrechnung — fehlerfrei",
        fixture!("invoic/valid/beispiel_31001_abschlagsrechnung.edi"),
    ),
    (
        "orders-17132",
        "ORDERS 17132 Bestellung — fehlerfrei",
        fixture!("orders/valid/beispiel_17132_bestellung.edi"),
    ),
];

/// The samples, without their text.
pub(crate) fn list() -> Value {
    Value::Array(
        SAMPLES
            .iter()
            .map(|(name, title, _)| json!({ "name": name, "title": title }))
            .collect(),
    )
}

/// One sample's text. The fixtures are ISO 8859-1, as UNOC prescribes, so
/// each byte is one character.
pub(crate) fn text(name: &str) -> Option<String> {
    SAMPLES
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, _, bytes)| bytes.iter().map(|&b| char::from(b)).collect())
}
