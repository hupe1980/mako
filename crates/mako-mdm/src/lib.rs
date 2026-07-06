//! `mako-mdm` — master data service library for German energy market (`MaKo`).
//!
//! # Architecture
//!
//! ```text
//! crates/mako-mdm/      ← traits, domain types, CloudEvents structs, makod HTTP client
//!                           NO axum, NO utoipa, NO server framework
//! services/mdmd/        ← axum handlers, routes, config, PostgreSQL impls, main.rs
//! ```
//!
//! This mirrors the `mako-engine` / `makod` split exactly:
//! `mako-engine` defines traits only; `makod` wires axum + `SlateDB`.
#![allow(clippy::doc_markdown)]
//!
//! # Modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`domain`] | `Sparte`, `ProcessStatus`; re-exports `MaloId`, `MeloId`, `MarktpartnerId`/`Gln` from [`rubo4e::identifiers`] |
//! | [`repository`] | Repository traits (`MaloRepository`, `MeloRepository`, `ContractRepository`, `SubscriptionRepository`, `CorrelationIndex`, `PartnerRepository`) and aggregate record types |
//! | [`cloudevents`] | CloudEvents 1.0 envelope types: `InboundMakoEvent` (from `makod`) and `MdmEvent` (MDM-emitted `de.mdm.*`) |
//! | [`makod_client`] | Typed HTTP client for `makod` admin + command APIs (reqwest-backed); requires `makod-client` feature |
//! | [`error`] | `MdmError` — all domain-level error variants |
//! | [`testing`] | In-memory test doubles (behind `testing` feature) |

#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]

pub mod cloudevents;
pub mod domain;
pub mod error;
#[cfg(feature = "makod-client")]
pub mod makod_client;
pub mod repository;

#[cfg(feature = "testing")]
pub mod testing;
