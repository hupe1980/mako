//! Unified error type for the `energy-api` crate.

use thiserror::Error;

/// All errors that can be returned by the energy-api crate.
#[derive(Debug, Error)]
pub enum Error {
    /// The remote service responded with a non-success HTTP status.
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },

    /// The server returned a 307 Temporary Redirect — the caller should
    /// follow the given URL and retry the request there.
    #[error("Redirect to: {url}")]
    Redirect { url: String },

    /// A requested directory record does not exist.
    #[error("record not found")]
    NotFound,

    /// JSON serialization or deserialization failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// An invalid URL was supplied or returned by the server.
    #[error("URL error: {0}")]
    Url(#[from] url::ParseError),

    /// JWS signature creation or verification failed.
    #[error("signature error: {0}")]
    Signature(String),

    /// Transport-level failure (TLS, DNS, connection reset, …).
    #[error("transport error: {0}")]
    Transport(String),

    /// The remote endpoint violated the API protocol.
    #[error("protocol error: {0}")]
    Protocol(String),
}

#[cfg(feature = "client")]
impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        if let Some(status) = e.status() {
            Error::Http {
                status: status.as_u16(),
                body: e.to_string(),
            }
        } else {
            Error::Transport(e.to_string())
        }
    }
}

#[cfg(feature = "websocket")]
impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::Transport(e.to_string())
    }
}
