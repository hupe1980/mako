//! Axum server for the MaLo Identification API v1.
//!
//! The **Netzbetreiber (NB)** implements [`MaloIdentHandler`] to:
//! - Receive `POST /maloId/request/v1` from the Lieferant.
//!
//! The NB delivers results by calling the LF's callback endpoint via
//! [`crate::client::MaloIdentClient`].
//!
//! The **Lieferant (LF)** also needs a server endpoint to receive the async
//! callback responses from the NB:
//! - `POST /maloId/dataForMarketLocationPositive/v1`
//! - `POST /maloId/dataForMarketLocationNegative/v1`
//!
//! Both sides can use the same [`MaloIdentHandler`] trait — override only
//! the methods relevant to your role; unimplemented methods return `501`.

use std::future::Future;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing};
use serde::Deserialize;

use crate::error::Error;
use crate::models::electricity::{
    IdentificationParameter, MaloIdentResultNegative, MaloIdentResultPositive,
};

// ── MaloRegistry trait ────────────────────────────────────────────────────────

/// Read-only port over the NB's cached MaLo master-data snapshot.
///
/// Implement this trait to provide a MaLo lookup backend for [`MaloIdentHandler`]
/// implementations. The implementation is provided by the operator; `makod`
/// ships `SlateDbMaloCache` as its production implementation.
///
/// # Tenant isolation
///
/// `tenant_id` is an opaque string key. In a single-tenant deployment pass
/// the operator's GLN. In a multi-tenant deployment it scopes lookup results
/// to the correct grid operator.
///
/// # Error semantics
///
/// - `Ok(Some(r))` — MaLo found; deliver positive callback with `r`.
/// - `Ok(None)`    — MaLo not found; deliver negative callback.
/// - `Err(_)`      — Transient storage error; retry via outbox.
#[allow(async_fn_in_trait)]
pub trait MaloRegistry: Send + Sync + 'static {
    /// Look up a market location by the BDEW identification parameters.
    async fn lookup(
        &self,
        tenant_id: &str,
        params: &IdentificationParameter,
    ) -> Result<Option<MaloIdentResultPositive>, Error>;
}

// ── Test/stub implementations ─────────────────────────────────────────────────

/// A [`MaloRegistry`] that always returns `None` (negative result).
///
/// Useful for stubs, tests where MaLo data is irrelevant, and for
/// deployments where the MaLo Identification API should temporarily return
/// negative results while the cache is being populated.
#[derive(Debug, Clone, Default)]
pub struct NoopMaloRegistry;

impl MaloRegistry for NoopMaloRegistry {
    async fn lookup(
        &self,
        _tenant_id: &str,
        _params: &IdentificationParameter,
    ) -> Result<Option<MaloIdentResultPositive>, Error> {
        Ok(None)
    }
}

/// A [`MaloRegistry`] backed by a fixed in-memory lookup table.
///
/// Keyed by `(tenant_id, malo_id)`. The `malo_id` used for matching is
/// `params.identification_parameter_id.as_ref()?.malo_id?.0` — i.e., the
/// explicit MaLo-ID field in the request. Falls back to `Ok(None)` for
/// address-only or parameter-only lookups (those require a real database index).
///
/// Suitable for unit tests and integration tests that need deterministic results.
#[derive(Debug, Clone, Default)]
pub struct StaticMaloRegistry {
    entries: std::collections::HashMap<(String, String), MaloIdentResultPositive>,
}

impl StaticMaloRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a MaLo result reachable by `(tenant_id, malo_id)` lookup.
    pub fn insert(
        &mut self,
        tenant_id: impl Into<String>,
        malo_id: impl Into<String>,
        result: MaloIdentResultPositive,
    ) {
        self.entries
            .insert((tenant_id.into(), malo_id.into()), result);
    }
}

impl MaloRegistry for StaticMaloRegistry {
    async fn lookup(
        &self,
        tenant_id: &str,
        params: &IdentificationParameter,
    ) -> Result<Option<MaloIdentResultPositive>, Error> {
        let malo_id = params
            .identification_parameter_id
            .as_ref()
            .and_then(|id| id.malo_id.as_ref())
            .map(|m| m.0.as_str())
            .unwrap_or("");
        Ok(self
            .entries
            .get(&(tenant_id.to_owned(), malo_id.to_owned()))
            .cloned())
    }
}

// ── Handler trait ─────────────────────────────────────────────────────────────

/// Business-logic callbacks for the MaLo Identification API v1.
pub trait MaloIdentHandler: Send + Sync + 'static {
    /// `POST /maloId/request/v1` — NB receives a MaLo-ID identification request.
    ///
    /// `sender_market_partner_id` is the 13-digit BDEW code of the requesting
    /// Lieferant, extracted from the `marketPartnerId` HTTP header.  The NB
    /// must use this to discover the LF's callback URL (e.g. from the BDEW
    /// Verzeichnisdienst or a static partner map) and deliver the result via
    /// `MaloIdentClient::send_positive_response` / `send_negative_response`.
    fn on_request(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _sender_market_partner_id: String,
        _params: IdentificationParameter,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 501,
                body: "not implemented".into(),
            })
        }
    }

    /// `POST /maloId/dataForMarketLocationPositive/v1` — LF receives a
    /// positive identification result from the NB.
    fn on_positive_result(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _reference_id: String,
        _result: MaloIdentResultPositive,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 501,
                body: "not implemented".into(),
            })
        }
    }

    /// `POST /maloId/dataForMarketLocationNegative/v1` — LF receives a
    /// negative identification result from the NB.
    fn on_negative_result(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _reference_id: String,
        _result: MaloIdentResultNegative,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 501,
                body: "not implemented".into(),
            })
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build an [`axum::Router`] for the MaLo Identification API v1.
pub fn router<S>(state: Arc<S>) -> Router
where
    S: MaloIdentHandler + Clone,
{
    Router::new()
        .route("/maloId/request/v1", routing::post(handle_request::<S>))
        .route(
            "/maloId/dataForMarketLocationPositive/v1",
            routing::post(handle_positive::<S>),
        )
        .route(
            "/maloId/dataForMarketLocationNegative/v1",
            routing::post(handle_negative::<S>),
        )
        .with_state(state)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

fn str_header(h: &HeaderMap, name: &str) -> String {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

#[derive(Deserialize)]
struct ReferenceQuery {
    #[serde(rename = "referenceId")]
    reference_id: String,
}

async fn handle_request<S: MaloIdentHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Json(params): Json<IdentificationParameter>,
) -> Response {
    match svc
        .on_request(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            str_header(&headers, "marketPartnerId"),
            params,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_positive<S: MaloIdentHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<ReferenceQuery>,
    Json(result): Json<MaloIdentResultPositive>,
) -> Response {
    match svc
        .on_positive_result(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            q.reference_id,
            result,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_negative<S: MaloIdentHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<ReferenceQuery>,
    Json(result): Json<MaloIdentResultNegative>,
) -> Response {
    match svc
        .on_negative_result(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            q.reference_id,
            result,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

fn handler_error(e: Error) -> Response {
    match e {
        Error::Http { status, body } => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        )
            .into_response(),
        other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()).into_response(),
    }
}
