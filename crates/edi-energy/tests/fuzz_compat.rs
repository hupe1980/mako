//! Compile-time completeness guard for the fuzz target.
//!
//! The fuzz target at `fuzz/fuzz_targets/fuzz_parse_validate.rs` manually
//! matches on `AnyMessage` variants.  If a new message type is added without
//! updating the fuzz target, the fuzz target silently skips coverage for the
//! new variant — introducing a security gap.
//!
//! This module has an **exhaustive** match on every `AnyMessage` variant that
//! is compiled in under `--all-features`.  No wildcard `_ =>` arm is present,
//! so adding a new variant to `AnyMessage` causes a compile error here,
//! forcing the developer to also update the fuzz target and this guard.
//!
//! To verify coverage manually:
//! ```text
//! cargo test --test fuzz_compat --all-features
//! ```

/// Statically assert that the fuzz target covers all compiled-in `AnyMessage`
/// variants.  The function is never called at runtime — it exists purely for
/// the exhaustiveness check that Rust performs at compile time.
///
/// This must be updated whenever a new variant is added to `AnyMessage` or a
/// new Cargo feature gate is introduced.
#[allow(
    unreachable_code,
    unused_variables,
    dead_code,
    clippy::result_large_err
)]
fn assert_fuzz_target_covers_all_variants(msg: edi_energy::AnyMessage) {
    // Mirrors edi_energy_validate() in fuzz/fuzz_targets/fuzz_parse_validate.rs.
    // DO NOT add a wildcard `_ =>` arm — exhaustiveness is the entire point.
    match msg {
        #[cfg(feature = "contrl")]
        edi_energy::AnyMessage::Contrl(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "utilmd")]
        edi_energy::AnyMessage::Utilmd(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "mscons")]
        edi_energy::AnyMessage::Mscons(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "aperak")]
        edi_energy::AnyMessage::Aperak(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "invoic")]
        edi_energy::AnyMessage::Invoic(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "remadv")]
        edi_energy::AnyMessage::Remadv(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "orders")]
        edi_energy::AnyMessage::Orders(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "iftsta")]
        edi_energy::AnyMessage::Iftsta(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "insrpt")]
        edi_energy::AnyMessage::Insrpt(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "reqote")]
        edi_energy::AnyMessage::Reqote(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "partin")]
        edi_energy::AnyMessage::Partin(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "ordchg")]
        edi_energy::AnyMessage::Ordchg(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "ordrsp")]
        edi_energy::AnyMessage::Ordrsp(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "quotes")]
        edi_energy::AnyMessage::Quotes(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "comdis")]
        edi_energy::AnyMessage::Comdis(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "pricat")]
        edi_energy::AnyMessage::Pricat(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        #[cfg(feature = "utilts")]
        edi_energy::AnyMessage::Utilts(m) => {
            let _ = edi_energy::EdiEnergyMessage::validate(&m);
        }
        // Unknown is intentionally NOT matched for validate() (no validate impl),
        // but IS listed here so Rust knows we handle it.
        edi_energy::AnyMessage::Unknown { .. } => {}
    }
}

/// Same exhaustiveness guard for the serialize helper in the fuzz target.
#[allow(
    unreachable_code,
    unused_variables,
    dead_code,
    clippy::result_large_err
)]
fn assert_fuzz_target_serialize_covers_all_variants(
    msg: edi_energy::AnyMessage,
) -> Result<Vec<u8>, edi_energy::Error> {
    match msg {
        #[cfg(feature = "contrl")]
        edi_energy::AnyMessage::Contrl(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "utilmd")]
        edi_energy::AnyMessage::Utilmd(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "mscons")]
        edi_energy::AnyMessage::Mscons(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "aperak")]
        edi_energy::AnyMessage::Aperak(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "invoic")]
        edi_energy::AnyMessage::Invoic(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "remadv")]
        edi_energy::AnyMessage::Remadv(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "orders")]
        edi_energy::AnyMessage::Orders(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "iftsta")]
        edi_energy::AnyMessage::Iftsta(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "insrpt")]
        edi_energy::AnyMessage::Insrpt(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "reqote")]
        edi_energy::AnyMessage::Reqote(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "partin")]
        edi_energy::AnyMessage::Partin(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "ordchg")]
        edi_energy::AnyMessage::Ordchg(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "ordrsp")]
        edi_energy::AnyMessage::Ordrsp(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "quotes")]
        edi_energy::AnyMessage::Quotes(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "comdis")]
        edi_energy::AnyMessage::Comdis(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "pricat")]
        edi_energy::AnyMessage::Pricat(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        #[cfg(feature = "utilts")]
        edi_energy::AnyMessage::Utilts(m) => edi_energy::EdiEnergyMessage::serialize(&m),
        edi_energy::AnyMessage::Unknown { .. } => Err(edi_energy::Error::UnknownMessageType {
            raw_code: "unknown".into(),
        }),
    }
}

/// Dummy test so this file is compiled and the exhaustiveness checks run.
#[test]
fn fuzz_compat_guard_compiles() {
    // The real assertion is at compile time: if the functions above
    // compile without error, all AnyMessage variants are covered.
    // This test exists only so `cargo test --test fuzz_compat` picks up the file.
}
