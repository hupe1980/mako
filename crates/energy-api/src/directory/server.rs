//! Axum-based server implementation scaffold for the Directory Service v1.
//!
//! Requires feature `server`.
//!
//! Implement the [`DirectoryServiceHandler`] trait and call [`router`] to get
//! an [`axum::Router`] that you can mount inside your application.
//!
//! # Example
//!
//! ```no_run
//! # #[cfg(feature = "server")]
//! # async fn example() {
//! use std::sync::Arc;
//! use energy_api::directory::server::{DirectoryServiceHandler, RecordResponse, PutRecordResponse, router};
//! use energy_api::directory::{ApiRecord, ServiceInfo};
//! use energy_api::Error;
//!
//! #[derive(Clone)]
//! struct MyDirectory;
//!
//! impl DirectoryServiceHandler for MyDirectory {
//!     async fn get_service_info(&self) -> Result<ServiceInfo, Error> { todo!() }
//!     async fn get_record(&self, _: &str, _: &str, _: i32) -> Result<RecordResponse, Error> { todo!() }
//!     async fn put_record(&self, _: ApiRecord, _: String, _: String) -> Result<PutRecordResponse, Error> { todo!() }
//!     async fn delete_record(&self, _: &str, _: &str, _: i32) -> Result<(), Error> { todo!() }
//!     async fn put_redirect(&self, _: &str, _: &str, _: i32, _: String) -> Result<(), Error> { todo!() }
//!     async fn delete_redirect(&self, _: &str, _: &str, _: i32) -> Result<(), Error> { todo!() }
//! }
//!
//! let app = router(Arc::new(MyDirectory));
//! # }
//! ```

use std::future::Future;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::{Router, routing};
use serde::Deserialize;

use crate::error::Error;
use crate::models::directory::{ApiRecord, ServiceInfo};

// ── Response types ────────────────────────────────────────────────────────────

/// Response from [`DirectoryServiceHandler::get_record`].
pub enum RecordResponse {
    /// Record found — return it with signature headers.
    Found {
        /// The directory record body.
        record: ApiRecord,
        /// RFC 9440 signing certificate (`X-BDEW-CERT`).
        signing_cert: String,
        /// Base64url JWS signature (`X-BDEW-SIGNATURE`).
        signature: String,
    },
    /// A redirect is configured for this entry — return 307.
    Redirect {
        /// URL to which the client should redirect and retry.
        target_url: String,
    },
    /// No entry exists — return 404.
    NotFound,
}

/// Response from [`DirectoryServiceHandler::put_record`].
pub enum PutRecordResponse {
    /// A new entry was created — return 201.
    Created,
    /// An existing entry was updated — return 204.
    Updated,
    /// Revision constraint violated — return 400 with `X-BDEW-EXPECTED-REVISION`.
    RevisionConflict {
        /// The revision number that would be accepted next.
        expected_revision: i64,
    },
    /// Selfservice not supported by this implementation — return 405.
    NotSupported,
}

// ── Handler trait ─────────────────────────────────────────────────────────────

/// Business-logic trait for the Directory Service REST API.
///
/// Implement on your service state type; then call [`router`] to build an
/// [`axum::Router`].  All async methods must be `Send`.
pub trait DirectoryServiceHandler: Send + Sync + 'static {
    /// `GET /info/service/v1`
    fn get_service_info(&self) -> impl Future<Output = Result<ServiceInfo, Error>> + Send;

    /// `GET /record/{providerId}/{apiId}/{majorVersion}/v1`
    fn get_record(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
    ) -> impl Future<Output = Result<RecordResponse, Error>> + Send;

    /// `PUT /record/{providerId}/{apiId}/{majorVersion}/v1`
    fn put_record(
        &self,
        record: ApiRecord,
        signing_cert: String,
        signature: String,
    ) -> impl Future<Output = Result<PutRecordResponse, Error>> + Send;

    /// `DELETE /record/{providerId}/{apiId}/{majorVersion}/v1`
    fn delete_record(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// `PUT /redirect/{providerId}/{apiId}/{majorVersion}/v1?url=…`
    fn put_redirect(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
        target_url: String,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// `DELETE /redirect/{providerId}/{apiId}/{majorVersion}/v1`
    fn delete_redirect(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
    ) -> impl Future<Output = Result<(), Error>> + Send;
}

// ── Router factory ────────────────────────────────────────────────────────────

/// Build an [`axum::Router`] implementing the Directory Service REST API.
///
/// Mount this at the root of your application.
pub fn router<S>(state: Arc<S>) -> Router
where
    S: DirectoryServiceHandler + Clone,
{
    Router::new()
        .route(
            "/info/service/v1",
            routing::get(handle_get_service_info::<S>),
        )
        .route(
            "/record/:provider_id/:api_id/:major_version/v1",
            routing::get(handle_get_record::<S>)
                .put(handle_put_record::<S>)
                .delete(handle_delete_record::<S>),
        )
        .route(
            "/redirect/:provider_id/:api_id/:major_version/v1",
            routing::put(handle_put_redirect::<S>).delete(handle_delete_redirect::<S>),
        )
        .with_state(state)
}

// ── Path / query parameter structs ────────────────────────────────────────────

#[derive(Deserialize)]
struct RecordPath {
    provider_id: String,
    api_id: String,
    major_version: i32,
}

#[derive(Deserialize)]
struct RedirectQuery {
    url: String,
}

// ── Axum handlers ─────────────────────────────────────────────────────────────

async fn handle_get_service_info<S: DirectoryServiceHandler + Clone>(
    State(svc): State<Arc<S>>,
) -> Response {
    match svc.get_service_info().await {
        Ok(info) => (StatusCode::OK, Json(info)).into_response(),
        Err(e) => service_error(e),
    }
}

async fn handle_get_record<S: DirectoryServiceHandler + Clone>(
    Path(p): Path<RecordPath>,
    State(svc): State<Arc<S>>,
) -> Response {
    match svc
        .get_record(&p.provider_id, &p.api_id, p.major_version)
        .await
    {
        Ok(RecordResponse::Found {
            record,
            signing_cert,
            signature,
        }) => {
            let mut resp = (StatusCode::OK, Json(record)).into_response();
            let headers = resp.headers_mut();
            set_str_header(headers, "x-bdew-cert", &signing_cert);
            set_str_header(headers, "x-bdew-signature", &signature);
            resp
        }
        Ok(RecordResponse::Redirect { target_url }) => {
            let mut resp = StatusCode::TEMPORARY_REDIRECT.into_response();
            set_str_header(resp.headers_mut(), "location", &target_url);
            resp
        }
        Ok(RecordResponse::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => service_error(e),
    }
}

async fn handle_put_record<S: DirectoryServiceHandler + Clone>(
    Path(p): Path<RecordPath>,
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Json(record): Json<ApiRecord>,
) -> Response {
    // Validate path params match body (spec requirement)
    if record.provider_id != p.provider_id
        || record.api_id != p.api_id
        || record.major_version != p.major_version
    {
        return (
            StatusCode::BAD_REQUEST,
            "path and body identifiers do not match",
        )
            .into_response();
    }
    let signing_cert = str_header(&headers, "x-bdew-cert");
    let signature = str_header(&headers, "x-bdew-signature");
    match svc.put_record(record, signing_cert, signature).await {
        Ok(PutRecordResponse::Created) => StatusCode::CREATED.into_response(),
        Ok(PutRecordResponse::Updated) => StatusCode::NO_CONTENT.into_response(),
        Ok(PutRecordResponse::RevisionConflict { expected_revision }) => {
            let mut resp = StatusCode::BAD_REQUEST.into_response();
            set_str_header(
                resp.headers_mut(),
                "x-bdew-expected-revision",
                &expected_revision.to_string(),
            );
            resp
        }
        Ok(PutRecordResponse::NotSupported) => StatusCode::NOT_IMPLEMENTED.into_response(),
        Err(e) => service_error(e),
    }
}

async fn handle_delete_record<S: DirectoryServiceHandler + Clone>(
    Path(p): Path<RecordPath>,
    State(svc): State<Arc<S>>,
) -> Response {
    match svc
        .delete_record(&p.provider_id, &p.api_id, p.major_version)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => service_error(e),
    }
}

async fn handle_put_redirect<S: DirectoryServiceHandler + Clone>(
    Path(p): Path<RecordPath>,
    Query(q): Query<RedirectQuery>,
    State(svc): State<Arc<S>>,
) -> Response {
    match svc
        .put_redirect(&p.provider_id, &p.api_id, p.major_version, q.url)
        .await
    {
        Ok(()) => StatusCode::CREATED.into_response(),
        Err(e) => service_error(e),
    }
}

async fn handle_delete_redirect<S: DirectoryServiceHandler + Clone>(
    Path(p): Path<RecordPath>,
    State(svc): State<Arc<S>>,
) -> Response {
    match svc
        .delete_redirect(&p.provider_id, &p.api_id, p.major_version)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => service_error(e),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn str_header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

fn set_str_header(headers: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        headers.insert(HeaderName::from_static(name), v);
    }
}

fn service_error(e: Error) -> Response {
    match e {
        Error::NotFound => StatusCode::NOT_FOUND.into_response(),
        Error::Http { status, body } => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        )
            .into_response(),
        other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()).into_response(),
    }
}
