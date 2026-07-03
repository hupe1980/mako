//! `PlannedResourceScheduleDocument` -- planned generation and consumption schedule for dispatchable resources.
use serde::{Deserialize, Serialize};

use crate::documents::activation::{ControlZoneRef, EicCodingScheme};
use crate::types::{
    AttrV, AttrVWithScheme, Direction, DocumentId, DocumentVersion, MarketParticipantId,
    MarketRoleType, MeasureUnit, Period, TimeInterval, UtcDateTime,
};

// ── DocumentType ──────────────────────────────────────────────────────────────

/// `DocumentType` codes for `PlannedResourceScheduleDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrsDocType {
    /// Day-ahead plan.
    #[serde(rename = "A14")]
    DayAheadPlan,
    /// Intraday plan (Planungsdaten).
    #[serde(rename = "Z08")]
    IntradayPlan,
    /// Stammdaten-based plan.
    #[serde(rename = "Z09")]
    StammdatenPlan,
    /// Aggregate plan.
    #[serde(rename = "Z11")]
    AggregatePlan,
    /// Corrected plan.
    #[serde(rename = "Z12")]
    CorrectedPlan,
}

/// `ProcessType` for `PlannedResourceScheduleDocument` (always forecast).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrsProcessType {
    /// Forecast (day-ahead / intraday).
    #[serde(rename = "A14")]
    Forecast,
}

/// `BusinessType` codes for `PlannedResourceTimeSeries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrsBusinessType {
    /// Production schedule (A01).
    #[serde(rename = "A01")]
    Production,
    /// Consumption schedule (A04).
    #[serde(rename = "A04")]
    Consumption,
    /// Generation forecast (A10).
    #[serde(rename = "A10")]
    GenerationForecast,
    /// Consumption forecast (A11).
    #[serde(rename = "A11")]
    ConsumptionForecast,
    /// Generation schedule (A12).
    #[serde(rename = "A12")]
    GenerationSchedule,
    /// System operator redispatching schedule (A46).
    #[serde(rename = "A46")]
    SystemOperatorRedispatching,
    /// Transmission capacity (A60).
    #[serde(rename = "A60")]
    TransmissionCapacity,
    /// Exchange schedule (A61).
    #[serde(rename = "A61")]
    ExchangeSchedule,
    /// Dispatchable production unit (A77).
    #[serde(rename = "A77")]
    ProductionDispatchable,
    /// Dispatchable consumption unit (A79).
    #[serde(rename = "A79")]
    ConsumptionDispatchable,
    /// Internal redispatch (A85).
    #[serde(rename = "A85")]
    InternalRedispatch,
    /// Controllable generation unit (A93).
    #[serde(rename = "A93")]
    ControllableGeneration,
    /// Controllable consumption unit (A94).
    #[serde(rename = "A94")]
    ControllableConsumption,
    /// Network element (B59).
    #[serde(rename = "B59")]
    NetworkElement,
    /// Flexibility resource (Z05).
    #[serde(rename = "Z05")]
    Flexibility,
}

/// `Status` codes for `PlannedResourceTimeSeries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrsTimeSeriesStatus {
    /// Activated.
    #[serde(rename = "A07")]
    Activated,
    /// Planned.
    #[serde(rename = "A36")]
    Planned,
    /// Bedarf (demand / requested).
    #[serde(rename = "Z06")]
    Demand,
}

/// Product code (always `"8716867000016"` — active power).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Product {
    /// Active power.
    #[serde(rename = "8716867000016")]
    ActivePower,
}

/// Coding scheme for grid element identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GridElementCodingScheme {
    /// EIC.
    #[serde(rename = "A01")]
    Eic,
    /// National resource code.
    #[serde(rename = "A02")]
    NationalResource,
    /// Other national scheme.
    #[serde(rename = "Z01")]
    Other,
}

// ── PlannedResourceTimeSeries ─────────────────────────────────────────────────

/// A single planned resource time series within a
/// `PlannedResourceScheduleDocument`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedResourceTimeSeries {
    /// Unique time-series identifier within this document (max 35 chars).
    #[serde(rename = "TimeSeriesIdentification")]
    pub time_series_identification: AttrV<DocumentId>,
    /// Business type.
    #[serde(rename = "BusinessType")]
    pub business_type: AttrV<PrsBusinessType>,
    /// Redispatch direction (optional — absent for non-directional series).
    #[serde(rename = "Direction", default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<AttrV<Direction>>,
    /// Control zone where the resource is connected (optional).
    #[serde(
        rename = "ConnectingArea",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub connecting_area: Option<ControlZoneRef>,
    /// Resource object identifier (optional).
    #[serde(
        rename = "ResourceObject",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub resource_object: Option<AttrVWithScheme<String, GridElementCodingScheme>>,
    /// Product code (always active power `8716867000016`).
    #[serde(rename = "Product")]
    pub product: AttrV<Product>,
    /// German TSO control block reference (optional).
    #[serde(
        rename = "AcquiringArea",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub acquiring_area: Option<AttrVWithScheme<String, EicCodingScheme>>,
    /// Grid element EIC or resource code (optional).
    #[serde(
        rename = "GridElement",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub grid_element: Option<AttrVWithScheme<String, GridElementCodingScheme>>,
    /// Physical unit of quantity values.
    #[serde(rename = "MeasureUnit")]
    pub measure_unit: AttrV<MeasureUnit>,
    /// Time series status (optional).
    #[serde(rename = "Status", default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AttrV<PrsTimeSeriesStatus>>,
    /// Resource provider (optional).
    #[serde(
        rename = "ResourceProvider",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub resource_provider: Option<AttrVWithScheme<MarketParticipantId>>,
    /// Quarter-hour power plan for the delivery day.
    #[serde(rename = "Period")]
    pub period: Period,
}

// ── PlannedResourceScheduleDocument ──────────────────────────────────────────

/// `PlannedResourceScheduleDocument` — day-ahead and intraday resource
/// planning data submitted by resource providers and DSOs.
///
/// XSD version: 1.0f (Fehlerkorrektur 2026-02-19)  
/// No XML namespace (no `targetNamespace` in XSD).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "PlannedResourceScheduleDocument")]
pub struct PlannedResourceScheduleDocument {
    /// Unique document identifier (max 35 chars).
    #[serde(rename = "DocumentIdentification")]
    pub document_identification: AttrV<DocumentId>,
    /// Document version number (1–999).
    #[serde(rename = "DocumentVersion")]
    pub document_version: AttrV<DocumentVersion>,
    /// Document type.
    #[serde(rename = "DocumentType")]
    pub document_type: AttrV<PrsDocType>,
    /// Process type (always forecast).
    #[serde(rename = "ProcessType")]
    pub process_type: AttrV<PrsProcessType>,
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
    #[serde(rename = "DocumentDateTime")]
    pub document_date_time: AttrV<UtcDateTime>,
    /// Delivery period covered by this document (UTC interval).
    #[serde(rename = "TimePeriodCovered")]
    pub time_period_covered: AttrV<TimeInterval>,
    /// Planned resource time series (one or more).
    #[serde(rename = "PlannedResourceTimeSeries", default)]
    pub time_series: Vec<PlannedResourceTimeSeries>,
}
