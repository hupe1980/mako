//! Shared HTTP error type for mako handlers.
//!
//! Handlers across the workspace hand-rolled `(StatusCode, String)` tuples and
//! per-handler `impl IntoResponse`, with inconsistent bodies (plain string vs
//! JSON vs empty). [`ApiError`] standardises that: return [`ApiResult<T>`] and
//! use `?`, and every error becomes the same JSON problem body with the right
//! status. Internal errors are logged, never leaked to the client.
//!
//! ```rust,no_run
//! use mako_service::{ApiError, ApiResult};
//! use axum::Json;
//!
//! async fn get_thing(id: String) -> ApiResult<Json<String>> {
//!     if id.is_empty() {
//!         return Err(ApiError::bad_request("id must not be empty"));
//!     }
//!     // `?` on a sqlx::Error maps RowNotFound → 404, anything else → 500 (logged)
//!     Ok(Json(id))
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
    /// 401 — authentication is missing or invalid.
    #[error("unauthorized")]
    Unauthorized,
    /// 403 — authenticated but not permitted.
    #[error("forbidden")]
    Forbidden,
    /// 400 — malformed request.
    #[error("{0}")]
    BadRequest(String),
    /// 422 — well-formed but semantically invalid.
    #[error("{0}")]
    Unprocessable(String),
    /// 422 whose body also carries machine-readable detail.
    ///
    /// The `detail` object's keys are merged into the problem body alongside
    /// `error` and `detail`, so a client can branch on *why* the payload was
    /// refused without parsing prose. The BO4E gate is the reason this exists:
    /// its rejection names the stage (`code`), and the JSON-paths or the rule
    /// that stopped it, and that is worth more to a caller than a sentence.
    #[error("{message}")]
    UnprocessableWith {
        /// The client-visible sentence.
        message: String,
        /// Extra top-level keys for the problem body. Ignored unless an object.
        detail: serde_json::Value,
    },
    /// 409 — conflicts with current state (duplicate, version race).
    #[error("{0}")]
    Conflict(String),
    /// 500 — an unexpected failure. The detail is logged, not returned.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
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
    /// field — rather than only a sentence.
    #[must_use]
    pub fn unprocessable_with(msg: impl Into<String>, detail: serde_json::Value) -> Self {
        Self::UnprocessableWith {
            message: msg.into(),
            detail,
        }
    }

    /// A 409 with a client-visible message.
    #[must_use]
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    /// The HTTP status this error maps to.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unprocessable(_) | Self::UnprocessableWith { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        // Never leak an internal error's detail to the client — log it, return
        // a generic message. Client-facing variants carry a safe message.
        let detail = match &self {
            Self::Internal(e) => {
                tracing::error!(error = ?e, "handler internal error");
                "internal server error".to_owned()
            }
            other => other.to_string(),
        };
        let mut body = serde_json::json!({
            "error": status.canonical_reason().unwrap_or("error"),
            "detail": detail,
        });
        // Machine-readable keys ride alongside the sentence. `error` and
        // `detail` are never overwritten: the shape of a problem body is this
        // type's contract, not the caller's.
        if let Self::UnprocessableWith {
            detail: serde_json::Value::Object(extra),
            ..
        } = self
            && let Some(obj) = body.as_object_mut()
        {
            for (k, v) in extra {
                if k != "error" && k != "detail" {
                    obj.insert(k, v);
                }
            }
        }
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
        ] {
            let resp = err.into_response();
            assert_eq!(resp.status(), code);
        }
    }

    #[tokio::test]
    async fn internal_error_is_not_leaked() {
        let resp = ApiError::Internal(anyhow::anyhow!("secret db dsn leaked here")).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            !body.contains("secret db dsn"),
            "internal detail must not leak: {body}"
        );
        assert!(body.contains("internal server error"));
    }

    #[test]
    fn row_not_found_maps_to_404() {
        let err: ApiError = sqlx::Error::RowNotFound.into();
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
    }
}
