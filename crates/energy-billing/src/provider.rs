//! The `BillingProvider` trait — one implementation per product type.
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

// ── Market time unit ──────────────────────────────────────────────────────────

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
/// needs no timezone conversion. This is the canonical spot-price map key:
/// [`Quantities::dynamic_epex_prices`](crate::Quantities::dynamic_epex_prices)
/// is keyed on it, and a consumption interval is floored to it before lookup.
///
/// There is deliberately no `SpotPriceSource` trait behind this. One existed,
/// with one implementation over a `HashMap` and a documented invitation to add
/// NordPool and Tibber adapters; every construction path in the workspace
/// passed it an **empty** map and priced from `dynamic_epex_prices` anyway. A
/// seam nothing enters is not an extension point, it is a second code path to
/// keep correct — and this one hid the price lookup behind a `dyn` call that
/// could return `None` for reasons the caller could not see.
#[must_use]
pub fn mtu_start(timestamp_utc: time::OffsetDateTime) -> time::OffsetDateTime {
    let step = MTU_MINUTES * 60;
    let secs = timestamp_utc.unix_timestamp();
    let floored = secs - secs.rem_euclid(step);
    time::OffsetDateTime::from_unix_timestamp(floored).unwrap_or(timestamp_utc)
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

    /// The VAT rate this provider charges a position that states none of its own.
    ///
    /// Only a tax provider answers. The engine stamps it onto every supply
    /// position before the tax pass, so the amount charged and the BG-23
    /// breakdown that states it are read off the same number: a § 19 UStG
    /// Kleinunternehmer document charges nothing and must therefore state
    /// nothing, and an invoice that prints a 19 % Steuerbetrag beside
    /// `mwst_eur = 0.00` is an unrechtmäßiger Steuerausweis (§ 14c Abs. 2 UStG).
    fn charged_tax_rate(&self) -> Option<rust_decimal::Decimal> {
        None
    }

    /// Produce regulatory compliance warnings without generating billing positions.
    ///
    /// Called by [`BillingEngine::validate()`](crate::BillingEngine::validate) and
    /// [`BillingEngine::bill()`](crate::BillingEngine::bill) to collect warnings
    /// before and during billing.
    ///
    /// Default implementation returns no warnings. Override in providers that must
    /// enforce regulatory preconditions (e.g. `DynamicElectricityProvider` enforces
    /// §41a iMSys requirement).
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
