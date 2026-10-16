//! TR-03116-3 Inhaltsdatensicherung (content-layer security) for electricity API calls.
//!
//! Applies to the **Control Measures API** and the **MaLo Identification API**.
//! The **Directory Service** uses a different scheme — see [`super::jws`].
//!
//! ## Algorithm (Regelungen zum Übertragungsweg für API-Webdienste 1.2, Kap. 3.3)
//!
//! Kap. 3.3 signs „die angesprochene Ressource (URI) inklusive aller
//! Query-Parameter, die http-Kopfzeilen creationDateTime, transactionId und
//! soweit vorhanden initialTransactionId, und die übermittelte Payload", with
//! the algorithms of TR-03116-3 Kap. 8 and 9:
//!
//! | Component | Source |
//! |-----------|--------|
//! | URI | Full request URL, including all query parameters |
//! | Payload | Request body in RFC 8785 canonical JSON form; `&[]` when no body |
//! | `creationDateTime` | Value of the `creationDateTime` request header |
//! | `transactionId` | Value of the `transactionId` request header |
//! | `initialTransactionId` | Value of the `initialTransactionId` header — **only when present** |
//!
//! ```text
//! M         = SHA-256(uri) ‖ SHA-256(payload) ‖ SHA-256(creationDateTime)
//!           ‖ SHA-256(transactionId) [‖ SHA-256(initialTransactionId)]
//! digest    = SHA-256(M)
//! signature = ECDSA-P256(prehash = digest)
//! ```
//!
//! Both values are **standard Base64** (with padding, RFC 4648 §4). The
//! signature travels in [`HEADER_SIGNATURE`] (`X-BDEW-SIGNATURE`), the digest
//! in [`HEADER_DIGEST`] (`X-BDEW-DIGEST`) and the signer's certificate in
//! [`HEADER_CERT`] (`X-BDEW-CERT`, RFC 9440 §2.1 encoding — see
//! [`cert_header_value`]).
//!
//! ## Signing keys
//!
//! Keys must belong to an **EMT.API** certificate issued by the BSI SM-PKI.
//! Use [`signing_key_from_pem`] / [`verifying_key_from_pem`] to load them.
//!
//! ## Client-side usage
//!
//! ```no_run
//! use energy_api::transport::content_security::{self, SignedInputs};
//!
//! let key = content_security::signing_key_from_pem(
//!     "-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----",
//! )?;
//! let inputs = SignedInputs {
//!     uri: "https://msb.example.de/[Post]/steuerbefehl/konfiguration/\
//!           ?locationId=E1234848431&commandControl=%7B...%7D",
//!     payload: &[], // no body for control-measures calls
//!     creation_date_time: "2025-06-01T10:00:00.000Z",
//!     transaction_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
//!     initial_transaction_id: None,
//! };
//! let (digest, sig) = content_security::sign_request(&inputs, &key)?;
//! // set `HEADER_DIGEST: digest`, `HEADER_SIGNATURE: sig` and `HEADER_CERT`
//! # Ok::<(), energy_api::Error>(())
//! ```
//!
//! ## Server-side usage
//!
//! ```no_run
//! use energy_api::transport::content_security::{self, SignedInputs};
//!
//! // verifying_key loaded from the X-BDEW-CERT certificate
//! # let verifying_key = content_security::verifying_key_from_pem(
//! #     "-----BEGIN PUBLIC KEY-----\n...\n-----END PUBLIC KEY-----")?;
//! let inputs = SignedInputs {
//!     uri: "https://my-server.example.de/[Post]/steuerbefehl/konfiguration/?...",
//!     payload: &[],
//!     creation_date_time: "2025-06-01T10:00:00.000Z",
//!     transaction_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
//!     initial_transaction_id: None,
//! };
//! content_security::verify_request(
//!     &inputs,
//!     /* from X-BDEW-DIGEST    */ "...",
//!     /* from X-BDEW-SIGNATURE */ "...",
//!     &verifying_key,
//! )?;
//! # Ok::<(), energy_api::Error>(())
//! ```

use base64ct::{Base64, Encoding};
use p256::ecdsa::signature::hazmat::{PrehashSigner, PrehashVerifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};

/// The ECDSA P-256 key types this module signs and verifies with.
///
/// Re-exported so a consumer needs no direct `p256` dependency of its own: two
/// `p256` versions in one build are two incompatible `SigningKey` types, and
/// the compiler error says only that the types differ.
pub use p256::ecdsa::{SigningKey as ContentSigningKey, VerifyingKey as ContentVerifyingKey};
use p256::pkcs8::{DecodePrivateKey, DecodePublicKey};
use sha2::{Digest as _, Sha256};

use crate::error::Error;

// ── Header names ─────────────────────────────────────────────────────────────

/// HTTP header carrying the signed digest `Hash(M)`, standard Base64
/// (Regelungen zum Übertragungsweg für API-Webdienste 1.2, Kap. 3.3).
pub const HEADER_DIGEST: &str = "X-BDEW-DIGEST";

/// HTTP header carrying the Plain-encoded ECDSA signature (TR-03111), standard
/// Base64 (same chapter).
pub const HEADER_SIGNATURE: &str = "X-BDEW-SIGNATURE";

/// HTTP header carrying the certificate of the signing key, encoded as RFC 9440
/// §2.1 encodes a TLS certificate (same chapter).
pub const HEADER_CERT: &str = "X-BDEW-CERT";

/// The value of [`HEADER_CERT`] for a DER certificate: RFC 9440 §2.1's Byte
/// Sequence — the DER in standard Base64, between colons.
#[must_use]
pub fn cert_header_value(cert_der: &[u8]) -> String {
    format!(":{}:", Base64::encode_string(cert_der))
}

// ── Public API ────────────────────────────────────────────────────────────────

/// The request components Kap. 3.3 signs.
#[derive(Debug, Clone, Copy)]
pub struct SignedInputs<'a> {
    /// Full request URI, **including** URL-encoded query parameters.
    pub uri: &'a str,
    /// RFC 8785 JCS bytes of the request body; `&[]` for an endpoint with no
    /// structured body.
    pub payload: &'a [u8],
    /// Value of the `creationDateTime` request header.
    pub creation_date_time: &'a str,
    /// Value of the `transactionId` request header.
    pub transaction_id: &'a str,
    /// Value of the `initialTransactionId` request header. Kap. 3.3 includes
    /// its hash „nur dann …, wenn diese vorhanden ist" — `None` leaves it out
    /// of `M`, `Some` appends it.
    pub initial_transaction_id: Option<&'a str>,
}

/// Sign an outgoing API request and return Base64-encoded `(digest, signature)`
/// strings for [`HEADER_DIGEST`] and [`HEADER_SIGNATURE`].
///
/// # Errors
///
/// Returns [`Error::Signature`] if ECDSA signing fails.
pub fn sign_request(
    inputs: &SignedInputs<'_>,
    signing_key: &SigningKey,
) -> Result<(String, String), Error> {
    let digest = compute_digest(inputs);
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
/// 1. The received [`HEADER_DIGEST`] matches the independently recomputed digest.
/// 2. The [`HEADER_SIGNATURE`] is a valid ECDSA-P256 signature over that digest.
///
/// `verifying_key` is the EMT.API P-256 public key of the sender's certificate.
///
/// # Errors
///
/// Returns [`Error::Signature`] on any mismatch or decode failure.
pub fn verify_request(
    inputs: &SignedInputs<'_>,
    digest_b64: &str,
    signature_b64: &str,
    verifying_key: &VerifyingKey,
) -> Result<(), Error> {
    let expected = compute_digest(inputs);

    // 1. Verify the digest header matches.
    let received = Base64::decode_vec(digest_b64)
        .map_err(|e| Error::Signature(format!("{HEADER_DIGEST} base64 decode: {e}")))?;
    if received.as_slice() != expected {
        return Err(Error::Signature(format!(
            "{HEADER_DIGEST} does not match the request components"
        )));
    }

    // 2. Verify the ECDSA signature.
    let sig_bytes = Base64::decode_vec(signature_b64)
        .map_err(|e| Error::Signature(format!("{HEADER_SIGNATURE} base64 decode: {e}")))?;
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

/// Compute the digest `Hash(M)` over the signed request components.
///
/// ```text
/// Hash(SHA-256(uri) ‖ SHA-256(payload) ‖ SHA-256(dt) ‖ SHA-256(tx_id) [‖ SHA-256(initial_tx_id)])
/// ```
///
/// This function is `pub` so callers can inspect the digest bytes directly
/// (e.g. for logging or debugging) without going through the full sign/verify flow.
#[must_use]
pub fn compute_digest(inputs: &SignedInputs<'_>) -> [u8; 32] {
    let mut m = Sha256::new();
    m.update(sha256(inputs.uri.as_bytes()));
    m.update(sha256(inputs.payload));
    m.update(sha256(inputs.creation_date_time.as_bytes()));
    m.update(sha256(inputs.transaction_id.as_bytes()));
    if let Some(initial) = inputs.initial_transaction_id {
        m.update(sha256(initial.as_bytes()));
    }
    m.finalize().into()
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

    fn inputs<'a>(uri: &'a str, payload: &'a [u8], dt: &'a str, tx: &'a str) -> SignedInputs<'a> {
        SignedInputs {
            uri,
            payload,
            creation_date_time: dt,
            transaction_id: tx,
            initial_transaction_id: None,
        }
    }

    #[test]
    fn roundtrip_no_body() {
        let key = test_signing_key();
        let vk = *key.verifying_key();
        let i = inputs(
            "https://msb.example.de/[Post]/steuerbefehl/konfiguration/?locationId=E1234848431",
            &[],
            "2025-06-01T10:00:00.000Z",
            "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
        );
        let (d, s) = sign_request(&i, &key).expect("sign");
        verify_request(&i, &d, &s, &vk).expect("verify");
    }

    #[test]
    fn roundtrip_with_body() {
        let key = test_signing_key();
        let vk = *key.verifying_key();
        let body =
            br#"{"identificationDateTime":"2025-06-01T22:00:00Z","energyDirection":"consumption"}"#;
        let i = inputs(
            "https://nb.example.de/maloId/request/v1",
            body,
            "2025-06-01T10:00:00.000Z",
            "aabbccdd-eeff-0011-2233-445566778899",
        );
        let (d, s) = sign_request(&i, &key).expect("sign");
        verify_request(&i, &d, &s, &vk).expect("verify");
    }

    #[test]
    fn digest_mismatch_is_rejected() {
        let key = test_signing_key();
        let vk = *key.verifying_key();
        let (digest_b64, sig_b64) = sign_request(
            &inputs(
                "https://example.de/api?x=1",
                b"body",
                "2025-06-01T10:00:00Z",
                "tx-000",
            ),
            &key,
        )
        .expect("sign");
        // Tampered URI — digest should not match.
        assert!(
            verify_request(
                &inputs(
                    "https://example.de/api?x=TAMPERED",
                    b"body",
                    "2025-06-01T10:00:00Z",
                    "tx-000"
                ),
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
        let i = inputs(
            "https://example.de/api",
            &[],
            "2025-06-01T10:00:00Z",
            "tx-000",
        );
        let (digest_b64, _) = sign_request(&i, &key).expect("sign");
        // Correct digest, wrong signature.
        let bad_sig = Base64::encode_string(&[0u8; 64]);
        assert!(verify_request(&i, &digest_b64, &bad_sig, &vk).is_err());
    }

    #[test]
    fn compute_digest_is_deterministic() {
        let i = inputs("uri", b"body", "dt", "tx");
        assert_eq!(compute_digest(&i), compute_digest(&i));
    }

    #[test]
    fn empty_and_non_empty_payload_differ() {
        assert_ne!(
            compute_digest(&inputs("uri", &[], "dt", "tx")),
            compute_digest(&inputs("uri", b"x", "dt", "tx"))
        );
    }

    /// Kap. 3.3: `M = Hash(URI) + Hash(Payload) + Hash(creationDateTime) +
    /// Hash(transactionId) + Hash(initialTransactionId)`, the last term only
    /// when the header is present. A digest that ignored it would let the
    /// header be altered under a valid signature.
    #[test]
    fn the_initial_transaction_id_is_signed_when_present() {
        let without = inputs("uri", b"body", "dt", "tx");
        let with = SignedInputs {
            initial_transaction_id: Some("first-tx"),
            ..without
        };
        assert_ne!(compute_digest(&without), compute_digest(&with));
        let mut m = Vec::new();
        for part in [&b"uri"[..], b"body", b"dt", b"tx", b"first-tx"] {
            m.extend_from_slice(&sha256(part));
        }
        assert_eq!(
            compute_digest(&with),
            sha256(&m),
            "Hash(M) over the five hashes"
        );

        let key = test_signing_key();
        let (d, s) = sign_request(&with, &key).expect("sign");
        let altered = SignedInputs {
            initial_transaction_id: Some("other-tx"),
            ..with
        };
        assert!(verify_request(&altered, &d, &s, key.verifying_key()).is_err());
    }

    /// The header names Kap. 3.3 (S. 5) publishes.
    #[test]
    fn the_headers_carry_the_published_names() {
        assert_eq!(HEADER_DIGEST, "X-BDEW-DIGEST");
        assert_eq!(HEADER_SIGNATURE, "X-BDEW-SIGNATURE");
        assert_eq!(HEADER_CERT, "X-BDEW-CERT");
        assert_eq!(cert_header_value(&[1, 2, 3]), ":AQID:");
    }
}
