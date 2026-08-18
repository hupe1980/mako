//! The one error type every `billingd` endpoint returns.
//!
//! Every failure renders the same envelope with a **stable machine-readable
//! code**, so a caller matches on it instead of sniffing the body to tell a
//! structured refusal from a bare string:
//!
//! ```json
//! { "error": { "code": "PERIOD_ALREADY_BILLED",
//!              "message": "…",
//!              "record_id": "…" } }
//! ```
//!
//! `mako_service::ApiError` is the workspace default and renders a flat
//! `{error, detail}` pair. billingd needs more than that — a straddling period
//! must name its Stichtage, a blocked validation must carry the engine's
//! warnings, a duplicate must name the record that already exists — so this is
//! the same idea with a structured `extra` payload. Internal detail is logged,
//! never returned.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// A handler failure, as the client sees it.
#[derive(Debug, thiserror::Error)]
pub enum BillingError {
    /// 400 — the request could not be understood.
    #[error("{message}")]
    BadRequest { code: &'static str, message: String },
    /// 404 — no such record in this tenant.
    #[error("{message}")]
    NotFound { code: &'static str, message: String },
    /// 403 — authenticated, but the Cedar policy said no.
    #[error("{message}")]
    Forbidden { code: &'static str, message: String },
    /// 409 — the request conflicts with state that already exists.
    #[error("{message}")]
    Conflict {
        code: &'static str,
        message: String,
        extra: serde_json::Value,
    },
    /// 422 — well-formed, but it cannot be billed as asked.
    #[error("{message}")]
    Unprocessable {
        code: &'static str,
        message: String,
        extra: serde_json::Value,
    },
    /// 502 — an upstream service this calculation depends on did not answer.
    #[error("{service}: {message}")]
    Upstream {
        service: &'static str,
        message: String,
    },
    /// 503 — the service is configured in a way that cannot serve this request.
    #[error("{message}")]
    Unavailable { code: &'static str, message: String },
    /// 500 — an unexpected failure. Logged on render, never returned.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// `Result` alias for handlers and the helpers they call.
pub type BillingResult<T> = Result<T, BillingError>;

impl BillingError {
    /// A 400 with a stable code.
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::BadRequest {
            code,
            message: message.into(),
        }
    }

    /// A 404 with a stable code.
    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::NotFound {
            code,
            message: message.into(),
        }
    }

    /// A 422 with a stable code and no extra payload.
    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::Unprocessable {
            code,
            message: message.into(),
            extra: serde_json::Value::Null,
        }
    }

    /// A 422 whose body carries the machine-readable detail a caller needs to
    /// act — the Stichtage of a straddling period, the engine's warnings.
    pub fn unprocessable_with(
        code: &'static str,
        message: impl Into<String>,
        extra: serde_json::Value,
    ) -> Self {
        Self::Unprocessable {
            code,
            message: message.into(),
            extra,
        }
    }

    /// A 409 with a stable code and no extra payload.
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::Conflict {
            code,
            message: message.into(),
            extra: serde_json::Value::Null,
        }
    }

    /// A 409 that names the record already occupying the slot, so a retrying
    /// caller can reconcile instead of guessing.
    pub fn conflict_with(
        code: &'static str,
        message: impl Into<String>,
        extra: serde_json::Value,
    ) -> Self {
        Self::Conflict {
            code,
            message: message.into(),
            extra,
        }
    }

    /// A 502 naming the upstream that failed.
    pub fn upstream(service: &'static str, message: impl std::fmt::Display) -> Self {
        Self::Upstream {
            service,
            message: message.to_string(),
        }
    }

    /// The HTTP status this failure maps to.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest { .. } => StatusCode::BAD_REQUEST,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::Forbidden { .. } => StatusCode::FORBIDDEN,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Unprocessable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Upstream { .. } => StatusCode::BAD_GATEWAY,
            Self::Unavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The stable code clients match on.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BadRequest { code, .. }
            | Self::NotFound { code, .. }
            | Self::Forbidden { code, .. }
            | Self::Conflict { code, .. }
            | Self::Unprocessable { code, .. }
            | Self::Unavailable { code, .. } => code,
            Self::Upstream { .. } => "UPSTREAM_UNAVAILABLE",
            Self::Internal(_) => "INTERNAL",
        }
    }

    /// The JSON body, without the HTTP envelope — used by the MCP bridge, which
    /// reports the same codes over a different transport.
    #[must_use]
    pub fn body(&self) -> serde_json::Value {
        let message = match self {
            // An internal error's detail is logged by the caller, not returned.
            Self::Internal(_) => "internal server error".to_owned(),
            other => other.to_string(),
        };
        let mut error = serde_json::json!({ "code": self.code(), "message": message });
        if let Self::Upstream { service, .. } = self {
            error["service"] = serde_json::json!(service);
        }
        if let Self::Conflict { extra, .. } | Self::Unprocessable { extra, .. } = self
            && let Some(fields) = extra.as_object()
        {
            for (k, v) in fields {
                error[k.clone()] = v.clone();
            }
        }
        serde_json::json!({ "error": error })
    }
}

impl IntoResponse for BillingError {
    fn into_response(self) -> Response {
        if let Self::Internal(ref e) = self {
            tracing::error!(error = ?e, "billingd: internal error");
        }
        (self.status(), Json(self.body())).into_response()
    }
}

impl From<sqlx::Error> for BillingError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::not_found("NOT_FOUND", "record not found"),
            other => Self::Internal(anyhow::Error::new(other)),
        }
    }
}

/// A blocked or refused engine calculation, as a 422 that names the code and
/// carries every warning behind it.
impl From<energy_billing::EngineError> for BillingError {
    fn from(e: energy_billing::EngineError) -> Self {
        Self::unprocessable_with(
            e.code(),
            e.to_string(),
            serde_json::json!({ "warnings": e.blocking_warnings() }),
        )
    }
}

/// A refused write, as the status the refusal actually is.
///
/// Both named refusals are the *caller's* state, not a server fault: a period
/// that already carries an issued document and a Rechnungsnummer already in use
/// are `409`s. A `500` with a raw database string would tell an operator
/// nothing and invite a retry that can never succeed.
impl From<crate::pg::InsertError> for BillingError {
    fn from(e: crate::pg::InsertError) -> Self {
        use crate::pg::InsertError as E;
        let message = e.to_string();
        match e {
            E::PeriodAlreadyIssued {
                malo_id,
                product_code,
                period_from,
                period_to,
            } => Self::conflict_with(
                "PERIOD_ALREADY_BILLED",
                message,
                serde_json::json!({
                    "malo_id": malo_id,
                    "product_code": product_code,
                    "period_from": period_from.to_string(),
                    "period_to": period_to.to_string(),
                }),
            ),
            E::DuplicateRechnungsnummer(nr) => Self::conflict_with(
                "RECHNUNGSNUMMER_IN_USE",
                message,
                serde_json::json!({ "rechnungsnummer": nr, "legal_basis": "§14 Abs. 4 Nr. 4 UStG" }),
            ),
            E::Other(inner) => Self::Internal(inner),
        }
    }
}

/// A period that straddles a statutory rate boundary, as a 422 that names the
/// Stichtage so the caller can split and retry.
impl From<crate::config::StraddlesRateBoundary> for BillingError {
    fn from(e: crate::config::StraddlesRateBoundary) -> Self {
        Self::unprocessable_with(
            "ZEITRAUM_UEBERSCHREITET_SATZGRENZE",
            e.to_string(),
            serde_json::json!({
                "category": e.category,
                "period_from": e.period_from.to_string(),
                "period_to": e.period_to.to_string(),
                "stichtage": e.stichtage.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "legal_basis": "§28 Abs. 5/6 UStG (Gas/Fernwärme), §10 BEHG",
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every failure renders the same envelope, and the code is the part a
    /// client matches on.
    #[test]
    fn the_envelope_is_the_same_shape_for_every_variant() {
        for err in [
            BillingError::bad_request("INVALID_PERIOD", "period_from must not be after period_to"),
            BillingError::not_found("RECORD_NOT_FOUND", "billing record not found"),
            BillingError::conflict("PERIOD_ALREADY_BILLED", "already issued"),
            BillingError::unprocessable("NO_METER_DATA", "no meter data"),
            BillingError::upstream("edmd", "connection refused"),
        ] {
            let body = err.body();
            assert!(body["error"]["code"].is_string(), "{body}");
            assert!(body["error"]["message"].is_string(), "{body}");
        }
    }

    /// The extra payload is flattened into `error`, so a caller reads
    /// `error.stichtage` and not `error.extra.stichtage`.
    #[test]
    fn structured_detail_is_part_of_the_error_object() {
        let err = BillingError::conflict_with(
            "PERIOD_ALREADY_BILLED",
            "already issued",
            serde_json::json!({ "record_id": "abc" }),
        );
        assert_eq!(err.body()["error"]["record_id"], "abc");
        assert_eq!(err.status(), StatusCode::CONFLICT);
    }

    /// An internal error's detail is logged, never returned.
    #[test]
    fn internal_detail_is_not_leaked() {
        let err = BillingError::Internal(anyhow::anyhow!("postgres://user:secret@host/db"));
        let body = err.body().to_string();
        assert!(!body.contains("secret"), "{body}");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
