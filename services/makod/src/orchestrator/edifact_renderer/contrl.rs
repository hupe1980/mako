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
/// | `messages`       | no       | [`NachrichtenFehler`] list: a Syntaxfehlermeldung **in der Nachricht** |
/// | `message_ref`    | no       | Derived from `causation_event_id` when absent                |
///
/// A non-empty `messages` renders the Kap. 3 column „Syntaxfehlermeldung in der
/// Nachricht": `UCI` DE 0083 = `4` with no DE 0085, then per message an
/// `SG1 UCM` (DE 0062, S009, DE 0083 = `4`, and DE 0085 + DE 0013 for a fault
/// in `UNH`/`UNT`) followed by its `SG2 UCS` (DE 0096, DE 0085 only when no
/// `UCD` follows) and `UCD` (DE 0085, S011 DE 0098 / DE 0104).
///
/// [`NachrichtenFehler`]: crate::contrl_ack::NachrichtenFehler
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
    let fehler: Vec<crate::contrl_ack::NachrichtenFehler> = match p.get("messages") {
        Some(v) if !v.is_null() => {
            serde_json::from_value(v.clone()).map_err(|e| RenderError::MissingField {
                message_type: mt.into(),
                field: format!("messages (SG1 UCM report): {e}").into(),
            })?
        }
        _ => Vec::new(),
    };
    if !fehler.is_empty() {
        if accepted {
            return Err(RenderError::MissingField {
                message_type: mt.into(),
                field: "accepted = false: a per-message report rejects the file \
                        (UCI DE 0083 = 4)"
                    .into(),
            });
        }
        // UCI DE 0083 = 4 with no DE 0085: the fault is named further down.
        let head = builder
            .action_code("4")
            .serialize()
            .map_err(|e| RenderError::BuilderError(e.to_string()))?;
        let message = splice_message_reports(&head, &fehler)?;
        return finish_interchange(Ok(message), sender, receiver, msg);
    }
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

/// Insert the `SG1 UCM` / `SG2 UCS` / `UCD` report before the `UNT` of a
/// rendered `UNH`…`UCI`…`UNT` message, and recount `UNT` DE 0074.
fn splice_message_reports(
    message: &[u8],
    fehler: &[crate::contrl_ack::NachrichtenFehler],
) -> Result<Vec<u8>, RenderError> {
    use crate::contrl_ack::Fehlerort;
    use edifact_rs::{OwnedElement, OwnedSegment};

    let text =
        std::str::from_utf8(message).map_err(|e| RenderError::BuilderError(e.to_string()))?;
    let unt = text
        .rfind("UNT+")
        .ok_or_else(|| RenderError::BuilderError("rendered CONTRL has no UNT".into()))?;
    let head = &text[..unt];
    let reference = text[unt..]
        .trim_end_matches('\'')
        .rsplit('+')
        .next()
        .unwrap_or_default()
        .to_owned();

    let mut report: Vec<OwnedSegment> = Vec::new();
    for f in fehler {
        let mut s009 = vec![
            f.message_type.clone(),
            f.version.clone(),
            f.release.clone(),
            f.agency.clone(),
        ];
        if let Some(association) = &f.association {
            s009.push(association.clone());
        }
        let mut ucm = vec![
            OwnedElement::of(std::slice::from_ref(&f.message_ref)),
            OwnedElement::of(&s009),
            OwnedElement::of(&["4"]),
        ];
        if let Fehlerort::Nachricht {
            code,
            service_segment,
        } = &f.fehler
        {
            ucm.push(OwnedElement::of(std::slice::from_ref(code)));
            ucm.push(OwnedElement::of(std::slice::from_ref(service_segment)));
        }
        report.push(OwnedSegment::new("UCM", ucm));
        if let Fehlerort::Segment {
            position,
            code,
            datenelement,
        } = &f.fehler
        {
            let mut ucs = vec![OwnedElement::of(&[position.to_string()])];
            if datenelement.is_none()
                && let Some(code) = code
            {
                ucs.push(OwnedElement::of(std::slice::from_ref(code)));
            }
            report.push(OwnedSegment::new("UCS", ucs));
            if let Some(de) = datenelement {
                let mut s011 = vec![de.position.to_string()];
                if let Some(k) = de.komponente {
                    s011.push(k.to_string());
                }
                report.push(OwnedSegment::new(
                    "UCD",
                    vec![
                        OwnedElement::of(std::slice::from_ref(&de.code)),
                        OwnedElement::of(&s011),
                    ],
                ));
            }
        }
    }
    let head_segments = head.matches('\'').count();
    let count = (head_segments + report.len() + 1).to_string();
    report.push(OwnedSegment::new(
        "UNT",
        vec![OwnedElement::of(&[count]), OwnedElement::of(&[reference])],
    ));
    let tail = edifact_rs::segments_to_bytes(&report)
        .map_err(|e| RenderError::BuilderError(e.to_string()))?;
    let mut out = head.as_bytes().to_vec();
    out.extend_from_slice(&tail);
    Ok(out)
}
