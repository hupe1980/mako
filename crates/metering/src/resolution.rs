//! `IntervalResolution` — typed interval length enum replacing raw `u32` seconds.
//!
//! ## Why typed?
//!
//! Raw `u32` seconds (e.g. `900`) are error-prone and opaque. `IntervalResolution`
//! makes the intended granularity explicit and prevents confusion between hourly
//! and daily data in billing aggregation.
//!
//! ## German market standard resolutions
//!
//! | Resolution | Seconds | Usage |
//! |---|---|---|
//! | `QuarterHour` | 900 | RLM, iMSys, SMGW (BSI TR-03109) |
//! | `HalfHour` | 1800 | Some old MSB systems |
//! | `Hour` | 3600 | Gas, some SLP reconstructed profiles |
//! | `Day` | 86400 | Daily meter reads, Jahresverbrauch |
//! | `Month` | variable | Monthly billing periods |
//! | `Year` | variable | Annual total |
//! | `Custom(u32)` | n seconds | Non-standard intervals |

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Typed interval resolution for meter data time series.
///
/// Use `as_seconds()` to get the equivalent duration in seconds for arithmetic.
/// Use `label()` to get a human-readable German name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum IntervalResolution {
    /// 15-minute intervals (900 s) — standard for RLM, iMSys, SMGW.
    QuarterHour,
    /// 30-minute intervals (1800 s) — some legacy MSB systems.
    HalfHour,
    /// Hourly intervals (3600 s) — Gas, some SLP reconstructed profiles.
    Hour,
    /// Daily intervals (86400 s) — daily meter reads.
    Day,
    /// Monthly intervals — billing period granularity.
    Month,
    /// Annual intervals — yearly totals.
    Year,
    /// Custom interval length in seconds (for non-standard cases).
    Custom(u32),
}

impl IntervalResolution {
    /// Returns the interval duration in whole seconds.
    ///
    /// For `Month` and `Year`, returns an approximate value (30×86400 and 365×86400).
    /// Use with caution for exact date arithmetic — prefer `time::Duration` for precision.
    #[must_use]
    pub fn as_seconds(self) -> u32 {
        match self {
            Self::QuarterHour => 900,
            Self::HalfHour => 1800,
            Self::Hour => 3600,
            Self::Day => 86400,
            Self::Month => 30 * 86400,
            Self::Year => 365 * 86400,
            Self::Custom(s) => s,
        }
    }

    /// Human-readable label (German).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::QuarterHour => "15-Minuten",
            Self::HalfHour => "30-Minuten",
            Self::Hour => "Stunde",
            Self::Day => "Tag",
            Self::Month => "Monat",
            Self::Year => "Jahr",
            Self::Custom(_) => "Benutzerdefiniert",
        }
    }

    /// Create from raw seconds. Returns `None` for zero.
    #[must_use]
    pub fn from_seconds(s: u32) -> Option<Self> {
        match s {
            0 => None,
            900 => Some(Self::QuarterHour),
            1800 => Some(Self::HalfHour),
            3600 => Some(Self::Hour),
            86400 => Some(Self::Day),
            _ => Some(Self::Custom(s)),
        }
    }

    /// `true` when this resolution supports real-time or near-real-time data.
    ///
    /// Quarter-hour and half-hour are the relevant resolutions for iMSys / SMGW.
    #[must_use]
    pub fn is_subhourly(self) -> bool {
        matches!(self, Self::QuarterHour | Self::HalfHour)
    }
}

impl std::fmt::Display for IntervalResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}s ({})", self.as_seconds(), self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_hour_is_900s() {
        assert_eq!(IntervalResolution::QuarterHour.as_seconds(), 900);
    }

    #[test]
    fn from_seconds_round_trip() {
        for r in [
            IntervalResolution::QuarterHour,
            IntervalResolution::HalfHour,
            IntervalResolution::Hour,
            IntervalResolution::Day,
        ] {
            let s = r.as_seconds();
            assert_eq!(IntervalResolution::from_seconds(s), Some(r));
        }
    }

    #[test]
    fn zero_returns_none() {
        assert!(IntervalResolution::from_seconds(0).is_none());
    }

    #[test]
    fn custom_seconds_preserved() {
        let r = IntervalResolution::Custom(7200);
        assert_eq!(r.as_seconds(), 7200);
        assert!(!r.is_subhourly());
    }

    #[test]
    fn subhourly_detection() {
        assert!(IntervalResolution::QuarterHour.is_subhourly());
        assert!(IntervalResolution::HalfHour.is_subhourly());
        assert!(!IntervalResolution::Hour.is_subhourly());
        assert!(!IntervalResolution::Day.is_subhourly());
    }
}
