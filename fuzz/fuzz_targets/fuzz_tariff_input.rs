//! Fuzz target: `fuzz_tariff_input`
//!
//! Verifies that:
//! 1. `TariffInput` JSON deserialization never panics on arbitrary input.
//! 2. `PricingModel::try_from(tariff)` never panics on any deserialized tariff.
//! 3. When `PricingModel::build_engine()` succeeds, billing a zero-consumption
//!    `Quantities` never panics.
//!
//! ## What this catches
//!
//! - Integer overflow in Grundpreis × days
//! - NaN/infinity from unusual Decimal values
//! - Panic in block tariff construction (contiguous band violations)
//! - Panic in indexed price resolution
//! - Panic in seasonal price lookup with edge-case month values
//!
//! ## Run locally
//!
//! ```text
//! cargo +nightly fuzz run fuzz_tariff_input
//! ```
//!
//! ## Corpus
//!
//! Add representative tarifbd JSONB samples to
//! `fuzz/corpus/fuzz_tariff_input/` to guide coverage-guided mutation.

#![no_main]

use energy_billing::{
    BillingContext, GridInput, InvoiceType, PricingModel, Quantities, RegulatoryRates, TariffInput,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // ── Step 1: Deserialize TariffInput from arbitrary bytes ──────────────────
    // Many inputs will not be valid JSON — that's fine, we just skip them.
    let Ok(json) = std::str::from_utf8(data) else { return };
    let Ok(tariff): Result<TariffInput, _> = serde_json::from_str(json) else { return };

    // ── Step 2: Try to build a PricingModel ───────────────────────────────────
    // Unknown or BUNDLE categories return Err — that's correct behaviour, not a bug.
    let Ok(model) = PricingModel::try_from(tariff) else { return };

    // ── Step 3: Try to build a BillingEngine ──────────────────────────────────
    let rates = RegulatoryRates::default();
    let Ok(engine) = model.build_engine(&GridInput::default(), &rates) else { return };

    // ── Step 4: Bill zero-consumption quantities ──────────────────────────────
    // We use an empty Quantities so no actual computation is needed.
    // The goal is to exercise all code paths that depend only on the tariff.
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "FUZZ-001".to_owned(),
        period_from: time::macros::date!(2026-01-01),
        period_to: time::macros::date!(2026-01-31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates,
        ..Default::default()
    };

    let _ = engine.bill(ctx, &Quantities::default());
    // Any panic here is a bug. We do not assert on the result —
    // billing may legitimately return Err for degenerate tariffs.
});
