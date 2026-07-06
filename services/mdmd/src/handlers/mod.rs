#![allow(clippy::type_complexity)]
//! Axum handler utilities shared across all endpoint modules.
//!
//! - `Claims` — JWT bearer extraction via `FromRequestParts`
//! - `IntoResponse for MdmError` — maps domain errors to HTTP status codes
//! - `parse_if_match` — `If-Match` header → `Option<i64>`
//! - `etag` — `i64` version → ETag header value

pub mod contract;
pub mod correlation;
pub mod event_ingest;
pub mod health;
pub mod malo;
pub mod melo;
pub mod partner;
pub mod subscription;

use axum::{
    Extension,
    extract::FromRequestParts,
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use mako_mdm::error::MdmError;

use crate::oidc::{JwtClaims, OidcError, OidcVerifier};

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

// ── Claims extractor ──────────────────────────────────────────────────────────

/// JWT claims extracted from `Authorization: Bearer <token>`.
///
/// The `OidcVerifier` is injected via `Extension<OidcVerifier>`.
#[derive(Debug, Clone)]
pub struct Claims(pub JwtClaims);

impl Claims {
    /// Returns `true` if the caller holds `role` (case-insensitive).
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.0.has_role(role)
    }

    #[must_use]
    pub fn sub(&self) -> &str {
        &self.0.sub
    }
}

/// Rejection type when bearer auth fails.
#[derive(Debug)]
pub struct AuthError(pub OidcError);

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        // RFC 7807 Problem Details
        let body = serde_json::json!({
            "type":   "https://docs.mako.energy/problems/unauthorized",
            "title":  "Unauthorized",
            "status": 401u16,
            "detail": self.0.to_string(),
        });
        let mut resp = (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response();
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            "application/problem+json"
                .parse()
                .expect("valid header value"),
        );
        resp
    }
}

impl<S> FromRequestParts<S> for Claims
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // Extract OidcVerifier from extensions (added as a layer in main.rs)
        let Extension(verifier): Extension<OidcVerifier> =
            Extension::from_request_parts(parts, state)
                .await
                .map_err(|_| {
                    AuthError(OidcError::MissingKid) // OidcVerifier not configured
                })?;

        let bearer = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));

        let token = bearer.ok_or(AuthError(OidcError::MissingKid))?;

        let claims = verifier.verify(token).map_err(AuthError)?;
        Ok(Claims(claims))
    }
}

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
