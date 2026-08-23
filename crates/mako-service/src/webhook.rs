//! [Standard Webhooks] signing and verification — the one scheme every mako
//! outbound carries and every mako receiver checks.
//!
//! ## The wire format
//!
//! ```text
//! webhook-id:        de.mako.process.initiated/3f2b…      ← also the dedup key
//! webhook-timestamp: 1786012800                           ← seconds since epoch
//! webhook-signature: v1,K5oT9r…                           ← base64, space-separated list
//! ```
//!
//! The signature is `base64(HMAC-SHA256(secret, "{id}.{timestamp}.{body}"))`.
//!
//! ## Why not a bare HMAC over the body
//!
//! The `sha256=<hex>`-over-the-body shape is simpler and worse on three counts:
//!
//! - **It authenticates bytes, not freshness.** A captured POST replays forever
//!   and stays valid, and the only defence is whatever each receiver happens to
//!   do about the message id. Binding the id and the timestamp *into the signed
//!   material* makes replay protection part of the contract, and
//!   [`verify_request`] enforces it so no receiver gets the choice.
//! - **Key rotation is inexpressible.** One header holding one signature means a
//!   secret can only change by breaking every receiver at once. This header is a
//!   **space-separated list**, so a rollover presents both and either verifies.
//! - **Every integrator writes the verifier themselves.** Standard Webhooks is a
//!   published spec with off-the-shelf implementations, which is worth more to
//!   an ERP team than a header only we document.
//!
//! ## What a receiver must not do itself
//!
//! Refusing a stale timestamp and deduplicating on the id are the interesting
//! half of the check, and the half a second implementation gets wrong. Both are
//! here: [`verify_request`] refuses outside [`TOLERANCE`] and hands the caller
//! back the [`WebhookId`] to deduplicate on, so the only way to skip either is
//! to ignore a return value.
//!
//! ## Constant-time comparison
//!
//! Signature comparison uses [`subtle::ConstantTimeEq`] to prevent timing
//! side-channels.
//!
//! [Standard Webhooks]: https://www.standardwebhooks.com/

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// The unique message id, and the key a receiver deduplicates on.
pub const ID_HEADER: &str = "webhook-id";
/// Seconds since the Unix epoch, as a decimal string.
pub const TIMESTAMP_HEADER: &str = "webhook-timestamp";
/// One or more space-separated `v1,<base64>` signatures.
pub const SIGNATURE_HEADER: &str = "webhook-signature";

/// The only signature version this implementation produces or accepts.
///
/// `v1` is symmetric HMAC-SHA256. The spec reserves `v1a` for asymmetric
/// signatures; an unknown version in the list is skipped rather than refused, so
/// a sender that offers several is not rejected for offering one we do not read.
const VERSION: &str = "v1";

/// How far a `webhook-timestamp` may be from ours before the request is refused.
///
/// Five minutes each way, as the spec recommends. Both directions matter: too
/// old is a replay, and too far in the future is a sender whose clock is wrong
/// in a way that would let a captured request stay valid past the window.
pub const TOLERANCE: std::time::Duration = std::time::Duration::from_secs(300);

/// A verified message id, for the receiver to deduplicate on.
///
/// Returned by [`verify_request`] rather than left in the headers, because
/// "authenticate the request" and "do not process it twice" are one obligation
/// and separating them is how the second half gets forgotten.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WebhookId(pub String);

impl WebhookId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WebhookId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why an inbound webhook was refused.
///
/// Named variants rather than a bare status so a receiver can log *which* check
/// failed — "the signature did not match" and "this arrived forty minutes ago"
/// are different operational problems with the same HTTP code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WebhookError {
    #[error("missing or malformed `{0}` header")]
    MissingHeader(&'static str),
    #[error(
        "`webhook-timestamp` is outside the {} s tolerance — a replayed or badly \
         clocked request",
        TOLERANCE.as_secs()
    )]
    StaleTimestamp,
    #[error("no `v1` signature matched")]
    SignatureMismatch,
}

impl From<WebhookError> for axum::http::StatusCode {
    fn from(_: WebhookError) -> Self {
        Self::UNAUTHORIZED
    }
}

/// The bytes a signature covers: `{id}.{timestamp}.{body}`.
///
/// Derived here and nowhere else. Signer and verifier agreeing byte for byte
/// about what is signed is the whole guarantee, and a second place building this
/// string is a second answer.
fn signed_payload(id: &str, timestamp: i64, body: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(id.len() + 24 + body.len());
    payload.extend_from_slice(id.as_bytes());
    payload.push(b'.');
    payload.extend_from_slice(timestamp.to_string().as_bytes());
    payload.push(b'.');
    payload.extend_from_slice(body);
    payload
}

/// Compute the `v1,<base64>` signature for one secret.
#[must_use]
pub fn sign(secret: &[u8], id: &str, timestamp: i64, body: &[u8]) -> String {
    // HMAC accepts keys of any size; new_from_slice never fails.
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any length");
    mac.update(&signed_payload(id, timestamp, body));
    let digest = mac.finalize().into_bytes();
    format!(
        "{VERSION},{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// The three headers an outbound request carries.
///
/// Returned together because sending two of the three produces a request that
/// fails verification for a reason nobody can read off the wire.
#[must_use]
pub fn headers(
    secret: &[u8],
    id: &str,
    timestamp: i64,
    body: &[u8],
) -> [(&'static str, String); 3] {
    [
        (ID_HEADER, id.to_owned()),
        (TIMESTAMP_HEADER, timestamp.to_string()),
        (SIGNATURE_HEADER, sign(secret, id, timestamp, body)),
    ]
}

/// Verify one signature-header value against a secret.
///
/// The header is a **space-separated list**, so a sender rotating a key may
/// present several. Any matching `v1` entry accepts; entries of another version
/// are skipped rather than refused.
#[must_use]
pub fn verify_signature(
    secret: &[u8],
    id: &str,
    timestamp: i64,
    body: &[u8],
    header: &str,
) -> bool {
    let expected = sign(secret, id, timestamp, body);
    let expected = expected.as_bytes();
    let prefix = format!("{VERSION},");
    header
        .split_whitespace()
        .filter(|entry| entry.starts_with(&prefix))
        .any(|entry| bool::from(entry.as_bytes().ct_eq(expected)))
}

/// Verify an inbound webhook and return the id to deduplicate on.
///
/// This is the one-call inbound guard every `CloudEvent` handler shares. It
/// checks all three things a receiver owes: the signature, the timestamp's
/// freshness, and — by handing the id back — that the caller has what it needs
/// not to process the same message twice.
///
/// `secret = None` disables verification (dev mode) and yields whatever
/// `webhook-id` the request carried, or `None` when it carried none.
///
/// # Errors
///
/// [`WebhookError::MissingHeader`] when a required header is absent or
/// unparseable, [`WebhookError::StaleTimestamp`] outside [`TOLERANCE`], and
/// [`WebhookError::SignatureMismatch`] when no `v1` entry matches. All three map
/// to `401`.
pub fn verify_request(
    secret: Option<&[u8]>,
    headers: &axum::http::HeaderMap,
    body: &[u8],
) -> Result<Option<WebhookId>, WebhookError> {
    let header = |name: &'static str| -> Result<&str, WebhookError> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .ok_or(WebhookError::MissingHeader(name))
    };

    let Some(secret) = secret else {
        // Dev mode verifies nothing, but still surfaces the id: a receiver that
        // deduplicates only when a secret happens to be configured is a receiver
        // that behaves differently in the environment nobody tests.
        return Ok(header(ID_HEADER).ok().map(|id| WebhookId(id.to_owned())));
    };

    let id = header(ID_HEADER)?;
    let timestamp: i64 = header(TIMESTAMP_HEADER)?
        .trim()
        .parse()
        .map_err(|_| WebhookError::MissingHeader(TIMESTAMP_HEADER))?;

    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let skew = now.saturating_sub(timestamp).unsigned_abs();
    if skew > TOLERANCE.as_secs() {
        return Err(WebhookError::StaleTimestamp);
    }

    if verify_signature(secret, id, timestamp, body, header(SIGNATURE_HEADER)?) {
        Ok(Some(WebhookId(id.to_owned())))
    } else {
        Err(WebhookError::SignatureMismatch)
    }
}

/// [`verify_request`], discarding the id, for a handler that deduplicates
/// elsewhere (or is genuinely idempotent).
///
/// Separate and explicit: dropping the id has to be a decision somebody wrote
/// down, not the shape of the default call.
///
/// # Errors
///
/// As [`verify_request`].
pub fn verify_request_only(
    secret: Option<&[u8]>,
    headers: &axum::http::HeaderMap,
    body: &[u8],
) -> Result<(), axum::http::StatusCode> {
    verify_request(secret, headers, body)
        .map(|_| ())
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};

    fn now() -> i64 {
        time::OffsetDateTime::now_utc().unix_timestamp()
    }

    /// The tolerance as a signed offset, for building out-of-window fixtures.
    fn tolerance_secs() -> i64 {
        i64::try_from(TOLERANCE.as_secs()).expect("the tolerance is minutes, not epochs")
    }

    fn signed(secret: &[u8], id: &str, ts: i64, body: &[u8]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in headers(secret, id, ts, body) {
            h.insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(&value).expect("ascii"),
            );
        }
        h
    }

    /// The [Standard Webhooks] reference vector, from the specification's own
    /// example. Pinning it is what makes "we implement the spec" checkable
    /// rather than asserted — a receiver in any other language must agree.
    ///
    /// [Standard Webhooks]: https://www.standardwebhooks.com/
    #[test]
    fn the_reference_vector_matches() {
        let secret = base64::engine::general_purpose::STANDARD
            .decode("MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw")
            .expect("the spec's secret is base64");
        let sig = sign(
            &secret,
            "msg_p5jXN8AQM9LWM0D4loKWxJek",
            1_614_265_330,
            br#"{"test": 2432232314}"#,
        );
        assert_eq!(sig, "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=");
    }

    /// The signature covers the id and the timestamp, not the body alone.
    ///
    /// This is the property the old scheme did not have, and the reason a
    /// captured POST could replay forever.
    #[test]
    fn the_signature_covers_the_id_and_the_timestamp() {
        let (secret, body, ts) = (b"s3cr3t".as_slice(), b"{}".as_slice(), now());
        let base = sign(secret, "msg_1", ts, body);
        assert_ne!(base, sign(secret, "msg_2", ts, body), "id is covered");
        assert_ne!(base, sign(secret, "msg_1", ts + 1, body), "ts is covered");
        assert_ne!(base, sign(secret, "msg_1", ts, b"x"), "body is covered");
    }

    /// A replayed request is refused even though its signature is still valid.
    #[test]
    fn a_stale_timestamp_is_refused() {
        let (secret, body) = (b"s3cr3t".as_slice(), b"{}".as_slice());
        let old = now() - tolerance_secs() - 1;
        let h = signed(secret, "msg_1", old, body);

        // The signature itself still verifies — which is exactly why the
        // timestamp check has to be the verifier's job and not the caller's.
        assert!(verify_signature(
            secret,
            "msg_1",
            old,
            body,
            h[SIGNATURE_HEADER].to_str().unwrap()
        ));
        assert_eq!(
            verify_request(Some(secret), &h, body),
            Err(WebhookError::StaleTimestamp)
        );
    }

    /// A clock far in the future is refused too: it would extend the window a
    /// captured request stays valid for.
    #[test]
    fn a_future_timestamp_is_refused() {
        let (secret, body) = (b"s3cr3t".as_slice(), b"{}".as_slice());
        let ahead = now() + tolerance_secs() + 1;
        assert_eq!(
            verify_request(Some(secret), &signed(secret, "m", ahead, body), body),
            Err(WebhookError::StaleTimestamp)
        );
    }

    /// Modest clock skew between two hosts is accepted in both directions.
    #[test]
    fn skew_inside_the_tolerance_is_accepted() {
        let (secret, body) = (b"s3cr3t".as_slice(), b"{}".as_slice());
        for offset in [-120_i64, -1, 0, 1, 120] {
            let ts = now() + offset;
            assert!(
                verify_request(Some(secret), &signed(secret, "m", ts, body), body).is_ok(),
                "offset {offset}s must verify"
            );
        }
    }

    /// The happy path hands back the id, so a receiver has what it needs to
    /// deduplicate without reaching into the headers again.
    #[test]
    fn a_valid_request_returns_the_id_to_deduplicate_on() {
        let (secret, body) = (b"s3cr3t".as_slice(), b"{\"a\":1}".as_slice());
        let h = signed(secret, "de.mako.process.initiated/42", now(), body);
        assert_eq!(
            verify_request(Some(secret), &h, body),
            Ok(Some(WebhookId("de.mako.process.initiated/42".to_owned())))
        );
    }

    /// **Key rotation.** The header is a list, so a sender may present the old
    /// and the new signature at once and either side of the rollover verifies.
    #[test]
    fn either_key_verifies_during_a_rotation() {
        let (old, new, body, ts) = (
            b"old-key".as_slice(),
            b"new-key".as_slice(),
            b"{}".as_slice(),
            now(),
        );
        let both = format!("{} {}", sign(old, "m", ts, body), sign(new, "m", ts, body));

        for secret in [old, new] {
            let mut h = HeaderMap::new();
            h.insert(HeaderName::from_static(ID_HEADER), "m".parse().unwrap());
            h.insert(
                HeaderName::from_static(TIMESTAMP_HEADER),
                ts.to_string().parse().unwrap(),
            );
            h.insert(
                HeaderName::from_static(SIGNATURE_HEADER),
                both.parse().unwrap(),
            );
            assert!(verify_request(Some(secret), &h, body).is_ok());
        }
        assert!(!verify_signature(b"third-key", "m", ts, body, &both));
    }

    /// A version this implementation does not read is skipped, not refused —
    /// a sender offering `v1a` alongside `v1` must still be accepted.
    #[test]
    fn an_unknown_signature_version_is_skipped() {
        let (secret, body, ts) = (b"s3cr3t".as_slice(), b"{}".as_slice(), now());
        let header = format!("v1a,ignored {}", sign(secret, "m", ts, body));
        assert!(verify_signature(secret, "m", ts, body, &header));
        // …but a header with *only* an unreadable version matches nothing.
        assert!(!verify_signature(secret, "m", ts, body, "v1a,ignored"));
    }

    #[test]
    fn a_wrong_secret_or_a_tampered_body_is_refused() {
        let (secret, body, ts) = (b"s3cr3t".as_slice(), b"{}".as_slice(), now());
        let h = signed(secret, "m", ts, body);
        assert_eq!(
            verify_request(Some(b"wrong"), &h, body),
            Err(WebhookError::SignatureMismatch)
        );
        assert_eq!(
            verify_request(Some(secret), &h, b"tampered"),
            Err(WebhookError::SignatureMismatch)
        );
    }

    /// Each missing header is named, so an integrator debugging a 401 learns
    /// which of the three they forgot.
    #[test]
    fn each_missing_header_names_itself() {
        let (secret, body, ts) = (b"s3cr3t".as_slice(), b"{}".as_slice(), now());
        let full = signed(secret, "m", ts, body);
        for missing in [ID_HEADER, TIMESTAMP_HEADER, SIGNATURE_HEADER] {
            let mut h = full.clone();
            h.remove(missing);
            assert_eq!(
                verify_request(Some(secret), &h, body),
                Err(WebhookError::MissingHeader(missing))
            );
        }
    }

    #[test]
    fn a_non_numeric_timestamp_is_a_header_fault() {
        let (secret, body) = (b"s3cr3t".as_slice(), b"{}".as_slice());
        let mut h = signed(secret, "m", now(), body);
        h.insert(
            HeaderName::from_static(TIMESTAMP_HEADER),
            "not-a-number".parse().unwrap(),
        );
        assert_eq!(
            verify_request(Some(secret), &h, body),
            Err(WebhookError::MissingHeader(TIMESTAMP_HEADER))
        );
    }

    /// Dev mode verifies nothing and still surfaces the id, so a handler's
    /// deduplication runs in the environment nobody tests too.
    #[test]
    fn dev_mode_still_surfaces_the_id() {
        let mut h = HeaderMap::new();
        h.insert(HeaderName::from_static(ID_HEADER), "m".parse().unwrap());
        assert_eq!(
            verify_request(None, &h, b"{}"),
            Ok(Some(WebhookId("m".to_owned())))
        );
        assert_eq!(verify_request(None, &HeaderMap::new(), b"{}"), Ok(None));
    }

    #[test]
    fn every_refusal_is_a_401() {
        for e in [
            WebhookError::MissingHeader(ID_HEADER),
            WebhookError::StaleTimestamp,
            WebhookError::SignatureMismatch,
        ] {
            assert_eq!(StatusCode::from(e), StatusCode::UNAUTHORIZED);
        }
    }

    /// `agentd`'s decision deliveries are signed by **agentplane**, not by this
    /// module, and are verified by mako receivers that use this one.
    ///
    /// Two implementations of one spec is exactly where a wire contract drifts,
    /// so this pins the shape both must produce: the three header names, the
    /// `v1,` prefix, and the `{id}.{timestamp}.{body}` payload. The reference
    /// vector above pins the arithmetic; this pins the envelope. If agentplane
    /// changes either, `agentd`'s audit webhook stops verifying — and it stops
    /// here instead.
    #[test]
    fn the_header_names_are_the_spec_s_and_agentplane_s() {
        assert_eq!(ID_HEADER, "webhook-id");
        assert_eq!(TIMESTAMP_HEADER, "webhook-timestamp");
        assert_eq!(SIGNATURE_HEADER, "webhook-signature");
        assert!(sign(b"k", "m", 0, b"{}").starts_with("v1,"));
        assert_eq!(
            signed_payload("m", 42, b"{}"),
            b"m.42.{}".to_vec(),
            "the signed payload is `{{id}}.{{timestamp}}.{{body}}`"
        );
    }

    #[test]
    fn an_empty_secret_and_body_round_trip() {
        let ts = now();
        assert!(verify_request(Some(b""), &signed(b"", "m", ts, b""), b"").is_ok());
    }
}
