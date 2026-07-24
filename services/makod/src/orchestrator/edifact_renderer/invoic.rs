//! INVOIC / REMADV renderers.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
// ── INVOIC ────────────────────────────────────────────────────────────────────

/// Render an INVOIC (Invoice) envelope from domain-intent JSON.
///
/// This produces a valid EDIFACT envelope with header segments (UNH, BGM, DTM,
/// NAD+MS, NAD+MR, UNT). The UNS+D detail section is intentionally empty —
/// invoices requiring line items and amounts must be rendered by the billing
/// module that has access to the BO4E Rechnung data.
///
/// The empty-detail INVOIC is conformant at the EDIFACT interchange level;
/// the receiving system will respond with REMADV acknowledging receipt.
///
/// Payload fields:
///
/// | Field           | Required | Description                                   |
/// |-----------------|----------|-----------------------------------------------|
/// | `sender`        | no       | Sender GLN (falls back to `tenant_party_id`)  |
/// | `receiver`      | no       | Receiver GLN (falls back to `msg.recipient`)  |
/// | `document_id`   | no       | BGM document identifier (Rechnungsnummer)     |
/// | `document_code` | no       | BGM type code (default `"380"`)               |
/// | `document_date` | no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent |
pub(super) fn render_invoic(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "INVOIC";

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let document_id = p.get("document_id").and_then(|v| v.as_str());
    let document_code = p.get("document_code").and_then(|v| v.as_str());
    let doc_date = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Invoic, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::InvoicBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    if let Some(id) = document_id {
        builder = builder.document_id(id);
    }
    if let Some(code) = document_code {
        builder = builder.document_code(code);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}

// ── REMADV ────────────────────────────────────────────────────────────────────

/// Render a REMADV (Remittance Advice) envelope from domain-intent JSON.
///
/// Produces a valid EDIFACT envelope (UNH, BGM, DTM, NAD+MS, NAD+MR, UNT)
/// that acknowledges receipt and acceptance of a billing document. The detail
/// section (amounts, references) must be added by the billing module.
///
/// Payload fields:
///
/// | Field           | Required | Description                                   |
/// |-----------------|----------|-----------------------------------------------|
/// | `sender`        | no       | Sender GLN (falls back to `registry.primary_mp_id()`)|
/// | `receiver`      | no       | Receiver GLN (falls back to `msg.recipient`)  |
/// | `document_id`   | no       | BGM document identifier (Avisnummer)          |
/// | `document_code` | no       | BGM type code (default `"239"`)               |
/// | `document_date` | no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent |
pub(super) fn render_remadv(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "REMADV";

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let document_id = p.get("document_id").and_then(|v| v.as_str());
    let document_code = p.get("document_code").and_then(|v| v.as_str());
    let doc_date = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Remadv, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::RemadvBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    if let Some(id) = document_id {
        builder = builder.document_id(id);
    }
    if let Some(code) = document_code {
        builder = builder.document_code(code);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
