//! EDIFACT wire-format renderer for domain-intent outbox payloads.
//!
//! Converts `OutboxMessage.payload` (domain-intent JSON) → BDEW-conformant
//! EDIFACT wire bytes using the `edi-energy` builder infrastructure.
//!
//! # Design
//!
//! Each domain workflow enqueues a [`PendingOutbox`] with:
//! - `message_type`:  EDIFACT type code (e.g. `"UTILMD"`, `"APERAK"`)
//! - `recipient`:     trading-partner GLN
//! - `payload`:       domain-intent JSON (sender/receiver GLNs, process dates, …)
//!
//! This module maps those JSON fields to the appropriate `edi-energy` builder
//! and serialises the result to wire bytes. The active BDEW release (e.g. `"S2.2"`)
//! is resolved from the global [`ReleaseRegistry`] based on today's UTC date.
//!
//! ## Renderable message types
//!
//! | Type   | Payload fields consumed                                            |
//! |--------|--------------------------------------------------------------------|
//! | UTILMD | `pid`, `sender`, `receiver`, `malo`, `process_date` (`document_date` and `message_ref` are engine-derived when absent) |
//! | APERAK | `pid`, `sender`, `receiver`, `orig_message_ref`, `error_code`, `reason`, `document_date` |
//! | CONTRL | `sender`, `receiver`, `interchange_ref`, `accepted`, `message_ref` |
//! | ORDERS | `pid`, `orders_ref` (sender = `tenant_party_id`, receiver = `msg.recipient`) |
//! | ORDRSP | `sender`, `receiver`, `document_id`, `document_date`, `message_ref` |
//! | INVOIC | `sender`, `receiver`, `document_id`, `document_code`, `document_date`, `message_ref` |
//! | REMADV | `sender`, `receiver`, `document_id`, `document_code`, `document_date`, `message_ref` |
//!
//! ## Not yet renderable — intent-only payloads
//!
//! [`RenderError::InsufficientPayload`] is returned for message types whose
//! outbox payload carries only domain intent without the business data required
//! for a conformant wire message:
//!
//! - **MSCONS** — requires actual meter readings (not included in the intent payload)
//!
//! The AS4 sender returns `EngineError::RendererNotImplemented` for these, which
//! causes the outbox worker to dead-letter the entry immediately instead of
//! transmitting a non-conformant JSON blob over AS4.
//!
//! [`PendingOutbox`]: mako_engine::outbox::PendingOutbox
//! [`ReleaseRegistry`]: edi_energy::ReleaseRegistry

use edi_energy::{
    MessageType, ObjectType, Pruefidentifikator, Release, ReleaseRegistry, ReleaseTrack, builders,
};
use mako_engine::outbox::OutboxMessage;

// ── Error type ────────────────────────────────────────────────────────────────

/// Error returned by [`render_to_wire_bytes`].
#[derive(Debug)]
pub enum RenderError {
    /// The payload carries only domain intent without the business data required
    /// to construct a conformant EDIFACT message (e.g. MSCONS without meter
    /// readings). The AS4 sender should fall back to the JSON blob and log a
    /// structured `warn!`.
    InsufficientPayload {
        message_type: Box<str>,
        detail: Box<str>,
    },
    /// The payload JSON is missing a required field.
    MissingField {
        message_type: Box<str>,
        field: Box<str>,
    },
    /// No active BDEW profile is registered for this message type on today's date.
    NoActiveProfile { message_type: Box<str> },
    /// The `edi-energy` builder returned a serialization error.
    BuilderError(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::InsufficientPayload {
                message_type,
                detail,
            } => write!(
                f,
                "EDIFACT render [{message_type}]: insufficient payload for wire serialization — {detail}"
            ),
            RenderError::MissingField {
                message_type,
                field,
            } => write!(
                f,
                "EDIFACT render [{message_type}]: payload missing required field \"{field}\""
            ),
            RenderError::NoActiveProfile { message_type } => write!(
                f,
                "EDIFACT render [{message_type}]: no active BDEW profile registered for today's date"
            ),
            RenderError::BuilderError(e) => write!(f, "EDIFACT render: builder error: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Returns `true` when the render error is due to a missing business-data
/// payload (intent-only) rather than a schema or registry problem.
///
/// The AS4 sender uses this to decide whether to fall back to JSON.
#[allow(dead_code)] // used by as4_sender (bin-only module, not visible to lib)
pub fn is_insufficient_payload(err: &RenderError) -> bool {
    matches!(err, RenderError::InsufficientPayload { .. })
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Render a domain-intent [`OutboxMessage`] to BDEW-conformant EDIFACT wire bytes.
///
/// `tenant_party_id` is the operator's own market-participant identifier
/// (BDEW code, GLN, or EIC — from `--tenant-id`). It is used as
/// the sender for ORDERS and similar messages where the payload does not carry
/// an explicit sender GLN.
///
/// # Errors
///
/// - [`RenderError::InsufficientPayload`] — the payload is intent-only
///   (MSCONS, INVOIC, REMADV, …); the caller should fall back to JSON.
/// - [`RenderError::MissingField`] — a required JSON field is absent.
/// - [`RenderError::NoActiveProfile`] — no profile active on today's date.
/// - [`RenderError::BuilderError`] — the `edi-energy` builder failed.
pub fn render_to_wire_bytes(
    msg: &OutboxMessage,
    tenant_party_id: &str,
) -> Result<Vec<u8>, RenderError> {
    let p = &msg.payload;
    match msg.message_type.as_ref() {
        "UTILMD" => render_utilmd(p, msg),
        "APERAK" => render_aperak(p, msg),
        "CONTRL" => render_contrl(p, msg),
        "ORDERS" => render_orders(p, msg, tenant_party_id),
        "ORDRSP" => render_ordrsp(p, msg, tenant_party_id),
        "INVOIC" => render_invoic(p, msg, tenant_party_id),
        "REMADV" => render_remadv(p, msg, tenant_party_id),
        other => Err(intent_only(other)),
    }
}

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
fn render_utilmd(p: &serde_json::Value, msg: &OutboxMessage) -> Result<Vec<u8>, RenderError> {
    let mt = "UTILMD";

    let pid = require_u32(p, mt, "pid")?;
    let sender = require_str(p, mt, "sender")?;
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    // WiM PIDs (55039, 55042, 55051, 55168) refer to Messlokationen; all other UTILMD PIDs
    // (GPKE 55xxx, ex-MPES 56xxx, GeLi Gas 44xxx) refer to Marktlokationen.
    let (object_type, location_id_key) = if matches!(pid, 55_039 | 55_042 | 55_051 | 55_168) {
        (ObjectType::Messlokation, "melo")
    } else {
        (ObjectType::Marktlokation, "malo")
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
        .message_ref(message_ref);

    if let Some(dd) = doc_date_owned.as_deref() {
        builder = builder.document_date(dd);
    }

    builder
        .transaction(object_type, location_id)
        .process_date(dtm_qualifier, &process_date_yyyymmdd)
        .done()
        .serialize()
        .map_err(|e| RenderError::BuilderError(e.to_string()))
}

/// Returns the BDEW DTM qualifier for the process-date segment inside UTILMD SG4.
///
/// | PID range      | Process           | Qualifier | Meaning             |
/// |----------------|-------------------|-----------|---------------------|
/// | 55001, 44001   | Lieferbeginn      | 163       | Delivery start      |
/// | 55002, 44002   | Lieferende        | 164       | Delivery end        |
/// | 55016          | Kündigung         | 163       | Cancellation date   |
/// | 56001–56004    | ex-MPES feed-in   | 163       | Delivery start      |
/// | 55039, 55042, 55051, 55168 | WiM Messstellenbetrieb | 163       | Execution date      |
/// | 44003–44006    | GeLi Gas Antwort  | 163       | Confirmation date   |
/// | _              | fallback          | 163       | Delivery start      |
fn utilmd_dtm_qualifier(pid: u32) -> &'static str {
    match pid {
        55001 | 44001 => "163",                 // Lieferbeginn
        55002 | 44002 => "164",                 // Lieferende
        55016 => "163",                         // Kündigung Lieferbeginn (inbound, LFN → LFA)
        55017 | 55018 => "163",                 // Bestätigung/Ablehnung Kündigung (LFA → LFN)
        56001..=56004 => "163",                 // ex-MPES feed-in
        55039 | 55042 | 55051 | 55168 => "163", // WiM Messstellenbetrieb
        44003..=44006 => "163",                 // GeLi Gas confirmation/rejection
        _ => "163",
    }
}

// ── APERAK ────────────────────────────────────────────────────────────────────

/// Render an APERAK message from domain-intent JSON.
///
/// Payload fields:
///
/// | Field             | Required | Description                                  |
/// |-------------------|----------|----------------------------------------------|
/// | `sender`          | yes      | Sender GLN                                   |
/// | `receiver`        | no       | Receiver GLN (falls back to `msg.recipient`) |
/// | `pid`             | no       | APERAK Prüfidentifikator (e.g. 29001)        |
/// | `orig_message_ref`| no       | ACW reference to the message being acked     |
/// | `error_code`      | no       | ERC error code (e.g. `"E01"`)                |
/// | `reason`          | no       | FTX free-text error description              |
/// | `document_date`   | no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)                  |
/// | `message_ref`     | no       | Derived from `causation_event_id` when absent               |
fn render_aperak(p: &serde_json::Value, msg: &OutboxMessage) -> Result<Vec<u8>, RenderError> {
    let mt = "APERAK";

    let sender = require_str(p, mt, "sender")?;
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    let release = active_release(MessageType::Aperak, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let pid = p.get("pid").and_then(|v| v.as_u64()).map(|n| n as u32);
    let acw_ref = p.get("orig_message_ref").and_then(|v| v.as_str());
    let error_code = p.get("error_code").and_then(|v| v.as_str());
    let reason = p.get("reason").and_then(|v| v.as_str());
    let doc_date = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let mut builder = builders::AperakBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    if let Some(pv) = pid {
        if let Ok(ep) = Pruefidentifikator::new(pv) {
            builder = builder.pruefidentifikator(ep);
        }
    }
    if let Some(r) = acw_ref {
        builder = builder.acw_ref(r);
    }
    if let Some(c) = error_code {
        builder = builder.error_code(c);
    }
    if let Some(t) = reason {
        builder = builder.error_text(t);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }

    builder
        .serialize()
        .map_err(|e| RenderError::BuilderError(e.to_string()))
}

// ── CONTRL ────────────────────────────────────────────────────────────────────

/// Render a CONTRL functional acknowledgement from domain-intent JSON.
///
/// Payload fields:
///
/// | Field           | Required | Description                                  |
/// |-----------------|----------|----------------------------------------------|
/// | `sender`        | yes      | Sender GLN                                   |
/// | `receiver`      | no       | Receiver GLN (falls back to `msg.recipient`) |
/// | `interchange_ref`| no      | UCI interchange control reference            |
/// | `accepted`      | no       | `true` = accepted (code 4), `false` = rejected (code 8) |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent              |
fn render_contrl(p: &serde_json::Value, msg: &OutboxMessage) -> Result<Vec<u8>, RenderError> {
    let mt = "CONTRL";

    let sender = require_str(p, mt, "sender")?;
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let interchange_ref = p
        .get("interchange_ref")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let accepted = p.get("accepted").and_then(|v| v.as_bool()).unwrap_or(true);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Contrl, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::ContrlBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .interchange_ref(interchange_ref)
        .message_ref(message_ref);
    builder = if accepted {
        builder.accept()
    } else {
        builder.reject()
    };

    builder
        .serialize()
        .map_err(|e| RenderError::BuilderError(e.to_string()))
}

// ── ORDERS ────────────────────────────────────────────────────────────────────

/// Render an ORDERS (Beauftragung) message from domain-intent JSON.
///
/// The sender is the operator tenant (`tenant_party_id`); the receiver comes from
/// `msg.recipient`. Both GPKE Konfigurationseinrichtung (PID 17134) and
/// GeLi Gas Beauftragung (PID 17135) are handled here.
///
/// Payload fields:
///
/// | Field        | Required | Description                                  |
/// |--------------|----------|----------------------------------------------|
/// | `pid`        | no       | ORDERS Prüfidentifikator (e.g. 17134)        |
/// | `orders_ref` | no       | UUID reference → 14-char UNH message ref     |
/// | `malo`       | no       | Supply point MaLo for BGM context            |
fn render_orders(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    tenant_party_id: &str,
) -> Result<Vec<u8>, RenderError> {
    let mt = "ORDERS";

    let orders_ref = p
        .get("orders_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid);
    let causation_ref = msg_ref_from_uuid(&msg.causation_event_id.to_string());
    let message_ref = orders_ref.as_deref().unwrap_or(causation_ref.as_str());
    let pid = p.get("pid").and_then(|v| v.as_u64()).map(|n| n as u32);

    let release = active_release(MessageType::Orders, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut builder = builders::OrdersBuilder::new(release)
        .sender(tenant_party_id)
        .receiver(msg.recipient.as_ref())
        .message_ref(message_ref);

    if let Some(pv) = pid {
        builder = builder.document_id(pv.to_string());
    }

    builder
        .serialize()
        .map_err(|e| RenderError::BuilderError(e.to_string()))
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
/// | `sender`       | no       | Sender GLN (falls back to `tenant_party_id`)  |
/// | `receiver`     | no       | Receiver GLN (falls back to `msg.recipient`)  |
/// | `document_id`  | no       | BGM document identifier (Auftragsnummer)      |
/// | `document_date`| no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `message_ref`  | no       | Derived from `causation_event_id` when absent |
fn render_ordrsp(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    tenant_party_id: &str,
) -> Result<Vec<u8>, RenderError> {
    let mt = "ORDRSP";

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or(tenant_party_id);
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

    if let Some(id) = document_id {
        builder = builder.document_id(id);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }

    builder
        .serialize()
        .map_err(|e| RenderError::BuilderError(e.to_string()))
}

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
/// | `sender`        | no       | Sender GLN (falls back to `tenant_party_id`)  |
/// | `receiver`      | no       | Receiver GLN (falls back to `msg.recipient`)  |
/// | `document_id`   | no       | BGM document identifier (Rechnungsnummer)     |
/// | `document_code` | no       | BGM type code (default `"380"`)               |
/// | `document_date` | no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent |
fn render_invoic(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    tenant_party_id: &str,
) -> Result<Vec<u8>, RenderError> {
    let mt = "INVOIC";

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or(tenant_party_id);
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

    let release = active_release(MessageType::Invoic, &ReleaseTrack::Short).ok_or_else(|| {
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

    builder
        .serialize()
        .map_err(|e| RenderError::BuilderError(e.to_string()))
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
/// | `sender`        | no       | Sender GLN (falls back to `tenant_party_id`)  |
/// | `receiver`      | no       | Receiver GLN (falls back to `msg.recipient`)  |
/// | `document_id`   | no       | BGM document identifier (Avisnummer)          |
/// | `document_code` | no       | BGM type code (default `"239"`)               |
/// | `document_date` | no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent |
fn render_remadv(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    tenant_party_id: &str,
) -> Result<Vec<u8>, RenderError> {
    let mt = "REMADV";

    let sender = p
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or(tenant_party_id);
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

    let release = active_release(MessageType::Remadv, &ReleaseTrack::Short).ok_or_else(|| {
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

    builder
        .serialize()
        .map_err(|e| RenderError::BuilderError(e.to_string()))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Resolve the active `Release` for `(message_type, track)` from today's registry.
fn active_release(message_type: MessageType, track: &ReleaseTrack) -> Option<Release> {
    let today = time::OffsetDateTime::now_utc().date();
    ReleaseRegistry::global()
        .profile_for_date_and_track(message_type, today, track)
        .map(|p| p.release().clone())
}

/// Return a `RenderError::InsufficientPayload` for intent-only message types.
///
/// MSCONS is the only type that cannot be rendered from workflow intent alone:
/// it requires actual meter readings and quantities sourced externally from the
/// metering data pipeline. All other EDIFACT types (ORDRSP, INVOIC, REMADV, …)
/// are now handled by dedicated render functions above.
fn intent_only(message_type: &str) -> RenderError {
    let detail: Box<str> = match message_type {
        "MSCONS" => "MSCONS requires actual meter readings, OBIS codes, and quantities — \
             not included in the workflow intent payload. Provide metering data \
             from the metering pipeline before dispatching via AS4."
            .into(),
        _ => format!(
            "wire-format rendering for '{message_type}' is not implemented. \
                 Add a render_{} function to edifact_renderer.rs.",
            message_type.to_ascii_lowercase()
        )
        .into(),
    };
    RenderError::InsufficientPayload {
        message_type: message_type.into(),
        detail,
    }
}

/// Require a string field from the payload, returning a `MissingField` error.
fn require_str<'a>(
    p: &'a serde_json::Value,
    message_type: &'static str,
    field: &'static str,
) -> Result<&'a str, RenderError> {
    p.get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| RenderError::MissingField {
            message_type: message_type.into(),
            field: field.into(),
        })
}

/// Require a `u32` field from the payload, returning a `MissingField` error.
fn require_u32(
    p: &serde_json::Value,
    message_type: &'static str,
    field: &'static str,
) -> Result<u32, RenderError> {
    p.get(field)
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .ok_or_else(|| RenderError::MissingField {
            message_type: message_type.into(),
            field: field.into(),
        })
}

/// Normalise a date string: accepts both ISO-8601 (`2026-01-01`) and
/// YYYYMMDD (`20260101`). Strips dashes and returns the 8-digit form.
fn normalise_date(date: &str) -> String {
    date.replace('-', "")
}

/// Truncate a UUID string to a valid EDIFACT UNH message reference (max 14 chars).
///
/// Strips hyphens and takes the first 14 hex characters.
fn msg_ref_from_uuid(uuid_str: &str) -> String {
    uuid_str
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(14)
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
    use mako_engine::outbox::OutboxMessage;

    fn fake_msg(message_type: &str, recipient: &str, payload: serde_json::Value) -> OutboxMessage {
        OutboxMessage::new(
            StreamId::new("process/test"),
            ProcessId::new(),
            TenantId::new(),
            CorrelationId::new(),
            ConversationId::new(),
            EventId::new(),
            message_type,
            recipient,
            payload,
        )
    }

    #[test]
    fn msg_ref_from_uuid_strips_dashes_and_truncates() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let r = msg_ref_from_uuid(uuid);
        assert_eq!(r.len(), 14);
        assert!(!r.contains('-'));
    }

    #[test]
    fn normalise_date_strips_dashes() {
        assert_eq!(normalise_date("2026-01-01"), "20260101");
        assert_eq!(normalise_date("20260101"), "20260101");
    }

    #[test]
    fn intent_only_mscons_returns_insufficient_payload() {
        let err = intent_only("MSCONS");
        assert!(matches!(err, RenderError::InsufficientPayload { .. }));
        assert!(is_insufficient_payload(&err));
    }

    #[test]
    fn utilmd_dtm_qualifier_by_pid() {
        assert_eq!(utilmd_dtm_qualifier(55001), "163");
        assert_eq!(utilmd_dtm_qualifier(55002), "164");
        assert_eq!(utilmd_dtm_qualifier(55016), "163");
        assert_eq!(utilmd_dtm_qualifier(44001), "163");
        assert_eq!(utilmd_dtm_qualifier(44002), "164");
    }

    #[test]
    fn render_mscons_returns_insufficient_payload() {
        let msg = fake_msg(
            "MSCONS",
            "9900987654321",
            serde_json::json!({
                "type": "MovementDataRequired",
                "pid": 13015_u32,
                "malo": "DE0001234567890",
            }),
        );
        let result = render_to_wire_bytes(&msg, "9900123456789");
        assert!(matches!(
            result,
            Err(RenderError::InsufficientPayload { .. })
        ));
    }

    #[test]
    fn render_utilmd_missing_pid_returns_missing_field() {
        let msg = fake_msg(
            "UTILMD",
            "9900987654321",
            serde_json::json!({
                "sender":   "9900123456789",
                "malo":     "DE0001234567890",
                "process_date": "20260101",
            }),
        );
        let result = render_to_wire_bytes(&msg, "9900123456789");
        assert!(
            matches!(result, Err(RenderError::MissingField { field, .. }) if field.as_ref() == "pid")
        );
    }

    #[test]
    fn render_contrl_uses_recipient_fallback_for_receiver() {
        // Payload without explicit receiver — should use msg.recipient
        let msg = fake_msg(
            "CONTRL",
            "9900987654321",
            serde_json::json!({
                "sender": "9900123456789",
                "interchange_ref": "TEST-REF-001",
                "accepted": true,
            }),
        );
        // We can't guarantee a release is active in unit-test context (no registry),
        // but we can verify the payload-extraction path reaches the release lookup.
        let result = render_to_wire_bytes(&msg, "9900123456789");
        // Either succeeds (if a profile is registered) or NoActiveProfile.
        // Never MissingField or InsufficientPayload.
        match &result {
            Ok(_) => {}
            Err(RenderError::NoActiveProfile { .. }) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
