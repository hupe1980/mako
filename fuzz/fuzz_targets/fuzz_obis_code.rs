//! Fuzz target: `fuzz_obis_code`
//!
//! Verifies that:
//! 1. `ObisCode::parse` never panics on arbitrary byte input.
//! 2. When parse succeeds, `to_string()` round-trips (parses back to the same code).
//! 3. All predicate methods (`is_electricity`, `is_import`, `is_reactive`, etc.)
//!    never panic on any valid `ObisCode`.
//! 4. The derived direction predicates agree with the `direction()` primitive,
//!    and neither the Fehlerregister nor the total register is reported as a
//!    tariff — reading either as one bills a fault counter as consumption.
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
    let _ = code.direction();
    let _ = code.is_import();
    let _ = code.is_export();
    let _ = code.is_reactive();
    let _ = code.is_lastgang();
    let _ = code.is_maximum();
    let _ = code.is_zaehlerstand();
    let _ = code.is_vorschub();
    let _ = code.is_ht();
    let _ = code.is_nt();
    let _ = code.is_total_register();
    let _ = code.is_fehlerregister();
    let _ = code.tariff_register();

    // 4. `is_import`/`is_export` are derived from `direction()`: at most one of
    //    them holds, and neither holds where the channel has no direction.
    assert!(!(code.is_import() && code.is_export()), "code {serialised} counts both ways");
    assert_eq!(
        code.direction().is_some(),
        code.is_import() || code.is_export(),
        "direction disagrees with its own derived predicates for {serialised}"
    );

    // 5. The Fehlerregister is not a tariff, and neither is the total register.
    assert!(
        !(code.is_fehlerregister() && code.tariff_register().is_some()),
        "Fehlerregister reported as tariff {:?}",
        code.tariff_register()
    );
    assert!(
        !(code.is_total_register() && code.tariff_register().is_some()),
        "total register reported as tariff {:?}",
        code.tariff_register()
    );
});
