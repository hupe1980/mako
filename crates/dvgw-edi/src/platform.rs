use edifact_rs::{OwnedSegment, ReaderConfig, from_bytes_owned_with_config};

use crate::{
    AnyDvgwMessage, DvgwMessageType, Error, error::sanitize_code,
    message::extract_message_type_code,
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
}

/// Dispatch parsed segments to the concrete message constructor.
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
        other => Err(Error::FeatureNotEnabled {
            message_type: other.as_str().to_owned(),
            feature: other.required_feature().to_owned(),
        }),
    }
}
