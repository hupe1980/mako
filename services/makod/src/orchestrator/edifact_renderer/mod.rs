//! EDIFACT wire-format renderer for domain-intent outbox payloads.
//!
//! Converts `OutboxMessage.payload` (domain-intent JSON) → BDEW-conformant
//! EDIFACT wire bytes using the `edi-energy` builder infrastructure.
//!
//! # Design
//!
//! Each domain workflow enqueues a [`PendingOutbox`] with:
//! - `message_type`:  EDIFACT type code (e.g. `"UTILMD"`, `"APERAK"`)
//! - `recipient`:     trading-partner MP-ID
//! - `payload`:       domain-intent JSON (sender/receiver MP-IDs, process dates, …)
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
//! | ORDERS | `pid`, `orders_ref` (sender = `tenant_party_id`, receiver = `msg.recipient`); ESA 17007/17008 additionally `korrelation_ref`, `abonnement`, `ausfuehrungsdatum` |
//! | REQOTE | `pid`, `sender`, `receiver`, `message_ref`; ESA 35003 additionally `location`, `messprodukt`, `wunschtermin`, `smgw` |
//! | ORDCHG | `pid`, `sender`, `receiver`, `korrelation_ref` |
//! | ORDRSP | `pid` (ESA 19011–19014), `sender`, `receiver`, `korrelation_ref`, `abonnement`, `antwort_code`, `antwort_codeliste`, `document_id`, `document_date`, `message_ref` |
//! | QUOTES | `pid` (ESA 15003), `sender`, `receiver`, `korrelation_ref`, `bindungsfrist_tage`, `fruehester_start`, `messprodukt`, `artikel_ids`, `preise`, `document_id`, `document_date`, `message_ref` |
//! | INVOIC | `sender`, `receiver`, `document_id`, `document_code`, `document_date`, `message_ref` |
//! | REMADV | `sender`, `receiver`, `document_id`, `document_code`, `document_date`, `message_ref` |
//! | IFTSTA | `pid` (WiM 21042), `sender`, `receiver`, `sts_code`, `korrelation_ref`, `beendigung_zum`, `document_id`, `document_date`, `message_ref` |
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
pub fn is_insufficient_payload(err: &RenderError) -> bool {
    matches!(err, RenderError::InsufficientPayload { .. })
}

/// Returns `true` when the message should be suppressed — no wire EDIFACT sent.
///
/// Used for Gas positive APERAKs (silence = acceptance per APERAK AHB 1.0 §2.3).
/// The AS4 sender acknowledges the outbox entry without transmitting.
/// The ERP webhook sender delivers the domain JSON payload instead.
pub fn is_suppressed(err: &RenderError) -> bool {
    matches!(err, RenderError::Suppressed { .. })
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Render a domain-intent [`OutboxMessage`] to BDEW-conformant EDIFACT wire bytes.
///
/// `registry` provides the operator's own MP-ID(s).  For ORDERS messages, the
/// sender MP-ID is resolved from `payload["sender"]` when present; otherwise the
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
        "IFTSTA" => render_iftsta(p, msg, registry),
        "INSRPT" => render_insrpt(p, msg, registry),
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
    finish_interchange_with_app_ref(serialized, sender, receiver, msg, None)
}

/// [`finish_interchange`] that also sets the UNB **Anwendungsreferenz**
/// (DE 0026), where the AHB fixes one — `TL` on the MSCONS Werte-nach-Typ-2
/// interchange (MSCONS AHB 3.2 §11.2).
fn finish_interchange_with_app_ref(
    serialized: Result<Vec<u8>, edi_energy::Error>,
    sender: &str,
    receiver: &str,
    msg: &OutboxMessage,
    application_reference: Option<&str>,
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

    let mut interchange = edi_energy::builders::InterchangeBuilder::new(sender, receiver, &dar)
        .transmission(&date, &hhmm);
    if let Some(app_ref) = application_reference {
        interchange = interchange.application_reference(app_ref);
    }
    let bytes = interchange
        .message(message)
        .build()
        .map_err(|e| RenderError::BuilderError(e.to_string()))?;

    Ok(RenderedInterchange {
        bytes,
        sender_mp_id: sender.into(),
        receiver_mp_id: receiver.into(),
        dar: dar.into(),
    })
}

// ── Per-message-type renderer submodules ─────────────────────────────────────

mod aperak;
mod contrl;
mod iftsta;
mod insrpt;
mod invoic;
mod mscons;
mod orders;
mod utilmd;

use aperak::*;
use contrl::*;
use iftsta::*;
use insrpt::*;
use invoic::*;
use mscons::*;
use orders::*;
use utilmd::*;
// ── Helpers ───────────────────────────────────────────────────────────────────

/// Resolve the active `Release` for `(message_type, track)` from today's registry.
fn active_release(message_type: MessageType, track: ReleaseTrack) -> Option<Release> {
    let today = mako_fristen::heute();
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

    /// **A REMADV names its Prüfidentifikator and the invoice it answers.**
    ///
    /// `RFF+Z13` and the whole `SG5` block (`DOC`, `MOA+9`, `MOA+12`,
    /// `DTM+137`) plus `SG4 CUX` are Muss on every use case REMADV AHB 1.0a
    /// publishes. Without the first, the answer routes to no process on the
    /// other side; without the second, it answers nothing.
    #[test]
    fn a_remadv_carries_its_pid_and_the_invoice_it_answers() {
        let msg = fake_msg(
            "REMADV",
            "9900357000004",
            serde_json::json!({
                "pid": 33_004,
                "sender": "9905550000005",
                "receiver": "9900357000004",
                "message_ref": "REMADV000001",
                "document_code": "239",
                "document_id": "AVIS-2026-7",
                "document_date": "2026-04-02",
                "rechnungsbezug": {
                    "dokumentenart": "380",
                    "rechnungsnummer": "RE-2026-4711",
                    "faelliger_betrag": "16.60",
                    // Condition [926]: an Abweisung transfers nothing.
                    "ueberweisungsbetrag": "0",
                    "rechnungsdatum": "2026-04-01",
                },
                "antwort_codeliste": "E_0264",
                "antwort_befunde": [
                    {
                        "code": "A11",
                        "ebene": "position",
                        "positionsnummer": 7,
                        "detail": "Preis weicht vom Angebot ab",
                    },
                    {
                        "code": "A20",
                        "ebene": "position",
                        "positionsnummer": 7,
                        "detail": "Rechenfehler",
                    },
                ],
                "ablehnungsgrund": "Preis weicht vom Angebot ab",
            }),
        );
        let rendered =
            render_to_wire_bytes(&msg, &test_registry("9905550000005")).expect("REMADV renders");
        let wire = String::from_utf8_lossy(&rendered.bytes);

        assert!(wire.contains("RFF+Z13:33004"), "{wire}");
        assert!(wire.contains("CUX+2:EUR:11"), "{wire}");
        assert!(wire.contains("DOC+380+RE-2026-4711"), "{wire}");
        assert!(wire.contains("MOA+9:16.60"), "{wire}");
        assert!(wire.contains("MOA+12:0"), "{wire}");
        // `SG10 DLI` names the Positionsnummer and `SG12 AJT` every code that
        // position was refused with — repeated „bis alle Fehler der
        // Positionsebene genannt sind" (condition [525]). One code where the
        // walk found two tells the issuer to correct half of it.
        assert!(wire.contains("DLI+1:7"), "{wire}");
        // The ESA tree, not the Netznutzungs one — `A11` means nothing in
        // `E_0406`, and DE 1082 is what the counterparty resolves against.
        assert!(wire.contains("AJT+A11+E_0264"), "{wire}");
        assert!(wire.contains("AJT+A20+E_0264"), "{wire}");
        assert!(
            wire.contains("UNS+S"),
            "Trennung Positions-/Summenteil: {wire}"
        );
        assert!(wire.contains("BGM+239+AVIS-2026-7"), "{wire}");
        edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("REMADV 33004 must be MIG-conformant: {e:#?}\n{wire}"));
    }

    /// A Zahlungsavis carries **no** `AJT` — agreement needs no Antwortcode
    /// (REMADV AHB 1.0a § 3.1.1) — and its BGM code is `481`, not `239`.
    #[test]
    fn a_zahlungsavis_states_no_antwortcode() {
        let msg = fake_msg(
            "REMADV",
            "9900357000004",
            serde_json::json!({
                "pid": 33_001,
                "sender": "9905550000005",
                "receiver": "9900357000004",
                "message_ref": "REMADV000002",
                "document_code": "481",
                "rechnungsbezug": {
                    "dokumentenart": "380",
                    "rechnungsnummer": "RE-2026-4711",
                    "faelliger_betrag": "16.60",
                    "ueberweisungsbetrag": "16.60",
                    "rechnungsdatum": "2026-04-01",
                },
            }),
        );
        let rendered =
            render_to_wire_bytes(&msg, &test_registry("9905550000005")).expect("REMADV renders");
        let wire = String::from_utf8_lossy(&rendered.bytes);
        assert!(wire.contains("RFF+Z13:33001"), "{wire}");
        assert!(wire.contains("BGM+481"), "{wire}");
        assert!(
            !wire.contains("AJT+"),
            "a Zahlungsavis carries no AJT: {wire}"
        );
        // …and the Überweisungsbetrag is the fällige Betrag, not zero.
        assert!(wire.contains("MOA+12:16.60"), "{wire}");
        edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("REMADV 33001 must be MIG-conformant: {e}\n{wire}"));
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

    /// The SG4 process date carries the qualifier the MIG defines for it.
    ///
    /// `163`/`164` are absent on purpose: UTILMD uses them for *Beginn* and
    /// *Ende Messperiode* inside SG8/SG9, never for a SG4 process date.
    /// Returning `163` for every PID and `164` for the Anmeldung
    /// *confirmations* puts a qualifier UTILMD does not define at SG4 on a
    /// message that marks the Anmeldung as a delivery *end*.
    #[test]
    fn utilmd_dtm_qualifier_by_pid() {
        use edi_energy::utilmd_codes::dtm;

        // Lieferbeginn — Anmeldung and both its answers: „Beginn zum".
        assert_eq!(utilmd_dtm_qualifier(55001), dtm::BEGINN_ZUM);
        assert_eq!(utilmd_dtm_qualifier(55002), dtm::BEGINN_ZUM);
        assert_eq!(utilmd_dtm_qualifier(55003), dtm::BEGINN_ZUM);
        assert_eq!(utilmd_dtm_qualifier(44001), dtm::BEGINN_ZUM);
        assert_eq!(utilmd_dtm_qualifier(44002), dtm::BEGINN_ZUM);

        // Lieferende, Beendigung der Zuordnung and Kündigung: „Ende zum".
        assert_eq!(utilmd_dtm_qualifier(55004), dtm::ENDE_ZUM);
        assert_eq!(utilmd_dtm_qualifier(55007), dtm::ENDE_ZUM);
        assert_eq!(utilmd_dtm_qualifier(55010), dtm::ENDE_ZUM);
        assert_eq!(utilmd_dtm_qualifier(55016), dtm::ENDE_ZUM);
        assert_eq!(utilmd_dtm_qualifier(44007), dtm::ENDE_ZUM);
        assert_eq!(utilmd_dtm_qualifier(44016), dtm::ENDE_ZUM);

        // Stammdatenänderung: „Änderung zum".
        assert_eq!(utilmd_dtm_qualifier(55109), dtm::AENDERUNG_ZUM);
        assert_eq!(utilmd_dtm_qualifier(55616), dtm::AENDERUNG_ZUM);

        // …but the two GPKE Teil 2 families that share the 556xx block are
        // not Stammdatenänderungen. Neuanlage and Ankündigung Zuordnung LF
        // both mark `SG4 DTM+92` „Datum Vertragsbeginn" Muss (UTILMD AHB
        // Strom 2.2), and `157` is not a qualifier their AHB defines.
        for pid in [55600_u32, 55601, 55602, 55603, 55604, 55605, 55607, 55608] {
            assert_eq!(
                utilmd_dtm_qualifier(pid),
                dtm::BEGINN_ZUM,
                "PID {pid} names a Vertragsbeginn, not a Stammdatenänderung"
            );
        }

        // WiM Messstellenbetrieb: the planned execution date.
        assert_eq!(utilmd_dtm_qualifier(55042), dtm::LEISTUNGSBEGINN_GEPLANT);

        // Nothing may resolve to a Messperioden-Qualifier.
        for pid in [55001_u32, 55007, 55010, 55016, 55042, 44001, 44007, 44016] {
            let q = utilmd_dtm_qualifier(pid);
            assert!(
                q != "163" && q != "164",
                "PID {pid} resolved to the Messperioden-Qualifier {q}"
            );
        }
    }

    /// **The Bilanzkreis is a Produktpaket, not a remark.** UTILMD AHB Strom 2.2
    /// Kap. 5.3 makes `SG8 SEQ+Z79` Muss on 55001, 55077, 55600, 55601, 55014
    /// and 55608 — „ohne die Angabe eines für den LF gültigen Bilanzkreises
    /// `[kann]` der NB den LF der Marktlokation bzw. Tranche nicht zuordnen" —
    /// and Codeliste der Konfigurationen 1.4 Kap. 6.1.1 fixes the Produkt-Code.
    /// `SG4 FTX+ACB` is admitted on the Ablehnung only (Bedingung `[48]`), so a
    /// Bilanzkreis carried there rode a segment the Bestätigung may not have and
    /// left the Muss group empty.
    #[test]
    fn the_bilanzkreis_rides_the_produktpaket_not_the_bemerkung() {
        let msg = fake_msg(
            "UTILMD",
            "9900987654321",
            serde_json::json!({
                "pid": 55608_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "process_date": "20261101",
                "antwort_code": "A01",
                "antwort_codeliste": "E_0604",
                "bilanzkreis": "11XBK-EEG-----1",
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("55608 renders");
        let text = String::from_utf8_lossy(&wire.bytes);
        assert!(text.contains("SEQ+Z79+1"), "{text}");
        assert!(text.contains("PIA+5+9991000002082:Z11"), "{text}");
        assert!(text.contains("CCI+Z66"), "{text}");
        assert!(text.contains("CAV+ZV4:::11XBK-EEG-----1"), "{text}");
        // `SG8 SEQ+ZH0` is Muss wherever `SEQ+Z79` is.
        assert!(text.contains("SEQ+ZH0+1"), "{text}");
        assert!(text.contains("CCI+Z65+++Z01"), "{text}");
        assert!(
            !text.contains("FTX+ACB"),
            "a 55608 Bestätigung carries no FTX+ACB: {text}"
        );
        // …and it names a Vertragsbeginn, not a Stammdatenänderung.
        assert!(text.contains("DTM+92:"), "{text}");
        assert!(!text.contains("DTM+157"), "{text}");
    }

    /// **GeLi Gas states the Bilanzkreis in one segment, not a Produktpaket.**
    /// UTILMD AHB Gas 1.2 marks `SG10 CCI+Z19` DE 7037 Muss on a 44001; there is
    /// no `SG8 SEQ+Z79` anywhere in the Gas AHB, so sending the Strom shape here
    /// puts segments the receiving AHB does not define on the wire and leaves
    /// the Muss segment empty.
    #[test]
    fn the_gas_anmeldung_states_its_bilanzkreis_in_cci_z19() {
        let msg = fake_msg(
            "UTILMD",
            "9900987654321",
            serde_json::json!({
                "pid": 44001_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "process_date": "20261101",
                "transaktionsgrund": "E03",
                "bilanzkreis": "9870000000006",
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("44001 renders");
        let text = String::from_utf8_lossy(&wire.bytes);
        assert!(text.contains("CCI+Z19++9870000000006"), "{text}");
        assert!(
            !text.contains("SEQ+Z79"),
            "the Strom Produktpaket has no Gas counterpart: {text}"
        );
    }

    /// **Three Strom shapes a Gas UTILMD must not carry**, each a segment or a
    /// code the receiving AHB does not define for the Anwendungsfall:
    ///
    /// | Strom | Gas | Fundstelle |
    /// |---|---|---|
    /// | `NAD…::293` BDEW | `::332` DVGW | UTILMD AHB Gas Kap. 5, `SG2 NAD` DE 3055 |
    /// | `LOC+Z16` Marktlokation | `LOC+172` Meldepunkt | `SG5 LOC` DE 3227 — `172` in all 31 Anwendungsfälle |
    /// | `STS+7++E03+ZW4` | `STS+7++E03` | `ZW3`/`ZW4`/`ZW5`/`ZAP` appear nowhere in the Gas AHB |
    ///
    /// A `293` would also contradict the `502` the same interchange declares in
    /// UNB DE 0007 — one message naming two issuing offices for the same party.
    #[test]
    fn a_gas_utilmd_uses_the_gas_code_lists() {
        let msg = fake_msg(
            "UTILMD",
            "9870987654325",
            serde_json::json!({
                "pid": 44001_u32,
                "sender": "9870123456789",
                "receiver": "9870987654325",
                "malo": "51238696012",
                "process_date": "20261101",
                "transaktionsgrund": "E03",
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9870123456789")).expect("44001 renders");
        let text = String::from_utf8_lossy(&wire.bytes);
        assert!(text.contains("NAD+MS+9870123456789::332"), "{text}");
        assert!(text.contains("NAD+MR+9870987654325::332"), "{text}");
        assert!(!text.contains("::293"), "no BDEW code list on Gas: {text}");
        assert!(text.contains("LOC+172+51238696012"), "{text}");
        assert!(!text.contains("LOC+Z16"), "Gas names a Meldepunkt: {text}");
        assert!(text.contains("STS+7++E03'"), "{text}");
        assert!(
            !text.contains("ZW4"),
            "the Transaktionsgrundergänzung is Strom-only: {text}"
        );
    }

    /// The three Zuordnungs-Meldungen, as the AHB tables define them.
    ///
    /// Every assertion here is a column of UTILMD AHB Strom 2.1/2.2 Kap. 8.11
    /// or Gas 1.1/1.2 Kap. 5.8, and each is a way the message would otherwise
    /// have been wrong: 55036 is an `E01` Anmeldung carrying **no** SG4 date at
    /// all, 55037 an `E02` naming a `DTM+93` Vertragsende, 55038 an `E02`
    /// naming the originally confirmed `DTM+92` Vertragsbeginn — and every Gas
    /// twin is `E44` at a `LOC+172` Meldepunkt.
    #[test]
    fn the_zuordnungsmeldungen_render_as_their_ahb_columns() {
        // Rendered *and* validated: the six PIDs now carry AHB rules, so a
        // message mako emits is checked against the same profile a counterparty
        // would check it against.
        let render = |payload: serde_json::Value, own: &str| {
            let pid = payload["pid"].as_u64().expect("pid");
            let msg = fake_msg("UTILMD", "9900987654321", payload);
            let rendered = render_to_wire_bytes(&msg, &test_registry(own)).expect("renders");
            let wire = String::from_utf8_lossy(&rendered.bytes).into_owned();
            edi_energy::EdiEnergyMessage::validate(
                &edi_energy::parse(wire.as_bytes()).expect("parse"),
            )
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("PID {pid} must be AHB-conformant: {e:#?}\n{wire}"));
            wire
        };

        // 55036 — Information über existierende Zuordnung (NB → LFN).
        let info = render(
            serde_json::json!({
                "pid": 55_036_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "document_code": "E01",
                "transaktionsgrund": "Z26",
                "lokationstyp": "Z16",
                "referenz_vorgangsnummer": "VG-4711",
                "beteiligte_marktpartner": ["9900555000005", "9900111000002"],
            }),
            "9900123456789",
        );
        assert!(info.contains("BGM+E01+55036"), "{info}");
        assert!(
            !info.contains("DTM+92") && !info.contains("DTM+93"),
            "55036 has no SG4 date column at all: {info}"
        );
        assert!(info.contains("STS+7++Z26"), "{info}");
        assert!(info.contains("LOC+Z16+51238696012"), "{info}");
        // `SG6 RFF+TN` — Muss on 55036, and the only thing tying it to the
        // Anmeldung it refers to.
        assert!(info.contains("RFF+TN:VG-4711"), "{info}");
        // Bedingung [518]: *all* Altlieferanten, so SG12 repeats.
        assert_eq!(info.matches("NAD+VY+").count(), 2, "{info}");

        // 55037 — Beendigung der Zuordnung (NB → LFA).
        let beendigung = render(
            serde_json::json!({
                "pid": 55_037_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "document_code": "E02",
                "transaktionsgrund": "ZC8",
                "process_date": "20261101",
                "lokationstyp": "Z16",
            }),
            "9900123456789",
        );
        assert!(beendigung.contains("BGM+E02+55037"), "{beendigung}");
        assert!(beendigung.contains("DTM+93:"), "Ende zum: {beendigung}");
        assert!(beendigung.contains("STS+7++ZC8"), "{beendigung}");
        assert!(
            !beendigung.contains("NAD+VY"),
            "55037 names no third party: {beendigung}"
        );

        // 55038 — Aufhebung einer zukünftigen Zuordnung (NB → LFZ). `DTM+92`,
        // „ursprünglich vom NB bestätigtes Beginndatum" (Bedingung [507]).
        let aufhebung = render(
            serde_json::json!({
                "pid": 55_038_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "document_code": "E02",
                "transaktionsgrund": "ZH0",
                "process_date": "20261201",
                "lokationstyp": "Z21",
                "beteiligte_marktpartner": ["9900555000005"],
            }),
            "9900123456789",
        );
        assert!(aufhebung.contains("BGM+E02+55038"), "{aufhebung}");
        assert!(aufhebung.contains("DTM+92:"), "Beginn zum: {aufhebung}");
        assert!(aufhebung.contains("LOC+Z21+"), "Tranche: {aufhebung}");
        assert!(aufhebung.contains("NAD+VY+9900555000005"), "{aufhebung}");

        // 44036 — the Gas twin: `E44` Informationsmeldung at a Meldepunkt,
        // with the DVGW code list throughout.
        let gas = render(
            serde_json::json!({
                "pid": 44_036_u32,
                "sender": "9870123456789",
                "receiver": "9871234567897",
                "malo": "51238696012",
                "document_code": "E44",
                "transaktionsgrund": "Z26",
                "lokationstyp": "172",
                "referenz_vorgangsnummer": "VG-99",
                "beteiligte_marktpartner": ["9871234567897"],
            }),
            "9870123456789",
        );
        assert!(gas.contains("BGM+E44+44036"), "{gas}");
        assert!(gas.contains("LOC+172+51238696012"), "{gas}");
        assert!(gas.contains("NAD+VY+9871234567897::332"), "{gas}");
        assert!(!gas.contains("::293"), "{gas}");
        assert!(!gas.contains("ZW4"), "Gas defines no Ergänzung: {gas}");

        // 44037 — the Gas Beendigung additionally carries `DTM+159`
        // Bilanzierungsende, Soll „wenn eine Bilanzierung stattfindet" ([29]).
        let gas_ende = render(
            serde_json::json!({
                "pid": 44_037_u32,
                "sender": "9870123456789",
                "receiver": "9871234567897",
                "malo": "51238696012",
                "document_code": "E44",
                "transaktionsgrund": "ZC8",
                "process_date": "20261101",
                "bilanzierungsende": "20261101",
                "lokationstyp": "172",
            }),
            "9870123456789",
        );
        assert!(gas_ende.contains("DTM+93:"), "{gas_ende}");
        assert!(gas_ende.contains("DTM+159:"), "{gas_ende}");
    }

    /// **The Modell-2 legs render as their AHB columns.**
    ///
    /// UTILMD AHB Strom 2.2 Kap. 11 fixes a different `BGM` DE 1001 and a
    /// different `SG4 DTM` pair per leg, and the Bestätigung carries a second
    /// `SG5 LOC`. Built through `mako_emob` rather than by hand, so the domain
    /// crate and the renderer are checked against each other and not each
    /// against its own idea of the payload.
    #[test]
    fn the_modell_2_legs_render_as_their_ahb_columns() {
        use mako_emob::modellwechsel::{
            ABMELDUNG, ANMELDUNG, EmobAntwort, ModellwechselCommand, ModellwechselEvent,
            ModellwechselState, Modellwechseldaten, ZUORDNUNGSENDE,
        };
        use mako_engine::workflow::Workflow;

        fn daten(pid: u32) -> Box<Modellwechseldaten> {
            Box::new(Modellwechseldaten {
                malo: mako_engine::types::MaLo::new("51238696012"),
                sender: mako_engine::types::MarktpartnerCode::new("9900123456789"),
                receiver: mako_engine::types::MarktpartnerCode::new("9900987654321"),
                process_date: "20270101".to_owned(),
                pruefidentifikator: mako_engine::types::Pruefidentifikator::const_new(pid),
                vorgangsnummer: Some("LPB-0001".to_owned()),
            })
        }

        fn wire_of(outbox: &mako_engine::outbox::PendingOutbox) -> String {
            let msg = fake_msg("UTILMD", "9900987654321", outbox.payload.clone());
            let bytes = render_to_wire_bytes(&msg, &test_registry("9900123456789"))
                .expect("renders")
                .bytes;
            String::from_utf8_lossy(&bytes).into_owned()
        }

        // ── The three requests ────────────────────────────────────────────
        for (leg, bgm, dtm, zweites_dtm) in [
            (ANMELDUNG, "E01", "DTM+92:", "DTM+158:"),
            (ZUORDNUNGSENDE, "E44", "DTM+93:", "DTM+159:"),
            (ABMELDUNG, "E02", "DTM+93:", "DTM+159:"),
        ] {
            let out = send(leg, daten(leg.anfrage_pid));
            let text = wire_of(&out);
            assert!(
                text.contains(&format!("BGM+{bgm}+{}", leg.anfrage_pid)),
                "{} wants BGM+{bgm}:\n{text}",
                leg.anfrage_pid
            );
            assert!(
                text.contains(dtm),
                "{} wants {dtm}:\n{text}",
                leg.anfrage_pid
            );
            assert!(
                text.contains(zweites_dtm),
                "{} wants {zweites_dtm} (AHB Bedingung [317]):\n{text}",
                leg.anfrage_pid
            );
        }

        // ── The Bestätigung names the ZP der NGZ beside the MaLo ──────────
        let received = ModellwechselEvent::AnfrageErhalten {
            data: daten(55_238),
            message_ref: mako_engine::types::MessageRef::new("MSG1"),
        };
        let state = <mako_emob::EmobAnmeldungWorkflow as Workflow>::apply(
            ModellwechselState::New,
            &received,
        );
        let answered = <mako_emob::EmobAnmeldungWorkflow as Workflow>::handle(
            &state,
            ModellwechselCommand::SendAntwort {
                antwort: Box::new(
                    EmobAntwort::zustimmung("A02", "E_0510")
                        .mit_zp_ngz("DE0001234567890000000000000000123"),
                ),
            },
        )
        .expect("answered");
        let text = wire_of(&answered.outbox[0]);
        assert!(text.contains("BGM+E01+55239"), "{text}");
        assert!(text.contains("LOC+Z16+51238696012"), "{text}");
        assert!(
            text.contains("LOC+Z15+DE0001234567890000000000000000123"),
            "AHB Bedingung [663] — the 55239 names the ZP der NGZ:\n{text}"
        );
        // `A01` refuses in `E_0510` and agrees in `E_0511`, so DE 1131 is the
        // only thing on the wire that says which tree answered.
        assert!(text.contains("STS+E01++A02:E_0510"), "{text}");
        assert!(text.contains("RFF+TN:LPB-0001"), "{text}");
    }

    /// Drive one leg's `Senden` command without naming its workflow type.
    fn send(
        leg: mako_emob::modellwechsel::LegWire,
        data: Box<mako_emob::modellwechsel::Modellwechseldaten>,
    ) -> mako_engine::outbox::PendingOutbox {
        use mako_emob::modellwechsel::ModellwechselCommand;
        use mako_engine::workflow::Workflow;
        let cmd = ModellwechselCommand::Senden { data };
        let out = match leg.anfrage_pid {
            55_238 => <mako_emob::EmobAnmeldungWorkflow as Workflow>::handle(
                &mako_emob::modellwechsel::ModellwechselState::New,
                cmd,
            ),
            55_240 => <mako_emob::EmobZuordnungsendeWorkflow as Workflow>::handle(
                &mako_emob::modellwechsel::ModellwechselState::New,
                cmd,
            ),
            _ => <mako_emob::EmobAbmeldungWorkflow as Workflow>::handle(
                &mako_emob::modellwechsel::ModellwechselState::New,
                cmd,
            ),
        }
        .expect("sent");
        out.outbox.into_iter().next().expect("one message")
    }

    /// **The payload key for DE 1131 has exactly one spelling.**
    ///
    /// `mako-gpke` and `mako-geli-gas` build their answers through
    /// [`mako_gpke::lf_antwort::antwort_outbox`]; `mako-wim` builds its own
    /// JSON. Both must land in `SG4 STS+E01` DE 1131, and the only thing that
    /// makes them do so is agreeing on the field name — the renderer reads one
    /// key and silently omits the Codeliste when it is spelled the other way.
    /// Where one answer PID carries several trees (55239 carries `E_0510`
    /// *and* `E_0513`, 55003 carries `E_0622` and `E_0623`) DE 1131 is the
    /// only thing on the wire that says which one answered.
    #[test]
    fn a_domain_built_answer_carries_its_codeliste_to_the_wire() {
        let outbox = mako_gpke::lf_antwort::antwort_outbox(
            55_009,
            &mako_gpke::lf_antwort::LfAntwort::ablehnung("A35", "E_0624"),
            &mako_engine::types::MaLo::new("51238696012"),
            &mako_engine::types::MarktpartnerCode::new("9900987654321"),
            &mako_engine::types::MarktpartnerCode::new("9900123456789"),
            "20261101",
            Some("NNV1234"),
        );
        let msg = fake_msg("UTILMD", "9900987654321", outbox.payload.clone());
        let wire = render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("renders");
        let text = String::from_utf8_lossy(&wire.bytes);
        assert!(
            text.contains("STS+E01++A35:E_0624"),
            "DE 1131 was dropped between the domain crate and the wire:\n{text}"
        );
    }

    /// `SG4 STS+Z35` — the LFA's own Ablehnungsgrund, restated by the NB.
    ///
    /// GPKE Teil 2 § 2.1.2 Nr. 6: „Der NB gibt zusätzlich den Grund der
    /// Ablehnung des LFA an, sofern dieser in Prozessschritt 4 die Anfrage
    /// abgelehnt hat." Without it the LFN learns that its Anmeldung failed and
    /// not why the incumbent would not release the Marktlokation — the one fact
    /// it can act on.
    #[test]
    fn an_a50_ablehnung_restates_the_lfas_own_ground() {
        let msg = fake_msg(
            "UTILMD",
            "9900987654321",
            serde_json::json!({
                "pid": 55_003_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "process_date": "20261101",
                "antwort_code": "A50",
                "antwort_codeliste": "E_0623",
                "dritter_antwortcode": "A35",
            }),
        );
        let wire = render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("renders");
        let text = String::from_utf8_lossy(&wire.bytes);
        assert!(text.contains("STS+E01++A50:E_0623"), "{text}");
        // The verbrauchende form: code and Codeliste only — a 55003's AHB
        // column carries neither a Lokations-Referenz nor an object type.
        assert!(text.contains("STS+Z35++A35:E_0624"), "{text}");
    }

    /// The erzeugende form names *which* object the restated answer is about,
    /// because Geschäftsvorfall 3 splits a Marktlokation across Tranchen and
    /// several LFA answer separately.
    #[test]
    fn an_a57_ablehnung_names_the_object_the_lfa_answered_for() {
        let msg = fake_msg(
            "UTILMD",
            "9900987654321",
            serde_json::json!({
                "pid": 55_080_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "process_date": "20261101",
                "antwort_code": "A57",
                "antwort_codeliste": "E_0623",
                "dritter_antwortcode": "A39",
                "dritter_referenz_lokation": "51238696012",
                "dritter_objekt": "ZW5",
            }),
        );
        let wire = render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("renders");
        let text = String::from_utf8_lossy(&wire.bytes);
        assert!(
            text.contains("STS+Z35+51238696012+A39:E_0624+ZW5"),
            "{text}"
        );
    }

    /// The obligation is enforced, not documented: an `A50` without the third
    /// party's ground is refused at render time rather than sent thinner than
    /// the AHB requires.
    #[test]
    fn an_a50_without_the_third_partys_ground_is_refused() {
        let msg = fake_msg(
            "UTILMD",
            "9900987654321",
            serde_json::json!({
                "pid": 55_003_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "process_date": "20261101",
                "antwort_code": "A50",
                "antwort_codeliste": "E_0623",
            }),
        );
        let err = render_to_wire_bytes(&msg, &test_registry("9900123456789"))
            .expect_err("A50 requires SG4 STS+Z35");
        assert!(format!("{err}").contains("dritter_antwortcode"), "{err}");
    }

    /// An ordinary Ablehnung is unaffected — the segment is Muss on exactly two
    /// codes, and requiring it elsewhere would refuse every other refusal.
    #[test]
    fn an_ordinary_ablehnung_needs_no_third_party() {
        let msg = fake_msg(
            "UTILMD",
            "9900987654321",
            serde_json::json!({
                "pid": 55_003_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "process_date": "20261101",
                "antwort_code": "A07",
                "antwort_codeliste": "E_0622",
            }),
        );
        let wire = render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("renders");
        assert!(!String::from_utf8_lossy(&wire.bytes).contains("STS+Z35"));
    }

    /// Strom keeps its own three, so the Gas fix is not a global one.
    #[test]
    fn a_strom_utilmd_keeps_the_strom_code_lists() {
        let msg = fake_msg(
            "UTILMD",
            "9900987654321",
            serde_json::json!({
                "pid": 55001_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "process_date": "20261101",
                "transaktionsgrund": "E03",
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("55001 renders");
        let text = String::from_utf8_lossy(&wire.bytes);
        assert!(text.contains("NAD+MS+9900123456789::293"), "{text}");
        assert!(text.contains("LOC+Z16+51238696012"), "{text}");
        assert!(text.contains("STS+7++E03+ZW4"), "{text}");
    }

    /// The LF's own Anmeldung needs the same package — that is what tells the NB
    /// which balancing circle to book the Marktlokation into.
    #[test]
    fn the_55001_anmeldung_carries_its_bilanzkreis() {
        let msg = fake_msg(
            "UTILMD",
            "9900987654321",
            serde_json::json!({
                "pid": 55001_u32,
                "sender": "9900123456789",
                "receiver": "9900987654321",
                "malo": "51238696012",
                "process_date": "20261101",
                "transaktionsgrund": "E03",
                "bilanzkreis": "11XBK-STD-----9",
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900123456789")).expect("55001 renders");
        let text = String::from_utf8_lossy(&wire.bytes);
        assert!(text.contains("CAV+ZV4:::11XBK-STD-----9"), "{text}");
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
                "mabis_zp_id": "DE0004030099000000000000000012345",
                "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--P",
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
        // ORDERS AHB 1.1b §4.15. The Bestellung references the QUOTES Angebot
        // in `RFF+AAG` (`ZG-T24`), the Abbestellung the ORDERS in `RFF+ACW`
        // (`ZG-T41`); both carry `BGM+Z57`, `DTM+203` and `IMD++<7081>`, and
        // **neither carries a LOC** — §4.15 lists no Meldepunkt segment.
        for (pid, qualifier, abo) in [(17007_u32, "AAG", "Z01"), (17008_u32, "ACW", "Z02")] {
            let msg = fake_msg(
                "ORDERS",
                "9900357000004",
                serde_json::json!({
                    "pid": pid,
                    "sender_mp_id": "9900555000005",
                    "receiver_mp_id": "9900357000004",
                    // A payload that still names a location must not put one on
                    // the wire for these PIDs.
                    "location": "51238696781",
                    "korrelation_ref": "ESA-REF-1",
                    "abonnement": abo,
                    "ausfuehrungsdatum": "2026-03-01",
                }),
            );
            let wire = render_to_wire_bytes(&msg, &test_registry("9900555000005"))
                .unwrap_or_else(|e| panic!("{pid} renders: {e:?}"));
            let wire = String::from_utf8(wire.bytes).expect("utf-8");
            assert!(wire.contains("BGM+Z57+"), "BGM 1001 = Z57: {wire}");
            assert!(
                wire.contains(&format!("RFF+{qualifier}:ESA-REF-1")),
                "PID {pid} correlates by RFF+{qualifier}: {wire}"
            );
            assert!(
                wire.contains(&format!("RFF+Z13:{pid}")),
                "Prüfidentifikator: {wire}"
            );
            assert!(wire.contains(&format!("IMD++{abo}")), "IMD 7081: {wire}");
            assert!(wire.contains("DTM+203:202603010000"), "DTM+203: {wire}");
            assert!(
                !wire.contains("LOC+"),
                "ORDERS {pid} carries no LOC (ORDERS AHB 1.1b §4.15): {wire}"
            );
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
        // ORDRSP AHB 1.1b §4.15. `SG2 AJT` is Muss on all four and carries the
        // Prüfschritt code with the EBD that publishes it; the correlation
        // qualifier differs per PID (`ON` → the ORDERS, `ACW` → the ORDCHG).
        for (pid, qualifier, ebd, code) in [
            (19011_u32, "ON", "E_0256", "A11"),
            (19012_u32, "ON", "E_0256", "A09"),
            (19013_u32, "ACW", "E_0257", "A04"),
            (19014_u32, "ACW", "E_0257", "A02"),
        ] {
            let msg = fake_msg(
                "ORDRSP",
                "9900555000005",
                serde_json::json!({
                    "pid": pid,
                    "sender": "9900357000004",
                    "receiver": "9900555000005",
                    "korrelation_ref": "ESA-BE-0001",
                    "abonnement": "Z01",
                    "antwort_codeliste": ebd,
                    "antwort_code": code,
                }),
            );
            let wire = render_to_wire_bytes(&msg, &test_registry("9900357000004"))
                .unwrap_or_else(|e| panic!("{pid} renders: {e:?}"));
            let wire = String::from_utf8(wire.bytes).expect("utf-8");
            assert!(!wire.contains("LOC+"), "ORDRSP carries no LOC: {wire}");
            assert!(wire.contains("BGM+Z57+"), "BGM 1001 = Z57: {wire}");
            assert!(
                wire.contains(&format!("RFF+{qualifier}:ESA-BE-0001")),
                "PID {pid} correlates by RFF+{qualifier}: {wire}"
            );
            assert!(
                wire.contains(&format!("AJT+{code}+{ebd}")),
                "SG2 AJT carries the Prüfschritt code and its EBD: {wire}"
            );
            edi_energy::EdiEnergyMessage::validate(
                &edi_energy::parse(wire.as_bytes()).expect("parse"),
            )
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("ORDRSP {pid} must be MIG-conformant: {e}\n{wire}"));
        }
    }

    /// `SG2 AJT` is Muss and its code must come from the answer's own EBD, so
    /// the renderer refuses rather than synthesising one.
    #[test]
    fn an_esa_ordrsp_without_an_antwortcode_is_refused() {
        let msg = fake_msg(
            "ORDRSP",
            "9900555000005",
            serde_json::json!({
                "pid": 19011_u32,
                "sender": "9900357000004",
                "receiver": "9900555000005",
                "korrelation_ref": "ESA-BE-0001",
            }),
        );
        let err = render_to_wire_bytes(&msg, &test_registry("9900357000004")).unwrap_err();
        assert!(format!("{err:?}").contains("antwort_code"), "{err:?}");
    }

    #[test]
    fn render_iftsta_21042_beendigung_is_mig_conformant() {
        // UC 4.4 „Beendigung durch MSB": the MSB → ESA IFTSTA 21042 now renders
        // to EDIFACT instead of degrading to a JSON render-intent. It must carry
        // the SG15 RFF+Z13 Prüfidentifikator, the STS Z21/105 „beendet" status,
        // the SG14 CNI Vorgangsnummer and the RFF+AGI back-reference.
        let msg = fake_msg(
            "IFTSTA",
            "9900555000005",
            serde_json::json!({
                "pid": 21042_u32,
                "sender": "4012345000023",
                "receiver": "9900555000005",
                "sts_code": "105",
                "korrelation_ref": "BEST-4711",
                "beendigung_zum": "2026-08-01T00:00:00Z",
            }),
        );
        let wire = render_to_wire_bytes(&msg, &test_registry("4012345000023"))
            .unwrap_or_else(|e| panic!("21042 renders: {e:?}"));
        let wire = String::from_utf8(wire.bytes).expect("utf-8");
        assert!(
            wire.contains("RFF+Z13:21042"),
            "PID in SG15 RFF+Z13: {wire}"
        );
        assert!(
            wire.contains("STS+Z21+:105"),
            "STS Z21/105 „beendet\": {wire}"
        );
        assert!(wire.contains("CNI+1"), "Vorgangsnummer SG14 CNI: {wire}");
        assert!(
            wire.contains("RFF+AGI:BEST-4711"),
            "order ref SG15 RFF+AGI: {wire}"
        );
        assert!(
            wire.contains("DTM+93:20260801"),
            "Vertragsende SG15 DTM+93 (date part only): {wire}"
        );
        edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("IFTSTA 21042 must be MIG-conformant: {e}\n{wire}"));
    }

    #[test]
    fn every_esa_step_round_trips_through_its_published_zuordnungsschluessel() {
        // What the renderer emits must be exactly what the ingest dispatcher
        // looks for — one table, read from both ends. The qualifiers differ per
        // PID and are not interchangeable: `RFF+ON` on a 19011 and `RFF+ACW` on
        // a 19013 point at different messages.
        for (msg_type, pid, payload_extra) in [
            (
                "ORDRSP",
                19011_u32,
                serde_json::json!({"antwort_codeliste": "E_0256", "antwort_code": "A11"}),
            ),
            (
                "ORDRSP",
                19013_u32,
                serde_json::json!({"antwort_codeliste": "E_0257", "antwort_code": "A04"}),
            ),
            ("ORDCHG", 39002_u32, serde_json::json!({})),
            (
                "ORDERS",
                17007_u32,
                serde_json::json!({"abonnement": "Z01"}),
            ),
            (
                "ORDERS",
                17008_u32,
                serde_json::json!({"abonnement": "Z02"}),
            ),
        ] {
            let mut payload = serde_json::json!({
                "pid": pid,
                "sender": "9900357000004",
                "receiver": "9900555000005",
                "korrelation_ref": "ORD-ABC-1",
            });
            if let Some(extra) = payload_extra.as_object() {
                for (k, v) in extra {
                    payload[k] = v.clone();
                }
            }
            let out = fake_msg(msg_type, "9900555000005", payload);
            let wire = render_to_wire_bytes(&out, &test_registry("9900357000004"))
                .unwrap_or_else(|e| panic!("{msg_type} {pid} renders: {e:?}"));
            let parsed = edi_energy::parse(&wire.bytes).expect("parse");
            assert_eq!(
                crate::ingest_dispatcher::esa_korrelation_key(&parsed, pid),
                "ORD-ABC-1",
                "{msg_type} {pid} must round-trip its Zuordnungsschlüssel"
            );
        }
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
    fn render_reqote_35003_is_mig_conformant() {
        let msg = fake_msg(
            "REQOTE",
            "9900357000004",
            serde_json::json!({
                "pid": 35003_u32,
                "sender": "9900555000005",
                "receiver": "9900357000004",
                "location": "51238696781",
                // MaLo, Wirkarbeit, Lastgang ¼h — Pflicht since 06.08.2024.
                "messprodukt": "9991000003056",
                "wunschtermin": "2026-03-01",
            }),
        );
        let wire =
            render_to_wire_bytes(&msg, &test_registry("9900555000005")).expect("35003 renders");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");
        assert!(wire.contains("BGM+Z57+"), "BGM 1001 = Z57: {wire}");
        assert!(wire.contains("DTM+76:202603010000"), "Wunschtermin: {wire}");
        assert!(wire.contains("NAD+DP"), "Liefer-/Bezugsort: {wire}");
        assert!(wire.contains("LOC+172+51238696781"), "Meldepunkt: {wire}");
        // `LIN+1+Z67` then `PIA+5+<code>:Z11` — 7143 is C212's *second*
        // component, so the code and its type are adjacent.
        assert!(wire.contains("LIN+1+Z67"), "Backend-Produktzeile: {wire}");
        assert!(
            wire.contains("PIA+5+9991000003056:Z11"),
            "Messprodukt-Code: {wire}"
        );
        assert!(
            !wire.contains("FTX+ACB++"),
            "no empty free text when no note was given: {wire}"
        );
        edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("35003 Werteanfrage must be MIG-conformant: {e}\n{wire}"));
    }

    /// `PIA+5 DE7140` accepts only Kapitel-4.6 codes (REQOTE AHB 1.2 §4.3
    /// condition `[41]`), so a Typ-1 Standard-Messprodukt is not orderable here.
    #[test]
    fn a_reqote_35003_without_a_catalogue_messprodukt_is_refused() {
        for produkt in [None, Some("9992000000011")] {
            let mut payload = serde_json::json!({
                "pid": 35003_u32,
                "sender": "9900555000005",
                "receiver": "9900357000004",
                "location": "51238696781",
                "wunschtermin": "2026-03-01",
            });
            if let Some(p) = produkt {
                payload["messprodukt"] = serde_json::Value::String(p.to_owned());
            }
            let msg = fake_msg("REQOTE", "9900357000004", payload);
            let err = render_to_wire_bytes(&msg, &test_registry("9900555000005")).unwrap_err();
            assert!(format!("{err:?}").contains("messprodukt"), "{err:?}");
        }
    }

    #[test]
    fn render_quotes_angebot_carries_the_bindungsfrist_ablehnung_carries_the_reason() {
        let esa = "9900555000005";
        // Angebot: Bindungsfrist as `DTM+273`, no FTX.
        let angebot = fake_msg(
            "QUOTES",
            esa,
            serde_json::json!({
                "pid": 15003_u32,
                "sender": "9900357000004",
                "receiver": esa,
                "location": "51238696781",
                "korrelation_ref": "ESA-WA-1",
                "bindungsfrist_tage": 14,
                "fruehester_start": "2026-03-01",
                "messprodukt": "9991000003056",
                // Condition [2042]/[2071]: one Artikel-ID per priced position,
                // and one `PRI+CAL` per Artikel-ID. The last two digits of the
                // ID select the price kind (01 Einrichtung, 03 Betrieb).
                "artikel_ids": ["9991000012301", "9991000012303"],
                "preise": [
                    { "betrag": "12.50", "art": "Z01", "einheit": "H87" },
                    { "betrag": "0.35",  "art": "Z03", "einheit": "DAY" }
                ],
            }),
        );
        let wire = render_to_wire_bytes(&angebot, &test_registry("9900357000004"))
            .expect("angebot renders");
        let wire = String::from_utf8(wire.bytes).expect("utf-8");
        // `DTM+273` is a duration: DE 2380 a count, DE 2379 the unit
        // (802 Monat / 803 Woche / 804 Tag) — not a `CCYYMMDD` date.
        assert!(wire.contains("DTM+273:14:804"), "{wire}");
        assert!(wire.contains("DTM+469:202603010000"), "{wire}");
        assert!(wire.contains("RFF+AAV:ESA-WA-1"), "ZG-T16: {wire}");
        assert!(wire.contains("BGM+Z57+"), "{wire}");
        assert!(wire.contains("NAD+DP"), "{wire}");
        assert!(
            !wire.contains("FTX+"),
            "an Angebot carries no reason: {wire}"
        );
        // Full MIG + AHB conformance: the 15003 Angebot validates clean.
        edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
            .expect("validate")
            .into_error_result()
            .unwrap_or_else(|e| panic!("15003 Angebot must be MIG-conformant: {e}\n{wire}"));

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

        // Angebot — a **priced** position block. UC 4.1.1 has the ESA asking
        // for „die Übermittlung von Werten und die damit verbundenen Kosten",
        // and QUOTES AHB 1.1a §4.3 makes `SG4 CUX`, the `PIA+Z02` Artikel-IDs,
        // the `SG31 PRI+CAL` prices and the `PIA+5 …:SRW` OBIS-Kennzahlen all
        // Muss. That, not the Bindungsfrist, is what tells an offer from a
        // refusal — `DTM+273` is Muss on the only published 15003 use case, so
        // a refusal carries one too and reading its absence as the signal made
        // every conformant Angebot parse as an Ablehnung.
        let angebot = render(serde_json::json!({
            "pid": 15003_u32, "sender": "9900357000004", "receiver": esa,
            "location": "51238696781", "bindungsfrist_tage": 14,
            "korrelation_ref": "ESA-WA-1",
            "messprodukt": "9991000003056",
            "currency": "EUR",
            "artikel_ids": ["9990001100002"],
            "obis": ["1-1:1.29.0"],
            "preise": [{ "betrag": "0.004500", "art": "Z03", "einheit": "DAY" }],
        }));
        let msg = edi_energy::parse(&angebot).expect("parse angebot");
        let cmd = registry
            .dispatch(&msg as &dyn std::any::Any, &fv)
            .expect("dispatch angebot");
        let C::ReceiveAngebot { angebot, .. } = cmd else {
            panic!("a priced 15003 must map to ReceiveAngebot")
        };
        assert_eq!(angebot.waehrung.as_deref(), Some("EUR"));
        assert_eq!(angebot.obis_kennzahlen, ["1-1:1.29.0"]);
        let preis = angebot
            .preis(mako_wim::esa::Preistyp::Betrieb)
            .expect("the Betriebspreis survives the round trip");
        assert_eq!(preis.betrag, "0.004500");
        assert_eq!(preis.einheit, "DAY");
        assert_eq!(preis.artikel_id, "9990001100002");

        // Ablehnung — no priced position, grounds in `FTX+ACB`.
        let ablehnung = render(serde_json::json!({
            "pid": 15003_u32, "sender": "9900357000004", "receiver": esa,
            "location": "51238696781", "reason": "Messprodukt nicht lieferbar",
        }));
        let msg = edi_energy::parse(&ablehnung).expect("parse ablehnung");
        let cmd = registry
            .dispatch(&msg as &dyn std::any::Any, &fv)
            .expect("dispatch ablehnung");
        match cmd {
            C::ReceiveAnfrageAblehnung { reason, .. } => {
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
                "korrelation_ref": "ORDERDOC0001",
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
        // PID 13027 travels in RFF+Z13; DE 1004 is a Dokumentennummer.
        assert!(wire.contains("RFF+Z13:13027"), "{wire}");
        // MSCONS AHB 3.2 §11.2: BGM 1001 = Z83 „Werte nach Typ 2", UNB DE 0026
        // = TL, DTM+137 in format 303, and RFF+AGI naming the ORDERS that
        // ordered the values (hint [574]).
        assert!(wire.contains("BGM+Z83+"), "BGM 1001 = Z83: {wire}");
        assert!(
            !wire.contains("BGM+7+13027"),
            "the PID is not a document code: {wire}"
        );
        assert!(
            wire.contains("+TL'"),
            "UNB application reference TL: {wire}"
        );
        assert!(wire.contains(":303'"), "DTM+137 format 303: {wire}");
        assert!(wire.contains("RFF+AGI:ORDERDOC0001"), "{wire}");
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
                    "mabis_zp_id": "DE0004030099000000000000000012345",
                "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--P",
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
                "mabis_zp_id": "DE0004030099000000000000000012345",
                "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--P",
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
                "mabis_zp_id": "DE0004030099000000000000000012345",
                "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--P",
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
                "mabis_zp_id": "DE0004030099000000000000000012345",
                "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--P",
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

    // ── ESA outbound leg — the MSB's answers on the wire ──────────────────────

    /// The rendered QUOTES/ORDRSP must parse back, carry the ESA
    /// Prüfidentifikator (BGM DE 1004) — otherwise the answer cannot close the
    /// 5-WT / 2-WT windows the inbound leg armed — and echo the correlation
    /// reference under the qualifier its Zuordnungsschlüssel names.
    fn assert_esa_roundtrip(message_type: &str, pid: u32) {
        use edi_energy::EdiEnergyMessage as _;
        let mut payload = serde_json::json!({
            "sender": "9900123456789",
            "receiver": "9900987654321",
            "pid": pid,
            "korrelation_ref": "ANFRAGE-REF-1",
            "abonnement": "Z01",
            "bindungsfrist_tage": 14,
            "messprodukt": "9991000003056",
        });
        if message_type == "ORDRSP" {
            let (ebd, code) = if pid >= 19013 {
                ("E_0257", "A04")
            } else {
                ("E_0256", "A11")
            };
            payload["antwort_codeliste"] = serde_json::Value::String(ebd.to_owned());
            payload["antwort_code"] = serde_json::Value::String(code.to_owned());
        }
        let msg = outbox_msg(message_type, payload);
        let rendered = render_to_wire_bytes(&msg, &test_registry("9900123456789"))
            .unwrap_or_else(|e| panic!("render {message_type} {pid} must succeed: {e}"));
        let parsed = edi_energy::Platform::with_all_profiles()
            .parse(&rendered.bytes)
            .unwrap_or_else(|e| panic!("{message_type} {pid} must parse back: {e}"));
        let detected = parsed
            .detect_pruefidentifikator()
            .unwrap_or_else(|e| panic!("{message_type} {pid}: PID must be detectable: {e}"));
        assert_eq!(detected.as_u32(), pid, "round-trip PID for {message_type}");
        assert_eq!(
            crate::ingest_dispatcher::esa_korrelation_key(&parsed, pid),
            "ANFRAGE-REF-1",
            "{message_type} {pid} must round-trip its Zuordnungsschlüssel"
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
