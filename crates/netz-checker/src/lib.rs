#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
// German regulatory terms (GPKE, GeLi, MaLo, MaStR, BK…) are not Rust items.
#![allow(clippy::doc_markdown)]
//! `netz-checker` — pure Anmeldung validation library for German energy market NB role.
//!
//! # Purpose
//!
//! Implements the **six deterministic NB checks** required by GPKE (BK6-22-024)
//! and GeLi Gas (BK7-24-01-009) for Anmeldung decisions:
//!
//! | # | Rule | Outcome on failure |
//! |---|------|-------------------|
//! | 1 | MaLo exists in NB grid | `Escalate` (data gap) |
//! | 2 | No conflicting active supply (`lf_mp_id_next` already set) | `Reject(A06)` |
//! | 3 | `process_date ≥ today` (no retroactive start) | `Reject(A97)` |
//! | 4 | Network area consistent (Bilanzierungsgebiet matches) | `Reject(A02)` |
//! | 5 | LF registered in partner directory | `Reject(A05)` |
//! | 6 | Mindestvorlauffrist met (Strom SLP: 15:00 cutoff; RLM: per AHB) | `Reject(A99)` |
//!
//! Checks are evaluated in order; the first failing check short-circuits the
//! rest.  A `NetzCheckResult::Accept` means **all** applicable rules passed.
//!
//! # Design constraints
//!
//! - **No I/O** — all inputs are passed as function arguments.
//! - **No clock** — the current instant is passed as `now` so that callers
//!   control time (testability, replay safety).
//! - **Deterministic** — the same inputs always produce the same output.
//! - **No async** — this crate is intentionally synchronous.
//!
//! # Usage
//!
//! ```rust,no_run
//! use netz_checker::{AnmeldungAnfrage, MaloGridRecord, evaluate};
//! use mako_markt::repository::{VersorgungsStatusRecord, LieferStatus};
//!
//! // Build inputs (normally obtained from marktd REST calls in processd)
//! let anfrage = AnmeldungAnfrage {
//!     pid:              55001,
//!     process_id:       uuid::Uuid::new_v4(),
//!     malo_id:          "51238696780".to_owned(),
//!     new_supplier_gln: "9900357000004".to_owned(),
//!     grid_operator_gln: "9900000000002".to_owned(),
//!     bilanzierungsgebiet: Some("11YB-TENNET-----W".to_owned()),
//!     process_date:     time::Date::from_calendar_date(2026, time::Month::August, 1).unwrap(),
//!     sparte:           mako_markt::domain::Sparte::Strom,
//!     messtyp:          netz_checker::Messtyp::Slp,
//! };
//! ```

pub mod checks;
pub mod error;
pub mod types;

pub use checks::evaluate;
pub use types::{AnmeldungAnfrage, MaloGridRecord, Messtyp, NetzCheckResult, RejectReason};
