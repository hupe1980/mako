//! `mako-markt` — market data library for German energy market (`MaKo` / `marktd`).
//!
//! # Architecture
//!
//! ```text
//! crates/mako-markt/   ← traits, domain types, CloudEvents structs, makod HTTP client
//!                         NO axum, NO utoipa, NO server framework
//! services/marktd/     ← axum handlers, routes, config, PostgreSQL impls, main.rs
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
//! | [`domain`] | `Sparte`, `ProcessStatus`; re-exports `MaloId`, `MeloId`, `MarktpartnerId` from [`rubo4e::identifiers`]; `nad_agency_code()` for NAD DE3055 coding authority |
//! | [`repository`] | Repository traits: `MaloRepository`, `MeloRepository`, `ContractRepository`, `SubscriptionRepository`, `CorrelationIndex`, `PartnerRepository`, `VersorgungsStatusRepository`, **`MaloGridRepository`**; record types incl. `VersorgungsStatusRecord`, `LieferStatus`, `NbContractRecord`, **`MaloGridRecord`** |
//! | [`cloudevents`] | CloudEvents 1.0 envelope types: `InboundMakoEvent` (from `makod`) and `MarktEvent` (`marktd`-emitted `de.markt.*`) |
//! | [`makod_client`] | Typed HTTP client for `makod` admin + command APIs (reqwest-backed); requires `makod-client` feature |
//! | [`error`] | `MdmError` — all domain-level error variants |
//! | [`testing`] | In-memory test doubles (behind `testing` feature): incl. `InMemoryVersorgungsStatusRepository`, `InMemoryMaloGridRepository` |

#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]

pub mod cloudevents;
pub mod domain;
pub mod error;
#[cfg(feature = "makod-client")]
pub mod makod_client;
#[cfg(feature = "marktd-client")]
pub mod marktd_client;
pub mod repository;

#[cfg(feature = "testing")]
pub mod testing;
