//! Unified error type for the `energy-api` crate.

use thiserror::Error;

/// All errors that can be returned by the energy-api crate.
#[derive(Debug, Error)]
pub enum Error {
    /// The remote service responded with a non-success HTTP status.
    #[error("HTTP {status}: {body}")]
    Http {
        /// HTTP status code (e.g. `400`, `503`).
        status: u16,
        /// Response body text.
        body: String,
    },

    /// The server returned a 307 Temporary Redirect — the caller should
    /// follow the given URL and retry the request there.
    #[error("Redirect to: {url}")]
    Redirect {
        /// Redirect target URL.
        url: String,
    },

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

    /// A BDEW identifier failed validation (length, alphabet or check digit).
    ///
    /// Surfaced as HTTP 400: the caller supplied a malformed MaLo-, MeLo-,
    /// NeLo-, SR- or TR-ID, and accepting it would let a bad identifier enter
    /// the identification path.
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
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
