//! `ActivationDocument` -- redispatch activation orders (ACO), confirmations (ACR), and activation adjustments (AAR).
use serde::{Deserialize, Serialize};

use crate::types::{
    AttrV, AttrVWithScheme, ControlZone, Direction, DocumentId, DocumentVersion,
    MarketParticipantId, MarketRoleType, MeasureUnit, Period, TimeInterval, UtcDateTime,
};

// ── Namespace ─────────────────────────────────────────────────────────────────

/// Expected XML namespace for `ActivationDocument`.
pub const NAMESPACE: &str = "urn:entsoe.eu:wgedi:errp:activationdocument:5:0";

// ── DocumentType ─────────────────────────────────────────────────────────────

/// `DocumentType` codes for `ActivationDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActivationDocType {
    /// Activation response (ACR).
    #[serde(rename = "A41")]
    ActivationResponse,
    /// Tender reduction (AAR).
    #[serde(rename = "A42")]
    TenderReduction,
    /// Redispatch activation document (ACO).
    #[serde(rename = "A96")]
    RedispatchActivation,
}

/// `ProcessType` codes for `ActivationDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActivationProcessType {
    /// Redispatch activation process.
    #[serde(rename = "A41")]
    Redispatch,
    /// Test / other.
    #[serde(rename = "Z01")]
    Other,
}

/// `Status` codes used in `ActivationTimeSeries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeSeriesStatus {
    /// Volumes available (used for partial-rejection ACR).
    #[serde(rename = "A06")]
    Available,
    /// Quantities activated (Information).
    #[serde(rename = "A07")]
    Activated,
    /// Quantities ordered (Anweisung / ACO).
    #[serde(rename = "A10")]
    Ordered,
}

/// `BusinessType` codes used in `ActivationTimeSeries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeSeriesBusinessType {
    /// System Operator redispatching (Deltaanweisung).
    #[serde(rename = "A46")]
    SystemOperatorRedispatching,
    /// Internal redispatch (Sollwertvorgabe).
    #[serde(rename = "A85")]
    InternalRedispatch,
}

// ── Document-level Reason ─────────────────────────────────────────────────────

/// Reason code at the `ActivationDocument` root level (document-level
/// acceptance/rejection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocumentReasonCode {
    /// Deadline exceeded (Frist abgelaufen).
    #[serde(rename = "A57")]
    DeadlineExceeded,
    /// Complementary information.
    #[serde(rename = "A95")]
    ComplementaryInfo,
    /// Technical constraint.
    #[serde(rename = "A96")]
    TechnicalConstraint,
}

/// Root-level reason attached to an `ActivationDocument`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentReason {
    /// Document-level reason code.
    #[serde(rename = "ReasonCode")]
    pub code: AttrV<DocumentReasonCode>,
    /// Optional free-text description.
    #[serde(
        rename = "ReasonText",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub text: Option<String>,
}

// ── ActivationTimeSeries ──────────────────────────────────────────────────────

/// Reference to a `ResourceObject` with its NDE coding scheme.
pub type ResourceObjectRef = AttrVWithScheme<String, ResourceObjectCodingScheme>;

/// Coding scheme for resource object identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceObjectCodingScheme {
    /// Germany National coding scheme (BDEW resource code).
    #[serde(rename = "NDE")]
    Nde,
}

/// Reference to a control zone (EIC) with `codingScheme = "A01"`.
pub type ControlZoneRef = AttrVWithScheme<ControlZone, EicCodingScheme>;

/// Coding scheme for EIC-coded identifiers (always `A01`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EicCodingScheme {
    /// EIC — Energy Identification Coding Scheme.
    #[serde(rename = "A01")]
    Eic,
}

/// A single activated time series within an `ActivationDocument`.
///
/// Each `ActivationTimeSeries` covers one direction (up/down) for one
/// `ResourceObject`. An `ActivationDocument` may contain up to two time
/// series (one per direction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivationTimeSeries {
    /// Unique time-series identifier within this document.
    #[serde(rename = "AllocationIdentification")]
    pub allocation_identification: AttrV<DocumentId>,
    /// Resource provider (EIV or NB) — optional when sender is the provider.
    #[serde(
        rename = "ResourceProvider",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub resource_provider: Option<AttrVWithScheme<MarketParticipantId>>,
    /// Business type: delta instruction (`A46`) or setpoint (`A85`).
    #[serde(rename = "BusinessType")]
    pub business_type: AttrV<TimeSeriesBusinessType>,
    /// German TSO control block (always `10YCB-GERMANY--8`, EIC).
    #[serde(rename = "AcquiringArea")]
    pub acquiring_area: AttrVWithScheme<String, EicCodingScheme>,
    /// Connecting control zone where the resource object is connected.
    #[serde(rename = "ConnectingArea")]
    pub connecting_area: ControlZoneRef,
    /// Physical unit of the quantity values (`MAW` or `P1`).
    #[serde(rename = "MeasureUnit")]
    pub measure_unit: AttrV<MeasureUnit>,
    /// Redispatch direction: up (`A01`) or down (`A02`).
    #[serde(rename = "Direction")]
    pub direction: AttrV<Direction>,
    /// Activation / order / availability status.
    #[serde(rename = "Status")]
    pub status: AttrV<TimeSeriesStatus>,
    /// Resource object identifier (BDEW resource code, NDE scheme).
    #[serde(rename = "ResourceObject")]
    pub resource_object: ResourceObjectRef,
    /// `DocumentIdentification` of the originating planning data (optional).
    #[serde(
        rename = "SendersDocumentIdentification",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub senders_document_identification: Option<AttrV<DocumentId>>,
    /// `DocumentVersion` of the originating planning data (optional).
    #[serde(
        rename = "SendersDocumentVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub senders_document_version: Option<AttrV<DocumentVersion>>,
    /// `CreationDateTime` of the originating planning data (optional).
    #[serde(
        rename = "SendersDocumentDateTime",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub senders_document_date_time: Option<AttrV<UtcDateTime>>,
    /// Original `TimeSeriesIdentification` (not used in practice).
    #[serde(
        rename = "SendersTimeSeriesIdentification",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub senders_time_series_identification: Option<AttrV<DocumentId>>,
    /// Original sender's market participant ID when forwarded via data provider.
    #[serde(
        rename = "OriginalSenderIdentification",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_sender_identification: Option<AttrVWithScheme<MarketParticipantId>>,
    /// Original `DocumentIdentification` when forwarded.
    #[serde(
        rename = "OriginalDocumentIdentification",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_document_identification: Option<AttrV<DocumentId>>,
    /// Original `DocumentVersion` when forwarded.
    #[serde(
        rename = "OriginalDocumentVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_document_version: Option<AttrV<DocumentVersion>>,
    /// Original `CreationDateTime` when forwarded.
    #[serde(
        rename = "OriginalDocumentDateTime",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_document_date_time: Option<AttrV<UtcDateTime>>,
    /// Original `AllocationIdentification` when forwarded.
    #[serde(
        rename = "OriginalAllocationIdentification",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_allocation_identification: Option<AttrV<DocumentId>>,
    /// Quarter-hour activation data for the delivery day.
    #[serde(rename = "Period")]
    pub period: Period,
}

// ── ActivationDocument ────────────────────────────────────────────────────────

/// `ActivationDocument` — Redispatch 2.0 activation instruction, response, or
/// reduction document.
///
/// XSD version: 1.1f (Fehlerkorrektur 2026-02-19)  
/// Namespace: `urn:entsoe.eu:wgedi:errp:activationdocument:5:0`
///
/// Three document types share this format:
/// - **ACO** (`A96`): Activation order sent by the requesting NB to the
///   resource provider's NB.
/// - **ACR** (`A41`): Activation response from the resource provider's NB.
/// - **AAR** (`A42`): Tender reduction sent by the requesting NB when it
///   reduces or cancels a previously issued ACO.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "ActivationDocument")]
pub struct ActivationDocument {
    /// Unique document identifier (max 35 chars, case-sensitive).
    #[serde(rename = "DocumentIdentification")]
    pub document_identification: AttrV<DocumentId>,
    /// Document version number (1–999).
    #[serde(rename = "DocumentVersion")]
    pub document_version: AttrV<DocumentVersion>,
    /// Document type: ACR (`A41`), AAR (`A42`), or ACO (`A96`).
    #[serde(rename = "DocumentType")]
    pub document_type: AttrV<ActivationDocType>,
    /// Process type: always `A41` (redispatch process).
    #[serde(rename = "ProcessType")]
    pub process_type: AttrV<ActivationProcessType>,
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
    /// Document creation timestamp (UTC, second precision).
    #[serde(rename = "CreationDateTime")]
    pub creation_date_time: AttrV<UtcDateTime>,
    /// Delivery day covered by this document (UTC interval, minute precision).
    #[serde(rename = "ActivationTimeInterval")]
    pub activation_time_interval: AttrV<TimeInterval>,
    /// `DocumentIdentification` of the ACO this ACR/AAR responds to (optional).
    #[serde(
        rename = "OrderIdentification",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub order_identification: Option<AttrV<DocumentId>>,
    /// `DocumentVersion` of the ACO this ACR/AAR responds to (optional).
    #[serde(
        rename = "OrderIdentificationVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub order_identification_version: Option<AttrV<DocumentVersion>>,
    /// Activated time series (1–2 entries; one per direction).
    #[serde(rename = "ActivationTimeSeries", default)]
    pub time_series: Vec<ActivationTimeSeries>,
    /// Document-level reason (optional; present on full rejections).
    #[serde(rename = "Reason", default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<DocumentReason>,
}
