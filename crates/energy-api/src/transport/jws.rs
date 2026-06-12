//! JWS ECDSA-SHA256 sign and verify for EDI-Energy directory records.
//!
//! The directory service uses *JSON Web Signature* (RFC 7515) with a fixed
//! JWS Protected Header and RFC 8785 (JCS) canonical JSON as the payload.
//!
//! ## Signing input
//!
//! ```text
//! protected = "eyJhbGciOiJodHRwOi8vd3d3LnczLm9yZy8yMDAxLzA0L3htbGRzaWd0LS1
//!              tb3JlI2VjZHNhLXNoYTI1NiIsInR5cCI6IkpXVCJ9"   (fixed)
//! payload   = BASE64URL(JCS(ApiRecord))
//! input     = protected + "." + payload
//! signature = ECDSA-P256-SHA256(input.as_bytes())
//! wire      = BASE64URL(raw_signature_bytes)   — header and payload omitted
//! ```
//!
//! ## Wire encoding
//!
//! REST: `X-BDEW-CERT` / `X-BDEW-SIGNATURE` HTTP headers.  
//! WebSocket: `SignedApiRecord::signing_cert` / `SignedApiRecord::signature` fields.
//!
//! Feature gate: `crypto`.

use base64ct::{Base64UrlUnpadded, Encoding};
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::{DecodePrivateKey, DecodePublicKey};

use crate::error::Error;
use crate::models::directory::ApiRecord;

/// Fixed JWS Protected Header for all directory record signatures.
///
/// Base64url-decodes to:
/// `{"alg":"http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256","typ":"JWT"}`
pub const JWS_PROTECTED_HEADER: &str = "eyJhbGciOiJodHRwOi8vd3d3LnczLm9yZy8yMDAxLzA0L3htbGRzaWctbW9yZSNlY2RzYS1zaGEyNTYiLCJ0eXAiOiJKV1QifQ";

// ── Public API ────────────────────────────────────────────────────────────────

/// Sign an [`ApiRecord`] and return the base64url-encoded JWS Signature value.
///
/// The `signing_key` must belong to an EMT.API certificate issued by SM-PKI.
///
/// # Errors
/// Returns [`Error::Signature`] if JCS serialisation fails.
pub fn sign(record: &ApiRecord, signing_key: &SigningKey) -> Result<String, Error> {
    let input = signing_input(record)?;
    let sig: Signature = signing_key.sign(input.as_bytes());
    Ok(Base64UrlUnpadded::encode_string(&sig.to_bytes()))
}

/// Verify the JWS signature on an [`ApiRecord`].
///
/// For production use, first extract the `VerifyingKey` from the signing
/// certificate (`X-BDEW-CERT` / `signing_cert`), validate the certificate
/// against the SM-PKI trust anchor, then call this function.
///
/// # Errors
/// Returns [`Error::Signature`] if the signature is malformed or does not match.
pub fn verify(
    record: &ApiRecord,
    signature_b64: &str,
    verifying_key: &VerifyingKey,
) -> Result<(), Error> {
    let input = signing_input(record)?;
    let sig_bytes = Base64UrlUnpadded::decode_vec(signature_b64)
        .map_err(|e| Error::Signature(format!("base64url decode: {e}")))?;
    let sig = Signature::from_slice(&sig_bytes)
        .map_err(|e| Error::Signature(format!("malformed signature: {e}")))?;
    verifying_key
        .verify(input.as_bytes(), &sig)
        .map_err(|_| Error::Signature("signature verification failed".into()))
}

// ── Key helpers ───────────────────────────────────────────────────────────────

/// Load a PKCS#8 PEM-encoded P-256 private (signing) key.
pub fn signing_key_from_pem(pem: &str) -> Result<SigningKey, Error> {
    SigningKey::from_pkcs8_pem(pem).map_err(|e| Error::Signature(format!("private key: {e}")))
}

/// Load a SPKI PEM-encoded P-256 public (verifying) key.
pub fn verifying_key_from_pem(pem: &str) -> Result<VerifyingKey, Error> {
    VerifyingKey::from_public_key_pem(pem).map_err(|e| Error::Signature(format!("public key: {e}")))
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Build `BASE64URL(protected_header) + "." + BASE64URL(JCS(record))`.
fn signing_input(record: &ApiRecord) -> Result<String, Error> {
    let canonical = serde_jcs::to_string(record)
        .map_err(|e| Error::Signature(format!("JCS serialisation: {e}")))?;
    let payload = Base64UrlUnpadded::encode_string(canonical.as_bytes());
    Ok(format!("{JWS_PROTECTED_HEADER}.{payload}"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use p256::SecretKey;
    use time::OffsetDateTime;
    use url::Url;

    fn sample_record() -> ApiRecord {
        use crate::models::directory::ApiStatus;
        ApiRecord {
            provider_id: "1234567890123".into(),
            api_id: "example".into(),
            major_version: 1,
            url: Url::parse("https://www.example.org/api/resource/v1").unwrap(),
            additional_metadata: None,
            last_updated: OffsetDateTime::from_unix_timestamp(1_727_740_800).unwrap(),
            revision: 1,
            status: ApiStatus::Test,
        }
    }

    #[test]
    fn sign_verify_roundtrip() {
        let secret = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let sk = SigningKey::from(&secret);
        let vk = VerifyingKey::from(&sk);
        let rec = sample_record();
        let sig = sign(&rec, &sk).expect("sign");
        verify(&rec, &sig, &vk).expect("verify");
    }

    #[test]
    fn tampered_record_fails_verification() {
        let secret = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let sk = SigningKey::from(&secret);
        let vk = VerifyingKey::from(&sk);
        let rec = sample_record();
        let sig = sign(&rec, &sk).expect("sign");
        let mut tampered = rec;
        tampered.revision = 999;
        assert!(verify(&tampered, &sig, &vk).is_err());
    }
}
