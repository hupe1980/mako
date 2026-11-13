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

    /// A directory record was refused because nothing proves it is the
    /// provider's own (see [`RecordRefusal`]).
    #[error("directory record refused: {0}")]
    UnverifiedRecord(RecordRefusal),

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

/// Why a directory record was refused.
///
/// Regelungen zum Verzeichnisdienst 1.1, Kap. 2.2 (S. 6): „Die aktuelle Signatur
/// eines Eintrags MUSS jederzeit anhand eines gültigen Signaturzertifikats der
/// Klasse EMT.API des entsprechenden Marktpartners überprüft werden können" — the
/// record is the provider's statement, so a record no trusted key of that
/// provider signed says nothing about where its API lives.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RecordRefusal {
    /// The caller supplied no key for the provider, so no signature could be
    /// checked.
    #[error("no trusted signing key for provider {provider_id}")]
    NoTrustedKey {
        /// The provider whose record was requested.
        provider_id: String,
    },
    /// The response carried no `X-BDEW-SIGNATURE`, or more than one.
    #[error("the record carries no single X-BDEW-SIGNATURE")]
    MissingSignature,
    /// The signature verifies against none of the trusted keys.
    #[error("the record signature verifies against no trusted key of provider {provider_id}")]
    BadSignature {
        /// The provider whose record was requested.
        provider_id: String,
    },
    /// The record names another entry than the one requested.
    #[error("requested {requested}, the directory answered {received}")]
    WrongEntry {
        /// `providerId/apiId/majorVersion` of the request.
        requested: String,
        /// `providerId/apiId/majorVersion` of the record received.
        received: String,
    },
    /// The directory redirected more often than the client follows.
    #[error("more than {max} redirects, last to {url}")]
    TooManyRedirects {
        /// The redirect limit.
        max: usize,
        /// The last redirect target.
        url: String,
    },
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
