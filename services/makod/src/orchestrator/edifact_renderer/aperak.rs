//! APERAK renderer.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use mako_engine::payload::AperakPayload;

use super::*;
// ── APERAK ────────────────────────────────────────────────────────────────────

/// Render an APERAK message from its typed payload, [`AperakPayload`] — the
/// struct the producing workflow built, so a field renamed on one side is a
/// compile error on the other.
///
/// `receiver` falls back to the outbox entry's recipient; the interchange
/// reference derives from `causation_event_id`.
pub(super) fn render_aperak(
    p: &serde_json::Value,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
    let payload: AperakPayload = typed_payload(p)?;

    // Gas positive APERAK: silence = acceptance per APERAK AHB 1.1 §2.3. The
    // entry still reaches the ERP webhook as domain JSON; nothing goes on the wire.
    if payload.suppress_wire {
        return Err(RenderError::Suppressed {
            reason: "Gas positive APERAK: suppress_wire=true (silence = acceptance, APERAK AHB 1.1 §2.3)"
                .into(),
        });
    }

    let sender = payload.sender.as_str();
    let receiver = payload
        .receiver
        .as_deref()
        .unwrap_or(msg.recipient.as_ref());

    let release = active_release(MessageType::Aperak, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: "APERAK".into(),
        }
    })?;

    // `SG2` (`RFF+ACE` + `DTM+171`, and `RFF+AGO` on an Anerkennungsmeldung) is
    // **Muss** in both APERAK Anwendungsfälle; `orig_message_ref` is a required
    // field of the payload, so an entry without it never reaches here.
    let message_ref = msg_ref_from_uuid(&msg.causation_event_id.to_string());

    // `BGM` DE 1001 follows from the presence of an error code — `313`
    // Verarbeitbarkeitsfehlermeldung, `312` Anerkennungsmeldung — and
    // `AperakBuilder` derives it.
    let mut builder = builders::AperakBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .acw_ref(&payload.orig_message_ref);
    if let Ok(ep) = Pruefidentifikator::new(payload.pid) {
        builder = builder.pruefidentifikator(ep);
    }
    if let Some(c) = payload.error_code.as_deref() {
        builder = builder.error_code(c);
    }
    if let Some(t) = payload.reason.as_deref() {
        builder = builder.error_text(t);
    }
    if let Some(o) = payload.ortsangabe.as_deref() {
        builder = builder.ortsangabe(o);
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
