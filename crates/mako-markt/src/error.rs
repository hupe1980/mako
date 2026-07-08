#![allow(clippy::doc_markdown)]
//! Domain error type for `mako-markt`.
//!
//! No axum dependency here — `services/marktd` maps `MdmError` to HTTP responses
//! via its own `IntoResponse` impl.

/// All domain-level errors produced by `mako-markt`.
#[derive(Debug, thiserror::Error)]
pub enum MdmError {
    /// A supplied MaLo-ID is syntactically or checksum-invalid.
    #[error("invalid MaLo-ID '{id}': {reason}")]
    InvalidMaloId { id: String, reason: String },

    /// A supplied MeLo-ID is syntactically invalid.
    #[error("invalid MeLo-ID '{id}': {reason}")]
    InvalidMeloId { id: String, reason: String },

    /// A supplied GLN / Codenummer is syntactically invalid.
    #[error("invalid GLN '{mp_id}': {reason}")]
    InvalidMpId { mp_id: String, reason: String },

    /// The requested resource does not exist.
    #[error("not found: {resource_type} '{id}'")]
    NotFound {
        resource_type: &'static str,
        id: String,
    },

    /// ETag / `If-Match` header mismatch — optimistic concurrency conflict.
    #[error("version conflict: expected etag '{expected}', found '{actual}'")]
    VersionConflict { expected: String, actual: String },

    /// The caller is not authorised to perform this operation.
    #[error("forbidden: {reason}")]
    Forbidden { reason: &'static str },

    /// The request body is semantically invalid (business-rule violation).
    #[error("unprocessable: {reason}")]
    Unprocessable { reason: String },

    /// A downstream call to the `makod` admin API failed.
    #[error("makod sync failed: {0}")]
    MakodSync(String),

    /// A downstream call to an ERP webhook failed.
    #[error("webhook delivery failed (subscriber={subscriber_id}): {reason}")]
    WebhookDelivery {
        subscriber_id: String,
        reason: String,
    },

    /// Unexpected internal storage or I/O error.
    #[error("internal: {0}")]
    Internal(String),
}

impl MdmError {
    /// HTTP status code as a raw `u16`.
    ///
    /// `services/marktd` converts this to `axum::http::StatusCode`.
    #[must_use]
    pub fn status_u16(&self) -> u16 {
        match self {
            Self::InvalidMaloId { .. }
            | Self::InvalidMeloId { .. }
            | Self::InvalidMpId { .. }
            | Self::Unprocessable { .. } => 422,
            Self::NotFound { .. } => 404,
            Self::VersionConflict { .. } => 412,
            Self::Forbidden { .. } => 403,
            Self::MakodSync(_) | Self::WebhookDelivery { .. } | Self::Internal(_) => 500,
        }
    }

    /// Stable machine-readable error code.
    #[must_use]
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidMaloId { .. } => "invalid_malo_id",
            Self::InvalidMeloId { .. } => "invalid_melo_id",
            Self::InvalidMpId { .. } => "invalid_gln",
            Self::NotFound { .. } => "not_found",
            Self::VersionConflict { .. } => "version_conflict",
            Self::Forbidden { .. } => "forbidden",
            Self::Unprocessable { .. } => "unprocessable",
            Self::MakodSync(_) => "makod_sync_failed",
            Self::WebhookDelivery { .. } => "webhook_delivery_failed",
            Self::Internal(_) => "internal_error",
        }
    }

    /// Human-readable problem title for RFC 7807 `"title"` field.
    #[must_use]
    pub fn error_title(&self) -> &'static str {
        match self {
            Self::InvalidMaloId { .. } => "Invalid MaLo-ID",
            Self::InvalidMeloId { .. } => "Invalid MeLo-ID",
            Self::InvalidMpId { .. } => "Invalid GLN",
            Self::NotFound { .. } => "Not Found",
            Self::VersionConflict { .. } => "Version Conflict",
            Self::Forbidden { .. } => "Forbidden",
            Self::Unprocessable { .. } => "Unprocessable Content",
            Self::MakodSync(_) | Self::WebhookDelivery { .. } | Self::Internal(_) => {
                "Internal Server Error"
            }
        }
    }
}
