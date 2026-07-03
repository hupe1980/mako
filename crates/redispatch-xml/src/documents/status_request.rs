//! `StatusRequest_MarketDocument` — TSO/DSO request for the current status of a Redispatch resource.
use serde::{Deserialize, Serialize};

use crate::documents::kaskade::ParticipantMrid;
use crate::types::{Mrid, SimpleContent, UtcDateTime};

// ── Namespace ─────────────────────────────────────────────────────────────────

/// Expected XML namespace for `StatusRequest_MarketDocument`.
pub const NAMESPACE: &str = "urn:iec62325.351:tc57wg16:451-5:statusrequestdocument:4:1";

// ── Enumerations ──────────────────────────────────────────────────────────────

/// Document type for `StatusRequest_MarketDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatusRequestDocType {
    /// Status request.
    #[serde(rename = "A60")]
    StatusRequest,
    /// Catalogue request.
    #[serde(rename = "Z15")]
    CatalogueRequest,
}

/// Sender role for `StatusRequest_MarketDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatusRequestSenderRole {
    /// Grid operator.
    #[serde(rename = "A18")]
    GridOperator,
    /// Data provider.
    #[serde(rename = "A39")]
    DataProvider,
}

/// Receiver role for `StatusRequest_MarketDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatusRequestReceiverRole {
    /// Grid operator.
    #[serde(rename = "A18")]
    GridOperator,
    /// Resource provider.
    #[serde(rename = "A27")]
    ResourceProvider,
    /// Other / central.
    #[serde(rename = "Z01")]
    Other,
}

/// Market participant status used in `MktActivityRecord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParticipantStatus {
    /// Deactivated.
    #[serde(rename = "A03")]
    Deactivated,
    /// Reactivated.
    #[serde(rename = "A04")]
    Reactivated,
    /// Withdrawn.
    #[serde(rename = "A13")]
    Withdrawn,
}

// ── Market role sub-element ───────────────────────────────────────────────────

/// Market role element within `sender_MarketParticipant`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusRequestSenderMarketRole {
    /// Role code.
    #[serde(rename = "type")]
    pub role_type: StatusRequestSenderRole,
}

/// Market role element within `receiver_MarketParticipant`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusRequestReceiverMarketRole {
    /// Role code.
    #[serde(rename = "type")]
    pub role_type: StatusRequestReceiverRole,
}

// ── Sender / receiver participants ───────────────────────────────────────────

/// Sender market participant reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusRequestSender {
    /// Market participant mRID (simpleContent: text + codingScheme).
    #[serde(rename = "mRID")]
    pub m_rid: ParticipantMrid,
    /// Market role.
    #[serde(rename = "marketRole")]
    pub market_role: StatusRequestSenderMarketRole,
}

/// Receiver market participant reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusRequestReceiver {
    /// Market participant mRID (simpleContent: text + codingScheme).
    #[serde(rename = "mRID")]
    pub m_rid: ParticipantMrid,
    /// Market role.
    #[serde(rename = "marketRole")]
    pub market_role: StatusRequestReceiverMarketRole,
}

// ── AttributeInstanceComponent ────────────────────────────────────────────────

/// A generic key–value attribute (used for query parameters in status requests).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributeInstanceComponent {
    /// Attribute name.
    #[serde(rename = "attribute")]
    pub attribute: String,
    /// Attribute value.
    #[serde(
        rename = "attributeValue",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub attribute_value: Option<String>,
}

// ── MktActivityRecord ─────────────────────────────────────────────────────────

/// Participant status record in a `StatusRequest_MarketDocument`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MktActivityRecord {
    /// Market participant identifier (max 16 chars).
    #[serde(rename = "MarketParticipant.mRID")]
    pub market_participant_m_rid: SimpleContent<String>,
    /// Participant status: deactivated, reactivated, or withdrawn.
    pub status: ParticipantStatus,
}

// ── StatusRequest_MarketDocument ──────────────────────────────────────────────

/// `StatusRequest_MarketDocument` — status request from a grid operator for
/// resource provider participation data.
///
/// XSD version: 1.1 (2025-04-01)  
/// Namespace: `urn:iec62325.351:tc57wg16:451-5:statusrequestdocument:4:1`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "StatusRequest_MarketDocument")]
pub struct StatusRequestMarketDocument {
    /// Unique message identifier (max 35 chars).
    #[serde(rename = "mRID")]
    pub m_rid: Mrid,
    /// Document type.
    #[serde(rename = "type")]
    pub doc_type: StatusRequestDocType,
    /// Sender market participant.
    #[serde(rename = "sender_MarketParticipant")]
    pub sender_market_participant: StatusRequestSender,
    /// Receiver market participant.
    #[serde(rename = "receiver_MarketParticipant")]
    pub receiver_market_participant: StatusRequestReceiver,
    /// Document creation timestamp (UTC, second precision).
    #[serde(rename = "createdDateTime")]
    pub created_date_time: UtcDateTime,
    /// Generic attribute–value components (query parameters).
    #[serde(
        rename = "AttributeInstanceComponent",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub attributes: Vec<AttributeInstanceComponent>,
    /// Participant status records.
    #[serde(
        rename = "MktActivityRecord",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub mkt_activity_records: Vec<MktActivityRecord>,
}
