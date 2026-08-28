//! Automated INVOIC plausibility and tariff validation, over BO4E.
//!
//! An invoice recipient — an LF, an NB or an ESA — receives INVOIC messages
//! (PIDs 31001–31011) from NB/GNB/MSB/BIKO counterparties for grid fees (NNE),
//! meter charges, Leistungen des Preisblatts B and Mehr-/Mindermengen (MMM)
//! settlement. This library runs automated business-rule checks over BO4E
//! [`Rechnung`][rubo4e::current::Rechnung] objects — the industry-standard German
//! energy domain model — and produces a [`CheckReport`] that drives the REMADV /
//! dispute workflow in `invoicd`.
//!
//! # Boundary with `mako-pruefung`
//!
//! `mako-pruefung` decides **published BDEW Antwortcodes** for the wire and
//! knows nothing but Prüfschritte. This crate decides mako's own [`Finding`]s
//! for the operator queue and the § 147 AO receipt, over BO4E, for **every**
//! INVOIC PID — including those with no Entscheidungsbaum. The dependency runs
//! one way: this crate maps BO4E onto [`mako_pruefung::rechnung`]'s Prüfschritte
//! and calls the walk, which holds no BO4E or money type. See the README for
//! why they are not one crate.
//!
//! Where a plausibility check asks the same question as a Prüfschritt —
//! position arithmetic against `A20`, the document total against `A24`, the tax
//! breakdown against `A22`/`A23` — **both paths read the same tolerance**:
//! Summen-level `total_tolerance_ppm`, position-level
//! `arithmetic_tolerance_ppm`. Two knobs for one question would let the engine
//! record a `TotalMismatch` Dispute while the walk dispatched a Zahlungsavis.
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
//! // A `Rechnung` that states no Umsatzsteuer is disputed: §14 Abs. 4 Nr. 8
//! // UStG makes the rate and the amount mandatory, and without them the
//! // recipient has no Vorsteuerabzug.
//! let rechnung = Rechnung::default();
//! let report = InvoicCheckEngine::check(
//!     31001,
//!     "9900357000004",
//!     &rechnung,
//!     &preisblatt_store,
//!     &CheckConfig::default(),
//! );
//! assert_eq!(report.outcome, CheckOutcome::Dispute);
//! ```
#![deny(unsafe_code)]

pub mod amount;
pub mod check;
pub mod error;
pub mod rechnung;
pub mod tariff;

// ── Convenient re-exports ─────────────────────────────────────────────────────

pub use amount::EuroAmount;
pub use check::{
    CheckConfig, CheckOutcome, CheckReport, Finding, FindingKind, InvoicCheckEngine, is_stornierung,
};
pub use error::CheckError;
pub use rechnung::{
    EmpfaengerFakten, StornoEmpfaengerFakten, antwort_auf_erneute_rechnung, antwort_auf_rechnung,
    antwort_auf_stornorechnung,
};
pub use tariff::{InMemoryPreisblattStore, PreisblattStore};
