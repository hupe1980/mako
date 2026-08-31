//! INVOIC / REMADV renderers.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
// ── INVOIC ────────────────────────────────────────────────────────────────────

/// Render an INVOIC (Invoice) envelope from domain-intent JSON.
///
/// This produces a valid EDIFACT envelope with header segments (UNH, BGM, DTM,
/// NAD+MS, NAD+MR, UNT). The UNS+D detail section is intentionally empty —
/// invoices requiring line items and amounts must be rendered by the billing
/// module that has access to the BO4E Rechnung data.
///
/// The empty-detail INVOIC is conformant at the EDIFACT interchange level;
/// the receiving system will respond with REMADV acknowledging receipt.
///
/// Payload fields:
///
/// | Field           | Required | Description                                   |
/// |-----------------|----------|-----------------------------------------------|
/// | `sender`        | no       | Sender MP-ID (falls back to `tenant_party_id`)  |
/// | `receiver`      | no       | Receiver MP-ID (falls back to `msg.recipient`)  |
/// | `document_id`   | no       | BGM document identifier (Rechnungsnummer)     |
/// | `document_code` | no       | BGM type code (default `"380"`)               |
/// | `document_date` | no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent |
/// | `antwort_code`  | no       | `SG7 AJT` DE 4465 — the Antwortcode on an Abweisung |
/// | `antwort_codeliste`   | no       | `SG7 AJT` DE 1082 — the EBD the code comes from |
/// | `ablehnungsgrund` | no     | Free-text reason, rendered as `FTX+ACB`        |
///
/// # An Abweisung must state why
///
/// `AJT` is what carries the reason: DE 4465 the code, DE 1082 the EBD. The
/// REMADV twin of UTILMD's `STS+E01++<code>:<ebd>`.
pub(super) fn render_invoic(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "INVOIC";

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let document_id = p.get("document_id").and_then(|v| v.as_str());
    let document_code = p.get("document_code").and_then(|v| v.as_str());
    let doc_date = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Invoic, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::InvoicBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    if let Some(id) = document_id {
        builder = builder.document_id(id);
    }
    if let Some(code) = document_code {
        builder = builder.document_code(code);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }
    finish_interchange(builder.serialize(), sender, receiver, msg)
}

// ── REMADV ────────────────────────────────────────────────────────────────────

/// Render a REMADV (Remittance Advice) envelope from domain-intent JSON.
///
/// Produces a valid EDIFACT envelope (UNH, BGM, DTM, NAD+MS, NAD+MR, UNT)
/// that acknowledges receipt and acceptance of a billing document. The detail
/// section (amounts, references) must be added by the billing module.
///
/// Payload fields:
///
/// | Field           | Required | Description                                   |
/// |-----------------|----------|-----------------------------------------------|
/// | `sender`        | no       | Sender MP-ID (falls back to `registry.primary_mp_id()`)|
/// | `receiver`      | no       | Receiver MP-ID (falls back to `msg.recipient`)  |
/// | `document_id`   | no       | BGM document identifier (Avisnummer)          |
/// | `document_code` | no       | BGM type code (default `"239"`)               |
/// | `document_date` | no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent |
/// | `pid`           | no       | `RFF+Z13` — 33001 / 33002 / 33003 / 33004    |
/// | `waehrung`      | no       | `SG4 CUX` DE 6345 (default `EUR`)             |
/// | `rechnungsbezug`| no       | `SG5` — the answered invoice and its amounts  |
/// | `antwort_code`  | no       | `SG7 AJT` DE 4465 — the Antwortcode on an Abweisung |
/// | `antwort_codeliste`   | no       | `SG7 AJT` DE 1082 — the EBD the code comes from |
/// | `ablehnungsgrund` | no     | Free-text reason, rendered as `FTX+ACB`        |
///
/// # An Abweisung must state why
///
/// `AJT` is what carries the reason: DE 4465 the code, DE 1082 the EBD. The
/// REMADV twin of UTILMD's `STS+E01++<code>:<ebd>`.
///
/// # What the answer has to name
///
/// A REMADV that names no Prüfidentifikator reaches no process on the other
/// side, and one that names no invoice answers nothing: `RFF+Z13` and the `SG5`
/// block (`DOC`, `MOA+9`, `MOA+12`, `DTM+137`) are **Muss** on every use case
/// REMADV AHB 1.0a publishes. `mako_invoic` fills them from the stored BO4E
/// `Rechnung` — the payer side keeps it precisely so the answer need not go
/// back to the EDIFACT archive.
pub(super) fn render_remadv(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "REMADV";

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let document_id = p.get("document_id").and_then(|v| v.as_str());
    let document_code = p.get("document_code").and_then(|v| v.as_str());
    let doc_date = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Remadv, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::RemadvBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    if let Some(id) = document_id {
        builder = builder.document_id(id);
    }
    if let Some(code) = document_code {
        builder = builder.document_code(code);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }
    // `RFF+Z13` — the Prüfidentifikator the receiving system routes on.
    if let Some(pid) = p.get("pid").and_then(serde_json::Value::as_u64) {
        builder = builder.pruefidentifikator(pid.to_string());
    }
    if let Some(w) = p.get("waehrung").and_then(|v| v.as_str()) {
        builder = builder.waehrung(w);
    }
    // `SG5` — DOC, MOA+9, MOA+12, DTM+137. All Muss.
    if let Some(r) = p.get("rechnungsbezug").filter(|v| v.is_object()) {
        let field = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        builder = builder.rechnungsbezug(builders::Rechnungsbezug {
            dokumentenart: field("dokumentenart"),
            rechnungsnummer: field("rechnungsnummer"),
            faelliger_betrag: field("faelliger_betrag"),
            ueberweisungsbetrag: field("ueberweisungsbetrag"),
            rechnungsdatum: normalise_date(&field("rechnungsdatum")),
        });
    }
    // `SG10 DLI` + `SG12 AJT` — the Positionsebene. `invoicd` puts every
    // Befund of the walk in `antwort_befunde` with its Ebene and, on a
    // position-level one, its Positionsnummer; `SG10` is Muss on 33004 and is
    // repeated „bis alle Fehler der Positionsebene genannt sind".
    let ebd = p.get("antwort_codeliste").and_then(|v| v.as_str());
    let positionsfehler = position_level_befunde(p, ebd);
    if !positionsfehler.is_empty() {
        builder = builder.positionsfehler(positionsfehler);
    }
    // `SG7 AJT` — the Abweichungsgrund. An Abweisung without one gives the
    // invoice sender nothing to correct; `invoicd` computes the reason and it
    // must reach the wire.
    if let Some(code) = p.get("antwort_code").and_then(|v| v.as_str()) {
        let grund = match ebd {
            Some(ebd) => builders::Abweichungsgrund::new(code, ebd),
            None => builders::Abweichungsgrund {
                code: code.to_owned(),
                ebd: None,
            },
        };
        builder = builder.abweichungsgrund(grund);
    }
    finish_interchange(builder.serialize(), sender, receiver, msg)
}

/// Group `antwort_befunde` into one [`builders::Positionsfehler`] per
/// Positionsnummer, in wire order.
///
/// Kopf- and Summen-level Befunde are deliberately skipped: they ride `SG7`,
/// which the caller emits once. Only a Befund that names a Positionsnummer
/// belongs in `SG10`/`SG12`.
fn position_level_befunde(
    p: &serde_json::Value,
    ebd: Option<&str>,
) -> Vec<builders::Positionsfehler> {
    let mut out: Vec<builders::Positionsfehler> = Vec::new();
    for b in p
        .get("antwort_befunde")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(code), Some(nr)) = (
            b.get("code").and_then(|v| v.as_str()),
            b.get("positionsnummer")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u16::try_from(n).ok()),
        ) else {
            continue;
        };
        let grund = match ebd {
            Some(ebd) => builders::Abweichungsgrund::new(code, ebd),
            None => builders::Abweichungsgrund {
                code: code.to_owned(),
                ebd: None,
            },
        };
        let detail = b.get("detail").and_then(|v| v.as_str()).map(str::to_owned);
        match out.iter_mut().find(|pf| pf.positionsnummer == nr) {
            Some(pf) => {
                pf.gruende.push(grund);
                pf.erlaeuterung = pf.erlaeuterung.take().or(detail);
            }
            None => out.push(builders::Positionsfehler {
                positionsnummer: nr,
                gruende: vec![grund],
                erlaeuterung: detail,
            }),
        }
    }
    out
}
