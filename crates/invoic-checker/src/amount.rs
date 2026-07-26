//! Monetary amount type for INVOIC plausibility checks.
//!
//! **Hard cut (billing crate migration):** `EuroAmount` is now a type alias for
//! [`billing::Amount<5>`] — a fixed-point integer that stores amounts in units of
//! 10⁻⁵ EUR (1/100 000 EUR), giving five decimal places of precision.
//!
//! All arithmetic, parsing, and rounding is delegated to the `billing` crate.
//!
//! # Migration guide
//!
//! | Old API | New API |
//! |---|---|
//! | `EuroAmount(3_456)` | `EuroAmount::from_raw_units(3_456)` |
//! | `EuroAmount::parse(s)` → `Option<Self>` | `EuroAmount::parse(s)` → `Result<Self, _>` |
//! | `price.multiply_by_kwh_decimal(kwh)` | `price.mul_qty(kwh)` |
//! | `a.within_tolerance(b, f64)` | `a.within_tolerance_ppm(b, u32)` → `bool` (billing 0.8: no `Result`, no `.unwrap_or`) |
//! | `a.to_eur_string()` | `format!("{a}")` (Display) |
//! | `a.abs_diff(b)` | `(a - b).abs()` |
//! | `EuroAmount::from_decimal(d)` → `Option` | [`euro_from_decimal`] (billing 0.8 removed the inherent method) |

pub use billing::Amount;
pub use billing::EuroAmount;
pub use billing::RoundingStrategy;

/// Convert a `Decimal` into a 5-dp [`EuroAmount`], rounding to the checker's
/// working resolution with an explicit strategy; `None` only when the value
/// overflows the representable range.
///
/// billing 0.8 removed the `Option`-returning `Amount::from_decimal` (its two
/// conversion paths disagreed and one hid the failure). `checked_from_decimal`
/// is now *exact* (errors on excess precision), but a plausibility checker works
/// at 5 dp, so rounding an over-precise INVOIC amount to that resolution is the
/// correct model — made explicit here via `from_decimal_rounded` rather than the
/// old silent rounding. Kept as a free fn so it still passes to
/// `.and_then`/`.filter_map`.
#[must_use]
pub fn euro_from_decimal(d: rust_decimal::Decimal) -> Option<EuroAmount> {
    EuroAmount::from_decimal_rounded(d, RoundingStrategy::MidpointAwayFromZero).ok()
}
