#![allow(clippy::doc_markdown)]
//! CloudEvents 1.0 envelope types for `mako-mdm`.
//!
//! Two flavours:
//! - [`InboundMakoEvent`] — CloudEvents POSTed by `makod` to the MDM ingest endpoint.
//! - [`MdmEvent`] — CloudEvents emitted by the MDM itself (`de.mdm.*`) to ERP subscribers.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use uuid::Uuid;

// ── Inbound from makod ────────────────────────────────────────────────────────

/// CloudEvents 1.0 structured-mode JSON envelope as sent by `makod`.
///
/// `makod` POSTs this to `POST /api/v1/mako/events` (the MDM ingest endpoint).
#[derive(Debug, Clone, Deserialize)]
pub struct InboundMakoEvent {
    pub specversion: String,
    pub id: String,
    pub source: String,
    #[serde(rename = "type")]
    pub ce_type: String,
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub subject: Option<String>,
    pub dataschema: Option<String>,
    pub datacontenttype: Option<String>,
    // mako extension attributes
    #[serde(default)]
    pub makopid: Option<u32>,
    #[serde(default)]
    pub makoconvid: Option<String>,
    #[serde(default)]
    pub makocausationid: Option<String>,
    #[serde(default)]
    pub makofailreason: Option<String>,
    /// Workflow family name from the originating `makod` process
    /// (e.g. `"gpke-sperrung"`, `"gpke-lf-anmeldung"`, `"wim-device-change"`).
    ///
    /// Used by `mdmd` `event_ingest` to derive `mdmrole` for role-scoped
    /// ERP subscriber fan-out:
    /// - `*-lf` suffix → `"LF"` role
    /// - `wim-*` prefix → `"MSB"` role
    /// - `mabis-*` prefix → `"BIKO"` role
    /// - everything else (gpke-*, geli-gas-*, gabi-gas-*) → `"NB"` role
    ///
    /// Absent when the outbox message was produced before this field was
    /// introduced (forward-compatible: `mdmrole` remains `None` in that case).
    #[serde(default)]
    pub makoworkflow: Option<String>,
    pub data: serde_json::Value,
}

impl InboundMakoEvent {
    /// Return the `process_id` (UUID) from the `subject` field.
    ///
    /// Returns `None` if the subject is absent or not a valid UUID v4.
    #[must_use]
    pub fn process_id(&self) -> Option<Uuid> {
        self.subject.as_deref().and_then(|s| s.parse().ok())
    }

    /// Return the `conversation_id` UUID from the `makoconvid` extension.
    #[must_use]
    pub fn conv_id(&self) -> Option<Uuid> {
        self.makoconvid.as_deref().and_then(|s| s.parse().ok())
    }
}

// ── MDM-emitted events ────────────────────────────────────────────────────────

/// An event emitted by `mdmd` itself (`de.mdm.*`).
///
/// Produced by the event enrichment pipeline and broadcast to the fan-out worker
/// via a `tokio::sync::broadcast::Sender<MdmEvent>`.
#[derive(Debug, Clone, Serialize)]
pub struct MdmEvent {
    /// CloudEvents `specversion` — always `"1.0"`.
    pub specversion: String,
    /// Unique idempotency key for this event (UUID v4).
    pub id: String,
    /// CloudEvents source — `"urn:mdm:tenant:{tenant_gln}"`.
    pub source: String,
    /// CloudEvents type, e.g. `"de.mdm.malo.updated"`.
    #[serde(rename = "type")]
    pub ce_type: String,
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    /// Business subject (e.g. MaLo-ID or contract ID).
    pub subject: String,
    pub datacontenttype: String,
    // MDM extension attributes (CloudEvents §3.3: lowercase alphanumeric)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mdmmaloid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mdmmeloid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mdmcontractid: Option<String>,
    /// Canonical `mdmrole` value (e.g. `"NB"`, `"LF"`, `"MSB"`, `"UNB"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mdmrole: Option<String>,
    /// ERP-supplied `Idempotency-Key` from the originating command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mdmerpref: Option<String>,
    /// BO4E payload (`_typ` + camelCase fields).
    pub data: serde_json::Value,
}

impl MdmEvent {
    /// Construct a new `MdmEvent` with sensible defaults.
    #[must_use]
    pub fn new(
        tenant_gln: &str,
        ce_type: impl Into<String>,
        subject: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            specversion: "1.0".into(),
            id: Uuid::new_v4().to_string(),
            source: format!("urn:mdm:tenant:{tenant_gln}"),
            ce_type: ce_type.into(),
            time: OffsetDateTime::now_utc(),
            subject: subject.into(),
            datacontenttype: "application/json".into(),
            mdmmaloid: None,
            mdmmeloid: None,
            mdmcontractid: None,
            mdmrole: None,
            mdmerpref: None,
            data,
        }
    }

    /// Set all MDM extension attributes from a bulk options struct.
    #[must_use]
    pub fn with_extensions(mut self, ext: EventExtensions) -> Self {
        self.mdmmaloid = ext.mdmmaloid;
        self.mdmmeloid = ext.mdmmeloid;
        self.mdmcontractid = ext.mdmcontractid;
        self.mdmrole = ext.mdmrole;
        self.mdmerpref = ext.mdmerpref;
        self
    }
}

/// MDM CloudEvents extension attributes bundled for ergonomic construction.
#[derive(Debug, Default, Clone)]
pub struct EventExtensions {
    pub mdmmaloid: Option<String>,
    pub mdmmeloid: Option<String>,
    pub mdmcontractid: Option<String>,
    pub mdmrole: Option<String>,
    pub mdmerpref: Option<String>,
}

// ── HMAC signature ────────────────────────────────────────────────────────────

/// Compute `X-Mako-Signature: <hex>` for an outbound webhook delivery.
///
/// Uses HMAC-SHA256 over the raw JSON body bytes.
///
/// # Panics
///
/// Never panics in practice — HMAC accepts any key length.
#[must_use]
pub fn compute_signature(secret: &[u8], body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let mut mac = <Hmac<Sha256>>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(body);
    let result = mac.finalize().into_bytes();
    hex::bytes_to_hex_str(&result)
}

/// Verify an `X-Mako-Signature` header in constant time.
///
/// Returns `true` if the signature matches.
#[must_use]
pub fn verify_signature(secret: &[u8], body: &[u8], provided_hex: &str) -> bool {
    use subtle::ConstantTimeEq;
    let expected = compute_signature(secret, body);
    expected.as_bytes().ct_eq(provided_hex.as_bytes()).into()
}

// ── hex helpers ───────────────────────────────────────────────────────────────

mod hex {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    pub(super) fn bytes_to_hex_str(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0xf) as usize] as char);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_roundtrip() {
        let secret = b"test-secret";
        let body = b"{\"type\":\"de.mako.process.completed\"}";
        let sig = compute_signature(secret, body);
        assert!(verify_signature(secret, body, &sig));
        assert!(!verify_signature(secret, body, "deadbeef"));
    }

    #[test]
    fn makoworkflow_deserializes_from_cloudevent_json() {
        let json = r#"{
            "specversion": "1.0",
            "id": "evt-001",
            "source": "urn:mako:tenant:9900000000001",
            "type": "de.mako.gpke.lieferbeginn.completed",
            "time": "2025-10-01T10:00:00Z",
            "makopid": 55003,
            "makoconvid": "a0000000-0000-0000-0000-000000000001",
            "makocausationid": "b0000000-0000-0000-0000-000000000002",
            "makoworkflow": "gpke-supplier-change",
            "data": {}
        }"#;
        let event: InboundMakoEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.makoworkflow.as_deref(), Some("gpke-supplier-change"));
    }

    #[test]
    fn makoworkflow_absent_deserializes_to_none() {
        let json = r#"{
            "specversion": "1.0",
            "id": "evt-002",
            "source": "urn:mako:tenant:9900000000001",
            "type": "de.mako.gpke.lieferbeginn.completed",
            "time": "2025-10-01T10:00:00Z",
            "data": {}
        }"#;
        let event: InboundMakoEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.makoworkflow, None);
    }

    #[test]
    fn mdm_event_with_extensions_sets_mdmrole() {
        let event = MdmEvent::new(
            "9900000000001",
            "de.mdm.malo.updated",
            "DE000000000001",
            serde_json::json!({}),
        )
        .with_extensions(EventExtensions {
            mdmrole: Some("NB".into()),
            ..Default::default()
        });
        assert_eq!(event.mdmrole.as_deref(), Some("NB"));

        // Verify round-trip serialization includes mdmrole
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["mdmrole"], "NB");
    }
}
