//! Core metering types: [`MeterInterval`], [`Sparte`], [`QualityFlag`].

use rust_decimal::Decimal;
use time::OffsetDateTime;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Energy commodity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Sparte {
    /// Electricity.
    #[default]
    Strom,
    /// Natural gas.
    Gas,
}

/// BDEW / MSCONS quality flag.
///
/// Maps to the `MESSWERTSTATUS` field in MSCONS and
/// the BO4E `Messwertstatus` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum QualityFlag {
    /// Reading as measured (Abgelesen).
    Measured,
    /// Estimated value (Prognosewert).
    Estimated,
    /// Substituted / replaced value (Ersatzwert).
    Substituted,
    /// Calculated / derived value (Vorlaeufiger Wert).
    Calculated,
    /// Quality not known.
    #[default]
    Unknown,
}

impl QualityFlag {
    /// `true` when this flag indicates the value is reliable for billing.
    #[must_use]
    pub fn is_billable(&self) -> bool {
        matches!(
            self,
            QualityFlag::Measured | QualityFlag::Calculated | QualityFlag::Substituted
        )
    }
}

/// A single metered interval: the fundamental unit of meter data.
///
/// All energy values are in **kWh** (Strom) or **kWh_Hs** (Gas after conversion).
/// Use [`crate::conversion::gas_m3_to_kwh_hs`] to convert Gas m³ readings before
/// creating `MeterInterval`s.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MeterInterval {
    /// Interval start (UTC, inclusive).
    pub from: OffsetDateTime,
    /// Interval end (UTC, exclusive).
    pub to: OffsetDateTime,
    /// Energy quantity in kWh (or kWh_Hs for Gas).
    pub value_kwh: Decimal,
    /// Reading quality.
    pub quality: QualityFlag,
    /// OBIS-Kennzahl (e.g. `"1-0:1.8.0"`). `None` when not provided by MSCONS.
    pub obis_code: Option<String>,
}

impl MeterInterval {
    /// Duration in whole seconds.
    #[must_use]
    pub fn duration_secs(&self) -> i64 {
        (self.to - self.from).whole_seconds()
    }

    /// Duration in minutes.
    #[must_use]
    pub fn duration_minutes(&self) -> i64 {
        (self.to - self.from).whole_minutes()
    }

    /// Instantaneous demand in kW, computed as `kWh ÷ (duration_h)`.
    ///
    /// Only meaningful for RLM intervals (15-min or 60-min).
    /// For a 15-min interval carrying 2.5 kWh: demand = 2.5 × 4 = 10 kW.
    #[must_use]
    pub fn demand_kw(&self) -> Option<Decimal> {
        let h = Decimal::from(self.duration_secs()) / Decimal::from(3600u32);
        if h.is_zero() {
            None
        } else {
            Some(self.value_kwh / h)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use time::macros::datetime;

    #[test]
    fn demand_kw_15min_interval() {
        let iv = MeterInterval {
            from: datetime!(2026-01-01 0:00 UTC),
            to: datetime!(2026-01-01 0:15 UTC),
            value_kwh: dec!(2.5),
            quality: QualityFlag::Measured,
            obis_code: None,
        };
        // 2.5 kWh in 15 min = 10 kW
        assert_eq!(iv.demand_kw(), Some(dec!(10)));
    }

    #[test]
    fn demand_kw_hourly() {
        let iv = MeterInterval {
            from: datetime!(2026-01-01 0:00 UTC),
            to: datetime!(2026-01-01 1:00 UTC),
            value_kwh: dec!(5.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        };
        // 5.0 kWh in 60 min = 5 kW
        assert_eq!(iv.demand_kw(), Some(dec!(5)));
    }

    #[test]
    fn quality_flag_billable() {
        assert!(QualityFlag::Measured.is_billable());
        assert!(QualityFlag::Substituted.is_billable());
        assert!(!QualityFlag::Estimated.is_billable());
        assert!(!QualityFlag::Unknown.is_billable());
    }
}
