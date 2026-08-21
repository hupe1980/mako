//! Error types for `mako-pruefung`.

use thiserror::Error;

/// Errors that can be returned by `mako-pruefung` — currently only input
/// validation errors. A tree walk itself never fails: a Prüfschritt the caller's
/// records cannot answer produces an escalation, not an `Err`.
#[derive(Debug, Error)]
pub enum CheckError {
    /// The `AnmeldungAnfrage.pid` is not a recognised Lieferbeginn PID.
    ///
    /// Only PIDs 55001, 55016 (Strom) and 44001 (Gas) are valid Lieferbeginn
    /// initiation messages.  All other PIDs should be handled by their own
    /// pipeline and must not be passed to `evaluate`.
    #[error("PID {0} is not a Lieferbeginn PID (valid: 55001, 55016, 44001)")]
    UnrecognisedPid(u32),
}
