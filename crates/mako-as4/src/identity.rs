//! One local MP-ID and the `asx-rs` client built for it.
//!
//! A receiving MSH checks that the signing certificate's subject OU carries
//! the `eb:From` MP-ID (BDEW AS4-Profil 1.2 Kap. 2.3.2, 2.3.4.4; Regelungen zum
//! Übertragungsweg für AS4 2.6, Kap. 5.2, S. 6). So every MP-ID a deployment
//! sends and receives for has its own Zertifikatstripel — signing, encryption
//! and TLS (Kap. 1.3, S. 3) — and its own `As4` client. A client that only
//! receives loads the TLS key pair too: the built-in client opens every
//! connection with it, and BDEW AS4-Profil 1.2 Kap. 2.2.6.1 (S. 10) states
//! „Client-Authentifizierung ist ERFORDERLICH."

use crate::partner_directory::PartnerDirectory;

/// The key material of one local MP-ID.
#[derive(Debug, Clone)]
pub struct LocalIdentity {
    /// The MP-ID (`eb:PartyId`).
    pub mp_id: String,
    /// The NAD DE3055 agency of the MP-ID; selects the party type.
    pub agency: String,
    /// Signs messages and receipts: the EMT.MAK signing certificate (leaf
    /// first, then the intermediates) and its key.
    pub signer: asx::KeyPair,
    /// Decrypts inbound messages.
    pub decrypter: asx::KeyPair,
    /// Encryption key pairs replaced by `decrypter` that still decrypt: a
    /// partner may encrypt to the previous certificate until the overlap of
    /// Regelungen zum Übertragungsweg für AS4 2.6, Kap. 5.3 (S. 6) ends („Im
    /// Überlappungszeitraum müssen alle Marktpartner in der Lage sein, sowohl
    /// mit den bisher genutzten als auch mit den neuen Zertifikaten signierte
    /// und verschlüsselte AS4-Nachrichten zu verarbeiten.").
    pub retired_decrypters: Vec<asx::KeyPair>,
    /// Opens the mutual-TLS connection to a partner MSH: an EC key pair.
    pub tls: asx::KeyPair,
}

/// The `asx-rs` builder for `identity`: `Profile::bdew()` capped at
/// [`MAX_MESSAGE_BYTES`](crate::constants::MAX_MESSAGE_BYTES), the SM-PKI root
/// as the only trust anchor, the CCRL-S(Root) as CRL signer, the TLS key pair
/// as the client identity and every partner of `partners`.
///
/// `build()` on the result is the startup check of the identity: every refusal
/// is `asx::ErrorKind::Config`, naming the setting and the rule. Under
/// `Profile::bdew()` it refuses a signing, decryption or TLS certificate
/// without the MP-ID in its subject OU or without exactly one URI subject
/// alternative name (Regelungen zum Übertragungsweg für AS4 2.6, Kap. 5.2,
/// S. 6), a TLS key pair that is not EC, signing, encryption and TLS
/// certificates that certify one key pair (BDEW AS4-Profil 1.2 Kap. 2.3.4.4,
/// compared by key value), and the same rules for every partner.
pub fn client_builder(
    identity: &LocalIdentity,
    trust_anchor: asx::Certificate,
    crl_signer: asx::Certificate,
    partners: &PartnerDirectory,
) -> asx::Builder<asx::as4::As4> {
    let mut local = asx::LocalParty::new(&identity.mp_id)
        .party_type(crate::constants::party_id_type_for_agency(&identity.agency))
        .signer(identity.signer.clone())
        .decrypter(identity.decrypter.clone())
        .tls_identity(identity.tls.clone());
    for retired in &identity.retired_decrypters {
        local = local.decrypter(retired.clone());
    }
    let mut builder = asx::as4::As4::builder(
        asx::Profile::bdew().max_message_bytes(crate::constants::MAX_MESSAGE_BYTES),
    )
    .local(local)
    .trust_anchor(trust_anchor)
    .crl_signer(crl_signer);
    for (_, partner) in partners.iter() {
        builder = builder.partner(partner.clone());
    }
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use asx::testkit::pki::{KeyAlg, TestCa};

    fn identity(party: &asx::testkit::pki::TestParty) -> LocalIdentity {
        LocalIdentity {
            mp_id: party.id.clone(),
            agency: "293".to_owned(),
            signer: party.signing.key_pair(),
            decrypter: party.encryption.key_pair(),
            retired_decrypters: Vec::new(),
            tls: party.tls.key_pair(),
        }
    }

    #[test]
    fn a_complete_identity_builds() {
        let ca = TestCa::sm_pki();
        let crl = ca.crl_signer("CCRL-S", KeyAlg::EcBrainpoolP256r1);
        let party = ca.bdew_party("9900357000004");
        client_builder(
            &identity(&party),
            ca.anchor(),
            crl.certificate(),
            &PartnerDirectory::default(),
        )
        .build()
        .expect("a BDEW identity under its CA builds");
    }

    /// BDEW AS4-Profil 1.2 Kap. 2.3.4.4 separates key pairs, not certificates:
    /// a TLS certificate issued over the signing key — another serial, a valid
    /// TLS certificate in every other respect — does not build.
    #[test]
    fn a_tls_certificate_over_the_signing_key_does_not_build() {
        use asx::testkit::pki::{Eku, IssueOptions, Nid};
        let ca = TestCa::sm_pki();
        let crl = ca.crl_signer("CCRL-S", KeyAlg::EcBrainpoolP256r1);
        let party = ca.bdew_party("9900357000004");
        let url = party
            .endpoint
            .clone()
            .expect("a BDEW party has an endpoint");
        let host = url
            .trim_start_matches("https://")
            .split('/')
            .next()
            .expect("the endpoint has a host")
            .to_owned();
        let mut id = identity(&party);
        id.tls = ca
            .issue_with_options(
                "tls",
                KeyAlg::EcBrainpoolP256r1,
                IssueOptions {
                    subject: vec![(Nid::ORGANIZATIONALUNITNAME, "9900357000004".to_owned())],
                    uri_sans: vec![url],
                    dns_sans: vec![host],
                    eku: vec![Eku::ServerAuth, Eku::ClientAuth],
                    key_pem: Some(party.signing.key_pem.clone()),
                    ..IssueOptions::default()
                },
            )
            .key_pair();
        assert_ne!(
            asx::Decrypter::certificate(&id.tls).der(),
            asx::Decrypter::certificate(&id.signer).der(),
            "two certificates, one key pair"
        );
        let err = client_builder(
            &id,
            ca.anchor(),
            crl.certificate(),
            &PartnerDirectory::default(),
        )
        .build()
        .unwrap_err();
        assert_eq!(err.kind(), asx::ErrorKind::Config, "{err}");
        assert!(err.to_string().contains("Kap. 2.3.4.4"), "{err}");
    }

    /// `build()` holds Regelungen zum Übertragungsweg für AS4 2.6, Kap. 5.2
    /// (S. 6) for every certificate of the triple: one issued to another MP-ID
    /// does not build, and the refusal names the rule.
    #[test]
    fn a_certificate_of_another_mp_id_does_not_build() {
        let ca = TestCa::sm_pki();
        let crl = ca.crl_signer("CCRL-S", KeyAlg::EcBrainpoolP256r1);
        let party = ca.bdew_party("9900357000004");
        let other = ca.bdew_party("9900357000005");
        for swap in 0..4 {
            let mut id = identity(&party);
            match swap {
                0 => id.signer = other.signing.key_pair(),
                1 => id.decrypter = other.encryption.key_pair(),
                2 => id.tls = other.tls.key_pair(),
                _ => id.retired_decrypters = vec![other.encryption.key_pair()],
            }
            let err = client_builder(
                &id,
                ca.anchor(),
                crl.certificate(),
                &PartnerDirectory::default(),
            )
            .build()
            .unwrap_err();
            assert_eq!(err.kind(), asx::ErrorKind::Config, "{swap}: {err}");
            assert!(err.to_string().contains("Kap. 5.2"), "{swap}: {err}");
        }
    }

    /// Kap. 5.2: a certificate without the AS4 address as its one URI subject
    /// alternative name does not build.
    #[test]
    fn a_certificate_without_a_uri_does_not_build() {
        use asx::testkit::pki::{IssueOptions, Nid};
        let ca = TestCa::sm_pki();
        let crl = ca.crl_signer("CCRL-S", KeyAlg::EcBrainpoolP256r1);
        let party = ca.bdew_party("9900357000004");
        let ou = |uris: Vec<String>| IssueOptions {
            subject: vec![(Nid::ORGANIZATIONALUNITNAME, "9900357000004".to_owned())],
            uri_sans: uris,
            ..IssueOptions::default()
        };
        for uris in [
            Vec::new(),
            vec![
                "https://a.example/as4".to_owned(),
                "https://b.example/as4".to_owned(),
            ],
        ] {
            let mut id = identity(&party);
            id.decrypter = ca
                .issue_with_options("enc", KeyAlg::EcBrainpoolP256r1, ou(uris.clone()))
                .key_pair();
            let err = client_builder(
                &id,
                ca.anchor(),
                crl.certificate(),
                &PartnerDirectory::default(),
            )
            .build()
            .unwrap_err();
            assert_eq!(err.kind(), asx::ErrorKind::Config, "{uris:?}: {err}");
            assert!(err.to_string().contains("Kap. 5.2"), "{uris:?}: {err}");
        }
    }

    /// The trust anchor is the SM-PKI root; a signer under another CA does
    /// not chain to it and the identity does not build.
    #[test]
    fn a_signer_outside_the_trust_anchor_does_not_build() {
        let ca = TestCa::sm_pki();
        let other = TestCa::sm_pki();
        let crl = ca.crl_signer("CCRL-S", KeyAlg::EcBrainpoolP256r1);
        let party = other.bdew_party("9900357000004");
        let err = client_builder(
            &identity(&party),
            ca.anchor(),
            crl.certificate(),
            &PartnerDirectory::default(),
        )
        .build()
        .unwrap_err();
        assert_eq!(err.kind(), asx::ErrorKind::Config);
    }
}
