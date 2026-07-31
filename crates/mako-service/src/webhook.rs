//! Webhook HMAC-SHA256 signature verification.
//!
//! Incoming webhook requests are signed by the sender using an HMAC-SHA256
//! over the raw request body.  The signature is delivered in the
//! `X-Mako-Signature` header as a lowercase hex string.
//!
//! ## Header format
//!
//! ```text
//! X-Mako-Signature: sha256=<hex_digest>
//! ```
//!
//! The `sha256=` prefix is optional — a bare 64-char hex string is also
//! accepted for compatibility with existing deployments.
//!
//! ## Constant-time comparison
//!
//! Signature comparison uses [`subtle::ConstantTimeEq`] to prevent
//! timing side-channels.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// The canonical HMAC signature header. Lowercase (HTTP headers are
/// case-insensitive, but a single spelling keeps producers and log/grep tooling
/// consistent). Emitters set it via [`sign`]; verifiers read it via
/// [`verify_request`].
pub const SIGNATURE_HEADER: &str = "x-mako-signature";

/// Verify an HMAC-SHA256 `X-Mako-Signature` header.
///
/// Returns `true` when `provided` matches the HMAC-SHA256 of `body` keyed
/// with `secret`.
///
/// `provided` may be:
/// - a bare 64-character lowercase hex string, or
/// - prefixed with `sha256=` (e.g. `sha256=abc123…`).
///
/// Returns `false` (never panics) when the header is missing, malformed, or
/// the signature does not match.
#[must_use]
pub fn verify_hmac(secret: &[u8], body: &[u8], provided: &str) -> bool {
    let provided = provided.trim_start_matches("sha256=");
    let expected = hmac_hex(secret, body);
    // Constant-time comparison to prevent timing side-channels
    expected.as_bytes().ct_eq(provided.as_bytes()).into()
}

/// Verify an inbound webhook's [`SIGNATURE_HEADER`] against the raw body.
///
/// This is the one-call inbound guard every `CloudEvent` handler shares: it reads
/// the header, verifies it in constant time, and maps the outcome to a status.
///
/// - `secret = None` → `Ok(())` (verification disabled / dev mode).
/// - header present and valid → `Ok(())`.
/// - header missing, malformed, or mismatched → `Err(StatusCode::UNAUTHORIZED)`.
///
/// # Errors
///
/// Returns [`axum::http::StatusCode::UNAUTHORIZED`] when a secret is configured
/// but the signature does not verify.
pub fn verify_request(
    secret: Option<&[u8]>,
    headers: &axum::http::HeaderMap,
    body: &[u8],
) -> Result<(), axum::http::StatusCode> {
    let Some(secret) = secret else {
        return Ok(());
    };
    let provided = headers
        .get(SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if verify_hmac(secret, body, provided) {
        Ok(())
    } else {
        Err(axum::http::StatusCode::UNAUTHORIZED)
    }
}

/// Sign `body` for an outbound webhook: HMAC-SHA256 keyed with `secret`, in the
/// canonical `X-Mako-Signature` header value format `sha256=<hex>`.
///
/// This is the ONE signer every emitter uses, so the `sha256=` prefix is never
/// omitted (a bare-hex emitter targeting a prefix-requiring verifier is the
/// class of bug this replaces). Pairs with [`verify_hmac`], which accepts this
/// value (and, tolerantly, a bare-hex one).
#[must_use]
pub fn sign(secret: &[u8], body: &[u8]) -> String {
    format!("sha256={}", hmac_hex(secret, body))
}

/// Compute HMAC-SHA256 over `body` keyed with `secret` and return as
/// lowercase hex.
///
/// This is a pure function with no I/O. Prefer [`sign`] for outbound headers —
/// it adds the canonical `sha256=` prefix.
#[must_use]
pub fn hmac_hex(secret: &[u8], body: &[u8]) -> String {
    // HMAC accepts keys of any size; new_from_slice never fails.
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any length");
    mac.update(body);
    let result = mac.finalize().into_bytes();
    hex::encode(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 HMAC-SHA256 test vector #2.
    ///
    /// Key: "Jefe" (4 bytes)
    /// Data: "what do ya want for nothing?"
    /// Expected: 5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843
    /// (verified against Python `hmac.new(b"Jefe", ..., hashlib.sha256).hexdigest()`)
    #[test]
    fn hmac_rfc4231_vector() {
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let hex = hmac_hex(key, data);
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn verify_bare_hex() {
        let secret = b"s3cr3t";
        let body = b"hello world";
        let sig = hmac_hex(secret, body);
        assert!(verify_hmac(secret, body, &sig));
    }

    #[test]
    fn verify_with_sha256_prefix() {
        let secret = b"s3cr3t";
        let body = b"hello world";
        let sig = format!("sha256={}", hmac_hex(secret, body));
        assert!(verify_hmac(secret, body, &sig));
    }

    #[test]
    fn verify_wrong_secret_fails() {
        let body = b"hello world";
        let sig = hmac_hex(b"right_secret", body);
        assert!(!verify_hmac(b"wrong_secret", body, &sig));
    }

    #[test]
    fn verify_tampered_body_fails() {
        let secret = b"s3cr3t";
        let sig = hmac_hex(secret, b"original");
        assert!(!verify_hmac(secret, b"tampered", &sig));
    }

    #[test]
    fn verify_empty_provided_fails() {
        let secret = b"s3cr3t";
        let body = b"hello";
        assert!(!verify_hmac(secret, body, ""));
    }

    #[test]
    fn sign_prefixes_and_round_trips() {
        let secret = b"s3cr3t";
        let body = b"hello world";
        let header = sign(secret, body);
        assert_eq!(header, format!("sha256={}", hmac_hex(secret, body)));
        assert!(verify_hmac(secret, body, &header));
    }

    #[test]
    fn sign_accepts_empty_secret_and_body() {
        // `new_from_slice` accepts a zero-length key; guard the `expect`.
        let header = sign(b"", b"");
        assert!(verify_hmac(b"", b"", &header));
    }

    #[test]
    fn verify_rejects_non_hex_and_wrong_length() {
        let secret = b"s3cr3t";
        let body = b"hello";
        assert!(!verify_hmac(secret, body, "sha256=zzzz"));
        assert!(!verify_hmac(secret, body, "sha256=deadbeef")); // right charset, wrong length
    }

    #[test]
    fn verify_is_case_sensitive() {
        // The digest is lowercase hex; an uppercase signature does not match.
        let secret = b"s3cr3t";
        let body = b"hello";
        let upper = hmac_hex(secret, body).to_uppercase();
        assert!(!verify_hmac(secret, body, &upper));
    }

    #[test]
    fn verify_request_covers_the_inbound_paths() {
        use axum::http::{HeaderMap, HeaderValue, StatusCode};
        let secret = b"s3cr3t";
        let body = b"{\"type\":\"de.x.y\"}";

        // No secret configured → accept (dev mode).
        assert!(verify_request(None, &HeaderMap::new(), body).is_ok());

        // Valid signature → accept.
        let mut headers = HeaderMap::new();
        headers.insert(
            SIGNATURE_HEADER,
            HeaderValue::from_str(&sign(secret, body)).unwrap(),
        );
        assert!(verify_request(Some(secret), &headers, body).is_ok());

        // Missing header → 401.
        assert_eq!(
            verify_request(Some(secret), &HeaderMap::new(), body),
            Err(StatusCode::UNAUTHORIZED)
        );

        // Tampered body → 401.
        assert_eq!(
            verify_request(Some(secret), &headers, b"tampered"),
            Err(StatusCode::UNAUTHORIZED)
        );
    }
}
