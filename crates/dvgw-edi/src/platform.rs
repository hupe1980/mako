//! [`DvgwPlatform`] — the parse and validate entry points.

use edifact_rs::{MessageWindowsIter, OwnedSegment, ReaderConfig};

use crate::{error::Error, message::DvgwMessage, report::DvgwReport, validate};

/// Parse configuration and entry points for DVGW EDIFACT.
///
/// `DvgwPlatform` is a handle rather than a set of free functions so several can
/// coexist with different limits — a test harness and a production gateway in
/// one process, say.
#[derive(Debug, Clone)]
pub struct DvgwPlatform {
    config: ReaderConfig,
}

impl Default for DvgwPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DvgwPlatform {
    /// A platform with the `edifact-rs` default limits, including the 64 KiB
    /// per-segment ceiling that guards the parser against oversized input.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ReaderConfig::default(),
        }
    }

    /// A platform with explicit reader limits.
    #[must_use]
    pub fn with_config(config: ReaderConfig) -> Self {
        Self { config }
    }

    /// Parse the **first** message of an interchange.
    ///
    /// Use [`parse_interchange`](Self::parse_interchange) when the input may hold
    /// more than one `UNH`…`UNT` window: handing every segment of a multi-message
    /// interchange to one constructor merges unrelated positions into a single
    /// message.
    ///
    /// # Errors
    ///
    /// - [`Error::Parse`] — the input is not valid EDIFACT.
    /// - [`Error::MissingSegment`] — no `UNH` or no `BGM`.
    /// - [`Error::UnknownDocumentCode`] — `BGM` DE 1001 is not a DVGW code.
    /// - [`Error::CarrierMismatch`] — `UNH` DE 0065 contradicts that code.
    pub fn parse(&self, input: &[u8]) -> Result<DvgwMessage, Error> {
        #[cfg(feature = "tracing")]
        let _span = tracing::debug_span!("dvgw_parse", input_len = input.len()).entered();

        self.parse_interchange(input)
            .next()
            .unwrap_or(Err(Error::MissingSegment("UNH")))
    }

    /// Parse every message of an interchange, one item per `UNH`…`UNT` window.
    ///
    /// Envelope segments (`UNB`/`UNZ`, `UNG`/`UNE`) are stripped: a DVGW
    /// interchange carries one message per envelope by convention, but nothing
    /// on the wire enforces it and a merged parse is silent corruption.
    ///
    /// # Errors
    ///
    /// The leading `Err` reports a tokeniser failure; per-message failures are
    /// yielded in place so one bad message does not hide the rest.
    pub fn parse_interchange(
        &self,
        input: &[u8],
    ) -> impl Iterator<Item = Result<DvgwMessage, Error>> + use<> {
        let tokenized: Result<Vec<OwnedSegment>, Error> =
            edifact_rs::from_bytes_owned_with_config(input, self.config)
                .collect::<Result<_, _>>()
                .map_err(Error::Parse);

        let (segments, fatal) = match tokenized {
            Ok(segments) => (Some(segments), None),
            Err(e) => (None, Some(e)),
        };

        fatal.map(Err).into_iter().chain(
            segments
                .into_iter()
                .flat_map(|segments| {
                    MessageWindowsIter::new(
                        segments.into_iter().map(Ok::<_, edifact_rs::EdifactError>),
                    )
                })
                .map(|window| {
                    let window = window.map_err(Error::Parse)?;
                    DvgwMessage::from_segments(window.segments)
                }),
        )
    }

    /// Parse the first message and check it against the Nachrichtenbeschreibung.
    ///
    /// Conformance findings land in the returned [`DvgwReport`]; only failures
    /// that prevent the message from being identified at all are `Err`.
    ///
    /// # Errors
    ///
    /// As [`parse`](Self::parse).
    pub fn validate(&self, input: &[u8]) -> Result<DvgwReport, Error> {
        #[cfg(feature = "tracing")]
        let _span = tracing::debug_span!("dvgw_validate", input_len = input.len()).entered();

        let message = self.parse(input)?;
        Ok(Self::validate_message(&message))
    }

    /// Check an already-parsed message.
    #[must_use]
    pub fn validate_message(message: &DvgwMessage) -> DvgwReport {
        DvgwReport::new(
            message.message_type,
            message.document,
            message.message_ref.clone(),
            validate::check(message),
        )
    }
}

/// Does this interchange carry a DVGW message?
///
/// Reads `BGM` C002 DE 1001 and nothing else, so it is cheap enough to run at an
/// ingest boundary before deciding which parser owns the bytes.
///
/// That decision cannot be made from `UNH`: a DVGW message rides `ORDERS` or
/// `ORDRSP`, both of which are *also* real BDEW EDI@Energy message types, so an
/// ALOCAT handed to a BDEW parser comes back as a perfectly well-formed
/// `ORDRSP` with a Prüfidentifikator that means something else entirely. The
/// document-name code is the only field that separates the two families.
///
/// Returns `None` for a BDEW interchange, for anything that is not EDIFACT, and
/// for a DVGW-shaped message whose document code this crate does not know.
#[must_use]
pub fn sniff(input: &[u8]) -> Option<crate::document::DvgwDocument> {
    let segments: Vec<OwnedSegment> =
        edifact_rs::from_bytes_owned_with_config(input, ReaderConfig::default())
            .take_while(Result::is_ok)
            .map_while(Result::ok)
            // `BGM` is the second segment of a message, so there is no reason to
            // tokenise a whole interchange to find it.
            .take(8)
            .collect();
    segments
        .iter()
        .find(|s| s.tag == "BGM")
        .and_then(|bgm| bgm.component_str(0, 0))
        .and_then(crate::document::DvgwDocument::from_code)
}
