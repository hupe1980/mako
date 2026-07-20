//! BDEW MaKo AS4 profile for German energy market communication.
//!
//! This crate encodes the **BDEW AS4-Profil v1.2** requirements on top
//! of [`asx_rs`](https://docs.rs/asx-rs) v0.9, providing:
//!
//! - [`constants`] — BDEW-specific URIs and algorithm identifiers
//! - [`pmode`] — [`BdewAction`] enum, [`bdew_pmode`] / [`bdew_pmode_sign_only`],
//! - [`profile`] — [`BdewAs4Profile`], [`bdew_mako_profile_stack`],
//!   [`bdew_push_policy`] (inbound policy with `require_encrypted_inbound`)
//! - [`testing`] *(feature)* — [`BdewTestPki`], [`MockAs4Endpoint`],
//!   [`generate_self_signed_bdew_keypair`] (BrainpoolP256r1)
//!
//! ## BDEW AS4-Profil v1.2 crypto requirements
//!
//! | Requirement | Algorithm | Source |
//! |---|---|---|
//! | Signing | **ECDSA-SHA256 + BrainpoolP256r1** | §2.2.6.2.1 / BSI TR-03116-3 §9.1 |
//! | Signing token | **X509PKIPathv1** (`BinarySecurityToken`) | §2.2.6.2.1 |
//! | Encryption | **ECDH-ES + ConcatKDF + AES-128-GCM** | §2.2.6.2.2 / BSI TR-03116-3 §9.2 |
//! | Key reference | **X509SKI** | §2.2.6.2.2 |
//! | EC curve | **BrainpoolP256r1** (both signing and encryption) | BSI TR-03116-3 |
//!
//! Both algorithms are **auto-detected** from the key/cert type —
//! supply EC (BrainpoolP256r1) material and the correct paths are selected automatically.
//!
//! ## Signature scope
//!
//! The BDEW profile requires `PMode[1].Security.X509.Sign` to be set *"nach
//! Maßgabe der Abschnitte 5.1.4 und 5.1.5 von \[AS4\]"* (§2.2.6.2.1), and those
//! sections put the `eb:Messaging` SOAP header block inside the signature.
//!
//! The **whole `eb:Messaging` block** is signed, referenced by
//! `wsu:Id="as4-messaging"`. That scope is what makes the signature meaningful:
//! `PartyInfo`, `CollaborationInfo` and `Action` are the routing and
//! authorization metadata, so a signature covering only `eb:MessageId` would
//! leave all of it tamperable.
//!
//! On receive, the block the parser consumes is bound to the block the
//! signature verified, so a relocated-but-still-resolvable signed element cannot
//! be paired with an injected unsigned replacement (XML Signature Wrapping).
//!
//! ### Interop
//!
//! Verification is strict in one direction: a receiver requiring the full block
//! rejects a sender that signs less, while a receiver checking only that *some*
//! signature verified accepts a conformant sender either way. Conformant partner
//! stacks sign the block, so strict verification rejects only non-conformant
//! senders.
//!
//! ## ECDH-ES key derivation
//!
//! ConcatKDF uses the SP 800-56A raw-concatenation form — no `keydatalen` field,
//! no JOSE-style length prefixes — which is what BSI TR-03116-3 §9.2 requires.
//! The derivation determines the KEK, so a peer deriving it differently cannot
//! decrypt the payload.
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
#[cfg(feature = "testing")]
pub mod testing;

// ── Top-level re-exports for ergonomics ──────────────────────────────────────

/// Re-export `InsecureBypassAs4Verifier` for test-only AS4 receive without PKI.
#[cfg(feature = "testing")]
pub use asx_rs::as4::InsecureBypassAs4Verifier;
pub use partner_directory::{PartnerDirectory, PartnerDirectoryParseError};
pub use pmode::{
    BdewAction, PModeRegistry, ParseBdewActionError, bdew_action_from_str, bdew_pmode,
    bdew_pmode_sign_only, bdew_pmode_with_endpoint,
};
pub use profile::{As4PushPolicy, BdewAs4Profile, bdew_mako_profile_stack, bdew_push_policy};
#[cfg(feature = "server")]
pub use server::bdew_router_config;
#[cfg(feature = "testing")]
pub use testing::{
    BdewCertPurpose, BdewKeypair, BdewTestPki, MockAs4Endpoint, MockReceivedMessage,
    generate_self_signed_bdew_keypair,
};
