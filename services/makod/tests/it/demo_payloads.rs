//! Guard: the request bodies `demos/nb-stp` posts to `makod` are ones `makod`
//! accepts.
//!
//! A demo payload is shipped API surface — it is what an evaluator copies into a
//! ticket as "this is what a command looks like". `ErpCommand` denies unknown
//! fields, so a body naming a field the envelope does not have is a `422`; what
//! this guard is really for is the opposite direction, a demo drifting from the
//! envelope while the demo still passes because its assertion is on the *effect*
//! rather than on the request.

use makod::commands_api::ErpCommand;

/// The manual NB dispatch the smoke test makes after the auto-responder has
/// already answered. Its `409` is the state-machine proof, so the body has to
/// reach the state machine rather than bounce off the envelope.
#[test]
fn the_nb_stp_bestaetigen_command_deserialises() {
    let cmd: ErpCommand = serde_json::from_value(serde_json::json!({
        "command": "gpke.lieferbeginn.bestaetigen",
        "payload": { "malo_id": "51238696012" }
    }))
    .expect("makod would refuse this demo payload — fix the demo, or the envelope");
    assert_eq!(cmd.command, "gpke.lieferbeginn.bestaetigen");
    assert_eq!(
        cmd.marktrolle, None,
        "`bestaetigen` is NB-only, so the role is inferred from the command name"
    );
}

/// The envelope's engine-owned fields are refused, not ignored.
///
/// `sender_party_id`, `receiver_party_id`, `pruefidentifikator` and
/// `message_ref` are resolved by the engine from the MaLo cache and the
/// registry. A caller that supplies one is naming a party or a PID the engine
/// will not use, and silently dropping it would put a different MP-ID on the
/// wire than the request asked for.
#[test]
fn an_engine_owned_field_is_refused() {
    let err = serde_json::from_value::<ErpCommand>(serde_json::json!({
        "command": "gpke.lieferbeginn.bestaetigen",
        "payload": { "malo_id": "51238696012" },
        "pruefidentifikator": 55002
    }))
    .expect_err("the PID is the engine's to choose, not the caller's");
    assert!(err.to_string().contains("unknown field"), "{err}");
}

/// The `payload` is a command envelope, not a BO4E document, and stays opaque.
#[test]
fn the_payload_is_not_gated_as_bo4e() {
    let cmd: ErpCommand = serde_json::from_value(serde_json::json!({
        "command": "gpke.lieferbeginn.anmelden",
        "marktrolle": "LF",
        "payload": { "malo_id": "51238696012", "lieferbeginn_datum": "2026-10-01" }
    }))
    .expect("a flat command payload is accepted as-is");
    assert_eq!(cmd.payload["lieferbeginn_datum"], "2026-10-01");
}
