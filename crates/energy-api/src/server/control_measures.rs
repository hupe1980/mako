//! Axum server for the Control Measures API v1.
//!
//! Implement [`ControlMeasuresHandler`] on your service state and call
//! [`router`] to get an [`axum::Router`].
//!
//! ## Endpoint ownership
//!
//! The same [`ControlMeasuresHandler`] trait covers both sides of the exchange:
//! - **MSB** implements `on_konfiguration` / `on_initial_zustand` (receives commands).
//! - **NB/LF** implements the six response/info handlers (receives callbacks).
//!
//! Unimplemented methods default to returning `405 Method Not Allowed`.

use std::future::Future;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Router, routing};
use serde::Deserialize;

use crate::error::Error;
use crate::models::electricity::{
    CommandControl, CommandRegular, LocationId, NeloId, PreliminaryStatePositive, ReasonNegative,
    SrId, StateNegative, StatePositive, StateUnknown,
};

// ── Handler trait ─────────────────────────────────────────────────────────────

/// Business-logic callbacks for the Control Measures API v1.
///
/// All methods have a default implementation that returns
/// `405 Method Not Allowed`, so implementors only need to override the
/// endpoints relevant to their role.
pub trait ControlMeasuresHandler: Send + Sync + 'static {
    // MSB side — receives commands from NB/LF

    /// `POST /[Post]/steuerbefehl/konfiguration/` — MSB receives a power-value
    /// control command.
    fn on_konfiguration(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _location_id: LocationId,
        _command: CommandControl,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 405,
                body: "not implemented".into(),
            })
        }
    }

    /// `POST /[Post]/steuerbefehl/initialZustand/` — MSB receives a reset command.
    fn on_initial_zustand(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _location_id: LocationId,
        _command: CommandRegular,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 405,
                body: "not implemented".into(),
            })
        }
    }

    // NB/LF side — receives response/info callbacks from MSB

    /// `POST /[Post]/steuerbefehl/vorlaeufigePositiveAntwort/`
    fn on_vorlaeufigepositiveantwort(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _reference_id: String,
        _location_id: LocationId,
        _state: PreliminaryStatePositive,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 405,
                body: "not implemented".into(),
            })
        }
    }

    /// `POST /[Post]/steuerbefehl/vorlaeufigeNegativeAntwort/`
    fn on_vorlaeufige_negative_antwort(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _reference_id: String,
        _location_id: LocationId,
        _state: StateNegative,
        _reason: ReasonNegative,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 405,
                body: "not implemented".into(),
            })
        }
    }

    /// `POST /[Post]/steuerbefehl/positiveAntwort/`
    fn on_positive_antwort(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _reference_id: String,
        _location_id: LocationId,
        _state: StatePositive,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 405,
                body: "not implemented".into(),
            })
        }
    }

    /// `POST /[Post]/steuerbefehl/negativeAntwort/`
    fn on_negative_antwort(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _reference_id: String,
        _location_id: LocationId,
        _state: StateNegative,
        _reason: ReasonNegative,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 405,
                body: "not implemented".into(),
            })
        }
    }

    /// `POST /[Post]/steuerbefehl/informationAnweisung/`
    fn on_information_anweisung(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _reference_id: String,
        _location_id: LocationId,
        _state: StateUnknown,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async {
            Err(Error::Http {
                status: 405,
                body: "not implemented".into(),
            })
        }
    }

    /// `POST /[Post]/steuerbefehl/information/`
    fn on_information(
        &self,
        _tx_id: String,
        _creation_dt: String,
        _location_id: LocationId,
        _partner_id: i64,
        _command_control: Option<CommandControl>,
        _command_regular: Option<CommandRegular>,
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

/// Build an [`axum::Router`] for the Control Measures API v1.
pub fn router<S>(state: Arc<S>) -> Router
where
    S: ControlMeasuresHandler + Clone,
{
    Router::new()
        .route(
            "/[Post]/steuerbefehl/konfiguration/",
            routing::post(handle_konfiguration::<S>),
        )
        .route(
            "/[Post]/steuerbefehl/initialZustand/",
            routing::post(handle_initial_zustand::<S>),
        )
        .route(
            "/[Post]/steuerbefehl/vorlaeufigePositiveAntwort/",
            routing::post(handle_vorlaeufigepositiveantwort::<S>),
        )
        .route(
            "/[Post]/steuerbefehl/vorlaeufigeNegativeAntwort/",
            routing::post(handle_vorlaeufige_negative_antwort::<S>),
        )
        .route(
            "/[Post]/steuerbefehl/positiveAntwort/",
            routing::post(handle_positive_antwort::<S>),
        )
        .route(
            "/[Post]/steuerbefehl/negativeAntwort/",
            routing::post(handle_negative_antwort::<S>),
        )
        .route(
            "/[Post]/steuerbefehl/informationAnweisung/",
            routing::post(handle_information_anweisung::<S>),
        )
        .route(
            "/[Post]/steuerbefehl/information/",
            routing::post(handle_information::<S>),
        )
        .with_state(state)
}

// ── Query / header extractors ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct KonfigurationQuery {
    #[serde(rename = "locationId")]
    location_id: String,
    #[serde(rename = "commandControl")]
    command_control: String,
}

#[derive(Deserialize)]
struct InitialZustandQuery {
    #[serde(rename = "locationId")]
    location_id: String,
    #[serde(rename = "commandRegular")]
    command_regular: String,
}

#[derive(Deserialize)]
struct NegativeResponseQuery {
    #[serde(rename = "referenceId")]
    reference_id: String,
    #[serde(rename = "locationId")]
    location_id: String,
    #[serde(rename = "resultNegative")]
    result_negative: String,
}

#[derive(Deserialize)]
struct PositiveResponseQuery {
    #[serde(rename = "referenceId")]
    reference_id: String,
    #[serde(rename = "locationId")]
    location_id: String,
    #[serde(rename = "resultPositive")]
    result_positive: String,
}

#[derive(Deserialize)]
struct PreliminaryPositiveQuery {
    #[serde(rename = "referenceId")]
    reference_id: String,
    #[serde(rename = "locationId")]
    location_id: String,
    #[serde(rename = "preliminaryResultPositive")]
    preliminary_result: String,
}

#[derive(Deserialize)]
struct InformationAnweisungQuery {
    #[serde(rename = "referenceId")]
    reference_id: String,
    #[serde(rename = "locationId")]
    location_id: String,
    #[serde(rename = "stateUnknown")]
    state_unknown: String,
}

#[derive(Deserialize)]
struct InformationQuery {
    #[serde(rename = "locationId")]
    location_id: String,
    #[serde(rename = "partnerId")]
    partner_id: i64,
    #[serde(rename = "commandControl")]
    command_control: Option<String>,
    #[serde(rename = "commandRegular")]
    command_regular: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

fn str_header(h: &HeaderMap, name: &str) -> String {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

fn parse_location(s: &str) -> LocationId {
    if s.starts_with('E') {
        LocationId::NetworkLocation(NeloId(s.to_owned()))
    } else {
        LocationId::ControllableResource(SrId(s.to_owned()))
    }
}

async fn handle_konfiguration<S: ControlMeasuresHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<KonfigurationQuery>,
) -> Response {
    let command: CommandControl = match serde_json::from_str(&q.command_control) {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_konfiguration(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            parse_location(&q.location_id),
            command,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_initial_zustand<S: ControlMeasuresHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<InitialZustandQuery>,
) -> Response {
    let command: CommandRegular = match serde_json::from_str(&q.command_regular) {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_initial_zustand(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            parse_location(&q.location_id),
            command,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_vorlaeufigepositiveantwort<S: ControlMeasuresHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<PreliminaryPositiveQuery>,
) -> Response {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        preliminary_state_positive: PreliminaryStatePositive,
    }
    let body: Body = match serde_json::from_str(&q.preliminary_result) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_vorlaeufigepositiveantwort(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            q.reference_id,
            parse_location(&q.location_id),
            body.preliminary_state_positive,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_vorlaeufige_negative_antwort<S: ControlMeasuresHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<NegativeResponseQuery>,
) -> Response {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        state_negative: StateNegative,
        reason_negative: ReasonNegative,
    }
    let body: Body = match serde_json::from_str(&q.result_negative) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_vorlaeufige_negative_antwort(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            q.reference_id,
            parse_location(&q.location_id),
            body.state_negative,
            body.reason_negative,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_positive_antwort<S: ControlMeasuresHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<PositiveResponseQuery>,
) -> Response {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        state_positive: StatePositive,
    }
    let body: Body = match serde_json::from_str(&q.result_positive) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_positive_antwort(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            q.reference_id,
            parse_location(&q.location_id),
            body.state_positive,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_negative_antwort<S: ControlMeasuresHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<NegativeResponseQuery>,
) -> Response {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        state_negative: StateNegative,
        reason_negative: ReasonNegative,
    }
    let body: Body = match serde_json::from_str(&q.result_negative) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_negative_antwort(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            q.reference_id,
            parse_location(&q.location_id),
            body.state_negative,
            body.reason_negative,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_information_anweisung<S: ControlMeasuresHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<InformationAnweisungQuery>,
) -> Response {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        state_unknown: StateUnknown,
    }
    let body: Body = match serde_json::from_str(&q.state_unknown) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_information_anweisung(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            q.reference_id,
            parse_location(&q.location_id),
            body.state_unknown,
        )
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => handler_error(e),
    }
}

async fn handle_information<S: ControlMeasuresHandler>(
    State(svc): State<Arc<S>>,
    headers: HeaderMap,
    Query(q): Query<InformationQuery>,
) -> Response {
    let command_control: Option<CommandControl> = match q
        .command_control
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
    {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let command_regular: Option<CommandRegular> = match q
        .command_regular
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
    {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match svc
        .on_information(
            str_header(&headers, "transactionId"),
            str_header(&headers, "creationDateTime"),
            parse_location(&q.location_id),
            q.partner_id,
            command_control,
            command_regular,
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
