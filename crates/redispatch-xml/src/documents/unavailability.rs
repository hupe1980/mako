//! `Unavailability_MarketDocument` — planned and forced unavailability declarations for generation resources.
use serde::{Deserialize, Serialize};

use crate::documents::activation::EicCodingScheme;
use crate::documents::kaskade::ParticipantMrid;
use crate::types::{Decimal3, Mrid, RevisionNumber, SimpleContent, UtcDateTime, UtcMinuteDateTime};

// ── Namespace ─────────────────────────────────────────────────────────────────

/// Expected XML namespace for `Unavailability_MarketDocument`.
pub const NAMESPACE: &str = "urn:iec62325.351:tc57wg16:451-6:outagedocument:3:0";

// ── Enumerations ──────────────────────────────────────────────────────────────

/// Document type codes for `Unavailability_MarketDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnavailabilityDocType {
    /// Planned unavailability.
    #[serde(rename = "A67")]
    PlannedUnavailability,
    /// Forced (unplanned) unavailability.
    #[serde(rename = "A76")]
    ForcedUnavailability,
    /// Production unavailability.
    #[serde(rename = "A80")]
    ProductionUnavailability,
}

/// Process type for `Unavailability_MarketDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnavailabilityProcessType {
    /// Day-ahead / intraday forecast.
    #[serde(rename = "A14")]
    Forecast,
    /// Outage information.
    #[serde(rename = "A26")]
    OutageInfo,
}

/// Business type for `Unavailability_MarketDocument` time series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnavailabilityBusinessType {
    /// Production.
    #[serde(rename = "A01")]
    Production,
    /// Planned maintenance.
    #[serde(rename = "A53")]
    PlannedMaintenance,
    /// Unplanned outage.
    #[serde(rename = "A54")]
    UnplannedOutage,
}

/// Sender role for `Unavailability_MarketDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnavailabilitySenderRole {
    /// Resource provider.
    #[serde(rename = "A27")]
    ResourceProvider,
    /// Data provider.
    #[serde(rename = "A39")]
    DataProvider,
}

/// Receiver role for `Unavailability_MarketDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnavailabilityReceiverRole {
    /// Grid operator.
    #[serde(rename = "A18")]
    GridOperator,
    /// Data provider.
    #[serde(rename = "A39")]
    DataProvider,
}

// ── Market participant helpers ────────────────────────────────────────────────

/// Market role type for `Unavailability_MarketDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnavailabilityMarketRoleType {
    /// Grid operator.
    #[serde(rename = "A18")]
    GridOperator,
    /// Resource provider.
    #[serde(rename = "A27")]
    ResourceProvider,
    /// Data provider.
    #[serde(rename = "A39")]
    DataProvider,
}

// The sender and receiver are **flat, dotted** elements on the wire —
// `<sender_MarketParticipant.mRID>` and
// `<sender_MarketParticipant.marketRole.type>` — not a nested
// `<sender_MarketParticipant>` container. That is the ENTSO-E CIM convention
// the BDEW XSD follows, and the difference is not cosmetic: a nested document
// fails XSD validation at the counterparty, and an inbound flat one loses the
// sender entirely, because `serde` skips elements the model does not declare.
// `original_sender_MarketParticipant.mRID` in the TimeSeries already had the
// right shape, which is what made the mismatch easy to miss.

/// One quarter-hour point of an unavailability curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnavailabilityPoint {
    /// 1-based position within the `Available_Period`.
    pub position: u32,
    /// Available capacity in that interval (MW).
    pub quantity: Decimal3,
}

/// The asset this unavailability applies to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetRegisteredResource {
    /// Asset identifier.
    #[serde(rename = "mRID")]
    pub m_rid: ParticipantMrid,
}

// ── UnavailabilityTimeInterval ────────────────────────────────────────────────

/// A UTC time interval expressed as separate `start` and `end` sub-elements
/// (minute precision).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnavailabilityTimeInterval {
    /// Start of the unavailability period (UTC, minute precision).
    pub start: UtcMinuteDateTime,
    /// End of the unavailability period (UTC, minute precision).
    pub end: UtcMinuteDateTime,
}

/// The interval-resolved availability curve.
///
/// This is the document's actual payload: `start_DateAndOrTime` and
/// `end_DateAndOrTime` say *when* the resource is affected, and these points
/// say *how much* capacity remains in each interval. A model without them
/// reduces an unavailability to a date range, and the Ausfallarbeit of
/// `BilAReM` Kap. 3.2.2.1 is bounded by exactly this figure — `P_bean`, „die
/// beanspruchbare Leistung der TR … die sich aus Subtraktion der
/// Nichtbeanspruchbarkeit von der installierten Leistung ergibt".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnavailabilityAvailablePeriod {
    /// Interval the curve covers.
    #[serde(rename = "timeInterval")]
    pub time_interval: UnavailabilityTimeInterval,
    /// Resolution of the points (ISO 8601 duration, e.g. `PT15M`).
    pub resolution: String,
    /// The points (at least one).
    #[serde(rename = "Point")]
    pub points: Vec<UnavailabilityPoint>,
}

// ── docStatus ─────────────────────────────────────────────────────────────────

/// Document withdrawal status (used instead of `TimeSeries` for withdrawals).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocStatus {
    /// Always `"A13"` (withdrawn).
    pub value: String,
}

// ── TimeSeries ────────────────────────────────────────────────────────────────

/// Bidding zone domain reference in `Unavailability_MarketDocument`.
pub type UnavailabilityBiddingZone = SimpleContent<String, EicCodingScheme>;

// `biddingZone_Domain.mRID` and `quantity_Measure_Unit.name` are likewise flat
// dotted elements, not containers.

/// A single unavailability time series.
///
/// Each `TimeSeries` covers one calendar day and one business type.
/// Instead of quarter-hour `Period/Interval` data, this uses separate
/// `start_DateAndOrTime.date` / `time` and `end_DateAndOrTime.date` / `time`
/// fields per IEC 62325.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnavailabilityTimeSeries {
    /// Unique time-series identifier within this document.
    #[serde(rename = "mRID")]
    pub m_rid: Mrid,
    /// Original sender mRID when forwarded via data provider (optional).
    #[serde(
        rename = "original_sender_MarketParticipant.mRID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_sender_m_rid: Option<ParticipantMrid>,
    /// Original document mRID when forwarded (optional).
    #[serde(
        rename = "original_document_mRID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_document_m_rid: Option<Mrid>,
    /// Original revision number when forwarded (optional).
    #[serde(
        rename = "original_revisionNumber",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_revision_number: Option<RevisionNumber>,
    /// Original creation timestamp when forwarded (optional).
    #[serde(
        rename = "original_createdDateTime",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_created_date_time: Option<UtcDateTime>,
    /// Original time-series mRID when forwarded (optional).
    #[serde(
        rename = "original_timeseries_mRID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_timeseries_m_rid: Option<Mrid>,
    /// Business type: production, planned maintenance, or unplanned outage.
    #[serde(rename = "businessType")]
    pub business_type: UnavailabilityBusinessType,
    /// Control zone of the resource.
    #[serde(rename = "biddingZone_Domain.mRID")]
    pub bidding_zone_domain_m_rid: UnavailabilityBiddingZone,
    /// The production resource this unavailability applies to.
    #[serde(
        rename = "production_RegisteredResource.mRID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub production_registered_resource_m_rid: Option<ParticipantMrid>,
    /// The power-system resource the production resource belongs to.
    #[serde(
        rename = "production_RegisteredResource.pSRType.powerSystemResources.mRID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub production_registered_resource_psr_type_m_rid: Option<ParticipantMrid>,
    /// Start date of the unavailability period (ISO date `yyyy-mm-dd`).
    #[serde(rename = "start_DateAndOrTime.date")]
    pub start_date: String,
    /// Start time of the unavailability period (`hh:mm:ssZ`).
    #[serde(rename = "start_DateAndOrTime.time")]
    pub start_time: String,
    /// End date of the unavailability period (ISO date `yyyy-mm-dd`).
    #[serde(rename = "end_DateAndOrTime.date")]
    pub end_date: String,
    /// End time of the unavailability period (`hh:mm:ssZ`).
    #[serde(rename = "end_DateAndOrTime.time")]
    pub end_time: String,
    /// Power unit of the availability curve (always `MAW`).
    #[serde(rename = "quantity_Measure_Unit.name")]
    pub quantity_measure_unit_name: String,
    /// Curve type.
    #[serde(rename = "curveType")]
    pub curve_type: String,
    /// The asset this unavailability applies to.
    #[serde(
        rename = "Asset_RegisteredResource",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub asset_registered_resource: Option<AssetRegisteredResource>,
    /// The interval-resolved availability curve.
    #[serde(rename = "Available_Period")]
    pub available_period: UnavailabilityAvailablePeriod,
}

// ── Unavailability_MarketDocument ─────────────────────────────────────────────

/// `Unavailability_MarketDocument` — planned or forced unavailability of a
/// generation resource.
///
/// XSD version: 1.1b (Fehlerkorrektur 2025-04-16)  
/// Namespace: `urn:iec62325.351:tc57wg16:451-6:outagedocument:3:0`
///
/// Each time series covers one complete calendar day. If the document carries
/// a `docStatus` (withdrawal), no `TimeSeries` elements are present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Unavailability_MarketDocument")]
pub struct UnavailabilityMarketDocument {
    /// Unique message identifier (max 35 chars).
    #[serde(rename = "mRID")]
    pub m_rid: Mrid,
    /// Revision number (1–999).
    #[serde(rename = "revisionNumber")]
    pub revision_number: RevisionNumber,
    /// Document type.
    #[serde(rename = "type")]
    pub doc_type: UnavailabilityDocType,
    /// Process type.
    #[serde(rename = "process.processType")]
    pub process_type: UnavailabilityProcessType,
    /// Document creation timestamp (UTC, second precision).
    #[serde(rename = "createdDateTime")]
    pub created_date_time: UtcDateTime,
    /// Sender market participant identifier.
    #[serde(rename = "sender_MarketParticipant.mRID")]
    pub sender_m_rid: ParticipantMrid,
    /// Sender market role.
    #[serde(rename = "sender_MarketParticipant.marketRole.type")]
    pub sender_market_role: UnavailabilityMarketRoleType,
    /// Receiver market participant identifier.
    #[serde(rename = "receiver_MarketParticipant.mRID")]
    pub receiver_m_rid: ParticipantMrid,
    /// Receiver market role.
    #[serde(rename = "receiver_MarketParticipant.marketRole.type")]
    pub receiver_market_role: UnavailabilityMarketRoleType,
    /// The overall unavailability period (one calendar day).
    ///
    /// One flat dotted element on the wire, not a
    /// `<unavailability_Time_Period>` container with a `<timeInterval>` child.
    #[serde(rename = "unavailability_Time_Period.timeInterval")]
    pub unavailability_time_interval: UnavailabilityTimeInterval,
    /// Document withdrawal status (mutually exclusive with `time_series`).
    #[serde(rename = "docStatus", default, skip_serializing_if = "Option::is_none")]
    pub doc_status: Option<DocStatus>,
    /// Unavailability time series (0–30; absent when `doc_status` is set).
    #[serde(rename = "TimeSeries", default, skip_serializing_if = "Vec::is_empty")]
    pub time_series: Vec<UnavailabilityTimeSeries>,
}
