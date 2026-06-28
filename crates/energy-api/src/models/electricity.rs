//! Wire-format types for the EDI-Energy electricity market APIs.
//!
//! Covers three API families:
//! - **Control Measures** (`controlMeasuresV1.yaml`) — Steuerungshandlungen
//!   between NB/LF and MSB.
//! - **MaLo Identification** (`maloIdentV1.yaml`) — MaLo-ID retrieval for the
//!   24 h supplier-switch process (GPKE part 2).
//! - **WiM Order** (`wimOrderV1.yaml`) — iMS Universalbestellprozess for smart
//!   meter commissioning (PIDs 11021–11023).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Shared identifiers ────────────────────────────────────────────────────────

/// External transaction ID (UUID RFC 4122), chosen by the sender.
pub type TransactionId = Uuid;
/// Idempotency key for retries (UUID RFC 4122).
pub type InitialTransactionId = Uuid;
/// External reference correlating a response to a prior request (UUID RFC 4122).
pub type ReferenceId = Uuid;
/// 13-digit market partner identifier.
pub type MarketPartnerId = i64;

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

// ── Control Measures ──────────────────────────────────────────────────────────

/// Maximum power value in kW (`"\d{0,6}(\.\d{1,3})?"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaximumPowerValue(pub String);

/// Regulate a location to a specific maximum power value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandControl {
    pub maximum_power_value: MaximumPowerValue,
    /// Start of effect period — ISO 8601 UTC, second precision (e.g. `"2023-08-01T12:30:00Z"`).
    pub execution_time_from: String,
    /// Optional end of effect period.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_until: Option<String>,
}

/// Reset a location to its initial / uncontrolled state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRegular {
    /// Start of effect period — ISO 8601 UTC, second precision.
    pub execution_time_from: String,
}

/// Reason for a negative response from the MSB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasonNegative {
    /// Communication to the control box was disrupted.
    #[serde(rename = "communicationFailure")]
    CommunicationFailure,
    /// MSB back-end is overloaded.
    #[serde(rename = "overload")]
    Overload,
    /// MSB is procedurally unable to fulfil the request.
    #[serde(rename = "unable")]
    Unable,
}

/// Terminal state for negative (failure) responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateNegative {
    #[serde(rename = "failed")]
    Failed,
}

/// Terminal state for positive (success) responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatePositive {
    #[serde(rename = "succeeded")]
    Succeeded,
}

/// Preliminary state — command is executable in principle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreliminaryStatePositive {
    #[serde(rename = "possible")]
    Possible,
}

/// State indicating the final outcome is not yet known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateUnknown {
    #[serde(rename = "unknown")]
    Unknown,
}

// ── MaLo Identification ───────────────────────────────────────────────────────

/// Market location identifier — 11-digit string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MaloId(pub String);

/// Metering location identifier — pattern `DE\d{11}[A-Z,\d]{20}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeloId(pub String);

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

/// Metering technology classification of a market location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeasurementTechnologyClassification {
    #[serde(rename = "intelligentMeasuringSystem")]
    IntelligentMeasuringSystem,
    #[serde(rename = "conventionalMeasuringSystem")]
    ConventionalMeasuringSystem,
    #[serde(rename = "noMeasurement")]
    NoMeasurement,
}

/// Whether the forecast basis may be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptionalChangeForecastBasis {
    #[serde(rename = "possible")]
    Possible,
    #[serde(rename = "notPossible")]
    NotPossible,
}

/// Lifecycle property / category of a market location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketLocationProperty {
    #[serde(rename = "customerFacility")]
    CustomerFacility,
    /// Dormant market location (spec spelling: `"nonActice"`).
    #[serde(rename = "nonActice")]
    NonActive,
    #[serde(rename = "standard")]
    Standard,
}

/// Tranche proportion type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProportionType {
    #[serde(rename = "bilateralAgreement")]
    BilateralAgreement,
    #[serde(rename = "percent")]
    Percent,
}

/// Input parameters for a MaLo identification request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentificationParameter {
    /// Effective date for identification — ISO 8601 UTC, day-boundary midnight.
    pub identification_date_time: String,
    pub energy_direction: EnergyDirection,
    /// Optional ID-based search criteria.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identification_parameter_id: Option<IdentificationParameterId>,
    /// Address-based search criteria.
    pub identification_parameter_address: IdentificationParameterAddress,
}

/// Optional ID-based identification parameters.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentificationParameterId {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub malo_id: Option<MaloId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tranchen_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub melo_ids: Option<Vec<MeloId>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meter_numbers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_number: Option<String>,
}

/// Address-based identification parameters.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentificationParameterAddress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<PersonName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<PostalAddress>,
}

/// Person or company name.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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

/// Positive identification result — all data the NB holds about the market
/// location from `identificationDateTime` onwards.
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

/// Negative identification result, referencing the applicable decision tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaloIdentResultNegative {
    /// Decision tree code from EDI@energy, e.g. `"E_0594"`.
    pub decision_tree: String,
    /// Response code from that tree, e.g. `"A10"`.
    pub response_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// NB that now holds the location (when it left this NB's grid area).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_operator: Option<MarketPartnerId>,
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
    pub market_partner_id: MarketPartnerId,
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

/// Data about a metering location (Messlokation) at the market location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataMeterLocation {
    pub melo_id: MeloId,
    pub meter_number: String,
    pub data_meter_location_measuring_point_operators: Vec<TimeSlicedMarketPartner>,
}

/// Data about a technical resource (Technische Ressource) at the market location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataTechnicalResource {
    pub tr_id: TrId,
}

/// Data about a controllable resource (Steuerbare Ressource) at the market location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataControllableResource {
    pub sr_id: SrId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_controllable_resource_measuring_point_operators: Option<Vec<SrMarketPartner>>,
}

/// Market partner assignment at a controllable resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SrMarketPartner {
    pub market_partner_id: MarketPartnerId,
    pub execution_time_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_until: Option<String>,
    pub market_partner_type_sr: String,
}

/// Data about a network location (Netzlokation) linked to the market location.
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

// ── WiM Order (iMS Universalbestellprozess) ───────────────────────────────────

/// Device category for the iMS Universalbestellprozess.
///
/// Specifies which type of smart meter the Netzbetreiber is ordering from the MSB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WimDeviceCategory {
    /// Intelligentes Messsystem (iMSys) — full smart meter system.
    #[serde(rename = "iMSys")]
    IMSys,
    /// Moderne Messeinrichtung (mME) — basic smart meter display.
    #[serde(rename = "mME")]
    Mme,
    /// Moderne Messeinrichtung mit Kommunikationsadapter (mME+KME).
    #[serde(rename = "mME+KME")]
    MmeKme,
}

/// Rejection reason code for a WiM Ablehnung response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WimRejectionReason {
    /// MeLo does not exist in the MSB's service territory.
    #[serde(rename = "meloUnknown")]
    MeloUnknown,
    /// MSB is not responsible for this MeLo.
    #[serde(rename = "notResponsible")]
    NotResponsible,
    /// Requested device category is not installable at this MeLo.
    #[serde(rename = "deviceCategoryNotSupported")]
    DeviceCategoryNotSupported,
    /// Regulatory prerequisites for iMSys rollout not yet met.
    #[serde(rename = "rolloutPreconditionNotMet")]
    RolloutPreconditionNotMet,
    /// MSB technical capacity exhausted.
    #[serde(rename = "capacityExhausted")]
    CapacityExhausted,
    /// Other / unspecified reason; see `reason_text` for details.
    #[serde(rename = "other")]
    Other,
}

/// Payload for a WiM Anmeldung (PID 11021) — NB orders iMS installation from MSB.
///
/// Sent by the Netzbetreiber to the Messstellenbetreiber over the REST channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WimAnmeldungRequest {
    /// Messlokation EIC code at which the device should be installed.
    pub melo_id: String,
    /// 13-digit GLN of the Netzbetreiber (sender).
    pub netzbetreiber_id: i64,
    /// Requested process date (ISO 8601, date only, e.g. `"2026-06-01"`).
    pub process_date: String,
    /// Requested device category.
    pub device_category: WimDeviceCategory,
    /// Optional free-text notes (e.g. access instructions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Payload for a WiM Bestätigung (PID 11022) — MSB confirms the order.
///
/// Sent by the MSB to the Netzbetreiber after accepting an Anmeldung.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WimBestaetigung {
    /// UUID of the original Anmeldung transaction this response refers to.
    pub reference_id: Uuid,
    /// Confirmed installation date (ISO 8601, date only).
    pub confirmed_process_date: String,
    /// Assigned device identifier (EIC or MSB-internal reference).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

/// Payload for a WiM Ablehnung (PID 11023) — MSB rejects the order.
///
/// Sent by the MSB to the Netzbetreiber after refusing an Anmeldung.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WimAblehnung {
    /// UUID of the original Anmeldung transaction this response refers to.
    pub reference_id: Uuid,
    /// Structured rejection reason code.
    pub reason: WimRejectionReason,
    /// Optional human-readable explanation (supplementary to `reason`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_text: Option<String>,
}
