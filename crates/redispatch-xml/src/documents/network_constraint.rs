use serde::{Deserialize, Serialize};

use crate::documents::activation::{ControlZoneRef, ResourceObjectRef};
use crate::documents::planned_resource_schedule::GridElementCodingScheme;
use crate::types::{
    AttrV, AttrVWithScheme, Direction, DocumentId, DocumentVersion, MarketParticipantId,
    MarketRoleType, MeasureUnit, Period, TimeInterval, UtcDateTime,
};

// ── DocumentType ──────────────────────────────────────────────────────────────

/// `DocumentType` for `NetworkConstraintDocument` (always `B15`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NcdDocType {
    /// Network constraint document.
    #[serde(rename = "B15")]
    NetworkConstraint,
}

/// `ProcessType` for `NetworkConstraintDocument` (always `A14`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NcdProcessType {
    /// Forecast.
    #[serde(rename = "A14")]
    Forecast,
}

/// `BusinessType` for `NetworkConstraintTimeSeries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NcdBusinessType {
    /// Dispatchable production (eligible resource).
    #[serde(rename = "A77")]
    ProductionDispatchable,
    /// Network element constraint.
    #[serde(rename = "B59")]
    NetworkElement,
}

/// Status codes for `NetworkConstraintTimeSeries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NcdStatus {
    /// Activated.
    #[serde(rename = "A07")]
    Activated,
    /// Planned.
    #[serde(rename = "A36")]
    Planned,
    /// Demand.
    #[serde(rename = "Z06")]
    Demand,
}

/// Optional document withdrawal status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NcdDocStatus {
    /// Withdrawal code (only `"A13"` used).
    #[serde(rename = "@v")]
    pub v: String,
}

// ── NetworkConstraintTimeSeries ───────────────────────────────────────────────

/// A single constraint time series within a `NetworkConstraintDocument`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkConstraintTimeSeries {
    /// Unique time-series identifier (max 35 chars).
    #[serde(rename = "TimeSeriesIdentification")]
    pub time_series_identification: AttrV<DocumentId>,
    /// Business type: dispatchable production (`A77`) or network element (`B59`).
    #[serde(rename = "BusinessType")]
    pub business_type: AttrV<NcdBusinessType>,
    /// Direction: up (`A01`) or down (`A02`).
    #[serde(rename = "Direction")]
    pub direction: AttrV<Direction>,
    /// Control zone where the resource / element is located.
    #[serde(rename = "ConnectingArea")]
    pub connecting_area: ControlZoneRef,
    /// Resource object identifier (BDEW resource code, NDE scheme).
    #[serde(rename = "ResourceObject")]
    pub resource_object: ResourceObjectRef,
    /// Grid element EIC or resource code.
    #[serde(rename = "GridElement")]
    pub grid_element: AttrVWithScheme<String, GridElementCodingScheme>,
    /// Physical unit of quantity values.
    #[serde(rename = "MeasurementUnit")]
    pub measurement_unit: AttrV<MeasureUnit>,
    /// Optional activation status.
    #[serde(rename = "Status", default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AttrV<NcdStatus>>,
    /// Quarter-hour constraint data for the delivery day.
    #[serde(rename = "Period")]
    pub period: Period,
}

// ── NetworkConstraintDocument ─────────────────────────────────────────────────

/// `NetworkConstraintDocument` — grid congestion constraint data submitted
/// by TSOs to resource providers' NB.
///
/// XSD version: 1.1b (2025-04-01)  
/// No XML namespace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "NetworkConstraintDocument")]
pub struct NetworkConstraintDocument {
    /// Unique document identifier (max 35 chars).
    #[serde(rename = "DocumentIdentification")]
    pub document_identification: AttrV<DocumentId>,
    /// Document version number (1–999).
    #[serde(rename = "DocumentVersion")]
    pub document_version: AttrV<DocumentVersion>,
    /// Document type (always `B15`).
    #[serde(rename = "DocumentType")]
    pub document_type: AttrV<NcdDocType>,
    /// Process type (always `A14`).
    #[serde(rename = "ProcessType")]
    pub process_type: AttrV<NcdProcessType>,
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
    /// Delivery period covered (UTC interval).
    #[serde(rename = "TimePeriodCovered")]
    pub time_period_covered: AttrV<TimeInterval>,
    /// Optional withdrawal status.
    #[serde(rename = "DocStatus", default, skip_serializing_if = "Option::is_none")]
    pub doc_status: Option<NcdDocStatus>,
    /// Network constraint time series (0+; absent when `doc_status` is set).
    #[serde(
        rename = "NetworkConstraintTimeSeries",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub time_series: Vec<NetworkConstraintTimeSeries>,
}
