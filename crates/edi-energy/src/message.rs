use edifact_rs::OwnedSegment;

use crate::{CustomRulePack, EdiEnergyReport, Error, MessageType, Pruefidentifikator, Release};

/// Core abstraction for all EDI@Energy message types.
///
/// Every concrete message type (`UtilmdMessage`, `MsconsMessage`, …) implements
/// this trait, and [`AnyMessage`](crate::AnyMessage) delegates to it via dynamic dispatch or
/// exhaustive matching.
///
/// The trait is object-safe: all methods return owned values or `Result<Owned, Error>`.
///
/// ## Dual-representation design
///
/// Each parsed message holds **two** views of the same data:
///
/// 1. **Raw segments** (`Vec<OwnedSegment>`) — the authoritative wire representation.
///    This is what [`serialize`](EdiEnergyMessage::serialize) serialises and what
///    [`validate`](EdiEnergyMessage::validate) runs against.
///
/// 2. **Typed fields** (e.g. `bgm`, `nad`, `dtm` on concrete structs) — pre-extracted
///    convenience views populated at parse time.  These are read-only helpers for
///    field access patterns like routing and logging.
///
/// **Mutations to typed fields are silently discarded on `serialize`.** If you need
/// to modify a message before re-sending it, use the builder API in
/// [`crate::builders`] to construct a new message from scratch, or manipulate the
/// raw segment bytes directly.
pub trait EdiEnergyMessage: Send + Sync {
    /// Returns the message-type discriminant, or `None` for unrecognised
    /// message types (i.e. [`AnyMessage::Unknown`](crate::AnyMessage)).
    ///
    /// This is the primary required method for message-type identification.
    /// Concrete message types (e.g. `UtilmdMessage`) always return `Some(…)`.
    #[must_use]
    fn try_message_type(&self) -> Option<MessageType>;

    /// Extracts the EDI@Energy release identifier from the UNH S009 composite (DE 0057).
    ///
    /// Returns `Err(Error::MissingRelease)` when the field is absent or empty.
    ///
    /// The returned reference borrows from the message; no allocation is performed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingRelease`] when the UNH S009 association code (DE 0057) is
    /// absent or empty.
    fn detect_release(&self) -> Result<&Release, Error>;

    /// Returns the UNH message reference identifier (DE 0062).
    ///
    /// This is the sender-assigned reference string that correlates UNH/UNT pairs
    /// and is mirrored in acknowledgement messages (APERAK, CONTRL).
    fn message_ref(&self) -> &str;

    /// Extracts the Pruefidentifikator from the BGM document-identifier field (DE 1004).
    ///
    /// Returns `Err(Error::MissingPruefidentifikator)` when the BGM segment is absent,
    /// or `Err(Error::InvalidPruefidentifikator)` when the value is outside 10000–99999.
    ///
    /// # Errors
    ///
    /// - [`Error::MissingPruefidentifikator`] — BGM segment absent or DE 1004 empty.
    /// - [`Error::InvalidPruefidentifikatorRange`] — value is outside the range 10000–99999.
    /// - [`Error::InvalidPruefidentifikatorFormat`] — value is not a valid integer.
    fn detect_pruefidentifikator(&self) -> Result<Pruefidentifikator, Error>;

    /// Validate the message using the profile registered for its detected release.
    ///
    /// Performs all applicable validation layers (1–5) for which profile data is available.
    /// Returns the full [`EdiEnergyReport`]; use [`EdiEnergyReport::is_valid`] to check
    /// pass/fail, or `.into_result()` to propagate errors.
    ///
    /// # Errors
    ///
    /// Returns `Err` only when validation itself cannot run (e.g. parse failure,
    /// profile not registered). Validation findings are carried in [`EdiEnergyReport`].
    #[must_use = "validation result must be checked for errors"]
    fn validate(&self) -> Result<EdiEnergyReport, Error>;

    /// Validate against an explicit release, overriding the detected one.
    ///
    /// Useful for strict conformance testing or when the release code is absent.
    ///
    /// # Unknown message types
    ///
    /// For [`AnyMessage::Unknown`](crate::AnyMessage) this returns `Ok(report)`
    /// carrying a single **error** with rule ID `"UNKNOWN-MSG-TYPE"`, so
    /// `report.is_valid()` is `false`. The `release` parameter is unused.
    ///
    /// It is an error rather than a warning because nothing was checked: a
    /// message type this build cannot validate has passed no rule at all, and
    /// reporting that as valid is the vacuous-validation trap — the caller
    /// cannot tell it from a message that was checked and conformed. `Ok` rather
    /// than `Err` so one unrecognised message does not abort validation of the
    /// rest of an interchange; read `is_valid()` for the verdict.
    ///
    /// # Errors
    ///
    /// Returns `Err(Error::ProfileNotFound)` when no profile is registered for
    /// the given `(message_type, release)` pair.
    #[must_use = "validation result must be checked for errors"]
    fn validate_against(&self, release: &Release) -> Result<EdiEnergyReport, Error>;

    /// Validate and merge an additional caller-supplied rule pack on top of all
    /// built-in validation layers (L1–L5).
    ///
    /// The `extra` pack runs after the standard semantic rules and can be used
    /// for application-level business rules, regulatory additions, or test-time
    /// strictness escalation — without needing to fork the message type.
    ///
    /// Use [`CustomRulePack`](crate::CustomRulePack) to construct the rule pack
    /// without a direct dependency on `edifact-rs`.
    ///
    /// # Errors
    ///
    /// Same as [`validate`](Self::validate).
    #[must_use = "validation result must be checked for errors"]
    fn validate_with_pack(&self, extra: CustomRulePack) -> Result<EdiEnergyReport, Error>;

    /// Validate the message for the normative date encoded in `ctx`.
    ///
    /// This is the primary entry point for AS4 adapter integration:
    ///
    /// - Checks that the message's declared release is acceptable on `ctx`'s
    ///   date — in force, or superseded but still inside the registry's
    ///   configured receive tolerance. Outside that window this returns
    ///   `Err(Error::ProfileNotFound)`.
    /// - Validates against the sender's declared release on `ctx`'s date.  This
    ///   preserves the sender's conformance claim: a message in the outgoing format
    ///   during the transition window is validated against the outgoing profile, not
    ///   the incoming one.
    ///
    /// # Date threading
    ///
    /// Both the `is_acceptable` check and the profile lookup use `ctx.date()` as
    /// the reference date — no call to `now_utc()` is made, so the method is
    /// deterministic for a caller that sets an explicit reference date. Reading
    /// the wall clock here would put an off-by-one risk at midnight.
    ///
    /// # Transition handling
    ///
    /// EDIFACT changes format at a single Anwendungszeitpunkt, so on any date
    /// exactly one release is in force. When the registry is configured with a
    /// non-zero receive tolerance, the superseded release stays acceptable for
    /// that many days afterwards and both validate against their own profile —
    /// callers do not need to dispatch on `TransitionState` themselves. See
    /// [`DEFAULT_RECEIVE_TOLERANCE_DAYS`](crate::DEFAULT_RECEIVE_TOLERANCE_DAYS).
    ///
    /// # Errors
    ///
    /// Returns `Err(Error::MissingRelease)` when the message has no release code.
    ///
    /// Returns `Err(Error::ProfileNotFound)` when the message's release is not
    /// acceptable on `ctx`'s date.
    ///
    /// Other errors mirror those of [`validate_against`](Self::validate_against).
    #[must_use = "validation result must be checked for errors"]
    fn validate_with_context(
        &self,
        ctx: &crate::registry::ProcessContext,
    ) -> Result<EdiEnergyReport, Error> {
        let release = self.detect_release()?;
        // Unknown message types have no typed MessageType and therefore cannot be
        // checked against a ProcessContext.  Fall through to validate_on_date,
        // which returns a warning report for Unknown variants.
        let Some(mt) = self.try_message_type() else {
            return self.validate_on_date(ctx.date());
        };
        if !ctx.is_acceptable(mt, release) {
            return Err(Error::ProfileNotFound {
                message_type: mt,
                release: release.clone(),
            });
        }
        // Use ctx.date() (not now_utc()) so profile lookup is deterministic for
        // date-sensitive tests and near-midnight race conditions are eliminated.
        self.validate_on_date(ctx.date())
    }

    /// Validate the message as of `reference_date` — the day it is judged on,
    /// which is what decides the applicable Formatversion.
    ///
    /// Equivalent to [`validate`](Self::validate) but uses `reference_date` for
    /// profile validity lookups instead of `time::OffsetDateTime::now_utc()`.
    /// This is the recommended way to write deterministic tests that exercise
    /// profile-version disambiguation without depending on the wall clock.
    ///
    /// # Example
    /// ```rust,ignore
    /// let date = time::Date::from_calendar_date(2026, time::Month::January, 15).unwrap();
    /// let report = message.validate_on_date(date)?;
    /// ```
    ///
    /// # Errors
    ///
    /// Same as [`validate`](Self::validate).
    #[must_use = "validation result must be checked for errors"]
    fn validate_on_date(&self, reference_date: time::Date) -> Result<EdiEnergyReport, Error>;

    /// Serialize the message back to EDIFACT wire bytes.
    ///
    /// The returned bytes are a valid EDIFACT document and can be re-parsed.
    ///
    /// **Serialization uses the raw segment list, not the typed fields.**
    /// Mutations to typed fields (e.g. `msg.bgm`, `msg.nad`) are not reflected
    /// in the output.  To modify a message before re-sending, use the builder
    /// API in [`crate::builders`] instead.
    ///
    /// # Errors
    ///
    /// Returns `Err(Error::Serialize(_))` when the underlying EDIFACT serializer
    /// cannot encode the segment data.  In practice this only occurs when segment
    /// content contains characters that are not valid in the EDIFACT character set
    /// (e.g. raw control bytes).  For messages produced by this crate's parsers
    /// (which have already validated input bytes) `serialize()` is effectively
    /// infallible; for messages constructed by mutating raw segments directly the
    /// caller should handle the error path.
    fn serialize(&self) -> Result<Vec<u8>, Error>;

    /// Returns the raw parsed segments (UNH … UNT inclusive).
    ///
    /// This slice is the authoritative source for serialization and validation.
    /// Typed fields on concrete message structs are derived views; mutations to
    /// those fields do **not** affect the segment list.
    fn segments(&self) -> &[OwnedSegment];
}
