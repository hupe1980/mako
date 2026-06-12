/// Cheap envelope-only view of a parsed EDIFACT message (F-004).
///
/// `LightMessage` holds the raw `Vec<OwnedSegment>` plus the small set of
/// fields that any AS4 router or forwarder needs:
///
/// - Message type code (e.g. `"UTILMD"`)
/// - Association assigned code / release (e.g. `"S2.1"`)
/// - UNH message reference
/// - Prüfidentifikator (when detectable from UNH/BGM/RFF without full parse)
///
/// Typed field extraction (the `Vec<Dtm>`, `Vec<Nad>`, etc. on concrete message
/// structs) is **not** performed.  For a MSCONS message with 1 000 delivery-point
/// groups, this avoids O(n) heap allocations on the routing hot path.
///
/// # Routing pattern
///
/// ```rust,no_run
/// use edi_energy::{parse_envelope_only, EdiEnergyMessage};
///
/// let bytes = b"UNB+...";
/// let light = parse_envelope_only(bytes)?;
/// println!("type  : {}", light.message_type_code());
/// println!("release: {}", light.assoc_code());
///
/// // Only pay the full-parse cost when you actually need typed access:
/// if light.message_type_code() == "UTILMD" {
///     let msg = light.into_message()?;
///     let report = msg.validate()?;
/// }
/// # Ok::<(), edi_energy::Error>(())
/// ```
use edifact_rs::OwnedSegment;

use crate::{AnyMessage, Error, MessageType, Pruefidentifikator, Release};

/// Envelope-only view of a parsed EDIFACT/EDI@Energy message.
///
/// See the module-level docs for a full description and the routing
/// pattern.
#[derive(Debug)]
pub struct LightMessage {
    /// All parsed segments (owned).  Used by [`into_message`](Self::into_message)
    /// to avoid re-parsing when the caller decides to upgrade to a full message.
    pub(crate) segments: Vec<OwnedSegment>,
    /// UNH DE 0065 — EDIFACT message type code (e.g. `"UTILMD"`, `"MSCONS"`).
    message_type_code: Box<str>,
    /// UNH S009 DE 0057 — association assigned code (e.g. `"S2.1"`, `"2.4c"`).
    assoc_code: Box<str>,
    /// UNH DE 0062 — message reference identifier.
    message_ref: Box<str>,
    /// BGM DE 1004 or RFF+Z13 — Prüfidentifikator, if detectable.
    pruefidentifikator: Option<u32>,
}

impl LightMessage {
    /// Construct from a parsed segment list and a registry reference.
    ///
    /// Extracts the UNH fields and PID without building any typed message struct.
    pub(crate) fn from_segments(
        segments: Vec<OwnedSegment>,
        registry: &crate::registry::ReleaseRegistry,
    ) -> Result<Self, Error> {
        let (message_ref, message_type_code, assoc_code) = {
            let unh = segments
                .iter()
                .find(|s| s.tag == "UNH")
                .ok_or(Error::MissingSegment("UNH"))?;
            let message_ref = unh.element_str(0).unwrap_or_default().to_owned();
            let message_type_code = unh
                .component_str(1, 0)
                .ok_or(Error::MalformedSegment("UNH"))?
                .to_owned();
            let assoc_code = unh.component_str(1, 4).unwrap_or_default().to_owned();
            (message_ref, message_type_code, assoc_code)
        };

        // Look up PID source from registry (same logic as full parse) so we can
        // surface the PID without constructing any typed struct.
        let pruefidentifikator: Option<u32> =
            match crate::parse::resolve_pid_source_pub(&message_type_code, &assoc_code, registry) {
                crate::registry::PidSource::RffZ13 => segments
                    .iter()
                    .find(|s| s.tag == "RFF" && s.element_str(0).is_some_and(|q| q == "Z13"))
                    .and_then(|rff| rff.component_str(0, 1))
                    .and_then(|s| s.parse().ok()),
                crate::registry::PidSource::BgmDe1004 => segments
                    .iter()
                    .find(|s| s.tag == "BGM")
                    .and_then(|bgm| bgm.element_str(1))
                    .and_then(|s| s.parse().ok()),
            };

        Ok(Self {
            segments,
            message_type_code: message_type_code.into_boxed_str(),
            assoc_code: assoc_code.into_boxed_str(),
            message_ref: message_ref.into_boxed_str(),
            pruefidentifikator,
        })
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Raw EDIFACT message type code from UNH S009 DE 0065 (e.g. `"UTILMD"`).
    #[must_use]
    pub fn message_type_code(&self) -> &str {
        &self.message_type_code
    }

    /// Parsed [`MessageType`] discriminant, or `None` for unknown types.
    #[must_use]
    pub fn try_message_type(&self) -> Option<MessageType> {
        MessageType::from_unh_code(&self.message_type_code)
    }

    /// Association assigned code from UNH S009 DE 0057 (e.g. `"S2.1"`, `"2.4c"`).
    #[must_use]
    pub fn assoc_code(&self) -> &str {
        &self.assoc_code
    }

    /// Parsed [`Release`] derived from [`assoc_code`](Self::assoc_code).
    #[must_use]
    pub fn release(&self) -> Release {
        Release::new(&self.assoc_code)
    }

    /// UNH message reference (DE 0062).
    #[must_use]
    pub fn message_ref(&self) -> &str {
        &self.message_ref
    }

    /// Prüfidentifikator extracted from BGM or RFF+Z13, if present.
    #[must_use]
    pub fn pruefidentifikator(&self) -> Option<Pruefidentifikator> {
        self.pruefidentifikator
            .and_then(|n| Pruefidentifikator::new(n).ok())
    }

    /// Raw segment slice.  Available for advanced callers that need to inspect
    /// specific segments without upgrading to a full [`AnyMessage`].
    #[must_use]
    pub fn segments(&self) -> &[OwnedSegment] {
        &self.segments
    }

    // ── Upgrade ───────────────────────────────────────────────────────────────

    /// Upgrade to a fully typed [`AnyMessage`], performing typed field extraction.
    ///
    /// The owned segment buffer is moved, so no re-allocation is required.  The
    /// additional cost is the typed field extraction pass (O(segments) work),
    /// which is what routing-only paths avoid by holding a `LightMessage`.
    ///
    /// # Errors
    ///
    /// Returns `Err` when the message type is compiled out (`FeatureNotEnabled`)
    /// or the UNH segment is malformed.
    pub fn into_message(self) -> Result<AnyMessage, Error> {
        crate::parse::dispatch_message(self.segments, crate::registry::ReleaseRegistry::global())
    }
}
