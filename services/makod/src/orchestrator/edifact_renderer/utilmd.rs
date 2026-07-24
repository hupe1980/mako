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
/// | `sender`        | yes      | Sender GLN (our own)                          |
/// | `receiver`      | no       | Receiver GLN (falls back to `msg.recipient`)  |
/// | `malo`          | yes*     | Marktlokations-ID (GPKE/GeLi Gas PIDs)        |
/// | `melo`          | yes*     | Messlokations-ID (WiM PIDs 55039, 55042, 55051, 55168) |
/// | `process_date`  | yes      | Process date (`YYYYMMDD` or `YYYY-MM-DD`)     |
/// | `document_date` | no       | Document date (defaults to today at dispatch time)     |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent          |
///
/// \* Exactly one of `malo` / `melo` is required, depending on the PID range.
pub(super) fn render_utilmd(
    p: &serde_json::Value,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "UTILMD";

    let pid = require_u32(p, mt, "pid")?;
    let sender = require_str(p, mt, "sender")?;
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    // The AHB fixes the IDE DE 7495 qualifier per Prüfidentifikator: the WiM
    // Messlokations-PIDs use `24` (Vorgang), everything else (GPKE 55xxx,
    // GeLi Gas 44xxx — Marktlokations processes) uses `Z19`, matching the
    // official Beispiel fixtures and the generated AHB rules.
    let (ide_qualifier, location_id_key) = if matches!(pid, 55_039 | 55_042 | 55_051 | 55_168) {
        ("24", "melo")
    } else {
        ("Z19", "malo")
    };
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

    let mut builder = builders::UtilmdBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .pruefidentifikator(edifact_pid)
        // AHB: RFF+Z13 is mandatory on every UTILMD Anwendungsfall — it
        // carries the process reference the counterparty echoes back.
        .rff("Z13", message_ref.clone())
        .message_ref(message_ref.clone());

    if let Some(dd) = doc_date_owned.as_deref() {
        builder = builder.document_date(dd);
    }

    let mut tx = builder
        .transaction_with_qualifier(ide_qualifier, location_id)
        .process_date(dtm_qualifier, &process_date_yyyymmdd);
    // SG4 STS Transaktionsgrund (e.g. EoG cause codes Z36/ZT6/ZC7, §38 EnWG).
    if let Some(grund) = p.get("transaktionsgrund").and_then(|v| v.as_str()) {
        tx = tx.status(grund);
    }
    finish_interchange(tx.done().serialize(), sender, receiver, msg)
}

/// Returns the BDEW DTM qualifier for the process-date segment inside UTILMD SG4.
///
/// | PID range      | Process           | Qualifier | Meaning             |
/// |----------------|-------------------|-----------|---------------------|
/// | 55001, 44001   | Lieferbeginn      | 163       | Delivery start      |
/// | 55002, 44002   | Lieferende        | 164       | Delivery end        |
/// | 55016          | Kündigung         | 163       | Cancellation date   |
/// | 55039, 55042, 55051, 55168 | WiM Messstellenbetrieb | 163       | Execution date      |
/// | 44003–44006    | GeLi Gas Antwort  | 163       | Confirmation date   |
/// | _              | fallback          | 163       | Delivery start      |
pub(super) fn utilmd_dtm_qualifier(pid: u32) -> &'static str {
    match pid {
        55001 | 44001 => "163",                 // Lieferbeginn
        55002 | 44002 => "164",                 // Lieferende
        55013..=55015 | 44013..=44015 => "163", // EoG Zuordnungsbeginn (§38 EnWG)
        55016 => "163",                         // Kündigung Lieferbeginn (inbound, LFN → LFA)
        55017 | 55018 => "163",                 // Bestätigung/Ablehnung Kündigung (LFA → LFN)
        55039 | 55042 | 55051 | 55168 => "163", // WiM Messstellenbetrieb
        44003..=44006 => "163",                 // GeLi Gas confirmation/rejection
        _ => "163",
    }
}
