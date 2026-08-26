//! The BO4E `Angebot` productd emits must be a valid BO4E document.
//!
//! `vertragd` builds the contract by reading this document back
//! (`angebot_bo4e.rs`) — what was quoted and what is contracted come from one
//! artefact, deliberately, so that they cannot drift. That makes the `Angebot`
//! an integration contract between two services, and an enum in it that decodes
//! to `Unknown` is a term neither side can act on.

use productd::bo4e_angebot::{build_angebot, status_from_str};
use rubo4e::current::Angebotsstatus;
use time::macros::date;

/// Every status the database can hold must map to a known BO4E value.
///
/// `status_from_str` ends in `_ => Angebotsstatus::Unknown`, which is the right
/// shape for a total function and the wrong thing to ever actually return: the
/// document would carry a status the counterparty cannot read. The `angebote`
/// CHECK constrains the column to these five, so today the arm is unreachable —
/// this test is what keeps it that way when a sixth status is added to the
/// CHECK and the mapper is forgotten.
#[test]
fn every_db_status_maps_to_a_known_bo4e_status() {
    let sql = include_str!("../migrations/0001_schema.sql");
    let start = sql
        .find("CHECK (status IN (")
        .expect("angebote.status CHECK");
    let end = start + sql[start..].find("))").expect("terminated CHECK");
    let db_values: Vec<String> = sql[start..end]
        .split(',')
        .filter_map(|tok| {
            let t = tok.trim();
            let q = t.find('\'')?;
            let rest = &t[q + 1..];
            let e = rest.find('\'')?;
            Some(rest[..e].to_owned())
        })
        .collect();

    assert!(
        db_values.len() >= 5,
        "parsed {} status value(s) — the CHECK list moved: {db_values:?}",
        db_values.len()
    );

    for v in &db_values {
        let mapped = status_from_str(v);
        assert_ne!(
            mapped,
            Angebotsstatus::Unknown,
            "angebote.status {v:?} has no BO4E Angebotsstatus — \
             `status_from_str` would emit an Angebot the counterparty cannot read"
        );
    }
}

/// Whatever `build_angebot` produces round-trips with no `Unknown` anywhere.
#[test]
fn an_emitted_angebot_is_valid_bo4e() {
    for status in [
        "ANGELEGT",
        "VERSANDT",
        "ANGENOMMEN",
        "ABGELEHNT",
        "ABGELAUFEN",
    ] {
        for sparte in [Some("STROM"), Some("GAS"), None] {
            let angebot = build_angebot(
                "ANG-2026-0001",
                status,
                date!(2026 - 03 - 31),
                Some(date!(2026 - 04 - 01)),
                sparte,
                &[],
            );
            // The outbound gate — out-of-schema enums *and* the BO4E-stated
            // rules. An Angebot productd emits is a document vertragd (and,
            // through it, a customer) receives, and mako refuses a received
            // document that breaks these.
            mako_markt::bo4e::ensure_conformant(&angebot).unwrap_or_else(|e| {
                panic!("status {status}, sparte {sparte:?}: mako would refuse this Angebot: {e}")
            });

            // And through JSON, which is how vertragd actually receives it.
            let json = serde_json::to_value(&angebot).expect("serialisable");
            let back: rubo4e::current::Angebot =
                serde_json::from_value(json).expect("round-trips as an Angebot");
            mako_markt::bo4e::ensure_conformant(&back)
                .unwrap_or_else(|e| panic!("status {status}: the JSON form would be refused: {e}"));
        }
    }
}
