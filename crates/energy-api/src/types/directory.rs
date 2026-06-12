//! Wire-format types for the EDI-Energy Directory Service v1.
//!
//! Derived from:
//! - `directoryServiceV1.yaml` (OpenAPI 3.0.1)
//! - `webSocketV1.yaml` (AsyncAPI 3.0.0)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

// ── Core record types ────────────────────────────────────────────────────────

/// Operational status of an API endpoint registered in the directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ApiStatus {
    /// The API is not available and cannot be called.
    Offline,
    /// The API accepts requests but performs no real processing (interop testing).
    Test,
    /// The API is temporarily unavailable for maintenance.
    Maintenance,
    /// The API is available and fully operational.
    Online,
}

/// A directory entry describing how to reach one major version of an API service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiRecord {
    /// Unique identifier of the responsible API provider.
    pub provider_id: String,
    /// Unique identifier of the API service (e.g. `controlMeasuresV1`).
    pub api_id: String,
    /// Major version of the API service.
    pub major_version: i32,
    /// Base URL of the API endpoint.
    pub url: Url,
    /// Optional supplementary key-value metadata (spec / caller-defined).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_metadata: Option<HashMap<String, String>>,
    /// Timestamp of the last update to this record (RFC 3339 / ISO 8601).
    #[serde(with = "time::serde::rfc3339")]
    pub last_updated: OffsetDateTime,
    /// Monotonically increasing revision counter; starts at 1.
    pub revision: i64,
    /// Current operational status of the registered endpoint.
    pub status: ApiStatus,
}

/// A lightweight reference to a directory entry (used in subscriptions and notifications).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiRecordRef {
    pub provider_id: String,
    pub api_id: String,
    pub major_version: i32,
}

/// A directory entry together with its JWS signature and the signing certificate.
///
/// Received in WebSocket [`DirectoryNotification::modified`] messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedApiRecord {
    /// The signed directory entry.
    pub content: ApiRecord,
    /// Base64url-encoded JWS Signature value (header and payload portions
    /// are omitted — reconstruct the full JWS using [`crate::jws`]).
    pub signature: String,
    /// Signing certificate encoded per RFC 9440 (`:cert:` header format).
    pub signing_cert: String,
}

// ── Service information ──────────────────────────────────────────────────────

/// Contact details of the directory service operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactInfo {
    /// Support e-mail address (at least one of email/phone must be set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Support phone number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

/// Information about a running directory service instance.
///
/// Returned by `GET /info/service/v1` and included in WebSocket notifications.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    /// Fully-qualified version of the implemented interface (e.g. `1.0.0`).
    pub version: String,
    /// Contact information for the technical operator.
    pub contact: ContactInfo,
    /// Timestamp of the last update to this service info.
    #[serde(with = "time::serde::rfc3339")]
    pub last_updated: OffsetDateTime,
    /// Monotonically increasing revision counter; starts at 1.
    pub revision: i64,
}

// ── WebSocket subscription protocol ─────────────────────────────────────────

/// Message sent by the **client** to manage its subscriptions.
///
/// Sent over the WebSocket channel `/ws/subscriptions/v1`.
/// The server responds asynchronously with a [`DirectoryNotification`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionRequest {
    /// Client-chosen correlation ID echoed back in the response notification.
    pub id: String,
    /// Subscriptions to add. `None` / empty means no new subscriptions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested: Option<Vec<SubscriptionItem>>,
    /// Subscriptions to cancel. `None` / empty means no cancellations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canceled: Option<Vec<ApiRecordRef>>,
}

/// One item in a subscription request — a record reference plus an optional
/// known revision to avoid resending data the client already has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionItem {
    /// The directory entry being subscribed to.
    pub record_ref: ApiRecordRef,
    /// The client's last-known revision for this entry.
    /// `0` or absent means the client has no local copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known_revision: Option<i64>,
}

/// Message sent by the **server** to notify the client of directory changes.
///
/// Received over the WebSocket channel `/ws/subscriptions/v1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryNotification {
    /// Echoed subscription request ID (set when responding to a subscribe/cancel).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<String>,
    /// UTC timestamp when this notification was generated (ISO 8601).
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// Current service information (included on first notification after connect
    /// or when service metadata changes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_info: Option<ServiceInfo>,
    /// Directory entries that were added or updated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<Vec<SignedApiRecord>>,
    /// Redirect configurations for directory entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirected: Option<Vec<RedirectInfo>>,
    /// References to directory entries that were deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Vec<ApiRecordRef>>,
    /// Subscriptions that were confirmed as canceled (by client or server).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canceled: Option<Vec<CanceledSubscription>>,
    /// Error information (mutually exclusive with the change fields above).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<NotificationError>,
}

/// Redirect target for a directory entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedirectInfo {
    /// The entry for which a redirect is configured (or was removed).
    pub record_ref: ApiRecordRef,
    /// Configured target URL. `None` when the redirect was removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<Url>,
}

/// A subscription that was confirmed as canceled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanceledSubscription {
    /// The canceled subscription's entry reference.
    pub record_ref: ApiRecordRef,
    /// `true` if the client initiated the cancel; `false` if server-initiated.
    pub canceled_by_client: bool,
    /// Human-readable reason (mandatory for server-initiated cancellations).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Error payload in a [`DirectoryNotification`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationError {
    /// HTTP status code describing the error.
    pub status_code: u32,
    /// Human-readable description.
    pub description: String,
    /// Base64-encoded original [`SubscriptionRequest`] that triggered the error,
    /// when the error arose from a subscribe operation and `subscription_id` is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
}
