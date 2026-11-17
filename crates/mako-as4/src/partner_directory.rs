//! The trading partners a deployment exchanges AS4 messages with.
//!
//! [`PartnerDirectory`] holds one [`asx::Partner`] per counterparty MP-ID:
//! its party type, its encryption certificate, the signing certificates
//! pinned for it and, where configured, its inbox endpoint. Every receipt is
//! verified with its NonRepudiationInformation — Regelungen zum Übertragungsweg
//! für AS4 2.6, Kap. 7.3 (S. 8): „Ohne eine NRR gilt die Übertragung als
//! gescheitert." — so no partner is configured to deliver without one. The
//! same directory feeds every `As4` client of the deployment (one per local
//! MP-ID) and answers the send path's lookup.
//!
//! The inbox address is the encryption certificate's URI subject alternative
//! name — Kap. 2 (S. 4): „Die URI des AS4-Webservices muss aus den vorliegenden
//! Zertifikaten dem Feld des Alternativnamens vom Typ URI entnommen werden." An
//! entry without an endpoint leaves `asx-rs` to take it from there; one with an
//! endpoint is held to it. Both, and the certificate rules of Kap. 5.2 and the
//! DNS host SNI needs (Kap. 3), are checked by `build()` of every client.
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
//! assert!(dir.deliverable("9900000000002").is_none(), "no encryption certificate, nothing to send to");
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
    /// The partner's AS4 inbox. `None` takes it from the encryption
    /// certificate; when set, `build()` refuses any other address.
    pub endpoint: Option<String>,
    /// The partner's encryption certificate. With it the partner is
    /// deliverable; without it mako only receives from the partner.
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
    /// An endpoint without the encryption certificate every message needs.
    #[error(
        "AS4 partner {0} has an endpoint but no encryption certificate: BDEW AS4-Profil 1.2 \
         Kap. 2.2.6.2.2 encrypts every message with the recipient's certificate"
    )]
    MissingEncryptionCert(String),
}

/// The trading partners, by MP-ID.
#[derive(Debug, Default, Clone)]
pub struct PartnerDirectory {
    partners: BTreeMap<String, asx::Partner>,
    deliverable: std::collections::BTreeSet<String>,
}

impl PartnerDirectory {
    /// Add a partner.
    ///
    /// # Errors
    ///
    /// An empty or repeated MP-ID, an endpoint without an encryption
    /// certificate. What the certificates and the endpoint must be is
    /// `build()`'s to refuse.
    pub fn insert(&mut self, entry: PartnerEntry) -> Result<(), PartnerDirectoryError> {
        let mp_id = entry.mp_id.trim().to_owned();
        if mp_id.is_empty() {
            return Err(PartnerDirectoryError::EmptyMpId);
        }
        if self.partners.contains_key(&mp_id) {
            return Err(PartnerDirectoryError::Duplicate(mp_id));
        }
        if entry.endpoint.is_some() && entry.encryption_cert.is_none() {
            return Err(PartnerDirectoryError::MissingEncryptionCert(mp_id));
        }
        let mut partner =
            asx::Partner::new(&mp_id).party_type(party_id_type_for_agency(&entry.agency));
        if let Some(endpoint) = &entry.endpoint {
            partner = partner.endpoint(endpoint);
        }
        if let Some(cert) = entry.encryption_cert {
            partner = partner.encryption_cert(cert);
            self.deliverable.insert(mp_id.clone());
        }
        for cert in entry.signing_certs {
            partner = partner.signing_cert(cert);
        }
        self.partners.insert(mp_id, partner);
        Ok(())
    }

    /// The partner `mp_id`, if configured.
    #[must_use]
    pub fn get(&self, mp_id: &str) -> Option<&asx::Partner> {
        self.partners.get(mp_id)
    }

    /// The partner `mp_id` when it has an encryption certificate, and so an
    /// address, to send to.
    #[must_use]
    pub fn deliverable(&self, mp_id: &str) -> Option<&asx::Partner> {
        self.deliverable
            .contains(mp_id)
            .then(|| self.partners.get(mp_id))
            .flatten()
    }

    /// Whether `mp_id` is a configured partner.
    #[must_use]
    pub fn contains(&self, mp_id: &str) -> bool {
        self.partners.contains_key(mp_id)
    }

    /// Every partner, by MP-ID.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &asx::Partner)> {
        self.partners.iter().map(|(id, p)| (id.as_str(), p))
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
    use asx::testkit::pki::{KeyAlg, TestCa, TestParty};

    const URL: &str = "https://partner.example/as4/inbox/9900000000002";

    fn entry(mp_id: &str) -> PartnerEntry {
        PartnerEntry {
            mp_id: mp_id.to_owned(),
            agency: "293".to_owned(),
            endpoint: None,
            encryption_cert: None,
            signing_certs: Vec::new(),
        }
    }

    /// One SM-PKI, a local party and the partner `9900000000002` at [`URL`].
    struct Pki {
        ca: TestCa,
        local: TestParty,
        partner: TestParty,
    }

    impl Pki {
        fn new() -> Self {
            let ca = TestCa::sm_pki();
            Self {
                local: ca.bdew_party("9900000000001"),
                partner: ca.bdew_party_at("9900000000002", URL),
                ca,
            }
        }

        /// `build()` of a client of the local party over `dir`.
        fn build(&self, dir: &PartnerDirectory) -> asx::Result<asx::as4::As4> {
            let identity = crate::LocalIdentity {
                mp_id: self.local.id.clone(),
                agency: "293".to_owned(),
                signer: self.local.signing.key_pair(),
                decrypter: self.local.encryption.key_pair(),
                retired_decrypters: Vec::new(),
                tls: self.local.tls.key_pair(),
            };
            let crl = self.ca.crl_signer("CCRL-S", KeyAlg::EcBrainpoolP256r1);
            crate::client_builder(&identity, self.ca.anchor(), crl.certificate(), dir).build()
        }

        fn with_endpoint(&self, endpoint: Option<&str>) -> PartnerDirectory {
            let mut dir = PartnerDirectory::default();
            dir.insert(PartnerEntry {
                endpoint: endpoint.map(str::to_owned),
                encryption_cert: Some(self.partner.encryption.certificate()),
                signing_certs: vec![self.partner.signing.certificate()],
                ..entry("9900000000002")
            })
            .unwrap();
            dir
        }
    }

    /// Kap. 2: without an endpoint the address is the certificate's, and the
    /// partner is deliverable.
    #[test]
    fn a_partner_without_an_endpoint_is_reached_at_its_certificates_address() {
        let pki = Pki::new();
        let dir = pki.with_endpoint(None);
        assert!(dir.deliverable("9900000000002").is_some());
        pki.build(&dir)
            .expect("the address is taken from the certificate");
        pki.build(&pki.with_endpoint(Some(URL)))
            .expect("the certificate's own address");
        pki.build(&pki.with_endpoint(Some("HTTPS://Partner.Example:443/as4/inbox/9900000000002")))
            .expect("scheme and host case and the default port do not count");
    }

    /// Kap. 2: an endpoint other than the certificate's address does not
    /// build, nor one that is not `https` or names an IP address (Kap. 3: SNI
    /// carries a DNS host name).
    #[test]
    fn an_endpoint_other_than_the_certificates_address_does_not_build() {
        let pki = Pki::new();
        for endpoint in [
            "https://other.example/as4/inbox/9900000000002",
            "http://partner.example/as4/inbox/9900000000002",
            "https://203.0.113.7/as4/inbox/9900000000002",
        ] {
            let err = pki.build(&pki.with_endpoint(Some(endpoint))).unwrap_err();
            assert_eq!(err.kind(), asx::ErrorKind::Config, "{endpoint}: {err}");
        }
        let at_ip = pki
            .ca
            .bdew_party_at("9900000000002", "https://203.0.113.7/as4");
        let mut dir = PartnerDirectory::default();
        dir.insert(PartnerEntry {
            encryption_cert: Some(at_ip.encryption.certificate()),
            ..entry("9900000000002")
        })
        .unwrap();
        let err = pki.build(&dir).unwrap_err();
        assert!(err.to_string().contains("Kap. 3"), "{err}");
    }

    /// Kap. 5.2: a message is encrypted to the certificate of the partner's
    /// own MP-ID, never to another market participant's.
    #[test]
    fn an_encryption_certificate_of_another_mp_id_does_not_build() {
        let pki = Pki::new();
        let other = pki.ca.bdew_party_at("9900000000003", URL);
        let mut dir = PartnerDirectory::default();
        dir.insert(PartnerEntry {
            encryption_cert: Some(other.encryption.certificate()),
            ..entry("9900000000002")
        })
        .unwrap();
        let err = pki.build(&dir).unwrap_err();
        assert!(err.to_string().contains("Kap. 5.2"), "{err}");
    }

    #[test]
    fn a_receive_only_partner_is_known_and_not_deliverable() {
        let mut dir = PartnerDirectory::default();
        dir.insert(entry("9900000000002")).unwrap();
        assert!(dir.contains("9900000000002"));
        assert!(dir.deliverable("9900000000002").is_none());
    }

    /// Kap. 2.2.6.2.2 — every message is encrypted for its recipient, so an
    /// endpoint without the certificate can deliver nothing.
    #[test]
    fn an_endpoint_without_an_encryption_certificate_is_refused() {
        let mut dir = PartnerDirectory::default();
        let err = dir
            .insert(PartnerEntry {
                endpoint: Some(URL.to_owned()),
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
}
