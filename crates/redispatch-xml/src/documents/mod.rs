//! Concrete document types for all nine Redispatch 2.0 document families.

pub mod acknowledgement;
pub mod activation;
pub mod kaskade;
pub mod kostenblatt;
pub mod network_constraint;
pub mod planned_resource_schedule;
pub mod stammdaten;
pub mod status_request;
pub mod unavailability;

pub use acknowledgement::AcknowledgementDocument;
pub use activation::ActivationDocument;
pub use kaskade::Kaskade;
pub use kostenblatt::Kostenblatt;
pub use network_constraint::NetworkConstraintDocument;
pub use planned_resource_schedule::PlannedResourceScheduleDocument;
pub use stammdaten::Stammdaten;
pub use status_request::StatusRequestMarketDocument;
pub use unavailability::UnavailabilityMarketDocument;

/// The set of all supported Redispatch 2.0 document types.
///
/// Used by [`crate::detect`] to identify the type of a document
/// from its root element name, and as the variant discriminant for
/// [`crate::Document`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocumentType {
    /// `ActivationDocument` (ACO/ACR/AAR).
    Activation,
    /// `PlannedResourceScheduleDocument`.
    PlannedResourceSchedule,
    /// `AcknowledgementDocument`.
    Acknowledgement,
    /// `Stammdaten`.
    Stammdaten,
    /// `StatusRequest_MarketDocument`.
    StatusRequest,
    /// `Unavailability_MarketDocument`.
    Unavailability,
    /// `Kaskade`.
    Kaskade,
    /// `NetworkConstraintDocument`.
    NetworkConstraint,
    /// `Kostenblatt`.
    Kostenblatt,
}

impl DocumentType {
    /// Return the XML root element name for this document type.
    pub fn root_element_name(self) -> &'static str {
        match self {
            Self::Activation => "ActivationDocument",
            Self::PlannedResourceSchedule => "PlannedResourceScheduleDocument",
            Self::Acknowledgement => "AcknowledgementDocument",
            Self::Stammdaten => "Stammdaten",
            Self::StatusRequest => "StatusRequest_MarketDocument",
            Self::Unavailability => "Unavailability_MarketDocument",
            Self::Kaskade => "Kaskade",
            Self::NetworkConstraint => "NetworkConstraintDocument",
            Self::Kostenblatt => "Kostenblatt",
        }
    }

    /// Return the expected XML namespace URI for this document type, if any.
    pub fn expected_namespace(self) -> Option<&'static str> {
        match self {
            Self::Activation => Some(activation::NAMESPACE),
            Self::StatusRequest => Some(status_request::NAMESPACE),
            Self::Unavailability | Self::Kaskade => Some(kaskade::NAMESPACE),
            Self::Stammdaten => Some("urn:kwep_stammdaten:1:0"),
            _ => None,
        }
    }

    /// Attempt to identify the document type from the root element local name.
    ///
    /// Returns `None` if the name is not a recognised Redispatch 2.0 root
    /// element.
    pub fn from_root_element(name: &str) -> Option<Self> {
        match name {
            "ActivationDocument" => Some(Self::Activation),
            "PlannedResourceScheduleDocument" => Some(Self::PlannedResourceSchedule),
            "AcknowledgementDocument" => Some(Self::Acknowledgement),
            "Stammdaten" => Some(Self::Stammdaten),
            "StatusRequest_MarketDocument" => Some(Self::StatusRequest),
            "Unavailability_MarketDocument" => Some(Self::Unavailability),
            "Kaskade" => Some(Self::Kaskade),
            "NetworkConstraintDocument" => Some(Self::NetworkConstraint),
            "Kostenblatt" => Some(Self::Kostenblatt),
            _ => None,
        }
    }
}
