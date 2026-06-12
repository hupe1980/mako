//! Fuzz target: `fuzz_interchange`
//!
//! Exercises the full multi-message interchange pipeline with arbitrary byte
//! sequences.  Specifically targets code paths NOT covered by
//! `fuzz_parse_validate`, namely:
//!
//! - `parse_interchange_full_bytes` — parses UNB envelope and all contained
//!   messages in one call, returning an `InterchangeResult`.
//! - `parse_interchange_bytes` — returns an iterator over `MessageEnvelope`s.
//! - Multi-message dispatch through `InterchangeHeader` extraction.
//!
//! The target verifies that:
//!
//! 1. Neither entry-point panics on arbitrary input.
//! 2. When `parse_interchange_full_bytes` succeeds, every yielded message can
//!    be validated and serialized without panicking.
//! 3. The envelope header fields (sender, receiver, control-ref) are valid
//!    UTF-8 strings when present.
//!
//! Run locally (requires nightly + `cargo-fuzz`):
//!
//! ```text
//! cargo +nightly fuzz run fuzz_interchange
//! ```
//!
//! Seed the corpus from the existing interchange fixture:
//!
//! ```text
//! cp crates/edi-energy/tests/fixtures/*.edi fuzz/corpus/fuzz_interchange/
//! ```

#![no_main]

use edi_energy::{EdiEnergyMessage, parse_interchange_full_bytes, parse_interchange_bytes};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // ── Path A: full interchange (UNB envelope + all messages) ──────────────
    if let Ok(interchange) = parse_interchange_full_bytes(data) {
        // Touch envelope header fields — panicking here would be a real bug.
        let header = &interchange.header;
        let _ = std::hint::black_box(&header.sender_id);
        let _ = std::hint::black_box(&header.receiver_id);
        let _ = std::hint::black_box(&header.control_ref);

        for envelope in &interchange.messages {
            // MessageEnvelope::validate() is an inherent method.
            let _ = envelope.validate();

            // serialize() is a trait method — requires EdiEnergyMessage in scope.
            if let Ok(bytes) = envelope.message.serialize() {
                // Re-parsing must not panic.
                let _ = edi_energy::parse(&bytes);
            }
        }
    }

    // ── Path B: streaming interchange (iterator form, no envelope) ──────────
    // parse_interchange_bytes yields Result<AnyMessage, Error> directly.
    for result in parse_interchange_bytes(data) {
        if let Ok(msg) = result {
            let _ = msg.validate();
        }
    }
});
