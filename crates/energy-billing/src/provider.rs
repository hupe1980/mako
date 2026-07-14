//! `BillingProvider` trait and `SpotPriceSource` abstraction.
//!
//! Every product type (electricity, gas, EEG feed-in, HEMS…) implements
//! `BillingProvider`. The `BillingEngine` orchestrates them in order.
//!
//! ## Execution order and tax computation
//!
//! Providers run in registration order. Each receives `prior_positions` — all
//! positions produced by earlier providers. Tax providers (MwSt) are typically
//! registered last and compute their amount from `prior_positions`.
//!
//! ```text
//! ElectricityProvider  → commodity + grid + Stromsteuer positions
//! GridChargeProvider   → NNE/KA positions (if not already in Electricity)
//! StromsteuerProvider  → levy position (if separate from ElectricityProvider)
//! MwStProvider         → tax position on sum(prior_positions)
//! ```
//!
//! The `is_tax_pass()` method marks tax providers so the engine knows to run them
//! in a second pass (after all commodity/levy providers have executed).

use billing::BillingError;

use crate::context::BillingContext;
use crate::position::BillingPosition;

// ── Quantities ────────────────────────────────────────────────────────────────

pub use crate::quantities::Quantities;

// ── SpotPriceSource ───────────────────────────────────────────────────────────

/// Abstraction over spot electricity price sources.
///
/// Decouples the §41a EnWG dynamic tariff implementation from any specific
/// exchange (EPEX, NordPool, Tibber, aWATTar, etc.).
///
/// ## Extension
///
/// Implement this trait to add:
/// - `NordPoolSource` (Nordic / Baltic day-ahead)
/// - `TibberSource` (real-time pricing)
/// - `aWATTarSource`
/// - `EntsoESource` (ENTSO-E transparency platform)
pub trait SpotPriceSource: Send + Sync {
    /// Price in ct/kWh for the given UTC timestamp.
    ///
    /// Returns `None` when price data is unavailable for the timestamp.
    fn price_ct_kwh(&self, timestamp_utc: time::OffsetDateTime) -> Option<rust_decimal::Decimal>;

    /// Source name for billing position labels (e.g. `"EPEX Spot Day-Ahead"`).
    fn source_name(&self) -> &str;
}

/// EPEX Spot Day-Ahead price lookup map.
///
/// Key: `(year, month, day, hour_CET)` — local German time (CET/CEST).
/// Value: spot price in ct/kWh.
pub struct EpexSpotSource {
    /// Maps `(year, month, day, hour_CET)` → ct/kWh.
    pub prices: std::collections::HashMap<(i32, u8, u8, u8), rust_decimal::Decimal>,
}

impl SpotPriceSource for EpexSpotSource {
    fn price_ct_kwh(&self, timestamp_utc: time::OffsetDateTime) -> Option<rust_decimal::Decimal> {
        // Convert UTC → German local time for the map key
        use time_tz::{OffsetDateTimeExt, timezones};
        let berlin = timezones::db::europe::BERLIN;
        let local = timestamp_utc.to_timezone(berlin);
        let key = (local.year(), local.month() as u8, local.day(), local.hour());
        self.prices.get(&key).copied()
    }

    fn source_name(&self) -> &str {
        "EPEX Spot Day-Ahead"
    }
}

// ── BillingProvider trait ─────────────────────────────────────────────────────

/// A product or service component that generates billing positions.
///
/// Implement this trait for each billable product type. The engine calls
/// `bill()` for each registered provider in order, passing the accumulated
/// positions from all earlier providers.
///
/// ## Tax providers
///
/// Override `is_tax_pass()` to return `true` when this provider computes taxes
/// on the accumulated positions (e.g. MwSt). The engine ensures all commodity/
/// levy providers run before any tax provider.
///
/// ## Example
///
/// ```rust,ignore
/// struct MyFlatFeeProvider { eur: Decimal }
///
/// impl BillingProvider for MyFlatFeeProvider {
///     fn bill(
///         &self,
///         _ctx: &BillingContext,
///         _quantities: &Quantities,
///         _prior: &[BillingPosition],
///     ) -> Result<Vec<BillingPosition>, BillingError> {
///         Ok(vec![
///             BillingPosition::debit("Service Fee", Decimal::ONE, "Pauschal", self.eur, PositionCategory::Fee)
///                 .with_tag("service_fee"),
///         ])
///     }
/// }
/// ```
pub trait BillingProvider: Send + Sync {
    /// Generate billing positions for this provider.
    ///
    /// `prior` contains all positions from providers that ran before this one.
    /// Most providers ignore `prior`; tax providers use it to compute their base.
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError>;

    /// `true` when this provider computes taxes on accumulated prior positions.
    ///
    /// Tax providers run in a second pass, after all commodity/levy providers
    /// have completed. The default is `false`.
    fn is_tax_pass(&self) -> bool {
        false
    }
}
