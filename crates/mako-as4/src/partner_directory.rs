//! The trading partners a deployment exchanges AS4 messages with.
//!
//! [`PartnerDirectory`] holds one [`asx::Partner`] per counterparty MP-ID:
//! its party type, its inbox endpoint, its encryption certificate, the signing
//! certificates pinned for it. Every receipt is verified with its
//! NonRepudiationInformation — Regelungen zum Übertragungsweg für AS4 2.6,
//! Kap. 7.3 (S. 8): „Ohne eine NRR gilt die Übertragung als gescheitert." — so
//! no partner is configured to deliver without one. The same directory feeds
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
//! })
//! .unwrap();
//! assert!(dir.contains("9900000000002"));
//! assert!(dir.deliverable("9900000000002").is_none(), "no endpoint, nothing to send to");
//! ```

use std::collections::BTreeMap;

use crate::certificate::{carries_mp_id, uri_subject_alt_names};
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
    /// An endpoint whose host is an IP address: the TLS connection must use
    /// Server Name Indication, and SNI carries a DNS host name only (RFC 6066
    /// Section 3).
    #[error(
        "AS4 partner {mp_id}: the endpoint {endpoint:?} names an IP address; Regelungen zum \
         Übertragungsweg für AS4 2.6, Kap. 3 (S. 5) requires Server Name Indication, which \
         carries a host name"
    )]
    EndpointWithoutHostName {
        /// The partner.
        mp_id: String,
        /// The configured endpoint.
        endpoint: String,
    },
    /// An encryption certificate whose subject `OU` is not the partner's
    /// MP-ID: a message encrypted to it is readable by another market
    /// participant.
    #[error(
        "AS4 partner {0}: the subject OU of the encryption certificate is not {0}; Regelungen \
         zum Übertragungsweg für AS4 2.6, Kap. 5.2 (S. 6) requires the MP-ID in the OU"
    )]
    MpIdNotInSubject(String),
    /// An encryption certificate with more than one URI subject alternative
    /// name.
    #[error(
        "AS4 partner {mp_id}: the encryption certificate names {uris:?}; Regelungen zum \
         Übertragungsweg für AS4 2.6, Kap. 5.2 (S. 6) admits one communication address per \
         certificate"
    )]
    SeveralAddresses {
        /// The partner.
        mp_id: String,
        /// The certificate's URI subject alternative names.
        uris: Vec<String>,
    },
    /// An endpoint other than the address the partner's certificate names.
    #[error(
        "AS4 partner {mp_id}: the endpoint {endpoint:?} is not the address {uri:?} of its \
         encryption certificate; Regelungen zum Übertragungsweg für AS4 2.6, Kap. 2 (S. 4) \
         takes the AS4 address from the certificate"
    )]
    EndpointNotInCertificate {
        /// The partner.
        mp_id: String,
        /// The configured endpoint.
        endpoint: String,
        /// The certificate's URI subject alternative name.
        uri: String,
    },
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
    /// An empty or repeated MP-ID, a non-`https` endpoint, an endpoint on an
    /// IP address, an endpoint without an encryption certificate, an
    /// encryption certificate of another MP-ID or with several addresses, an
    /// endpoint other than the address the certificate names.
    ///
    /// Regelungen zum Übertragungsweg für AS4 2.6, Kap. 2 (S. 4): „Die URI des
    /// AS4-Webservices muss aus den vorliegenden Zertifikaten dem Feld des
    /// Alternativnamens vom Typ URI entnommen werden.“ A certificate without
    /// that name is not refused here; makod reports it at startup.
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
            if names_ip_address(endpoint) {
                return Err(PartnerDirectoryError::EndpointWithoutHostName {
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
            if !carries_mp_id(&cert, &mp_id) {
                return Err(PartnerDirectoryError::MpIdNotInSubject(mp_id));
            }
            let uris = uri_subject_alt_names(cert.der()).unwrap_or_default();
            if uris.len() > 1 {
                return Err(PartnerDirectoryError::SeveralAddresses { mp_id, uris });
            }
            if let (Some(endpoint), Some(uri)) = (&entry.endpoint, uris.first())
                && endpoint != uri
            {
                return Err(PartnerDirectoryError::EndpointNotInCertificate {
                    mp_id,
                    endpoint: endpoint.clone(),
                    uri: uri.clone(),
                });
            }
            partner = partner.encryption_cert(cert);
        }
        for cert in entry.signing_certs {
            partner = partner.signing_cert(cert);
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

/// Whether the host of the `https` URL `endpoint` is an IPv4 or IPv6 literal.
fn names_ip_address(endpoint: &str) -> bool {
    let rest = endpoint.strip_prefix("https://").unwrap_or(endpoint);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.starts_with('[') {
        return true;
    }
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    host.parse::<std::net::Ipv4Addr>().is_ok()
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

    /// Kap. 3: SNI carries a host name, so an endpoint on an IP address
    /// cannot use it.
    #[test]
    fn an_endpoint_on_an_ip_address_is_refused() {
        for endpoint in [
            "https://203.0.113.7/as4",
            "https://203.0.113.7:8443/as4",
            "https://[2001:db8::7]/as4",
        ] {
            let mut dir = PartnerDirectory::default();
            let err = dir
                .insert(PartnerEntry {
                    endpoint: Some(endpoint.to_owned()),
                    encryption_cert: Some(enc_cert()),
                    ..entry("9900000000002")
                })
                .unwrap_err();
            assert!(
                matches!(err, PartnerDirectoryError::EndpointWithoutHostName { .. }),
                "{endpoint}: {err:?}"
            );
        }
        assert!(!names_ip_address("https://partner.example:8443/as4"));
    }

    /// Kap. 5.2: a message is encrypted to the certificate of the partner's
    /// own MP-ID, never to another market participant's.
    #[test]
    fn an_encryption_certificate_of_another_mp_id_is_refused() {
        let other = asx::testkit::pki::TestCa::sm_pki()
            .bdew_party("9900000000003")
            .encryption
            .certificate();
        let mut dir = PartnerDirectory::default();
        let err = dir
            .insert(PartnerEntry {
                endpoint: Some("https://partner.example/as4".to_owned()),
                encryption_cert: Some(other),
                ..entry("9900000000002")
            })
            .unwrap_err();
        assert_eq!(
            err,
            PartnerDirectoryError::MpIdNotInSubject("9900000000002".to_owned())
        );
    }

    /// Kap. 2 and 5.2: the endpoint is the one address the certificate
    /// names.
    #[test]
    fn the_endpoint_is_the_address_of_the_certificate() {
        let key = enc_cert();
        let cert = |uris: &[&str]| crate::certificate::test_cert::with(&key, "9900000000002", uris);
        let insert = |endpoint: &str, cert: asx::Certificate| {
            PartnerDirectory::default().insert(PartnerEntry {
                endpoint: Some(endpoint.to_owned()),
                encryption_cert: Some(cert),
                ..entry("9900000000002")
            })
        };
        insert(
            "https://partner.example/as4",
            cert(&["https://partner.example/as4"]),
        )
        .expect("the certificate's address");
        assert!(matches!(
            insert(
                "https://other.example/as4",
                cert(&["https://partner.example/as4"])
            ),
            Err(PartnerDirectoryError::EndpointNotInCertificate { .. })
        ));
        assert!(matches!(
            insert(
                "https://partner.example/as4",
                cert(&["https://partner.example/as4", "https://partner.example/b"])
            ),
            Err(PartnerDirectoryError::SeveralAddresses { .. })
        ));
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
}
