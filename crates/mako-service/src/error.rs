//! Shared HTTP error type for mako handlers.
//!
//! Return [`ApiResult<T>`] and use `?`: every error becomes the same JSON
//! problem body — `{ "error", "detail", … }` — with the right status.
//!
//! Three kinds of failure, three bodies:
//!
//! | Failure | Variant | Body |
//! |---|---|---|
//! | the caller's mistake | `BadRequest`, `Unprocessable`, `Conflict`, `NotFound`, `PreconditionFailed`, `PayloadTooLarge`, … | `detail` is the sentence the handler wrote for the caller |
//! | not now, or not in this build | `TooManyRequests`, `Unavailable`, `NotImplemented` | `detail` says whether a retry can help |
//! | this service's own fault | [`ApiError::Internal`] | `detail` is `"internal server error"`, plus an `error_id` |
//! | a service this one calls failed | [`ApiError::Upstream`] | `detail` names the upstream service only, plus an `error_id` |
//!
//! Any of them may carry machine-readable keys beside `error` and `detail`
//! ([`ApiError::with_fields`], [`ApiError::with_code`]): a stable `code` a
//! client branches on — `error` is only the status's reason phrase — and the
//! identifiers the refusal is about.
//!
//! An internal or upstream error's own text — a database message, a
//! constraint name, the body another service answered with — never reaches
//! the response. It is logged at `error` level beside the same `error_id`
//! the body carries, so the identifier a caller reports finds the log line.
//!
//! ```rust,no_run
//! use mako_service::{ApiError, ApiResult, Json};
//!
//! async fn get_thing(pool: &sqlx::PgPool, id: String) -> ApiResult<Json<String>> {
//!     if id.is_empty() {
//!         return Err(ApiError::bad_request("id must not be empty"));
//!     }
//!     // `?` on a sqlx::Error maps RowNotFound → 404, anything else → 500 (logged)
//!     let name: String = sqlx::query_scalar("SELECT name FROM thing WHERE id = $1")
//!         .bind(&id)
//!         .fetch_one(pool)
//!         .await?;
//!     Ok(Json(name))
//! }
//! ```

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// A handler error that renders as a standard JSON problem response.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// 404 — the resource does not exist.
    #[error("not found")]
    NotFound,
    /// 404 with a sentence for the caller — what was looked for, or where the
    /// resource comes from when it does not exist yet. Built by
    /// [`ApiError::not_found`].
    #[error("{0}")]
    Missing(String),
    /// 401 — authentication is missing or invalid.
    #[error("unauthorized")]
    Unauthorized,
    /// 401 with the reason the credential was refused. Built by
    /// [`ApiError::unauthorized`]; the reason describes the caller's own
    /// credential and never this deployment.
    #[error("{0}")]
    Unauthenticated(String),
    /// 403 — authenticated but not permitted.
    #[error("forbidden")]
    Forbidden,
    /// 403 with the reason the policy gave — the principal, action and tenant
    /// a Cedar denial names, so an operator can tell a wrong role from a wrong
    /// policy. Built by [`ApiError::forbidden`].
    #[error("{0}")]
    Denied(String),
    /// 400 — malformed request.
    #[error("{0}")]
    BadRequest(String),
    /// 412 — a precondition the caller stated (`If-Match`) does not hold.
    #[error("{0}")]
    PreconditionFailed(String),
    /// 413 — the request is larger than this route accepts.
    #[error("{0}")]
    PayloadTooLarge(String),
    /// 415 — the body's media type is not one this route reads.
    ///
    /// Distinct from a 400 because the client's fix is different: the body may
    /// be perfectly good JSON that arrived without `Content-Type:
    /// application/json`, and telling it to fix the *body* sends it looking in
    /// the wrong place.
    #[error("{0}")]
    UnsupportedMediaType(String),
    /// 422 — well-formed but semantically invalid.
    #[error("{0}")]
    Unprocessable(String),
    /// 409 — conflicts with current state (duplicate, version race).
    #[error("{0}")]
    Conflict(String),
    /// 429 — the request is to be sent again later. Built by
    /// [`ApiError::too_many_requests`].
    #[error("{0}")]
    TooManyRequests(String),
    /// 501 — the route exists and this build or release does not serve it, so
    /// a retry will not help. Built by [`ApiError::not_implemented`].
    #[error("{0}")]
    NotImplemented(String),
    /// 503 — this service cannot answer the request safely right now and a
    /// retry may succeed (a store it must consult is unavailable). The sentence
    /// is the caller's; the cause is logged by the handler. A dependency that
    /// is down is [`ApiError::Upstream`] instead.
    #[error("{0}")]
    Unavailable(String),
    /// 500 — an unexpected failure of this service.
    ///
    /// The error is logged with a fresh `error_id` (UUID v4) when the response
    /// is rendered; the body carries that id and `"internal server error"`,
    /// never the error's own text. Built by `?` on an `anyhow::Error` or a
    /// non-`RowNotFound` `sqlx::Error`, or by [`ApiError::internal`].
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
    /// 502 / 503 / 504 — a service this one calls failed.
    ///
    /// The body names `service` and nothing else the upstream said: its status
    /// and body go to the log beside the `error_id`, because an upstream's
    /// error text is that service's internals — and may carry a secret it was
    /// sent. Built by [`ApiError::upstream`], [`ApiError::upstream_status`],
    /// [`ApiError::upstream_request`] or [`ApiError::upstream_unreachable`].
    #[error("upstream service {service} failed")]
    Upstream {
        /// The upstream's name as the caller knows it (`marktd`, `edmd`, …).
        service: String,
        /// Which status the failure answers with.
        failure: UpstreamFailure,
        /// What went wrong, for the log only.
        cause: anyhow::Error,
    },
    /// Any other variant, with machine-readable keys added to its body.
    ///
    /// The keys are merged into the problem body beside `error` and `detail`,
    /// so a client can branch on *why* without parsing prose: a stable `code`
    /// ([`ApiError::with_code`]), the JSON-paths the BO4E gate refused, the
    /// `process_id` a duplicate names. `error`, `detail`, `error_id` and
    /// `upstream` are this type's contract and are never overwritten. Built by
    /// [`ApiError::with_fields`], [`ApiError::with_code`],
    /// [`ApiError::unprocessable_with`] or [`ApiError::conflict_with`].
    #[error("{inner}")]
    WithFields {
        /// The error whose status and sentence the response carries.
        inner: Box<ApiError>,
        /// The extra top-level keys.
        fields: serde_json::Map<String, serde_json::Value>,
    },
}

/// The problem-body keys [`ApiError::WithFields`] never overwrites.
const RESERVED_KEYS: &[&str] = &["error", "detail", "error_id", "upstream"];

/// How an upstream failed — decides the status of an [`ApiError::Upstream`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamFailure {
    /// 502 — the upstream answered, with an error status or a body this
    /// service cannot read.
    BadResponse,
    /// 503 — the upstream could not be reached (connection refused, DNS).
    Unreachable,
    /// 504 — the upstream did not answer in time.
    Timeout,
}

/// `Result` alias for handlers that return [`ApiError`].
pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    /// A 400 with a client-visible message.
    #[must_use]
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    /// A 422 with a client-visible message.
    #[must_use]
    pub fn unprocessable(msg: impl Into<String>) -> Self {
        Self::Unprocessable(msg.into())
    }

    /// A 422 with a client-visible message and machine-readable detail.
    ///
    /// `detail`'s keys are merged into the problem body. Use it wherever the
    /// reason for the refusal is structured — a JSON-path, a rule name, a
    /// field — rather than only a sentence; a stable machine code goes in
    /// its `code` key.
    #[must_use]
    pub fn unprocessable_with(msg: impl Into<String>, detail: serde_json::Value) -> Self {
        Self::unprocessable(msg).with_fields(detail)
    }

    /// A 404 with a sentence for the caller instead of the bare "not found".
    #[must_use]
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::Missing(msg.into())
    }

    /// A 401 naming why the caller's credential was refused.
    #[must_use]
    pub fn unauthorized(reason: impl Into<String>) -> Self {
        Self::Unauthenticated(reason.into())
    }

    /// A 412: the caller's `If-Match` (or other precondition) does not hold.
    #[must_use]
    pub fn precondition_failed(msg: impl Into<String>) -> Self {
        Self::PreconditionFailed(msg.into())
    }

    /// A 413 naming the limit the request exceeded.
    #[must_use]
    pub fn payload_too_large(msg: impl Into<String>) -> Self {
        Self::PayloadTooLarge(msg.into())
    }

    /// A 429: send the request again later.
    #[must_use]
    pub fn too_many_requests(msg: impl Into<String>) -> Self {
        Self::TooManyRequests(msg.into())
    }

    /// A 501: this build or release does not serve the route.
    #[must_use]
    pub fn not_implemented(msg: impl Into<String>) -> Self {
        Self::NotImplemented(msg.into())
    }

    /// A 503 for a request this service cannot answer safely right now.
    #[must_use]
    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self::Unavailable(msg.into())
    }

    /// A 409 with a stable machine `code` and extra keys — the
    /// `process_id` of the process a duplicate collides with, say.
    #[must_use]
    pub fn conflict_with(
        code: impl Into<String>,
        msg: impl Into<String>,
        fields: serde_json::Value,
    ) -> Self {
        Self::conflict(msg).with_code(code).with_fields(fields)
    }

    /// This error with `fields`' keys added to its body. A non-object adds
    /// nothing; a reserved key (`error`, `detail`, `error_id`, `upstream`) is
    /// dropped; a later key replaces an earlier one of the same name.
    #[must_use]
    pub fn with_fields(self, fields: serde_json::Value) -> Self {
        let serde_json::Value::Object(extra) = fields else {
            return self;
        };
        let (inner, mut merged) = match self {
            Self::WithFields { inner, fields } => (inner, fields),
            other => (Box::new(other), serde_json::Map::new()),
        };
        for (k, v) in extra {
            if !RESERVED_KEYS.contains(&k.as_str()) {
                merged.insert(k, v);
            }
        }
        Self::WithFields {
            inner,
            fields: merged,
        }
    }

    /// This error with a stable machine-readable `code` key — the value a
    /// client branches on, where `error` is the status's reason phrase.
    #[must_use]
    pub fn with_code(self, code: impl Into<String>) -> Self {
        self.with_fields(serde_json::json!({ "code": code.into() }))
    }

    /// The `code` key this error carries, if any.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::WithFields { fields, .. } => fields.get("code").and_then(|c| c.as_str()),
            _ => None,
        }
    }

    /// A 403 naming why — the one denial every handler returns, instead of a
    /// per-service `forbidden` helper building its own body.
    #[must_use]
    pub fn forbidden(reason: impl Into<String>) -> Self {
        Self::Denied(reason.into())
    }

    /// A 409 with a client-visible message.
    #[must_use]
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    /// A 500 for a failure of this service — `.map_err(ApiError::internal)`.
    ///
    /// For a `sqlx::Error` prefer `?`, which keeps `RowNotFound` a 404.
    #[must_use]
    pub fn internal(err: impl Into<anyhow::Error>) -> Self {
        Self::Internal(err.into())
    }

    /// A 502: `service` answered, but not with something this service can use
    /// (a body that does not parse, a field it lacks). `cause` is logged.
    #[must_use]
    pub fn upstream(service: impl Into<String>, cause: impl Into<anyhow::Error>) -> Self {
        Self::Upstream {
            service: service.into(),
            failure: UpstreamFailure::BadResponse,
            cause: cause.into(),
        }
    }

    /// A 502 for an upstream that answered with an error status. `status` and
    /// `body` are logged; neither reaches the caller.
    ///
    /// Map a status the caller should see — a 404 for a resource that does not
    /// exist upstream either — to its own variant before reaching for this.
    #[must_use]
    pub fn upstream_status(
        service: impl Into<String>,
        status: impl std::fmt::Display,
        body: impl std::fmt::Display,
    ) -> Self {
        let service = service.into();
        let cause = anyhow::anyhow!("{service} answered {status}: {body}");
        Self::Upstream {
            service,
            failure: UpstreamFailure::BadResponse,
            cause,
        }
    }

    /// A 503 for an upstream that could not be reached. `cause` is logged.
    #[must_use]
    pub fn upstream_unreachable(
        service: impl Into<String>,
        cause: impl Into<anyhow::Error>,
    ) -> Self {
        Self::Upstream {
            service: service.into(),
            failure: UpstreamFailure::Unreachable,
            cause: cause.into(),
        }
    }

    /// The `reqwest` failure of a call to `service`, classified: a timeout is a
    /// 504, a connection failure a 503, anything else (a body that does not
    /// decode, `error_for_status`) a 502. The error is logged.
    #[must_use]
    pub fn upstream_request(service: impl Into<String>, err: reqwest::Error) -> Self {
        let failure = if err.is_timeout() {
            UpstreamFailure::Timeout
        } else if err.is_connect() {
            UpstreamFailure::Unreachable
        } else {
            UpstreamFailure::BadResponse
        };
        Self::Upstream {
            service: service.into(),
            failure,
            cause: err.into(),
        }
    }

    /// The HTTP status this error maps to.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound | Self::Missing(_) => StatusCode::NOT_FOUND,
            Self::Unauthorized | Self::Unauthenticated(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden | Self::Denied(_) => StatusCode::FORBIDDEN,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Upstream { failure, .. } => match failure {
                UpstreamFailure::BadResponse => StatusCode::BAD_GATEWAY,
                UpstreamFailure::Unreachable => StatusCode::SERVICE_UNAVAILABLE,
                UpstreamFailure::Timeout => StatusCode::GATEWAY_TIMEOUT,
            },
            Self::WithFields { inner, .. } => inner.status(),
        }
    }

    /// The status and problem body; an internal or upstream cause is logged
    /// here, under the `error_id` the body carries.
    fn render(self) -> (StatusCode, serde_json::Value) {
        let status = self.status();
        let mut body = serde_json::json!({
            "error": status.canonical_reason().unwrap_or("error"),
        });
        // Never leak an internal or upstream error's text to the client — log
        // it under a fresh id and put only the id in the body, so the id a
        // caller reports finds the log line. Client-facing variants carry a
        // sentence written for the caller.
        match self {
            Self::WithFields { inner, fields } => {
                let (_, mut body) = inner.render();
                if let Some(obj) = body.as_object_mut() {
                    for (k, v) in fields {
                        if !RESERVED_KEYS.contains(&k.as_str()) {
                            obj.insert(k, v);
                        }
                    }
                }
                return (status, body);
            }
            Self::Internal(e) => {
                let error_id = uuid::Uuid::new_v4();
                tracing::error!(%error_id, error = ?e, "handler internal error");
                body["detail"] = "internal server error".into();
                body["error_id"] = error_id.to_string().into();
            }
            ref up @ Self::Upstream {
                ref service,
                ref cause,
                ..
            } => {
                let error_id = uuid::Uuid::new_v4();
                tracing::error!(
                    %error_id,
                    upstream = %service,
                    error = ?cause,
                    "upstream service failed"
                );
                body["detail"] = up.to_string().into();
                body["upstream"] = service.as_str().into();
                body["error_id"] = error_id.to_string().into();
            }
            other => body["detail"] = other.to_string().into(),
        }
        (status, body)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = self.render();
        (status, Json(body)).into_response()
    }
}

/// Map database errors: a missing row is a 404; everything else is an internal
/// error (logged on render, not leaked).
impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound,
            other => Self::Internal(anyhow::Error::new(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn variants_map_to_expected_status_and_json_body() {
        for (err, code) in [
            (ApiError::NotFound, StatusCode::NOT_FOUND),
            (ApiError::Unauthorized, StatusCode::UNAUTHORIZED),
            (ApiError::Forbidden, StatusCode::FORBIDDEN),
            (ApiError::bad_request("x"), StatusCode::BAD_REQUEST),
            (
                ApiError::unprocessable("x"),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (ApiError::conflict("x"), StatusCode::CONFLICT),
            (ApiError::not_found("x"), StatusCode::NOT_FOUND),
            (ApiError::unauthorized("x"), StatusCode::UNAUTHORIZED),
            (
                ApiError::precondition_failed("x"),
                StatusCode::PRECONDITION_FAILED,
            ),
            (
                ApiError::payload_too_large("x"),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
            (
                ApiError::too_many_requests("x"),
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (ApiError::not_implemented("x"), StatusCode::NOT_IMPLEMENTED),
            (ApiError::unavailable("x"), StatusCode::SERVICE_UNAVAILABLE),
            (
                ApiError::conflict_with("c", "x", serde_json::json!({})),
                StatusCode::CONFLICT,
            ),
        ] {
            let resp = err.into_response();
            assert_eq!(resp.status(), code);
        }
    }

    /// Render `err` with a log subscriber installed; return the status, the
    /// parsed body and everything that was logged.
    async fn render_logged(err: ApiError) -> (StatusCode, serde_json::Value, String) {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("log buffer").extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = Buf(Arc::new(Mutex::new(Vec::new())));
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let resp = tracing::subscriber::with_default(subscriber, || err.into_response());
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap();
        let log = String::from_utf8(buf.0.lock().expect("log buffer").clone()).unwrap();
        (status, body, log)
    }

    #[tokio::test]
    async fn internal_error_is_not_leaked() {
        let (status, body, _) = render_logged(ApiError::Internal(anyhow::anyhow!(
            "secret db dsn leaked here"
        )))
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            !body.to_string().contains("secret db dsn"),
            "internal detail must not leak: {body}"
        );
        assert_eq!(body["detail"], "internal server error");
    }

    /// The id in the body is a UUID v4 and is the one the log line carries,
    /// beside the full error — so the id a caller reports finds the cause.
    #[tokio::test]
    async fn an_internal_error_carries_an_error_id_the_log_line_carries() {
        let (_, body, log) = render_logged(ApiError::internal(anyhow::anyhow!(
            "duplicate key value violates unique constraint \"malo_pkey\""
        )))
        .await;
        let id = body["error_id"].as_str().expect("error_id in the body");
        let parsed = uuid::Uuid::parse_str(id).expect("error_id is a UUID");
        assert_eq!(parsed.get_version_num(), 4, "{id}");
        assert!(log.contains(id), "the log line must carry {id}: {log}");
        assert!(log.contains("malo_pkey"), "the log keeps the error: {log}");
        assert!(!body.to_string().contains("malo_pkey"), "{body}");
    }

    #[tokio::test]
    async fn every_internal_response_has_its_own_error_id() {
        let (_, a, _) = render_logged(ApiError::internal(anyhow::anyhow!("x"))).await;
        let (_, b, _) = render_logged(ApiError::internal(anyhow::anyhow!("x"))).await;
        assert_ne!(a["error_id"], b["error_id"]);
    }

    /// A database error's text — table, constraint, SQL — stays in the log.
    #[tokio::test]
    async fn a_database_error_renders_without_its_text() {
        let err: ApiError =
            sqlx::Error::Protocol("relation \"meter_billing_periods\" does not exist".into())
                .into();
        let (status, body, log) = render_logged(err).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            !body.to_string().contains("meter_billing_periods"),
            "{body}"
        );
        assert!(log.contains("meter_billing_periods"), "{log}");
        assert!(log.contains(body["error_id"].as_str().unwrap()), "{log}");
    }

    /// An upstream's body may carry what it was sent — a token, a DSN. The
    /// response names the upstream and nothing it said; the log keeps both.
    #[tokio::test]
    async fn an_upstream_failure_names_the_service_and_echoes_nothing() {
        let (status, body, log) = render_logged(ApiError::upstream_status(
            "marktd",
            StatusCode::INTERNAL_SERVER_ERROR,
            "pq: password authentication failed for user secret-user",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"], "Bad Gateway");
        assert_eq!(body["detail"], "upstream service marktd failed");
        assert_eq!(body["upstream"], "marktd");
        assert!(!body.to_string().contains("secret-user"), "{body}");
        let id = body["error_id"].as_str().expect("error_id in the body");
        assert!(log.contains(id) && log.contains("secret-user"), "{log}");
    }

    #[test]
    fn upstream_failures_map_to_their_gateway_status() {
        for (failure, code) in [
            (UpstreamFailure::BadResponse, StatusCode::BAD_GATEWAY),
            (
                UpstreamFailure::Unreachable,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (UpstreamFailure::Timeout, StatusCode::GATEWAY_TIMEOUT),
        ] {
            let err = ApiError::Upstream {
                service: "edmd".into(),
                failure,
                cause: anyhow::anyhow!("x"),
            };
            assert_eq!(err.status(), code);
        }
        assert_eq!(
            ApiError::upstream_unreachable("edmd", anyhow::anyhow!("refused")).status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    /// A connection failure is a 503, classified from the `reqwest` error.
    #[tokio::test]
    async fn a_refused_connection_is_unreachable() {
        // Bind and drop: the port is free and nothing listens on it.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .expect_err("nothing listens");
        let api = ApiError::upstream_request("marktd", err);
        assert_eq!(api.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// A client-facing variant carries no `error_id`: there is no log line to
    /// find, and the sentence is the whole answer.
    #[tokio::test]
    async fn a_client_error_has_no_error_id() {
        let (_, body, _) = render_logged(ApiError::bad_request("from must precede to")).await;
        assert_eq!(body["detail"], "from must precede to");
        assert!(body.get("error_id").is_none(), "{body}");
    }

    #[tokio::test]
    async fn a_denial_is_a_403_that_names_its_reason() {
        let resp = ApiError::forbidden("read-melo-msb denied for tenant t1").into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "Forbidden");
        assert_eq!(body["detail"], "read-melo-msb denied for tenant t1");
    }

    /// Every sentence-carrying variant renders the caller's sentence as
    /// `detail` under its status's reason phrase, and no `error_id`.
    #[tokio::test]
    async fn each_client_variant_renders_its_sentence() {
        for (err, reason) in [
            (ApiError::not_found("no MaLo 51238696012"), "Not Found"),
            (ApiError::unauthorized("token expired"), "Unauthorized"),
            (
                ApiError::precondition_failed("If-Match 3, current 4"),
                "Precondition Failed",
            ),
            (
                ApiError::payload_too_large("batch too large: 9 > 8"),
                "Payload Too Large",
            ),
            (
                ApiError::too_many_requests("resend later"),
                "Too Many Requests",
            ),
            (
                ApiError::not_implemented("NB role not compiled in"),
                "Not Implemented",
            ),
            (ApiError::unavailable("retry"), "Service Unavailable"),
        ] {
            let sentence = err.to_string();
            let (_, body, _) = render_logged(err).await;
            assert_eq!(body["error"], reason, "{body}");
            assert_eq!(body["detail"], sentence.as_str(), "{body}");
            assert!(body.get("error_id").is_none(), "{body}");
        }
    }

    /// A code and extra keys ride beside the sentence; the problem shape's own
    /// keys are not the caller's to replace.
    #[tokio::test]
    async fn fields_ride_beside_the_sentence_and_never_replace_it() {
        let err = ApiError::conflict_with(
            "duplicate_process",
            "an active process exists",
            serde_json::json!({
                "process_id": "p-1",
                "error": "overwritten?",
                "detail": "overwritten?",
                "error_id": "forged",
            }),
        );
        assert_eq!(err.code(), Some("duplicate_process"));
        let (status, body, _) = render_logged(err).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "Conflict");
        assert_eq!(body["detail"], "an active process exists");
        assert_eq!(body["code"], "duplicate_process");
        assert_eq!(body["process_id"], "p-1");
        assert!(body.get("error_id").is_none(), "{body}");
    }

    /// A 429 carries the body a retrying emitter reads beside the sentence.
    #[tokio::test]
    async fn a_too_many_requests_carries_its_body() {
        let err = ApiError::too_many_requests("no specialist admitted")
            .with_fields(serde_json::json!({ "runs": [] }));
        let (status, body, _) = render_logged(err).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["runs"], serde_json::json!([]));
    }

    /// A 404 may say where the resource comes from.
    #[tokio::test]
    async fn a_not_found_carries_a_hint() {
        let err = ApiError::not_found("no billable reading in the period")
            .with_fields(serde_json::json!({ "hinweis": "Saldo unbekannt, nicht null" }));
        let (status, body, _) = render_logged(err).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["detail"], "no billable reading in the period");
        assert_eq!(body["hinweis"], "Saldo unbekannt, nicht null");
    }

    /// Fields on an internal error keep its rules: the id is logged and
    /// rendered, the cause stays in the log.
    #[tokio::test]
    async fn fields_on_an_internal_error_keep_the_error_id_rule() {
        let err = ApiError::internal(anyhow::anyhow!("secret-dsn")).with_code("store_failed");
        let (status, body, log) = render_logged(err).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["code"], "store_failed");
        assert_eq!(body["detail"], "internal server error");
        let id = body["error_id"].as_str().expect("error_id");
        assert!(log.contains(id) && log.contains("secret-dsn"), "{log}");
        assert!(!body.to_string().contains("secret-dsn"), "{body}");
    }

    #[test]
    fn row_not_found_maps_to_404() {
        let err: ApiError = sqlx::Error::RowNotFound.into();
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
    }
}
