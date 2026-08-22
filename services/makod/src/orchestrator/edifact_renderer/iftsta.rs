//! IFTSTA renderer — WiM status messages.
//!
//! IFTSTA was inbound-only until the WiM Teil 2 UC 4.4 „Beendigung durch MSB"
//! (MSB → ESA) path needed an outbound status message. This renders PID **21042**
//! (WiM / Umsetzungsstatus, „Bestellung (WiM)"; IFTSTA AHB 2.0g Kap. 6.10) with
//! `BGM+Z09`, SG14 `CNI` Vorgangsnummer, SG15 `STS` 9015=Z21 / 4405=105
//! („beendet"), SG15 `RFF+Z13` Prüfidentifikator, SG15 `RFF+AGI` Beantragungs-
//! nummer (the Bestellung this ends), and SG15 `DTM+93` Vertragsende.

use super::*;

/// Render an IFTSTA WiM status message from domain-intent JSON.
///
/// | Field             | Required | Description                                    |
/// |-------------------|----------|------------------------------------------------|
/// | `pid`             | yes      | Prüfidentifikator (21042) → SG15 `RFF+Z13`     |
/// | `sender`          | yes      | Sender (MSB) MP-ID                             |
/// | `receiver`        | no       | Receiver (ESA) MP-ID (falls back to `msg.recipient`) |
/// | `sts_code`        | no       | STS DE4405 status reason (default `105` „beendet") |
/// | `korrelation_ref` | no       | Belegnummer of the Bestellung → SG15 `RFF+AGI` (`ZG-T47`) |
/// | `beendigung_zum`  | no       | Vertragsende date → SG15 `DTM+93`              |
/// | `document_id`     | no       | BGM Dokumentennummer                           |
/// | `document_date`   | no       | DTM+137 date                                   |
/// | `message_ref`     | no       | Derived from `causation_event_id` when absent  |
pub(super) fn render_iftsta(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "IFTSTA";

    let pid = require_u32(p, mt, "pid")?;
    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    let release = active_release(MessageType::Iftsta, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    // STS 9015 = Z21 „Bestellung"; 4405 = the status reason (default 105 „beendet").
    let sts_code = p.get("sts_code").and_then(|v| v.as_str()).unwrap_or("105");

    let mut builder = builders::IftstaBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code("Z09")
        .pruefidentifikator(pid)
        .status("Z21", sts_code)
        .vorgangsnummer("1");

    if let Some(id) = p.get("document_id").and_then(|v| v.as_str()) {
        builder = builder.document_id(id);
    }
    if let Some(d) = p.get("document_date").and_then(|v| v.as_str()) {
        builder = builder.document_date(normalise_date(d));
    }
    if let Some(order_ref) = p
        .get("korrelation_ref")
        .or_else(|| p.get("order_reference"))
        .and_then(|v| v.as_str())
    {
        builder = builder.order_reference(order_ref);
    }
    if let Some(ende) = p.get("beendigung_zum").and_then(|v| v.as_str()) {
        // May arrive RFC 3339 (`2026-08-01T00:00:00Z`) or bare date — take the
        // date part and strip dashes for the `DTM+93` CCYYMMDD form.
        let date = ende.split('T').next().unwrap_or(ende);
        builder = builder.vertragsende(normalise_date(date));
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
