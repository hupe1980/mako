//! TR-03116-3 Inhaltsdatensicherung (content-layer security) for electricity API calls.
//!
//! Applies to the **Control Measures API** and the **MaLo Identification API**.
//! The **Directory Service** uses a different scheme — see [`super::jws`].
//!
//! ## Algorithm (TR-03116-3 chapter 9, referenced by `controlMeasuresV1.yaml`)
//!
//! Four components are signed per request:
//!
//! | Component | Source |
//! |-----------|--------|
//! | URI | Full request URL, including all query parameters |
//! | Payload | Request body in RFC 8785 canonical JSON form; `&[]` when no body |
//! | `creationDateTime` | Value of the `creationDateTime` request header |
//! | `transactionId` | Value of the `transactionId` request header |
//!
//! ```text
//! digest = SHA-256(
//!              SHA-256(uri_bytes)
//!            ‖ SHA-256(canonical_payload_bytes)
//!            ‖ SHA-256(creation_dt_bytes)
//!            ‖ SHA-256(tx_id_bytes)
//!          )
//! signature = ECDSA-P256(prehash = digest)
//! ```
//!
//! Both values are **standard Base64** (with padding, RFC 4648 §4) and are
//! transmitted as HTTP headers [`HEADER_DIGEST`] and [`HEADER_SIGNATURE`].
//!
//! ## Signing keys
//!
//! Keys must belong to an **EMT.API** certificate issued by the BSI SM-PKI.
//! Use [`signing_key_from_pem`] / [`verifying_key_from_pem`] to load them.
//!
//! ## Client-side usage
//!
//! ```no_run
//! # #[cfg(feature = "crypto")] {
//! use energy_api::transport::content_security::{self, HEADER_DIGEST, HEADER_SIGNATURE};
//!
//! let key = content_security::signing_key_from_pem(
//!     "-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----",
//! )?;
//! let uri = "https://msb.example.de/[Post]/steuerbefehl/konfiguration/\
//!             ?locationId=E1234848431&commandControl=%7B...%7D";
//! let (digest, sig) = content_security::sign_request(
//!     uri,
//!     &[],                                      // no body for control-measures calls
//!     "2025-06-01T10:00:00.000Z",
//!     "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
//!     &key,
//! )?;
//! // add `HEADER_DIGEST: digest` and `HEADER_SIGNATURE: sig` to the outgoing request
//! # }
//! # Ok::<(), energy_api::Error>(())
//! ```
//!
//! ## Server-side usage
//!
//! ```no_run
//! # #[cfg(feature = "crypto")] {
//! use energy_api::transport::content_security;
//!
//! // verifying_key loaded from the X-BDEW-CERT / DER certificate in the TLS handshake
//! # let verifying_key = content_security::verifying_key_from_pem(
//! #     "-----BEGIN PUBLIC KEY-----\n...\n-----END PUBLIC KEY-----")?;
//! content_security::verify_request(
//!     "https://my-server.example.de/[Post]/steuerbefehl/konfiguration/?...",
//!     &[],
//!     "2025-06-01T10:00:00.000Z",
//!     "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
//!     /* digest_b64 from DIGEST header   */ "...",
//!     /* sig_b64   from SIGNATURE header */ "...",
//!     &verifying_key,
//! )?;
//! # }
//! # Ok::<(), energy_api::Error>(())
//! ```
//!
//! Feature gate: `crypto`.

use base64ct::{Base64, Encoding};
use p256::ecdsa::signature::hazmat::{PrehashSigner, PrehashVerifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::{DecodePrivateKey, DecodePublicKey};
use sha2::{Digest as _, Sha256};

use crate::error::Error;

// ── Header names ─────────────────────────────────────────────────────────────

/// HTTP header name carrying the Base64-encoded (standard, padded) SHA-256 digest.
///
/// Value: `"DIGEST"`
pub const HEADER_DIGEST: &str = "DIGEST";

/// HTTP header name carrying the Base64-encoded (standard, padded) ECDSA-P256 signature.
///
/// Value: `"SIGNATURE"`
pub const HEADER_SIGNATURE: &str = "SIGNATURE";

// ── Public API ────────────────────────────────────────────────────────────────

/// Sign an outgoing API request and return Base64-encoded `(digest, signature)` strings
/// ready to be set as [`HEADER_DIGEST`] and [`HEADER_SIGNATURE`] HTTP headers.
///
/// # Arguments
///
/// - `uri` — full request URI, **including** URL-encoded query parameters.
/// - `canonical_payload` — RFC 8785 JCS bytes of the request body;
///   pass `&[]` for endpoints with no structured body.
/// - `creation_dt` — value of the `creationDateTime` request header (ISO 8601 UTC).
/// - `tx_id` — value of the `transactionId` request header (UUID string).
/// - `signing_key` — EMT.API P-256 private key from the SM-PKI.
///
/// # Errors
///
/// Returns [`Error::Signature`] if ECDSA signing fails.
pub fn sign_request(
    uri: &str,
    canonical_payload: &[u8],
    creation_dt: &str,
    tx_id: &str,
    signing_key: &SigningKey,
) -> Result<(String, String), Error> {
    let digest = compute_digest(uri, canonical_payload, creation_dt, tx_id);
    let sig: Signature = signing_key
        .sign_prehash(&digest)
        .map_err(|e| Error::Signature(format!("ECDSA prehash sign: {e}")))?;
    Ok((
        Base64::encode_string(&digest),
        Base64::encode_string(&sig.to_bytes()),
    ))
}

/// Verify the content-security headers of an incoming API request.
///
/// Checks that:
/// 1. The received `DIGEST` header matches the independently recomputed digest.
/// 2. The `SIGNATURE` header is a valid ECDSA-P256 signature over that digest.
///
/// # Arguments
///
/// - `uri` — full request URI, **including** URL-encoded query parameters.
/// - `canonical_payload` — RFC 8785 JCS bytes of the request body;
///   pass `&[]` for endpoints with no structured body.
/// - `creation_dt` — value of the `creationDateTime` request header.
/// - `tx_id` — value of the `transactionId` request header.
/// - `digest_b64` — value of the `DIGEST` HTTP header (standard Base64).
/// - `signature_b64` — value of the `SIGNATURE` HTTP header (standard Base64).
/// - `verifying_key` — EMT.API P-256 public key extracted from the sender's certificate.
///
/// # Errors
///
/// Returns [`Error::Signature`] on any mismatch or decode failure.
pub fn verify_request(
    uri: &str,
    canonical_payload: &[u8],
    creation_dt: &str,
    tx_id: &str,
    digest_b64: &str,
    signature_b64: &str,
    verifying_key: &VerifyingKey,
) -> Result<(), Error> {
    let expected = compute_digest(uri, canonical_payload, creation_dt, tx_id);

    // 1. Verify the digest header matches.
    let received = Base64::decode_vec(digest_b64)
        .map_err(|e| Error::Signature(format!("DIGEST base64 decode: {e}")))?;
    if received.as_slice() != expected {
        return Err(Error::Signature(
            "DIGEST header does not match request components".into(),
        ));
    }

    // 2. Verify the ECDSA signature.
    let sig_bytes = Base64::decode_vec(signature_b64)
        .map_err(|e| Error::Signature(format!("SIGNATURE base64 decode: {e}")))?;
    let sig = Signature::from_slice(&sig_bytes)
        .map_err(|e| Error::Signature(format!("malformed ECDSA signature bytes: {e}")))?;
    verifying_key
        .verify_prehash(&expected, &sig)
        .map_err(|_| Error::Signature("ECDSA signature verification failed".into()))
}

/// Serialise a `serde`-serialisable value as RFC 8785 canonical JSON bytes.
///
/// Use the result as `canonical_payload` in [`sign_request`] / [`verify_request`]
/// for endpoints that carry a structured JSON body (e.g. MaLo-ID requests).
///
/// # Errors
///
/// Returns [`Error::Signature`] if JSON canonicalisation fails.
pub fn canonical_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    serde_jcs::to_string(value)
        .map(std::string::String::into_bytes)
        .map_err(|e| Error::Signature(format!("RFC 8785 JCS canonicalisation: {e}")))
}

// ── Key helpers ───────────────────────────────────────────────────────────────

/// Load a PKCS#8 PEM-encoded P-256 private (signing) key.
///
/// The key must belong to an EMT.API certificate issued by the BSI SM-PKI.
///
/// # Errors
///
/// Returns [`Error::Signature`] if the PEM cannot be decoded.
pub fn signing_key_from_pem(pem: &str) -> Result<SigningKey, Error> {
    SigningKey::from_pkcs8_pem(pem)
        .map_err(|e| Error::Signature(format!("content-security private key: {e}")))
}

/// Load a SubjectPublicKeyInfo (SPKI) PEM-encoded P-256 public (verifying) key.
///
/// # Errors
///
/// Returns [`Error::Signature`] if the PEM cannot be decoded.
pub fn verifying_key_from_pem(pem: &str) -> Result<VerifyingKey, Error> {
    VerifyingKey::from_public_key_pem(pem)
        .map_err(|e| Error::Signature(format!("content-security public key: {e}")))
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Compute the two-level SHA-256 digest over the four signed request components.
///
/// ```text
/// digest = SHA-256(SHA-256(uri) ‖ SHA-256(payload) ‖ SHA-256(dt) ‖ SHA-256(tx_id))
/// ```
///
/// This function is `pub` so callers can inspect the digest bytes directly
/// (e.g. for logging or debugging) without going through the full sign/verify flow.
pub fn compute_digest(
    uri: &str,
    canonical_payload: &[u8],
    creation_dt: &str,
    tx_id: &str,
) -> [u8; 32] {
    let mut combined = [0u8; 128]; // four × 32-byte inner hashes
    combined[0..32].copy_from_slice(&sha256(uri.as_bytes()));
    combined[32..64].copy_from_slice(&sha256(canonical_payload));
    combined[64..96].copy_from_slice(&sha256(creation_dt.as_bytes()));
    combined[96..128].copy_from_slice(&sha256(tx_id.as_bytes()));
    sha256(&combined)
}

#[inline]
fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6979 §A.2.5 P-256/SHA-256 test private key scalar.
    fn test_signing_key() -> SigningKey {
        let d: [u8; 32] = [
            0xC9, 0xAF, 0xA9, 0xD8, 0x45, 0xBA, 0x75, 0x16, 0x6B, 0x5C, 0x21, 0x57, 0x67, 0xB1,
            0xD6, 0x93, 0x4E, 0x50, 0xC3, 0xDB, 0x36, 0xE8, 0x9B, 0x12, 0x7B, 0x8A, 0x62, 0x2B,
            0x12, 0x0F, 0x67, 0x21,
        ];
        let secret = p256::SecretKey::from_bytes((&d).into()).expect("valid RFC 6979 scalar");
        SigningKey::from(&secret)
    }

    #[test]
    fn roundtrip_no_body() {
        let key = test_signing_key();
        let vk = *key.verifying_key();

        let (d, s) = sign_request(
            "https://msb.example.de/[Post]/steuerbefehl/konfiguration/?locationId=E1234848431",
            &[],
            "2025-06-01T10:00:00.000Z",
            "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            &key,
        )
        .expect("sign");

        verify_request(
            "https://msb.example.de/[Post]/steuerbefehl/konfiguration/?locationId=E1234848431",
            &[],
            "2025-06-01T10:00:00.000Z",
            "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            &d,
            &s,
            &vk,
        )
        .expect("verify");
    }

    #[test]
    fn roundtrip_with_body() {
        let key = test_signing_key();
        let vk = *key.verifying_key();
        let body =
            br#"{"identificationDateTime":"2025-06-01T22:00:00Z","energyDirection":"consumption"}"#;

        let (d, s) = sign_request(
            "https://nb.example.de/maloId/request/v1",
            body,
            "2025-06-01T10:00:00.000Z",
            "aabbccdd-eeff-0011-2233-445566778899",
            &key,
        )
        .expect("sign");

        verify_request(
            "https://nb.example.de/maloId/request/v1",
            body,
            "2025-06-01T10:00:00.000Z",
            "aabbccdd-eeff-0011-2233-445566778899",
            &d,
            &s,
            &vk,
        )
        .expect("verify");
    }

    #[test]
    fn digest_mismatch_is_rejected() {
        let key = test_signing_key();
        let vk = *key.verifying_key();

        let (digest_b64, sig_b64) = sign_request(
            "https://example.de/api?x=1",
            b"body",
            "2025-06-01T10:00:00Z",
            "tx-000",
            &key,
        )
        .expect("sign");

        // Tampered URI — digest should not match.
        assert!(
            verify_request(
                "https://example.de/api?x=TAMPERED",
                b"body",
                "2025-06-01T10:00:00Z",
                "tx-000",
                &digest_b64,
                &sig_b64,
                &vk,
            )
            .is_err()
        );
    }

    #[test]
    fn signature_mismatch_is_rejected() {
        let key = test_signing_key();
        let vk = *key.verifying_key();

        let (digest_b64, _) = sign_request(
            "https://example.de/api",
            &[],
            "2025-06-01T10:00:00Z",
            "tx-000",
            &key,
        )
        .expect("sign");

        // Correct digest, wrong signature.
        let bad_sig = Base64::encode_string(&[0u8; 64]);
        assert!(
            verify_request(
                "https://example.de/api",
                &[],
                "2025-06-01T10:00:00Z",
                "tx-000",
                &digest_b64,
                &bad_sig,
                &vk,
            )
            .is_err()
        );
    }

    #[test]
    fn compute_digest_is_deterministic() {
        let d1 = compute_digest("uri", b"body", "dt", "tx");
        let d2 = compute_digest("uri", b"body", "dt", "tx");
        assert_eq!(d1, d2);
    }

    #[test]
    fn empty_and_non_empty_payload_differ() {
        let d_empty = compute_digest("uri", &[], "dt", "tx");
        let d_body = compute_digest("uri", b"x", "dt", "tx");
        assert_ne!(d_empty, d_body);
    }
}
