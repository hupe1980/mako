//! Error types for Redispatch 2.0 XML.

/// All errors that can occur when parsing, validating, or serializing
/// Redispatch 2.0 XML documents.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RedispatchXmlError {
    /// Low-level XML tokenization or encoding error from `quick-xml`.
    #[error("XML error: {0}")]
    Xml(#[from] quick_xml::Error),

    /// Serde-based deserialization error.
    #[error("deserialization error: {0}")]
    Deserialize(#[from] quick_xml::de::DeError),

    /// Serde-based serialization error.
    #[error("serialization error: {0}")]
    Serialize(String),

    /// A document identifier violates the XSD constraint (1–35 characters).
    #[error("invalid document id {0:?}: must be 1–35 characters")]
    InvalidDocumentId(String),

    /// A document version number violates the XSD constraint (integer 1–999).
    #[error("invalid document version {0}: must be 1–999")]
    InvalidDocumentVersion(u32),

    /// A UTC timestamp is malformed or uses a non-UTC offset.
    ///
    /// All BDEW Redispatch 2.0 timestamps must end with `Z` (UTC).
    #[error("invalid UTC timestamp {0:?}: must match yyyy-mm-ddThh:mm:ssZ")]
    InvalidTimestamp(String),

    /// A market participant ID violates the XSD pattern (exactly 13 decimal digits).
    #[error("invalid market participant id {0:?}: must be exactly 13 decimal digits")]
    InvalidMarketParticipantId(String),

    /// A time interval string is malformed.
    ///
    /// BDEW intervals use minute-precision UTC: `yyyy-mm-ddThh:mmZ/yyyy-mm-ddThh:mmZ`.
    #[error("invalid time interval {0:?}: must be yyyy-mm-ddThh:mmZ/yyyy-mm-ddThh:mmZ")]
    InvalidTimeInterval(String),

    /// The root element is not a recognised Redispatch 2.0 document type.
    #[error("unknown document root element {0:?}: not a supported Redispatch 2.0 document")]
    UnknownDocumentType(String),

    /// The XML namespace URI on the root element does not match the expected value.
    #[error("namespace mismatch: expected {expected}, found {found}")]
    NamespaceMismatch {
        /// Expected XML namespace URI for this document type.
        expected: &'static str,
        /// Actual namespace URI found on the root element.
        found: String,
    },

    /// An XSD structural constraint (maxLength, pattern, range, enumeration) was
    /// violated during explicit validation.
    #[error("structural validation: {0}")]
    StructuralError(String),
}
