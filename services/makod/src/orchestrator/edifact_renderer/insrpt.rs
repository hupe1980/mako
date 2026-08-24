//! INSRPT renderer — WiM Störungsmeldung / Ablesesteuerung.
//!
//! `mako-wim` enqueued `INSRPT` outbox entries with no renderer behind them, so
//! `render_to_wire_bytes` returned `InsufficientPayload` and the AS4 sender put
//! the raw domain JSON on the wire in place of an EDIFACT interchange. The
//! message left the system, looked delivered, and could not be parsed by the
//! receiving MSB.
//!
//! The INSRPT AHB marks `BGM`, `DOC`, `DTM`, `LIN`, `LOC`, `NAD`, `RFF` and
//! `STS` mandatory for every Prüfidentifikator, so all of them are emitted here;
//! omitting one produces a message that parses but fails AHB validation.

use super::*;

/// Render an INSRPT status/inspection message from domain-intent JSON.
///
/// | Field           | Required | Description                                      |
/// |-----------------|----------|--------------------------------------------------|
/// | `pid`           | yes      | Prüfidentifikator → `BGM` DE1004 and `RFF+Z13`   |
/// | `melo`          | yes      | Addressed Messlokation → SG8 `LOC+172`           |
/// | `sender`        | no       | Sender MP-ID (falls back to the primary MP-ID)   |
/// | `receiver`      | no       | Receiver MP-ID (falls back to `msg.recipient`)   |
/// | `doc_reference` | no       | SG3 `DOC` Referenz (default qualifier `Z41`)     |
/// | `status_code`   | no       | SG7 `STS` Statuscode (default `Z01`)             |
/// | `position`      | no       | SG7 `LIN` Positionsnummer (default `1`)          |
/// | `document_date` | no       | `DTM+137` date (defaults to today at dispatch)   |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent    |
pub(super) fn render_insrpt(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "INSRPT";

    let pid = require_u32(p, mt, "pid")?;
    let melo = require_str(p, mt, "melo")?;
    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    let release = active_release(MessageType::Insrpt, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    // The AHB carries the Prüfidentifikator in `BGM` DE1004 as an 8-digit,
    // zero-padded value — the form `detect_pruefidentifikator` reads.
    let document_id = format!("{pid:08}");

    let mut builder = builders::InsrptBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code("4")
        .document_id(document_id)
        .pruefidentifikator(pid)
        .position(
            p.get("position")
                .and_then(|v| v.as_str())
                .unwrap_or("1")
                .to_owned(),
        )
        .status(
            p.get("status_code")
                .and_then(|v| v.as_str())
                .unwrap_or("Z01")
                .to_owned(),
        )
        .location("172", melo);

    // `DOC` is mandatory; default its reference to the process' own PID so a
    // caller that has no external Förderreferenz still produces a valid message.
    let (doc_qualifier, doc_id) = match p.get("doc_reference") {
        Some(serde_json::Value::String(id)) => ("Z41".to_owned(), id.clone()),
        Some(serde_json::Value::Object(o)) => (
            o.get("qualifier")
                .and_then(|v| v.as_str())
                .unwrap_or("Z41")
                .to_owned(),
            o.get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&pid.to_string())
                .to_owned(),
        ),
        _ => ("Z41".to_owned(), pid.to_string()),
    };
    builder = builder.doc_reference(doc_qualifier, doc_id);

    if let Some(d) = p.get("document_date").and_then(|v| v.as_str()) {
        builder = builder.document_date(normalise_date(d));
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
