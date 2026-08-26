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

    /// The store **refused** the write, and the same write will always be
    /// refused.
    ///
    /// Distinct from [`Self::Database`], and the distinction decides what an
    /// ingest door answers. A storage failure is transient — a lost connection,
    /// a lock the statement declined to queue for — so the door answers `5xx`
    /// and the fan-out redelivers until it works. A refusal is about the
    /// *delivery*: overlapping spans within one version, two network operators
    /// for one reading, a non-canonical OBIS code, a value restated under an
    /// existing version. Redelivering that is a loop that never terminates, and
    /// the message has to change before it can be stored at all.
    ///
    /// `meterstore::Error::is_retryable` is what sorts the two; `constraint`
    /// carries the rule's own name where the store reports one, so an operator
    /// is told which rule refused rather than being handed a message to parse.
    #[error("storage refused the write: {detail}{}", .constraint.as_deref().map(|c| format!(" ({c})")).unwrap_or_default())]
    Rejected {
        detail: String,
        constraint: Option<String>,
    },

    /// Generic internal error (e.g. serialization failure).
    #[error("internal error: {0}")]
    Internal(String),
}

impl EdmError {
    /// Whether retrying the identical operation could succeed.
    ///
    /// The question an ingest door has to answer before choosing a status code:
    /// `5xx` asks the fan-out to redeliver, and asking it to redeliver
    /// something that can never be stored is a poison message that retries for
    /// as long as the retention window allows.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::Database(_) => true,
            Self::Rejected { .. }
            | Self::MaloNotFound { .. }
            | Self::InvalidMaloId { .. }
            | Self::NoData { .. }
            | Self::Internal(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EdmError;

    /// The split exists to decide one thing: whether an ingest door asks the
    /// fan-out to redeliver. Everything that describes the *delivery* must not
    /// be retried — the same bytes are refused every time, and the retry runs
    /// until the budget is gone while a 5xx sits on someone's dashboard.
    #[test]
    fn only_transient_failures_are_retryable() {
        assert!(EdmError::Database("connection reset".to_owned()).is_retryable());

        for permanent in [
            EdmError::Rejected {
                detail: "meter_reads refused a write: overlapping span".to_owned(),
                constraint: Some("meter_reads_pkey".to_owned()),
            },
            EdmError::InvalidMaloId {
                malo_id: "nope".to_owned(),
                reason: "not 11 digits".to_owned(),
            },
            EdmError::MaloNotFound {
                malo_id: "51238696012".to_owned(),
            },
            EdmError::Internal("schema mismatch".to_owned()),
        ] {
            assert!(
                !permanent.is_retryable(),
                "{permanent} must not be redelivered"
            );
        }
    }

    /// The constraint's own name reaches the message, so an operator is told
    /// which rule refused rather than being handed a string to parse.
    #[test]
    fn a_rejection_names_the_rule_where_the_store_reported_one() {
        let named = EdmError::Rejected {
            detail: "meter_reads refused a write: value restated".to_owned(),
            constraint: Some("one_version_scope".to_owned()),
        };
        assert!(named.to_string().contains("one_version_scope"), "{named}");

        let unnamed = EdmError::Rejected {
            detail: "cold tier refused a write".to_owned(),
            constraint: None,
        };
        assert!(!unnamed.to_string().contains('('), "{unnamed}");
    }
}
