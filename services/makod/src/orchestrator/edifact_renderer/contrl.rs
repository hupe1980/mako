//! CONTRL renderer.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
// ── CONTRL ────────────────────────────────────────────────────────────────────

/// Render a CONTRL functional acknowledgement from domain-intent JSON.
///
/// Payload fields:
///
/// | Field           | Required | Description                                  |
/// |-----------------|----------|----------------------------------------------|
/// | `sender`        | yes      | Sender MP-ID                                   |
/// | `receiver`      | no       | Receiver MP-ID (falls back to `msg.recipient`) |
/// | `interchange_ref`| no      | UCI interchange control reference            |
/// | `accepted`      | no       | `true` = accepted (code 4), `false` = rejected (code 8) |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent              |
pub(super) fn render_contrl(
    p: &serde_json::Value,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "CONTRL";

    let sender = require_str(p, mt, "sender")?;
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let interchange_ref = p
        .get("interchange_ref")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let accepted = p.get("accepted").and_then(|v| v.as_bool()).unwrap_or(true);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Contrl, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::ContrlBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .interchange_ref(interchange_ref)
        .message_ref(message_ref);
    builder = if accepted {
        builder.accept()
    } else {
        builder.reject()
    };

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
