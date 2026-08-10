//! Monetary amount type for INVOIC plausibility checks.
//!
//! `EuroAmount` is [`billing::Amount<5>`] — a fixed-point integer that stores
//! amounts in units of 10⁻⁵ EUR (1/100 000 EUR), giving five decimal places of
//! precision. All arithmetic, parsing, and rounding is delegated to the
//! `billing` crate; this module adds only the checker-specific conversion helper
//! below.

pub use billing::Amount;
pub use billing::RoundingStrategy;

/// A monetary amount in euro at the checker's 5-decimal working resolution.
///
/// `billing` 0.12 removed its own `EuroAmount` alias because the crate is
/// currency-agnostic and the name asserted a currency the type does not carry.
/// mako's INVOIC domain *is* euro-denominated (BDEW MaKo settles in EUR), so the
/// alias is correct here — it just belongs to the domain crate, not the engine.
pub type EuroAmount = Amount<5>;

/// Convert a `Decimal` into a 5-dp [`EuroAmount`], rounding to the checker's
/// working resolution with an explicit strategy; `None` only when the value
/// overflows the representable range.
///
/// `billing`'s own `checked_from_decimal` is *exact* — it errors on excess
/// precision rather than rounding. A plausibility checker works at 5 dp, so
/// rounding an over-precise INVOIC amount to that resolution is the correct
/// model; this helper makes that explicit via `from_decimal_rounded`. Kept as a
/// free fn so it still passes to `.and_then` / `.filter_map`.
#[must_use]
pub fn euro_from_decimal(d: rust_decimal::Decimal) -> Option<EuroAmount> {
    EuroAmount::from_decimal_rounded(d, RoundingStrategy::MidpointAwayFromZero).ok()
}
