//! BDEW-specific test helpers for AS4 integration testing without WIRK certificates.
//!
//! Enabled by the `testing` Cargo feature — **never compile into production binaries**.
//!
//! # What this provides
//!
//! - [`generate_self_signed_bdew_keypair`] — generate ephemeral EC (BrainpoolP256r1)
//!   certificate + private key pairs.  Delegates to `asx_rs::fixtures::generate_self_signed_ec_keypair`
//!   with `EcCurve::BrainpoolP256r1`.
//! - [`BdewTestPki`] — a three-keypair bundle (TLS, signing, encryption) in one call.
//! - [`MockAs4Endpoint`] — re-exported from `asx_rs::as4::mock_endpoint` for convenience.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use mako_as4::testing::{BdewTestPki, MockAs4Endpoint};
//!
//! # async fn example() {
//! // Full PKI bundle in one call
//! let pki = BdewTestPki::generate("Demo NB 9900357000004");
//!
//! // In-process mock AS4 endpoint — no WIRK certificates required
//! let endpoint = MockAs4Endpoint::bind("127.0.0.1:0").await.unwrap();
//! let url = endpoint.local_url();
//!
//! // Wire pki.signing into SessionContextBuilder for the sender side,
//! // pki.encryption.cert_pem into register_partner_encryption_cert() for the receiver side.
//! # }
//! ```

// Re-export asx-rs v0.6 testing primitives
pub use asx_rs::as4::mock_endpoint::{MockAs4Endpoint, MockReceivedMessage};
pub use asx_rs::fixtures::{
    EcCurve, generate_self_signed_ec_keypair, generate_self_signed_rsa_keypair,
};

/// Purpose of a generated BDEW test keypair.
///
/// BDEW AS4-Profil v1.2 requires **separate** keypairs for signing and encryption
/// (BSI TR-03116-3 §9.1 / §9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BdewCertPurpose {
    /// WS-Security XMLDSig signing (§2.2.6.2.1, KeyUsage: `digitalSignature`).
    Signing,
    /// ECDH-ES key agreement for XML Encryption (§2.2.6.2.2, KeyUsage: `keyAgreement`).
    Encryption,
}

/// A self-signed BrainpoolP256r1 certificate + private key pair for BDEW AS4 testing.
#[derive(Debug, Clone)]
pub struct BdewKeypair {
    /// PEM-encoded X.509 certificate.
    pub cert_pem: Vec<u8>,
    /// PEM-encoded EC private key (PKCS#8 format).
    pub key_pem: Vec<u8>,
}

impl BdewKeypair {
    /// Return the certificate PEM as a UTF-8 `&str`.
    pub fn cert_pem_str(&self) -> &str {
        std::str::from_utf8(&self.cert_pem).expect("cert PEM is UTF-8")
    }

    /// Return the private key PEM as a UTF-8 `&str`.
    pub fn key_pem_str(&self) -> &str {
        std::str::from_utf8(&self.key_pem).expect("key PEM is UTF-8")
    }
}

/// Generate a self-signed EC (BrainpoolP256r1) certificate + private key pair.
///
/// Both signing and encryption keypairs use `BrainpoolP256r1` as mandated by
/// BDEW AS4-Profil v1.2 §2.2.6.2.1/§2.2.6.2.2 (BSI TR-03116-3 §9.1/§9.2).
///
/// The `purpose` parameter is informational for callers — asx-rs v0.6 generates
/// a single cert with both `digitalSignature + keyAgreement` KeyUsage, which is
/// sufficient for local testing. Production WIRK certs use separate KeyUsage.
///
/// # Panics
///
/// Panics if OpenSSL cannot generate BrainpoolP256r1 keypairs.
///
/// # Example
///
/// ```rust
/// use mako_as4::testing::{BdewCertPurpose, generate_self_signed_bdew_keypair};
///
/// let signing = generate_self_signed_bdew_keypair("CN=Test NB", BdewCertPurpose::Signing);
/// let encrypt = generate_self_signed_bdew_keypair("CN=Test NB", BdewCertPurpose::Encryption);
/// assert!(signing.cert_pem_str().starts_with("-----BEGIN CERTIFICATE-----"));
/// ```
pub fn generate_self_signed_bdew_keypair(
    subject_cn: &str,
    _purpose: BdewCertPurpose,
) -> BdewKeypair {
    let (cert_pem, key_pem) = generate_self_signed_ec_keypair(subject_cn, EcCurve::BrainpoolP256r1);
    BdewKeypair { cert_pem, key_pem }
}

/// A complete BDEW certificate bundle for testing.
///
/// Mirrors the **certificate triplet** required by BDEW AS4-Profil v1.2:
/// TLS, WS-Security signing, and XML Encryption — all on BrainpoolP256r1.
///
/// # Example
///
/// ```rust
/// use mako_as4::testing::BdewTestPki;
///
/// let pki = BdewTestPki::generate("Test NB 9900357000004");
/// // All three certs are distinct (different serial numbers)
/// assert_ne!(pki.signing.cert_pem, pki.encryption.cert_pem);
/// ```
#[derive(Debug, Clone)]
pub struct BdewTestPki {
    /// TLS client/server certificate (mTLS transport authentication).
    pub tls: BdewKeypair,
    /// WS-Security signing keypair — ECDSA-SHA256, BrainpoolP256r1 (§2.2.6.2.1).
    pub signing: BdewKeypair,
    /// XML Encryption keypair — ECDH-ES key agreement, BrainpoolP256r1 (§2.2.6.2.2).
    pub encryption: BdewKeypair,
}

impl BdewTestPki {
    /// Generate a complete three-keypair BDEW test bundle.
    ///
    /// All three certificates are self-signed on BrainpoolP256r1.
    ///
    /// # Panics
    ///
    /// Panics if OpenSSL cannot generate BrainpoolP256r1 keypairs.
    pub fn generate(subject_cn: &str) -> Self {
        Self {
            tls: generate_self_signed_bdew_keypair(subject_cn, BdewCertPurpose::Signing),
            signing: generate_self_signed_bdew_keypair(subject_cn, BdewCertPurpose::Signing),
            encryption: generate_self_signed_bdew_keypair(subject_cn, BdewCertPurpose::Encryption),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_bdew_keypair() {
        let kp = generate_self_signed_bdew_keypair("Test NB", BdewCertPurpose::Signing);
        assert!(kp.cert_pem_str().starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(!kp.key_pem.is_empty());
    }

    #[test]
    fn test_pki_three_distinct_keypairs() {
        let pki = BdewTestPki::generate("Test NB 9900357000004");
        assert!(!pki.signing.cert_pem.is_empty());
        assert!(!pki.encryption.cert_pem.is_empty());
        assert_ne!(pki.signing.cert_pem, pki.encryption.cert_pem);
    }
}
