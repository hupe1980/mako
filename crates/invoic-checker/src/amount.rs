//! Monetary amount type for INVOIC plausibility checks.
//!
//! [`EuroAmount`] is a fixed-point integer that stores amounts in units of
//! 10⁻⁵ EUR (1/100 000 EUR), giving five decimal places of precision.
//! This avoids all floating-point rounding issues when comparing invoice
//! amounts against PRICAT tariff rates.
//!
//! # Precision rationale
//!
//! BDEW INVOIC unit prices (NNE, Messentgelt, MMM) typically have 4–5 decimal
//! places in EUR/kWh, e.g. `0.03456 EUR/kWh`. Five decimal places are sufficient
//! to represent the full precision without loss.

use std::fmt;

/// Monetary amount in 1/100 000 EUR (10⁻⁵ EUR).
///
/// | `EuroAmount(n)` | Value       |
/// |-----------------|-------------|
/// | `100_000`       | 1.00000 EUR |
/// | `3_456`         | 0.03456 EUR |
/// | `-50_000`       | -0.50000 EUR|
///
/// Use [`EuroAmount::parse`] to construct from an EDIFACT decimal string
/// (both `.` and `,` decimal separators are accepted).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct EuroAmount(pub i64);

impl EuroAmount {
    /// Zero amount.
    pub const ZERO: Self = Self(0);

    /// Internal scale factor: 1 EUR = 100_000 units.
    const SCALE: i64 = 100_000;

    /// Parse an EDIFACT decimal string to `EuroAmount`.
    ///
    /// Accepts both `.` and `,` as decimal separators (EDIFACT allows `,`).
    /// Truncates to 5 decimal places (BDEW INVOIC precision limit).
    /// Returns `None` when the string is empty, non-numeric, or overflows.
    ///
    /// # Examples
    /// ```rust
    /// use invoic_checker::amount::EuroAmount;
    ///
    /// assert_eq!(EuroAmount::parse("1.00"), Some(EuroAmount(100_000)));
    /// assert_eq!(EuroAmount::parse("0.03456"), Some(EuroAmount(3_456)));
    /// assert_eq!(EuroAmount::parse("12345.67"), Some(EuroAmount(1_234_567_000)));
    /// assert_eq!(EuroAmount::parse("-0.50"), Some(EuroAmount(-50_000)));
    /// assert_eq!(EuroAmount::parse("1234"), Some(EuroAmount(123_400_000)));
    /// assert_eq!(EuroAmount::parse(""), None);
    /// assert_eq!(EuroAmount::parse("abc"), None);
    /// ```
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        // Normalize decimal separator.
        let s_norm;
        let s: &str = if s.contains(',') {
            s_norm = s.replace(',', ".");
            &s_norm
        } else {
            s
        };

        // Handle leading sign.
        let negative = s.starts_with('-');
        let s = if negative { &s[1..] } else { s };
        // Strip leading '+' if present.
        let s = s.strip_prefix('+').unwrap_or(s);

        let (whole_str, frac_str) = if let Some((w, f)) = s.split_once('.') {
            (w, f)
        } else {
            (s, "")
        };

        // Parse whole part.
        let whole: i64 = whole_str.parse().ok()?;

        // Pad or truncate fractional part to exactly 5 digits.
        let frac_len = frac_str.len().min(5);
        // Safety: frac_len <= frac_str.len() and frac_str is ASCII.
        let frac_significant = &frac_str[..frac_len];
        // Left-pad with the significant digits and right-pad to 5 with zeros.
        let frac_padded = format!("{frac_significant:0<5}");
        let frac_val: i64 = frac_padded.parse().ok()?;

        let amount = whole.checked_mul(Self::SCALE)?.checked_add(frac_val)?;
        Some(Self(if negative { -amount } else { amount }))
    }

    /// Convert to a human-readable EUR string with 5 decimal places.
    ///
    /// # Examples
    /// ```rust
    /// use invoic_checker::amount::EuroAmount;
    ///
    /// assert_eq!(EuroAmount(100_000).to_eur_string(), "1.00000");
    /// assert_eq!(EuroAmount(3_456).to_eur_string(), "0.03456");
    /// assert_eq!(EuroAmount(-50_000).to_eur_string(), "-0.50000");
    /// ```
    #[must_use]
    pub fn to_eur_string(self) -> String {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let whole = abs / (Self::SCALE as u64);
        let frac = abs % (Self::SCALE as u64);
        format!("{sign}{whole}.{frac:05}")
    }

    /// Returns `true` if `|self − expected| / |expected| ≤ tolerance`.
    ///
    /// When `expected` is zero: returns `true` iff `self` is also zero.
    ///
    /// `tolerance` is a fraction (e.g. `0.01` = 1 %).
    #[must_use]
    pub fn within_tolerance(self, expected: Self, tolerance: f64) -> bool {
        if expected.0 == 0 {
            return self.0 == 0;
        }
        let diff = (self.0 - expected.0).unsigned_abs() as f64;
        let base = expected.0.unsigned_abs() as f64;
        diff / base <= tolerance
    }

    /// Compute `self × quantity` where `self` is a per-kWh price and
    /// `quantity` is in kWh.  Returns the monetary total.
    ///
    /// Note: this uses `f64` for the multiplication — the result is rounded to
    /// the nearest `EuroAmount` unit (1/100 000 EUR ≈ 0.01 cent).
    #[must_use]
    pub fn multiply_by_kwh(self, kwh: f64) -> Self {
        Self((self.0 as f64 * kwh).round() as i64)
    }

    /// Absolute difference from `other`.
    #[must_use]
    pub fn abs_diff(self, other: Self) -> Self {
        Self((self.0 - other.0).abs())
    }
}

impl std::ops::Add for EuroAmount {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl std::ops::Sub for EuroAmount {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl std::ops::Neg for EuroAmount {
    type Output = Self;
    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

impl fmt::Display for EuroAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_eur_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_integer() {
        assert_eq!(EuroAmount::parse("1234"), Some(EuroAmount(123_400_000)));
        assert_eq!(EuroAmount::parse("0"), Some(EuroAmount(0)));
    }

    #[test]
    fn parse_decimal_dot() {
        assert_eq!(EuroAmount::parse("1.00"), Some(EuroAmount(100_000)));
        assert_eq!(EuroAmount::parse("0.03456"), Some(EuroAmount(3_456)));
        assert_eq!(
            EuroAmount::parse("12345.67"),
            Some(EuroAmount(1_234_567_000))
        );
    }

    #[test]
    fn parse_decimal_comma() {
        // EDIFACT allows comma as decimal separator.
        assert_eq!(EuroAmount::parse("0,03456"), Some(EuroAmount(3_456)));
        assert_eq!(
            EuroAmount::parse("12345,67"),
            Some(EuroAmount(1_234_567_000))
        );
    }

    #[test]
    fn parse_negative() {
        assert_eq!(EuroAmount::parse("-0.50"), Some(EuroAmount(-50_000)));
        assert_eq!(EuroAmount::parse("-12.50"), Some(EuroAmount(-1_250_000)));
    }

    #[test]
    fn parse_truncates_to_5_decimals() {
        // 0.123456789 → truncate to 0.12345 (5 digits)
        assert_eq!(EuroAmount::parse("0.123456789"), Some(EuroAmount(12_345)));
    }

    #[test]
    fn parse_short_fraction_padded() {
        // "1.5" → 5 digits = "50000" → 50000
        assert_eq!(EuroAmount::parse("1.5"), Some(EuroAmount(150_000)));
        // "1.50" → "50000" → 50000
        assert_eq!(EuroAmount::parse("1.50"), Some(EuroAmount(150_000)));
    }

    #[test]
    fn parse_invalid() {
        assert_eq!(EuroAmount::parse(""), None);
        assert_eq!(EuroAmount::parse("abc"), None);
        assert_eq!(EuroAmount::parse("  "), None);
    }

    #[test]
    fn to_eur_string_roundtrip() {
        let cases = [
            EuroAmount(100_000),
            EuroAmount(3_456),
            EuroAmount(1_234_567_000),
            EuroAmount(-50_000),
            EuroAmount(0),
        ];
        for a in cases {
            let s = a.to_eur_string();
            let parsed = EuroAmount::parse(&s).unwrap_or_else(|| panic!("failed to re-parse {s}"));
            assert_eq!(parsed, a, "roundtrip failed for {a}");
        }
    }

    #[test]
    fn within_tolerance() {
        let base = EuroAmount(100_000); // 1.00000 EUR
        // 1% tolerance
        assert!(EuroAmount(101_000).within_tolerance(base, 0.01)); // +1% → ok
        assert!(EuroAmount(99_000).within_tolerance(base, 0.01)); // -1% → ok
        assert!(!EuroAmount(102_000).within_tolerance(base, 0.01)); // +2% → fail
    }

    #[test]
    fn multiply_by_kwh() {
        let price = EuroAmount(3_456); // 0.03456 EUR/kWh
        let total = price.multiply_by_kwh(100.0); // 100 kWh → 3.456 EUR
        assert_eq!(total, EuroAmount(345_600));
    }

    #[test]
    fn arithmetic_ops() {
        let a = EuroAmount(100_000);
        let b = EuroAmount(50_000);
        assert_eq!(a + b, EuroAmount(150_000));
        assert_eq!(a - b, EuroAmount(50_000));
        assert_eq!(-a, EuroAmount(-100_000));
        assert_eq!(a.abs_diff(b), EuroAmount(50_000));
    }
}
