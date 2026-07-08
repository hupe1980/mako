#![allow(clippy::doc_markdown)]
//! CloudEvents 1.0 envelope types for `mako-markt`.
//!
//! Two flavours:
//! - [`InboundMakoEvent`] — CloudEvents POSTed by `makod` to the `marktd` ingest endpoint.
//! - [`MarktEvent`] — CloudEvents emitted by `marktd` itself (`de.markt.*`) to ERP subscribers.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use uuid::Uuid;

// ── Inbound from makod ────────────────────────────────────────────────────────

/// CloudEvents 1.0 structured-mode JSON envelope as sent by `makod`.
///
/// `makod` POSTs this to `POST /api/v1/mako/events` (the marktd ingest endpoint).
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
    /// Used by `marktd` `event_ingest` to derive `marktrole` for role-scoped
    /// ERP subscriber fan-out:
    /// - `*-lf` suffix → `"LF"` role
    /// - `wim-*` prefix → `"MSB"` role
    /// - `mabis-*` prefix → `"BIKO"` role
    /// - everything else (gpke-*, geli-gas-*, gabi-gas-*) → `"NB"` role
    ///
    /// Absent when the outbox message was produced before this field was
    /// introduced (forward-compatible: `marktrole` remains `None` in that case).
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

// ── marktd-emitted events ─────────────────────────────────────────────────────

/// An event emitted by `marktd` itself (`de.markt.*`).
///
/// Produced by the event enrichment pipeline and broadcast to the fan-out worker
/// via an unbounded MPSC sender.
///
/// L3 services (`invoicd`, `edmd`, `obsd`) receive this struct deserialized from
/// the JSON webhook body posted by `marktd`'s fan-out worker.
///
/// # CloudEvents extensions (§3.3: lowercase alphanumeric)
///
/// | Field | Description |
/// |---|---|
/// | `marktmaloid` | Resolved Marktlokations-ID |
/// | `marktmeloid` | Resolved Messlokations-ID |
/// | `marktcontractid` | MDM contract UUID |
/// | `marktrole` | Marktrolle: `"NB"`, `"LF"`, `"MSB"`, `"BIKO"`, `"UNB"` |
/// | `markterpref` | ERP-supplied idempotency key |
/// | `makopid` | Forwarded BDEW Prüfidentifikator |
/// | `makoworkflow` | Forwarded workflow family name |
/// | `makoerc` | Forwarded BDEW ERC error code (on `aperak.rejected`) |
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarktEvent {
    /// CloudEvents `specversion` — always `"1.0"`.
    pub specversion: String,
    /// Unique idempotency key for this event (UUID v4).
    pub id: String,
    /// CloudEvents source — `"urn:markt:tenant:{tenant_gln}"`.
    pub source: String,
    /// CloudEvents type, e.g. `"de.markt.malo.updated"`.
    #[serde(rename = "type")]
    pub ce_type: String,
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    /// Business subject (e.g. MaLo-ID or process ID).
    pub subject: String,
    pub datacontenttype: String,
    // markt extension attributes (CloudEvents §3.3: lowercase alphanumeric)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marktmaloid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marktmeloid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marktcontractid: Option<String>,
    /// Canonical `marktrole` value (e.g. `"NB"`, `"LF"`, `"MSB"`, `"BIKO"`, `"UNB"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marktrole: Option<String>,
    /// ERP-supplied `Idempotency-Key` from the originating command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markterpref: Option<String>,
    // ── Forwarded `mako*` extension attributes from the originating makod event ─
    /// Prüfidentifikator of the originating EDIFACT process.
    ///
    /// Forwarded from the `makopid` CloudEvents extension set by `makod` and
    /// preserved by `marktd` so that L3 subscribers (e.g. `edmd`, `obsd`) can
    /// filter events without parsing the `data` payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub makopid: Option<u32>,
    /// Workflow family name of the originating `makod` process
    /// (e.g. `"gpke-lf-anmeldung"`, `"wim-device-change"`).
    ///
    /// Forwarded from `makoworkflow`; used by L3 services for fine-grained
    /// routing without PID ambiguity (PID 17115 exists in both NB and LF roles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub makoworkflow: Option<String>,
    /// BDEW ERC error code from an `AperakRejected` event
    /// (e.g. `"E01"`, `"Z29"`).
    ///
    /// Forwarded from the `makoerc` CloudEvents extension; present only when
    /// `ce_type == "de.mako.aperak.rejected"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub makoerc: Option<String>,
    /// BO4E payload (`_typ` + camelCase fields).
    pub data: serde_json::Value,
}

impl MarktEvent {
    /// Construct a new `MarktEvent` with sensible defaults.
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
            source: format!("urn:markt:tenant:{tenant_gln}"),
            ce_type: ce_type.into(),
            time: OffsetDateTime::now_utc(),
            subject: subject.into(),
            datacontenttype: "application/json".into(),
            marktmaloid: None,
            marktmeloid: None,
            marktcontractid: None,
            marktrole: None,
            markterpref: None,
            makopid: None,
            makoworkflow: None,
            makoerc: None,
            data,
        }
    }

    /// Set all extension attributes from a bulk options struct.
    #[must_use]
    pub fn with_extensions(mut self, ext: EventExtensions) -> Self {
        self.marktmaloid = ext.marktmaloid;
        self.marktmeloid = ext.marktmeloid;
        self.marktcontractid = ext.marktcontractid;
        self.marktrole = ext.marktrole;
        self.markterpref = ext.markterpref;
        self.makopid = ext.makopid;
        self.makoworkflow = ext.makoworkflow;
        self.makoerc = ext.makoerc;
        self
    }
}

/// `markt*` + forwarded `mako*` CloudEvents extension attributes for ergonomic construction.
#[derive(Debug, Default, Clone)]
pub struct EventExtensions {
    pub marktmaloid: Option<String>,
    pub marktmeloid: Option<String>,
    pub marktcontractid: Option<String>,
    pub marktrole: Option<String>,
    pub markterpref: Option<String>,
    /// Forwarded `makopid` from originating `InboundMakoEvent`.
    pub makopid: Option<u32>,
    /// Forwarded `makoworkflow` from originating `InboundMakoEvent`.
    pub makoworkflow: Option<String>,
    /// Forwarded `makoerc` from originating `InboundMakoEvent`.
    pub makoerc: Option<String>,
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
    fn markt_event_source_prefix() {
        let event = MarktEvent::new(
            "9900000000001",
            "de.markt.malo.updated",
            "DE000000000001",
            serde_json::json!({}),
        );
        assert!(event.source.starts_with("urn:markt:tenant:"));
    }

    #[test]
    fn markt_event_with_extensions_sets_marktrole() {
        let event = MarktEvent::new(
            "9900000000001",
            "de.markt.malo.updated",
            "DE000000000001",
            serde_json::json!({}),
        )
        .with_extensions(EventExtensions {
            marktrole: Some("NB".into()),
            ..Default::default()
        });
        assert_eq!(event.marktrole.as_deref(), Some("NB"));

        // Verify round-trip serialization includes marktrole (legacy mdmrole must be absent)
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["marktrole"], "NB");
        assert!(
            json.get("mdmrole").is_none(),
            "must not emit legacy mdmrole field"
        );
    }

    #[test]
    fn markt_event_roundtrip_json() {
        let orig = MarktEvent::new(
            "9900000000001",
            "de.markt.versorgung.beliefert",
            "51238696780",
            serde_json::json!({"lieferstatus": "Beliefert"}),
        )
        .with_extensions(EventExtensions {
            marktmaloid: Some("51238696780".into()),
            marktrole: Some("LF".into()),
            makopid: Some(55003),
            ..Default::default()
        });
        let json = serde_json::to_string(&orig).unwrap();
        let back: MarktEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.marktmaloid, orig.marktmaloid);
        assert_eq!(back.marktrole, orig.marktrole);
        assert_eq!(back.makopid, orig.makopid);
        assert_eq!(back.ce_type, "de.markt.versorgung.beliefert");
    }
}
