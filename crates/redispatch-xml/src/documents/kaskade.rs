//! `Kaskade` -- emergency cascade measure document for congestion relief between TSO and DSO.
use serde::{Deserialize, Serialize};

use crate::documents::activation::EicCodingScheme;
use crate::types::{Decimal3, Mrid, RevisionNumber, SimpleContent, UtcDateTime, UtcMinuteDateTime};

// ── Namespace ─────────────────────────────────────────────────────────────────

/// Expected XML namespace for `Kaskade`.
pub const NAMESPACE: &str = "urn:iec62325.351:tc57wg16:451-6:outagedocument:3:0";

// ── Enumerations ──────────────────────────────────────────────────────────────

/// Status value for the `Kaskade` document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KaskadeStatus {
    /// Activated (information).
    #[serde(rename = "A07")]
    Activated,
    /// Ordered (Anweisung).
    #[serde(rename = "A10")]
    Ordered,
    /// Deactivation.
    #[serde(rename = "A16")]
    Deactivation,
    /// Preliminary.
    #[serde(rename = "A35")]
    Preliminary,
}

/// Document type for `Kaskade`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KaskadeType {
    /// Emergency measures per § 13(2) EnWG.
    #[serde(rename = "Z16")]
    EmergencyMeasures,
    /// Test message.
    #[serde(rename = "Z17")]
    TestMessage,
}

/// Market role type used in `Kaskade` sender/receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KaskadeRoleType {
    /// Grid operator (NB).
    #[serde(rename = "A18")]
    GridOperator,
}

/// Business type for `Kaskade` time series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KaskadeBusinessType {
    /// Production.
    #[serde(rename = "A01")]
    Production,
    /// Consumption.
    #[serde(rename = "A04")]
    Consumption,
}

/// Curve type for `Kaskade` time series (always `A03`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CurveType {
    /// Variable sized block.
    #[serde(rename = "A03")]
    VariableSizedBlock,
}

/// Measure unit for `Kaskade` quantity (always `MAW`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KaskadeMeasureUnit {
    /// Megawatt.
    #[serde(rename = "MAW")]
    Megawatt,
}

/// Reason code for `Kaskade` time series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KaskadeReasonCode {
    /// Local grid problem.
    #[serde(rename = "Z19")]
    LocalGridProblem,
    /// System balance problem.
    #[serde(rename = "Z20")]
    SystemBalanceProblem,
}

// ── IEC 62325 Market Participant ──────────────────────────────────────────────

/// `mRID` element with `codingScheme` attribute (IEC 62325 simpleContent).
pub type ParticipantMrid = SimpleContent<String>;

// The sender and receiver are **flat, dotted** elements on the wire —
// `<sender_MarketParticipant.mRID>` and
// `<sender_MarketParticipant.marketRole.type>` — not a nested
// `<sender_MarketParticipant>` container. That is the ENTSO-E CIM convention
// the BDEW XSD follows, and the difference is not cosmetic: a nested document
// fails XSD validation at the counterparty, and an inbound flat one loses the
// sender entirely, because `serde` skips elements the model does not declare.

// ── Status ────────────────────────────────────────────────────────────────────

/// Status sub-element: `<status><value>A07</value></status>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusElement {
    /// Status code.
    pub value: KaskadeStatus,
}

// ── Time period ───────────────────────────────────────────────────────────────

/// Time interval (separate start/end elements, minute precision).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KaskadeTimeInterval {
    /// Interval start timestamp (optional — absent for non-time-restricted measures).
    #[serde(rename = "start", default, skip_serializing_if = "Option::is_none")]
    pub start: Option<UtcMinuteDateTime>,
    /// Interval end timestamp (required — marks when the emergency measure expires).
    pub end: UtcMinuteDateTime,
}

/// The `Available_Period` element wrapping the time interval and optional
/// point data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailablePeriod {
    /// Time interval for this period.
    #[serde(rename = "timeInterval")]
    pub time_interval: KaskadeTimeInterval,
    /// Resolution (optional; `PT1M` when present).
    #[serde(
        rename = "resolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub resolution: Option<String>,
    /// Point values within this period.
    #[serde(rename = "Point", default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<KaskadePoint>,
}

/// A single point within `Available_Period`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KaskadePoint {
    /// Position in the series (always 1 for emergency measures).
    pub position: u32,
    /// Quantity in MW (the curtailed / required power).
    pub quantity: Decimal3,
}

// ── Reason ────────────────────────────────────────────────────────────────────

/// Reason for the cascade outage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KaskadeReason {
    /// Reason code: local grid problem (`Z19`) or system balance (`Z20`).
    pub code: KaskadeReasonCode,
    /// Optional free-text description (max 512 chars).
    #[serde(
        rename = "ReasonText",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reason_text: Option<String>,
}

// ── ResourceObject ────────────────────────────────────────────────────────────

/// Coding scheme for resource object identifiers within `Kaskade`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceObjScheme {
    /// EIC (Energy Identification Code) coding scheme (A01).
    #[serde(rename = "A01")]
    Eic,
    /// National resource coding scheme (A02).
    #[serde(rename = "A02")]
    NationalResource,
    /// NDE (German national format) coding scheme.
    #[serde(rename = "NDE")]
    Nde,
    /// Other or proprietary coding scheme (Z01).
    #[serde(rename = "Z01")]
    Other,
}

/// A network connection point or resource object reference (simpleContent).
pub type ResourceObjectRef = SimpleContent<String, ResourceObjScheme>;

// ── BiddingZoneDomain ─────────────────────────────────────────────────────────

/// Bidding zone domain reference (control zone EIC, simpleContent + A01).
pub type BiddingZoneMrid = SimpleContent<String, EicCodingScheme>;

// `biddingZone_Domain.mRID` and `quantity_Measure_Unit.name` are likewise flat
// dotted elements, not containers.

// ── KaskadeTimeSeries ─────────────────────────────────────────────────────────

/// The time series within a `Kaskade` document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KaskadeTimeSeries {
    /// Time-series identifier.
    #[serde(rename = "mRID")]
    pub m_rid: Mrid,
    /// `mRID` of the original document this message relates to (optional).
    #[serde(
        rename = "senders_document_mRID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub senders_document_m_rid: Option<Mrid>,
    /// Revision number of the original document (optional).
    #[serde(
        rename = "senders_revisionNumber",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub senders_revision_number: Option<RevisionNumber>,
    /// Creation timestamp of the original document (optional).
    #[serde(
        rename = "senders_createdDateTime",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub senders_created_date_time: Option<UtcDateTime>,
    /// Business type: production (`A01`) or consumption (`A04`).
    #[serde(rename = "businessType")]
    pub business_type: KaskadeBusinessType,
    /// Network connection points / resource objects (0+).
    #[serde(
        rename = "ResourceObject",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub resource_objects: Vec<ResourceObjectRef>,
    /// Bidding zone domain (control zone EIC).
    #[serde(rename = "biddingZone_Domain.mRID")]
    pub bidding_zone_domain_m_rid: BiddingZoneMrid,
    /// Power unit (always `MAW`).
    #[serde(rename = "quantity_Measure_Unit.name")]
    pub quantity_measure_unit_name: KaskadeMeasureUnit,
    /// Curve type (always `A03`).
    #[serde(rename = "curveType")]
    pub curve_type: CurveType,
    /// Available period with time interval and optional point data.
    #[serde(rename = "Available_Period")]
    pub available_period: AvailablePeriod,
    /// Reason for the cascade measure.
    #[serde(rename = "Reason")]
    pub reason: KaskadeReason,
}

// ── Kaskade ───────────────────────────────────────────────────────────────────

/// `Kaskade` — cascade outage / emergency measure notification.
///
/// XSD version: 1.0 (Fehlerkorrektur 2026-02-19)  
/// Namespace: `urn:iec62325.351:tc57wg16:451-6:outagedocument:3:0`
///
/// Sent by a grid operator to notify downstream operators of an emergency
/// curtailment measure under § 13(2) EnWG. All fields are in IEC 62325 /
/// ENTSO-E direct-text style (not attr-v).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Kaskade")]
pub struct Kaskade {
    /// Document creation timestamp (UTC, second precision).
    #[serde(rename = "createdDateTime")]
    pub created_date_time: UtcDateTime,
    /// Unique message identifier.
    #[serde(rename = "mRID")]
    pub m_rid: Mrid,
    /// Revision number (1–999).
    #[serde(rename = "revisionNumber")]
    pub revision_number: RevisionNumber,
    /// Status of this document revision.
    pub status: StatusElement,
    /// Document type: emergency measure (`Z16`) or test (`Z17`).
    #[serde(rename = "type")]
    pub doc_type: KaskadeType,
    /// Sender market participant identifier.
    #[serde(rename = "sender_MarketParticipant.mRID")]
    pub sender_m_rid: ParticipantMrid,
    /// Sender market role.
    #[serde(rename = "sender_MarketParticipant.marketRole.type")]
    pub sender_market_role: KaskadeRoleType,
    /// Receiver market participant identifier.
    #[serde(rename = "receiver_MarketParticipant.mRID")]
    pub receiver_m_rid: ParticipantMrid,
    /// Receiver market role.
    #[serde(rename = "receiver_MarketParticipant.marketRole.type")]
    pub receiver_market_role: KaskadeRoleType,
    /// The curtailment / emergency time series.
    #[serde(rename = "TimeSeries")]
    pub time_series: KaskadeTimeSeries,
}
