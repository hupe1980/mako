//! Fuzz target: `fuzz_tariff_input`
//!
//! Verifies that:
//! 1. `Product` JSON deserialization never panics on arbitrary input.
//! 2. `Product::build_engine` never panics on any deserialized product.
//! 3. Billing a zero-consumption `Quantities` through the resulting engine
//!    never panics.
//!
//! ## What this catches
//!
//! - Integer overflow in Grundpreis × days
//! - Panic from unusual `Decimal` values (extreme scale or magnitude)
//! - Panic in block tariff construction (contiguous band violations)
//! - Panic in indexed price resolution
//! - Panic in seasonal price lookup with edge-case month values
//!
//! ## Threat model
//!
//! A product definition is untrusted JSONB: it reaches the billing engine from
//! the retail catalogue, where it is authored by an operator rather than
//! type-checked at the boundary. A panic here takes down a billing run.
//!
//! ## Run locally
//!
//! ```text
//! cargo +nightly fuzz run fuzz_tariff_input
//! ```
//!
//! ## Corpus
//!
//! Add representative retail-catalogue JSONB samples to
//! `fuzz/corpus/fuzz_tariff_input/` to guide coverage-guided mutation.

#![no_main]

use energy_billing::{
    BillingContext, BillingPeriod, GridInput, InvoiceType, Product, Quantities, RegulatoryRates,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // ── Step 1: Deserialize a Product from arbitrary bytes ────────────────────
    // Many inputs will not be valid JSON — that's fine, we just skip them.
    let Ok(json) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(product): Result<Product, _> = serde_json::from_str(json) else {
        return;
    };

    // ── Step 2: Build the engine for this product ─────────────────────────────
    let rates = RegulatoryRates::default();
    let engine = product.build_engine(&GridInput::default(), &rates);

    // ── Step 3: Bill zero-consumption quantities ──────────────────────────────
    // An empty `Quantities` keeps the run to the code paths that depend only on
    // the product definition itself.
    let Ok(period) = BillingPeriod::new(
        time::macros::date!(2026-01-01),
        time::macros::date!(2026-01-31),
    ) else {
        return;
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "FUZZ-001".to_owned(),
        period,
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates,
        ..Default::default()
    };

    let _ = engine.bill(ctx, &Quantities::default());
    // Any panic here is a bug. We do not assert on the result —
    // billing may legitimately return Err for degenerate products.
});
