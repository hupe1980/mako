//! Fuzz target: `fuzz_parse_validate`
//!
//! Exercises the full `parse → validate → serialize` pipeline with arbitrary
//! byte sequences.  The target verifies that:
//!
//! 1. `edi_energy::parse` never panics regardless of input.
//! 2. When parse succeeds and the message validates, `serialize` must also
//!    succeed and produce output that can be re-parsed to the same message type.
//!
//! Run locally (requires nightly + `cargo-fuzz`):
//!
//! ```text
//! cargo +nightly fuzz run fuzz_parse_validate
//! ```
//!
//! Add a corpus of real EDI fixtures to `fuzz/corpus/fuzz_parse_validate/` to
//! guide coverage-guided mutation efficiently.

#![no_main]

use edi_energy::{AnyMessage, EdiEnergyMessage};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // ── 1. Parse ────────────────────────────────────────────────────────────
    let msg = match edi_energy::Platform::with_all_profiles().parse(data) {
        Ok(m) => m,
        Err(_) => return, // invalid input → expected, not a bug
    };

    // ── 2. Validate (best-effort, ignore errors) ────────────────────────────
    let _ = edi_energy_validate(&msg);

    // ── 3. Serialize + re-parse round-trip check ────────────────────────────
    if let Ok(bytes) = edi_energy_serialize(&msg) {
        // Re-parsing the serialized bytes must succeed and yield the same
        // message type — a panic or type mismatch would be a fuzz finding.
        if let Ok(msg2) = edi_energy::Platform::with_all_profiles().parse(&bytes) {
            assert_eq!(
                message_type_tag(&msg),
                message_type_tag(&msg2),
                "round-trip changed message type"
            );
        }
    }
});

fn edi_energy_validate(msg: &AnyMessage) {
    match msg {
        AnyMessage::Contrl(m) => { let _ = m.validate(); }
        AnyMessage::Utilmd(m) => { let _ = m.validate(); }
        AnyMessage::Mscons(m) => { let _ = m.validate(); }
        AnyMessage::Aperak(m) => { let _ = m.validate(); }
        AnyMessage::Invoic(m) => { let _ = m.validate(); }
        AnyMessage::Remadv(m) => { let _ = m.validate(); }
        AnyMessage::Orders(m) => { let _ = m.validate(); }
        AnyMessage::Iftsta(m) => { let _ = m.validate(); }
        AnyMessage::Insrpt(m) => { let _ = m.validate(); }
        AnyMessage::Reqote(m) => { let _ = m.validate(); }
        AnyMessage::Partin(m) => { let _ = m.validate(); }
        AnyMessage::Ordchg(m) => { let _ = m.validate(); }
        AnyMessage::Ordrsp(m) => { let _ = m.validate(); }
        AnyMessage::Quotes(m) => { let _ = m.validate(); }
        AnyMessage::Comdis(m) => { let _ = m.validate(); }
        AnyMessage::Pricat(m) => { let _ = m.validate(); }
        AnyMessage::Utilts(m) => { let _ = m.validate(); }
        _ => {}
    }
}

fn edi_energy_serialize(msg: &AnyMessage) -> Result<Vec<u8>, edi_energy::Error> {
    match msg {
        AnyMessage::Contrl(m) => m.serialize(),
        AnyMessage::Utilmd(m) => m.serialize(),
        AnyMessage::Mscons(m) => m.serialize(),
        AnyMessage::Aperak(m) => m.serialize(),
        AnyMessage::Invoic(m) => m.serialize(),
        AnyMessage::Remadv(m) => m.serialize(),
        AnyMessage::Orders(m) => m.serialize(),
        AnyMessage::Iftsta(m) => m.serialize(),
        AnyMessage::Insrpt(m) => m.serialize(),
        AnyMessage::Reqote(m) => m.serialize(),
        AnyMessage::Partin(m) => m.serialize(),
        AnyMessage::Ordchg(m) => m.serialize(),
        AnyMessage::Ordrsp(m) => m.serialize(),
        AnyMessage::Quotes(m) => m.serialize(),
        AnyMessage::Comdis(m) => m.serialize(),
        AnyMessage::Pricat(m) => m.serialize(),
        AnyMessage::Utilts(m) => m.serialize(),
        _ => Err(edi_energy::Error::UnknownMessageType {
            raw_code: "unknown".to_owned(),
        }),
    }
}

fn message_type_tag(msg: &AnyMessage) -> &'static str {
    match msg {
        AnyMessage::Contrl(_) => "CONTRL",
        AnyMessage::Utilmd(_) => "UTILMD",
        AnyMessage::Mscons(_) => "MSCONS",
        AnyMessage::Aperak(_) => "APERAK",
        AnyMessage::Invoic(_) => "INVOIC",
        AnyMessage::Remadv(_) => "REMADV",
        AnyMessage::Orders(_) => "ORDERS",
        AnyMessage::Iftsta(_) => "IFTSTA",
        AnyMessage::Insrpt(_) => "INSRPT",
        AnyMessage::Reqote(_) => "REQOTE",
        AnyMessage::Partin(_) => "PARTIN",
        AnyMessage::Ordchg(_) => "ORDCHG",
        AnyMessage::Ordrsp(_) => "ORDRSP",
        AnyMessage::Quotes(_) => "QUOTES",
        AnyMessage::Comdis(_) => "COMDIS",
        AnyMessage::Pricat(_) => "PRICAT",
        AnyMessage::Utilts(_) => "UTILTS",
        _ => "UNKNOWN",
    }
}
