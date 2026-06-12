/// Entry points for parsing EDIFACT/EDI@Energy messages.
///
/// # Quick start
///
/// ```no_run
/// use edi_energy::{parse, EdiEnergyMessage};
///
/// let input = std::fs::read("message.edi").unwrap();
/// let msg = parse(&input).unwrap();
/// if let Some(mt) = msg.try_message_type() { println!("type: {}", mt.as_str()); }
/// let report = msg.validate().unwrap();
/// println!("valid: {}", report.is_valid());
/// ```
use std::io::{BufReader, Read};

use edifact_rs::{MessageWindowsIter, OwnedSegment, ReaderConfig, from_bufread_stream_with_config};

use crate::{AnyMessage, Error, MessageType};

// ── Security helpers ─────────────────────────────────────────────────────────

/// Sanitize an untrusted release code before including it in any log output.
///
/// Valid BDEW release codes are ≤ 16 ASCII alphanumeric characters plus `.`.
/// Anything outside that set could contain log-injection sequences, ANSI escape
/// codes, or GDPR-sensitive data that must not appear in operator logs.
fn sanitize_release_code(s: &str) -> std::borrow::Cow<'_, str> {
    const MAX_LEN: usize = 16;
    if s.len() <= MAX_LEN && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
        std::borrow::Cow::Borrowed(s)
    } else {
        std::borrow::Cow::Owned(format!("<invalid-code:{} bytes>", s.len()))
    }
}

// ── PID source lookup ─────────────────────────────────────────────────────────

/// Determine where the Prüfidentifikator lives for the given message type and
/// release by consulting the given profile registry.
///
/// Falls back to [`crate::registry::PidSource::BgmDe1004`] when no profile is
/// registered (feature disabled, unknown release, etc.).  This is safe because
/// the `dispatch_message` path for unknown types ultimately produces
/// [`AnyMessage::Unknown`] regardless.
fn resolve_pid_source(
    msg_type_code: &str,
    assoc_code: &str,
    registry: &crate::registry::ReleaseRegistry,
) -> crate::registry::PidSource {
    MessageType::from_unh_code(msg_type_code)
        .and_then(|mt| {
            let rel = crate::release::Release::new(assoc_code);
            registry.profile(mt, &rel).ok()
        })
        .map(super::registry::Profile::pid_source)
        .unwrap_or_default()
}

/// Same as `resolve_pid_source` but accessible from `light_message.rs`.
pub(crate) fn resolve_pid_source_pub(
    msg_type_code: &str,
    assoc_code: &str,
    registry: &crate::registry::ReleaseRegistry,
) -> crate::registry::PidSource {
    resolve_pid_source(msg_type_code, assoc_code, registry)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Default per-segment byte limit for [`ParseConfig`].
///
/// 64 KiB matches the built-in limit of `edifact_rs::from_bytes` and guards
/// against maliciously crafted oversized segments.
pub const DEFAULT_MAX_SEGMENT_BYTES: usize = 64 * 1024;

/// Configuration for the EDIFACT byte-slice parser.
///
/// Use [`ParseConfig::default()`] for standard EDI@Energy messages.  The
/// default applies a 64 KiB per-segment limit to protect against `DoS` attacks.
/// To disable the limit entirely (trusted, size-bounded sources only), set
/// `max_segment_bytes` to [`usize::MAX`].
#[derive(Debug, Clone, Copy)]
pub struct ParseConfig {
    /// Maximum number of bytes allowed per EDIFACT segment.
    ///
    /// Defaults to [`DEFAULT_MAX_SEGMENT_BYTES`] (64 KiB).  Set to
    /// [`usize::MAX`] to disable the limit for trusted, bounded inputs.
    pub max_segment_bytes: usize,
    /// Maximum number of segments to parse before returning an error.
    ///
    /// `None` means no limit (default).
    pub max_segments: Option<usize>,
    /// Maximum total input bytes to consume before returning an error.
    ///
    /// `None` means no limit (default).
    pub max_input_bytes: Option<usize>,
    /// Maximum number of messages (UNH…UNT pairs) allowed per interchange.
    ///
    /// Defaults to `Some(1000)`.  Set to `None` to disable the limit for
    /// trusted, size-bounded sources only.  A crafted interchange with many
    /// lightweight messages inside the per-message limits can otherwise consume
    /// unbounded memory.
    pub max_messages_per_interchange: Option<usize>,
    /// Reference date used for profile validity lookups during `validate()`.
    ///
    /// When `None` (the default), `time::OffsetDateTime::now_utc().date()` is
    /// used at validation time.  Set this to a fixed date in tests so that
    /// profile resolution is deterministic regardless of when the test runs.
    ///
    /// # Example
    /// ```rust
    /// use edi_energy::ParseConfig;
    /// let cfg = ParseConfig::default()
    ///     .with_reference_date(
    ///         time::Date::from_calendar_date(2026, time::Month::January, 1).unwrap()
    ///     );
    /// ```
    pub reference_date: Option<time::Date>,
}

impl Default for ParseConfig {
    fn default() -> Self {
        Self {
            max_segment_bytes: DEFAULT_MAX_SEGMENT_BYTES,
            max_segments: Some(10_000),
            max_input_bytes: Some(10 * 1024 * 1024),
            max_messages_per_interchange: Some(1_000),
            reference_date: None,
        }
    }
}

impl ParseConfig {
    /// Set the reference date used for profile validity lookups during `validate()`.
    ///
    /// See [`ParseConfig::reference_date`] for details.
    #[must_use]
    pub fn with_reference_date(mut self, date: time::Date) -> Self {
        self.reference_date = Some(date);
        self
    }

    pub(crate) fn to_reader_config(self) -> ReaderConfig {
        let mut cfg = ReaderConfig::default().max_segment_bytes(self.max_segment_bytes);
        if let Some(n) = self.max_segments {
            cfg = cfg.max_segments(n);
        }
        if let Some(n) = self.max_input_bytes {
            cfg = cfg.max_input_bytes(n as u64);
        }
        cfg
    }
}

/// Parse a single EDI@Energy message from an in-memory byte slice.
///
/// The byte slice must contain exactly one EDIFACT message (UNB…UNZ envelope
/// with a single UNH…UNT message). If the byte slice contains multiple messages,
/// use [`parse_interchange`] instead.
///
/// # Errors
///
/// Returns `Err` when the input is not valid EDIFACT syntax or the message type
/// is unknown / compiled out.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(input), fields(bytes = input.len()))
)]
pub fn parse(input: &[u8]) -> Result<AnyMessage, Error> {
    parse_with_registry(
        input,
        ParseConfig::default(),
        crate::registry::ReleaseRegistry::global(),
    )
}

/// Parse only the UNH/BGM envelope fields from a byte slice, **without**
/// constructing typed message structs.
///
/// Returns a [`LightMessage`] that exposes message type, release, message
/// reference, and Prüfidentifikator at minimal cost.  Typed field extraction
/// (the `Vec<Dtm>`, `Vec<Nad>`, etc. on concrete message structs) is deferred
/// to [`LightMessage::into_message`].
///
/// Use this for routing/forwarding paths that must inspect envelope fields
/// before deciding whether to run full validation or typed access.
///
/// The default [`ParseConfig`] is applied.  For custom limits use
/// [`Parser::parse_envelope_only`].
///
/// # Errors
///
/// Returns `Err` on EDIFACT syntax errors or a missing UNH segment.
pub fn parse_envelope_only(input: &[u8]) -> Result<crate::light_message::LightMessage, Error> {
    let cfg = ParseConfig::default().to_reader_config();
    let segments: Vec<OwnedSegment> = edifact_rs::from_bytes_owned_with_config(input, cfg)
        .collect::<Result<_, _>>()
        .map_err(Error::Parse)?;
    crate::light_message::LightMessage::from_segments(
        segments,
        crate::registry::ReleaseRegistry::global(),
    )
}

/// Parse a single message using an explicit registry.
///
/// Used by [`Platform::parse`] to avoid the global-registry singleton.
/// Free functions (`parse`) delegate to this with
/// [`ReleaseRegistry::global()`].
pub(crate) fn parse_with_registry(
    input: &[u8],
    config: ParseConfig,
    registry: &crate::registry::ReleaseRegistry,
) -> Result<AnyMessage, Error> {
    let cfg = config.to_reader_config();
    let segments: Vec<OwnedSegment> = edifact_rs::from_bytes_owned_with_config(input, cfg)
        .collect::<Result<_, _>>()
        .map_err(Error::Parse)?;
    dispatch_message(segments, registry)
}

/// Parse all messages from an EDIFACT interchange (UNB…UNZ envelope containing
/// multiple UNH…UNT messages).
///
/// The default [`ParseConfig`] is applied, which enforces a limit of
/// **1 000 messages per interchange** ([`ParseConfig::default`] sets
/// `max_messages_per_interchange = Some(1_000)`).  Use [`Parser::with_config`]
/// to raise or remove this limit for large interchanges.
///
/// Returns a lazy iterator yielding one [`AnyMessage`] per message window.
/// An empty interchange (no messages) produces an empty iterator.
///
/// Each item is a `Result` — errors are surfaced per-message rather than
/// terminating the whole parse.  Collect to `Result<Vec<_>, _>` to fail fast,
/// or process each `Result` individually to skip bad messages.
///
/// # Examples
///
/// ```no_run
/// use edi_energy::{parse_interchange, EdiEnergyMessage};
///
/// let input = std::io::Cursor::new(b"UNB+UNOC:3+S:14+R:14+200101:0900+1'UNH+1+UTILMD:D:11A:UN:5.5.3a'BGM+E03+00011001+9'UNT+3+1'UNZ+1+1'");
/// for msg in parse_interchange(input) {
///     let msg = msg?;
///     if let Some(mt) = msg.try_message_type() { println!("type: {}", mt.as_str()); }
/// }
/// # Ok::<(), edi_energy::Error>(())
/// ```
///
/// # Errors
///
/// Each iterator item is `Result<AnyMessage, Error>`.
/// - [`Error::Parse`] — I/O or EDIFACT syntax error.
/// - [`Error::TooManyMessages`] — interchange contains more messages than
///   `ParseConfig::max_messages_per_interchange` (default: 1 000).  The
///   item at position `limit` is the first to return this error; the iterator
///   is not fused and will continue returning `TooManyMessages` for subsequent
///   items.  Use [`Parser::with_config`] with a higher limit if needed.
pub fn parse_interchange(reader: impl Read) -> impl Iterator<Item = Result<AnyMessage, Error>> {
    parse_interchange_with_registry(reader, ParseConfig::default())
}

/// Parse all messages from an interchange using the global registry.
///
/// Used by the free-function API with the global singleton.  Delegates to
/// `parse_interchange_impl` with the global `Arc`.
pub(crate) fn parse_interchange_with_registry(
    reader: impl Read,
    config: ParseConfig,
) -> impl Iterator<Item = Result<AnyMessage, Error>> {
    parse_interchange_impl(
        reader,
        config,
        std::sync::Arc::clone(crate::registry::ReleaseRegistry::global_arc()),
    )
}

/// Shared implementation for both the `&'static` and `Arc`-owned registry paths.
///
/// Eliminates the duplicate `max_messages_per_interchange` / `map` logic that
/// previously appeared in both `parse_interchange_with_registry` and
/// `parse_interchange_with_arc_registry` (F-020).
pub(crate) fn parse_interchange_impl(
    reader: impl Read,
    config: ParseConfig,
    registry: std::sync::Arc<crate::registry::ReleaseRegistry>,
) -> impl Iterator<Item = Result<AnyMessage, Error>> {
    let limit = config.max_messages_per_interchange;
    let cfg = config.to_reader_config();
    MessageWindowsIter::new(from_bufread_stream_with_config(BufReader::new(reader), cfg))
        .enumerate()
        .map(move |(index, window)| {
            if let Some(lim) = limit {
                if index >= lim {
                    return Err(Error::TooManyMessages { limit: lim });
                }
            }
            let window = window.map_err(Error::Parse)?;
            dispatch_message(window.segments, &registry)
        })
}

/// Parse all messages from an interchange using an `Arc`-owned registry.
///
/// Used by [`Platform::parse_interchange`] to avoid the `'static` bound on
/// the free-function variant.  Delegates to `parse_interchange_impl`.
pub(crate) fn parse_interchange_with_arc_registry(
    reader: impl Read,
    config: ParseConfig,
    registry: std::sync::Arc<crate::registry::ReleaseRegistry>,
) -> impl Iterator<Item = Result<AnyMessage, Error>> {
    parse_interchange_impl(reader, config, registry)
}

/// Shared implementation for the buffered (header-first, lazy-messages) path.
///
/// Used by [`Parser::parse_interchange_buffered`] with the global Arc and by
/// [`Platform`](crate::Platform) with its own isolated registry.  Holds the full
/// segment Vec in memory for the lifetime of the returned [`InterchangeIter`].
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(reader, config, registry))
)]
pub(crate) fn parse_interchange_buffered_impl(
    reader: impl Read,
    config: ParseConfig,
    registry: std::sync::Arc<crate::registry::ReleaseRegistry>,
) -> Result<(crate::interchange::InterchangeHeader, InterchangeIter), Error> {
    let cfg = config.to_reader_config();
    let segments: Vec<OwnedSegment> = from_bufread_stream_with_config(BufReader::new(reader), cfg)
        .collect::<Result<_, _>>()
        .map_err(Error::Parse)?;

    // Parse UNB header eagerly so callers can route before deserialising messages.
    let header = parse_interchange_header_from_segments(&segments)?;

    let unz_ref = segments
        .iter()
        .rfind(|s| s.tag == "UNZ")
        .and_then(|unz| unz.element_str(1))
        .map(str::to_owned);
    let declared_count = segments
        .iter()
        .rfind(|s| s.tag == "UNZ")
        .and_then(|unz| unz.element_str(0))
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);

    let msg_iter = MessageWindowsIter::new(segments.into_iter().map(
        Ok::<_, edifact_rs::EdifactError>
            as fn(OwnedSegment) -> Result<OwnedSegment, edifact_rs::EdifactError>,
    ));

    let iter = InterchangeIter {
        inner: msg_iter,
        header: header.clone(),
        registry,
        limit: config.max_messages_per_interchange,
        index: 0,
        actual_count: 0,
        declared_count,
        unz_ref,
        unz_checked: false,
        done: false,
    };

    Ok((header, iter))
}

// ── Parser struct ────────────────────────────────────────────────────────────

/// A configured parser for EDI@Energy messages and interchanges.
///
/// `Parser` is the primary API for parsing with custom [`ParseConfig`].
/// Construct with [`Parser::new`] (default config) or [`Parser::with_config`].
///
/// The free functions [`parse`] and [`parse_interchange`] are convenience
/// wrappers around `Parser::new()` for the default-config case.  Use `Parser`
/// directly whenever you need custom segment limits, a reference date, or the
/// more advanced interchange paths.
///
/// # Example
///
/// ```no_run
/// use edi_energy::{Parser, ParseConfig};
///
/// let config = ParseConfig { max_segments: Some(5_000), ..ParseConfig::default() };
/// let parser = Parser::with_config(config);
///
/// // Single message from bytes
/// let msg = parser.parse(b"UNH+1+UTILMD:D:11A:UN:S2.1'...")?;
///
/// // Single message from a reader
/// let reader = std::fs::File::open("message.edi")?;
/// let msg = parser.parse_reader(reader)?;
///
/// // Interchange — lazy iterator
/// let reader = std::fs::File::open("interchange.edi")?;
/// for result in parser.parse_interchange(reader) {
///     let _msg: edi_energy::AnyMessage = result?;
/// }
///
/// // Interchange — envelope first, messages lazily
/// let reader = std::fs::File::open("interchange.edi")?;
/// let (header, iter) = parser.parse_interchange_buffered(reader)?;
/// println!("sender: {}", header.sender_id);
/// for result in iter { let env = result?; }
///
/// // Interchange — fully materialise into ParsedInterchange
/// let reader = std::fs::File::open("interchange.edi")?;
/// let ic = parser.parse_interchange_full(reader)?;
/// assert!(ic.is_structurally_valid());
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Parser {
    config: ParseConfig,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    /// Create a `Parser` with the default [`ParseConfig`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ParseConfig::default(),
        }
    }

    /// Create a `Parser` with the given [`ParseConfig`].
    #[must_use]
    pub fn with_config(config: ParseConfig) -> Self {
        Self { config }
    }

    /// Parse a single EDI@Energy message from an in-memory byte slice.
    ///
    /// # Errors
    ///
    /// Returns `Err` on EDIFACT syntax errors or unknown message type.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, input), fields(bytes = input.len()))
    )]
    pub fn parse(&self, input: &[u8]) -> Result<AnyMessage, Error> {
        parse_with_registry(
            input,
            self.config,
            crate::registry::ReleaseRegistry::global(),
        )
    }

    /// Parse a single EDI@Energy message from a [`Read`] source.
    ///
    /// Reads the entire source into a segment list, then dispatches to the
    /// appropriate typed message variant.  For `&[u8]` inputs, prefer
    /// [`Parser::parse`] to avoid the buffered read.
    ///
    /// # Errors
    ///
    /// Returns `Err` on I/O errors, EDIFACT syntax errors, or unknown message type.
    pub fn parse_reader(&self, reader: impl Read) -> Result<AnyMessage, Error> {
        let cfg = self.config.to_reader_config();
        let segments: Vec<OwnedSegment> =
            from_bufread_stream_with_config(BufReader::new(reader), cfg)
                .collect::<Result<_, _>>()
                .map_err(Error::Parse)?;
        dispatch_message(segments, crate::registry::ReleaseRegistry::global())
    }

    /// Parse only the UNH/BGM envelope fields from a byte slice, without
    /// constructing typed message structs.
    ///
    /// Returns a [`LightMessage`] that exposes message type, release, message
    /// reference, and Prüfidentifikator at minimal cost (~zero allocation beyond
    /// the raw segment buffer).  Useful for routing and forwarding paths that
    /// must inspect envelope fields before deciding whether to run full validation.
    ///
    /// Call [`LightMessage::into_message`] when full typed access is needed.
    ///
    /// # Errors
    ///
    /// Returns `Err` on EDIFACT syntax errors or a missing UNH segment.
    pub fn parse_envelope_only(
        &self,
        input: &[u8],
    ) -> Result<crate::light_message::LightMessage, Error> {
        let cfg = self.config.to_reader_config();
        let segments: Vec<OwnedSegment> = edifact_rs::from_bytes_owned_with_config(input, cfg)
            .collect::<Result<_, _>>()
            .map_err(Error::Parse)?;
        crate::light_message::LightMessage::from_segments(
            segments,
            crate::registry::ReleaseRegistry::global(),
        )
    }

    /// Parse all messages from an EDIFACT interchange (lazy iterator).
    ///
    /// Returns a lazy iterator yielding one `Result<AnyMessage, Error>` per
    /// UNH…UNT message window.  The UNB/UNZ envelope is consumed but not
    /// preserved; use [`Parser::parse_interchange_buffered`] or
    /// [`Parser::parse_interchange_full`] when the envelope is needed.
    ///
    /// The `max_messages_per_interchange` from the parser's [`ParseConfig`]
    /// (default: 1 000) is enforced.  Override via [`Parser::with_config`].
    ///
    /// # Errors
    ///
    /// Each iterator item is `Result<AnyMessage, Error>`.
    /// - [`Error::Parse`] — I/O or EDIFACT syntax error.
    /// - [`Error::TooManyMessages`] — interchange exceeds the configured
    ///   message limit (`ParseConfig::max_messages_per_interchange`).
    pub fn parse_interchange(
        &self,
        reader: impl Read,
    ) -> impl Iterator<Item = Result<AnyMessage, Error>> {
        parse_interchange_with_registry(reader, self.config)
    }

    /// Parse an EDIFACT interchange, returning the [`InterchangeHeader`] eagerly
    /// and messages lazily via [`InterchangeIter`].
    ///
    /// **Segment tokenization is eager** — the entire input is tokenized into a
    /// `Vec<OwnedSegment>` before this method returns.  **Message deserialization
    /// is lazy** — typed struct construction is deferred to each `next()` call.
    ///
    /// This is the recommended path for AS4 adapters that must inspect the
    /// UNB sender/receiver GLN and decide whether to process a message before
    /// paying the deserialization cost.
    ///
    /// The `max_messages_per_interchange` from the parser's [`ParseConfig`]
    /// (default: 1 000) is enforced during iteration.
    ///
    /// # Errors
    ///
    /// Returns `Err` eagerly on I/O errors, syntax errors, or a missing UNB.
    /// Per-message errors and [`Error::TooManyMessages`] are returned as
    /// `Err` iterator items from the returned [`InterchangeIter`].
    pub fn parse_interchange_buffered(
        &self,
        reader: impl Read,
    ) -> Result<(crate::interchange::InterchangeHeader, InterchangeIter), Error> {
        parse_interchange_buffered_impl(
            reader,
            self.config,
            std::sync::Arc::clone(crate::registry::ReleaseRegistry::global_arc()),
        )
    }

    /// Fully parse an EDIFACT interchange into a [`ParsedInterchange`], materialising
    /// all messages eagerly.
    ///
    /// Use this when you need all messages and the UNB/UNZ envelope together.  For
    /// large interchanges prefer [`Parser::parse_interchange_buffered`] to keep memory
    /// usage proportional to the number of messages you actually need.
    ///
    /// Validates the UNZ control reference and message count before returning.
    ///
    /// # Errors
    ///
    /// Returns `Err` on I/O errors, syntax errors, envelope structural errors
    /// (missing UNB/UNZ, mismatched control reference or count), or individual
    /// message parse errors.
    pub fn parse_interchange_full(
        &self,
        reader: impl Read,
    ) -> Result<crate::interchange::ParsedInterchange, Error> {
        let reader_cfg = self.config.to_reader_config();
        let segments: Vec<OwnedSegment> =
            from_bufread_stream_with_config(BufReader::new(reader), reader_cfg)
                .collect::<Result<_, _>>()
                .map_err(Error::Parse)?;
        parse_interchange_full_from_segments(segments, &self.config)
    }
}

/// Lazy iterator over [`MessageEnvelope`][crate::interchange::MessageEnvelope]s
/// from a parsed EDIFACT interchange.
///
/// Returned by [`Parser::parse_interchange_buffered`].
///
/// After all messages are yielded, the iterator emits one final `Err` item if
/// the UNZ control reference or message count is mismatched; after that it
/// returns `None` permanently.
pub struct InterchangeIter {
    #[allow(clippy::type_complexity)]
    inner: MessageWindowsIter<
        std::iter::Map<
            std::vec::IntoIter<OwnedSegment>,
            fn(OwnedSegment) -> Result<OwnedSegment, edifact_rs::EdifactError>,
        >,
    >,
    header: crate::interchange::InterchangeHeader,
    registry: std::sync::Arc<crate::registry::ReleaseRegistry>,
    limit: Option<usize>,
    index: usize,
    actual_count: usize,
    declared_count: usize,
    unz_ref: Option<String>,
    unz_checked: bool,
    done: bool,
}

impl Iterator for InterchangeIter {
    type Item = Result<crate::interchange::MessageEnvelope, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        // Try to advance the message window iterator.
        if let Some(window_result) = self.inner.next() {
            let index = self.index;
            self.index += 1;
            self.actual_count += 1;

            // Check per-interchange message limit.
            if let Some(lim) = self.limit {
                if index >= lim {
                    self.done = true;
                    return Some(Err(Error::TooManyMessages { limit: lim }));
                }
            }

            let result = (|| {
                let window = window_result.map_err(Error::Parse)?;
                let message = dispatch_message(window.segments, &self.registry)?;
                Ok(crate::interchange::MessageEnvelope {
                    message,
                    header: self.header.clone(),
                    message_index: index,
                })
            })();
            Some(result)
        } else {
            // All message windows exhausted — check UNZ.
            if self.unz_checked {
                self.done = true;
                return None;
            }
            self.unz_checked = true;
            // Validate UNZ control reference.
            if let Some(ref uref) = self.unz_ref {
                if !uref.is_empty() && uref.as_str() != self.header.control_ref.as_ref() {
                    self.done = true;
                    return Some(Err(Error::InterchangeRefMismatch {
                        unb_ref: self.header.control_ref.to_string(),
                        unz_ref: uref.clone(),
                    }));
                }
            }
            // Validate UNZ message count.
            if self.declared_count != 0 && self.declared_count != self.actual_count {
                self.done = true;
                return Some(Err(Error::InterchangeCountMismatch {
                    declared: self.declared_count,
                    actual: self.actual_count,
                }));
            }
            self.done = true;
            None
        }
    }
}

/// Extract the [`InterchangeHeader`][crate::interchange::InterchangeHeader] from
/// the UNB segment in a segment list.
fn parse_interchange_header_from_segments(
    segments: &[OwnedSegment],
) -> Result<crate::interchange::InterchangeHeader, Error> {
    use crate::interchange::InterchangeHeader;
    let unb = segments
        .iter()
        .find(|s| s.tag == "UNB")
        .ok_or(Error::MissingSegment("UNB"))?;

    let syntax_id = unb.component_str(0, 0).unwrap_or("UNOC").to_owned();
    let syntax_version: u8 = unb
        .component_str(0, 1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let sender_id = unb.component_str(1, 0).unwrap_or("").to_owned();
    let sender_qualifier = unb.component_str(1, 2).unwrap_or("").to_owned();
    let receiver_id = unb.component_str(2, 0).unwrap_or("").to_owned();
    let receiver_qualifier = unb.component_str(2, 2).unwrap_or("").to_owned();
    let transmission_datetime = parse_unb_datetime(
        unb.component_str(3, 0).unwrap_or(""),
        unb.component_str(3, 1).unwrap_or(""),
    );
    let control_ref = unb.element_str(4).unwrap_or("").to_owned();

    Ok(InterchangeHeader {
        sender_id: sender_id.into_boxed_str(),
        sender_qualifier: sender_qualifier.into_boxed_str(),
        receiver_id: receiver_id.into_boxed_str(),
        receiver_qualifier: receiver_qualifier.into_boxed_str(),
        transmission_datetime,
        control_ref: control_ref.into_boxed_str(),
        syntax_id: syntax_id.into_boxed_str(),
        syntax_version,
    })
}

/// Core implementation: parse UNB+UNZ envelope and all messages from a flat segment list.
fn parse_interchange_full_from_segments(
    segments: Vec<OwnedSegment>,
    config: &ParseConfig,
) -> Result<crate::interchange::ParsedInterchange, Error> {
    use crate::interchange::{InterchangeHeader, MessageEnvelope, ParsedInterchange};

    // ── Parse UNB ──────────────────────────────────────────────────────────────
    let unb = segments
        .iter()
        .find(|s| s.tag == "UNB")
        .ok_or(Error::MissingSegment("UNB"))?;

    // S001: syntax identifier composite — components: [syntax_id, syntax_version]
    let syntax_id = unb.component_str(0, 0).unwrap_or("UNOC").to_owned();
    let syntax_version: u8 = unb
        .component_str(0, 1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);

    // S002: sender — components: [id, sub_id, id_qualifier, routing]
    let sender_id = unb.component_str(1, 0).unwrap_or("").to_owned();
    let sender_qualifier = unb.component_str(1, 2).unwrap_or("").to_owned();

    // S003: receiver — components: [id, sub_id, id_qualifier, routing]
    let receiver_id = unb.component_str(2, 0).unwrap_or("").to_owned();
    let receiver_qualifier = unb.component_str(2, 2).unwrap_or("").to_owned();

    // S004: date+time of preparation — components: [date (YYMMDD), time (HHMM)]
    let transmission_datetime = parse_unb_datetime(
        unb.component_str(3, 0).unwrap_or(""),
        unb.component_str(3, 1).unwrap_or(""),
    );

    // DE 0020: interchange control reference
    let control_ref = unb.element_str(4).unwrap_or("").to_owned();

    #[cfg(feature = "tracing")]
    let _span = tracing::debug_span!(
        "parse_interchange",
        sender = %sender_id,
        receiver = %receiver_id,
        control_ref = %control_ref,
        segment_count = segments.len(),
    )
    .entered();

    let header = InterchangeHeader {
        sender_id: sender_id.into_boxed_str(),
        sender_qualifier: sender_qualifier.into_boxed_str(),
        receiver_id: receiver_id.into_boxed_str(),
        receiver_qualifier: receiver_qualifier.into_boxed_str(),
        transmission_datetime,
        control_ref: control_ref.into_boxed_str(),
        syntax_id: syntax_id.into_boxed_str(),
        syntax_version,
    };

    // ── Parse UNZ ─────────────────────────────────────────────────────────────
    let unz = segments.iter().rfind(|s| s.tag == "UNZ");
    let (trailer_ref, declared_message_count) = match unz {
        Some(unz) => {
            let count: usize = unz.element_str(0).and_then(|s| s.parse().ok()).unwrap_or(0);
            let tref = unz.element_str(1).unwrap_or("").to_owned();
            (tref.into_boxed_str(), count)
        }
        None => ("".into(), 0),
    };

    // ── Dispatch all messages using MessageWindowsIter ────────────────────────
    let msg_iter = edifact_rs::MessageWindowsIter::new(
        segments.into_iter().map(Ok::<_, edifact_rs::EdifactError>),
    );

    let mut messages: Vec<MessageEnvelope> = Vec::new();
    for (index, window_result) in msg_iter.enumerate() {
        // F-012: enforce max_messages_per_interchange before parsing the next message.
        if let Some(limit) = config.max_messages_per_interchange {
            if index >= limit {
                return Err(Error::TooManyMessages { limit });
            }
        }
        let window = window_result.map_err(Error::Parse)?;
        let message =
            dispatch_message(window.segments, crate::registry::ReleaseRegistry::global())?;
        messages.push(MessageEnvelope {
            message,
            header: header.clone(),
            message_index: index,
        });
    }

    // F-013: validate UNZ control reference matches UNB control reference.
    if !trailer_ref.is_empty() && trailer_ref.as_ref() != header.control_ref.as_ref() {
        return Err(Error::InterchangeRefMismatch {
            unb_ref: header.control_ref.to_string(),
            unz_ref: trailer_ref.to_string(),
        });
    }

    // F-013: validate UNZ message count matches actual message count.
    if declared_message_count != 0 && declared_message_count != messages.len() {
        return Err(Error::InterchangeCountMismatch {
            declared: declared_message_count,
            actual: messages.len(),
        });
    }

    Ok(ParsedInterchange {
        header,
        messages,
        trailer_ref,
        declared_message_count,
    })
}

/// Parse a UNB S004 date+time into an `OffsetDateTime`.
///
/// EDIFACT date format: `YYMMDD` or `YYYYMMDD`; time format: `HHMM` or `HHMMSS`.
fn parse_unb_datetime(date: &str, time: &str) -> Option<time::OffsetDateTime> {
    use time::{Date, Month, OffsetDateTime, Time, UtcOffset};

    let (year, month_n, day) = match date.len() {
        6 => {
            // YYMMDD — interpret YY as 20YY (valid for 2000–2099)
            let yy: i32 = date[0..2].parse().ok()?;
            let mm: u8 = date[2..4].parse().ok()?;
            let dd: u8 = date[4..6].parse().ok()?;
            (2000 + yy, mm, dd)
        }
        8 => {
            let yyyy: i32 = date[0..4].parse().ok()?;
            let mm: u8 = date[4..6].parse().ok()?;
            let dd: u8 = date[6..8].parse().ok()?;
            (yyyy, mm, dd)
        }
        _ => return None,
    };

    let month = Month::try_from(month_n).ok()?;
    let d = Date::from_calendar_date(year, month, day).ok()?;

    let (hh, mi, ss) = match time.len() {
        4 => {
            let hh: u8 = time[0..2].parse().ok()?;
            let mi: u8 = time[2..4].parse().ok()?;
            (hh, mi, 0u8)
        }
        6 => {
            let hh: u8 = time[0..2].parse().ok()?;
            let mi: u8 = time[2..4].parse().ok()?;
            let ss: u8 = time[4..6].parse().ok()?;
            (hh, mi, ss)
        }
        _ => return None,
    };

    let t = Time::from_hms(hh, mi, ss).ok()?;
    Some(OffsetDateTime::new_utc(d, t).replace_offset(UtcOffset::UTC))
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// Inspect the UNH segment and dispatch to the correct [`AnyMessage`] variant,
/// using the provided registry to look up PID source strategies.
pub(crate) fn dispatch_message(
    segments: Vec<OwnedSegment>,
    registry: &crate::registry::ReleaseRegistry,
) -> Result<AnyMessage, Error> {
    // Locate the UNH segment (always the second segment after UNB).
    // Extract all needed strings before releasing the borrow so `segments` can
    // be moved into the concrete message constructor below.
    let (message_ref, msg_type_code, assoc_code) = {
        let unh = segments
            .iter()
            .find(|s| s.tag == "UNH")
            .ok_or(Error::MissingSegment("UNH"))?;

        let message_ref = unh.element_str(0).unwrap_or_default().to_owned();
        // S009 composite — element 1:
        //   component 0: DE 0065 — message type (e.g. "UTILMD")
        //   component 4: DE 0057 — association assigned code (e.g. "5.5.3a")
        let msg_type_code = unh
            .component_str(1, 0)
            .ok_or(Error::MalformedSegment("UNH"))?
            .to_owned();
        let assoc_code = unh.component_str(1, 4).unwrap_or_default().to_owned();
        (message_ref, msg_type_code, assoc_code)
    };

    // Prüfidentifikator extraction: look up the profile to determine whether
    // this message type stores its PID in BGM element 1 (DE 1004) or in a
    // top-level RFF+Z13 segment.  The strategy is driven by the profile
    // registry so that no message-type list needs to be maintained here.
    let pruefidentifikator: Option<u32> =
        match resolve_pid_source(&msg_type_code, &assoc_code, registry) {
            crate::registry::PidSource::RffZ13 => segments
                .iter()
                .find(|s| s.tag == "RFF" && (s.element_str(0) == Some("Z13")))
                .and_then(|rff| rff.component_str(0, 1))
                .and_then(|s| s.parse().ok()),
            crate::registry::PidSource::BgmDe1004 => segments
                .iter()
                .find(|s| s.tag == "BGM")
                .and_then(|bgm| bgm.element_str(1))
                .and_then(|s| s.parse().ok()),
        };

    // Warn when the association code is not one of the recognised EDI@Energy release
    // patterns.  An `Opaque` release will cause `validate()` to return
    // `ProfileNotFound`; surfacing the warning here makes it easier to diagnose.
    if matches!(
        crate::release::Release::new(&assoc_code).kind(),
        crate::release::ReleaseKind::Opaque(_)
    ) {
        // Sanitize the release code before including it in any log output.
        // Valid BDEW codes are ≤ 16 ASCII alphanumeric chars plus '.'; anything
        // else may contain log-injection sequences or GDPR-sensitive data.
        let safe_code = sanitize_release_code(&assoc_code);
        #[cfg(feature = "tracing")]
        tracing::warn!(
            release = %safe_code,
            "unrecognised EDI@Energy release code — validate() will return ProfileNotFound"
        );
        // Always surface the warning even when the `tracing` feature is disabled,
        // so misconfigured senders are never silently accepted.
        #[cfg(not(feature = "tracing"))]
        eprintln!(
            "edi-energy: warning: unrecognised release code `{safe_code}` — \
             validate() will return ProfileNotFound"
        );
    } else {
        #[cfg(feature = "tracing")]
        tracing::debug!(
            message_type = %msg_type_code,
            release = %assoc_code,
            segment_count = segments.len(),
            "parsed EDIFACT message"
        );
    }

    dispatch_by_type(
        &msg_type_code,
        segments,
        message_ref,
        assoc_code,
        pruefidentifikator,
    )
}

/// Dispatch to a concrete `AnyMessage` variant based on the decoded type code.
#[allow(unused_variables)] // params unused when all features are disabled
#[allow(clippy::too_many_lines)]
fn dispatch_by_type(
    msg_type_code: &str,
    segments: Vec<OwnedSegment>,
    message_ref: String,
    assoc_code: String,
    pruefidentifikator: Option<u32>,
) -> Result<AnyMessage, Error> {
    match msg_type_code {
        #[cfg(feature = "utilmd")]
        "UTILMD" => Ok(AnyMessage::Utilmd(
            crate::messages::utilmd::UtilmdMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "mscons")]
        "MSCONS" => Ok(AnyMessage::Mscons(
            crate::messages::mscons::MsconsMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "aperak")]
        "APERAK" => Ok(AnyMessage::Aperak(
            crate::messages::aperak::AperakMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "contrl")]
        "CONTRL" => Ok(AnyMessage::Contrl(
            crate::messages::contrl::ContrlMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "invoic")]
        "INVOIC" => Ok(AnyMessage::Invoic(
            crate::messages::invoic::InvoicMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "remadv")]
        "REMADV" => Ok(AnyMessage::Remadv(
            crate::messages::remadv::RemadvMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "orders")]
        "ORDERS" => Ok(AnyMessage::Orders(
            crate::messages::orders::OrdersMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "iftsta")]
        "IFTSTA" => Ok(AnyMessage::Iftsta(
            crate::messages::iftsta::IftstaMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "insrpt")]
        "INSRPT" => Ok(AnyMessage::Insrpt(
            crate::messages::insrpt::InsrptMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "reqote")]
        "REQOTE" => Ok(AnyMessage::Reqote(
            crate::messages::reqote::ReqoteMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "partin")]
        "PARTIN" => Ok(AnyMessage::Partin(
            crate::messages::partin::PartinMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "ordchg")]
        "ORDCHG" => Ok(AnyMessage::Ordchg(
            crate::messages::ordchg::OrdchgMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "ordrsp")]
        "ORDRSP" => Ok(AnyMessage::Ordrsp(
            crate::messages::ordrsp::OrdrespMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "quotes")]
        "QUOTES" => Ok(AnyMessage::Quotes(
            crate::messages::quotes::QuotesMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "comdis")]
        "COMDIS" => Ok(AnyMessage::Comdis(
            crate::messages::comdis::ComdisMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "pricat")]
        "PRICAT" => Ok(AnyMessage::Pricat(
            crate::messages::pricat::PricatMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        #[cfg(feature = "utilts")]
        "UTILTS" => Ok(AnyMessage::Utilts(
            crate::messages::utilts::UtiltsMessage::from_parts(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
            ),
        )),
        other => {
            // Check if this is a known EDI@Energy message type whose Cargo feature
            // is not compiled in.  Return FeatureNotEnabled to give the caller
            // actionable guidance instead of silently producing Unknown.
            if let Some(mt) = MessageType::from_unh_code(other) {
                if !mt.is_feature_enabled() {
                    return Err(Error::FeatureNotEnabled {
                        message_type: other.to_owned(),
                        feature: mt.as_str().to_lowercase(),
                    });
                }
            }
            // Truly unknown type (not in EDI@Energy): return Unknown for pass-through.
            Ok(AnyMessage::Unknown {
                message_type_code: other.into(),
                release: crate::Release::new(&assoc_code),
                message_ref: message_ref.into(),
                segments,
            })
        }
    }
}

// ── MessageType helper ────────────────────────────────────────────────────────

impl MessageType {
    /// Returns `true` when the Cargo feature for this message type is compiled in.
    #[must_use]
    pub fn is_feature_enabled(self) -> bool {
        match self {
            MessageType::Utilmd => cfg!(feature = "utilmd"),
            MessageType::Mscons => cfg!(feature = "mscons"),
            MessageType::Aperak => cfg!(feature = "aperak"),
            MessageType::Contrl => cfg!(feature = "contrl"),
            MessageType::Invoic => cfg!(feature = "invoic"),
            MessageType::Remadv => cfg!(feature = "remadv"),
            MessageType::Orders => cfg!(feature = "orders"),
            MessageType::Iftsta => cfg!(feature = "iftsta"),
            MessageType::Insrpt => cfg!(feature = "insrpt"),
            MessageType::Reqote => cfg!(feature = "reqote"),
            MessageType::Partin => cfg!(feature = "partin"),
            MessageType::Ordchg => cfg!(feature = "ordchg"),
            MessageType::Ordrsp => cfg!(feature = "ordrsp"),
            MessageType::Quotes => cfg!(feature = "quotes"),
            MessageType::Comdis => cfg!(feature = "comdis"),
            MessageType::Pricat => cfg!(feature = "pricat"),
            MessageType::Utilts => cfg!(feature = "utilts"),
        }
    }
}
