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
    let doc_type = doc.document_type();
    let bytes = match doc {
        Document::Activation(d) => serialize_as(d.as_ref(), true)?,
        Document::PlannedResourceSchedule(d) => serialize_as(d.as_ref(), true)?,
        Document::Acknowledgement(d) => serialize_as(d.as_ref(), true)?,
        Document::Stammdaten(d) => serialize_as(d.as_ref(), true)?,
        Document::StatusRequest(d) => serialize_as(d.as_ref(), true)?,
        Document::Unavailability(d) => serialize_as(d.as_ref(), true)?,
        Document::Kaskade(d) => serialize_as(d.as_ref(), true)?,
        Document::NetworkConstraint(d) => serialize_as(d.as_ref(), true)?,
        Document::Kostenblatt(d) => serialize_as(d.as_ref(), true)?,
    };
    Ok(inject_default_namespace(bytes, doc_type))
}

/// Insert the document type's default `xmlns` into the root element.
///
/// `quick-xml`'s serde serializer cannot emit namespace declarations, so a
/// serialized document previously failed the namespace check of [`parse`] —
/// round-trips only worked through the unchecked `parse_as` path. Injecting
/// the XSD's default namespace makes `parse(serialize(d))` genuinely
/// lossless. Documents without an XSD namespace pass through unchanged.
///
/// [`parse`]: crate::parse::parse
fn inject_default_namespace(bytes: Vec<u8>, doc_type: crate::documents::DocumentType) -> Vec<u8> {
    let Some(ns) = doc_type.expected_namespace() else {
        return bytes;
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Vec::new(); // unreachable: serializer produced valid UTF-8
    };
    // Root element = first '<' after the XML declaration.
    let Some(root_start) = text
        .find("?>")
        .map(|i| i + 2)
        .and_then(|from| text[from..].find('<').map(|j| from + j))
    else {
        return text.into_bytes();
    };
    if text[root_start..].contains("xmlns=")
        && text[root_start
            ..text[root_start..]
                .find('>')
                .map_or(text.len(), |k| root_start + k)]
            .contains("xmlns=")
    {
        return text.into_bytes(); // already namespaced
    }
    let Some(rel_end) = text[root_start..].find(['>', ' ']) else {
        return text.into_bytes();
    };
    let insert_at = root_start + rel_end;
    let mut out = String::with_capacity(text.len() + ns.len() + 10);
    out.push_str(&text[..insert_at]);
    out.push_str(&format!(" xmlns=\"{ns}\""));
    out.push_str(&text[insert_at..]);
    out.into_bytes()
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
