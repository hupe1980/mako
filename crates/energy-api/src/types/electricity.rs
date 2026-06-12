//! Wire-format types for the EDI-Energy electricity market APIs:
//!
//! - **Control Measures** (`controlMeasuresV1.yaml`) — Steuerungshandlungen
//!   (grid control commands between NB/LF and MSB).
//! - **MaLo Identification** (`maloIdentV1.yaml`) — MaLo-ID retrieval
//!   (supplier-switch 24h process, GPKE part 2).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Shared primitive types ────────────────────────────────────────────────────

/// External transaction ID (UUID RFC 4122), chosen by the sender.
pub type TransactionId = Uuid;

/// Idempotency key for retries (UUID RFC 4122).
pub type InitialTransactionId = Uuid;

/// External reference ID correlating a response to a prior request (UUID RFC 4122).
pub type ReferenceId = Uuid;

/// Network location identifier — pattern `E[A-Z0-9]{9}[0-9]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NeloId(pub String);

/// Controllable resource identifier — pattern `C[A-Z0-9]{9}[0-9]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SrId(pub String);

/// Either a network location ID or a controllable resource ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LocationId {
    NetworkLocation(NeloId),
    ControllableResource(SrId),
}

impl std::fmt::Display for NeloId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for SrId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for LocationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocationId::NetworkLocation(id) => id.fmt(f),
            LocationId::ControllableResource(id) => id.fmt(f),
        }
    }
}

// ── Control Measures types ────────────────────────────────────────────────────

/// Maximum power value in kW, e.g. `"24.123"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaximumPowerValue(pub String);

/// Control command to regulate to a specific power value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandControl {
    /// Maximum power value in kW.
    pub maximum_power_value: MaximumPowerValue,
    /// Start of the effect period (ISO 8601 UTC, second precision).
    pub execution_time_from: String,
    /// Optional end of the effect period.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_until: Option<String>,
}

/// Control command to reset to the initial / uncontrolled state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRegular {
    /// Start of the effect period (ISO 8601 UTC, second precision).
    pub execution_time_from: String,
}

/// Reason code for a negative (failure) response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReasonNegative {
    /// Communication connection to the control box was disrupted.
    #[serde(rename = "communicationFailure")]
    CommunicationFailure,
    /// The MSB back-end is overloaded.
    #[serde(rename = "overload")]
    Overload,
    /// The MSB is procedurally unable to fulfil the request (e.g. maintenance).
    #[serde(rename = "unable")]
    Unable,
}

/// Terminal state of a negative response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateNegative {
    #[serde(rename = "failed")]
    Failed,
}

/// Terminal state of a positive (success) response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatePositive {
    #[serde(rename = "succeeded")]
    Succeeded,
}

/// State for a preliminary positive response (command is executable in principle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreliminaryStatePositive {
    #[serde(rename = "possible")]
    Possible,
}

/// State indicating the final command outcome is not yet known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateUnknown {
    #[serde(rename = "unknown")]
    Unknown,
}

// ── MaLo Identification types ─────────────────────────────────────────────────

/// Market location identifier — 11-digit string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MaloId(pub String);

/// Technical resource identifier — pattern `D[A-Z0-9]{9}[0-9]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrId(pub String);

/// Energy flow direction at a market location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnergyDirection {
    #[serde(rename = "consumption")]
    Consumption,
    #[serde(rename = "production")]
    Production,
}

/// Metering technology class of a market location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeasurementTechnologyClassification {
    #[serde(rename = "intelligentMeasuringSystem")]
    IntelligentMeasuringSystem,
    #[serde(rename = "conventionalMeasuringSystem")]
    ConventionalMeasuringSystem,
    #[serde(rename = "noMeasurement")]
    NoMeasurement,
}

/// Whether the forecast basis can be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptionalChangeForecastBasis {
    #[serde(rename = "possible")]
    Possible,
    #[serde(rename = "notPossible")]
    NotPossible,
}

/// Proportion type for a tranche (billing tranche at a market location).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProportionType {
    #[serde(rename = "bilateralAgreement")]
    BilateralAgreement,
    #[serde(rename = "percent")]
    Percent,
}

/// Property / lifecycle category of a market location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketLocationProperty {
    #[serde(rename = "customerFacility")]
    CustomerFacility,
    /// Note: `"nonActice"` is the spec-defined spelling (sic).
    #[serde(rename = "nonActice")]
    NonActive,
    #[serde(rename = "standard")]
    Standard,
}

/// Input parameters for a MaLo identification request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentificationParameter {
    /// Effective date for which identification is requested (ISO 8601 UTC, day boundary).
    pub identification_date_time: String,
    /// Energy flow direction at the location.
    pub energy_direction: EnergyDirection,
    /// Optional ID-based search parameters (MaLo-ID, meter numbers, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identification_parameter_id: Option<IdentificationParameterId>,
    /// Address-based search parameters.
    pub identification_parameter_address: IdentificationParameterAddress,
}

/// Optional ID-based identification parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentificationParameterId {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub malo_id: Option<MaloId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tranchen_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub melo_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meter_numbers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_number: Option<String>,
}

/// Address-based identification parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentificationParameterAddress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<PersonName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<PostalAddress>,
}

/// Person or company name fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonName {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surnames: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firstnames: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
}

/// German postal address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostalAddress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zip_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub house_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub house_number_addition: Option<String>,
}

// ── MaLo Identification response types ───────────────────────────────────────

/// Positive identification response containing all data known about the
/// requested market location and its associated locations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaloIdentResultPositive {
    pub data_market_location: DataMarketLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_tranches: Option<Vec<DataTranche>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_meter_locations: Option<Vec<DataMeterLocation>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_technical_resources: Option<Vec<DataTechnicalResource>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_controllable_resources: Option<Vec<DataControllableResource>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_network_locations: Option<Vec<DataNetworkLocation>>,
}

/// Negative identification response referencing the applicable decision tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaloIdentResultNegative {
    /// Decision tree code from the EDI@energy document, e.g. `"E_0594"`.
    pub decision_tree: String,
    /// Response code from that decision tree, e.g. `"A10"`.
    pub response_code: String,
    /// Optional free-text explanation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Market partner ID of the NB that now holds the location (when it left the grid area).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_operator: Option<i64>,
}

/// Full data about the identified market location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataMarketLocation {
    pub malo_id: MaloId,
    pub energy_direction: EnergyDirection,
    pub measurement_technology_classification: MeasurementTechnologyClassification,
    pub optional_change_forecast_basis: OptionalChangeForecastBasis,
    pub data_market_location_properties: Vec<MarketLocationProperties>,
    pub data_market_location_network_operators: Vec<TimeSlicedMarketPartner>,
    pub data_market_location_transmission_system_operators: Vec<TimeSlicedMarketPartner>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_market_location_measuring_point_operators: Option<Vec<TimeSlicedMarketPartner>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_market_location_suppliers: Option<Vec<TimeSlicedMarketPartner>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_market_location_name: Option<PersonName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_market_location_address: Option<PostalAddress>,
}

/// A market partner assignment valid for a specific time slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeSlicedMarketPartner {
    pub market_partner_id: i64,
    pub execution_time_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_until: Option<String>,
}

/// Property of a market location valid for a specific time slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketLocationProperties {
    pub market_location_property: MarketLocationProperty,
    pub execution_time_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_until: Option<String>,
}

/// Data about a metering location associated with the market location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataMeterLocation {
    pub melo_id: String,
    pub meter_number: String,
    pub data_meter_location_measuring_point_operators: Vec<TimeSlicedMarketPartner>,
}

/// Data about a technical resource associated with the market location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataTechnicalResource {
    pub tr_id: TrId,
}

/// Data about a controllable resource at the market location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataControllableResource {
    pub sr_id: SrId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_controllable_resource_measuring_point_operators: Option<Vec<SrMarketPartner>>,
}

/// A market partner assignment at a controllable resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SrMarketPartner {
    pub market_partner_id: i64,
    pub execution_time_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_until: Option<String>,
    pub market_partner_type_sr: String,
}

/// Data about a network location associated with the market location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataNetworkLocation {
    pub nelo_id: NeloId,
    pub data_network_location_measuring_point_operators: Vec<TimeSlicedMarketPartner>,
}

/// A billing tranche at a market location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataTranche {
    pub tranchen_id: String,
    pub proportion: ProportionType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    pub data_tranche_suppliers: Vec<TimeSlicedMarketPartner>,
}
