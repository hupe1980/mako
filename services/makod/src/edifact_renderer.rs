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
//! | ORDRSP | `pid` (ESA 19011/19012/19013/19014), `sender`, `receiver`, `order_reference`, `document_id`, `document_date`, `message_ref` |
//! | QUOTES | `pid` (ESA 15003), `sender`, `receiver`, `order_reference`, `document_id`, `document_date`, `message_ref` |
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
    MessageType, Pruefidentifikator, Release, ReleaseRegistry, ReleaseTrack, builders,
};
use mako_engine::outbox::OutboxMessage;

use crate::party_registry::MpIdRegistry;

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
    /// The message should be silently suppressed — no wire EDIFACT should be sent.
    ///
    /// Used for Gas positive APERAKs: per APERAK AHB 1.0 §2.3, silence = acceptance
    /// for Gas processes. The domain outbox entry exists for ERP webhook delivery,
    /// but no wire EDIFACT is emitted over AS4.
    Suppressed { reason: Box<str> },
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
            RenderError::Suppressed { reason } => {
                write!(f, "EDIFACT render: suppressed — {reason}")
            }
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

/// Returns `true` when the message should be suppressed — no wire EDIFACT sent.
///
/// Used for Gas positive APERAKs (silence = acceptance per APERAK AHB 1.0 §2.3).
/// The AS4 sender acknowledges the outbox entry without transmitting.
/// The ERP webhook sender delivers the domain JSON payload instead.
#[allow(dead_code)] // used by as4_sender (bin-only module, not visible to lib)
pub fn is_suppressed(err: &RenderError) -> bool {
    matches!(err, RenderError::Suppressed { .. })
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Render a domain-intent [`OutboxMessage`] to BDEW-conformant EDIFACT wire bytes.
///
/// `registry` provides the operator's own GLN(s).  For ORDERS messages, the
/// sender GLN is resolved from `payload["sender"]` when present; otherwise the
/// registry's static PID → role table is used.
/// For all other fallbacks (ORDRSP, INVOIC, REMADV without explicit `sender`)
/// [`MpIdRegistry::primary_mp_id`] is used.
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
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let p = &msg.payload;
    match msg.message_type.as_ref() {
        "UTILMD" => render_utilmd(p, msg),
        "APERAK" => render_aperak(p, msg),
        "CONTRL" => render_contrl(p, msg),
        "REQOTE" => render_reqote(p, msg, registry),
        "ORDERS" => render_orders(p, msg, registry),
        "ORDCHG" => render_ordchg(p, msg, registry),
        "ORDRSP" => render_ordrsp(p, msg, registry),
        "QUOTES" => render_quotes(p, msg, registry),
        "INVOIC" => render_invoic(p, msg, registry),
        "REMADV" => render_remadv(p, msg, registry),
        "MSCONS" => render_mscons(p, msg, registry),
        other => Err(intent_only(other)),
    }
}

// ── Übertragungsdatei envelope ────────────────────────────────────────────────

/// A rendered EDIFACT Übertragungsdatei — the full `UNB…UNZ` interchange —
/// with the envelope identities the transport layer needs.
///
/// Allgemeine Festlegungen 6.1d, Kap. 2: every EDIFACT Übertragungsdatei
/// carries the UNB segment at interchange level, and the MP-IDs used in UNB
/// and NAD for sender and receiver must be identical. The envelope is built
/// here, from the *same* sender/receiver values the message body's `NAD+MS` /
/// `NAD+MR` were built from, so the identity equality holds by construction.
#[derive(Debug)]
pub struct RenderedInterchange {
    /// The complete wire bytes: `UNB … UNH … UNT … UNZ`.
    pub bytes: Vec<u8>,
    /// UNB DE0004 — identical to the message's `NAD+MS` MP-ID.
    pub sender_mp_id: Box<str>,
    /// UNB DE0010 — identical to the message's `NAD+MR` MP-ID.
    pub receiver_mp_id: Box<str>,
    /// UNB DE0020 Datenaustauschreferenz — repeated in UNZ DE0036 and used
    /// as the DAR component of the §2.12 Content-Disposition filename.
    /// Derived from the outbox message id, so retries reuse the same DAR.
    pub dar: Box<str>,
}

/// UNB DE0007 Teilnehmerbezeichnung-Qualifier for an MP-ID.
///
/// Allgemeine Festlegungen 6.1d, UNB segment table: `14` = GS1,
/// `500` = DE, BDEW, `502` = DE, DVGW Service & Consult GmbH.
/// BDEW-issued 13-digit MP-IDs start with `99`, DVGW-issued with `98`;
/// 16-character EIC codes are issued by BDEW as the German issuing office.
fn unb_qualifier(mp_id: &str) -> &'static str {
    if mp_id.len() == 13 && mp_id.starts_with("99") {
        "500"
    } else if mp_id.len() == 13 && mp_id.starts_with("98") {
        "502"
    } else if mp_id.len() == 13 {
        "14"
    } else {
        "500"
    }
}

/// The UNB DE0020 / UNZ DE0036 Datenaustauschreferenz for an outbox message.
///
/// First 14 uppercase hex chars of the outbox message UUID: unique per
/// message, stable across delivery retries, and within the UNOC character
/// set and the `an..14` length bound of DE0020.
fn dar_for(msg: &OutboxMessage) -> String {
    msg.message_id
        .to_string()
        .replace('-', "")
        .to_uppercase()
        .chars()
        .take(14)
        .collect()
}

/// Wrap rendered message bytes in the `UNB…UNZ` interchange envelope.
///
/// The builders emit one message (`UNH…UNT`); the regulated wire format is
/// the Übertragungsdatei, which carries exactly this envelope around it.
/// Written through `edifact_rs::Writer::write_composites`, so component
/// boundaries are structural and the values are escaped for the wire.
fn finish_interchange(
    serialized: Result<Vec<u8>, edi_energy::Error>,
    sender: &str,
    receiver: &str,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
    let message = serialized.map_err(|e| RenderError::BuilderError(e.to_string()))?;
    let dar = dar_for(msg);
    let now = time::OffsetDateTime::now_utc();
    let date = format!(
        "{:02}{:02}{:02}",
        now.year() % 100,
        now.month() as u8,
        now.day()
    );
    let hhmm = format!("{:02}{:02}", now.hour(), now.minute());

    let mut w = edifact_rs::Writer::new(Vec::with_capacity(message.len() + 96));
    w.write_composites(
        "UNB",
        &[
            &["UNOC", "3"],
            &[sender, unb_qualifier(sender)],
            &[receiver, unb_qualifier(receiver)],
            &[&date, &hhmm],
            &[&dar],
        ],
    )
    .map_err(|e| RenderError::BuilderError(format!("UNB envelope: {e}")))?;
    let mut bytes = w
        .finish()
        .map_err(|e| RenderError::BuilderError(format!("UNB envelope: {e}")))?;
    bytes.extend_from_slice(&message);
    let mut w = edifact_rs::Writer::new(bytes);
    w.write_composites("UNZ", &[&["1"], &[&dar]])
        .map_err(|e| RenderError::BuilderError(format!("UNZ envelope: {e}")))?;
    let bytes = w
        .finish()
        .map_err(|e| RenderError::BuilderError(format!("UNZ envelope: {e}")))?;

    Ok(RenderedInterchange {
        bytes,
        sender_mp_id: sender.into(),
        receiver_mp_id: receiver.into(),
        dar: dar.into(),
    })
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
fn render_utilmd(
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

    finish_interchange(
        builder
            .transaction_with_qualifier(ide_qualifier, location_id)
            .process_date(dtm_qualifier, &process_date_yyyymmdd)
            .done()
            .serialize(),
        sender,
        receiver,
        msg,
    )
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
fn utilmd_dtm_qualifier(pid: u32) -> &'static str {
    match pid {
        55001 | 44001 => "163",                 // Lieferbeginn
        55002 | 44002 => "164",                 // Lieferende
        55016 => "163",                         // Kündigung Lieferbeginn (inbound, LFN → LFA)
        55017 | 55018 => "163",                 // Bestätigung/Ablehnung Kündigung (LFA → LFN)
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
fn render_aperak(
    p: &serde_json::Value,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "APERAK";

    // Gas positive APERAK: silence = acceptance per APERAK AHB 1.0 §2.3.
    // Payload carries `suppress_wire: true` to signal no wire EDIFACT should be sent.
    // The outbox entry is still delivered as domain JSON to the ERP webhook.
    if p.get("suppress_wire")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(RenderError::Suppressed {
            reason: "Gas positive APERAK: suppress_wire=true (silence = acceptance, APERAK AHB 1.0 §2.3)"
                .into(),
        });
    }

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

    // BGM+313 (Verarbeitbarkeitsfehlermeldung) is mandatory when an error code
    // is present; BGM+312 (Anerkennungsmeldung) would be used for positive acks.
    // The BDEW APERAK AHB 1.0 §2.1.1 requires BGM+313 for all APERAK rejections.
    // The `document_code` payload field allows an explicit override when needed.
    let document_code = p.get("document_code").and_then(|v| v.as_str());
    if let Some(code) = document_code {
        builder = builder.document_code(code);
    } else if error_code.is_some() {
        // Auto-select BGM+313: error APERAK (Verarbeitbarkeitsfehlermeldung).
        builder = builder.document_code("313");
    }

    if let Some(pv) = pid
        && let Ok(ep) = Pruefidentifikator::new(pv)
    {
        builder = builder.pruefidentifikator(ep);
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

    finish_interchange(builder.serialize(), sender, receiver, msg)
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
fn render_contrl(
    p: &serde_json::Value,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
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

    finish_interchange(builder.serialize(), sender, receiver, msg)
}

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
fn render_reqote(
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

fn render_orders(
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
fn render_ordchg(
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
fn render_ordrsp(
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
fn render_quotes(
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
/// | `sender`        | no       | Sender GLN (falls back to `registry.primary_mp_id()`)|
/// | `receiver`      | no       | Receiver GLN (falls back to `msg.recipient`)  |
/// | `document_id`   | no       | BGM document identifier (Avisnummer)          |
/// | `document_code` | no       | BGM type code (default `"239"`)               |
/// | `document_date` | no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent |
fn render_remadv(
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

    finish_interchange(builder.serialize(), sender, receiver, msg)
}

/// MSCONS "Übertragung Summenzeitreihe" (MaBiS), AHB 3.2 §8.3.1.
const MSCONS_PID_SUMMENZEITREIHE: u64 = 13003;

/// BGM DE 1001 document-name code for an MSCONS Anwendungsfall.
///
/// The code is not constant across MSCONS: it names what kind of document the
/// message is, and the AHB fixes a different one per use case. Sending the
/// wrong code labels a Summenzeitreihe as a Prozessdatenbericht, which the
/// receiver routes by.
const fn mscons_document_code(pid: u64) -> &'static str {
    match pid {
        // "Zeitreihen im Rahmen der Bilanzkreisabrechnung"
        MSCONS_PID_SUMMENZEITREIHE => "BK",
        // "Redispatch"
        MSCONS_PID_AUSFALLARBEIT_SZR => "Z46",
        // "Bewegungsdaten im Kalenderjahr vor Lieferbeginn"
        MSCONS_PID_ARBEIT_LEISTUNGSMAX => "Z27",
        // "Energiemenge und Leistungsmaximum"
        MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX => "Z28",
        // "Prozessdatenbericht"
        _ => "7",
    }
}

/// MSCONS "Energiemenge (Strom)", AHB 3.2 — energy for a billing period, with
/// no power maximum.
const MSCONS_PID_ENERGIEMENGE: u64 = 13019;

/// MSCONS "Energiemenge und Leistungsmaximum", AHB 3.2.
const MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX: u64 = 13016;

/// MSCONS "Arbeit / Leistungsmaximum im Kalenderjahr vor Lieferbeginn",
/// AHB 3.2 — the movement data a Netznutzungsvertrag requires when an RLM
/// Marktlokation changes supplier mid-year (GPKE Kap. 6.1).
const MSCONS_PID_ARBEIT_LEISTUNGSMAX: u64 = 13015;

/// MSCONS "Redispatch 2.0 Ausfallarbeits-summenzeitreihe", AHB 3.2.
///
/// Same segment shape as the MaBiS Summenzeitreihe — a summed series over
/// settlement slots for one Zählpunkt — so it renders through the same path.
const MSCONS_PID_AUSFALLARBEIT_SZR: u64 = 13023;

/// MSCONS "Werte nach Typ 2" (MSB → ESA), UC 4.2 / §60 Abs. 1 MsbG.
///
/// A MaLo + OBIS interval delivery addressed to the ESA (NAD+MR). Renders
/// through [`render_mscons_typ2`].
const MSCONS_PID_WERTE_TYP2: u64 = 13027;

/// Render a summed MSCONS time series (Prüfidentifikator 13003 or 13023).
///
/// The payload carries the identifying 3-tuple — MaBiS-Zählpunkt,
/// Bilanzierungsmonat, Version (BK6-24-174 Anlage 3 §3.8.2) — and one entry per
/// settlement slot. MaBiS settles per quarter-hour, so the slots are the
/// message: a period total would carry the right sum and the wrong shape.
///
/// # Errors
///
/// [`RenderError::MissingField`] when the 3-tuple or the intervals are absent —
/// a Summenzeitreihe without them cannot be placed on the settlement grid.
fn render_mscons(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "MSCONS";

    // MSCONS carries many Anwendungsfälle with materially different segment
    // shapes. Dispatching on the Prüfidentifikator keeps an unsupported one from
    // being rendered in the shape of a supported one, which would produce a
    // syntactically valid message stating something the sender did not mean.
    let pid = p
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    match pid {
        // Summenzeitreihe (MaBiS) and Redispatch 2.0
        // Ausfallarbeits-summenzeitreihe share the same shape: a summed series
        // over settlement slots for one Zählpunkt.
        MSCONS_PID_SUMMENZEITREIHE | MSCONS_PID_AUSFALLARBEIT_SZR => {}
        MSCONS_PID_ARBEIT_LEISTUNGSMAX
        | MSCONS_PID_ENERGIEMENGE
        | MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX => {
            return render_mscons_arbeit_leistungsmax(p, msg, registry);
        }
        MSCONS_PID_WERTE_TYP2 => {
            return render_mscons_typ2(p, msg, registry);
        }
        other => {
            return Err(RenderError::InsufficientPayload {
                message_type: mt.into(),
                detail: format!(
                    "MSCONS Prüfidentifikator {other} has no renderer. Supported: \
                     {MSCONS_PID_SUMMENZEITREIHE} (Summenzeitreihe), \
                     {MSCONS_PID_AUSFALLARBEIT_SZR} (Redispatch Ausfallarbeits-SZR), \
                     {MSCONS_PID_ARBEIT_LEISTUNGSMAX} (Arbeit/Leistungsmaximum), \
                     {MSCONS_PID_ENERGIEMENGE} (Energiemenge), \
                     {MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX} (Energiemenge + Leistungsmaximum), \
                     {MSCONS_PID_WERTE_TYP2} (Werte nach Typ 2, MSB→ESA)."
                )
                .into(),
            });
        }
    }

    let sender = p
        .get("sender_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    let zaehlpunkt = p
        .get("bilanzierungsgebiet_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "bilanzierungsgebiet_id".into(),
        })?;
    let balancing_period = p
        .get("balancing_period")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "balancing_period (CCYYMM)".into(),
        })?;
    let version =
        p.get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RenderError::MissingField {
                message_type: mt.into(),
                field: "version (CCYYMMDDHHMMSSZZZ)".into(),
            })?;

    let intervals = p
        .get("intervals")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "intervals".into(),
        })?;

    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Mscons, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut mp = builders::MsconsBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(mscons_document_code(pid))
        .pruefidentifikator(
            edi_energy::Pruefidentifikator::new(u32::try_from(pid).unwrap_or_default()).map_err(
                |e| RenderError::BuilderError(format!("invalid Prüfidentifikator {pid}: {e}")),
            )?,
        )
        .metering_point(zaehlpunkt)
        .balancing_period(balancing_period)
        .version(version);

    for iv in intervals {
        let (Some(from), Some(to), Some(qty)) = (
            iv.get("from").and_then(|v| v.as_str()),
            iv.get("to").and_then(|v| v.as_str()),
            iv.get("quantity_kwh").and_then(|v| v.as_str()),
        ) else {
            return Err(RenderError::MissingField {
                message_type: mt.into(),
                field: "intervals[].{from,to,quantity_kwh}".into(),
            });
        };
        // DE 6063 `79` = "Energiemenge summiert (Summenwert, Bilanzsumme)";
        // DE 6411 `KWH` (MSCONS AHB 3.2, SG10 QTY). A consumption qualifier
        // would describe one metering point's draw, not the aggregate of a
        // Bilanzierungsgebiet.
        mp = mp.quantity_for_period(
            edi_energy::builders::QTY_ENERGIE_SUMMIERT,
            qty,
            "KWH",
            from,
            to,
        );
    }

    finish_interchange(mp.done().serialize(), sender, receiver, msg)
}

/// Render MSCONS "Arbeit / Leistungsmaximum im Kalenderjahr vor Lieferbeginn"
/// (Prüfidentifikator 13015).
///
/// Shape per AHB 3.2: SG9 repeats two to three times for one `NAD+DP` — once
/// for the energy from the start of the calendar year to Lieferbeginn, then
/// once or twice for the highest and second-highest monthly power maxima
/// (needed for the KAV concession-levy band).
///
/// Each maximum carries the period it fell in as `DTM+306`: format `610`
/// (`CCYYMM`) under a monthly or yearly Leistungspreissystem, `102`
/// (`CCYYMMDD`) under a daily one. A magnitude without that period cannot be
/// attributed to a month, which is what the KAV band depends on.
///
/// # Errors
///
/// [`RenderError::MissingField`] when the MaLo, the work entry or its period is
/// absent.
fn render_mscons_arbeit_leistungsmax(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let pid = p
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    use edi_energy::builders::{
        MSCONS_UNITS, QTY_ERSATZWERT, QTY_WAHRER_WERT, is_valid_mscons_unit,
    };

    let mt = "MSCONS";
    let missing = |field: &str| RenderError::MissingField {
        message_type: mt.into(),
        field: field.into(),
    };
    // The AHB's per-Anwendungsfall table has no DE 6411 row for 13015, so the
    // unit follows the MIG's closed code list rather than a value fixed here:
    // the work entry is energy (`KWH`), a maximum is power (`KWT`).
    let checked_unit = |unit: &str| -> Result<(), RenderError> {
        if is_valid_mscons_unit(unit) {
            Ok(())
        } else {
            Err(RenderError::InsufficientPayload {
                message_type: mt.into(),
                detail: format!(
                    "unit {unit:?} is not a MSCONS DE 6411 code; expected one of {MSCONS_UNITS:?}"
                )
                .into(),
            })
        }
    };

    let sender = p
        .get("sender_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let malo_id = p
        .get("malo_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| missing("malo_id"))?;

    let arbeit = p.get("arbeit").ok_or_else(|| missing("arbeit"))?;
    let (Some(arbeit_kwh), Some(from), Some(to)) = (
        arbeit.get("quantity").and_then(|v| v.as_str()),
        arbeit.get("from").and_then(|v| v.as_str()),
        arbeit.get("to").and_then(|v| v.as_str()),
    ) else {
        return Err(missing("arbeit.{quantity,from,to}"));
    };

    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Mscons, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    // DE 6063 distinguishes a measured value from a substitute one. Reporting a
    // substitute as measured would assert a reading that was never taken.
    let qualifier = |v: &serde_json::Value| {
        if v.get("ersatzwert").and_then(serde_json::Value::as_bool) == Some(true) {
            QTY_ERSATZWERT
        } else {
            QTY_WAHRER_WERT
        }
    };

    let arbeit_unit = arbeit.get("unit").and_then(|v| v.as_str()).unwrap_or("KWH");
    checked_unit(arbeit_unit)?;

    let mut mp = builders::MsconsBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(mscons_document_code(pid))
        .pruefidentifikator(
            edi_energy::Pruefidentifikator::new(u32::try_from(pid).unwrap_or_default()).map_err(
                |e| RenderError::BuilderError(format!("invalid Prüfidentifikator {pid}: {e}")),
            )?,
        )
        .metering_point(malo_id)
        .quantity_for_period(qualifier(arbeit), arbeit_kwh, arbeit_unit, from, to);

    let maxima = p
        .get("leistungsmaxima")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();

    // 13019 is energy alone — the AHB marks no Leistungsperiode row for it, so
    // a maximum sent under it would have no period to be attributed to.
    if pid == MSCONS_PID_ENERGIEMENGE && !maxima.is_empty() {
        return Err(RenderError::InsufficientPayload {
            message_type: mt.into(),
            detail: format!(
                "Prüfidentifikator {MSCONS_PID_ENERGIEMENGE} carries Energiemenge only; \
                 send {MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX} to report a Leistungsmaximum"
            )
            .into(),
        });
    }

    // Up to two maxima. The AHB permits one or two; more would exceed the
    // segment-group repeat the message allows.
    if maxima.len() > 2 {
        return Err(RenderError::InsufficientPayload {
            message_type: mt.into(),
            detail: format!(
                "at most two Monatsleistungsmaxima may be sent, got {}",
                maxima.len()
            )
            .into(),
        });
    }

    for m in maxima {
        let (Some(value), Some(period)) = (
            m.get("quantity").and_then(|v| v.as_str()),
            m.get("period").and_then(|v| v.as_str()),
        ) else {
            return Err(missing("leistungsmaxima[].{quantity,period}"));
        };
        // `610` for a `CCYYMM` period, `102` for `CCYYMMDD` — the caller knows
        // which Leistungspreissystem applies.
        let period_format = m
            .get("period_format")
            .and_then(|v| v.as_str())
            .unwrap_or("610");
        // Power, not energy.
        let unit = m.get("unit").and_then(|v| v.as_str()).unwrap_or("KWT");
        checked_unit(unit)?;

        mp = mp
            .next_line_item()
            .quantity(qualifier(m), value, unit)
            .leistungsperiode(period, period_format);
    }

    finish_interchange(mp.done().serialize(), sender, receiver, msg)
}

/// Render an outbound MSCONS 13027 "Werte nach Typ 2" (MSB → ESA), UC 4.2.
///
/// A MaLo + OBIS interval delivery **addressed to the ESA** (NAD+MR = the ESA's
/// MP-ID from `receiver_mp_id`). Intervals are grouped by OBIS into line items;
/// each carries its quarter-hour quantities as Wirkarbeit (KWH). This is the
/// §60 Abs. 1 MsbG delivery duty on the wire — the values are non-authoritative
/// and land in the ESA's separate Typ-2 store.
fn render_mscons_typ2(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    use edi_energy::builders::{QTY_ERSATZWERT, QTY_WAHRER_WERT};

    let mt = "MSCONS";
    let missing = |field: &str| RenderError::MissingField {
        message_type: mt.into(),
        field: field.into(),
    };

    let sender = p
        .get("sender_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    // The recipient is the ESA — this is the whole point of the addressing gap:
    // the MSB emits 13027 to a party that is neither NB nor LF.
    let receiver = p
        .get("receiver_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let malo_id = p
        .get("malo_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| missing("malo_id"))?;

    let reads = p
        .get("reads")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| missing("reads"))?;

    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Mscons, &ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let qualifier = |r: &serde_json::Value| {
        if r.get("ersatzwert").and_then(serde_json::Value::as_bool) == Some(true) {
            QTY_ERSATZWERT
        } else {
            QTY_WAHRER_WERT
        }
    };

    let mut mp = builders::MsconsBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(mscons_document_code(MSCONS_PID_WERTE_TYP2))
        .pruefidentifikator(
            edi_energy::Pruefidentifikator::new(
                u32::try_from(MSCONS_PID_WERTE_TYP2).unwrap_or_default(),
            )
            .map_err(|e| RenderError::BuilderError(format!("invalid Prüfidentifikator: {e}")))?,
        )
        .metering_point(malo_id);

    // Group by OBIS into line items so one register's quarter-hour series lands
    // under one line item; a delivery without OBIS falls back to a bare item.
    let mut current_obis: Option<String> = None;
    let mut first_item = true;
    for r in reads {
        let (Some(quantity), Some(from), Some(to)) = (
            r.get("quantity_kwh").and_then(|v| v.as_str()),
            r.get("dtm_from").and_then(|v| v.as_str()),
            r.get("dtm_to").and_then(|v| v.as_str()),
        ) else {
            return Err(missing("reads[].{quantity_kwh,dtm_from,dtm_to}"));
        };
        let obis = r.get("obis_code").and_then(|v| v.as_str());
        if obis.map(str::to_owned) != current_obis {
            // Start a new line item for a new OBIS register.
            if !first_item {
                mp = mp.next_line_item();
            }
            if let Some(code) = obis {
                let parsed = rubo4e::identifiers::ObisCode::new(code).map_err(|e| {
                    RenderError::BuilderError(format!("invalid OBIS code {code:?}: {e}"))
                })?;
                mp = mp.line_item(parsed);
            }
            current_obis = obis.map(str::to_owned);
            first_item = false;
        }
        mp = mp.quantity_for_period(qualifier(r), quantity, "KWH", from, to);
    }

    finish_interchange(mp.done().serialize(), sender, receiver, msg)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Resolve the active `Release` for `(message_type, track)` from today's registry.
fn active_release(message_type: MessageType, track: &ReleaseTrack) -> Option<Release> {
    let today = time::OffsetDateTime::now_utc().date();
    ReleaseRegistry::global()
        .profile_for_date_and_track(message_type, today, track)
        .map(|p| p.release().clone())
}

/// Return a `RenderError::InsufficientPayload` for a message type with no
/// dedicated renderer.
fn intent_only(message_type: &str) -> RenderError {
    let detail: Box<str> = format!(
        "wire-format rendering for '{message_type}' is not implemented. \
         Add a render_{} function to edifact_renderer.rs.",
        message_type.to_ascii_lowercase()
    )
    .into();
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
/// Normalise an arbitrary reference string into a wire-safe EDIFACT UNH message
/// reference (DE 0062 is `an..14`): keep only alphanumerics, truncate to 14.
///
/// The function is idempotent, so a caller that registers a process under
/// `msg_ref_from_uuid(raw)` gets the exact reference the renderer later emits on
/// the wire — which the ESA↔MSB order-reference correlation relies on.
pub(crate) fn msg_ref_from_uuid(uuid_str: &str) -> String {
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
    use crate::config::PartyConfig;
    use crate::party_registry::MpIdRegistry;
    use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
    use mako_engine::outbox::OutboxMessage;

    pub(super) fn test_registry(mp_id: &str) -> MpIdRegistry {
        let party = PartyConfig {
            mp_id: mp_id.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
        };
        MpIdRegistry::from_config(&[party]).expect("test registry")
    }

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
    fn utilmd_dtm_qualifier_by_pid() {
        assert_eq!(utilmd_dtm_qualifier(55001), "163");
        assert_eq!(utilmd_dtm_qualifier(55002), "164");
        assert_eq!(utilmd_dtm_qualifier(55016), "163");
        assert_eq!(utilmd_dtm_qualifier(44001), "163");
        assert_eq!(utilmd_dtm_qualifier(44002), "164");
    }

    #[test]
    fn render_mscons_without_the_identifying_tuple_is_a_missing_field() {
        // A Summenzeitreihe is keyed by (MaBiS-ZP, Bilanzierungsmonat, Version).
        // Rendering one without them would produce a message the BIKO cannot
        // place on the settlement grid.
        let msg = fake_msg(
            "MSCONS",
            "9900987654321",
            serde_json::json!({
                "pid": 13003_u32,
                "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--A",
            }),
        );
        let result = render_to_wire_bytes(&msg, &test_registry("9900123456789"));
        assert!(
            matches!(result, Err(RenderError::MissingField { ref field, .. }) if field.contains("balancing_period")),
            "expected a missing balancing_period, got {result:?}"
        );
    }

    #[test]
    fn render_mscons_13015_carries_work_and_up_to_two_maxima() {
        // AHB 3.2: SG9 repeats two to three times — once for the energy from
        // the start of the calendar year to Lieferbeginn, then one or two
        // monthly maxima, each with the month it fell in (DTM+306).
        let msg = fake_msg(
            "MSCONS",
            "9900987654321",
            serde_json::json!({
                "pid": 13015_u32,
                "sender_mp_id": "9900357000004",
                "receiver_mp_id": "9900987654321",
                "malo_id": "51238696781",
                "arbeit": {
                    "quantity": "184500.000",
                    "from": "202601010000+00",
                    "to": "202605010000+00",
                },
                "leistungsmaxima": [
                    { "quantity": "412.5", "period": "202602" },
                    { "quantity": "398.0", "period": "202601", "ersatzwert": true },
                ],
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("13015 must render");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");

        // Work: DE 6063 `220` (Wahrer Wert), bounded by the billing period.
        assert!(wire.contains("QTY+220:184500.000:KWH"), "{wire}");
        assert!(wire.contains("DTM+163:202601010000?+00:303"), "{wire}");
        assert!(wire.contains("DTM+164:202605010000?+00:303"), "{wire}");

        // Maxima: power, each with the month it occurred in.
        assert!(wire.contains("QTY+220:412.5:KWT"), "{wire}");
        assert!(wire.contains("DTM+306:202602:610"), "{wire}");
        // A substitute maximum must be declared as one, not reported as measured.
        assert!(wire.contains("QTY+67:398.0:KWT"), "{wire}");
        assert!(wire.contains("DTM+306:202601:610"), "{wire}");

        // Three line items: one work entry plus two maxima.
        assert_eq!(wire.matches("LIN+").count(), 3, "{wire}");
    }

    #[test]
    fn render_orders_esa_17007_17008_are_mig_conformant() {
        for pid in [17007_u32, 17008] {
            let msg = fake_msg(
                "ORDERS",
                "9900357000004",
                serde_json::json!({
                    "pid": pid,
                    "sender_mp_id": "9900555000005",
                    "receiver_mp_id": "9900357000004",
                    "malo_id": "51238696781",
                    "location": "51238696781",
                }),
            );
            let wire = render_to_wire_bytes(&msg, &test_registry("9900555000005"))
                .unwrap_or_else(|e| panic!("{pid} renders: {e:?}"));
            let wire = String::from_utf8(wire.bytes).expect("utf-8");
            edi_energy::EdiEnergyMessage::validate(
                &edi_energy::parse(wire.as_bytes()).expect("parse"),
            )
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("ORDERS {pid} must be MIG-conformant: {e}\n{wire}"));
        }
    }

    #[test]
    fn render_ordrsp_esa_answers_are_mig_conformant() {
        for pid in [19011_u32, 19012, 19013, 19014] {
            let msg = fake_msg(
                "ORDRSP",
                "9900555000005",
                serde_json::json!({
                    "pid": pid,
                    "sender": "9900357000004",
                    "receiver": "9900555000005",
                    "order_reference": "ESA-BE-0001",
                }),
            );
            let wire = render_to_wire_bytes(&msg, &test_registry("9900357000004"))
                .unwrap_or_else(|e| panic!("{pid} renders: {e:?}"));
            let wire = String::from_utf8(wire.bytes).expect("utf-8");
            assert!(!wire.contains("LOC+"), "ORDRSP carries no LOC: {wire}");
            assert!(wire.contains("BGM+7+"), "BGM 1001 = 7: {wire}");
            edi_energy::EdiEnergyMessage::validate(
                &edi_energy::parse(wire.as_bytes()).expect("parse"),
            )
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("ORDRSP {pid} must be MIG-conformant: {e}\n{wire}"));
        }
    }

    #[test]
    fn ordrsp_and_ordchg_carry_a_correlatable_order_reference() {
        // ORDRSP echoes the answered order in RFF+ACW; ORDCHG references the
        // original Bestellung in RFF+ON. The ingest dispatcher resumes the
        // LOC-less message by this reference.
        let ordrsp = fake_msg(
            "ORDRSP",
            "9900555000005",
            serde_json::json!({
                "pid": 19011,
                "sender": "9900357000004",
                "receiver": "9900555000005",
                "order_reference": "ORD-ABC-1",
            }),
        );
        let wire = render_to_wire_bytes(&ordrsp, &test_registry("9900357000004")).unwrap();
        let msg = edi_energy::parse(&wire.bytes).expect("parse ordrsp");
        assert_eq!(
            crate::ingest_dispatcher::extract_order_ref_from_msg(&msg),
            "ORD-ABC-1",
        );

        let ordchg = fake_msg(
            "ORDCHG",
            "9900357000004",
            serde_json::json!({
                "pid": 39002,
                "sender": "9900555000005",
                "receiver": "9900357000004",
                "order_reference": "ORD-ABC-1",
            }),
        );
        let wire = render_to_wire_bytes(&ordchg, &test_registry("9900555000005")).unwrap();
        let msg = edi_energy::parse(&wire.bytes).expect("parse ordchg");
        assert_eq!(
            crate::ingest_dispatcher::extract_order_ref_from_msg(&msg),
            "ORD-ABC-1",
            "the Stornierung must reference the original Bestellung"
        );
    }

    #[test]
    fn render_ordchg_39002_is_mig_conformant_and_carries_no_loc() {
        let msg = fake_msg(
            "ORDCHG",
            "9900357000004",
            serde_json::json!({
                "pid": 39002_u32,
                "sender": "9900555000005",
                "receiver": "9900357000004",
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900555000005")).expect("39002 renders");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");
        // ORDCHG has no LOC in any profile — it must not be emitted.
        assert!(!wire.contains("LOC+"), "ORDCHG carries no LOC: {wire}");
        assert!(
            wire.contains("RFF+"),
            "the mandatory SG1 RFF is present: {wire}"
        );
        edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("39002 Stornierung must be MIG-conformant: {e}\n{wire}"));
    }

    #[test]
    fn render_reqote_35002_is_mig_conformant() {
        let msg = fake_msg(
            "REQOTE",
            "9900357000004",
            serde_json::json!({
                "pid": 35002_u32,
                "sender": "9900555000005",
                "receiver": "9900357000004",
                "location": "51238696781",
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900555000005")).expect("35002 renders");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");
        edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("35002 Werteanfrage must be MIG-conformant: {e}\n{wire}"));
    }

    #[test]
    fn render_quotes_angebot_carries_the_bindungsfrist_ablehnung_carries_the_reason() {
        let esa = "9900555000005";
        // Angebot: Bindungsfrist as DTM+273 (the MIG-permitted DE 2005), no FTX.
        let angebot = fake_msg(
            "QUOTES",
            esa,
            serde_json::json!({
                "pid": 15003_u32,
                "sender": "9900357000004",
                "receiver": esa,
                "location": "51238696781",
                "bindungsfrist": "20260316",
            }),
        );
        let wire = render_to_wire_bytes(&angebot, &test_registry("9900357000004"))
            .expect("angebot renders");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");
        // DE 2005 = 273 (the MIG-permitted Bindungsfrist qualifier), not Z12.
        assert!(wire.contains("DTM+273:20260316:102"), "{wire}");
        assert!(
            !wire.contains("FTX+"),
            "an Angebot carries no reason: {wire}"
        );
        // Full MIG + AHB conformance: the 15003 Angebot validates clean.
        edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("15003 Angebot must be MIG-conformant: {e}"));

        // Ablehnung: reason as FTX+ACB (the only MIG-permitted DE 4451), no
        // Bindungsfrist.
        let ablehnung = fake_msg(
            "QUOTES",
            esa,
            serde_json::json!({
                "pid": 15003_u32,
                "sender": "9900357000004",
                "receiver": esa,
                "location": "51238696781",
                "reason": "Messprodukt nicht lieferbar",
            }),
        );
        let wire = render_to_wire_bytes(&ablehnung, &test_registry("9900357000004"))
            .expect("ablehnung renders");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");
        assert!(
            !wire.contains("DTM+273"),
            "an Ablehnung carries no Bindungsfrist: {wire}"
        );
        // DE 4451 = ACB (the only MIG-permitted FTX qualifier), not ABO.
        assert!(
            wire.contains("FTX+ACB") && wire.contains("Messprodukt nicht lieferbar"),
            "{wire}"
        );
        assert!(edi_energy::parse(wire.as_bytes()).is_ok());
    }

    #[test]
    fn quotes_angebot_and_ablehnung_round_trip_through_the_esa_adapter() {
        use mako_wim::esa_wertebestellung::EsaWertebestellungCommand as C;

        let esa = "9900555000005";
        let fv = crate::adapters::known_fvs()
            .into_iter()
            .next()
            .expect("a known format version");
        let registry = crate::adapters::esa_wertebestellung_registry();

        let render = |payload: serde_json::Value| -> Vec<u8> {
            render_to_wire_bytes(
                &fake_msg("QUOTES", esa, payload),
                &test_registry("9900357000004"),
            )
            .expect("render")
            .bytes
        };

        // Angebot (has Bindungsfrist) → ReceiveAngebot.
        let angebot = render(serde_json::json!({
            "pid": 15003_u32, "sender": "9900357000004", "receiver": esa,
            "location": "51238696781", "bindungsfrist": "20260316",
        }));
        let msg = edi_energy::parse(&angebot).expect("parse angebot");
        let cmd = registry
            .dispatch(&msg as &dyn std::any::Any, &fv)
            .expect("dispatch angebot");
        assert!(
            matches!(cmd, C::ReceiveAngebot { .. }),
            "Angebot must map to ReceiveAngebot"
        );

        // Ablehnung (no Bindungsfrist, FTX reason) → ReceiveAnfrageAblehnung.
        let ablehnung = render(serde_json::json!({
            "pid": 15003_u32, "sender": "9900357000004", "receiver": esa,
            "location": "51238696781", "reason": "Messprodukt nicht lieferbar",
        }));
        let msg = edi_energy::parse(&ablehnung).expect("parse ablehnung");
        let cmd = registry
            .dispatch(&msg as &dyn std::any::Any, &fv)
            .expect("dispatch ablehnung");
        match cmd {
            C::ReceiveAnfrageAblehnung { reason } => {
                assert_eq!(reason.as_deref(), Some("Messprodukt nicht lieferbar"));
            }
            _ => panic!("Ablehnung must map to ReceiveAnfrageAblehnung"),
        }
    }

    #[test]
    fn render_mscons_13027_addresses_the_esa_and_carries_the_intervals() {
        // UC 4.2: the MSB delivers "Werte nach Typ 2" to an ESA — a recipient
        // that is neither NB nor LF. The recipient must appear as NAD+MR.
        let esa = "9900555000005";
        let msg = fake_msg(
            "MSCONS",
            esa,
            serde_json::json!({
                "pid": 13027_u32,
                "sender_mp_id": "9900357000004",
                "receiver_mp_id": esa,
                "malo_id": "51238696781",
                "reads": [
                    { "dtm_from": "202603100000+00", "dtm_to": "202603100015+00",
                      "quantity_kwh": "0.250", "obis_code": "1-0:1.29.0" },
                    { "dtm_from": "202603100015+00", "dtm_to": "202603100030+00",
                      "quantity_kwh": "0.310", "obis_code": "1-0:1.29.0" }
                ]
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900357000004")).expect("13027 must render");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");

        // The ESA is the recipient (NAD+MR) — the addressing this feature adds.
        assert!(wire.contains(&format!("NAD+MR+{esa}")), "{wire}");
        // PID 13027 travels in RFF+Z13.
        assert!(wire.contains("RFF+Z13:13027"), "{wire}");
        // Both quarter-hour Wirkarbeit values, under one OBIS line item.
        assert!(wire.contains("QTY+220:0.250:KWH"), "{wire}");
        assert!(wire.contains("QTY+220:0.310:KWH"), "{wire}");
        assert_eq!(wire.matches("LIN+").count(), 1, "one OBIS register: {wire}");

        // The message re-parses cleanly.
        let parsed = edi_energy::parse(wire.as_bytes());
        assert!(parsed.is_ok(), "13027 must round-trip: {parsed:?}");
    }

    #[test]
    fn render_mscons_13015_refuses_more_than_two_maxima() {
        let msg = fake_msg(
            "MSCONS",
            "9900987654321",
            serde_json::json!({
                "pid": 13015_u32,
                "malo_id": "51238696781",
                "arbeit": { "quantity": "1", "from": "202601010000+00", "to": "202602010000+00" },
                "leistungsmaxima": [
                    { "quantity": "1", "period": "202601" },
                    { "quantity": "2", "period": "202602" },
                    { "quantity": "3", "period": "202603" },
                ],
            }),
        );
        let result = render_to_wire_bytes(&msg, &test_registry("9900123456789"));
        assert!(
            matches!(result, Err(RenderError::InsufficientPayload { .. })),
            "expected a refusal, got {result:?}"
        );
    }

    #[test]
    fn each_mscons_use_case_carries_its_own_bgm_document_code() {
        // BGM DE 1001 names what kind of document the message is, and the
        // receiver routes by it. It is not constant across MSCONS: sending the
        // default `7` would label a Summenzeitreihe a Prozessdatenbericht.
        for (pid, expected) in [
            (13003_u64, "BGM+BK"), // Zeitreihen im Rahmen der Bilanzkreisabrechnung
            (13023, "BGM+Z46"),    // Redispatch
            (13015, "BGM+Z27"),    // Bewegungsdaten im Kalenderjahr vor Lieferbeginn
            (13016, "BGM+Z28"),    // Energiemenge und Leistungsmaximum
            (13019, "BGM+7"),      // Prozessdatenbericht
        ] {
            let payload = if pid == 13003 || pid == 13023 {
                serde_json::json!({
                    "pid": pid,
                    "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--A",
                    "balancing_period": "202606",
                    "version": "20260714050000+00",
                    "intervals": [
                        { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "1" },
                    ],
                })
            } else {
                serde_json::json!({
                    "pid": pid,
                    "malo_id": "51238696781",
                    "arbeit": {
                        "quantity": "1",
                        "from": "202601010000+00",
                        "to": "202602010000+00",
                    },
                })
            };
            let msg = fake_msg("MSCONS", "9900987654321", payload);
            let wire = render_to_wire_bytes(&msg, &test_registry("9900123456789"))
                .unwrap_or_else(|e| panic!("PID {pid} must render: {e:?}"));
            let wire = String::from_utf8(wire.bytes).expect("utf-8");
            assert!(
                wire.contains(expected),
                "PID {pid} must carry {expected}, got: {wire}"
            );
        }
    }

    #[test]
    fn render_mscons_13019_refuses_a_leistungsmaximum() {
        // The AHB marks no Leistungsperiode row for 13019, so a maximum sent
        // under it would carry no period to be attributed to.
        let msg = fake_msg(
            "MSCONS",
            "9900987654321",
            serde_json::json!({
                "pid": 13019_u32,
                "malo_id": "51238696781",
                "arbeit": { "quantity": "1", "from": "202601010000+00", "to": "202602010000+00" },
                "leistungsmaxima": [{ "quantity": "5", "period": "202601" }],
            }),
        );
        let result = render_to_wire_bytes(&msg, &test_registry("9900123456789"));
        assert!(
            matches!(result, Err(RenderError::InsufficientPayload { ref detail, .. }) if detail.contains("13016")),
            "expected a refusal pointing at 13016, got {result:?}"
        );
    }

    #[test]
    fn render_mscons_refuses_an_unrecognised_unit() {
        // DE 6411 is a closed code list (MIG 2.5). A typo must not reach the
        // wire as a syntactically valid but uninterpretable unit.
        let msg = fake_msg(
            "MSCONS",
            "9900987654321",
            serde_json::json!({
                "pid": 13015_u32,
                "malo_id": "51238696781",
                "arbeit": {
                    "quantity": "1",
                    "from": "202601010000+00",
                    "to": "202602010000+00",
                    "unit": "kWh",
                },
            }),
        );
        let result = render_to_wire_bytes(&msg, &test_registry("9900123456789"));
        assert!(
            matches!(result, Err(RenderError::InsufficientPayload { .. })),
            "lower-case `kWh` is not the DE 6411 code `KWH`, got {result:?}"
        );
    }

    #[test]
    fn render_mscons_refuses_an_unsupported_pid() {
        // A payload for an unimplemented Anwendungsfall rendered in the
        // Summenzeitreihe shape would be syntactically valid and mean something
        // the sender did not say.
        let msg = fake_msg(
            "MSCONS",
            "9900987654321",
            serde_json::json!({
                "pid": 13021_u32,
                "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--A",
                "balancing_period": "202606",
                "version": "20260714050000+00",
                "intervals": [
                    { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "1" },
                ],
            }),
        );
        let result = render_to_wire_bytes(&msg, &test_registry("9900123456789"));
        assert!(
            matches!(result, Err(RenderError::InsufficientPayload { ref detail, .. }) if detail.contains("13021")),
            "expected a refusal naming the PID, got {result:?}"
        );
    }

    #[test]
    fn render_mscons_renders_the_redispatch_ausfallarbeit_series() {
        // 13023 shares the summed-series shape, so it renders through the same
        // path as 13003.
        let msg = fake_msg(
            "MSCONS",
            "9900987654321",
            serde_json::json!({
                "pid": 13023_u32,
                "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--A",
                "balancing_period": "202606",
                "version": "20260714050000+00",
                "intervals": [
                    { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "7.5" },
                ],
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("13023 must render");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");
        // DE 6063 `79` = Energiemenge summiert (MSCONS AHB 3.2, SG10 QTY).
        assert!(wire.contains("QTY+79:7.5:KWH"), "{wire}");
        assert!(wire.contains("DTM+293:20260714050000?+00:304"), "{wire}");
    }

    #[test]
    fn render_mscons_emits_the_summenzeitreihe_slots() {
        let msg = fake_msg(
            "MSCONS",
            "9900077000006",
            serde_json::json!({
                "pid": 13003_u32,
                "sender_mp_id": "9900357000004",
                "receiver_mp_id": "9900077000006",
                "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--A",
                "balancing_period": "202606",
                "version": "20260714050000+00",
                "intervals": [
                    { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "12.5" },
                    { "from": "202606010015+00", "to": "202606010030+00", "quantity_kwh": "13.0" },
                ],
            }),
        );
        let wire = render_to_wire_bytes(&msg, &test_registry("9900123456789"))
            .expect("MSCONS 13003 must render");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");

        assert!(wire.contains("DTM+492:202606:610"), "{wire}");
        assert!(wire.contains("DTM+293:20260714050000?+00:304"), "{wire}");
        assert!(wire.contains("QTY+79:12.5:KWH"), "{wire}");
        assert!(wire.contains("QTY+79:13.0:KWH"), "{wire}");
        // Every quantity carries its own slot bounds.
        assert_eq!(wire.matches("DTM+163:").count(), 2, "{wire}");
        assert_eq!(wire.matches("DTM+164:").count(), 2, "{wire}");
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
        let result = render_to_wire_bytes(&msg, &test_registry("9900123456789"));
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
        let result = render_to_wire_bytes(&msg, &test_registry("9900123456789"));
        // Either succeeds (if a profile is registered) or NoActiveProfile.
        // Never MissingField or InsufficientPayload.
        match &result {
            Ok(_) => {}
            Err(RenderError::NoActiveProfile { .. }) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}

#[cfg(test)]
mod envelope_tests {
    use super::tests::test_registry;
    use super::*;

    fn outbox_msg(message_type: &str, payload: serde_json::Value) -> OutboxMessage {
        use mako_engine::ids::{
            ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId,
        };
        let tenant = TenantId::from_party_id("9900123456789");
        let process = ProcessId::new();
        OutboxMessage::new(
            StreamId::for_process(tenant, &process),
            process,
            tenant,
            CorrelationId::new(),
            ConversationId::new(),
            EventId::new(),
            message_type,
            "9900987654321",
            payload,
        )
    }

    /// AF 6.1d, Kap. 2: the Übertragungsdatei carries UNB/UNZ, and the UNB
    /// MP-IDs equal the NAD+MS / NAD+MR MP-IDs. The DAR in UNB DE0020 is
    /// repeated in UNZ DE0036.
    #[test]
    fn envelope_identities_match_nad_and_unz_repeats_dar() {
        let msg = outbox_msg(
            "APERAK",
            serde_json::json!({
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "orig_message_ref": "ABC123",
            }),
        );
        let rendered =
            render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("must render");
        let wire = String::from_utf8(rendered.bytes.clone()).expect("utf-8");

        // Envelope present, exactly one message.
        assert!(wire.starts_with("UNB+UNOC:3+"), "{wire}");
        assert!(wire.contains(&format!("UNZ+1+{}'", rendered.dar)), "{wire}");

        // UNB DE0004/DE0010 with BDEW qualifier 500 (99…-prefixed MP-IDs),
        // AF 6.1d UNB segment table.
        assert!(
            wire.contains("UNB+UNOC:3+9900123456789:500+9900987654321:500+"),
            "{wire}"
        );
        // …and identical to the NAD MP-IDs (AF 6.1d: "Die im UNB- und
        // NAD-Segment … verwendeten MP-ID sind identisch").
        assert!(wire.contains("NAD+MS+9900123456789"), "{wire}");
        assert!(wire.contains("NAD+MR+9900987654321"), "{wire}");
        assert_eq!(rendered.sender_mp_id.as_ref(), "9900123456789");
        assert_eq!(rendered.receiver_mp_id.as_ref(), "9900987654321");

        // DAR is stable across retries: derived from the message id.
        let again = render_to_wire_bytes(&msg, &test_registry("9900123456789")).unwrap();
        assert_eq!(rendered.dar, again.dar);

        // And the whole interchange parses back.
        edi_energy::Platform::with_all_profiles()
            .parse(&rendered.bytes)
            .expect("envelope must be parseable");
    }

    /// DE0007 qualifier derivation per AF 6.1d UNB segment table:
    /// 14 = GS1, 500 = DE BDEW, 502 = DE DVGW.
    #[test]
    fn unb_qualifier_per_af61d() {
        assert_eq!(unb_qualifier("9900123456789"), "500");
        assert_eq!(unb_qualifier("9870123456789"), "502");
        assert_eq!(unb_qualifier("4012345000023"), "14");
        assert_eq!(unb_qualifier("10XDE-EON-NETZ-I"), "500");
    }

    // ── ESA outbound leg — the MSB's answers on the wire ──────────────────────

    /// The rendered QUOTES/ORDRSP must parse back and carry the ESA
    /// Prüfidentifikator (BGM DE 1004): otherwise the answer cannot close the
    /// 5-WT / 2-WT windows the inbound leg armed.
    fn assert_esa_roundtrip(message_type: &str, pid: u32) {
        use edi_energy::EdiEnergyMessage as _;
        let msg = outbox_msg(
            message_type,
            serde_json::json!({
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "pid": pid,
                "order_reference": "ANFRAGE-REF-1",
            }),
        );
        let rendered = render_to_wire_bytes(&msg, &test_registry("9900123456789"))
            .unwrap_or_else(|e| panic!("render {message_type} {pid} must succeed: {e}"));
        let parsed = edi_energy::Platform::with_all_profiles()
            .parse(&rendered.bytes)
            .unwrap_or_else(|e| panic!("{message_type} {pid} must parse back: {e}"));
        let detected = parsed
            .detect_pruefidentifikator()
            .unwrap_or_else(|e| panic!("{message_type} {pid}: PID must be detectable: {e}"));
        assert_eq!(detected.as_u32(), pid, "round-trip PID for {message_type}");
        let wire = String::from_utf8(rendered.bytes).expect("utf-8");
        assert!(
            wire.contains("RFF+ACW:ANFRAGE-REF-1"),
            "order reference: {wire}"
        );
    }

    #[test]
    fn esa_quotes_angebot_15003_roundtrips() {
        assert_esa_roundtrip("QUOTES", 15003);
    }

    #[test]
    fn esa_ordrsp_answers_roundtrip() {
        // 19011/19012 Ab-/Bestellung, 19013/19014 Stornierung.
        for pid in [19011, 19012, 19013, 19014] {
            assert_esa_roundtrip("ORDRSP", pid);
        }
    }
}
