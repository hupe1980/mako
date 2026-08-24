//! Error type and the log-injection guard shared by every diagnostic path.

/// Errors produced by `dvgw-edi`.
///
/// All public API entry points return `Result<_, Error>`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The underlying EDIFACT tokeniser rejected the input.
    #[error("EDIFACT parse error: {0}")]
    Parse(#[from] edifact_rs::EdifactError),

    /// Rendering an outbound message back to EDIFACT bytes failed.
    #[error("EDIFACT serialization error: {0}")]
    Serialize(String),

    /// A segment the DVGW Nachrichtenbeschreibung marks `Muss` is absent, so the
    /// message cannot even be identified.
    #[error("required segment {0} is missing")]
    MissingSegment(&'static str),

    /// `BGM` C002 DE 1001 does not carry a document-name code this crate knows.
    ///
    /// DVGW identifies the logical message (ALOCAT / NOMINT / NOMRES) by this
    /// code — **not** by the `UNH` message type, which is always the UN/EDIFACT
    /// carrier `ORDERS` or `ORDRSP`.
    ///
    /// The raw code is kept out of `Display` so an untrusted value cannot reach
    /// operator logs; read it from `raw_code` when diagnosing.
    #[error("unknown DVGW document-name code (check BGM C002 DE 1001)")]
    UnknownDocumentCode {
        /// The sanitized `BGM` DE 1001 value.
        raw_code: String,
    },

    /// `UNH` S009 DE 0065 names a carrier that does not match the document code.
    ///
    /// NOMINT rides `ORDERS`; ALOCAT and NOMRES ride `ORDRSP`. A mismatch means
    /// the two identifying fields disagree, which no conformant sender produces.
    #[error(
        "UNH carrier and BGM document code disagree: document {document} is carried by \
         {expected} but UNH names a different message type"
    )]
    CarrierMismatch {
        /// The `BGM` DE 1001 code that was read.
        document: &'static str,
        /// The carrier that code requires.
        expected: &'static str,
        /// The sanitized `UNH` DE 0065 value that was found instead.
        raw_code: String,
    },

    /// A wrapped I/O error from the reader-based entry points.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// ── Sanitization helper ───────────────────────────────────────────────────────

/// Sanitize an untrusted EDIFACT code for safe inclusion in error fields and logs.
///
/// DVGW codes are at most a handful of ASCII alphanumerics plus `.`; anything
/// else is replaced with `?` so ANSI escapes and other log-injection payloads
/// are neutralised.
///
/// Truncation is done on **character** boundaries, not byte offsets: the value
/// comes straight off the wire and slicing a multi-byte character in half would
/// panic on the parsing hot path.
pub(crate) fn sanitize_code(s: &str) -> String {
    const MAX_CHARS: usize = 16;
    s.chars()
        .take(MAX_CHARS)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '?'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::sanitize_code;

    #[test]
    fn truncates_on_character_boundaries() {
        // 17 two-byte characters: a byte-offset slice at 16 would split one.
        let hostile = "ü".repeat(17);
        assert_eq!(sanitize_code(&hostile), "?".repeat(16));
    }

    #[test]
    fn passes_plain_codes_through() {
        assert_eq!(sanitize_code("ORDRSP"), "ORDRSP");
        assert_eq!(sanitize_code("5.11a"), "5.11a");
        assert_eq!(sanitize_code("X1G\u{1b}[31m"), "X1G??31m");
    }
}
