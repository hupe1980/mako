//! The one error type every `outputd` endpoint returns.
//!
//! Every failure renders the same envelope with a stable code, and a template
//! that does not compile carries its diagnostics as a list: they are structured
//! (`path:line:col: message`, with hints and a call trace), and joined into one
//! string they are unusable to the editor the operator writes the template in.
//!
//! ```json
//! { "error": { "code": "TEMPLATE_DID_NOT_COMPILE",
//!              "message": "the template did not render",
//!              "diagnostics": ["/template.typ:12:4: unknown variable: invoce"] } }
//! ```
//!
//! A `500` says nothing about its cause and carries an `error_id` instead —
//! the id the log line recording the cause carries:
//!
//! ```json
//! { "error": { "code": "INTERNAL", "message": "internal server error",
//!              "error_id": "0b9c6f0e-6a0e-4f43-9d55-2f1d3c4b5a69" } }
//! ```

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mako_service::Json;

/// A handler failure, as the client sees it.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    /// 400 — the request could not be understood.
    #[error("{message}")]
    BadRequest { code: &'static str, message: String },
    /// 403 — authenticated, but the Cedar policy said no.
    #[error("{message}")]
    Forbidden { code: &'static str, message: String },
    /// 404 — no such template for this tenant.
    #[error("{message}")]
    NotFound { code: &'static str, message: String },
    /// 409 — the request conflicts with what is already stored.
    #[error("{message}")]
    Conflict { code: &'static str, message: String },
    /// 422 — well-formed, but it cannot be rendered or stored as asked.
    #[error("{message}")]
    Unprocessable {
        code: &'static str,
        message: String,
        /// Compiler diagnostics, each already pointing at a line of the
        /// operator's own file. Empty for refusals that are not a compile
        /// failure.
        diagnostics: Vec<String>,
    },
    /// 500 — an unexpected failure. Logged on render under the `error_id` the
    /// body carries; its text is never returned.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// `Result` alias for handlers and the helpers they call.
pub type OutputResult<T> = Result<T, OutputError>;

impl OutputError {
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

    /// A 409 with a stable code.
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::Conflict {
            code,
            message: message.into(),
        }
    }

    /// A 422 that carries no diagnostics — a refusal decided before rendering.
    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::Unprocessable {
            code,
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }

    /// A 422 carrying the compiler's own diagnostics, one per entry.
    pub fn diagnostics(
        code: &'static str,
        message: impl Into<String>,
        diagnostics: Vec<String>,
    ) -> Self {
        Self::Unprocessable {
            code,
            message: message.into(),
            diagnostics,
        }
    }

    /// The HTTP status this failure maps to.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest { .. } => StatusCode::BAD_REQUEST,
            Self::Forbidden { .. } => StatusCode::FORBIDDEN,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Unprocessable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The stable code clients match on.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BadRequest { code, .. }
            | Self::Forbidden { code, .. }
            | Self::NotFound { code, .. }
            | Self::Conflict { code, .. }
            | Self::Unprocessable { code, .. } => code,
            Self::Internal(_) => "INTERNAL",
        }
    }

    /// The JSON body, without the HTTP envelope.
    #[must_use]
    pub fn body(&self) -> serde_json::Value {
        let message = match self {
            // An internal error's detail is logged, never returned.
            Self::Internal(_) => "internal server error".to_owned(),
            other => other.to_string(),
        };
        let mut error = serde_json::json!({ "code": self.code(), "message": message });
        if let Self::Unprocessable { diagnostics, .. } = self
            && !diagnostics.is_empty()
        {
            error["diagnostics"] = serde_json::json!(diagnostics);
        }
        serde_json::json!({ "error": error })
    }
}

impl IntoResponse for OutputError {
    /// An internal error is logged under a fresh `error_id` (UUID v4) and the
    /// body carries only the id, so the id a caller reports finds the log line
    /// and the cause never reaches the caller. A client error carries no id:
    /// no log line stands behind it.
    fn into_response(self) -> Response {
        let mut body = self.body();
        if let Self::Internal(ref e) = self {
            let error_id = uuid::Uuid::new_v4();
            tracing::error!(%error_id, error = ?e, "outputd: internal error");
            body["error"]["error_id"] = error_id.to_string().into();
        }
        (self.status(), Json(body)).into_response()
    }
}

impl From<sqlx::Error> for OutputError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::not_found("NOT_FOUND", "not found"),
            other => Self::Internal(anyhow::Error::new(other)),
        }
    }
}

/// A render failure, as the status it actually is.
///
/// Every variant is the *caller's* input — an operator's template that does not
/// compile, a standard that cannot carry the payload, an attachment name that
/// is not a name, a budget that ran out. None of them is a server fault, and a
/// compile failure hands back the diagnostics as a list rather than as prose.
impl From<crate::document::RenderError> for OutputError {
    fn from(e: crate::document::RenderError) -> Self {
        use crate::document::RenderError as R;
        match e {
            R::Compile(diagnostics) => Self::diagnostics(
                "TEMPLATE_DID_NOT_COMPILE",
                "the template did not render",
                diagnostics,
            ),
            R::Standard(message) => Self::unprocessable("PDF_STANDARD_UNUSABLE", message),
            R::Attachment(message) => Self::unprocessable("ATTACHMENT_NAME_INVALID", message),
            R::Date(date) => Self::unprocessable(
                "DATE_NOT_REPRESENTABLE",
                format!("document date {date} is not a representable PDF date"),
            ),
            R::Timeout(budget) => Self::unprocessable(
                "RENDER_BUDGET_EXCEEDED",
                format!(
                    "the template did not finish rendering within {budget:?} — it is doing far \
                     more work than one document needs, or the renderer is saturated"
                ),
            ),
        }
    }
}

/// A refused store write, as the status it actually is.
impl From<crate::template_store::StoreError> for OutputError {
    fn from(e: crate::template_store::StoreError) -> Self {
        use crate::template_store::StoreError as S;
        let message = e.to_string();
        match e {
            S::IdentityCollision { .. } => Self::conflict("TEMPLATE_IDENTITY_TAKEN", message),
            S::NotPublished(..) => Self::unprocessable("TEMPLATE_NOT_PUBLISHED", message),
            S::Db(inner) => Self::Internal(anyhow::Error::new(inner)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compiler diagnostics reach the client as a list, not as a blob.
    ///
    /// This is the difference between an operator's editor being able to jump
    /// to line 12 and an operator squinting at a paragraph.
    #[test]
    fn a_compile_failure_carries_its_diagnostics_separately() {
        let err: OutputError = crate::document::RenderError::Compile(vec![
            "/template.typ:12:4: unknown variable: invoce".to_owned(),
            "/template.typ:19:1: expected content".to_owned(),
        ])
        .into();
        let body = err.body();
        assert_eq!(body["error"]["code"], "TEMPLATE_DID_NOT_COMPILE");
        assert_eq!(
            body["error"]["diagnostics"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// A refusal that is not a compile failure carries no empty list.
    #[test]
    fn a_plain_refusal_has_no_diagnostics_key() {
        let err = OutputError::unprocessable("PDF_STANDARD_UNUSABLE", "a-2b cannot carry a file");
        assert!(err.body()["error"].get("diagnostics").is_none());
    }

    /// Render `err` with a log subscriber installed; return the status, the
    /// parsed body and everything that was logged.
    async fn render_logged(err: OutputError) -> (StatusCode, serde_json::Value, String) {
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
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = serde_json::from_slice(&bytes).expect("a JSON body");
        let log = String::from_utf8(buf.0.lock().expect("log buffer").clone()).expect("utf-8");
        (status, body, log)
    }

    /// A 500 carries an `error_id`, a UUID v4, and the log line that records
    /// the cause carries the same id — so the id a caller reports finds the
    /// cause. The cause itself stays in the log.
    #[tokio::test]
    async fn an_internal_error_carries_an_error_id_the_log_line_carries() {
        let (status, body, log) = render_logged(OutputError::Internal(anyhow::anyhow!(
            "relation \"output_templates\" does not exist"
        )))
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["code"], "INTERNAL", "{body}");
        let id = body["error"]["error_id"]
            .as_str()
            .expect("error_id in the body");
        let parsed = uuid::Uuid::parse_str(id).expect("error_id is a UUID");
        assert_eq!(parsed.get_version_num(), 4, "{id}");
        assert!(log.contains(id), "the log line must carry {id}: {log}");
        assert!(
            log.contains("output_templates"),
            "the log keeps the error: {log}"
        );
        assert!(!body.to_string().contains("output_templates"), "{body}");
    }

    /// Each 500 has its own id; a client error has none, because no log line
    /// stands behind it.
    #[tokio::test]
    async fn only_an_internal_error_has_an_error_id_and_each_its_own() {
        let (_, a, _) = render_logged(OutputError::Internal(anyhow::anyhow!("x"))).await;
        let (_, b, _) = render_logged(OutputError::Internal(anyhow::anyhow!("x"))).await;
        assert_ne!(a["error"]["error_id"], b["error"]["error_id"]);
        let (_, refused, _) = render_logged(OutputError::not_found(
            "TEMPLATE_NOT_FOUND",
            "no such template",
        ))
        .await;
        assert!(refused["error"].get("error_id").is_none(), "{refused}");
    }

    /// An internal error's detail is logged, never returned.
    #[test]
    fn internal_detail_is_not_leaked() {
        let err = OutputError::Internal(anyhow::anyhow!("postgres://user:secret@host/db"));
        assert!(!err.body().to_string().contains("secret"));
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
