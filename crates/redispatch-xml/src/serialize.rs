//! Serialization helpers for Redispatch 2.0 XML documents.

use serde::Serialize;

use crate::error::RedispatchXmlError;
use crate::parse::Document;

// ── Public API ────────────────────────────────────────────────────────────────

/// Serialise a [`Document`] to an XML byte vector.
///
/// An `<?xml version="1.0" encoding="UTF-8"?>` declaration is prepended
/// automatically.
///
/// # Errors
///
/// Returns [`RedispatchXmlError::Xml`] on serialization failure.
pub fn serialize(doc: &Document) -> Result<Vec<u8>, RedispatchXmlError> {
    match doc {
        Document::Activation(d) => serialize_as(d.as_ref(), true),
        Document::PlannedResourceSchedule(d) => serialize_as(d.as_ref(), true),
        Document::Acknowledgement(d) => serialize_as(d.as_ref(), true),
        Document::Stammdaten(d) => serialize_as(d.as_ref(), true),
        Document::StatusRequest(d) => serialize_as(d.as_ref(), true),
        Document::Unavailability(d) => serialize_as(d.as_ref(), true),
        Document::Kaskade(d) => serialize_as(d.as_ref(), true),
        Document::NetworkConstraint(d) => serialize_as(d.as_ref(), true),
        Document::Kostenblatt(d) => serialize_as(d.as_ref(), true),
    }
}

/// Serialise a specific document type `T` to an XML byte vector.
///
/// # Errors
///
/// Returns [`RedispatchXmlError::Xml`] on serialization failure.
pub fn serialize_as<T: Serialize>(
    doc: &T,
    add_xml_decl: bool,
) -> Result<Vec<u8>, RedispatchXmlError> {
    let xml_str =
        quick_xml::se::to_string(doc).map_err(|e| RedispatchXmlError::Serialize(e.to_string()))?;
    let mut out = Vec::with_capacity(xml_str.len() + 40);
    if add_xml_decl {
        out.extend_from_slice(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    }
    out.extend_from_slice(xml_str.as_bytes());
    Ok(out)
}
