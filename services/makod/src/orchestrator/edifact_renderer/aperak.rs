//! APERAK renderer.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
// ── APERAK ────────────────────────────────────────────────────────────────────

/// Render an APERAK message from domain-intent JSON.
///
/// Payload fields:
///
/// | Field             | Required | Description                                  |
/// |-------------------|----------|----------------------------------------------|
/// | `sender`          | yes      | Sender MP-ID                                   |
/// | `receiver`        | no       | Receiver MP-ID (falls back to `msg.recipient`) |
/// | `pid`             | no       | APERAK Prüfidentifikator (29001 or 29002)    |
/// | `orig_message_ref`| **yes**  | `UNH` DE 0062 of the message being acked     |
/// | `error_code`      | no       | ERC error code (e.g. `"E01"`)                |
/// | `reason`          | no       | `FTX+ABO` free-text error description        |
/// | `ortsangabe`      | no       | `FTX+Z02` — where the AHB is violated; Muss for the six AHB-error codes, defaulting to `reason` |
/// | `document_date`   | no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)                  |
/// | `message_ref`     | no       | Derived from `causation_event_id` when absent               |
pub(super) fn render_aperak(
    p: &serde_json::Value,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "APERAK";

    // Gas positive APERAK: silence = acceptance per APERAK AHB 1.0 §2.3.
    // Payload carries `suppress_wire: true` to signal no wire EDIFACT should be sent.
    // The outbox entry is still delivered as domain JSON to the ERP webhook.
    if p.get("suppress_wire")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(RenderError::Suppressed {
            reason: "Gas positive APERAK: suppress_wire=true (silence = acceptance, APERAK AHB 1.0 §2.3)"
                .into(),
        });
    }

    let sender = require_str(p, mt, "sender")?;
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    let release = active_release(MessageType::Aperak, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let pid = p.get("pid").and_then(|v| v.as_u64()).map(|n| n as u32);
    // `SG2` (`RFF+ACE` + `DTM+171`, and `RFF+AGO` on an Anerkennungsmeldung) is
    // **Muss** in both APERAK Anwendungsfälle, and the builder emits the whole
    // group only when this reference is present. Rendering without it produces
    // a message that fails the receiving Marktpartner's own AHB check with
    // three findings — better to dead-letter the outbox entry, where an
    // operator sees it, than to put that on the wire.
    let acw_ref = require_str(p, mt, "orig_message_ref")?;
    let error_code = p.get("error_code").and_then(|v| v.as_str());
    let reason = p.get("reason").and_then(|v| v.as_str());
    let doc_date = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let mut builder = builders::AperakBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    // `BGM` DE 1001 follows from the presence of an error code — `313`
    // Verarbeitbarkeitsfehlermeldung, `312` Anerkennungsmeldung — and
    // `AperakBuilder` derives it. Only an explicit `document_code` overrides
    // that, and nothing in mako sets one.
    if let Some(code) = p.get("document_code").and_then(|v| v.as_str()) {
        builder = builder.document_code(code);
    }

    if let Some(pv) = pid
        && let Ok(ep) = Pruefidentifikator::new(pv)
    {
        builder = builder.pruefidentifikator(ep);
    }
    builder = builder.acw_ref(acw_ref);
    if let Some(c) = error_code {
        builder = builder.error_code(c);
    }
    if let Some(t) = reason {
        builder = builder.error_text(t);
    }
    if let Some(o) = p.get("ortsangabe").and_then(|v| v.as_str()) {
        builder = builder.ortsangabe(o);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
