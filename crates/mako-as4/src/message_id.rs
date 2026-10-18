//! `eb:MessageId` values.
//!
//! ebMS3 requires an RFC 2822 `msg-id` — `id-left "@" id-right` — and
//! `asx-rs` refuses any other form at send time. The right-hand side is a
//! domain the operator controls; the left-hand side is the outbox entry id,
//! so every retry of one entry carries the same message id.

use std::fmt;

/// The `id-right` of every message id a deployment sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageIdDomain(String);

/// A configured message-id domain that is not an RFC 5322 `dot-atom-text`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{0:?} is not a message-id domain: RFC 2822 §3.6.4 takes a dot-atom (letters, digits, \
     `-`, `_`, `.` between non-empty labels), e.g. as4.stadtwerke-musterstadt.de"
)]
pub struct InvalidMessageIdDomain(pub String);

impl MessageIdDomain {
    /// Parse a domain. Restricted to letters, digits, `-` and `_` in
    /// dot-separated non-empty labels — a subset of RFC 5322 `dot-atom-text`
    /// that every MSH accepts.
    ///
    /// # Errors
    ///
    /// [`InvalidMessageIdDomain`] for anything else.
    pub fn parse(domain: &str) -> Result<Self, InvalidMessageIdDomain> {
        let ok = !domain.is_empty()
            && domain.len() <= 253
            && domain.split('.').all(|label| {
                !label.is_empty()
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            });
        if ok {
            Ok(Self(domain.to_owned()))
        } else {
            Err(InvalidMessageIdDomain(domain.to_owned()))
        }
    }

    /// The message id `{local}@{domain}`.
    #[must_use]
    pub fn message_id(&self, local: impl fmt::Display) -> String {
        format!("{local}@{}", self.0)
    }

    /// The domain as configured.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_id_is_left_at_right() {
        let d = MessageIdDomain::parse("as4.example.de").unwrap();
        assert_eq!(
            d.message_id("3f2c0d0e-5b1a-4c55-8a2e-6f1f6c3b9d10"),
            "3f2c0d0e-5b1a-4c55-8a2e-6f1f6c3b9d10@as4.example.de"
        );
    }

    #[test]
    fn a_domain_that_is_no_dot_atom_is_refused() {
        for bad in ["", "a..b", ".a", "a.", "a b", "a@b", "<a>"] {
            assert!(MessageIdDomain::parse(bad).is_err(), "{bad:?}");
        }
        assert!(MessageIdDomain::parse("localhost").is_ok());
    }
}
