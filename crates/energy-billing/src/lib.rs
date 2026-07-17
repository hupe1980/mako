//! Pure multi-product retail energy billing for German markets.
//!
//! ## Architecture
//!
//! This crate is the **commercial billing engine for the Lieferant (LF)**. It
//! answers: *"What does the customer's invoice look like?"*
//!
//! ```text
//! metering       — "What quantities are billable?"
//!     ↓
//! eeg-billing    — "What EEG remuneration applies?" (NB-side)
//!     ↓
//! energy-billing — "What does the customer's invoice look like?" (LF-side)
//!     ↓
//! accountingd    — Payments, Ledger, Dunning
//! ```
//!
//! ## Primary API — `Product::build_engine`
//!
//! ```rust
//! use energy_billing::{Product, BillingContext, InvoiceType, MeterInput, Quantities, RegulatoryRates, GridInput};
//! use rust_decimal_macros::dec;
//! use time::macros::date;
//!
//! let json = r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0,"grundpreis_ct_per_day":8.0}"#;
//! let product: Product = serde_json::from_str(json).unwrap();
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
//!     electricity: Some(MeterInput { arbeitsmenge_kwh: dec!(500), ..Default::default() }),
//!     ..Default::default()
//! };
//! let invoice = product.build_engine(&GridInput::default(), &RegulatoryRates::default())
//!     .bill(ctx, &quantities).unwrap();
//! assert!(invoice.brutto_eur > invoice.netto_eur);
//! ```
//!
//! ## Product categories
//!
//! | Category | Provider | Legal basis |
//! |---|---|---|
//! | `STROM` | `ElectricityProvider` | §41 EnWG |
//! | `WAERMEPUMPE` | `ControllableLoadProvider` (§14a) | §14a EnWG |
//! | `WALLBOX` | `ControllableLoadProvider` (§14a) | §14a EnWG |
//! | `GAS` | `GasProvider` | §41 EnWG |
//! | `WAERME` | `HeatProvider` | §41 EnWG |
//! | `SOLAR` | `SolarProvider` | §38a/§42a EEG 2023 |
//! | `EEG` | `EegProvider` (→ eeg-billing) | §§20–21 EEG 2023 |
//! | `EINSPEISUNG` | `EinspeisungProvider` | §20 EEG 2023 |
//! | `HEMS` | `HemsProvider` | — |
//! | `EMOBILITY` | `EmobilityProvider` | §41a EnWG |
//! | `ENERGIEDIENSTLEISTUNG` | `ServiceProvider` | — |
//! | `STROM` + `dynamic_epex=true` | `DynamicElectricityProvider` | §41a EnWG |
//! | `SHARING` | `ElectricityProvider` + `EnergyShareProvider` | §42c EnWG |

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

// Core billing types
pub use context::{
    AbschlagDeduction, BillingContext, CustomerKategorie, InvoiceType, Verbrauchshistorie,
};
pub use engine::BillingEngine;
pub use invoice::{Invoice, negate_rechnung_json_for_correction};
pub use position::{
    BillingPosition, BillingWarning, PositionCategory, PositionTrace, WarningSeverity,
};
pub use provider::{BillingProvider, EpexSpotSource, SpotPriceSource};
pub use quantities::{
    Abschlagsplan, AbschlagsplanEntry, DynamicInterval, EegMeterInput, EmobilityMeterInput,
    EnergyShareMeterInput, GasMeterInput, GgvNutzungsplan, GgvNutzungsplanEntry, GgvSolarInput,
    GridInput, HemsMeterInput, MeterInput, MeteringMode, ProsumerMeterInput, Quantities,
    Sect41aAnnualComparison, ServiceMeterInput, SolarMeterInput, WaermeMeterInput,
};
pub use rates::{
    BEHG_CO2_FACTOR_H_GAS, BEHG_CO2_FACTOR_L_GAS, RegulatoryRates, behg_ct_per_kwh_for_year,
    energiesteuer_gas_for_year, stromsteuer_for_year,
};

// Typed Product enum + per-category product structs
pub use tariff::{
    BlockTierInput, ControllableLoadProduct, EegProduct, EinspeisungProduct, ElectricityProduct,
    EmobilityProduct, EnergieQuellen, GasProduct, HeatProduct, HemsProduct, IndexedPriceConfig,
    Product, SeasonalPriceOverride, ServiceProduct, SharingProduct, SolarProduct,
    StromsteuerBefreiung,
};

// Concrete providers
pub use providers::{
    ControllableLoadProvider, DynamicElectricityProvider, EegProvider, EinspeisungProvider,
    ElectricityProvider, EmobilityProvider, EnergyShareProvider, GasProvider, HeatProvider,
    HemsProvider, MwStProvider, ServiceProvider, SolarProvider,
};

// Error type (re-export from billing crate)
pub use billing::BillingError;
