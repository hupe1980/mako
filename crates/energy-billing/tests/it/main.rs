//! Integration tests of `energy-billing`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

#[cfg(feature = "bo4e")]
mod calculator_tests;
#[cfg(feature = "en16931")]
mod en16931_conformance;
#[cfg(feature = "bo4e")]
mod golden_scenarios;
#[cfg(all(feature = "bo4e", feature = "en16931"))]
mod one_rounding_point;
mod priceless_product_is_refused;
mod proptest_invoice;
