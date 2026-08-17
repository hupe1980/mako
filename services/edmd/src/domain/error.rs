use thiserror::Error;

/// Error type for [`crate::domain::repository::TimeSeriesRepository`] operations.
#[derive(Debug, Error)]
pub enum EdmError {
    /// Underlying database / storage error.
    #[error("database error: {0}")]
    Database(String),

    /// The requested MaLo does not exist in the master data registry.
    #[error("MaLo not found: {malo_id}")]
    MaloNotFound { malo_id: String },

    /// The string offered as a MaLo-ID is not one.
    ///
    /// Distinct from [`Self::MaloNotFound`], and the distinction is the useful
    /// part: *not found* means a well-formed ID nobody has registered, while
    /// this means the value cannot be a MaLo at all — wrong length, a non-digit,
    /// or a check digit that does not match the BDEW Bildungsvorschrift. The
    /// first is a lookup miss a caller may legitimately retry after master data
    /// lands; the second is a bad request that will never succeed.
    #[error("not a MaLo-ID: {malo_id} ({reason})")]
    InvalidMaloId { malo_id: String, reason: String },

    /// No reads available for the requested MaLo and time range.
    #[error("no data for {malo_id} in period {from}..{to}")]
    NoData {
        malo_id: String,
        from: String,
        to: String,
    },

    /// Generic internal error (e.g. serialization failure).
    #[error("internal error: {0}")]
    Internal(String),
}
