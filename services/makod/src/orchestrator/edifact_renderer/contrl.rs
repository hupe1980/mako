//! CONTRL renderer.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
// ── CONTRL ────────────────────────────────────────────────────────────────────

/// Render a CONTRL from domain-intent JSON.
///
/// CONTRL AHB 1.0 Kap. 3 admits exactly two `UCI` DE 0083 values: `7`
/// „Übertragung bestätigt" for the Empfangsbestätigung and `4` „Diese Ebene und
/// alle tieferen Ebenen zurückgewiesen" for the Syntaxfehlermeldung, which
/// additionally carries the DE 0085 Syntaxfehler code.
///
/// Payload fields:
///
/// | Field            | Required | Description                                                  |
/// |------------------|----------|--------------------------------------------------------------|
/// | `sender`         | yes      | Sender MP-ID                                                 |
/// | `accepted`       | yes      | `true` = Empfangsbestätigung (DE 0083 `7`), `false` = Syntaxfehlermeldung (DE 0083 `4`) |
/// | `receiver`       | no       | Receiver MP-ID (falls back to `msg.recipient`)               |
/// | `interchange_ref`| no       | UCI interchange control reference                            |
/// | `syntax_error`   | no       | DE 0085 on a Syntaxfehlermeldung; `12` when absent           |
/// | `message_ref`    | no       | Derived from `causation_event_id` when absent                |
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
    // Stated, never defaulted: a CONTRL says either „syntaktisch fehlerfrei
    // empfangen" or „zurückgewiesen und nicht weiterbearbeitet", and both are
    // binding statements to the counterparty. Defaulting the flag would make the
    // confirming one out of an absent decision.
    let accepted = p
        .get("accepted")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "accepted (UCI DE 0083: true = 7 Übertragung bestätigt, \
                    false = 4 zurückgewiesen)"
                .into(),
        })?;
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
    // A Syntaxfehlermeldung names the error in `UCI` DE 0085 (CONTRL AHB
    // 1.0 Kap. 3.1). `12` „Ungültiger Wert" is the catch-all the ingest uses
    // when the parser reported no finer code.
    builder = if accepted {
        builder.accept()
    } else {
        builder.reject(
            p.get("syntax_error")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("12"),
        )
    };

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
