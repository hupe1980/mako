//! BDEW AS4-Profil 1.2 for German energy market communication, on
//! [`asx-rs`](https://docs.rs/asx-rs) 0.16.
//!
//! `asx::Profile::bdew()` fixes every security and wire setting of the
//! profile — ECDSA on brainpoolP256r1 with an `X509PKIPathv1` token, ECDH-ES
//! with ConcatKDF and AES-128-GCM with an `X509SKI` reference, sign then
//! encrypt, gzip, the fixed Action, roles and `AgreementRef`, an empty
//! `ConversationId`, CRL revocation with the three-day distrust rule. Nothing
//! here can relax it. What this crate adds is what a mako deployment decides:
//!
//! - [`identity`] — one [`LocalIdentity`] per local MP-ID and the
//!   [`client_builder`] for its `As4` client, refusing two uses of one key pair
//! - [`partner_directory`] — [`PartnerDirectory`], one `asx::Partner` per
//!   counterparty MP-ID
//! - [`action`] — [`BdewAction`], the EDIFACT message type, and the
//!   [`Service`] it is sent under
//! - [`file_name`] — the Übertragungsdatei name of Allgemeine Festlegungen
//!   V6.1d §2.12
//! - [`message_id`] — the RFC 2822 `eb:MessageId` of an outbox entry
//! - [`constants`] — party types per issuing agency, the payload media type,
//!   the size limit and the 72 h retry window
//!
//! ## Example
//!
//! ```rust
//! use mako_as4::{BdewAction, Service, bdew_action_from_str};
//!
//! let action = bdew_action_from_str("UTILMD");
//! assert_eq!(action, BdewAction::Utilmd);
//! assert_eq!(
//!     action.service().uri(),
//!     "https://www.bdew.de/as4/communication/services/MP"
//! );
//! assert_eq!(Service::from_uri(Service::Redispatch.uri()), Some(Service::Redispatch));
//! ```

#![deny(unsafe_code)]

pub mod action;
pub mod constants;
pub mod file_name;
pub mod identity;
pub mod keys;
pub mod message_id;
pub mod partner_directory;
pub mod service;

pub use action::{BdewAction, bdew_action_from_str};
pub use file_name::uebertragungsdatei_name;
pub use identity::{IdentityError, LocalIdentity, client_builder};
pub use message_id::{InvalidMessageIdDomain, MessageIdDomain};
pub use partner_directory::{PartnerDirectory, PartnerDirectoryError, PartnerEntry, parse_quirk};
pub use service::Service;
