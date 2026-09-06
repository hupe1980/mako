//! Inbound header reads that refuse ambiguity.
//!
//! `HeaderMap::get` returns the **first** value when a header appears more than
//! once, and every other reader in the path — a reverse proxy, an API gateway,
//! a partner's own edge — is free to pick a different one. For a header that
//! carries identity or decides idempotency that is a security property, not a
//! parsing detail: a caller sending two `X-Tenant-Id` values has the gateway
//! authorize one tenant while the service acts for the other.
//!
//! [`single_str`] reads such a header and reports a repeat as an error, so the
//! request is refused instead of resolved by header order.

use axum::http::HeaderMap;

/// A header that must appear at most once appeared more than once.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "the {name} header appears {count} times; it must appear at most once — \
     intermediaries may not agree on which value counts"
)]
pub struct DuplicateHeader {
    /// The offending header, lowercased.
    pub name: String,
    /// How many values arrived under it.
    pub count: usize,
}

/// Read a header that must appear at most once.
///
/// Returns `Ok(None)` when the header is absent **or** its single value is not
/// valid UTF-8 — a byte string is never one of the identifiers this is used for,
/// and callers already handle absence. Returns [`DuplicateHeader`] when the
/// header arrived more than once.
///
/// `name` is matched case-insensitively, as HTTP requires.
pub fn single_str<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<Option<&'a str>, DuplicateHeader> {
    let mut values = headers.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    let extra = values.count();
    if extra > 0 {
        return Err(DuplicateHeader {
            name: name.to_ascii_lowercase(),
            count: extra + 1,
        });
    }
    Ok(first.to_str().ok())
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{DuplicateHeader, single_str};

    #[test]
    fn absent_is_none() {
        assert_eq!(single_str(&HeaderMap::new(), "x-tenant-id"), Ok(None));
    }

    #[test]
    fn one_value_is_read_case_insensitively() {
        let mut h = HeaderMap::new();
        h.insert("X-Tenant-Id", HeaderValue::from_static("stadtwerke"));
        assert_eq!(single_str(&h, "x-tenant-id"), Ok(Some("stadtwerke")));
        assert_eq!(single_str(&h, "X-TENANT-ID"), Ok(Some("stadtwerke")));
    }

    /// The property the module exists for: two values are an error, not a
    /// first-wins pick that an intermediary may resolve the other way.
    #[test]
    fn a_repeat_is_refused() {
        let mut h = HeaderMap::new();
        h.append("x-tenant-id", HeaderValue::from_static("stadtwerke"));
        h.append("x-tenant-id", HeaderValue::from_static("nachbar-ag"));
        assert_eq!(
            single_str(&h, "x-tenant-id"),
            Err(DuplicateHeader {
                name: "x-tenant-id".to_owned(),
                count: 2,
            })
        );
    }

    /// Two identical values are still two values: the request is malformed and
    /// the sender should be told so rather than have one silently dropped.
    #[test]
    fn a_repeated_identical_value_is_refused_too() {
        let mut h = HeaderMap::new();
        h.append("idempotency-key", HeaderValue::from_static("k-1"));
        h.append("idempotency-key", HeaderValue::from_static("k-1"));
        assert!(single_str(&h, "idempotency-key").is_err());
    }

    #[test]
    fn non_utf8_reads_as_absent() {
        let mut h = HeaderMap::new();
        h.insert(
            "x-tenant-id",
            HeaderValue::from_bytes(&[0xFF, 0xFE]).expect("valid header bytes"),
        );
        assert_eq!(single_str(&h, "x-tenant-id"), Ok(None));
    }
}
