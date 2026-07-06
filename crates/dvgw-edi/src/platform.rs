use edifact_rs::{
    OwnedSegment, ProfileRulePack, ReaderConfig, ValidationContext, ValidationIssue,
    ValidationSeverity, from_bytes_owned_with_config,
};

use crate::{
    AnyDvgwMessage, DvgwMessageType, Error,
    error::sanitize_code,
    message::{extract_message_ref, extract_message_type_code},
    report::DvgwReport,
    validate as sem,
};

/// Explicit application handle for DVGW EDIFACT processing.
///
/// `DvgwPlatform` bundles message-type dispatch and parse configuration so
/// multiple platform instances can coexist in the same process — e.g. for
/// test isolation or multi-tenant gateways.
///
/// For most applications, [`DvgwPlatform::default()`] is sufficient.
///
/// # Usage
///
/// ```rust,no_run
/// use dvgw_edi::DvgwPlatform;
///
/// let platform = DvgwPlatform::default();
/// let input: &[u8] = b"...EDIFACT bytes...";
/// let msg = platform.parse(input)?;
/// # Ok::<(), dvgw_edi::Error>(())
/// ```
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
    /// Construct a platform with default parse configuration.
    ///
    /// Uses the `edifact_rs` default [`ReaderConfig`], which applies a 64 KiB
    /// per-segment byte limit to guard against `DoS` attacks.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ReaderConfig::default(),
        }
    }

    /// Construct a platform with all built-in DVGW profiles.
    ///
    /// Equivalent to [`DvgwPlatform::new()`]. The name mirrors the intended
    /// future API once compiled-in profile data and strict segment validation
    /// are added. Prefer this name in application code for forward-compatibility.
    #[must_use]
    pub fn with_all_profiles() -> Self {
        Self::new()
    }

    /// Construct a platform with a custom `edifact_rs` [`ReaderConfig`].
    #[must_use]
    pub fn with_config(config: ReaderConfig) -> Self {
        Self { config }
    }

    /// Parse a raw DVGW EDIFACT interchange from a byte slice.
    ///
    /// Tokenises the input using the `edifact_rs` parser, extracts the message
    /// type from the UNH segment, and dispatches to the appropriate concrete
    /// message constructor.
    ///
    /// # Errors
    ///
    /// - [`Error::Parse`] — the input is not valid EDIFACT syntax.
    /// - [`Error::UnknownMessageType`] — the UNH type code is not a recognised DVGW format.
    /// - [`Error::FeatureNotEnabled`] — the message type is known but its Cargo feature
    ///   is not compiled in.
    #[must_use = "parse result must be checked"]
    pub fn parse(&self, input: &[u8]) -> Result<AnyDvgwMessage, Error> {
        #[cfg(feature = "tracing")]
        let _span = tracing::debug_span!("dvgw_parse", input_len = input.len()).entered();

        let segments: Vec<OwnedSegment> = from_bytes_owned_with_config(input, self.config)
            .collect::<Result<_, _>>()
            .map_err(Error::Parse)?;

        // Extract the message type from UNH
        let type_code = extract_message_type_code(&segments).unwrap_or_default();

        let Some(msg_type) = DvgwMessageType::from_unh_code(&type_code) else {
            #[cfg(feature = "tracing")]
            tracing::warn!(raw_code = %crate::error::sanitize_code(&type_code), "unknown DVGW message type");
            return Err(Error::UnknownMessageType {
                raw_code: sanitize_code(&type_code),
            });
        };

        #[cfg(feature = "tracing")]
        tracing::debug!(message_type = %msg_type, "dispatching DVGW message");

        dispatch(segments, msg_type)
    }

    /// Parse and validate a raw DVGW EDIFACT interchange from a byte slice.
    ///
    /// Runs two validation passes:
    ///
    /// 1. **Envelope validation** (when a UNB/UNZ wrapper is present) — checks
    ///    that UNB/UNZ message counts are correct and the interchange structure
    ///    is well-formed.  A hard structural failure (e.g. missing UNZ, stray
    ///    segments) returns `Err(Error::Parse(…))`.
    ///
    /// 2. **Semantic validation** — checks message-type-specific mandatory
    ///    elements: BGM presence, NAD+MS / NAD+MR role codes, mandatory DTM
    ///    timing qualifiers, and correlation references (e.g. NOMRES RFF+Z13).
    ///    Semantic findings are collected as `DvgwIssue` items in the returned
    ///    [`DvgwReport`] rather than hard errors.
    ///
    /// Use [`parse`](Self::parse) when you only need the parsed struct without
    /// validation overhead.  Use `validate` when processing inbound messages
    /// from trading partners where conformance must be verified.
    ///
    /// # Errors
    ///
    /// - [`Error::Parse`] — the EDIFACT byte stream is syntactically invalid or
    ///   the UNB/UNZ interchange envelope is structurally broken.
    /// - [`Error::UnknownMessageType`] — the UNH type code is not a recognised
    ///   DVGW format.
    /// - [`Error::FeatureNotEnabled`] — the message type is known but its Cargo
    ///   feature is not compiled in.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use dvgw_edi::DvgwPlatform;
    ///
    /// # let input: &[u8] = b"";
    /// let report = DvgwPlatform::default().validate(input)?;
    ///
    /// if !report.is_valid() {
    ///     for issue in report.errors() {
    ///         eprintln!("{}: {}", issue.severity, issue.message);
    ///     }
    /// }
    /// # Ok::<(), dvgw_edi::Error>(())
    /// ```
    #[must_use = "validate result must be checked"]
    pub fn validate(&self, input: &[u8]) -> Result<DvgwReport, Error> {
        #[cfg(feature = "tracing")]
        let _span = tracing::debug_span!("dvgw_validate", input_len = input.len()).entered();

        let segments: Vec<OwnedSegment> = from_bytes_owned_with_config(input, self.config)
            .collect::<Result<_, _>>()
            .map_err(Error::Parse)?;

        // Layer 1: EDIFACT envelope validation (lenient) — only when a UNB
        // interchange wrapper is present.  Bare message streams (UNH…UNT
        // without UNB/UNZ) are valid in DVGW contexts and skip this layer.
        //
        // Unlike the strict path used by `parse()`, the lenient path converts
        // UNT/UNZ count mismatches into report issues rather than hard errors.
        // A structurally unrecoverable interchange (missing UNB/UNZ entirely,
        // stray segments) still triggers a hard `Error::Parse` because the
        // message content cannot be extracted.
        let mut envelope_issues: Vec<ValidationIssue> = Vec::new();
        if segments.iter().any(|s| s.tag == "UNB") {
            let lenient = edifact_rs::validate_envelope_lenient_owned(&segments);
            if lenient.interchange.is_none() {
                // Structurally unrecoverable — the interchange topology is
                // broken and we cannot extract message content.
                return Err(Error::Parse(
                    lenient.errors.into_iter().next().unwrap_or_else(|| {
                        edifact_rs::EdifactError::MissingSegment {
                            tag: "UNB".to_owned(),
                            expected_position: "start of interchange".to_owned(),
                        }
                    }),
                ));
            }
            // Fold count-only violations into the report.
            for e in lenient.errors {
                envelope_issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!("EDIFACT envelope violation: {e}"),
                    )
                    .with_rule_id("ENVELOPE-COUNT-MISMATCH")
                    .with_segment("UNZ"),
                );
            }
        }

        // Extract message type and reference for dispatch and report header.
        let type_code = extract_message_type_code(&segments).unwrap_or_default();
        let Some(msg_type) = DvgwMessageType::from_unh_code(&type_code) else {
            return Err(Error::UnknownMessageType {
                raw_code: sanitize_code(&type_code),
            });
        };
        let message_ref = extract_message_ref(&segments).to_owned();

        // Strip interchange/functional-group envelope segments before
        // semantic validation — identical to the `parse()` path.
        let msg_segs: Vec<OwnedSegment> = segments
            .into_iter()
            .filter(|s| !matches!(s.tag.as_str(), "UNB" | "UNZ" | "UNG" | "UNE"))
            .collect();

        // Layer 2: semantic validation via ProfileRulePack +
        // ValidationContext::validate_lenient_owned — the same machinery used
        // by edi-energy (minus the MIG/AHB directory layers that DVGW formats
        // do not yet have compiled-in profiles for).
        let pack = semantic_pack(msg_type)?;
        let report = ValidationContext::builder()
            .with_message_type(msg_type.as_str())
            .with_message_ref(&message_ref)
            .bail_on_first_critical(true)
            .with_profile_pack(pack)
            .build()
            .validate_lenient_owned(&msg_segs);

        // Merge envelope violations (Layer 1) with semantic issues (Layer 2)
        // into a single flat list, envelope first.
        let all: Vec<ValidationIssue> = envelope_issues
            .into_iter()
            .chain(report.iter_issues().cloned())
            .collect();

        Ok(DvgwReport::new(msg_type, message_ref, all))
    }
}

/// Dispatch parsed segments to the concrete message constructor.
#[allow(clippy::too_many_lines)]
fn dispatch(
    segments: Vec<edifact_rs::OwnedSegment>,
    msg_type: DvgwMessageType,
) -> Result<AnyDvgwMessage, Error> {
    match msg_type {
        #[cfg(feature = "alocat")]
        DvgwMessageType::Alocat => {
            use crate::messages::alocat::AlocatMessage;
            Ok(AnyDvgwMessage::Alocat(Box::new(
                AlocatMessage::from_segments(segments),
            )))
        }
        #[cfg(not(feature = "alocat"))]
        DvgwMessageType::Alocat => Err(Error::FeatureNotEnabled {
            message_type: "ALOCAT".into(),
            feature: "alocat".into(),
        }),
        #[cfg(feature = "nomint")]
        DvgwMessageType::Nomint => {
            use crate::messages::nomint::NomintMessage;
            Ok(AnyDvgwMessage::Nomint(Box::new(
                NomintMessage::from_segments(segments),
            )))
        }
        #[cfg(not(feature = "nomint"))]
        DvgwMessageType::Nomint => Err(Error::FeatureNotEnabled {
            message_type: "NOMINT".into(),
            feature: "nomint".into(),
        }),
        #[cfg(feature = "nomres")]
        DvgwMessageType::Nomres => {
            use crate::messages::nomres::NomresMessage;
            Ok(AnyDvgwMessage::Nomres(Box::new(
                NomresMessage::from_segments(segments),
            )))
        }
        #[cfg(not(feature = "nomres"))]
        DvgwMessageType::Nomres => Err(Error::FeatureNotEnabled {
            message_type: "NOMRES".into(),
            feature: "nomres".into(),
        }),
        #[cfg(feature = "schedl")]
        DvgwMessageType::Schedl => {
            use crate::messages::schedl::SchedlMessage;
            Ok(AnyDvgwMessage::Schedl(Box::new(
                SchedlMessage::from_segments(segments),
            )))
        }
        #[cfg(not(feature = "schedl"))]
        DvgwMessageType::Schedl => Err(Error::FeatureNotEnabled {
            message_type: "SCHEDL".into(),
            feature: "schedl".into(),
        }),
        #[cfg(feature = "imbnot")]
        DvgwMessageType::Imbnot => {
            use crate::messages::imbnot::ImbalanceMessage;
            Ok(AnyDvgwMessage::Imbnot(Box::new(
                ImbalanceMessage::from_segments(segments),
            )))
        }
        #[cfg(not(feature = "imbnot"))]
        DvgwMessageType::Imbnot => Err(Error::FeatureNotEnabled {
            message_type: "IMBNOT".into(),
            feature: "imbnot".into(),
        }),
        #[cfg(feature = "tranot")]
        DvgwMessageType::Tranot => {
            use crate::messages::tranot::TransportNotificationMessage;
            Ok(AnyDvgwMessage::Tranot(Box::new(
                TransportNotificationMessage::from_segments(segments),
            )))
        }
        #[cfg(not(feature = "tranot"))]
        DvgwMessageType::Tranot => Err(Error::FeatureNotEnabled {
            message_type: "TRANOT".into(),
            feature: "tranot".into(),
        }),
        #[cfg(feature = "delord")]
        DvgwMessageType::Delord => {
            use crate::messages::delord::DeliveryOrderMessage;
            Ok(AnyDvgwMessage::Delord(Box::new(
                DeliveryOrderMessage::from_segments(segments),
            )))
        }
        #[cfg(not(feature = "delord"))]
        DvgwMessageType::Delord => Err(Error::FeatureNotEnabled {
            message_type: "DELORD".into(),
            feature: "delord".into(),
        }),
        #[cfg(feature = "delres")]
        DvgwMessageType::Delres => {
            use crate::messages::delres::DeliveryResponseMessage;
            Ok(AnyDvgwMessage::Delres(Box::new(
                DeliveryResponseMessage::from_segments(segments),
            )))
        }
        #[cfg(not(feature = "delres"))]
        DvgwMessageType::Delres => Err(Error::FeatureNotEnabled {
            message_type: "DELRES".into(),
            feature: "delres".into(),
        }),
        other => Err(Error::FeatureNotEnabled {
            message_type: other.as_str().to_owned(),
            feature: other.required_feature().to_owned(),
        }),
    }
}

/// Build the semantic [`ProfileRulePack`] for the given DVGW message type.
///
/// Returns `Err(Error::FeatureNotEnabled)` when the required Cargo feature
/// is not compiled in, mirroring the `dispatch` behaviour so callers get a
/// consistent error regardless of which method they use.
#[allow(unused_variables)]
fn semantic_pack(msg_type: DvgwMessageType) -> Result<ProfileRulePack, Error> {
    match msg_type {
        #[cfg(feature = "alocat")]
        DvgwMessageType::Alocat => Ok(sem::alocat_pack()),
        #[cfg(not(feature = "alocat"))]
        DvgwMessageType::Alocat => Err(Error::FeatureNotEnabled {
            message_type: "ALOCAT".into(),
            feature: "alocat".into(),
        }),
        #[cfg(feature = "nomint")]
        DvgwMessageType::Nomint => Ok(sem::nomint_pack()),
        #[cfg(not(feature = "nomint"))]
        DvgwMessageType::Nomint => Err(Error::FeatureNotEnabled {
            message_type: "NOMINT".into(),
            feature: "nomint".into(),
        }),
        #[cfg(feature = "nomres")]
        DvgwMessageType::Nomres => Ok(sem::nomres_pack()),
        #[cfg(not(feature = "nomres"))]
        DvgwMessageType::Nomres => Err(Error::FeatureNotEnabled {
            message_type: "NOMRES".into(),
            feature: "nomres".into(),
        }),
        #[cfg(feature = "schedl")]
        DvgwMessageType::Schedl => Ok(sem::schedl_pack()),
        #[cfg(not(feature = "schedl"))]
        DvgwMessageType::Schedl => Err(Error::FeatureNotEnabled {
            message_type: "SCHEDL".into(),
            feature: "schedl".into(),
        }),
        #[cfg(feature = "imbnot")]
        DvgwMessageType::Imbnot => Ok(sem::imbnot_pack()),
        #[cfg(not(feature = "imbnot"))]
        DvgwMessageType::Imbnot => Err(Error::FeatureNotEnabled {
            message_type: "IMBNOT".into(),
            feature: "imbnot".into(),
        }),
        #[cfg(feature = "tranot")]
        DvgwMessageType::Tranot => Ok(sem::tranot_pack()),
        #[cfg(not(feature = "tranot"))]
        DvgwMessageType::Tranot => Err(Error::FeatureNotEnabled {
            message_type: "TRANOT".into(),
            feature: "tranot".into(),
        }),
        #[cfg(feature = "delord")]
        DvgwMessageType::Delord => Ok(sem::delord_pack()),
        #[cfg(not(feature = "delord"))]
        DvgwMessageType::Delord => Err(Error::FeatureNotEnabled {
            message_type: "DELORD".into(),
            feature: "delord".into(),
        }),
        #[cfg(feature = "delres")]
        DvgwMessageType::Delres => Ok(sem::delres_pack()),
        #[cfg(not(feature = "delres"))]
        DvgwMessageType::Delres => Err(Error::FeatureNotEnabled {
            message_type: "DELRES".into(),
            feature: "delres".into(),
        }),
        other => Err(Error::FeatureNotEnabled {
            message_type: other.as_str().to_owned(),
            feature: other.required_feature().to_owned(),
        }),
    }
}
