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
//! the methods relevant to your role; unimplemented methods return `405`.

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

// ── Handler trait ─────────────────────────────────────────────────────────────

/// Business-logic callbacks for the MaLo Identification API v1.
pub trait MaloIdentHandler: Send + Sync + 'static {
    /// `POST /maloId/request/v1` — NB receives a MaLo-ID identification request.
    fn on_request(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _params: IdentificationParameter,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 405,
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
                status: 405,
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
                status: 405,
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
