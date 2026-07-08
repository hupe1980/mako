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

/// Compute HMAC-SHA256 over `body` keyed with `secret` and return as
/// lowercase hex.
///
/// This is a pure function with no I/O.
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
}
