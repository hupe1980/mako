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
//! Input → validation → Settlement Engine → SettlementResult → into_rechnung() (service layer)
//! ```
//!
//! [`SettlementResult`] is the canonical output. The service layer (`netzbilanzd`,
//! `invoicd`) converts it to BO4E `Rechnung`. This keeps `grid-billing`
//! publishable to crates.io without pulling in the internal `rubo4e` crate.
//!
//! ## Explainability
//!
//! Every [`SettlementPosition`] carries a [`CalculationTrace`] with:
//! - input values (quantity, unit price)
//! - gross intermediate result before rounding
//! - applicable [`LegalReference`]s (e.g. `StromNEV §17`, `KAV §2`)
//! - the [`TariffSource`] justifying each rate
//!
//! ## No float money
//!
//! Quantities, rates, and factors are `rust_decimal::Decimal`; every EUR
//! result is range-checked through the `billing::EuroAmount` newtype
//! (`Amount<5>`) before it leaves the crate.
//!
//! ## Example
//!
//! ```rust,no_run
//! use grid_billing::{
//!     ArbeitspreisModell, InvoiceDocument, KaKundengruppe, Konzessionsabgabe, MengePreis,
//!     NneInput, SettlementPeriod, settle_nne,
//! };
//! use rust_decimal::Decimal;
//! use time::macros::date;
//!
//! fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }
//!
//! // The engine is given what was supplied and at what rates — no invoice
//! // number, no issue date, no Prüfidentifikator.
//! let settlement = settle_nne(&NneInput {
//!     malo_id: "51238696780".into(),
//!     nb_mp_id: "9900357000004".into(),
//!     lf_mp_id: "9900012345678".into(),
//!     period: SettlementPeriod::new(date!(2025-01-01), date!(2025-01-31))?,
//!     // One value, not twelve loose fields: flat rate, §14a Modul 1, Modul 2
//!     // HT/NT and Modul 3 are mutually exclusive by construction.
//!     arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
//!         menge_kwh: d("1500"),
//!         preis_ct_per_kwh: d("3.5"),
//!     }),
//!     leistungspreis: None,
//!     letztverbrauchergruppe: Default::default(),
//!     sect19_umlage_ct_per_kwh: None,
//!     offshore_umlage_ct_per_kwh: None,
//!     kwkg_umlage_ct_per_kwh: None,
//!     grundpreis: None,
//!     // Recorded so an auditor can check the rate came from the right sheet.
//!     netzebene: Some(grid_billing::netzebene::Netzebene::Niederspannung),
//!     sect19: None,
//!     gas_kapazitaet: None,
//!     jahreshoechstleistung_kw: None,
//!     jahresarbeit_kwh: Some(d("18000")),
//!     // Rate and customer group travel together, so the KAV §2 Höchstbetrag is
//!     // always checked.
//!     konzessionsabgabe: Some(Konzessionsabgabe {
//!         satz_ct_per_kwh: d("0.11"),
//!         klasse: KaKundengruppe::Sondervertragskunde,
//!     }),
//!     tariff_sheet_id: Some("Preisblatt-NNE-2025-Q1".into()),
//!     sparte: grid_billing::Sparte::Strom,
//! })?;
//!
//! // Every position explains itself:
//! for pos in &settlement.positions {
//!     println!("{}: {}", pos.text, pos.trace.explanation);
//! }
//! for r in settlement.all_legal_refs() {
//!     println!("  → {r}");
//! }
//!
//! // Presenting it as an invoice is a separate step, and the only place
//! // document identity enters.
//! let document = InvoiceDocument {
//!     settlement,
//!     pid: 31001,
//!     rechnungsnummer: "NNE-2025-001".into(),
//!     correction_of: None,
//!     invoice_date: date!(2025-02-15),
//!     due_date: date!(2025-03-15),
//! };
//! for (number, pos) in document.numbered_positions() {
//!     println!("{number}. {}", pos.text);
//! }
//! # Ok::<(), grid_billing::BillingError>(())
//! ```
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod billing;
pub mod error;
pub mod gas;
pub mod msbg;
pub mod netzebene;
pub mod redispatch;
pub mod regulatory;
pub mod sect18;
pub mod sect19;
pub mod types;
pub mod umlagen;

pub use billing::{correct, reverse, settle_gas_awh, settle_mmm, settle_msb, settle_nne};
pub use error::BillingError;
pub use redispatch::{
    RedispatchVerguetung, RedispatchVerguetungInput, RedispatchVerguetungsart,
    eeg_entgangene_einnahmen, redispatch_verguetung,
};
pub use types::{
    // The settlement — what is owed and why.
    ArbeitspreisModell,
    // AWH positions
    AwhPositionInput,
    // BDEW Artikelnummer bridge — maps SettlementPosition.kind → BdewArtikelnummer in service layer
    BillingPositionKind,
    // Domain types for explainability + audit
    CalculationTrace,
    // Input types
    GasAwhInput,
    GemeindeGroesse,
    Grundpreis,
    // Presenting a settlement as an invoice: numbers, dates, Prüfidentifikator.
    InvoiceDocument,
    KaKundengruppe,
    Konzessionsabgabe,
    LegalReference,
    Leistungspreis,
    MengePreis,
    MmmInput,
    MsbInput,
    NneInput,
    // The pricing formula behind a rate, as a value rather than a document.
    PriceReference,
    PriceStep,
    QuantityUnit,
    Reduktionsfaktor,
    Sect14aModul3Interval,
    // §14a module type (replaces module: u8 for type safety)
    Sect14aModule,
    SettlementPeriod,
    SettlementPosition,
    SettlementResult,
    SettlementStatus,
    SettlementType,
    SettlementWarning,
    Sparte,
    SpotPriceFormula,
    TariffCalculationMethod,
    TariffSource,
    ValidationResult,
    WarningSeverity,
    // Validation functions
    validate_gas_awh_input,
    validate_mmm_input,
    validate_msb_input,
};
