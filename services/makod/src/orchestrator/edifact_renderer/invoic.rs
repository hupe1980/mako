//! INVOIC / REMADV renderers.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
// ── INVOIC ──────────────────────────────────────────────────────────────────

/// Wrap the INVOIC an issuing process enqueued in its interchange.
///
/// The message itself was built from the BO4E `Rechnung` and validated against
/// the Prüfschablone of its PID at the command (`orchestrator::invoic_ausgang`),
/// before `SendInvoic` recorded it; this renderer builds nothing and only adds
/// `UNB` … `UNZ` around it, with the sender and receiver its `NAD+MS` /
/// `NAD+MR` carry.
///
/// Payload fields:
///
/// | Field       | Required | Description                                     |
/// |-------------|----------|-------------------------------------------------|
/// | `nachricht` | yes      | The INVOIC, `UNH` … `UNT`, as EDIFACT text       |
/// | `sender`    | yes      | Sender MP-ID — `NAD+MS`, `UNB` DE 0004           |
/// | `receiver`  | yes      | Receiver MP-ID — `NAD+MR`, `UNB` DE 0010         |
///
/// An entry without a `nachricht` is refused: an INVOIC without its positions,
/// tax and totals is not an invoice, and putting one on the wire is worse than
/// sending none.
pub(super) fn render_invoic(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    _registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let p: mako_engine::payload::InvoicPayload = super::typed_payload(p)?;
    for (field, value) in [
        ("nachricht", &p.nachricht),
        ("sender", &p.sender),
        ("receiver", &p.receiver),
    ] {
        if value.is_empty() {
            return Err(RenderError::MissingField {
                message_type: "INVOIC".into(),
                field: field.into(),
            });
        }
    }
    finish_interchange(
        Ok(p.nachricht.as_bytes().to_vec()),
        &p.sender,
        &p.receiver,
        msg,
    )
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
/// | `ablehnungsgrund` | no     | `SG7 FTX+ABO` beside the Kopf-`AJT` (`[548]`)  |
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
    let p: mako_engine::payload::RemadvPayload = super::typed_payload(p)?;

    let sender = text(&p.sender).unwrap_or_else(|| registry.primary_mp_id());
    let receiver = text(&p.receiver).unwrap_or(msg.recipient.as_ref());
    let document_id = text(&p.document_id);
    let document_code = text(&p.document_code);
    let doc_date = text(&p.document_date)
        .map(|d| dokumentdatum_303(mt, d))
        .transpose()?;
    let message_ref = text(&p.message_ref)
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = release_for(msg, MessageType::Remadv, ReleaseTrack::Short)?;

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
    if let Some(pid) = checked_pid(p.pid, mt)? {
        builder = builder.pruefidentifikator(pid.to_string());
    }
    if let Some(w) = text(&p.waehrung) {
        builder = builder.waehrung(w);
    }
    // `SG5` — DOC, MOA+9, MOA+12, DTM+137. All Muss.
    if let Some(r) = &p.rechnungsbezug {
        builder = builder.rechnungsbezug(builders::Rechnungsbezug {
            dokumentenart: r.dokumentenart.clone(),
            rechnungsnummer: r.rechnungsnummer.clone(),
            faelliger_betrag: r.faelliger_betrag.clone(),
            ueberweisungsbetrag: r.ueberweisungsbetrag.clone(),
            // `SG5 DTM+137` is `303`: the invoice's own document date, a
            // calendar date becoming the instant that German day begins.
            rechnungsdatum: dokumentdatum_303(mt, &r.rechnungsdatum)?,
        });
    }
    // `SG10 DLI` + `SG12 AJT` — the Positionsebene. `invoicd` puts every
    // Befund of the walk in `antwort_befunde` with its Ebene and, on a
    // position-level one, its Positionsnummer; `SG10` is Muss on 33004 and is
    // repeated „bis alle Fehler der Positionsebene genannt sind".
    let ebd = text(&p.antwort_codeliste);
    let positionsfehler = position_level_befunde(&p.antwort_befunde, ebd);
    if !positionsfehler.is_empty() {
        builder = builder.positionsfehler(positionsfehler);
    }
    // `SG7 AJT` — the Abweichungsgrund. An Abweisung without one gives the
    // invoice sender nothing to correct; `invoicd` computes the reason and it
    // must reach the wire.
    if let Some(code) = text(&p.antwort_code) {
        let grund = match ebd {
            Some(ebd) => builders::Abweichungsgrund::new(code, ebd),
            None => builders::Abweichungsgrund {
                code: code.to_owned(),
                ebd: None,
                erlaeuterung: None,
            },
        };
        // `SG7 FTX+ABO` — the explanation the EBD asks for (`[548]`); invoicd
        // states it in German and within `an..512`.
        let grund = match text(&p.ablehnungsgrund) {
            Some(text) if !text.trim().is_empty() => grund.mit_erlaeuterung(text),
            _ => grund,
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
    befunde: &[mako_engine::payload::RemadvBefund],
    ebd: Option<&str>,
) -> Vec<builders::Positionsfehler> {
    let mut out: Vec<builders::Positionsfehler> = Vec::new();
    for b in befunde {
        let Some(nr) = b.positionsnummer else {
            continue;
        };
        let code = b.code.as_str();
        let grund = match ebd {
            Some(ebd) => builders::Abweichungsgrund::new(code, ebd),
            None => builders::Abweichungsgrund {
                code: code.to_owned(),
                ebd: None,
                erlaeuterung: None,
            },
        };
        let detail = b.detail.clone();
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
