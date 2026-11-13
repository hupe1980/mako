//! IFTSTA renderer — WiM status messages.
//!
//! Every outbound WiM IFTSTA is a `BGM+Z09` „WiM Meldung(en)" — the GPKE
//! Bearbeitungsstandsmeldung a `BGM+Z33` „GPKE Meldung(en)" — and every one names
//! its Prüfidentifikator in SG15 `RFF+Z13`. What differs is the **Planungs-
//! status**: `SG15 STS` DE 9015 says which subject the message reports on, and
//! DE 4405 the state that subject reached. Two Anwendungsfall families are
//! rendered here:
//!
//! | PIDs | Kapitel | `STS` DE 9015 | DE 4405 | Also carries |
//! |---|---|---|---|---|
//! | 21029 / 21030 / 21031 | 6.7 Ersteinbau eines iMS | `Z19` Ersteinbau iMS | `Z17` geplant · `Z30` zugestimmt · `Z31` widersprochen | `SG14 LOC+172` Meldepunkt (Muss), `SG15 DTM+76` geplanter Umstellungszeitpunkt, and on the two answers the `E_0233` Prüfschritt in DE 9013/1131 and `SG15 RFF+ACW` / `RFF+ADY` the 21029 answered (S. 33) |
//! | 21025 / 21027 | 6.4 Messstellenumbaustatus (MSB → LF resp. NB / MSB, S. 24–27) | `Z15` Messstellenumbau | `Z13` gescheitert · `Z14` erfolgreich | `SG14 LOC+172`; DE 9013 `Z74`/`Z75` with no Codeliste, or an `E_0286` code under it, and beside `A99:E_0286` the `SG25 GID`/`FTX+ACB` |
//! | 21009 / 21010 | 6.2 Status des Gesamtvorgangs (MSBN → NB) | `Z10` MSB-Wechsel | `Z13` gescheitert · `Z14` erfolgreich | `SG14 LOC+172`; on 21010 `SG15 DTM+293` the takeover instant |
//! | 21011 / 21012 | 6.2 (NB → MSBN / MSBA / LF) | `Z10` | `Z13` · `Z14` | `SG15 RFF+ACW` / `RFF+ADY` the report answered; on 21011 the `E_0232` / `E_2003` code under `S_0057` / `G_0055`; on 21012 `DTM+293` |
//! | 21013 | 6.2 (NB → MSBN / MSBA / LF, WiM Teil 1 Kap. 2.3.2 Nr. 16–18) | `Z10` | `Z13` | `SG14 LOC+172`, DE 9013 `E17` with no Codeliste |
//! | 21042 | 6.10 Umsetzungsstatus (UC 4.4, MSB → ESA) | `Z21` Bestellung | `105` beendet | `SG15 RFF+AGI` Beantragungsnummer, `SG15 DTM+93` Vertragsende |
//! | 21047 | 7.7 Bearbeitungsstandsmeldung (GPKE, S. 61–63) | `Z43` Veränderungsstatus der Daten | `Z47` Änderung · `Z48` keine Änderung der Daten | `BGM+Z33`, the EBD code in DE 9013/1131, `SG15 RFF+ACW` the answered UTILMD's `IDE` DE 7402, and beside `A99` under `Z48` the `SG25 GID`/`FTX+ACB` |
//!
//! Rendering one family in the other's shape produces a well-formed message
//! about the wrong subject: a Vorabinformation zum Ersteinbau that says
//! „Bestellung beendet" and names no Messlokation. So a PID without an arm is
//! refused, never given a neighbour's shape.
//!
//! Source: IFTSTA AHB 2.1 (01.04.2026) Kap. 6.2 / 6.7 / 6.10 / 7.7.

use super::*;

/// Render an IFTSTA status message from its [`IftstaPayload`].
///
/// [`IftstaPayload`]: mako_engine::payload::IftstaPayload
pub(super) fn render_iftsta(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "IFTSTA";
    let p: mako_engine::payload::IftstaPayload = super::typed_payload(p)?;
    let missing = |field: &'static str| RenderError::MissingField {
        message_type: mt.into(),
        field: field.into(),
    };

    let pid = p.pid;
    let sender = p
        .sender
        .as_deref()
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p.receiver.as_deref().unwrap_or(msg.recipient.as_ref());

    let release = release_for(msg, MessageType::Iftsta, ReleaseTrack::Short)?;

    let message_ref = p
        .message_ref
        .as_deref()
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    // `SG15 STS` DE 9015 / DE 4405 — the Planungsstatus. Per-PID rather than a
    // default, because the pair *is* what the message says: `Z21`/`105` states
    // „Bestellung beendet", which on an Ersteinbau Anwendungsfall is a
    // well-formed statement about something the process never touched.
    let (sts_category, default_reason) = match pid {
        21_009 | 21_011 | 21_013 => ("Z10", Some("Z13")), // MSB-Wechsel / gescheitert
        21_010 | 21_012 => ("Z10", Some("Z14")),          // MSB-Wechsel / erfolgreich
        21_025 | 21_027 => ("Z15", Some("Z13")),          // Messstellenumbau / gescheitert
        21_029 => ("Z19", Some("Z17")),                   // Ersteinbau iMS / geplant
        21_030 => ("Z19", Some("Z30")),                   // Ersteinbau iMS / zugestimmt
        21_031 => ("Z19", Some("Z31")),                   // Ersteinbau iMS / widersprochen
        21_042 => ("Z21", Some("105")),                   // Bestellung / beendet — UC 4.4
        // Veränderungsstatus der Daten — `Z47` / `Z48` is the answer itself,
        // so the producer states it and nothing defaults it.
        21_047 => ("Z43", None),
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
    let sts_code = match (text(&p.sts_code), default_reason) {
        (Some(code), _) | (None, Some(code)) => code,
        (None, None) => return Err(missing("sts_code (SG15 STS DE 4405)")),
    };

    let mut builder = builders::IftstaBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(if pid == 21_047 { "Z33" } else { "Z09" })
        .pruefidentifikator(pid)
        .status(sts_category, sts_code)
        .vorgangsnummer("1");

    // `SG15 STS` DE 9013 — the 21013 column fixes `E17` and names no Codeliste.
    if pid == 21_013 {
        builder = builder.status_code("E17");
    }
    // The Gesamtvorgang answers name the report they answer: `RFF+ACW` its
    // `BGM` DE 1004, `RFF+ADY` its `CNI` DE 1490 (Muss on 21011 / 21012, and
    // on the Ersteinbau answers 21030 / 21031 naming the 21029, Kap. 6.7,
    // S. 33), and 21011 carries its `E_0232` / `E_2003` code — DE 9013 Muss.
    if matches!(pid, 21_011 | 21_012 | 21_030 | 21_031) {
        builder = builder
            .referenz_nachricht(
                text(&p.referenz_nachricht).ok_or_else(|| missing("referenz_nachricht"))?,
            )
            .referenz_vorgang(
                text(&p.referenz_vorgang).ok_or_else(|| missing("referenz_vorgang"))?,
            );
    }
    // 21025 / 21027 (Kap. 6.4, S. 25–26, one table): DE 9013 is Muss — an `E_0286` code under
    // that Codeliste (`[153]`), or `Z74` / `Z75` with none (`[103]`) — and
    // `SG25` with its `FTX+ACB` rides exactly beside `A99:E_0286` (`[154]`).
    if matches!(pid, 21_025 | 21_027) {
        let code =
            text(&p.antwort_code).ok_or_else(|| missing("antwort_code (SG15 STS DE 9013)"))?;
        match text(&p.antwort_codeliste) {
            Some(liste) => {
                builder = builder.pruefschritt(code, liste);
                if code == "A99" && liste == "E_0286" {
                    builder = builder.freitext(
                        text(&p.bemerkung)
                            .ok_or_else(|| missing("bemerkung (SG25 FTX+ACB beside A99)"))?,
                    );
                }
            }
            None => builder = builder.status_code(code),
        }
    }
    if matches!(pid, 21_011 | 21_047) {
        builder = builder.pruefschritt(
            text(&p.antwort_code).ok_or_else(|| missing("antwort_code"))?,
            text(&p.antwort_codeliste).ok_or_else(|| missing("antwort_codeliste"))?,
        );
    }
    // 21047 (IFTSTA AHB 2.1 Kap. 7.7, S. 61–63): `SG15 RFF+ACW` is Muss and
    // carries the answered UTILMD's `IDE` DE 7402 (`[535]`); `SG25` with its
    // `FTX+ACB` is Muss exactly when the answer is `STS+Z43+Z48+A99:E_0595`
    // (`[148]`), and in no other case.
    if pid == 21_047 {
        builder = builder.referenz_nachricht(text(&p.referenz_nachricht).ok_or_else(|| {
            missing("referenz_nachricht (SG15 RFF+ACW — the UTILMD's IDE DE 7402)")
        })?);
        if sts_code == "Z48" && text(&p.antwort_code) == Some("A99") {
            builder = builder.freitext(
                text(&p.bemerkung).ok_or_else(|| missing("bemerkung (SG25 FTX+ACB beside A99)"))?,
            );
        }
    }
    // `SG15 DTM+293` — on 21010 / 21012 the instant the MSBN takes over, the
    // German day's beginning in its Sparte (`[UB3]`, `[521]`).
    if matches!(pid, 21_010 | 21_012) {
        let sparte = match p.sparte.ok_or_else(|| missing("sparte"))? {
            mako_engine::types::Sparte::Gas => mako_fristen::Sparte::Gas,
            mako_engine::types::Sparte::Strom => mako_fristen::Sparte::Strom,
        };
        let beginn = text(&p.zuordnungsbeginn).ok_or_else(|| missing("zuordnungsbeginn"))?;
        builder = builder.fertigstellung(prozessdatum_303(mt, "zuordnungsbeginn", beginn, sparte)?);
    }
    // `SG14 LOC+172` — Muss on the Ersteinbau Anwendungsfälle and 21013, the only
    // thing that says which Messlokation is being rebuilt. `172` is IFTSTA's
    // one DE 3227 value in both Sparten.
    if let Some(zp) = text(&p.melo) {
        builder = builder.meldepunkt(zp);
    }
    // `SG15 RFF+AVE` — the Marktlokation, Muss on a 21029 addressed to an LF.
    if let Some(malo) = text(&p.malo) {
        builder = builder.marktlokation(malo);
    }
    // `SG15 DTM+76` — the planned Umstellungszeitpunkt.
    if let Some(geplant) = text(&p.umstellungszeitpunkt) {
        builder = builder.leistungsbeginn(normalise_date(geplant));
    }
    // `SG15 STS` DE 9013 / DE 1131 — the Prüfschritt and its Codeliste. The two
    // answers of an Ersteinbau carry an `E_0233` code whose cluster the AHB
    // pins to the Anwendungsfall: Zustimmung on 21030, Ablehnung on 21031.
    if !matches!(pid, 21_011 | 21_025 | 21_027 | 21_047)
        && let (Some(code), Some(liste)) = (text(&p.antwort_code), text(&p.antwort_codeliste))
    {
        builder = builder.pruefschritt(code, liste);
    }

    if let Some(id) = text(&p.document_id) {
        builder = builder.document_id(id);
    }
    if let Some(d) = text(&p.document_date) {
        builder = builder.document_date(dokumentdatum_303(mt, d)?);
    }
    if let Some(order_ref) = text(&p.korrelation_ref) {
        builder = builder.order_reference(order_ref);
    }
    if let Some(ende) = text(&p.beendigung_zum) {
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
