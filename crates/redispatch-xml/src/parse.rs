//! Two-phase XML parsing pipeline for Redispatch 2.0 documents.
//!
//! ## Pipeline
//!
//! 1. **Detect** — scan the opening bytes of the input to identify the root
//!    element name and, where present, the `xmlns` namespace.
//! 2. **Deserialize** — pass the full input to [`quick_xml::de::from_str`].
//! 3. **Validate namespace** — for document types that carry a `targetNamespace`,
//!    confirm the detected namespace matches the expected value.
//!
//! No libxml2 / XSD validation is performed at parse time. Use the
//! [`crate::validation`] module for post-parse semantic/structural checks.

use crate::documents::{
    self, AcknowledgementDocument, ActivationDocument, DocumentType, Kaskade, Kostenblatt,
    NetworkConstraintDocument, PlannedResourceScheduleDocument, Stammdaten,
    StatusRequestMarketDocument, UnavailabilityMarketDocument,
};
use crate::error::RedispatchXmlError;

// ── Document sum type ─────────────────────────────────────────────────────────

/// A parsed Redispatch 2.0 document (any of the nine supported types).
#[derive(Debug, Clone, PartialEq)]
pub enum Document {
    /// Activation document (`ActivationDocument`): ACO, ACR, or AAR.
    Activation(Box<ActivationDocument>),
    /// Planned resource schedule document (`PlannedResourceScheduleDocument`).
    PlannedResourceSchedule(Box<PlannedResourceScheduleDocument>),
    /// Acknowledgement document (`AcknowledgementDocument`).
    Acknowledgement(Box<AcknowledgementDocument>),
    /// Stammdaten (master data) document.
    Stammdaten(Box<Stammdaten>),
    /// Status request market document (`StatusRequest_MarketDocument`).
    StatusRequest(Box<StatusRequestMarketDocument>),
    /// Unavailability market document (`Unavailability_MarketDocument`).
    Unavailability(Box<UnavailabilityMarketDocument>),
    /// Kaskade (cascade) document.
    Kaskade(Box<Kaskade>),
    /// Network constraint document (`NetworkConstraintDocument`).
    NetworkConstraint(Box<NetworkConstraintDocument>),
    /// Kostenblatt (cost sheet) document.
    Kostenblatt(Box<Kostenblatt>),
}

impl Document {
    /// Return the [`DocumentType`] variant for this document.
    pub fn document_type(&self) -> DocumentType {
        match self {
            Self::Activation(_) => DocumentType::Activation,
            Self::PlannedResourceSchedule(_) => DocumentType::PlannedResourceSchedule,
            Self::Acknowledgement(_) => DocumentType::Acknowledgement,
            Self::Stammdaten(_) => DocumentType::Stammdaten,
            Self::StatusRequest(_) => DocumentType::StatusRequest,
            Self::Unavailability(_) => DocumentType::Unavailability,
            Self::Kaskade(_) => DocumentType::Kaskade,
            Self::NetworkConstraint(_) => DocumentType::NetworkConstraint,
            Self::Kostenblatt(_) => DocumentType::Kostenblatt,
        }
    }

    /// Return the document's primary identifier (mRID or `DocumentIdentification`).
    ///
    /// This is the correlation key used by the process engine to route inbound
    /// documents to the correct workflow instance.
    pub fn mrid(&self) -> &str {
        match self {
            Self::Activation(d) => d.document_identification.v.as_str(),
            Self::PlannedResourceSchedule(d) => d.document_identification.v.as_str(),
            Self::Acknowledgement(d) => d.document_identification.v.as_str(),
            Self::Stammdaten(d) => d.document_identification.as_str(),
            Self::StatusRequest(d) => d.m_rid.as_str(),
            Self::Unavailability(d) => d.m_rid.as_str(),
            Self::Kaskade(d) => d.m_rid.as_str(),
            Self::NetworkConstraint(d) => d.document_identification.v.as_str(),
            Self::Kostenblatt(d) => d.document_identification.v.as_str(),
        }
    }

    /// Return the 13-digit GLN / EIC of the document sender.
    pub fn sender_id(&self) -> &str {
        match self {
            Self::Activation(d) => d.sender_identification.v.as_str(),
            Self::PlannedResourceSchedule(d) => d.sender_identification.v.as_str(),
            Self::Acknowledgement(d) => d.sender_identification.v.as_str(),
            Self::Stammdaten(d) => d.sender.code.as_str(),
            Self::StatusRequest(d) => d.sender_market_participant.m_rid.value.as_str(),
            Self::Unavailability(d) => d.sender_market_participant.m_rid.value.as_str(),
            Self::Kaskade(d) => d.sender_market_participant.m_rid.value.as_str(),
            Self::NetworkConstraint(d) => d.sender_identification.v.as_str(),
            Self::Kostenblatt(d) => d.sender_identification.v.as_str(),
        }
    }

    /// Return the 13-digit GLN / EIC of the document receiver.
    pub fn receiver_id(&self) -> &str {
        match self {
            Self::Activation(d) => d.receiver_identification.v.as_str(),
            Self::PlannedResourceSchedule(d) => d.receiver_identification.v.as_str(),
            Self::Acknowledgement(d) => d.receiver_identification.v.as_str(),
            Self::Stammdaten(d) => d.empfaenger.code.as_str(),
            Self::StatusRequest(d) => d.receiver_market_participant.m_rid.value.as_str(),
            Self::Unavailability(d) => d.receiver_market_participant.m_rid.value.as_str(),
            Self::Kaskade(d) => d.receiver_market_participant.m_rid.value.as_str(),
            Self::NetworkConstraint(d) => d.receiver_identification.v.as_str(),
            Self::Kostenblatt(d) => d.receiver_identification.v.as_str(),
        }
    }
}

// ── From<T> for Document ──────────────────────────────────────────────────────

impl From<ActivationDocument> for Document {
    fn from(d: ActivationDocument) -> Self {
        Self::Activation(Box::new(d))
    }
}
impl From<PlannedResourceScheduleDocument> for Document {
    fn from(d: PlannedResourceScheduleDocument) -> Self {
        Self::PlannedResourceSchedule(Box::new(d))
    }
}
impl From<AcknowledgementDocument> for Document {
    fn from(d: AcknowledgementDocument) -> Self {
        Self::Acknowledgement(Box::new(d))
    }
}
impl From<Stammdaten> for Document {
    fn from(d: Stammdaten) -> Self {
        Self::Stammdaten(Box::new(d))
    }
}
impl From<StatusRequestMarketDocument> for Document {
    fn from(d: StatusRequestMarketDocument) -> Self {
        Self::StatusRequest(Box::new(d))
    }
}
impl From<UnavailabilityMarketDocument> for Document {
    fn from(d: UnavailabilityMarketDocument) -> Self {
        Self::Unavailability(Box::new(d))
    }
}
impl From<Kaskade> for Document {
    fn from(d: Kaskade) -> Self {
        Self::Kaskade(Box::new(d))
    }
}
impl From<NetworkConstraintDocument> for Document {
    fn from(d: NetworkConstraintDocument) -> Self {
        Self::NetworkConstraint(Box::new(d))
    }
}
impl From<documents::Kostenblatt> for Document {
    fn from(d: documents::Kostenblatt) -> Self {
        Self::Kostenblatt(Box::new(d))
    }
}

// ── Detection ─────────────────────────────────────────────────────────────────

/// Scan the first 4 KiB of `xml` for the first element start tag and optional
/// `xmlns` attribute, returning `(root_element_local_name, Option<namespace>)`.
///
/// This is intentionally a lightweight byte scan — not a full XML parse — so
/// that detection is fast even for large documents.
fn detect_root(xml: &[u8]) -> (String, Option<String>) {
    // Strip UTF-8 BOM (U+FEFF, encoded as EF BB BF) if present.
    let xml = xml.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(xml);

    // Work with only the first 4096 bytes.
    let window = &xml[..xml.len().min(4096)];
    let text = String::from_utf8_lossy(window);

    // Find the first '<' that is not '<?' or '<!'.
    let mut root_name = String::new();
    let mut namespace = None;

    for i in 0..text.len() {
        let ch = text.as_bytes()[i];
        if ch != b'<' {
            continue;
        }
        let rest = &text[i + 1..];
        if rest.starts_with('?') || rest.starts_with('!') {
            continue;
        }
        // Extract the local name (up to first space, '>' or '/').
        let name_end = rest
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(rest.len());
        let raw_name = &rest[..name_end];
        // Strip namespace prefix if present.
        root_name = if let Some(pos) = raw_name.rfind(':') {
            raw_name[pos + 1..].to_string()
        } else {
            raw_name.to_string()
        };

        // Scan the opening tag for xmlns="..." or xmlns:xxx="...".
        let tag_end = rest.find('>').unwrap_or(rest.len());
        let tag_slice = &rest[..tag_end];
        namespace = extract_default_namespace(tag_slice);
        break;
    }

    (root_name, namespace)
}

/// Extract the value of the first `xmlns="..."` or `xmlns:xxx="..."` attribute
/// from a raw tag fragment.
fn extract_default_namespace(tag: &str) -> Option<String> {
    // Look for xmlns="..." (default namespace).
    if let Some(pos) = tag.find("xmlns=\"") {
        let after = &tag[pos + 7..];
        if let Some(end) = after.find('"') {
            return Some(after[..end].to_string());
        }
    }
    // Fall back to xmlns:xxx="..." (prefixed namespace — first occurrence).
    if let Some(pos) = tag.find("xmlns:") {
        let after = &tag[pos..];
        if let Some(eq) = after.find("=\"") {
            let ns_part = &after[eq + 2..];
            if let Some(end) = ns_part.find('"') {
                return Some(ns_part[..end].to_string());
            }
        }
    }
    None
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Detect the document type of a Redispatch 2.0 XML message without fully
/// deserializing it.
///
/// # Errors
///
/// Returns [`RedispatchXmlError::UnknownDocumentType`] if the root element is
/// not a recognised Redispatch 2.0 document type.
pub fn detect(xml: &[u8]) -> Result<DocumentType, RedispatchXmlError> {
    let (root_name, _) = detect_root(xml);
    DocumentType::from_root_element(&root_name)
        .ok_or(RedispatchXmlError::UnknownDocumentType(root_name))
}

/// Deserialise a Redispatch 2.0 XML document into the appropriate [`Document`]
/// variant.
///
/// The document type is detected automatically from the root element.
///
/// # Errors
///
/// - [`RedispatchXmlError::UnknownDocumentType`] — unrecognised root element.
/// - [`RedispatchXmlError::Deserialize`] — XML deserialization failure.
/// - [`RedispatchXmlError::NamespaceMismatch`] — wrong or missing namespace.
pub fn parse(xml: &[u8]) -> Result<Document, RedispatchXmlError> {
    let (root_name, detected_ns) = detect_root(xml);
    let doc_type = DocumentType::from_root_element(&root_name)
        .ok_or(RedispatchXmlError::UnknownDocumentType(root_name))?;

    // Validate namespace where required.
    if let Some(expected_ns) = doc_type.expected_namespace() {
        match detected_ns.as_deref() {
            Some(found) if found == expected_ns => {}
            Some(found) => {
                return Err(RedispatchXmlError::NamespaceMismatch {
                    expected: expected_ns,
                    found: found.to_string(),
                });
            }
            None => {
                return Err(RedispatchXmlError::NamespaceMismatch {
                    expected: expected_ns,
                    found: String::new(),
                });
            }
        }
    }

    let text =
        std::str::from_utf8(xml).map_err(|e| RedispatchXmlError::StructuralError(e.to_string()))?;

    match doc_type {
        DocumentType::Activation => {
            let doc: ActivationDocument =
                quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)?;
            Ok(Document::Activation(Box::new(doc)))
        }
        DocumentType::PlannedResourceSchedule => {
            let doc: PlannedResourceScheduleDocument =
                quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)?;
            Ok(Document::PlannedResourceSchedule(Box::new(doc)))
        }
        DocumentType::Acknowledgement => {
            let doc: AcknowledgementDocument =
                quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)?;
            Ok(Document::Acknowledgement(Box::new(doc)))
        }
        DocumentType::Stammdaten => {
            let doc: Stammdaten =
                quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)?;
            Ok(Document::Stammdaten(Box::new(doc)))
        }
        DocumentType::StatusRequest => {
            let doc: StatusRequestMarketDocument =
                quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)?;
            Ok(Document::StatusRequest(Box::new(doc)))
        }
        DocumentType::Unavailability => {
            let doc: UnavailabilityMarketDocument =
                quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)?;
            Ok(Document::Unavailability(Box::new(doc)))
        }
        DocumentType::Kaskade => {
            let doc: Kaskade =
                quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)?;
            Ok(Document::Kaskade(Box::new(doc)))
        }
        DocumentType::NetworkConstraint => {
            let doc: NetworkConstraintDocument =
                quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)?;
            Ok(Document::NetworkConstraint(Box::new(doc)))
        }
        DocumentType::Kostenblatt => {
            let doc: documents::Kostenblatt =
                quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)?;
            Ok(Document::Kostenblatt(Box::new(doc)))
        }
    }
}

/// Deserialise a Redispatch 2.0 XML document into a specific type `T`.
///
/// Use this when the document type is known at compile time.
///
/// # Errors
///
/// Returns [`RedispatchXmlError::Deserialize`] on parse failure.
pub fn parse_as<T>(xml: &[u8]) -> Result<T, RedispatchXmlError>
where
    T: serde::de::DeserializeOwned,
{
    let text =
        std::str::from_utf8(xml).map_err(|e| RedispatchXmlError::StructuralError(e.to_string()))?;
    quick_xml::de::from_str(text).map_err(RedispatchXmlError::Deserialize)
}

/// Parse a Redispatch 2.0 XML document **and** run structural + semantic
/// validation in one step.
///
/// Equivalent to calling [`parse`] followed by [`crate::validate`], but more
/// ergonomic when you always want validation.
///
/// # Errors
///
/// Returns the first [`RedispatchXmlError`] encountered during parsing.
/// If parsing succeeds but validation finds errors, returns the first
/// [`RedispatchXmlError::StructuralError`].
pub fn parse_and_validate(xml: &[u8]) -> Result<Document, RedispatchXmlError> {
    let doc = parse(xml)?;
    let result = crate::validation::validate(&doc);
    result
        .into_result()
        .map(|_| doc)
        .map_err(|e| RedispatchXmlError::StructuralError(e.to_string()))
}
