//! Fuzz target: `fuzz_builder`
//!
//! Exercises EDIFACT message builder field setters with arbitrary UTF-8 input
//! to verify that:
//!
//! 1. No builder method panics regardless of the string content.
//! 2. `build()` and `serialize()` either succeed (producing valid EDIFACT bytes)
//!    or return a structured `Err` — but never panic.
//! 3. When `serialize()` succeeds, the resulting bytes can be re-parsed
//!    without panicking (round-trip safety).
//!
//! This covers attack surfaces not addressed by `fuzz_parse_validate`:
//! - Injection of EDIFACT special chars (`'`, `+`, `:`, `?`) in field values
//! - Empty strings, very long strings
//! - NUL bytes and other non-printable characters in GLN/EIC fields
//!
//! Run locally (requires nightly + `cargo-fuzz`):
//!
//! ```text
//! cargo +nightly fuzz run fuzz_builder
//! ```

#![no_main]

use edi_energy::{
    Platform, Pruefidentifikator, Release,
    builders::{AperakBuilder, ContrlBuilder, RemadvBuilder},
};
use libfuzzer_sys::{arbitrary, fuzz_target};

/// Structured fuzz inputs so `libfuzzer` can do guided mutation rather than
/// raw byte mutation. This dramatically improves fuzzer efficiency for builder
/// code paths because it avoids wasting mutations on the field delimiter.
#[derive(arbitrary::Arbitrary, Debug)]
struct BuilderInputs {
    sender_id:   String,
    receiver_id: String,
    message_ref: String,
    error_code:  String,
    error_text:  String,
    acw_ref:     String,
    /// PID value 10000–99999 to occasionally hit valid range.
    raw_pid:     u32,
}

fuzz_target!(|inputs: BuilderInputs| {
    let platform = Platform::with_all_profiles();

    // ── APERAK builder ────────────────────────────────────────────────────────
    {
        // Use a fixed release string — we are fuzzing field values, not the
        // release lookup path (which is covered by fuzz_parse_validate).
        let release = Release::new("2.1i");

        // Build an APERAK — errors (e.g. unknown PID) are expected; panics are not.
        let pid_value = (inputs.raw_pid % 90000) + 10000; // 10000–99999
        let pid = Pruefidentifikator::new(pid_value).unwrap_or_else(|_| {
            Pruefidentifikator::new(11001).unwrap()
        });

        let builder = AperakBuilder::new(release.clone())
            .sender(&inputs.sender_id)
            .receiver(&inputs.receiver_id)
            .message_ref(&inputs.message_ref)
            .error_code(&inputs.error_code)
            .error_text(&inputs.error_text)
            .acw_ref(&inputs.acw_ref)
            .pruefidentifikator(pid);

        // serialize() is the full path: build → encode → serialise.
        if let Ok(bytes) = builder.serialize() {
            let _ = platform.parse(&bytes);
        }
    }

    // ── CONTRL builder ────────────────────────────────────────────────────────
    {
        let release = Release::new("D.96A");
        let builder = ContrlBuilder::new(release)
            .sender(&inputs.sender_id)
            .receiver(&inputs.receiver_id)
            .interchange_ref(&inputs.message_ref)
            .message_ref(&inputs.acw_ref);

        if let Ok(bytes) = builder.serialize() {
            let _ = platform.parse(&bytes);
        }
    }

    // ── REMADV builder ────────────────────────────────────────────────────────
    {
        let release = Release::new("2.9d");
        let builder = RemadvBuilder::new(release)
            .sender(&inputs.sender_id)
            .receiver(&inputs.receiver_id)
            .message_ref(&inputs.message_ref);

        if let Ok(bytes) = builder.serialize() {
            let _ = platform.parse(&bytes);
        }
    }
});
