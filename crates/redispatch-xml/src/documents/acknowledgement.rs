//! `AcknowledgementDocument` — application-level acknowledgement for all Redispatch 2.0 message types.
use serde::{Deserialize, Serialize};

use crate::types::{
    AttrV, AttrVWithScheme, DocumentId, DocumentVersion, MarketParticipantId, MarketRoleType,
    UtcDateTime,
};

// ── DocumentType / ReasonCode ─────────────────────────────────────────────────

/// Document types that an `AcknowledgementDocument` may reference in
/// `ReceivingDocumentType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AckReceivingDocType {
    /// Day-ahead plan / forecast document (A14).
    #[serde(rename = "A14")]
    DayAheadPlan,
    /// Activation response document (A41).
    #[serde(rename = "A41")]
    ActivationResponse,
    /// Tender reduction / ACR document (A42).
    #[serde(rename = "A42")]
    TenderReduction,
    /// Status request document (A60).
    #[serde(rename = "A60")]
    StatusRequest,
    /// Planned unavailability document (A67).
    #[serde(rename = "A67")]
    PlannedUnavailability,
    /// Forced (unplanned) unavailability document (A76).
    #[serde(rename = "A76")]
    ForcedUnavailability,
    /// Production unavailability document (A80).
    #[serde(rename = "A80")]
    ProductionUnavailability,
    /// Redispatch activation / ACO document (A96).
    #[serde(rename = "A96")]
    RedispatchActivation,
    /// Network constraint document (B15).
    #[serde(rename = "B15")]
    NetworkConstraint,
    /// Stammdaten creation document (Z01).
    #[serde(rename = "Z01")]
    StammdatenCreation,
    /// Stammdaten update document (Z02).
    #[serde(rename = "Z02")]
    StammdatenUpdate,
    /// Stammdaten deactivation document (Z03).
    #[serde(rename = "Z03")]
    StammdatenDeactivation,
    /// Grid operator aggregate Stammdaten document (Z04).
    #[serde(rename = "Z04")]
    StammdatenNbAggregate,
    /// Kostenblatt (cost sheet) document (Z05).
    #[serde(rename = "Z05")]
    Kostenblatt,
    /// Intraday plan document (Z08).
    #[serde(rename = "Z08")]
    IntradayPlan,
    /// Stammdaten plan document (Z09).
    #[serde(rename = "Z09")]
    StammdatenPlan,
    /// Aggregate plan document (Z11).
    #[serde(rename = "Z11")]
    AggregatePlan,
    /// Corrected plan document (Z12).
    #[serde(rename = "Z12")]
    CorrectedPlan,
    /// Reserved document type (Z13).
    #[serde(rename = "Z13")]
    Reserved13,
    /// Bilanzkreisstammdaten (balance zone master data) document (Z14).
    #[serde(rename = "Z14")]
    Bilanzkreisstammdaten,
    /// Alternative status request document (Z15).
    #[serde(rename = "Z15")]
    StatusRequestAlt,
    /// Kaskade (cascade) document (Z16).
    #[serde(rename = "Z16")]
    Kaskade,
    /// Test message document (Z17).
    #[serde(rename = "Z17")]
    TestMessage,
}

/// Reason codes used at the `AcknowledgementDocument` root level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AckReasonCode {
    /// Message fully accepted.
    #[serde(rename = "A01")]
    FullyAccepted,
    /// Message fully rejected.
    #[serde(rename = "A02")]
    FullyRejected,
    /// Syntax error detected.
    #[serde(rename = "Z12")]
    SyntaxError,
    /// Assignment error.
    #[serde(rename = "Z13")]
    AssignmentError,
    /// Document identification not unique.
    #[serde(rename = "Z14")]
    DocumentIdNotUnique,
    /// Sender not authorised.
    #[serde(rename = "Z15")]
    SenderUnauthorised,
    /// Not permitted per AWT (Anwendungstabelle).
    #[serde(rename = "Z16")]
    NotPermitted,
    /// Format version invalid.
    #[serde(rename = "Z17")]
    FormatVersionInvalid,
    /// Report period invalid.
    #[serde(rename = "Z18")]
    ReportPeriodInvalid,
}

/// Reason element at the `AcknowledgementDocument` root or `TimeSeriesRejection` level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AckReason {
    /// Reason code.
    #[serde(rename = "ReasonCode")]
    pub code: AttrV<AckReasonCode>,
    /// Optional free-text description.
    #[serde(
        rename = "ReasonText",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub text: Option<String>,
}

// ── TimeSeriesRejection ───────────────────────────────────────────────────────

/// Rejection detail for a single time series within the received document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeSeriesRejection {
    /// The `TimeSeriesIdentification` or `AllocationIdentification` of the
    /// rejected time series.
    #[serde(
        rename = "TimeSeriesIdentification",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub time_series_identification: Option<AttrV<DocumentId>>,
    /// Reasons for the rejection.
    #[serde(rename = "Reason", default)]
    pub reasons: Vec<AckReason>,
}

// ── AcknowledgementDocument ───────────────────────────────────────────────────

/// `AcknowledgementDocument` — application-level acknowledgement for all
/// Redispatch 2.0 document types.
///
/// XSD version: 1.0g (2025-10-01)  
/// No XML namespace.
///
/// An `AcknowledgementDocument` is sent in response to any received Redispatch
/// 2.0 document. The root-level `Reason` list indicates overall acceptance
/// (`A01`) or rejection (`A02`). Per-time-series details appear in
/// `time_series_rejections`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "AcknowledgementDocument")]
pub struct AcknowledgementDocument {
    /// Unique document identifier (max 35 chars).
    #[serde(rename = "DocumentIdentification")]
    pub document_identification: AttrV<DocumentId>,
    /// Document creation timestamp (UTC, second precision).
    #[serde(rename = "DocumentDateTime")]
    pub document_date_time: AttrV<UtcDateTime>,
    /// Sender's market participant identifier.
    #[serde(rename = "SenderIdentification")]
    pub sender_identification: AttrVWithScheme<MarketParticipantId>,
    /// Sender's market role.
    #[serde(rename = "SenderRole")]
    pub sender_role: AttrV<MarketRoleType>,
    /// Receiver's market participant identifier.
    #[serde(rename = "ReceiverIdentification")]
    pub receiver_identification: AttrVWithScheme<MarketParticipantId>,
    /// Receiver's market role.
    #[serde(rename = "ReceiverRole")]
    pub receiver_role: AttrV<MarketRoleType>,
    /// `DocumentIdentification` of the acknowledged document (optional).
    #[serde(
        rename = "ReceivingDocumentIdentification",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub receiving_document_identification: Option<AttrV<DocumentId>>,
    /// `DocumentVersion` of the acknowledged document (optional).
    #[serde(
        rename = "ReceivingDocumentVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub receiving_document_version: Option<AttrV<DocumentVersion>>,
    /// `DocumentType` of the acknowledged document (optional).
    #[serde(
        rename = "ReceivingDocumentType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub receiving_document_type: Option<AttrV<AckReceivingDocType>>,
    /// Original filename of the received AS4 payload (optional).
    #[serde(
        rename = "ReceivingPayloadName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub receiving_payload_name: Option<AttrV<String>>,
    /// `DocumentDateTime` / `CreationDateTime` of the acknowledged document (optional).
    #[serde(
        rename = "DateTimeReceivingDocument",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub date_time_receiving_document: Option<AttrV<UtcDateTime>>,
    /// Per-time-series rejection details (optional).
    #[serde(
        rename = "TimeSeriesRejection",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub time_series_rejections: Vec<TimeSeriesRejection>,
    /// Document-level acceptance / rejection reasons (required, at least 1).
    #[serde(rename = "Reason")]
    pub reasons: Vec<AckReason>,
}
