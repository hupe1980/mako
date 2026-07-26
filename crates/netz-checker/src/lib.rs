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
//! Implements the **six deterministic NB checks** required by GPKE
//! (BK6-24-174, EBD E_0622) and GeLi Gas (AWH GeLi Gas 2.0, codeliste
//! G_0011) for Anmeldung decisions:
//!
//! | # | Rule | Outcome on failure |
//! |---|------|-------------------|
//! | 1 | MaLo exists in NB grid | `Escalate` (data gap) |
//! | 2 | MaLo participates in MaKo (not Stillgelegt/Ruhend) | `Reject(A02)` |
//! | 3 | No conflicting Anmeldung in Bearbeitung | `Reject(A06)` |
//! | 4 | Date plausibility, Transaktionsgrund-aware (Strom: LFW24 future rule; Gas: 6-week retro window for E01/E02 SLP, 10 WT for E03) | `Reject(A07)` Strom / `Reject(E17)` Gas / `Escalate` |
//! | 5 | Bilanzierungsgebiet consistent | `Reject(A05)` |
//! | 6 | LF registered in partner directory | `Reject(A05)` |
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
//!     // SG4 STS Transaktionsgrund — E01 Ein-/Auszug, E03 Wechsel, …
//!     transaktionsgrund: Some("E03".to_owned()),
//!     // true when a ZW3 „Erzeugende Marktlokation" ergänzung is present (EEG/KWKG).
//!     ist_erzeugende_marktlokation: false,
//! };
//! ```

pub mod checks;
pub mod config;
pub mod error;
pub mod types;

pub use checks::evaluate;
pub use config::NetzCheckConfig;
pub use mako_engine::fristen::HolidayCalendar;
pub use types::{AnmeldungAnfrage, MaloGridRecord, Messtyp, NetzCheckResult, RejectReason};
