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

/// Render an INSRPT status/inspection message from its [`InsrptPayload`].
///
/// [`InsrptPayload`]: mako_engine::payload::InsrptPayload
pub(super) fn render_insrpt(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "INSRPT";
    let p: mako_engine::payload::InsrptPayload = super::typed_payload(p)?;

    let pid = checked_pid(Some(p.pid), mt)?.unwrap_or(p.pid);
    if p.melo.is_empty() {
        return Err(RenderError::MissingField {
            message_type: mt.into(),
            field: "melo".into(),
        });
    }
    let sender = text(&p.sender).unwrap_or_else(|| registry.primary_mp_id());
    let receiver = text(&p.receiver).unwrap_or(msg.recipient.as_ref());

    let release = release_for(msg, MessageType::Insrpt, ReleaseTrack::Short)?;

    let message_ref = text(&p.message_ref)
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
        .position(text(&p.position).unwrap_or("1").to_owned())
        // `SG7 STS+Z06` Gerätestatus: `Z11` gestört (the Störungsmeldung's
        // own state), `Z12` the other one the column admits.
        .status(text(&p.status_code).unwrap_or("Z11").to_owned())
        // `SG5 NAD+MS` + `CTA`/`COM` — the sender's Ansprechpartner, Muss.
        .contact(
            text(&p.contact).unwrap_or("Service"),
            text(&p.contact_comm).unwrap_or(sender),
        )
        .location("172", &p.melo);

    // `SG3 DOC` is mandatory and DE 1001 `21` the one code the column admits;
    // its reference defaults to the process' own PID so a caller with no
    // external Förderreferenz still produces a valid message.
    builder = builder.doc_reference(
        "21".to_owned(),
        text(&p.doc_reference).map_or_else(|| pid.to_string(), str::to_owned),
    );

    if let Some(d) = text(&p.document_date) {
        builder = builder.document_date(dokumentdatum_303("INSRPT", d)?);
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
