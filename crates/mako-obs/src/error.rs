use thiserror::Error;

/// Error type for [`crate::repository::ProcessProjectionRepository`] operations.
#[derive(Debug, Error)]
pub enum ObsError {
    #[error("database error: {0}")]
    Database(String),

    #[error("process not found: {process_id}")]
    NotFound { process_id: uuid::Uuid },

    #[error("no data for pid={pid} in period {from}..{to}")]
    NoKpiData { pid: u32, from: String, to: String },

    #[error("internal error: {0}")]
    Internal(String),
}
