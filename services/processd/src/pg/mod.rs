//! PostgreSQL repository implementations for `processd`.

pub mod anmeldung;
pub mod approval;
// `E_0608` is an NB tree; without the NB role there is no Prüflauf to remember.
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
pub mod neuanlage;

pub use anmeldung::PgAnmeldungRepository;
pub use approval::PgApprovalQueue;
