//! UTILMD renderer.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
// ── UTILMD ────────────────────────────────────────────────────────────────────

/// Render a UTILMD outbound message from domain-intent JSON.
///
/// Payload fields (all sourced from workflow `handle` implementations):
///
/// | Field           | Required | Description                                  |
/// |-----------------|----------|----------------------------------------------|
/// | `pid`           | yes      | Prüfidentifikator (u32)                       |
/// | `sender`        | yes      | Sender MP-ID (our own)                        |
/// | `receiver`      | no       | Receiver MP-ID (falls back to `msg.recipient`) |
/// | `malo` / `melo` | yes*     | Lokations-ID → `SG5 LOC+Z16` / `LOC+Z17`      |
/// | `vorgangsnummer`| no       | `IDE+24` DE 7402 (defaults to the message ref) |
/// | `referenz_vorgangsnummer` | on answers | `SG4 SG6 RFF+TN` — the **request's** `IDE+24` |
/// | `process_date`  | yes      | Process date (`YYYYMMDD` or `YYYY-MM-DD`)     |
/// | `document_date` | no       | Document date (defaults to today at dispatch time) |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent  |
/// | `transaktionsgrund` | no   | `SG4 STS+7` DE 9013 element 2                  |
/// | `transaktionsgrund_ergaenzung` | no | `STS+7` DE 9013 element 3 (`ZW3`…`ZAP`); defaults to `ZW4` when a Grund is present |
/// | `antwort_code`  | no       | `SG4 STS+E01` DE 9013 — **required on every Antwort-PID** |
/// | `antwort_codeliste` | no   | `STS+E01` DE 1131, the **Codeliste** the code comes from (`E_0622`, `S_0090`, `G_0051`, …) |
/// | `bemerkung`     | no       | `FTX+ACB` free text (mandatory alongside a catch-all Ablehnungscode) |
/// | `bilanzkreis`   | on 55001/55014/55608, 44001 | Strom: `SG8 SEQ+Z79` Produktpaket · Gas: `SG10 CCI+Z19` — its own slot, never `bemerkung` |
///
/// \* Exactly one of `malo` / `melo` is required, depending on the PID range.
///
/// # What the MIG fixes here
///
/// `IDE` DE 7495 has exactly two values (`24` Vorgang, `Z01` Liste) and DE 7402
/// carries a **Vorgangsnummer** — the Lokations-ID belongs in `SG5 LOC`. The SG4
/// date qualifiers are `92`/`93`/`157`/`76`, never the Messperioden-Qualifier
/// `163`/`164`.
///
/// The Prüfidentifikator travels in `SG4 SG6 RFF+Z13`, „genau einmal je SG4 IDE
/// (Vorgang) anzugeben"; the builder emits it. An answer additionally carries
/// `SG4 SG6 RFF+TN` with the request's Vorgangsnummer, because DE 7402 must be
/// globally unique and so the answer cannot reuse the requester's.
pub(super) fn render_utilmd(
    p: &serde_json::Value,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
    use edi_energy::utilmd_codes::{AntwortStatus, Transaktionsgrund, ergaenzung};

    let mt = "UTILMD";

    let pid = require_u32(p, mt, "pid")?;
    let sender = require_str(p, mt, "sender")?;
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    // The MSB is assigned to the **Messlokation**, never to the Marktlokation
    // („Der MSB ist ausschließlich dem Objekt Messlokation zugeordnet" — WiM
    // Strom Teil 1 Kap. 2.1.2 d, AWH WiM Gas 2.0 Kap. 3.1.2 d), so every WiM
    // MSB-Wechsel message names a MeLo in `SG5 LOC` — the Anfrage and both
    // answers, in both Sparten. Everything else names a MaLo.
    let names_messlokation = mako_wim::geraetewechsel::wim_sparte(pid).is_some()
        || mako_wim::antwort_pid_meaning(pid)
            .is_some_and(|(request, _)| mako_wim::geraetewechsel::wim_sparte(request).is_some());
    let location_id_key = if names_messlokation { "melo" } else { "malo" };
    let location_id = require_str(p, mt, location_id_key)?;

    let process_date = require_str(p, mt, "process_date")?;

    let doc_date_owned = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    // Determine UTILMD release track from PID: 44xxx = Gas, everything else = Strom.
    let track = if (44_000..=44_999).contains(&pid) {
        ReleaseTrack::Gas
    } else {
        ReleaseTrack::Strom
    };
    let release =
        active_release(MessageType::Utilmd, track).ok_or_else(|| RenderError::NoActiveProfile {
            message_type: mt.into(),
        })?;

    let edifact_pid = Pruefidentifikator::new(pid).map_err(|e| RenderError::MissingField {
        message_type: mt.into(),
        field: format!("pid value {pid} is invalid: {e}").into(),
    })?;

    let dtm_qualifier = utilmd_dtm_qualifier(pid);
    let process_date_yyyymmdd = normalise_date(process_date);

    // `SG4 SG6 RFF+Z13` carries the Prüfidentifikator and the builder emits it
    // per Vorgang from `pruefidentifikator` — DE 1154 is `R n5`, so nothing but
    // the five-digit code belongs there.
    let mut builder = builders::UtilmdBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .pruefidentifikator(edifact_pid)
        .message_ref(message_ref.clone());

    if let Some(dd) = doc_date_owned.as_deref() {
        builder = builder.document_date(dd);
    }

    // `IDE+24` DE 7402. The workflow may supply its own Vorgangsnummer; the
    // message reference is a serviceable default because it is already unique
    // per outbound message and is what the counterparty echoes in RFF.
    let vorgangsnummer = p
        .get("vorgangsnummer")
        .and_then(|v| v.as_str())
        .unwrap_or(message_ref.as_str());

    let mut tx = builder
        .transaction(vorgangsnummer)
        .date(dtm_qualifier, &process_date_yyyymmdd);

    // `SG4 SG6 RFF+TN` — „Referenz Vorgangsnummer (aus Anfragenachricht)",
    // Muss on every Antwortnachricht (UTILMD AHB Strom 2.2 / Gas 1.2). The
    // answer's own `IDE+24` must be a fresh number, so this is the only thing
    // that ties it to the request.
    if let Some(referenz) = p
        .get("referenz_vorgangsnummer")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        tx = tx.referenz_vorgangsnummer(referenz);
    }

    // `SG4 STS+7` — Transaktionsgrund, and in the GPKE/GeLi Gas processes its
    // Ergänzung. The AHB marks the Ergänzung Muss wherever the Grund is there,
    // and `ZW4` (verbrauchende Marktlokation) is the case every core process
    // describes unless the workflow says otherwise.
    //
    // **The WiM MSB-Wechsel has no Ergänzung.** Its Anwendungsübersichten list
    // `SG4 STS 9015 = 7` and `SG4 STS 9013 = E01|E02|E03|…` and nothing else
    // (UTILMD AHB Strom 2.2 Kap. 10, Gas 1.2 Kap. 6). Defaulting `ZW4` onto a
    // Messlokations-Vorgang states „verbrauchende Marktlokation" in an element
    // the Anwendungsfall does not define.
    if let Some(grund) = p.get("transaktionsgrund").and_then(|v| v.as_str()) {
        let t = if names_messlokation {
            Transaktionsgrund::bare(grund)
        } else {
            let erg = p
                .get("transaktionsgrund_ergaenzung")
                .and_then(|v| v.as_str())
                .unwrap_or(ergaenzung::VERBRAUCHENDE_MALO);
            Transaktionsgrund::new(grund, erg)
        };
        tx = tx.transaktionsgrund(t);
    }

    // `SG4 STS+E01` — the Prüfschritt code and the Codeliste it comes from.
    // Without it a Bestätigung or Ablehnung is not a well-formed answer: the
    // AHB marks the segment Muss and constrains the code to that Codeliste's
    // cluster. DE 1131 is the Codeliste identifier, which is the EBD number
    // only where the AHB says „EBD Nr." — every WiM MSB-Wechsel answer names an
    // `S_00xx` or `G_00xx` list instead.
    if let Some(code) = p.get("antwort_code").and_then(|v| v.as_str()) {
        let antwort = match p.get("antwort_codeliste").and_then(|v| v.as_str()) {
            Some(cl) => AntwortStatus::from_codeliste(code, cl),
            None => AntwortStatus::bare(code),
        };
        tx = tx.antwort(antwort);
    }

    // `FTX+ACB` Bemerkung — mandatory alongside the catch-all Ablehnungscodes
    // (`A99` Strom, `E14` Gas), which require a written Erläuterung.
    if let Some(text) = p.get("bemerkung").and_then(|v| v.as_str()) {
        tx = tx.free_text("ACB", text);
    }

    // The Bilanzkreis — **two different segments, one per Festlegung**.
    //
    // GPKE Strom carries it in the Produktpaket `SG8 SEQ+Z79` with Produkt-Code
    // `9991000002082` and the value in `SG10 CAV+ZV4` (UTILMD AHB Strom 2.2
    // Kap. 5.3, Codeliste der Konfigurationen 1.4 Kap. 6.1.1), Muss on 55001,
    // 55077, 55600, 55601, 55014 and 55608. GeLi Gas has no Produktpaket at
    // all: UTILMD AHB Gas 1.2 puts the Bilanzkreis in `SG10 CCI+Z19` DE 7037,
    // Muss on 44001. Sending either shape on the other Sparte is a segment the
    // receiving AHB does not define.
    //
    // Neither is an `FTX+ACB` remark: on a Strom Zuordnungs-Bestätigung that
    // segment is admitted on the Ablehnung only (Bedingung [48]).
    if let Some(bilanzkreis) = p
        .get("bilanzkreis")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        tx = match track {
            ReleaseTrack::Gas => tx.merkmal(
                edi_energy::utilmd_codes::produkt::CCI_BILANZKREIS_GAS,
                bilanzkreis,
            ),
            _ => tx.produktpaket(edi_energy::utilmd_codes::Produktpaket::bilanzkreis(
                bilanzkreis,
            )),
        };
    }

    let tx = if names_messlokation {
        tx.messlokation(location_id)
    } else {
        tx.marktlokation(location_id)
    };

    finish_interchange(tx.done().serialize(), sender, receiver, msg)
}

/// The `SG4 DTM` DE 2005 qualifier for the process date of a given PID.
///
/// | Process | Qualifier | MIG name |
/// |---|---|---|
/// | Anmeldung / Lieferbeginn | `92` | Beginn zum (Datum Vertragsbeginn) |
/// | Abmeldung / Lieferende / Beendigung der Zuordnung | `93` | Ende zum (Datum Vertragsende) |
/// | Kündigung | `93` | Ende zum — the Kündigungstermin |
/// | Stammdatenänderung | `157` | Änderung zum, Gültigkeit Beginndatum |
/// | WiM Messstellenbetrieb | `76` | Datum zum geplanten Leistungsbeginn |
///
/// `163`/`164` appear nowhere in this table: the MIG uses them for *Beginn* and
/// *Ende Messperiode* inside SG8/SG9, not for a SG4 process date.
pub(super) fn utilmd_dtm_qualifier(pid: u32) -> &'static str {
    use edi_energy::utilmd_codes::dtm;
    match pid {
        // Lieferbeginn: Anmeldung and its Bestätigung/Ablehnung.
        55_001..=55_003 | 55_013..=55_015 | 55_077 | 55_078 | 55_080 => dtm::BEGINN_ZUM,
        44_001..=44_003 | 44_013..=44_015 => dtm::BEGINN_ZUM,
        // Lieferende von LF an NB, Lieferende von NB an LF, Beendigung der
        // Zuordnung, Kündigung — every one of them names a Vertragsende.
        55_004..=55_012 | 55_016..=55_018 => dtm::ENDE_ZUM,
        44_004..=44_012 | 44_016..=44_018 => dtm::ENDE_ZUM,
        // ── WiM Messstellenbetrieb ────────────────────────────────────────
        //
        // **Three different qualifiers, not one.** A Kündigung rendered with
        // `DTM+76` names a Leistungsbeginn where the AHB requires a
        // Vertragsende, and the receiver rejects it.
        //
        // | Anwendungsfall | Qualifier | AHB |
        // |---|---|---|
        // | Kündigung Messstellenbetrieb | `93` Datum Vertragsende ¹ | Strom 2.2 Kap. 10.1 / Gas 1.2 Kap. 6.1 |
        // | Anmeldung Messstellenbetrieb | `76` Lieferdatum/-zeit, geplant | Strom 2.2 Kap. 10.2 / Gas 1.2 Kap. 6.2 |
        // | Ende Messstellenbetrieb | `93` Datum Vertragsende ² | Strom 2.2 Kap. 10.4 / Gas 1.2 Kap. 6.4 |
        // | Verpflichtungsanfrage | `76` Lieferdatum/-zeit, geplant | Strom 2.2 Kap. 10.3 / Gas 1.2 Kap. 6.5 |
        //
        // ¹ XOR `DTM+471` „Ende zum nächstmöglichem Termin", which the workflow
        //   sets explicitly when the Kündigung names no fixed date; the
        //   Ablehnung additionally carries `157`/`Z01`/`Z10` under `Z12`.
        // ² alongside `DTM+92` Datum Vertragsbeginn, which the Abmeldung also
        //   carries.
        55_039..=55_041 | 44_039..=44_041 => dtm::ENDE_ZUM,
        55_051..=55_053 | 44_051..=44_053 | 44_183 => dtm::ENDE_ZUM,
        55_042..=55_044 | 44_042..=44_044 => dtm::LEISTUNGSBEGINN_GEPLANT,
        55_168..=55_170 | 44_168 | 44_169 => dtm::LEISTUNGSBEGINN_GEPLANT,
        // Two families inside the 556xx block are GPKE **Teil 2**, not Teil 4,
        // and both mark `SG4 DTM+92` „Datum Vertragsbeginn" Muss: Neuanlage
        // (55600–55605) and Ankündigung Zuordnung LF (55607/55608). Their AHB
        // does not define `157` at all. 55609 carries no SG4 date; the one
        // emitted here is an unlisted segment, not a missing Muss.
        55_600..=55_609 => dtm::BEGINN_ZUM,
        // Stammdatenänderung (GPKE Teil 4 / GeLi Gas): Änderung zum.
        55_109 | 55_110 | 55_136 | 55_137 | 55_610..=55_699 => dtm::AENDERUNG_ZUM,
        44_109..=44_199 => dtm::AENDERUNG_ZUM,
        // A PID with no entry here would otherwise get a silently wrong
        // qualifier; `Beginn zum` is the least surprising default and the
        // `utilmd_dtm_qualifier_covers_every_rendered_pid` test keeps the list
        // honest for everything mako actually sends.
        _ => dtm::BEGINN_ZUM,
    }
}
