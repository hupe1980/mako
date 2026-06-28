//! Axum server for the WiM Order API v1 — iMS Universalbestellprozess.
//!
//! Implement [`WimOrderHandler`] on your service state and call [`router`] to
//! get an [`axum::Router`].
//!
//! ## Endpoint overview
//!
//! | Path | Direction | PID |
//! |---|---|---|
//! | `POST /wimBestellung/v1/anmeldung/` | NB → MSB | 11021 |
//! | `POST /wimBestellung/v1/bestaetigung/` | MSB → NB | 11022 |
//! | `POST /wimBestellung/v1/ablehnung/` | MSB → NB | 11023 |
//!
//! The **MSB role** implements `on_anmeldung` (receives orders from NB).
//! The **NB role** implements `on_bestaetigung` / `on_ablehnung` (receives
//! responses from MSB).
//!
//! Unimplemented methods default to returning `501 Not Implemented`.

use std::future::Future;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Router, routing};
use serde::Deserialize;

use crate::error::Error;
use crate::models::electricity::{WimAblehnung, WimAnmeldungRequest, WimBestaetigung};

// ── Handler trait ─────────────────────────────────────────────────────────────

/// Business-logic callbacks for the WiM Order API v1.
///
/// All methods have a default implementation that returns
/// `501 Not Implemented`, so implementors only need to override the endpoints
/// relevant to their role.
pub trait WimOrderHandler: Send + Sync + 'static {
    // MSB side — receives order from NB

    /// `POST /wimBestellung/v1/anmeldung/` — MSB receives an iMS installation
    /// order from the Netzbetreiber (PID 11021).
    ///
    /// On success return `Ok(())` — the framework sends `202 Accepted`.
    /// The MSB must respond with a [`WimBestaetigung`] or [`WimAblehnung`]
    /// within the statutory deadline.
    fn on_anmeldung(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _request: WimAnmeldungRequest,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 501,
                body: "not implemented".into(),
            })
        }
    }

    // NB side — receives response from MSB

    /// `POST /wimBestellung/v1/bestaetigung/` — NB receives an order
    /// confirmation from the MSB (PID 11022).
    fn on_bestaetigung(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _response: WimBestaetigung,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 501,
                body: "not implemented".into(),
            })
        }
    }

    /// `POST /wimBestellung/v1/ablehnung/` — NB receives an order rejection
    /// from the MSB (PID 11023).
    fn on_ablehnung(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _response: WimAblehnung,
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

/// Build an [`axum::Router`] for the WiM Order API v1.
pub fn router<S>(state: Arc<S>) -> Router
where
    S: WimOrderHandler + Clone,
{
    Router::new()
        .route(
            "/wimBestellung/v1/anmeldung/",
            routing::post(handle_anmeldung::<S>),
        )
        .route(
            "/wimBestellung/v1/bestaetigung/",
            routing::post(handle_bestaetigung::<S>),
        )
        .route(
            "/wimBestellung/v1/ablehnung/",
            routing::post(handle_ablehnung::<S>),
        )
        .with_state(state)
}

// ── Query extractors ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AnmeldungQuery {
    #[serde(rename = "anmeldung")]
    anmeldung: String,
}

#[derive(Deserialize)]
struct BestaetigungQuery {
    #[serde(rename = "bestaetigung")]
    bestaetigung: String,
}

#[derive(Deserialize)]
struct AblehnungQuery {
    #[serde(rename = "ablehnung")]
    ablehnung: String,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

fn str_header(h: &HeaderMap, name: &str) -> String {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

async fn handle_anmeldung<S: WimOrderHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<AnmeldungQuery>,
) -> Response {
    let request: WimAnmeldungRequest = match serde_json::from_str(&q.anmeldung) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_anmeldung(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            request,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_bestaetigung<S: WimOrderHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<BestaetigungQuery>,
) -> Response {
    let response: WimBestaetigung = match serde_json::from_str(&q.bestaetigung) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_bestaetigung(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            response,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_ablehnung<S: WimOrderHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<AblehnungQuery>,
) -> Response {
    let response: WimAblehnung = match serde_json::from_str(&q.ablehnung) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_ablehnung(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            response,
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
