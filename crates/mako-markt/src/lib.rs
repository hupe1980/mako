//! `mako-markt` — market data library for German energy market (`MaKo` / `marktd`).
//!
//! # Architecture
//!
//! ```text
//! crates/mako-markt/   ← record types, domain types, CloudEvents structs, makod HTTP client
//!                         NO axum, NO utoipa, NO server framework
//! services/marktd/     ← axum handlers, routes, config, PostgreSQL impls, main.rs
//! ```
//!
//! The storage itself is marktd's: each `Pg*` type in `services/marktd/src/pg/`
//! carries its methods inherently, over the record types defined here.
//!
//! # Modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`domain`] | `Sparte`, `ProcessStatus`; re-exports `MaloId`, `MeloId`, `MarktpartnerId` from [`rubo4e::identifiers`]; `nad_agency_code()` for NAD DE3055 coding authority |
//! | [`repository`] | The record types of every `marktd` aggregate, incl. `VersorgungsStatusRecord`, `LieferStatus`, `NbContractRecord`, **`MaloGridRecord`**, **`NbEnergiemixRecord`** |
//! | [`cloudevents`] | CloudEvents 1.0 envelope types: `InboundMakoEvent` (from `makod`) and `MarktEvent` (`marktd`-emitted `de.markt.*`) |
//! | [`commands`] | ERP command-name constants shared between `makod`'s registry and dispatching services (`processd`, …) |
//! | [`makod_client`] | Typed HTTP client for `makod` admin + command APIs (reqwest-backed); requires `makod-client` feature |
//! | [`error`] | `MdmError` — all domain-level error variants |

#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![expect(
    clippy::module_name_repetitions,
    reason = "a pedantic style lint this crate does not adopt"
)]
#![allow(
    clippy::missing_errors_doc,
    reason = "a pedantic style lint this crate does not adopt; it fires in some feature combinations of this crate and not in others"
)]

pub mod bo4e;
pub mod cloudevents;
pub mod commands;
pub mod domain;
pub mod error;
#[cfg(feature = "makod-client")]
pub mod makod_client;
#[cfg(feature = "marktd-client")]
pub mod marktd_client;
pub mod repository;
