#![allow(clippy::type_complexity)]
//! Axum handler utilities shared across all endpoint modules.
//!
//! - `Claims` — JWT bearer extraction via `FromRequestParts`
//! - `IntoResponse for MdmError` — maps domain errors to HTTP status codes
//! - `parse_if_match` — `If-Match` header → [`IfMatch`]
//! - `etag` — `i64` version → ETag header value

pub mod bilanzierung;
pub mod correlation;
pub mod device;
pub mod dlq;
pub mod einwilligung;
pub mod event_ingest;
pub mod event_log;
pub mod grundversorger;
pub mod lokationszuordnung;
pub mod mabis_zp;
pub mod malo;
pub mod malo_grid;
pub mod melo;
pub mod melo_msb;
pub mod mmma_preise;
pub mod msb_rahmenvertrag_gas;
pub mod nb_contract;
pub mod nb_energiemix;
pub mod nelo;
pub mod netzzugang;
pub mod partner;
pub mod preisblatt;
pub mod pricat;
pub mod subscription;
pub mod tranche;
pub mod versorgung;

use axum::{
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use mako_markt::error::MdmError;

// Re-export Claims from mako-service so handlers use the shared implementation.
pub use mako_service::oidc::Claims;

// ── MdmError → axum response ──────────────────────────────────────────────────

/// Local newtype so we can impl `IntoResponse` for the foreign `MdmError`.
///
/// Use `.into_response()` via the `IntoMdmResponse` extension trait (below),
/// which avoids having to wrap at every call site.
pub struct MdmErrorResponse(pub MdmError);

impl IntoResponse for MdmErrorResponse {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        // RFC 7807 Problem Details for HTTP APIs
        let body = serde_json::json!({
            "type":   format!("https://docs.mako.energy/problems/{}", self.0.error_code()),
            "title":  self.0.error_title(),
            "status": self.0.status_u16(),
            "detail": self.0.to_string(),
        });
        let mut resp = (status, axum::Json(body)).into_response();
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            "application/problem+json"
                .parse()
                .expect("valid header value"),
        );
        resp
    }
}

/// Extension trait that lets `MdmError` turn itself into an axum `Response`
/// without the orphan newtype boilerplate at every call site.
pub trait IntoMdmResponse {
    fn into_response(self) -> Response;
}

impl IntoMdmResponse for MdmError {
    fn into_response(self) -> Response {
        MdmErrorResponse(self).into_response()
    }
}

// ── TenantGln Extension ───────────────────────────────────────────────────────

/// The instance's primary tenant GLN, injected as an Axum `Extension`.
///
/// Set once at startup from `[markt] tenant`.
/// Used by handlers that don't have direct access to `AppState` (e.g. `preisblatt`)
/// as the `resource_tenant` argument to [`mako_service::cedar::CedarEnforcer::check`].
#[derive(Debug, Clone)]
pub struct TenantGln(pub String);

// ── If-Match / ETag helpers ───────────────────────────────────────────────────

/// The outcome of reading an `If-Match` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfMatch {
    /// No `If-Match` — an unconditional write.
    Absent,
    /// `If-Match: *` — "must already exist", with no version constraint.
    Any,
    /// A concrete version the caller expects to still be current.
    Version(i64),
    /// Present but unusable.
    Malformed,
}

/// Read the `If-Match` header.
///
/// Accepts a bare or quoted version (`3`, `"3"`), the weak form (`W/"3"`), and
/// `*`. Anything else is [`IfMatch::Malformed`] and the caller must answer
/// `400`, **not** treat it as absent: silently ignoring an unparsable
/// precondition turns the conditional write the client asked for into an
/// unconditional one, which is the lost update it was trying to prevent
/// (RFC 9110 § 13.1.1).
#[must_use]
pub fn parse_if_match(headers: &HeaderMap) -> IfMatch {
    let Some(raw) = headers.get("if-match") else {
        return IfMatch::Absent;
    };
    let Ok(raw) = raw.to_str() else {
        return IfMatch::Malformed;
    };
    let raw = raw.trim();
    if raw == "*" {
        return IfMatch::Any;
    }
    // A list of candidate etags is legal but this resource has exactly one
    // version, so anything beyond a single entry cannot be honoured.
    if raw.contains(',') {
        return IfMatch::Malformed;
    }
    let value = raw.strip_prefix("W/").unwrap_or(raw).trim_matches('"');
    value
        .parse::<i64>()
        .map_or(IfMatch::Malformed, IfMatch::Version)
}

/// The `400` for an `If-Match` this resource cannot evaluate.
#[must_use]
pub fn malformed_if_match() -> Response {
    MdmError::Unprocessable {
        reason: "If-Match must be a version number (`\"3\"`, `W/\"3\"`) or `*`".to_owned(),
    }
    .into_response()
}

/// Build an ETag header value from a version number (`"<version>"`).
#[must_use]
pub fn etag(version: i64) -> String {
    format!("\"{version}\"")
}

/// Serialise a validated BO4E object for storage, or fail the request.
///
/// The obvious spelling, `serde_json::to_value(&bo).unwrap_or_default()`, yields
/// `Value::Null` on failure — and PostgreSQL accepts a JSON `null` into a
/// `JSONB NOT NULL` column, because SQL `NULL` and JSON `null` are different
/// things. A validated document would therefore have been replaced by the JSON
/// literal `null` with a `204` in reply. Failure here is not reachable for the
/// generated BO4E types, which is exactly why it should be stated rather than
/// defaulted: if it ever becomes reachable, the write must not happen.
pub(crate) fn serialise_or_500<T: serde::Serialize>(
    value: &T,
    what: &str,
) -> Result<serde_json::Value, (axum::http::StatusCode, serde_json::Value)> {
    serde_json::to_value(value).map_err(|e| {
        tracing::error!(bo = what, error = %e, "validated BO4E object is not serialisable");
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": format!("could not serialise {what}: {e}") }),
        )
    })
}

#[cfg(test)]
mod if_match_tests {
    use super::{IfMatch, parse_if_match};
    use axum::http::HeaderMap;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("if-match", value.parse().expect("header value"));
        h
    }

    #[test]
    fn no_header_is_an_unconditional_write() {
        assert_eq!(parse_if_match(&HeaderMap::new()), IfMatch::Absent);
    }

    #[test]
    fn quoted_bare_and_weak_etags_all_yield_the_version() {
        for raw in ["\"3\"", "3", "W/\"3\"", "  \"3\"  "] {
            assert_eq!(
                parse_if_match(&headers(raw)),
                IfMatch::Version(3),
                "{raw:?} must read as version 3"
            );
        }
    }

    #[test]
    fn a_star_means_must_exist_without_a_version() {
        assert_eq!(parse_if_match(&headers("*")), IfMatch::Any);
    }

    #[test]
    fn an_unusable_precondition_is_malformed_not_absent() {
        // Treating any of these as "absent" would silently downgrade the
        // caller's conditional write to an unconditional one — the lost update
        // the header exists to prevent.
        for raw in ["\"abc\"", "", "\"3\", \"4\"", "etag-3"] {
            assert_eq!(
                parse_if_match(&headers(raw)),
                IfMatch::Malformed,
                "{raw:?} must not be silently ignored"
            );
        }
    }
}
