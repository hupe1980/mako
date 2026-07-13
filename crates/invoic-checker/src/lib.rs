//! Automated INVOIC plausibility and tariff validation for the LF (Lieferant) role.
//!
//! A German LF receives INVOIC messages (PIDs 31001–31011) from NB/GNB/MSB/BIKO
//! counterparties for grid fees (NNE), meter charges, and Mehr-/Mindermengen (MMM)
//! settlement. This library runs automated business-rule checks over BO4E
//! [`Rechnung`][rubo4e::current::Rechnung] objects — the industry-standard German
//! energy domain model — and produces a [`CheckReport`] that drives the REMADV /
//! dispute workflow in `invoicd`.
//!
//! ```text
//! EDIFACT INVOIC segments
//!   → [makod adapter: anti-corruption layer]
//!   → BO4E Rechnung            — industry-standard domain model, stored in events
//!   → InvoicCheckEngine::check — pure business rules, no EDIFACT dependency
//!   → CheckReport { Ok | Warn | Dispute }
//!       → REMADV auto-dispatch or dispute workflow
//! ```
//!
//! # Design principles
//!
//! - **Format-agnostic**: zero dependency on `edifact-rs`. Operates solely on
//!   the BO4E domain model. EDIFACT → BO4E translation belongs in the `makod`
//!   transport adapter (anti-corruption layer).
//! - **Pure library** — no I/O, no async, no Tokio dependency.
//! - **Trait-injected stores** — [`PreisblattStore`] is injected by the caller
//!   (e.g. `invoicd` injects an in-memory store seeded from `marktd`'s price-sheet API).
//! - **No floating-point money** — all amounts are [`EuroAmount`] (`i64` ×10⁻⁵ EUR).
//!
//! # Monetary precision
//!
//! [`EuroAmount`] stores values as `i64` in units of 10⁻⁵ EUR (1/100 000 EUR):
//! - `EuroAmount(100_000)` = 1.00000 EUR
//! - `EuroAmount(3_456)`   = 0.03456 EUR (typical NNE unit price per kWh)
//!
//! This gives five decimal places — sufficient for all BDEW INVOIC precision
//! requirements (NNE unit prices: typically 4 decimal places).
//!
//! # Example
//!
//! ```rust,no_run
//! use invoic_checker::{
//!     check::{CheckConfig, CheckOutcome, InvoicCheckEngine},
//!     tariff::InMemoryPreisblattStore,
//!     amount::EuroAmount,
//! };
//! use rubo4e::current::{PreisblattNetznutzung, Rechnung};
//!
//! let preisblatt_store = InMemoryPreisblattStore::default();
//!
//! let rechnung = Rechnung::default();
//! let report = InvoicCheckEngine::check(
//!     31001,
//!     "9900357000004",
//!     &rechnung,
//!     &preisblatt_store,
//!     &CheckConfig::default(),
//! );
//! assert_eq!(report.outcome, CheckOutcome::Ok);
//! ```
#![deny(unsafe_code)]

pub mod amount;
pub mod check;
pub mod error;
pub mod tariff;

// ── Convenient re-exports ─────────────────────────────────────────────────────

pub use amount::EuroAmount;
pub use check::{
    CheckConfig, CheckOutcome, CheckReport, Finding, FindingKind, InvoicCheckEngine, is_stornierung,
};
pub use error::CheckError;
pub use tariff::{InMemoryPreisblattStore, PreisblattStore};
