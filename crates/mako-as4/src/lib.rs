#![deny(missing_docs)]
//! BDEW MaKo AS4 profile for German energy market communication.
//!
//! This crate encodes the **BDEW AS4 Kommunikationshandbuch** requirements on top
//! of [`asx_rs`](https://docs.rs/asx-rs), providing:
//!
//! - [`constants`] — BDEW-specific URIs, algorithm identifiers, and retry parameters
//! - [`pmode`] — [`BdewAction`] enum + [`bdew_pmode`] / [`bdew_pmode_encrypted`] factory functions
//! - [`profile`] — [`BdewAs4Profile`] entry point + [`bdew_mako_profile_stack`]
//!
//! ## BDEW AS4 requirements
//!
//! | Requirement | Specification |
//! |---|---|
//! | Transport | HTTPS + mutual TLS (TLS 1.2 minimum) |
//! | SOAP version | 1.2 |
//! | MEP | One-Way/Push (mandatory) |
//! | Signing | RSA-SHA256 + Exclusive C14N + SHA-256 digest — **mandatory** |
//! | Encryption | AES-128-CBC or AES-256-GCM + RSA-OAEP — **optional** |
//! | Party ID | 13-digit GLN, type `urn:oasis:names:tc:ebcore:partyid-type:iso6523:0088` |
//! | Retry window | 72 hours, up to 5 attempts |
//! | Deduplication | Required (persistent `asx_rs::storage::TtlDedupStorage`) |
//!
//! AS4 became mandatory for electricity market communication on **1 April 2024**
//! and for gas market communication on **1 April 2025**
//! (BNetzA order BK6-22-024 / BK7-22-023).
//!
//! ## Quick start
//!
//! ```rust
//! use mako_as4::{BdewAs4Profile, BdewAction, bdew_pmode, constants};
//!
//! // Build a profile and register bilateral P-Modes for each trading partner
//! let mut profile = BdewAs4Profile::new();
//! profile
//!     .register_pmode(bdew_pmode("pm-utilmd-a", "9900000000001", BdewAction::Utilmd))
//!     .register_pmode(bdew_pmode("pm-aperak-a", "9900000000001", BdewAction::Aperak));
//!
//! // Fail-fast at startup
//! profile.validate().expect("BDEW MaKo profile must satisfy all security invariants");
//!
//! // Resolve a P-Mode at send time
//! let pm = profile.resolve_pmode(
//!     "9900000000001",
//!     constants::SERVICE,
//!     &BdewAction::Utilmd.as_uri(),
//! );
//! assert!(pm.is_some());
//! ```

pub mod constants;
pub mod partner_directory;
pub mod pmode;
pub mod profile;
#[cfg(feature = "server")]
pub mod server;

// ── Top-level re-exports for ergonomics ──────────────────────────────────────

pub use partner_directory::{PartnerDirectory, PartnerDirectoryParseError};
pub use pmode::{BdewAction, PModeRegistry, bdew_pmode, bdew_pmode_encrypted};
pub use profile::{BdewAs4Profile, bdew_mako_profile_stack};
#[cfg(feature = "server")]
pub use server::bdew_router_config;
