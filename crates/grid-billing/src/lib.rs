//! Role-neutral **grid invoice calculation** for German energy market billing.
//!
//! Covers all DSO/TSO-side INVOIC documents:
//! - **NNE** (Netznutzungsentgelt) — PIDs 31001, 31005, 31011
//! - **MMM** (Mehr-/Mindermengensaldo) — PID 31002
//! - **MSB** (Messstellenbetrieb) — PID 31009
//! - **Selbst ausgestellte Rechnung** — PID 31006 (LF acting as NB)
//!
//! Used by both the **NB** (`netzbilanzd`) to generate invoices and the **LF**
//! (`invoicd`) to self-issue invoices under §20 MessZV.  The formula is identical
//! for both roles — only who initiates differs.
//!
//! Generates [`GridInvoice`] domain objects from meter readings and
//! tariff data.  The service layer (`netzbilanzd`, `invoicd`) converts
//! `GridInvoice` to `rubo4e::current::Rechnung` via a local `into_rechnung()`
//! helper — keeping BO4E as a service-layer concern.  The generated invoices are:
//! - **PID 31001** — `MMM-Rechnung NNE Strom` (NB → LF, monthly network usage)
//! - **PID 31002** — `MMM-Stornorechnung NNE Strom` (NB → LF, correction/reversal)
//! - **PID 31005** — `MMM-Rechnung NNE Gas` (NB → LF, monthly gas NNE)
//! - **PID 31006** — `MMM-selbst ausgestellte Rechnung` (LF selbstausstellt, same formula)
//!
//! # Design
//!
//! - **Pure library** — zero I/O, zero async.  All calculations are deterministic.
//! - **No floating-point money** — uses `EuroAmount` (`i64 × 10⁻⁵ EUR`) for all
//!   monetary arithmetic to avoid rounding errors.
//! - **Self-validating** — all generated invoices satisfy `invoic-checker` checks 1–3
//!   (period validity, position arithmetic, document total) by construction.
//!   Check 4–5 (tariff deviation) depends on the `PreisblattStore` supplied by the caller.
//! - **BO4E-free output** — returns [`GridInvoice`] (pure domain type).  The service
//!   layer (netzbilanzd, invoicd) owns the `rubo4e::current::Rechnung` conversion via a
//!   local `into_rechnung()` helper.  This keeps grid-billing publishable to crates.io
//!   without pulling in the internal `rubo4e` crate.
//!
//! # Example
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
//! }).expect("valid billing input");
//! ```
#![deny(unsafe_code)]

pub mod billing;
pub mod error;
pub mod types;

pub use billing::{calculate_mmm_invoice, calculate_msb_invoice, calculate_nne_invoice};
pub use error::BillingError;
pub use types::{GridInvoice, InvoicePosition, MmmInput, MsbInput, NneInput, QuantityUnit};
