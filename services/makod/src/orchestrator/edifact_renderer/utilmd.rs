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
/// | `antwort_ebd`   | no       | `STS+E01` DE 1131, the EBD the code comes from |
/// | `bemerkung`     | no       | `FTX+ACB` free text (mandatory alongside a catch-all Ablehnungscode) |
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

    // WiM Messlokations-PIDs name a MeLo; everything else names a MaLo.
    let names_messlokation = matches!(pid, 55_039 | 55_042 | 55_051 | 55_168);
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
    let release = active_release(MessageType::Utilmd, &track).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
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

    // `SG4 STS+7` — Transaktionsgrund plus its Ergänzung. The AHB marks the
    // Ergänzung Muss wherever the Grund is, and `ZW4` (verbrauchende
    // Marktlokation) is the case every GPKE/GeLi Gas core process describes
    // unless the workflow says otherwise.
    if let Some(grund) = p.get("transaktionsgrund").and_then(|v| v.as_str()) {
        let erg = p
            .get("transaktionsgrund_ergaenzung")
            .and_then(|v| v.as_str())
            .unwrap_or(ergaenzung::VERBRAUCHENDE_MALO);
        tx = tx.transaktionsgrund(Transaktionsgrund::new(grund, erg));
    }

    // `SG4 STS+E01` — the EBD Antwortcode. Without it a Bestätigung or
    // Ablehnung is not a well-formed answer: the AHB marks the segment Muss and
    // constrains the code to the named EBD's cluster.
    if let Some(code) = p.get("antwort_code").and_then(|v| v.as_str()) {
        let antwort = match p.get("antwort_ebd").and_then(|v| v.as_str()) {
            Some(ebd) => AntwortStatus::from_ebd(code, ebd),
            None => AntwortStatus::bare(code),
        };
        tx = tx.antwort(antwort);
    }

    // `FTX+ACB` Bemerkung — mandatory alongside the catch-all Ablehnungscodes
    // (`A99` Strom, `E14` Gas), which require a written Erläuterung.
    if let Some(text) = p.get("bemerkung").and_then(|v| v.as_str()) {
        tx = tx.free_text("ACB", text);
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
        // Stammdatenänderung (GPKE Teil 4 / GeLi Gas): Änderung zum.
        55_109 | 55_110 | 55_136 | 55_137 | 55_600..=55_699 => dtm::AENDERUNG_ZUM,
        44_109..=44_199 => dtm::AENDERUNG_ZUM,
        // WiM Messstellenbetrieb: the planned execution date.
        55_039..=55_053 | 55_168..=55_170 => dtm::LEISTUNGSBEGINN_GEPLANT,
        // A PID with no entry here would otherwise get a silently wrong
        // qualifier; `Beginn zum` is the least surprising default and the
        // `utilmd_dtm_qualifier_covers_every_rendered_pid` test keeps the list
        // honest for everything mako actually sends.
        _ => dtm::BEGINN_ZUM,
    }
}
