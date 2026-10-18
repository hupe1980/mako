//! The trading partners a deployment exchanges AS4 messages with.
//!
//! [`PartnerDirectory`] holds one [`asx::Partner`] per counterparty MP-ID:
//! its party type, its inbox endpoint, its encryption certificate, the signing
//! certificates pinned for it and the quirks it needs. The same directory feeds
//! every `As4` client of the deployment (one per local MP-ID) and answers the
//! send path's lookup.
//!
//! ```rust
//! use mako_as4::partner_directory::{PartnerDirectory, PartnerEntry};
//!
//! let mut dir = PartnerDirectory::default();
//! dir.insert(PartnerEntry {
//!     mp_id: "9900000000002".to_owned(),
//!     agency: "293".to_owned(),
//!     endpoint: None,
//!     encryption_cert: None,
//!     signing_certs: Vec::new(),
//!     quirks: Vec::new(),
//! })
//! .unwrap();
//! assert!(dir.contains("9900000000002"));
//! assert!(dir.deliverable("9900000000002").is_none(), "no endpoint, nothing to send to");
//! ```

use std::collections::BTreeMap;

use crate::constants::party_id_type_for_agency;

/// What the directory knows about one counterparty.
#[derive(Debug, Clone)]
pub struct PartnerEntry {
    /// The partner's MP-ID (`eb:PartyId`).
    pub mp_id: String,
    /// The NAD DE3055 agency of the MP-ID; selects the party type.
    pub agency: String,
    /// The partner's AS4 inbox, `https` only. `None` for a partner mako only
    /// receives from.
    pub endpoint: Option<String>,
    /// The partner's encryption certificate; required with an endpoint.
    pub encryption_cert: Option<asx::Certificate>,
    /// Signing certificates pinned for the partner: an inbound message from it
    /// must be signed by one of them. Several for a rollover; none to rely on
    /// the SM-PKI chain and the MP-ID in the certificate's OU alone.
    pub signing_certs: Vec<asx::Certificate>,
    /// Interoperability quirks the partner needs.
    pub quirks: Vec<asx::Quirk>,
}

/// A directory entry that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PartnerDirectoryError {
    /// The MP-ID is empty.
    #[error("an AS4 partner has an empty MP-ID")]
    EmptyMpId,
    /// The MP-ID is configured twice.
    #[error("AS4 partner {0} is configured twice")]
    Duplicate(String),
    /// The endpoint is not `https`.
    #[error("AS4 partner {mp_id}: the endpoint must use HTTPS (got {endpoint:?})")]
    InsecureEndpoint {
        /// The partner.
        mp_id: String,
        /// The configured endpoint.
        endpoint: String,
    },
    /// An endpoint without the encryption certificate every message needs.
    #[error(
        "AS4 partner {0} has an endpoint but no encryption certificate: BDEW AS4-Profil 1.2 \
         §2.2.6.2.2 encrypts every message with the recipient's certificate"
    )]
    MissingEncryptionCert(String),
    /// An unknown quirk name.
    #[error(
        "AS4 partner {mp_id}: unknown quirk {quirk:?}; the one AS4 quirk is \
         \"receipt-without-non-repudiation\""
    )]
    UnknownQuirk {
        /// The partner.
        mp_id: String,
        /// The configured name.
        quirk: String,
    },
}

/// The AS4 quirk a configuration names by `name`.
///
/// `receipt-without-non-repudiation` accepts a partner's *signed* receipt that
/// carries no NonRepudiationInformation; an unsigned or mis-signed receipt is
/// still refused.
///
/// # Errors
///
/// [`PartnerDirectoryError::UnknownQuirk`] for any other name.
pub fn parse_quirk(mp_id: &str, name: &str) -> Result<asx::Quirk, PartnerDirectoryError> {
    match name {
        "receipt-without-non-repudiation" => Ok(asx::Quirk::ReceiptWithoutNonRepudiation),
        other => Err(PartnerDirectoryError::UnknownQuirk {
            mp_id: mp_id.to_owned(),
            quirk: other.to_owned(),
        }),
    }
}

/// One counterparty as the directory holds it.
#[derive(Debug, Clone)]
struct Held {
    partner: asx::Partner,
    deliverable: bool,
}

/// The trading partners, by MP-ID.
#[derive(Debug, Default, Clone)]
pub struct PartnerDirectory {
    partners: BTreeMap<String, Held>,
}

impl PartnerDirectory {
    /// Add a partner.
    ///
    /// # Errors
    ///
    /// An empty or repeated MP-ID, a non-`https` endpoint, an endpoint without
    /// an encryption certificate, or an unknown quirk.
    pub fn insert(&mut self, entry: PartnerEntry) -> Result<(), PartnerDirectoryError> {
        let mp_id = entry.mp_id.trim().to_owned();
        if mp_id.is_empty() {
            return Err(PartnerDirectoryError::EmptyMpId);
        }
        if self.partners.contains_key(&mp_id) {
            return Err(PartnerDirectoryError::Duplicate(mp_id));
        }
        let mut partner =
            asx::Partner::new(&mp_id).party_type(party_id_type_for_agency(&entry.agency));
        if let Some(endpoint) = &entry.endpoint {
            if !endpoint.starts_with("https://") {
                return Err(PartnerDirectoryError::InsecureEndpoint {
                    mp_id,
                    endpoint: endpoint.clone(),
                });
            }
            if entry.encryption_cert.is_none() {
                return Err(PartnerDirectoryError::MissingEncryptionCert(mp_id));
            }
            partner = partner.endpoint(endpoint);
        }
        if let Some(cert) = entry.encryption_cert {
            partner = partner.encryption_cert(cert);
        }
        for cert in entry.signing_certs {
            partner = partner.signing_cert(cert);
        }
        for quirk in entry.quirks {
            partner = partner.quirk(quirk);
        }
        self.partners.insert(
            mp_id,
            Held {
                partner,
                deliverable: entry.endpoint.is_some(),
            },
        );
        Ok(())
    }

    /// The partner `mp_id`, if configured.
    #[must_use]
    pub fn get(&self, mp_id: &str) -> Option<&asx::Partner> {
        self.partners.get(mp_id).map(|h| &h.partner)
    }

    /// The partner `mp_id` when it has an endpoint to send to.
    #[must_use]
    pub fn deliverable(&self, mp_id: &str) -> Option<&asx::Partner> {
        self.partners
            .get(mp_id)
            .filter(|h| h.deliverable)
            .map(|h| &h.partner)
    }

    /// Whether `mp_id` is a configured partner.
    #[must_use]
    pub fn contains(&self, mp_id: &str) -> bool {
        self.partners.contains_key(mp_id)
    }

    /// Every partner, by MP-ID.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &asx::Partner)> {
        self.partners
            .iter()
            .map(|(id, h)| (id.as_str(), &h.partner))
    }

    /// Number of partners.
    #[must_use]
    pub fn len(&self) -> usize {
        self.partners.len()
    }

    /// `true` when no partner is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.partners.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mp_id: &str) -> PartnerEntry {
        PartnerEntry {
            mp_id: mp_id.to_owned(),
            agency: "293".to_owned(),
            endpoint: None,
            encryption_cert: None,
            signing_certs: Vec::new(),
            quirks: Vec::new(),
        }
    }

    fn enc_cert() -> asx::Certificate {
        asx::testkit::pki::TestCa::sm_pki()
            .bdew_party("9900000000002")
            .encryption
            .certificate()
    }

    #[test]
    fn a_partner_with_an_endpoint_is_deliverable() {
        let mut dir = PartnerDirectory::default();
        dir.insert(PartnerEntry {
            endpoint: Some("https://partner.example/as4/inbox".to_owned()),
            encryption_cert: Some(enc_cert()),
            ..entry("9900000000002")
        })
        .unwrap();
        assert!(dir.deliverable("9900000000002").is_some());
        assert_eq!(dir.len(), 1);
    }

    #[test]
    fn a_receive_only_partner_is_known_and_not_deliverable() {
        let mut dir = PartnerDirectory::default();
        dir.insert(entry("9900000000002")).unwrap();
        assert!(dir.contains("9900000000002"));
        assert!(dir.deliverable("9900000000002").is_none());
    }

    #[test]
    fn a_plaintext_endpoint_is_refused() {
        let mut dir = PartnerDirectory::default();
        let err = dir
            .insert(PartnerEntry {
                endpoint: Some("http://partner.example/as4".to_owned()),
                encryption_cert: Some(enc_cert()),
                ..entry("9900000000002")
            })
            .unwrap_err();
        assert!(matches!(
            err,
            PartnerDirectoryError::InsecureEndpoint { .. }
        ));
    }

    /// §2.2.6.2.2 — every message is encrypted for its recipient, so an
    /// endpoint without the certificate can deliver nothing.
    #[test]
    fn an_endpoint_without_an_encryption_certificate_is_refused() {
        let mut dir = PartnerDirectory::default();
        let err = dir
            .insert(PartnerEntry {
                endpoint: Some("https://partner.example/as4".to_owned()),
                ..entry("9900000000002")
            })
            .unwrap_err();
        assert_eq!(
            err,
            PartnerDirectoryError::MissingEncryptionCert("9900000000002".to_owned())
        );
    }

    #[test]
    fn a_partner_configured_twice_is_refused() {
        let mut dir = PartnerDirectory::default();
        dir.insert(entry("9900000000002")).unwrap();
        assert!(matches!(
            dir.insert(entry("9900000000002")),
            Err(PartnerDirectoryError::Duplicate(_))
        ));
    }

    #[test]
    fn the_receipt_quirk_is_the_only_one() {
        assert!(matches!(
            parse_quirk("9900000000002", "receipt-without-non-repudiation"),
            Ok(asx::Quirk::ReceiptWithoutNonRepudiation)
        ));
        assert!(parse_quirk("9900000000002", "lenient").is_err());
    }
}
