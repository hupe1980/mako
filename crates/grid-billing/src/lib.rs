//! Role-neutral **German grid settlement calculation** engine.
//!
//! Covers all DSO/TSO-side INVOIC documents:
//! - **NNE** (Netznutzungsentgelt) — PIDs 31001, 31005, 31006, 31011
//! - **MMM** (Mehr-/Mindermengensaldo) — PID 31002
//! - **MSB** (Messstellenbetrieb) — PID 31009
//!
//! ## Calculation flow
//!
//! ```text
//! Input → validation → Settlement Engine → GridSettlement → into_rechnung() (service layer)
//! ```
//!
//! [`GridSettlement`] is the canonical output. The service layer (`netzbilanzd`,
//! `invoicd`) converts it to BO4E `Rechnung`. This keeps `grid-billing`
//! publishable to crates.io without pulling in the internal `rubo4e` crate.
//!
//! ## Explainability
//!
//! Every [`InvoicePosition`] carries a [`CalculationTrace`] with:
//! - input values (quantity, unit price)
//! - gross intermediate result before rounding
//! - applicable [`LegalReference`]s (e.g. `StromNEV §17`, `KAV §2`)
//! - the [`TariffSource`] justifying each rate
//!
//! ## No float money
//!
//! All amounts use `rust_decimal::Decimal` and the `billing::EuroAmount` newtype.
//!
//! ## Example
//!
//! ```rust,no_run
//! use grid_billing::{NneInput, calculate_nne_invoice};
//! use rust_decimal::Decimal;
//! use time::macros::date;
//!
//! fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }
//!
//! let result = calculate_nne_invoice(&NneInput {
//!     malo_id: "51238696780".into(),
//!     nb_mp_id: "9900357000004".into(),
//!     lf_mp_id: "9900012345678".into(),
//!     rechnungsnummer: "NNE-2025-001".into(),
//!     period_from: date!(2025-01-01),
//!     period_to:   date!(2025-01-31),
//!     invoice_date: date!(2025-02-15),
//!     due_date: date!(2025-03-15),
//!     arbeitsmenge_kwh: d("1500"),
//!     arbeitspreis_ct_per_kwh: d("3.5"),
//!     arbeitsmenge_ht_kwh: None,
//!     arbeitspreis_ht_ct_per_kwh: None,
//!     arbeitsmenge_nt_kwh: None,
//!     arbeitspreis_nt_ct_per_kwh: None,
//!     spitzenleistung_kw: None,
//!     leistungspreis_eur_per_kw: None,
//!     ka_satz_ct_per_kwh: Some(d("0.11")),
//!     sect14a_modul1_reduction_factor: None,
//!     nne_grundpreis_eur_per_month: None,
//!     nne_grundpreis_months: None,
//!     tariff_sheet_id: Some("Preisblatt-NNE-2025-Q1".into()),
//!     sparte: grid_billing::Sparte::Strom,
//!     ka_klasse: Some(grid_billing::KaKlasse::TarifkundeLow),
//!     sect14a_modul3_intervals: vec![],
//! }).expect("valid billing input");
//!
//! // Every position explains itself:
//! for pos in &result.positions {
//!     println!("{}: {}", pos.text, pos.trace.explanation);
//! }
//!
//! // Legal references used:
//! for r in result.all_legal_refs() {
//!     println!("  → {r}");
//! }
//! ```
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod billing;
pub mod error;
pub mod types;

pub use billing::{
    calculate_correction, calculate_gas_awh_invoice, calculate_mmm_invoice, calculate_msb_invoice,
    calculate_nne_invoice, calculate_reversal,
};
pub use error::BillingError;
pub use types::{
    // AWH positions
    AwhPositionInput,
    // BDEW Artikelnummer bridge — maps InvoicePosition.kind → BdewArtikelnummer in service layer
    BillingPositionKind,
    // Domain types for explainability + audit
    CalculationTrace,
    // Input types
    GasAwhInput,
    // Backward-compatible alias for GridSettlement — same type, kept for call-site stability
    GridInvoice,
    // Core settlement output
    GridSettlement,
    InvoicePosition,
    KaKlasse,
    LegalReference,
    MmmInput,
    MsbInput,
    NneInput,
    QuantityUnit,
    // §14a module type (replaces module: u8 for type safety)
    Sect14aModule,
    SettlementStatus,
    SettlementType,
    SettlementWarning,
    Sparte,
    TariffSource,
    ValidationResult,
    WarningSeverity,
    // Validation functions
    validate_gas_awh_input,
    validate_mmm_input,
    validate_msb_input,
    validate_nne_input,
};
