/// Errors produced by `dvgw-edi`.
///
/// All public API entry points return `Result<_, Error>`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The underlying EDIFACT tokeniser rejected the input.
    #[error("EDIFACT parse error: {0}")]
    Parse(#[from] edifact_rs::EdifactError),

    /// The message type code from UNH is recognised as a DVGW type but the
    /// corresponding Cargo feature was not compiled in.
    ///
    /// Enable the `feature` Cargo feature to parse `message_type` messages.
    #[error("DVGW message type {message_type:?} requires the disabled `{feature}` Cargo feature")]
    FeatureNotEnabled {
        /// The EDIFACT message type code, e.g. `"ALOCAT"`.
        message_type: String,
        /// The Cargo feature name that must be enabled, e.g. `"alocat"`.
        feature: String,
    },

    /// The UNH message type code is not a recognised DVGW format.
    ///
    /// The raw code is not included in `Display` output to prevent
    /// log-injection.  Access via the `raw_code` field for diagnostics.
    #[error("unknown DVGW message type (check UNH DE 0065 element 1 component 0)")]
    UnknownMessageType {
        /// The sanitized UNH type code.
        raw_code: String,
    },

    /// No profile is registered for the requested `(MessageType, version)` pair.
    ///
    /// This is returned when a message is received for a DVGW version that is
    /// not compiled into this binary.
    #[error("no DVGW profile registered for message type {message_type:?} version {version:?}")]
    ProfileNotFound {
        /// The DVGW message type.
        message_type: String,
        /// The version string extracted from the UNH association code (DE 0057).
        version: String,
    },

    /// A mandatory EDIFACT segment is absent.
    #[error("required segment {0} is missing")]
    MissingSegment(&'static str),

    /// A segment was found but its content is structurally invalid.
    #[error("malformed segment {0}")]
    MalformedSegment(&'static str),
}

// ── Sanitization helper ───────────────────────────────────────────────────────

/// Sanitize an untrusted EDIFACT type-code string for safe inclusion in error
/// fields and log output.
///
/// Valid DVGW type codes are ≤ 16 ASCII alphanumeric characters plus `.`.
/// Characters outside that set are replaced with `?`.
pub(crate) fn sanitize_code(s: &str) -> String {
    const MAX_LEN: usize = 16;
    let truncated = if s.len() > MAX_LEN { &s[..MAX_LEN] } else { s };
    truncated
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '?'
            }
        })
        .collect()
}
