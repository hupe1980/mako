//! `dvgw-edi` — DVGW EDIFACT format parser for the German gas transport and
//! balancing market.
//!
//! This crate implements parsing of **DVGW-governed EDIFACT formats** used in
//! gas network balancing (`GaBi Gas 2.0`, `BNetzA` BK7-14-020). It is the DVGW
//! counterpart to the [`edi-energy`] crate which covers BDEW EDI@Energy.
//!
//! # Supported formats
//!
//! | Message | Version | Valid from | UN/EDIFACT base | Description |
//! |---|---|---|---|---|
//! | `ALOCAT` | 5.11a | 2024-10-01 | D03A | Gas quantity allocation (Allokationsnachricht) |
//! | `NOMINT` | 4.6 FK | 2026-02-01 | D01B | Nomination integration (Nominierungsintegration) |
//! | `NOMRES` | 4.7 FK | 2026-02-01 | D01B | Nomination response (Nominierungsantwort) |
//!
//! # Quick start
//!
//! ```rust,no_run
//! use dvgw_edi::{DvgwPlatform, AnyDvgwMessage, DvgwMessage};
//!
//! # let input: &[u8] = b"";
//! let msg = DvgwPlatform::default().parse(input)?;
//!
//! if let AnyDvgwMessage::Nomint(nomint) = msg {
//!     println!("nomination ref: {:?}", nomint.nomination_ref);
//!     println!("sender EIC: {:?}", nomint.sender_eic());
//!     for qty in &nomint.quantities {
//!         println!("  location={} qty={} {}", qty.location_code, qty.quantity,
//!                  qty.unit.as_deref().unwrap_or("?"));
//!     }
//! }
//! # Ok::<(), dvgw_edi::Error>(())
//! ```
//!
//! # Market roles
//!
//! | Role | Abbreviation | Description |
//! |---|---|---|
//! | Fernleitungsnetzbetreiber | FNB | Gas transmission system operator |
//! | Verteilnetzbetreiber | VNB | Gas distribution system operator |
//! | Bilanzkreisverantwortlicher | BKV | Balance responsible party |
//! | Marktgebietsverantwortlicher | MGV | Market area manager |
//!
//! # Message routing
//!
//! DVGW messages do not use a BGM Prüfidentifikator. Routing uses the
//! combination of message type and direction qualifier from the NAD+MS/MR
//! role codes. Use [`AnyDvgwMessage::detect_pid`] to obtain the synthetic PID
//! for registration with the `mako-engine` PID router:
//!
//! ```rust,no_run
//! use dvgw_edi::DvgwPlatform;
//!
//! # let input: &[u8] = b"";
//! let msg = DvgwPlatform::default().parse(input)?;
//! // BKV → FNB nomination → synthetic PID 90011
//! let pid = msg.detect_pid(Some("Z01"));
//! # Ok::<(), dvgw_edi::Error>(())
//! ```
//!
//! Use [`AnyDvgwMessage::as_trait`] to access the [`message::DvgwMessage`]
//! trait methods for the sender/receiver EIC codes.
//!
//! # Regulatory references
//!
//! - **`GasNZV`** — statutory basis for gas network access and balancing
//! - **`BNetzA` BK7-14-020** — `GaBi` Gas 2.0 ruling (current)
//! - **`DVGW G 685`** — technical standard for gas metering and allocation
//! - DVGW AHBs and MIGs: <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas>
//!
//! # Relationship to other crates
//!
//! | Crate | Layer |
//! |---|---|
//! | `dvgw-edi` | EDIFACT parsing (ALOCAT, NOMINT, NOMRES) — **this crate** |
//! | `mako-gabi-gas` | GaBi Gas process engine (Workflow, deadline handling) |
//! | `mako-engine` | Event-sourced workflow runtime |
//! | `edi-energy` | BDEW EDI@Energy formats (UTILMD, MSCONS, APERAK, …) |

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::return_self_not_must_use)]

mod any_message;
mod error;
mod message;
mod message_type;
mod platform;
mod report;
mod validate;
mod version;

/// Concrete DVGW message type structs.
pub mod messages;

pub use any_message::AnyDvgwMessage;
pub use error::Error;
pub use message::DvgwMessage;
pub use message_type::DvgwMessageType;
pub use platform::DvgwPlatform;
pub use report::{DvgwIssue, DvgwReport};
pub use version::DvgwVersion;
