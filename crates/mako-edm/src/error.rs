use thiserror::Error;

/// Error type for [`crate::repository::TimeSeriesRepository`] operations.
#[derive(Debug, Error)]
pub enum EdmError {
    /// Underlying database / storage error.
    #[error("database error: {0}")]
    Database(String),

    /// The requested MaLo does not exist in the master data registry.
    #[error("MaLo not found: {malo_id}")]
    MaloNotFound { malo_id: String },

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
