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
//! | `a.within_tolerance(b, f64)` | `a.within_tolerance_ppm(b, u32)` — ppm, no f64, zero = strict equality |
//! | `a.to_eur_string()` | `format!("{a}")` (Display) |
//! | `a.abs_diff(b)` | `(a - b).abs()` |

pub use billing::Amount;
pub use billing::EuroAmount;
pub use billing::RoundingStrategy;
