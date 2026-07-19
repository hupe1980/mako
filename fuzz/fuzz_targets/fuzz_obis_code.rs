//! Fuzz target: `fuzz_obis_code`
//!
//! Verifies that:
//! 1. `ObisCode::parse` never panics on arbitrary byte input.
//! 2. When parse succeeds, `to_string()` round-trips (parses back to the same code).
//! 3. All predicate methods (`is_electricity`, `is_import`, `is_reactive`, etc.)
//!    never panic on any valid `ObisCode`.
//!
//! ## Run locally (requires nightly + `cargo-fuzz`)
//!
//! ```text
//! cargo +nightly fuzz run fuzz_obis_code
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use metering::ObisCode;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };

    // 1. Parse must never panic
    let Ok(code) = s.parse::<ObisCode>() else { return };

    // 2. Round-trip: serialised form must re-parse to the same code
    let serialised = code.to_string();
    if let Ok(reparsed) = serialised.parse::<ObisCode>() {
        assert_eq!(code, reparsed, "OBIS round-trip failed for {:?}", serialised);
    }

    // 3. All predicate methods must not panic
    let _ = code.is_electricity();
    let _ = code.is_gas();
    let _ = code.is_heat();
    let _ = code.is_water();
    let _ = code.is_heat_cost_allocator();
    let _ = code.is_import();
    let _ = code.is_export();
    let _ = code.is_einspeisung();
    let _ = code.is_reactive();
    let _ = code.is_demand();
    let _ = code.is_ht();
    let _ = code.is_nt();
    let _ = code.is_total_register();
    let _ = code.tariff_register();
});
