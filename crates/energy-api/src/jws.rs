//! JWS ECDSA-SHA256 sign and verify for EDI-Energy directory records.
//!
//! The directory service uses *JSON Web Signature* (RFC 7515) with a fixed
//! JWS Protected Header and RFC 8785 (JCS) canonical JSON as the payload.
//!
//! ## Algorithm
//!
//! ```text
//! JWS Protected Header (fixed, base64url-encoded):
//!   eyJhbGciOiJodHRwOi8vd3d3LnczLm9yZy8yMDAxLzA0L3htbGRzaWctbW9yZSNlY2RzYS1zaGEyNTYiLCJ0eXAiOiJKV1QifQ
//!
//! Signing input = BASE64URL(header) + "." + BASE64URL(JCS(ApiRecord))
//! Signature     = ECDSA-P256-SHA256(signing_input.as_bytes())
//! Wire format   = base64url(raw_signature_bytes)  — header/payload omitted
//! ```
//!
//! The signing certificate is transmitted in the `X-BDEW-CERT` HTTP response
//! header (REST) or in [`SignedApiRecord::signing_cert`] (WebSocket), encoded
//! per RFC 9440 (`:cert:` bare-item value).

use base64ct::{Base64Url, Base64UrlUnpadded, Encoding};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::ecdsa::signature::{Signer, Verifier};
use p256::pkcs8::{DecodePrivateKey, DecodePublicKey};
use sha2::Digest;

use crate::error::Error;
use crate::types::directory::ApiRecord;

/// Fixed JWS Protected Header for all directory record signatures.
///
/// Decodes to `{"alg":"http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256","typ":"JWT"}`.
///
/// # Deviation from RFC 7518
///
/// The BDEW API-Webdienste Strom specification mandates the W3C XMLDSig URI
/// (`http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256`) as the `alg`
/// identifier rather than the JOSE-standard `"ES256"` (RFC 7518 §3.1).
/// This is intentional and must not be changed to `"ES256"` — standard JWT
/// libraries that validate against JOSE algorithm names will reject these
/// tokens, but BDEW-conformant software expects the XMLDSig URI form.
///
/// Reference: BDEW API-Webdienste Strom, Abschnitt "Signatur", aktuelle Fassung.
pub const JWS_PROTECTED_HEADER: &str =
    "eyJhbGciOiJodHRwOi8vd3d3LnczLm9yZy8yMDAxLzA0L3htbGRzaWctbW9yZSNlY2RzYS1zaGEyNTYiLCJ0eXAiOiJKV1QifQ";

// ── Signing ───────────────────────────────────────────────────────────────────

/// Sign an [`ApiRecord`] and return the base64url-encoded JWS Signature value.
///
/// The caller is responsible for providing the correct signing key (from an
/// EMT.API certificate issued by SM-PKI).
///
/// # Errors
/// Returns [`Error::Signature`] if canonicalization or signing fails.
pub fn sign(record: &ApiRecord, signing_key: &SigningKey) -> Result<String, Error> {
    let signing_input = build_signing_input(record)?;
    let signature: Signature = signing_key.sign(signing_input.as_bytes());
    Ok(Base64UrlUnpadded::encode_string(&signature.to_bytes()))
}

/// Verify a [`SignedApiRecord`]'s signature against the provided DER-encoded
/// public key.
///
/// For production use, extract the public key from the signing certificate
/// (`signing_cert` field / `X-BDEW-CERT` header), validate it against the
/// SM-PKI trust anchors, and then call this function.
///
/// # Errors
/// Returns [`Error::Signature`] if the signature is malformed or invalid.
pub fn verify(
    record: &ApiRecord,
    signature_b64: &str,
    verifying_key: &VerifyingKey,
) -> Result<(), Error> {
    let signing_input = build_signing_input(record)?;
    let sig_bytes = Base64UrlUnpadded::decode_vec(signature_b64)
        .map_err(|e| Error::Signature(format!("base64url decode: {e}")))?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| Error::Signature(format!("malformed signature: {e}")))?;
    verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| Error::Signature("signature verification failed".into()))
}

// ── Key helpers ───────────────────────────────────────────────────────────────

/// Load a PKCS#8 PEM-encoded P-256 private key.
///
/// # Errors
/// Returns [`Error::Signature`] if the PEM data is malformed.
pub fn signing_key_from_pem(pem: &str) -> Result<SigningKey, Error> {
    SigningKey::from_pkcs8_pem(pem)
        .map_err(|e| Error::Signature(format!("private key: {e}")))
}

/// Load a SPKI PEM-encoded P-256 public key.
///
/// # Errors
/// Returns [`Error::Signature`] if the PEM data is malformed.
pub fn verifying_key_from_pem(pem: &str) -> Result<VerifyingKey, Error> {
    VerifyingKey::from_public_key_pem(pem)
        .map_err(|e| Error::Signature(format!("public key: {e}")))
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Construct the JWS signing input:
/// `BASE64URL(header) + "." + BASE64URL(JCS(record))`
fn build_signing_input(record: &ApiRecord) -> Result<String, Error> {
    let canonical = canonical_json(record)?;
    let payload_b64 = Base64UrlUnpadded::encode_string(canonical.as_bytes());
    Ok(format!("{JWS_PROTECTED_HEADER}.{payload_b64}"))
}

/// Serialize `record` to RFC 8785 canonical JSON (JCS).
///
/// This ensures:
/// - Keys sorted lexicographically (per struct field declaration order for
///   fixed fields; HashMap keys sorted for `additional_metadata`).
/// - No whitespace.
/// - Number formatting per IEEE 754.
/// - Unicode escaping per ECMAScript.
fn canonical_json(record: &ApiRecord) -> Result<String, Error> {
    serde_jcs::to_string(record).map_err(|e| Error::Signature(format!("JCS serialization: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;
    use url::Url;

    fn sample_record() -> ApiRecord {
        ApiRecord {
            provider_id: "1234567890123".into(),
            api_id: "example".into(),
            major_version: 1,
            url: Url::parse("https://www.example.org/api/resource/v1").unwrap(),
            additional_metadata: None,
            last_updated: OffsetDateTime::from_unix_timestamp(1_727_740_800).unwrap(),
            revision: 1,
            status: crate::types::directory::ApiStatus::Test,
        }
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        use p256::SecretKey;
        // Generate an ephemeral key for the test
        let secret = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let signing_key = SigningKey::from(&secret);
        let verifying_key = VerifyingKey::from(&signing_key);

        let record = sample_record();
        let sig = sign(&record, &signing_key).expect("sign");
        verify(&record, &sig, &verifying_key).expect("verify");
    }

    #[test]
    fn tampered_record_fails_verification() {
        use p256::SecretKey;
        let secret = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let signing_key = SigningKey::from(&secret);
        let verifying_key = VerifyingKey::from(&signing_key);

        let record = sample_record();
        let sig = sign(&record, &signing_key).expect("sign");

        let mut tampered = record;
        tampered.revision = 999;
        assert!(verify(&tampered, &sig, &verifying_key).is_err());
    }
}
