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
//! | 21009 / 21010 | 6.2 Status des Gesamtvorgangs (MSBN → NB) | `Z10` MSB-Wechsel | `Z13` gescheitert · `Z14` erfolgreich | `SG14 LOC+172`; on 21010 `SG15 DTM+293` the takeover instant |
//! | 21011 / 21012 | 6.2 (NB → MSBN / MSBA / LF) | `Z10` | `Z13` · `Z14` | `SG15 RFF+ACW` / `RFF+ADY` the report answered; on 21011 the `E_0232` / `E_2003` code under `S_0057` / `G_0055`; on 21012 `DTM+293` |
//! | 21013 | 6.2 (NB → MSBN / MSBA / LF, WiM Teil 1 Kap. 2.3.2 Nr. 16–18) | `Z10` | `Z13` | `SG14 LOC+172`, DE 9013 `E17` with no Codeliste |
//! | 21042 | 6.10 Umsetzungsstatus (UC 4.4, MSB → ESA) | `Z21` Bestellung | `105` beendet | `SG15 RFF+AGI` Beantragungsnummer, `SG15 DTM+93` Vertragsende |
//!
//! Rendering one family in the other's shape produces a well-formed message
//! about the wrong subject: a Vorabinformation zum Ersteinbau that says
//! „Bestellung beendet" and names no Messlokation. So a PID without an arm is
//! refused, never given a neighbour's shape.
//!
//! Source: IFTSTA AHB 2.1 (01.04.2026) Kap. 6.2 / 6.7 / 6.10.

use super::*;

/// Render an IFTSTA WiM status message from domain-intent JSON.
///
/// | Field             | Required | Description                                    |
/// |-------------------|----------|------------------------------------------------|
/// | `pid`             | yes      | Prüfidentifikator → SG15 `RFF+Z13`; one with no arm is refused |
/// | `referenz_nachricht` + `referenz_vorgang` | on 21011/21012 | `BGM` DE 1004 / `CNI` DE 1490 of the report answered → `RFF+ACW` / `RFF+ADY` |
/// | `zuordnungsbeginn` + `sparte` | on 21010/21012 | takeover day → `DTM+293` (`303`, the day's beginning in the Sparte) |
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

    let release = release_for(msg, MessageType::Iftsta, ReleaseTrack::Short)?;

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
        21_009 | 21_011 | 21_013 => ("Z10", "Z13"), // MSB-Wechsel / gescheitert
        21_010 | 21_012 => ("Z10", "Z14"),          // MSB-Wechsel / erfolgreich
        21_029 => ("Z19", "Z17"),                   // Ersteinbau iMS / geplant
        21_030 => ("Z19", "Z30"),                   // Ersteinbau iMS / zugestimmt
        21_031 => ("Z19", "Z31"),                   // Ersteinbau iMS / widersprochen
        21_042 => ("Z21", "105"),                   // Bestellung / beendet — UC 4.4
        other => {
            return Err(RenderError::InsufficientPayload {
                message_type: mt.into(),
                detail: format!(
                    "IFTSTA {other} has no renderer arm — its AHB column is not built, and \
                     no other Anwendungsfall's shape stands in for it"
                )
                .into(),
            });
        }
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

    // `SG15 STS` DE 9013 — the 21013 column fixes `E17` and names no Codeliste.
    if pid == 21_013 {
        builder = builder.status_code("E17");
    }
    // The Gesamtvorgang answers name the report they answer: `RFF+ACW` its
    // `BGM` DE 1004, `RFF+ADY` its `CNI` DE 1490 (Muss on 21011 / 21012), and
    // 21011 carries its `E_0232` / `E_2003` code — DE 9013 Muss.
    if matches!(pid, 21_011 | 21_012) {
        builder = builder
            .referenz_nachricht(require_str(p, mt, "referenz_nachricht")?)
            .referenz_vorgang(require_str(p, mt, "referenz_vorgang")?);
    }
    if pid == 21_011 {
        builder = builder.pruefschritt(
            require_str(p, mt, "antwort_code")?,
            require_str(p, mt, "antwort_codeliste")?,
        );
    }
    // `SG15 DTM+293` — on 21010 / 21012 the instant the MSBN takes over, the
    // German day's beginning in its Sparte (`[UB3]`, `[521]`).
    if matches!(pid, 21_010 | 21_012) {
        let sparte = match require_str(p, mt, "sparte")? {
            "gas" => mako_fristen::Sparte::Gas,
            "strom" => mako_fristen::Sparte::Strom,
            other => {
                return Err(RenderError::InsufficientPayload {
                    message_type: mt.into(),
                    detail: format!("sparte {other:?} is neither \"strom\" nor \"gas\"").into(),
                });
            }
        };
        let beginn = require_str(p, mt, "zuordnungsbeginn")?;
        builder = builder.fertigstellung(prozessdatum_303(mt, "zuordnungsbeginn", beginn, sparte)?);
    }
    // `SG14 LOC+172` — Muss on the Ersteinbau Anwendungsfälle and 21013, the only
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
        builder = builder.document_date(dokumentdatum_303(mt, d)?);
    }
    if let Some(order_ref) = p
        .get("korrelation_ref")
        .or_else(|| p.get("order_reference"))
        .and_then(|v| v.as_str())
    {
        builder = builder.order_reference(order_ref);
    }
    if let Some(ende) = p.get("beendigung_zum").and_then(|v| v.as_str()) {
        // May arrive RFC 3339 (`2026-08-01T00:00:00Z`) or as a bare date; the
        // date part is the German Vertragsende, and `DTM+93` is `303`, so it
        // goes out as the UTC instant that day begins. 21042 is MSB → ESA, a
        // Strom role.
        builder = builder.vertragsende(prozessdatum_303(
            mt,
            "beendigung_zum",
            ende,
            mako_fristen::Sparte::Strom,
        )?);
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
