//! One local MP-ID and the `asx-rs` client built for it.
//!
//! A receiving MSH checks that the signing certificate's subject OU carries
//! the `eb:From` MP-ID (BDEW AS4-Profil 1.2 §2.3.2, §2.3.4.4; RzÜ AS4 2.6
//! §5.2: „Das Feld Organisational Unit („OU") des Subject muss die MP-ID
//! enthalten"). So every MP-ID a deployment sends and receives for has its own
//! signing, encryption and TLS key pairs — and its own `As4` client.

use crate::keys::same_public_key;
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
    /// Opens the mutual-TLS connection to a partner MSH.
    pub tls: asx::KeyPair,
}

/// A local identity that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// Two of the three key pairs certify the same public key.
    #[error(
        "AS4 identity {mp_id}: the {first} and {second} certificates certify the same key pair; \
         BDEW AS4-Profil 1.2 §2.3.4.4 requires different key pairs for signing, encryption and TLS"
    )]
    SharedKeyPair {
        /// The MP-ID.
        mp_id: String,
        /// One use.
        first: &'static str,
        /// The other use.
        second: &'static str,
    },
}

/// The `asx-rs` builder for `identity`: `Profile::bdew()` capped at
/// [`MAX_MESSAGE_BYTES`](crate::constants::MAX_MESSAGE_BYTES), the SM-PKI root
/// as the only trust anchor, the CCRL-S(Root) as CRL signer and every partner
/// of `partners`.
///
/// `build()` on the result is the startup check of the identity: every refusal
/// is `asx::ErrorKind::Config`, naming the setting and the profile section.
///
/// # Errors
///
/// [`IdentityError::SharedKeyPair`] when two of the signing, encryption and
/// TLS certificates certify one key — a case `build()` does not see, because it
/// compares certificates.
pub fn client_builder(
    identity: &LocalIdentity,
    trust_anchor: asx::Certificate,
    crl_signer: asx::Certificate,
    partners: &PartnerDirectory,
) -> Result<asx::Builder<asx::as4::As4>, IdentityError> {
    let certs = [
        (
            "signing",
            asx::Decrypter::certificate(&identity.signer).clone(),
        ),
        (
            "encryption",
            asx::Decrypter::certificate(&identity.decrypter).clone(),
        ),
        ("TLS", asx::Decrypter::certificate(&identity.tls).clone()),
    ];
    for (i, (first, a)) in certs.iter().enumerate() {
        for (second, b) in &certs[i + 1..] {
            if same_public_key(a, b) {
                return Err(IdentityError::SharedKeyPair {
                    mp_id: identity.mp_id.clone(),
                    first,
                    second,
                });
            }
        }
    }
    let local = asx::LocalParty::new(&identity.mp_id)
        .party_type(crate::constants::party_id_type_for_agency(&identity.agency))
        .signer(identity.signer.clone())
        .decrypter(identity.decrypter.clone())
        .tls_identity(identity.tls.clone());
    let mut builder = asx::as4::As4::builder(
        asx::Profile::bdew().max_message_bytes(crate::constants::MAX_MESSAGE_BYTES),
    )
    .local(local)
    .trust_anchor(trust_anchor)
    .crl_signer(crl_signer);
    for (_, partner) in partners.iter() {
        builder = builder.partner(partner.clone());
    }
    Ok(builder)
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
        .expect("distinct key pairs")
        .build()
        .expect("a BDEW identity under its CA builds");
    }

    /// One key pair for TLS and signing is refused here, before `build()`.
    #[test]
    fn a_tls_key_pair_that_is_the_signing_key_pair_is_refused() {
        let ca = TestCa::sm_pki();
        let crl = ca.crl_signer("CCRL-S", KeyAlg::EcBrainpoolP256r1);
        let party = ca.bdew_party("9900357000004");
        let mut id = identity(&party);
        id.tls = party.signing.key_pair();
        let err = client_builder(
            &id,
            ca.anchor(),
            crl.certificate(),
            &PartnerDirectory::default(),
        )
        .unwrap_err();
        assert_eq!(
            err,
            IdentityError::SharedKeyPair {
                mp_id: "9900357000004".to_owned(),
                first: "signing",
                second: "TLS",
            }
        );
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
        .unwrap()
        .build()
        .unwrap_err();
        assert_eq!(err.kind(), asx::ErrorKind::Config);
    }
}
