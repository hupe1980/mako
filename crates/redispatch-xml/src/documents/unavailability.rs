use serde::{Deserialize, Serialize};

use crate::documents::activation::EicCodingScheme;
use crate::documents::kaskade::ParticipantMrid;
use crate::types::{Mrid, RevisionNumber, SimpleContent, UtcDateTime, UtcMinuteDateTime};

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

/// Market role sub-element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnavailabilityMarketRole {
    #[serde(rename = "type")]
    pub role_type: UnavailabilityMarketRoleType,
}

/// Market participant reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnavailabilityParticipant {
    #[serde(rename = "mRID")]
    pub m_rid: ParticipantMrid,
    #[serde(rename = "marketRole")]
    pub market_role: UnavailabilityMarketRole,
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

/// `unavailability_Time_Period` wrapper element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnavailabilityTimePeriod {
    #[serde(rename = "timeInterval")]
    pub time_interval: UnavailabilityTimeInterval,
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

/// `biddingZone_Domain` element in `Unavailability_MarketDocument`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnavailabilityBiddingZoneDomain {
    #[serde(rename = "mRID")]
    pub m_rid: UnavailabilityBiddingZone,
}

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
    #[serde(rename = "biddingZone_Domain")]
    pub bidding_zone_domain: UnavailabilityBiddingZoneDomain,
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
    /// Sender market participant.
    #[serde(rename = "sender_MarketParticipant")]
    pub sender_market_participant: UnavailabilityParticipant,
    /// Receiver market participant.
    #[serde(rename = "receiver_MarketParticipant")]
    pub receiver_market_participant: UnavailabilityParticipant,
    /// The overall unavailability period (one calendar day).
    #[serde(rename = "unavailability_Time_Period")]
    pub unavailability_time_period: UnavailabilityTimePeriod,
    /// Document withdrawal status (mutually exclusive with `time_series`).
    #[serde(rename = "docStatus", default, skip_serializing_if = "Option::is_none")]
    pub doc_status: Option<DocStatus>,
    /// Unavailability time series (0–30; absent when `doc_status` is set).
    #[serde(rename = "TimeSeries", default, skip_serializing_if = "Vec::is_empty")]
    pub time_series: Vec<UnavailabilityTimeSeries>,
}
