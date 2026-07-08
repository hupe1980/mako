//! Error types for `netz-checker`.

use thiserror::Error;

/// Errors that can be returned by `netz-checker` — currently only input
/// validation errors.  The `evaluate` function itself never fails;
/// structural problems with the input produce `NetzCheckResult::Escalate`.
#[derive(Debug, Error)]
pub enum NetzCheckerError {
    /// The `AnmeldungAnfrage.pid` is not a recognised Lieferbeginn PID.
    ///
    /// Only PIDs 55001, 55016 (Strom) and 44001 (Gas) are valid Lieferbeginn
    /// initiation messages.  All other PIDs should be handled by their own
    /// pipeline and must not be passed to `evaluate`.
    #[error("PID {0} is not a Lieferbeginn PID (valid: 55001, 55016, 44001)")]
    UnrecognisedPid(u32),
}
