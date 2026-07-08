//! PostgreSQL repository implementations for `processd`.

pub mod anmeldung;
pub mod approval;

pub use anmeldung::PgAnmeldungRepository;
pub use approval::PgApprovalQueue;
