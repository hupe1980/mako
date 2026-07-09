#![allow(clippy::type_complexity)]
//! Axum handler utilities shared across all endpoint modules.
//!
//! - `Claims` — JWT bearer extraction via `FromRequestParts`
//! - `IntoResponse for MdmError` — maps domain errors to HTTP status codes
//! - `parse_if_match` — `If-Match` header → `Option<i64>`
//! - `etag` — `i64` version → ETag header value

pub mod contract;
pub mod correlation;
pub mod dlq;
pub mod event_ingest;
pub mod health;
pub mod malo;
pub mod malo_grid;
pub mod melo;
pub mod metrics;
pub mod nb_contract;
pub mod nelo;
pub mod partner;
pub mod preisblatt;
pub mod pricat;
pub mod subscription;
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
/// Set once at startup from `cfg.makod.tenant_id`.
/// Used by handlers that don't have direct access to `AppState` (e.g. `preisblatt`)
/// as the `resource_tenant` argument to [`mako_service::cedar::CedarEnforcer::check`].
#[derive(Debug, Clone)]
pub struct TenantGln(pub String);

// ── If-Match / ETag helpers ───────────────────────────────────────────────────

/// Parse the `If-Match` header value (e.g. `"3"`) into a version number.
#[must_use]
pub fn parse_if_match(headers: &HeaderMap) -> Option<i64> {
    let raw = headers.get("if-match")?.to_str().ok()?;
    let stripped = raw.trim_matches('"');
    stripped.parse::<i64>().ok()
}

/// Build an ETag header value from a version number (`"<version>"`).
#[must_use]
pub fn etag(version: i64) -> String {
    format!("\"{version}\"")
}
