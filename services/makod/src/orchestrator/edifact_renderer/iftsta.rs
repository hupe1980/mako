//! IFTSTA renderer — WiM status messages.
//!
//! Every outbound IFTSTA is a `BGM+Z09` „WiM Meldung(en)", and every one names
//! its Prüfidentifikator in SG15 `RFF+Z13`. What differs is the **Planungs-
//! status**: `SG15 STS` DE 9015 says which subject the message reports on, and
//! DE 4405 the state that subject reached. Two Anwendungsfall families are
//! rendered here:
//!
//! | PIDs | Kapitel | `STS` DE 9015 | DE 4405 | Also carries |
//! |---|---|---|---|---|
//! | 21029 / 21030 / 21031 | 6.7 Ersteinbau eines iMS | `Z19` Ersteinbau iMS | `Z17` geplant · `Z30` zugestimmt · `Z31` widersprochen | `SG14 LOC+172` Meldepunkt (Muss), `SG15 DTM+76` geplanter Umstellungszeitpunkt, and on the two answers the `E_0233` Prüfschritt in DE 9013/1131 |
//! | 21042 | 6.10 Umsetzungsstatus (UC 4.4, MSB → ESA) | `Z21` Bestellung | `105` beendet | `SG15 RFF+AGI` Beantragungsnummer, `SG15 DTM+93` Vertragsende |
//!
//! Rendering one family in the other's shape produces a well-formed message
//! about the wrong subject: a Vorabinformation zum Ersteinbau that says
//! „Bestellung beendet" and names no Messlokation.
//!
//! Source: IFTSTA AHB 2.1 (01.04.2026) Kap. 6.7 / 6.10.

use super::*;

/// Render an IFTSTA WiM status message from domain-intent JSON.
///
/// | Field             | Required | Description                                    |
/// |-------------------|----------|------------------------------------------------|
/// | `pid`             | yes      | Prüfidentifikator → SG15 `RFF+Z13`             |
/// | `sender`          | yes      | Sender (MSB) MP-ID                             |
/// | `receiver`        | no       | Receiver (ESA) MP-ID (falls back to `msg.recipient`) |
/// | `sts_code`        | no       | STS DE4405 status reason; defaults per Anwendungsfall (see the table above) |
/// | `melo`            | on 21029–21031 | Zählpunktbezeichnung → SG14 `LOC+172` (Muss) |
/// | `malo`            | on a 21029 to an LF | Marktlokations-ID → SG15 `RFF+AVE` |
/// | `umstellungszeitpunkt` | on 21029/21030 | geplanter Umstellungszeitpunkt → SG15 `DTM+76` |
/// | `antwort_code` + `antwort_codeliste` | on 21030/21031 | `E_0233` Prüfschritt → SG15 `STS` DE 9013/1131 |
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

    let release = active_release(MessageType::Iftsta, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    // `SG15 STS` DE 9015 / DE 4405 — the Planungsstatus. Per-PID rather than a
    // default, because the pair *is* what the message says: `Z21`/`105` states
    // „Bestellung beendet", which on an Ersteinbau Anwendungsfall is a
    // well-formed statement about something the process never touched.
    let (sts_category, default_reason) = match pid {
        21_029 => ("Z19", "Z17"), // Ersteinbau iMS / geplant
        21_030 => ("Z19", "Z30"), // Ersteinbau iMS / zugestimmt
        21_031 => ("Z19", "Z31"), // Ersteinbau iMS / widersprochen
        _ => ("Z21", "105"),      // Bestellung / beendet — UC 4.4 Umsetzungsstatus
    };
    let sts_code = p
        .get("sts_code")
        .and_then(|v| v.as_str())
        .unwrap_or(default_reason);

    let mut builder = builders::IftstaBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code("Z09")
        .pruefidentifikator(pid)
        .status(sts_category, sts_code)
        .vorgangsnummer("1");

    // `SG14 LOC+172` — Muss on the Ersteinbau Anwendungsfälle, and the only
    // thing that says which Messlokation is being rebuilt. `172` is IFTSTA's
    // one DE 3227 value in both Sparten, so the Zählpunktbezeichnung rides it
    // whichever key the workflow used.
    if let Some(zp) = p
        .get("melo")
        .or_else(|| p.get("mabis_zaehlpunkt"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        builder = builder.meldepunkt(zp);
    }
    // `SG15 RFF+AVE` — the Marktlokation, Muss on a 21029 addressed to an LF.
    if let Some(malo) = p
        .get("malo")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        builder = builder.marktlokation(malo);
    }
    // `SG15 DTM+76` — the planned Umstellungszeitpunkt.
    if let Some(geplant) = p
        .get("umstellungszeitpunkt")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        builder = builder.leistungsbeginn(normalise_date(geplant));
    }
    // `SG15 STS` DE 9013 / DE 1131 — the Prüfschritt and its Codeliste. The two
    // answers of an Ersteinbau carry an `E_0233` code whose cluster the AHB
    // pins to the Anwendungsfall: Zustimmung on 21030, Ablehnung on 21031.
    if let (Some(code), Some(liste)) = (
        p.get("antwort_code").and_then(|v| v.as_str()),
        p.get("antwort_codeliste").and_then(|v| v.as_str()),
    ) {
        builder = builder.pruefschritt(code, liste);
    }

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
