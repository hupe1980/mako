//! Meter lifecycle events: installation, exchange, and retirement.
//!
//! ## Legal basis
//!
//! - **WiM Gerätewechsel-Dokumentation**: Messstellenbetreiber must document meter exchanges.
//! - **§14 MsbG**: Meter data must be available at supply handover.
//! - **BDEW GPKE**: Zählerwechsel triggers a Sonderablesung (INSRPT PID 23003).
//!
//! ## Why this matters for billing
//!
//! When a meter is replaced mid-period, two separate readings exist:
//! - **Old meter**: last reading before exchange
//! - **New meter**: first reading after exchange
//!
//! The `MeterExchangeEvent` anchors both readings to a single point in time,
//! enabling correct Mehr-/Mindermengensaldo calculation and billing continuity.

use rust_decimal::Decimal;
use time::{Date, OffsetDateTime};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Status of a physical meter at a delivery point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum MeterStatus {
    /// Meter is installed and in service.
    #[default]
    Active,
    /// Meter has been removed / decommissioned.
    Removed,
    /// Meter is installed but not yet commissioned.
    Pending,
    /// Meter was tested in lab; now deployed to a MeLo.
    Deployed,
}

/// Type of lifecycle event affecting a physical meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum MeterLifecycleEventType {
    /// Initial installation at a delivery point.
    Installed,
    /// Meter replaced with a new unit at the same delivery point.
    Replaced,
    /// Meter removed without replacement (end of supply).
    Removed,
    /// Meter firmware or calibration updated (SMGW update, new Eichung).
    Updated,
    /// Meter sealed / calibration renewed (Eichung, §28 MessEG).
    Recalibrated,
}

/// A lifecycle event for a physical meter.
///
/// Stored as an immutable audit log — never updated, only appended.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MeterLifecycleEvent {
    /// Unique event identifier.
    pub event_id: String,
    /// Meter serial number (Zählernummer / Gerätenummer).
    pub meter_serial: String,
    /// 11-digit Messlokations-ID of the delivery point.
    pub melo_id: String,
    /// Type of lifecycle event.
    pub event_type: MeterLifecycleEventType,
    /// When the event occurred (UTC).
    pub occurred_at: OffsetDateTime,
    /// The meter reading at the time of the event (kWh).
    /// `None` when not applicable (e.g., firmware update).
    pub reading_kwh: Option<Decimal>,
    /// OBIS code of the reading (when `reading_kwh` is set).
    pub obis_code: Option<crate::obis::ObisCode>,
    /// Free-text reason / operator note.
    pub reason: Option<String>,
    /// BDEW PID that triggered this event (e.g., 23003 for Zählerwechsel).
    pub triggered_by_pid: Option<u32>,
}

/// A meter exchange event: the old meter is replaced by a new one.
///
/// ## Billing continuity
///
/// The pair `(old_reading, new_reading)` at `exchange_at` enables seamless
/// computation of consumption across the exchange boundary:
///
/// ```text
/// consumption_before = old_reading_kwh − period_start_reading
/// consumption_after  = period_end_reading − new_first_reading_kwh
/// total_period       = consumption_before + consumption_after
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MeterExchangeEvent {
    /// Unique exchange identifier.
    pub exchange_id: String,
    /// 11-digit MeLo where the exchange took place.
    pub melo_id: String,
    /// Serial number of the removed meter.
    pub old_meter_serial: String,
    /// Final reading of the old meter (kWh).
    pub old_final_reading_kwh: Decimal,
    /// Serial number of the newly installed meter.
    pub new_meter_serial: String,
    /// First reading of the new meter (kWh).
    pub new_first_reading_kwh: Decimal,
    /// Date and time of the exchange (UTC).
    pub exchange_at: OffsetDateTime,
    /// Calendar date of the exchange (for billing period alignment).
    pub exchange_date: Date,
    /// BDEW PID that triggered this exchange (typically 23003).
    pub triggered_by_pid: Option<u32>,
    /// INSRPT process ID that reported this exchange.
    pub insrpt_process_id: Option<String>,
    /// Technician or system that performed the exchange.
    pub performed_by: Option<String>,
}

impl MeterExchangeEvent {
    /// Consumption attributable to the old meter during a billing period.
    ///
    /// `period_start_reading` is the old meter's reading at the beginning of
    /// the billing period (could be from a prior Jahresablesung or MSCONS read).
    #[must_use]
    pub fn consumption_old_meter_kwh(&self, period_start_reading: Decimal) -> Decimal {
        (self.old_final_reading_kwh - period_start_reading).max(Decimal::ZERO)
    }

    /// Consumption attributable to the new meter up to the billing period end.
    ///
    /// `period_end_reading` is the new meter's reading at the end of the
    /// billing period (from the final annual read or current MSCONS).
    #[must_use]
    pub fn consumption_new_meter_kwh(&self, period_end_reading: Decimal) -> Decimal {
        (period_end_reading - self.new_first_reading_kwh).max(Decimal::ZERO)
    }

    /// Total consumption across the exchange boundary for one billing period.
    #[must_use]
    pub fn total_consumption_kwh(
        &self,
        period_start_reading: Decimal,
        period_end_reading: Decimal,
    ) -> Decimal {
        self.consumption_old_meter_kwh(period_start_reading)
            + self.consumption_new_meter_kwh(period_end_reading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::{date, datetime};

    fn make_exchange() -> MeterExchangeEvent {
        MeterExchangeEvent {
            exchange_id: "EX-001".to_owned(),
            melo_id: "DE000123456789".to_owned(),
            old_meter_serial: "OLD-1234".to_owned(),
            old_final_reading_kwh: dec!(12500),
            new_meter_serial: "NEW-5678".to_owned(),
            new_first_reading_kwh: dec!(0), // new meter starts at 0
            exchange_at: datetime!(2026-06-15 8:00 UTC),
            exchange_date: date!(2026 - 06 - 15),
            triggered_by_pid: Some(23003),
            insrpt_process_id: None,
            performed_by: Some("MSB-9900357000004".to_owned()),
        }
    }

    #[test]
    fn consumption_split_across_exchange() {
        let ex = make_exchange();
        // Period: June 1–30
        // Old meter: started June 1 at 12000 kWh, ended June 15 at 12500 kWh → 500 kWh
        // New meter: started June 15 at 0, ended June 30 at 800 kWh → 800 kWh
        let old = ex.consumption_old_meter_kwh(dec!(12000));
        let new = ex.consumption_new_meter_kwh(dec!(800));
        assert_eq!(old, dec!(500));
        assert_eq!(new, dec!(800));
        assert_eq!(ex.total_consumption_kwh(dec!(12000), dec!(800)), dec!(1300));
    }

    #[test]
    fn rollover_protection() {
        // Old meter reading is lower than period start (rollover or error) → 0
        let ex = make_exchange();
        let old = ex.consumption_old_meter_kwh(dec!(13000)); // period start > final
        assert_eq!(old, Decimal::ZERO, "rollover should return 0, not negative");
    }
}
