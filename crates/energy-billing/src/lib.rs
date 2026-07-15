//! Pure multi-product retail energy billing for German markets.
//!
//! ## Architecture
//!
//! This crate is the **commercial billing engine for the Lieferant (LF)**. It
//! answers the question: *"What does the customer's invoice look like?"*
//!
//! The three-layer billing stack:
//!
//! ```text
//! metering          — "What quantities are billable?"
//!     ↓
//! eeg-billing       — "What remuneration or statutory credits apply?" (NB-side)
//!     ↓
//! energy-billing    — "What does the customer's invoice look like?" (LF-side)
//!     ↓
//! accountingd       — Payments, Ledger, Dunning
//! ```
//!
//! ## Primary API
//!
//! Build a [`BillingEngine`] from one or more [`BillingProvider`]s, supply a
//! [`BillingContext`] and [`Quantities`], and receive an [`Invoice`]:
//!
//! ```rust
//! use energy_billing::*;
//! use rust_decimal_macros::dec;
//! use time::macros::date;
//!
//! let ctx = BillingContext {
//!     malo_id:         "51238696781".to_owned(),
//!     lf_mp_id:        "9900000000001".to_owned(),
//!     rechnungsnummer: "R2026-001".to_owned(),
//!     period_from:      date!(2026-01-01),
//!     period_to:        date!(2026-01-31),
//!     invoice_type:     InvoiceType::Initial,
//!     contract_id:      None,
//!     regulatory_rates: RegulatoryRates::default(),
//!     ..Default::default()
//! };
//! let quantities = Quantities {
//!     electricity: Some(MeterInput {
//!         arbeitsmenge_kwh: dec!(500),
//!         ..Default::default()
//!     }),
//!     ..Default::default()
//! };
//! let tariff: TariffInput =
//!     serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#).unwrap();
//! let invoice = BillingEngine::new()
//!     .add(ElectricityProvider::from_tariff(&tariff, &GridInput::default()))
//!     .add(MwStProvider::new(dec!(0.19)))
//!     .bill(ctx, &quantities)
//!     .unwrap();
//! assert!(invoice.brutto_eur > invoice.netto_eur);
//! ```
//!
//! ## Product categories
//!
//! | Category | Provider | Legal basis |
//! |---|---|---|
//! | `STROM` | [`ElectricityProvider`] | §41 EnWG |
//! | `WAERMEPUMPE` | [`ElectricityProvider`] + §14a | §14a EnWG |
//! | `WALLBOX` | [`ElectricityProvider`] + §14a | §14a EnWG |
//! | `GAS` | [`GasProvider`] | §41 EnWG |
//! | `WAERME` | [`HeatProvider`] | §41 EnWG |
//! | `SOLAR` | [`SolarProvider`] | §38a/§42a EEG 2023 |
//! | `EEG` | [`EegProvider`] (→ eeg-billing) | §§20–21 EEG 2023 |
//! | `EINSPEISUNG` | [`EinspeisungProvider`] | §20 EEG 2023 |
//! | `HEMS` | [`HemsProvider`] | — |
//! | `EMOBILITY` | [`EmobilityProvider`] | §41a EnWG |
//! | `ENERGIEDIENSTLEISTUNG` | [`ServiceProvider`] | — |
//! | `STROM` + `dynamic_epex` | [`DynamicElectricityProvider`] | §41a EnWG |
//!
//! ## Shortcut: `TariffInput::build_engine`
//!
//! For `billingd`'s dispatch path:
//! ```rust,ignore
//! let invoice = tariff
//!     .build_engine(&grid, &rates)
//!     .ok_or("unsupported category")?
//!     .bill(ctx, &quantities)?;
//! ```

#![deny(unsafe_code)]

// ── Modules ───────────────────────────────────────────────────────────────────

pub mod context;
pub mod engine;
pub mod invoice;
pub mod position;
pub mod provider;
pub mod providers;
pub mod quantities;
pub mod rates;
pub mod tariff;

// ── Primary API re-exports ────────────────────────────────────────────────────

// Core types
pub use context::{AbschlagDeduction, BillingContext, InvoiceType, Verbrauchshistorie};
pub use engine::BillingEngine;
pub use invoice::{Invoice, negate_rechnung_json_for_correction};
pub use position::{BillingPosition, PositionCategory};
pub use provider::{BillingProvider, EpexSpotSource, SpotPriceSource};
pub use quantities::{
    Abschlagsplan, AbschlagsplanEntry, DynamicInterval, EegMeterInput, EmobilityMeterInput,
    GasMeterInput, GgvNutzungsplan, GgvNutzungsplanEntry, GgvSolarInput, GridInput, HemsMeterInput,
    MeterInput, MeteringMode, ProsumerMeterInput, Quantities, Sect41aAnnualComparison,
    ServiceMeterInput, SolarMeterInput, WaermeMeterInput,
};
pub use rates::{
    RegulatoryRates, behg_ct_per_kwh_for_year, energiesteuer_gas_for_year, stromsteuer_for_year,
};
pub use tariff::{
    BlockTierInput, IndexedPriceConfig, PricingModel, SeasonalPriceOverride, TariffInput,
};

// Concrete providers
pub use providers::{
    DynamicElectricityProvider, EegProvider, EinspeisungProvider, ElectricityProvider,
    EmobilityProvider, GasProvider, HeatProvider, HemsProvider, MwStProvider, ServiceProvider,
    SolarProvider,
};

// Error type (re-export from billing crate)
pub use billing::BillingError;
