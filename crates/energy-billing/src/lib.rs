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
//! use energy_billing::{BillingContext, BillingPeriod, GridInput, InvoiceType, MeterInput, Product, Quantities, RegulatoryRates};
//! use rust_decimal::dec;
//! use time::macros::date;
//!
//! let json = r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"30.0","grundpreis_ct_per_day":"8.0"}"#;
//! let product: Product = serde_json::from_str(json).unwrap();
//! let ctx = BillingContext {
//!     malo_id:         "51238696012".to_owned(),
//!     lf_mp_id:        "9900000000001".to_owned(),
//!     rechnungsnummer: "R2026-001".to_owned(),
//!     period: BillingPeriod::new(date!(2026-01-01), date!(2026-01-31)).unwrap(),
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
//! | `WAERME` | `HeatProvider` | §41 EnWG; AVBFernwärmeV §24; CO2KostAufG §3; §14 WPG |
//! | `WASSER` | `WaterProvider` | AVBWasserV; §12 Abs. 2 Nr. 1 UStG (7 %); gesplittete Abwassergebühr |
//! | `SOLAR` | `SolarProvider` | §42a Abs. 4 EnWG (Mieterstrom-Preisdeckel) / §42b EnWG (GGV) |
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
/// EN 16931 semantic-model bridge (`Invoice::to_en16931`), behind `en16931`.
#[cfg(feature = "en16931")]
pub mod en16931_map;
pub mod engine;
pub mod error;
pub mod invoice;
pub mod position;
pub mod provider;
pub mod providers;
pub mod quantities;
pub mod rates;
/// Verbrauchsteuerliche Begünstigungen — Befreiung, Ermäßigung, Entlastung.
///
/// Only the first two change what a supplier invoices; the third is the
/// customer's own claim at the Hauptzollamt. Keeping them apart is what stops a
/// § 9b StromStG relief from being billed as a § 9 Abs. 1 exemption.
pub mod steuer;
pub mod tariff;

// ── Primary API re-exports ────────────────────────────────────────────────────

// Core billing types
pub use context::{
    AbschlagDeduction, BillingContext, BillingPeriod, CustomerKategorie, InvoiceType,
    SettlementForm, Verbraucherinformationen, Verbrauchshistorie, Vertragsart,
    Vertragsinformationen,
};
pub use engine::BillingEngine;
pub use error::EngineError;
pub use invoice::{
    Invoice, TaxSubtotal, VatCategory, negate_rechnung_json_for_correction, tax_subtotals_of,
};
pub use position::{
    BillingPosition, BillingWarning, PositionCategory, PositionTrace, WarningSeverity,
};
pub use provider::{BillingProvider, MTU_MINUTES, mtu_start};
pub use quantities::{
    Ablesungsart, Abschlagsplan, AbschlagsplanEntry, Absetzung, AbsetzungsGrund, DynamicInterval,
    EegMeterInput, EmobilityMeterInput, EnergyShareMeterInput, GasMeterInput, GgvNutzungsplan,
    GgvNutzungsplanEntry, GgvSolarInput, GridInput, HemsMeterInput, MeterInput, MeteringMode,
    ProsumerMeterInput, Quantities, Sect14aModul3Verbrauch, Sect41aAnnualComparison,
    ServiceMeterInput, SolarMeterInput, WaermeMeterInput, WasserMeterInput,
};
pub use rates::{
    BEHG_CO2_FACTOR_H_GAS, BEHG_CO2_FACTOR_L_GAS, RegulatoryRates, RoundMoney,
    behg_ct_per_kwh_for_year, behg_ct_per_kwh_from_price, energiesteuer_gas_for_year,
    mwst_rate_for_gas_waerme_period, mwst_rate_for_period, round_money,
    steuer_stichtage_im_zeitraum, stromsteuer_for_year,
};
pub use steuer::{
    EnergiesteuerBefreiung, EnergiesteuerTarif, Steuerentlastung, StromsteuerBefreiung,
    StromsteuerErmaessigung, StromsteuerTarif,
};

// Typed Product enum + per-category product structs
pub use tariff::{
    AbwasserRegime, BlockTierInput, ControllableLoadProduct, EegProduct, EinspeisungProduct,
    ElectricityProduct, EmobilityProduct, EnergieQuellen, GasProduct, HeatProduct, HemsProduct,
    IndexedPriceConfig, Product, SeasonalPriceOverride, ServiceProduct, SharingProduct,
    SolarProduct, WaterProduct,
};

// Concrete providers
pub use providers::{
    ControllableLoadProvider, DynamicElectricityProvider, EegProvider, EinspeisungProvider,
    ElectricityProvider, EmobilityProvider, EnergyShareProvider, GasProvider, HeatProvider,
    HemsProvider, MwStProvider, ServiceProvider, SolarProvider, WaterProvider,
};

// The arithmetic core — `Amount<P>` fixed-point money, the canonical
// `RoundingStrategy` (kaufmännisch by convention in this workspace), and the
// error reachable through [`EngineError::Arithmetic`]. `round_money` /
// `RoundMoney` delegate their mode to this crate; use `Amount` directly
// where the precision is statutory (cents, 10⁻⁵-EUR unit prices).
pub use billing::{Amount, BillingError, RoundingStrategy};

/// A monetary amount in euro at 10⁻⁵-EUR resolution.
///
/// `billing` 0.12 dropped its own `EuroAmount` alias — the engine is
/// currency-agnostic and the name asserted a currency the type does not carry.
/// German retail energy billing *is* euro-denominated, so the alias is correct
/// here; it just belongs to the domain crate rather than the engine.
pub type EuroAmount = Amount<5>;
