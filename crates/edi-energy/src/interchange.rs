/// Process-layer types for EDIFACT interchange envelope handling (F-032).
///
/// An EDIFACT *interchange* (UNB…UNZ envelope) wraps one or more messages.
/// Standard `parse_interchange()` discards the UNB metadata; the types here
/// preserve it so downstream code can build acknowledgement messages, route by
/// sender/receiver GLN, and cross-check control references.
use crate::{AnyMessage, EdiEnergyMessage, EdiEnergyReport, Release};

// ── InterchangeHeader ─────────────────────────────────────────────────────────

/// Parsed UNB interchange envelope header fields.
///
/// All fields come from the UNB segment of the EDIFACT interchange.
/// Use these for routing, acknowledgement generation, and audit logging.
#[derive(Debug, Clone)]
pub struct InterchangeHeader {
    /// Sender identification (UNB S002, DE 0004 — e.g. a 13-digit GLN).
    pub sender_id: Box<str>,
    /// Sender qualifier (UNB S002, DE 0007 — e.g. `"14"` for GS1 GLN).
    pub sender_qualifier: Box<str>,
    /// Recipient identification (UNB S003, DE 0010 — e.g. a 13-digit GLN).
    pub receiver_id: Box<str>,
    /// Recipient qualifier (UNB S003, DE 0007).
    pub receiver_qualifier: Box<str>,
    /// Preparation date+time from UNB S004 (DE 0017 + DE 0019).
    ///
    /// `None` when the UNB date/time fields are absent or malformed.
    pub transmission_datetime: Option<time::OffsetDateTime>,
    /// Interchange control reference (UNB DE 0020).
    pub control_ref: Box<str>,
    /// EDIFACT syntax identifier from UNB S001 (DE 0001 — e.g. `"UNOC"`).
    pub syntax_id: Box<str>,
    /// EDIFACT syntax version number from UNB S001 (DE 0002 — e.g. `3`).
    pub syntax_version: u8,
}

impl InterchangeHeader {
    /// Extract the transmission date component only.
    ///
    /// Returns `None` when [`transmission_datetime`][Self::transmission_datetime] is `None`.
    #[must_use]
    pub fn transmission_date(&self) -> Option<time::Date> {
        self.transmission_datetime.map(time::OffsetDateTime::date)
    }
}

// ── ReceiptContext ─────────────────────────────────────────────────────────────

/// Context needed to build an acknowledgement (APERAK / CONTRL) for a received
/// message.
///
/// Produced by [`MessageEnvelope::receipt_context`].  Pass this to
/// `AperakBuilder::for_receipt()` or `ContrlBuilder::for_interchange()` to
/// construct the outgoing acknowledgement with the correct mirror fields.
#[derive(Debug, Clone)]
pub struct ReceiptContext<'m> {
    /// GLN or ID of the original sender (becomes the recipient in the ACK).
    pub original_sender: &'m str,
    /// GLN or ID of the original receiver (becomes the sender in the ACK).
    pub original_receiver: &'m str,
    /// UNH message reference of the message being acknowledged.
    pub message_ref: &'m str,
    /// Wire release code of the message being acknowledged.
    pub release: Release,
    /// Transmission date of the original interchange, if available.
    pub transmission_date: Option<time::Date>,
}

// ── MessageEnvelope ───────────────────────────────────────────────────────────

/// A single parsed message together with its enclosing interchange envelope header.
///
/// When an interchange contains multiple messages, each yields a separate
/// `MessageEnvelope` from [`Interchange::messages`].
#[derive(Debug)]
pub struct MessageEnvelope {
    /// The parsed message.
    pub message: AnyMessage,
    /// The interchange header from the enclosing UNB segment.
    pub header: InterchangeHeader,
    /// 0-based index of this message within the interchange.
    pub message_index: usize,
}

impl MessageEnvelope {
    /// Validate the message using the profile registry.
    ///
    /// Delegates to [`EdiEnergyMessage::validate`] for all known message types.
    /// For [`AnyMessage::Unknown`] this returns `Ok(report)` where the report
    /// contains a single `Warning` with rule ID `"UNKNOWN-MSG-TYPE"`, consistent
    /// with the behaviour of `EdiEnergyMessage::validate_against` on unknown variants.
    ///
    /// **Rationale for returning `Ok` instead of `Err`:** an interchange may
    /// legitimately contain message types that are not compiled into the current
    /// binary (e.g. when only the `mscons` feature is enabled).  Returning `Err`
    /// would abort validation of the entire interchange on the first unknown message,
    /// preventing valid messages later in the interchange from being validated.
    /// Callers that want strict unknown-type rejection can check
    /// `report.is_valid() && report.warnings().is_empty()` after the fact.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::ProfileNotFound`] when no profile matches the message's
    /// release code (known message type, unregistered release).
    pub fn validate(&self) -> Result<EdiEnergyReport, crate::Error> {
        // Delegate to the EdiEnergyMessage trait impl for ALL variants, including
        // Unknown.  The Unknown impl returns Ok(report) with a warning, which is
        // exactly the consistent behaviour we want here (resolves F-015).
        EdiEnergyMessage::validate(&self.message)
    }

    /// Return `true` when the wire release code in this message is normatively
    /// acceptable on `date` (considering the grace window configured on `registry`).
    ///
    /// Pass the registry from the owning [`crate::Platform`] rather than calling
    /// this via the global singleton.  Using an explicit registry is required for
    /// test isolation and multi-tenant deployments (F-007 fix).
    ///
    /// For convenience in simple single-registry programs, call
    /// [`MessageEnvelope::is_wire_code_acceptable_on_global`] instead.
    #[must_use]
    pub fn is_wire_code_acceptable_on(
        &self,
        date: time::Date,
        registry: &crate::registry::ReleaseRegistry,
    ) -> bool {
        let Some(mt) = self.message.try_message_type() else {
            return false;
        };
        let Ok(release) = EdiEnergyMessage::detect_release(&self.message) else {
            return false;
        };
        registry.is_acceptable_on(mt, release, date)
    }

    /// Convenience wrapper that uses the process-global registry.
    ///
    /// Prefer [`MessageEnvelope::is_wire_code_acceptable_on`] with an explicit
    /// registry when working with a [`crate::Platform`] instance.
    #[must_use]
    pub fn is_wire_code_acceptable_on_global(&self, date: time::Date) -> bool {
        self.is_wire_code_acceptable_on(date, crate::registry::ReleaseRegistry::global())
    }

    /// Extract the GLN from a 13-digit numeric sender ID, or return `None`.
    #[must_use]
    pub fn sender_gln(&self) -> Option<&str> {
        extract_gln(&self.header.sender_id)
    }

    /// Extract the GLN from a 13-digit numeric receiver ID, or return `None`.
    #[must_use]
    pub fn receiver_gln(&self) -> Option<&str> {
        extract_gln(&self.header.receiver_id)
    }

    /// The transmission date from the interchange header, if present.
    #[must_use]
    pub fn transmission_date(&self) -> Option<time::Date> {
        self.header.transmission_date()
    }

    /// Build a [`ReceiptContext`] for constructing an acknowledgement.
    ///
    /// The context mirrors sender/receiver so that `AperakBuilder::for_receipt()`
    /// and `ContrlBuilder::for_interchange()` swap them correctly.
    #[must_use]
    pub fn receipt_context(&self) -> ReceiptContext<'_> {
        let release = EdiEnergyMessage::detect_release(&self.message)
            .cloned()
            .unwrap_or_else(|_| Release::new(""));
        ReceiptContext {
            original_sender: &self.header.sender_id,
            original_receiver: &self.header.receiver_id,
            message_ref: EdiEnergyMessage::message_ref(&self.message),
            release,
            transmission_date: self.header.transmission_date(),
        }
    }
}

// ── Interchange ───────────────────────────────────────────────────────────────

/// A fully parsed EDIFACT interchange with envelope metadata preserved.
///
/// Produced by [`crate::parse_interchange_full`].  Contains the interchange
/// header (UNB fields) and all contained messages with their shared header
/// attached to each envelope.
#[derive(Debug)]
pub struct ParsedInterchange {
    /// The UNB interchange header.
    pub header: InterchangeHeader,
    /// All contained messages in document order.
    pub messages: Vec<MessageEnvelope>,
    /// UNZ control reference (must match [`InterchangeHeader::control_ref`]).
    pub trailer_ref: Box<str>,
    /// Message count declared in UNZ (should equal `messages.len()`).
    pub declared_message_count: usize,
}

impl ParsedInterchange {
    /// Return the number of messages that were actually parsed.
    #[must_use]
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Check whether the declared message count in UNZ matches the actual count.
    #[must_use]
    pub fn count_matches_declared(&self) -> bool {
        self.messages.len() == self.declared_message_count
    }

    /// Check whether the UNZ control reference matches the UNB control reference.
    #[must_use]
    pub fn control_refs_match(&self) -> bool {
        self.trailer_ref == self.header.control_ref
    }

    /// Return `true` when both structural integrity checks pass.
    #[must_use]
    pub fn is_structurally_valid(&self) -> bool {
        self.count_matches_declared() && self.control_refs_match()
    }
}

// ── GLN extraction helper ─────────────────────────────────────────────────────

fn extract_gln(id: &str) -> Option<&str> {
    if id.len() == 13 && id.bytes().all(|b| b.is_ascii_digit()) {
        Some(id)
    } else {
        None
    }
}
