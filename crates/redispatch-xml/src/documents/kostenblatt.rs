//! `Kostenblatt` -- cost sheet for redispatch measures (billing document, ANB to VNB/UNB).
use serde::{Deserialize, Serialize};

use crate::documents::activation::{ControlZoneRef, ResourceObjectRef};
use crate::documents::planned_resource_schedule::Product;
use crate::types::{
    AttrV, AttrVWithScheme, Direction, DocumentId, DocumentVersion, MarketParticipantId,
    MarketRoleType, Period, TimeInterval, UtcDateTime,
};

// ── DocumentType ──────────────────────────────────────────────────────────────

/// `DocumentType` for `Kostenblatt` (always `Z05`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KostenblattDocType {
    /// Cost sheet (Kostenblatt).
    #[serde(rename = "Z05")]
    Kostenblatt,
}

/// `ProcessType` for `Kostenblatt` (always `A14`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KostenblattProcessType {
    /// Forecast (Planungsdaten).
    #[serde(rename = "A14")]
    Forecast,
}

/// `BusinessType` codes for `CostTimeSeries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CostBusinessType {
    /// Production — energy-dependent costs (Arbeitspreis).
    #[serde(rename = "A01")]
    ProductionEnergy,
    /// Consumption — energy-dependent costs.
    #[serde(rename = "A04")]
    ConsumptionEnergy,
    /// Startup costs (Anfahrkosten).
    #[serde(rename = "Z01")]
    StartupCosts,
    /// Extra operating-hour costs (Betriebsstundenkosten).
    #[serde(rename = "Z02")]
    ExtraOperatingHourCosts,
    /// Avoided network fees (vermiedene Netzentgelte).
    #[serde(rename = "Z03")]
    AvoidedNetworkFees,
    /// Additional wRDV costs.
    #[serde(rename = "Z06")]
    AdditionalWrdvCosts,
}

// ── CostTimeSeries ────────────────────────────────────────────────────────────

/// A single cost time series within a `Kostenblatt`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostTimeSeries {
    /// Unique time-series identifier (max 35 chars).
    #[serde(rename = "TimeSeriesIdentification")]
    pub time_series_identification: AttrV<DocumentId>,
    /// Cost type.
    #[serde(rename = "BusinessType")]
    pub business_type: AttrV<CostBusinessType>,
    /// Direction (optional — absent for startup/fixed costs).
    #[serde(rename = "Direction", default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<AttrV<Direction>>,
    /// Product (always active power `8716867000016`).
    #[serde(rename = "Product")]
    pub product: AttrV<Product>,
    /// Control zone of the resource (optional).
    #[serde(
        rename = "ConnectingArea",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub connecting_area: Option<ControlZoneRef>,
    /// Resource object identifier (BDEW resource code, NDE scheme; optional).
    #[serde(
        rename = "ResourceObject",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub resource_object: Option<ResourceObjectRef>,
    /// Quarter-hour cost data for the delivery day.
    #[serde(rename = "Period")]
    pub period: Period,
}

// ── Kostenblatt ───────────────────────────────────────────────────────────────

/// `Kostenblatt` — cost sheet for redispatch billing between grid operators.
///
/// XSD version: 1.0d (Fehlerkorrektur 2025-04-16)  
/// No XML namespace.
///
/// Contains energy-dependent and fixed cost data for each activated
/// controllable resource in a given delivery day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Kostenblatt")]
pub struct Kostenblatt {
    /// Unique document identifier (max 35 chars).
    #[serde(rename = "DocumentIdentification")]
    pub document_identification: AttrV<DocumentId>,
    /// Document version number (1–999).
    #[serde(rename = "DocumentVersion")]
    pub document_version: AttrV<DocumentVersion>,
    /// Document type (always `Z05`).
    #[serde(rename = "DocumentType")]
    pub document_type: AttrV<KostenblattDocType>,
    /// Process type (always `A14`).
    #[serde(rename = "ProcessType")]
    pub process_type: AttrV<KostenblattProcessType>,
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
    /// Cost time series (one or more).
    #[serde(rename = "CostTimeSeries", default)]
    pub time_series: Vec<CostTimeSeries>,
}
