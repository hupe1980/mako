//! ORDERS / REQOTE / ORDCHG / ORDRSP / QUOTES renderers.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
// ── ORDERS ────────────────────────────────────────────────────────────────────

/// Render an ORDERS (Beauftragung) message from domain-intent JSON.
///
/// **Sender resolution** (in priority order):
/// 1. `payload["sender"]` — set this in the workflow for deterministic
///    multi-GLN deployments.
/// 2. [`MpIdRegistry::sender_mp_id_for_orders_pid`] — static PID → role lookup.
/// 3. [`MpIdRegistry::primary_mp_id`] — final fallback.
///
/// The receiver comes from `msg.recipient`.
///
/// Payload fields:
///
/// | Field        | Required | Description                                  |
/// |--------------|----------|----------------------------------------------|
/// | `sender`     | no       | Sender GLN (overrides registry lookup)       |
/// | `pid`        | no       | ORDERS Prüfidentifikator (e.g. 17134)        |
/// | `orders_ref` | no       | UUID reference → 14-char UNH message ref     |
/// | `malo`       | no       | Supply point MaLo for BGM context            |
/// Render an ESA-originated REQOTE 35002 Werteanfrage (WiM Teil 2 UC 4.1 Nr. 1).
///
/// The PID travels in BGM DE 1004 (`document_id`) and the addressed location in
/// `LOC+172`, so the MSB can correlate the request to a Marktlokation.
pub(super) fn render_reqote(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "REQOTE";

    let pid = p.get("pid").and_then(|v| v.as_u64()).map(|n| n as u32);

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());

    let explicit_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid);
    let causation_ref = msg_ref_from_uuid(&msg.causation_event_id.to_string());
    let message_ref = explicit_ref.as_deref().unwrap_or(causation_ref.as_str());

    let release = active_release(MessageType::Reqote, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::ReqoteBuilder::new(release)
        .sender(sender)
        .receiver(msg.recipient.as_ref())
        .message_ref(message_ref);

    if let Some(pv) = pid {
        builder = builder.document_id(pv.to_string());
    }
    if let Some(loc) = p.get("location").and_then(|v| v.as_str())
        && !loc.is_empty()
    {
        builder = builder.location(loc);
    }

    // ── ESA Werteanfrage (PID 35002) full-conformance content ────────────────
    // 35002 mandates SG1 RFF, SG14 CTA/COM and an SG27 LIN on top of the shared
    // skeleton. Emitted only for 35002 so other REQOTE PIDs are untouched.
    if pid == Some(35002) {
        builder = builder.reference("Z13", "35002");
        let contact = p
            .get("contact")
            .and_then(|v| v.as_str())
            .unwrap_or("ESA-Service");
        let comm = p
            .get("contact_comm")
            .and_then(|v| v.as_str())
            .unwrap_or("esa@example.de");
        builder = builder.contact(contact, comm).line_item();
    }

    finish_interchange(builder.serialize(), sender, msg.recipient.as_ref(), msg)
}

pub(super) fn render_orders(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "ORDERS";

    let pid = p.get("pid").and_then(|v| v.as_u64()).map(|n| n as u32);

    // Sender: explicit in payload first, then registry lookup by PID, then primary.
    let sender = p.get("sender").and_then(|v| v.as_str()).unwrap_or_else(|| {
        pid.map(|p| registry.sender_mp_id_for_orders_pid(p))
            .unwrap_or_else(|| registry.primary_mp_id())
    });

    // Prefer the caller's Belegnummer (`message_ref`) so the wire UNH reference
    // matches the key the ESA registered its process under — the MSB's ORDRSP
    // answer echoes this reference and it is how a LOC-less answer correlates.
    let explicit_ref = p
        .get("message_ref")
        .or_else(|| p.get("orders_ref"))
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid);
    let causation_ref = msg_ref_from_uuid(&msg.causation_event_id.to_string());
    let message_ref = explicit_ref.as_deref().unwrap_or(causation_ref.as_str());

    let release = active_release(MessageType::Orders, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::OrdersBuilder::new(release)
        .sender(sender)
        .receiver(msg.recipient.as_ref())
        .message_ref(message_ref);

    if let Some(pv) = pid {
        builder = builder.document_id(pv.to_string());
    }
    // ESA-originated Bestellung/Abbestellung carries the location in LOC.
    if let Some(loc) = p.get("location").and_then(|v| v.as_str())
        && !loc.is_empty()
    {
        builder = builder.location(loc);
    }
    // ── ESA Bestellung/Abbestellung (17007/17008) full conformance ───────────
    // A valid BGM 1001 (Z55–Z64), the mandatory SG1 RFF+Z13 and an IMD, emitted
    // only for the ESA order PIDs so the many other ORDERS PIDs are untouched.
    if pid == Some(17007) || pid == Some(17008) {
        builder = builder
            .document_code("Z56")
            .reference("Z13", pid.map_or_else(String::new, |p| p.to_string()))
            .item_description("Werte nach Typ 2");
    }

    finish_interchange(builder.serialize(), sender, msg.recipient.as_ref(), msg)
}

// ── ORDCHG ────────────────────────────────────────────────────────────────────

/// Render an ORDCHG (Purchase Order Change) from domain-intent JSON.
///
/// Used for Stornierung of a pending order — chiefly PID 39000 (Stornierung
/// Sperr-/Entsperrauftrag, LF → NB) emitted by `gpke-sperrung-lf`, and PID 39002
/// (Stornierung der Bestellung, ESA → MSB).
///
/// Payload keys: `pid` (u32, required for the document ID), `sender` (optional —
/// falls back to the registry), `message_ref` (optional — falls back to the
/// causation event ID). `receiver` is always `msg.recipient`.
pub(super) fn render_ordchg(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "ORDCHG";

    let pid = p.get("pid").and_then(|v| v.as_u64()).map(|n| n as u32);

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());

    let explicit_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid);
    let causation_ref = msg_ref_from_uuid(&msg.causation_event_id.to_string());
    let message_ref = explicit_ref.as_deref().unwrap_or(causation_ref.as_str());

    // ORDCHG BDEW releases are `1.x` (no trailing letter), which parse to the
    // `Other` track rather than `Short` — asking for `Short` here silently
    // returned NoActiveProfile for every ORDCHG (39000/39001/39002).
    let release = active_release(MessageType::Ordchg, &ReleaseTrack::Other).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::OrdchgBuilder::new(release)
        .sender(sender)
        .receiver(msg.recipient.as_ref())
        .message_ref(message_ref);

    if let Some(pv) = pid {
        builder = builder.document_id(pv.to_string());
    }
    // ORDCHG mandates an SG1 RFF and carries no LOC. Reference the original
    // order (`ON` = its message reference) when the caller supplies one, else
    // fall back to the Prüfidentifikator (`Z13`). The ESA Stornierung's target
    // MaLo travels via this reference, not a location segment.
    if let Some(order_ref) = p.get("order_reference").and_then(|v| v.as_str())
        && !order_ref.is_empty()
    {
        builder = builder.reference("ON", order_ref);
    } else if let Some(pv) = pid {
        builder = builder.reference("Z13", pv.to_string());
    }

    finish_interchange(builder.serialize(), sender, msg.recipient.as_ref(), msg)
}

// ── ORDRSP ────────────────────────────────────────────────────────────────────

/// Render an ORDRSP (Purchase Order Response) from domain-intent JSON.
///
/// Used for WiM Stornierung responses (PIDs 39001/39002), WiM Geräteübernahme
/// responses (PIDs 17003/17004), and any other ORDERS-response workflow paths.
///
/// Payload fields:
///
/// | Field          | Required | Description                                   |
/// |----------------|----------|-----------------------------------------------|
/// | `sender`       | no       | Sender GLN (falls back to `registry.primary_mp_id()`)|
/// | `receiver`     | no       | Receiver GLN (falls back to `msg.recipient`)  |
/// | `document_id`  | no       | BGM document identifier (Auftragsnummer)      |
/// | `document_date`| no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `message_ref`  | no       | Derived from `causation_event_id` when absent |
pub(super) fn render_ordrsp(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "ORDRSP";

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let document_id = p.get("document_id").and_then(|v| v.as_str());
    let doc_date = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Ordrsp, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::OrdrespBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    // ESA / MaKo: the Prüfidentifikator (BGM DE 1004) is the routing key of the
    // answer — 19011/19012 (Ab-/Bestellung) or 19013/19014 (Stornierung).
    if let Some(pid) = p.get("pid").and_then(serde_json::Value::as_u64) {
        builder = builder.pruefidentifikator(pid as u32);
    }
    if let Some(id) = document_id {
        builder = builder.document_id(id);
    }
    // Reference the original ORDERS/ORDCHG this ORDRSP answers.
    if let Some(order_ref) = p
        .get("order_reference")
        .or_else(|| p.get("orig_message_ref"))
        .and_then(serde_json::Value::as_str)
    {
        builder = builder.order_reference(order_ref);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }

    // ── ESA answer (19011-19014) full conformance ────────────────────────────
    // ORDRSP carries no LOC — the ESA correlates by the RFF+ACW echo above.
    // AJT is mandatory for all four; 19011/19012 also need IMD; 19011 (a
    // Bestätigung) additionally carries an SG27 LIN + FTX.
    let esa_pid = p.get("pid").and_then(serde_json::Value::as_u64);
    if matches!(esa_pid, Some(19011..=19014)) {
        builder = builder.adjustment("Z10");
        if matches!(esa_pid, Some(19011 | 19012)) {
            builder = builder.item_description();
        }
        if esa_pid == Some(19011) {
            // Bestätigung: an SG2 coded reason (FTX) plus an SG27 line item.
            builder = builder.adjustment_reason("Z27").line_item();
        }
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}

/// Render a QUOTES (Angebot) envelope — the MSB's answer to an ESA Werteanfrage
/// (REQOTE 35002), UC 4.1 Nr. 2. Prüfidentifikator 15003 in BGM DE 1004.
///
/// Payload fields:
///
/// | Field             | Required | Description                                  |
/// |-------------------|----------|----------------------------------------------|
/// | `pid`             | yes      | Prüfidentifikator (15003)                    |
/// | `sender`          | no       | Sender MP-ID (falls back to primary)         |
/// | `receiver`        | no       | Receiver MP-ID (falls back to `msg.recipient`)|
/// | `document_id`     | no       | BGM document id (Angebotsnummer)             |
/// | `order_reference` | no       | RFF+ACW — the REQOTE this answers            |
/// | `document_date`   | no       | DTM+137 date                                 |
/// | `message_ref`     | no       | Derived from `causation_event_id` when absent|
pub(super) fn render_quotes(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "QUOTES";

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let doc_date = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Quotes, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::QuotesBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    if let Some(pid) = p.get("pid").and_then(serde_json::Value::as_u64) {
        builder = builder.pruefidentifikator(pid as u32);
    }
    if let Some(id) = p.get("document_id").and_then(serde_json::Value::as_str) {
        builder = builder.document_id(id);
    }
    if let Some(order_ref) = p
        .get("order_reference")
        .or_else(|| p.get("orig_message_ref"))
        .and_then(serde_json::Value::as_str)
    {
        builder = builder.order_reference(order_ref);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }
    // Echo the location so an ESA can correlate this answer to its process.
    if let Some(loc) = p.get("location").and_then(|v| v.as_str())
        && !loc.is_empty()
    {
        builder = builder.location(loc);
    }
    // Bindungsfrist (Angebot only) — its presence distinguishes an Angebot from
    // an Anfrage-Ablehnung on the ESA inbound side.
    if let Some(bf) = p.get("bindungsfrist").and_then(|v| v.as_str())
        && !bf.is_empty()
    {
        builder = builder.bindungsfrist(bf);
    }
    // Ablehnungsgrund (Ablehnung der Anfrage) — free text.
    if let Some(reason) = p.get("reason").and_then(|v| v.as_str())
        && !reason.is_empty()
    {
        builder = builder.reason(reason);
    }

    // ── ESA Angebot (PID 15003) full-conformance content ─────────────────────
    // 15003 mandates SG1 RFF, SG4 CUX, SG14 CTA/COM, SG27 LIN/PIA, SG31 PRI on
    // top of the shared skeleton. Emit them (payload-supplied, sensible
    // defaults) only for 15003, so the Geräteübernahme Angebote (15001/15002)
    // that share this renderer stay untouched. An Ablehnung (reason set) carries
    // no Angebot content.
    let is_angebot = p.get("pid").and_then(serde_json::Value::as_u64) == Some(15003)
        && p.get("reason").is_none();
    if is_angebot {
        // SG1 RFF+Z13 = Prüfidentifikator.
        builder = builder.reference("Z13", "15003");
        builder = builder.currency(p.get("currency").and_then(|v| v.as_str()).unwrap_or("EUR"));
        let contact = p
            .get("contact")
            .and_then(|v| v.as_str())
            .unwrap_or("ESA-Service");
        let comm = p
            .get("contact_comm")
            .and_then(|v| v.as_str())
            .unwrap_or("esa@example.de");
        builder = builder.contact(contact, comm);
        // Messprodukt (OBIS or product code) and offer price.
        let product = p
            .get("product")
            .and_then(|v| v.as_str())
            .unwrap_or("1-0:1.29.0");
        builder = builder.product(product);
        builder = builder.price(p.get("price").and_then(|v| v.as_str()).unwrap_or("0"));
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
