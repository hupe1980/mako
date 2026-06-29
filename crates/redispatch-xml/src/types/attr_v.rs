use std::fmt;

use serde::{Deserialize, Serialize};

/// Generic wrapper for the ENTSO-E/BDEW attr-v pattern:
///
/// ```xml
/// <ElementName v="value"/>
/// ```
///
/// The `v` attribute holds the actual value. This pattern appears in all
/// ENTSO-E-derived and most BDEW XML documents (ActivationDocument,
/// PlannedResourceScheduleDocument, AcknowledgementDocument,
/// NetworkConstraintDocument, Kostenblatt).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttrV<T> {
    /// The element value, stored in the `v` XML attribute.
    #[serde(rename = "@v")]
    pub v: T,
}

impl<T> AttrV<T> {
    /// Create a new `AttrV` wrapper.
    pub fn new(v: T) -> Self {
        Self { v }
    }
}

impl<T: Clone> AttrV<T> {
    /// Unwrap and clone the inner value.
    pub fn value(&self) -> T {
        self.v.clone()
    }
}

impl<T> From<T> for AttrV<T> {
    fn from(v: T) -> Self {
        Self { v }
    }
}

impl<T: fmt::Display> fmt::Display for AttrV<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.v.fmt(f)
    }
}

/// Wrapper for elements whose value is in a `v` attribute **and** that have
/// an additional `codingScheme` attribute:
///
/// ```xml
/// <SenderIdentification v="4045399000008" codingScheme="A10"/>
/// ```
///
/// Used for market participant IDs (`SenderIdentification`,
/// `ReceiverIdentification`, `ResourceProvider`) and control zone references
/// (`ConnectingArea`, `AcquiringArea`) in the attr-v document family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttrVWithScheme<T, S = crate::types::CodingScheme> {
    /// The element value.
    #[serde(rename = "@v")]
    pub v: T,
    /// The coding scheme qualifier.
    #[serde(rename = "@codingScheme")]
    pub coding_scheme: S,
}

impl<T, S> AttrVWithScheme<T, S> {
    /// Create a new `AttrVWithScheme` wrapper.
    pub fn new(v: T, coding_scheme: S) -> Self {
        Self { v, coding_scheme }
    }
}

/// Wrapper for IEC 62325 simpleContent elements: text content with an
/// additional `codingScheme` attribute:
///
/// ```xml
/// <mRID codingScheme="A10">4045399000008</mRID>
/// ```
///
/// Used in Kaskade, Unavailability_MarketDocument, and
/// StatusRequest_MarketDocument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimpleContent<T, S = crate::types::CodingScheme> {
    /// Text content of the element.
    #[serde(rename = "$text")]
    pub value: T,
    /// Coding scheme qualifier attribute.
    #[serde(rename = "@codingScheme")]
    pub coding_scheme: S,
}

impl<T, S> SimpleContent<T, S> {
    /// Create a new `SimpleContent` wrapper.
    pub fn new(value: T, coding_scheme: S) -> Self {
        Self {
            value,
            coding_scheme,
        }
    }
}
