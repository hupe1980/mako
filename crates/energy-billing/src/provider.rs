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

use crate::error::EngineError;

use crate::context::BillingContext;
use crate::position::{BillingPosition, BillingWarning};

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

/// Market Time Unit (MTU) length for the EPEX day-ahead auction, in minutes.
///
/// Since 2025-10-01 the SDAC day-ahead auction settles on **15-minute**
/// products (96 quarter-hours per delivery day; 92/100 on the DST days) —
/// EPEX SPOT go-live 01.10.2025. All spot pricing is keyed on this MTU.
pub const MTU_MINUTES: i64 = 15;

/// Floor a UTC instant to the start of its EPEX market time unit (quarter-hour).
///
/// CET/CEST are whole-hour offsets, so a local quarter-hour boundary is always
/// a UTC quarter-hour boundary — flooring in UTC is therefore DST-safe and
/// needs no timezone conversion. This is the canonical spot-price map key.
#[must_use]
pub fn mtu_start(timestamp_utc: time::OffsetDateTime) -> time::OffsetDateTime {
    let step = MTU_MINUTES * 60;
    let secs = timestamp_utc.unix_timestamp();
    let floored = secs - secs.rem_euclid(step);
    time::OffsetDateTime::from_unix_timestamp(floored).unwrap_or(timestamp_utc)
}

/// EPEX Spot Day-Ahead price lookup map.
///
/// Key: the UTC start instant of the 15-minute market time unit ([`mtu_start`]).
/// Value: spot price in ct/kWh. DST-safe and resolution-agnostic — a lookup
/// floors any timestamp to its quarter-hour before matching.
pub struct EpexSpotSource {
    /// Maps the quarter-hour MTU start (UTC) → ct/kWh.
    pub prices: std::collections::HashMap<time::OffsetDateTime, rust_decimal::Decimal>,
}

impl SpotPriceSource for EpexSpotSource {
    fn price_ct_kwh(&self, timestamp_utc: time::OffsetDateTime) -> Option<rust_decimal::Decimal> {
        self.prices.get(&mtu_start(timestamp_utc)).copied()
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
///     ) -> Result<Vec<BillingPosition>, EngineError> {
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
    ) -> Result<Vec<BillingPosition>, EngineError>;

    /// `true` when this provider computes taxes on accumulated prior positions.
    ///
    /// Tax providers run in a second pass, after all commodity/levy providers
    /// have completed. The default is `false`.
    fn is_tax_pass(&self) -> bool {
        false
    }

    /// Produce regulatory compliance warnings without generating billing positions.
    ///
    /// Called by [`BillingEngine::validate()`](crate::BillingEngine::validate) and
    /// [`BillingEngine::bill()`](crate::BillingEngine::bill) to collect warnings
    /// before and during billing.
    ///
    /// Default implementation returns no warnings. Override in providers that must
    /// enforce regulatory preconditions (e.g. `DynamicElectricityProvider` enforces
    /// §41b iMSys requirement).
    ///
    /// Warnings with `WarningSeverity::Error` cause `BillingEngine::bill()` to return
    /// [`EngineError::ValidationBlocked`] before any positions are generated.
    fn validate_warnings(
        &self,
        _ctx: &BillingContext,
        _quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        vec![]
    }
}
