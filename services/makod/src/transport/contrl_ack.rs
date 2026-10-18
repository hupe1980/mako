//! CONTRL Empfangsbestätigung service — CONTRL AHB 1.0 §2.3 / APERAK AHB 1.1 §2.3.
//!
//! ## Regulatory obligation
//!
//! > „In der Sparte Gas hat der Empfänger auf jede eingehende Übertragungsdatei
//! > immer eine CONTRL (entweder in der Ausprägung Empfangsbestätigung
//! > (UCI DE0083 = 7) oder Syntaxfehlermeldung (UCI DE0083 = 4)) zu versenden,
//! > außer als Reaktion auf eine CONTRL."
//! >
//! > — CONTRL AHB 1.0 §2.3.1
//!
//! > „Auf eine APERAK ist immer eine CONTRL zu senden."
//! >
//! > — APERAK AHB 1.1 §2.3 (Gas rules)
//!
//! For every inbound **Gas** interchange (UNB…UNZ) **or Gas APERAK**, makod MUST
//! send a CONTRL Empfangsbestätigung (UCI DE0083 = 7) back to the sender.
//! Only CONTRL-on-CONTRL is forbidden (§2.2.2.2).
//!
//! For **Strom** interchanges no Empfangsbestätigung is required: §2.4 uses the
//! CONTRL there for nothing but the Syntaxfehlermeldung.
//!
//! ## The two Ausprägungen
//!
//! [`Verdict`] is the pair `UCI` DE 0083 admits — `7` „Übertragung bestätigt"
//! and `4` „Diese Ebene und alle tieferen Ebenen zurückgewiesen". A rejection
//! names its fault either on the `UCI` (DE 0085, a [`SyntaxFehler`]) or, when
//! the fault sits inside messages, on an `SG1 UCM` per broken message with its
//! `SG2 UCS` / `UCD` (CONTRL AHB 1.0 Kap. 3, „Syntaxfehlermeldung in der
//! Nachricht"; [`syntaxpruefung`] decides which). Either way the whole
//! Übertragungsdatei is rejected (Kap. 1.3). The Sparte decides whether a
//! *clean* interchange is acknowledged; it never decides whether a broken one
//! is reported, which both Sparten owe.
//!
//! ## The window is not one number
//!
//! Six wall-clock hours is the Regelfall (§2.3.1, §2.4.1). §2.4.1 shortens a
//! Strom UTILMD or ORDERS to **15 minutes** — six hours again when it arrived on
//! a Saturday — and §2.3.1 shortens a GABi-Gas ALOCAT to **45 minutes**. The
//! deadline registered beside the outbox entry carries whichever applies; see
//! [`mako_fristen::ContrlAnlass`].
//!
//! ## Architecture
//!
//! [`ContrlAckService`] is wired into both ingest paths:
//! - REST `POST /edifact` — via [`crate::edifact_api::EdifactApiState`]
//! - AS4 inbound — via [`crate::as4_ingest::BdewAs4IngestHandler`]
//!
//! Call [`ContrlAckService::emit_for_interchange`] once per successfully-parsed
//! interchange, passing all messages contained in the UNB…UNZ **and the
//! recipient MP-ID** (UNB DE0010 — the own party the interchange is addressed to).
//! The recipient MP-ID resolves the interchange's Sparte via the own-party
//! registry (each `[[party]]` is exactly one Sparte, BDEW §2.13); only Gas
//! interchanges get a CONTRL. The service enqueues a single [`OutboxMessage`] of
//! type `"CONTRL"` which the [`OutboxWorker`] renders via
//! `edifact_renderer::render_contrl` and delivers to the counterparty's AS4 endpoint.
//!
//! An enqueue failure is logged at `error` level and returned to the caller,
//! which dead-letters it (§ 147 AO): the 6h window is a regulatory obligation
//! and nothing else retries the CONTRL. The HTTP / AS4 response itself is
//! unaffected — the message was received either way.
//!
//! [`OutboxWorker`]: mako_engine::builder::OutboxWorker

use std::sync::Arc;

use edi_energy::{AnyMessage, EdiEnergyMessage as _};
use edifact_rs::{ContrlLocation, RecoveredInterchange, RecoveryConfig};
use mako_engine::{
    deadline::Deadline,
    error::EngineError,
    ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId},
    outbox::OutboxMessage,
    store_slatedb::SlateDbStore,
    version::WorkflowId,
};

use crate::party_registry::{MpIdRegistry, RoleSparte};

// ── Verdict ──────────────────────────────────────────────────────────────────

/// What the CONTRL states about the interchange it answers.
///
/// CONTRL AHB 1.0 Kap. 2 gives `UCI` DE 0083 exactly two values in this market:
/// `7` „Übertragung bestätigt" and `4` „Diese Ebene und alle tieferen Ebenen
/// zurückgewiesen". The second carries a DE 0085 Syntaxfehler code, the first
/// carries none — an Empfangsbestätigung reports no error, and an empty trailing
/// element is not the same as an absent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Empfangsbestätigung — `UCI` DE 0083 = `7`. Gas only: in Strom the CONTRL
    /// is used **exclusively** as a Syntaxfehlermeldung (AHB §2.4).
    Empfangsbestaetigung,
    /// Syntaxfehlermeldung — `UCI` DE 0083 = `4` plus the DE 0085 code, and with
    /// it the statement that the Übertragungsdatei is not processed further.
    Syntaxfehler(SyntaxFehler),
    /// Syntaxfehlermeldung **in der Nachricht** — `UCI` DE 0083 = `4` with no
    /// DE 0085, the faults named on the `SG1 UCM` / `SG2` beneath it (CONTRL
    /// AHB 1.0 Kap. 3). The file is rejected just the same.
    SyntaxfehlerInNachrichten,
}

impl Verdict {
    const fn accepted(self) -> bool {
        matches!(self, Self::Empfangsbestaetigung)
    }

    const fn syntax_error(self) -> Option<&'static str> {
        match self {
            Self::Empfangsbestaetigung | Self::SyntaxfehlerInNachrichten => None,
            Self::Syntaxfehler(code) => Some(code.de0085()),
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Empfangsbestaetigung => "Empfangsbestätigung",
            Self::Syntaxfehler(_) => "Syntaxfehlermeldung",
            Self::SyntaxfehlerInNachrichten => "Syntaxfehlermeldung in der Nachricht",
        }
    }
}

/// The `UCI` DE 0085 codes the AHB admits at interchange level.
///
/// CONTRL AHB 1.0 Kap. 3 lists thirteen; these are the ones mako can decide from
/// what it knows about a rejected Übertragungsdatei. Each is also an
/// [`edifact_rs::contrl::SyntaxError`], which is where the code value comes
/// from — ISO 9735-4 Annex A fixes it, and reading it off the upstream enum
/// keeps a literal out of this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxFehler {
    /// `12` „Ungültiger Wert" — the catch-all for a file that did not parse.
    UngueltigerWert,
    /// `25` „Test-Kennzeichen nicht unterstützt" — `UNB` DE 0035 = `1` on a
    /// production endpoint (Allgemeine Festlegungen §3).
    TestKennzeichen,
    /// `7` „Empfänger der Übertragungsdatei ist nicht der tatsächliche
    /// Empfänger" — `UNB` DE 0010 names a party this deployment does not hold.
    FalscherEmpfaenger,
}

impl SyntaxFehler {
    /// The DE 0085 code, read off the ISO 9735-4 Annex A enum.
    const fn de0085(self) -> &'static str {
        use edifact_rs::contrl::SyntaxError as E;
        match self {
            Self::UngueltigerWert => E::InvalidValue.code(),
            Self::TestKennzeichen => E::TestIndicatorNotSupported.code(),
            Self::FalscherEmpfaenger => E::NotActualRecipient.code(),
        }
    }

    /// The Syntaxfehler a failed parse reports.
    ///
    /// [`edifact_rs::contrl::SyntaxError::for_error`] maps the parser's own
    /// error to the Annex-A code; anything it resolves to a code the AHB does
    /// not admit at interchange level falls back to `12` „Ungültiger Wert",
    /// which Kap. 3 does admit and which the AHB itself treats as the catch-all.
    #[must_use]
    pub fn from_parse_error(error: &edi_energy::Error) -> Self {
        let edi_energy::Error::Parse(inner) = error else {
            return Self::UngueltigerWert;
        };
        use edifact_rs::contrl::SyntaxError as E;
        match E::for_error(inner) {
            E::TestIndicatorNotSupported => Self::TestKennzeichen,
            E::NotActualRecipient => Self::FalscherEmpfaenger,
            _ => Self::UngueltigerWert,
        }
    }
}

// ── Syntaxprüfung of one Übertragungsdatei ───────────────────────────────────

/// The outcome of the syntax check of one Übertragungsdatei, as the CONTRL
/// reports it.
///
/// CONTRL AHB 1.0 (Stand 11.12.2025) Kap. 1.3: „In der CONTRL wird entweder
/// eine Übertragungsdatei bestätigt oder die gesamte Übertragungsdatei
/// zurückgewiesen." A fault in any one message therefore rejects the whole
/// file — Kap. 1.2: „bei Syntaxfehlern \[werden\] alle in der Übertragungsdatei
/// enthaltenen Geschäftsvorfälle vom Empfänger nicht verarbeitet" — and what
/// differs between the two rejecting variants is only where the CONTRL places
/// the fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Syntaxpruefung {
    /// Every message parsed.
    Fehlerfrei,
    /// Faults located inside messages: the Kap. 3 column „Syntaxfehlermeldung
    /// in der Nachricht" — `UCI` DE 0083 = `4`, one `SG1 UCM` per broken message.
    Nachrichten(Vec<NachrichtenFehler>),
    /// A fault that cannot be placed on a message: the column
    /// „Syntaxfehlermeldung in der Übertragungsdatei", `UCI` DE 0083 = `4` with
    /// its DE 0085 code.
    Uebertragungsdatei(SyntaxFehler),
}

/// One broken message of an Übertragungsdatei — its `SG1 UCM` and what goes
/// beneath it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NachrichtenFehler {
    /// `UCM` DE 0062 — the message's `UNH` reference.
    pub message_ref: String,
    /// `UCM` S009 DE 0065.
    pub message_type: String,
    /// `UCM` S009 DE 0052.
    pub version: String,
    /// `UCM` S009 DE 0054.
    pub release: String,
    /// `UCM` S009 DE 0051.
    pub agency: String,
    /// `UCM` S009 DE 0057, the BDEW Versionsnummer.
    pub association: Option<String>,
    /// Where in the message the fault is.
    pub fehler: Fehlerort,
}

/// Where a fault inside a message sits (CONTRL AHB 1.0 Kap. 3, MIG 2.0b).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "ebene", rename_all = "snake_case")]
pub enum Fehlerort {
    /// A fault in `UNH` or `UNT`: `UCM` DE 0085 with the service segment in
    /// DE 0013, and no `SG2`.
    Nachricht {
        /// `UCM` DE 0085.
        code: String,
        /// `UCM` DE 0013 — `UNH` or `UNT`.
        service_segment: String,
    },
    /// A fault in a segment between them: `SG2 UCS`, and `UCD` when the data
    /// element is known.
    Segment {
        /// `UCS` DE 0096 — the segment's position, `UNH` counting as 1.
        position: u32,
        /// `UCS` DE 0085 — only when no `UCD` can be given (MIG 2.0b).
        code: Option<String>,
        /// `UCD` — the data element.
        datenelement: Option<Datenelement>,
    },
}

/// A `UCD` — the faulty data element of the segment its `UCS` names.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Datenelement {
    /// `UCD` DE 0085.
    pub code: String,
    /// `UCD` DE 0098 — the segment tag is position 1, each data element or
    /// composite after it one more (CONTRL MIG 2.0b).
    pub position: u32,
    /// `UCD` DE 0104 — the component inside a composite, from 1.
    pub komponente: Option<u32>,
}

/// `UCM` DE 0085 codes — CONTRL AHB 1.0 Kap. 3, SG1 UCM table (S. 26).
const UCM_0085: &[&str] = &["12", "13", "16", "21", "22", "26", "28", "29", "39"];
/// `UCS` DE 0085 codes — CONTRL AHB 1.0 Kap. 3, SG2 UCS table (S. 27).
const UCS_0085: &[&str] = &["13", "15", "16", "22", "35", "36"];
/// `UCD` DE 0085 codes — CONTRL AHB 1.0 Kap. 3, SG2 UCD table (S. 27).
const UCD_0085: &[&str] = &["12", "13", "16", "19", "21", "22", "37", "38", "39", "40"];

/// Check one Übertragungsdatei: `results` is what the parser made of each
/// `UNH`…`UNT` window of `interchange`, in wire order.
///
/// The windows, each message's `UCM` identity and every fault position come from
/// one recovering scan of the same bytes ([`RecoveredInterchange`]), so the
/// CONTRL places a fault where the parser refused it.
///
/// A fault is placed as deep as its location is certain (Kap. 2.2.2: „Die
/// Position des Fehlers ist so genau wie möglich durch die Verwendung der tiefst
/// möglichen Meldungsebene anzugeben"). Where it is not, it is reported at the
/// `UCI` instead, which is still a true statement about the file: the `UCM`
/// carries DE 0085 only for a fault in `UNH` or `UNT`, and without DE 0013 it
/// needs an `SG2 UCS` (CONTRL AHB 1.0 Kap. 3, S. 26, conditions `[2]`, `[3]`, `[9]`). The same holds when the
/// windows cannot be matched to the results, or a message has no readable
/// `UNH` DE 0062 / DE 0065 (both Muss on the `UCM`).
#[must_use]
pub fn syntaxpruefung(
    interchange: &[u8],
    results: &[Result<AnyMessage, edi_energy::Error>],
) -> Syntaxpruefung {
    let first_error = results.iter().find_map(|r| r.as_ref().err());
    let Some(first_error) = first_error else {
        return Syntaxpruefung::Fehlerfrei;
    };
    let whole_file =
        Syntaxpruefung::Uebertragungsdatei(SyntaxFehler::from_parse_error(first_error));
    let decoded = decoded(interchange);
    let recovered = RecoveredInterchange::scan(&decoded, &RecoveryConfig::default());
    let Some(faults) = located_faults(&recovered) else {
        return whole_file;
    };
    if recovered.messages().len() != results.len() {
        return whole_file;
    }
    let mut fehler = Vec::new();
    for (index, (window, result)) in recovered.messages().iter().zip(results).enumerate() {
        let Err(error) = result else { continue };
        let Some(identity) = window
            .identity()
            .filter(|i| !i.message_ref().is_empty() && !i.message_type().is_empty())
        else {
            return whole_file;
        };
        let Some(ort) = fehlerort(&recovered, &faults, index, error) else {
            return whole_file;
        };
        fehler.push(NachrichtenFehler {
            message_ref: identity.message_ref().to_owned(),
            message_type: identity.message_type().to_owned(),
            version: identity.version().to_owned(),
            release: identity.release().to_owned(),
            agency: identity.controlling_agency().to_owned(),
            association: Some(identity.association_code())
                .filter(|a| !a.is_empty())
                .map(str::to_owned),
            fehler: ort,
        });
    }
    Syntaxpruefung::Nachrichten(fehler)
}

/// Where in message `message` of `recovered` the parser's `error` sits, or
/// `None` when that cannot be said on the message's `UCM` / `SG2`.
///
/// An error of the EDIFACT parser is placed by [`RecoveredInterchange::locate`]
/// — or, when it is one of the scan's own faults inside that message, by that
/// fault — and the [`ContrlLocation`] maps as follows:
///
/// | `ContrlLocation` | `Fehlerort` |
/// |---|---|
/// | `UCM` with service segment `UNH` / `UNT` | [`Fehlerort::Nachricht`]: DE 0085, DE 0013 |
/// | `UCS` | [`Fehlerort::Segment`]: DE 0096, DE 0085, no `UCD` |
/// | `UCD` | [`Fehlerort::Segment`]: DE 0096 and a [`Datenelement`] with DE 0085, DE 0098, DE 0104 |
/// | anything else, or another message | `None` — the `UCI` |
///
/// Each code must also be one the AHB lists for that segment ([`UCM_0085`],
/// [`UCS_0085`], [`UCD_0085`]); a code it does not list goes to the `UCI`.
///
/// # An inexact location is not a `UCS`
///
/// An error without a byte position (`MissingRequiredElement`, …) names only a
/// tag, and `locate` places it on the **first** segment of the message with that
/// tag, marked [inexact](ContrlLocation::is_exact). When the tag occurs more than
/// once in the message that position is a guess, and a guessed DE 0096 names a
/// segment the fault may not be in. Only a tag that occurs exactly once makes the
/// position certain, so only then is the `UCS` written.
///
/// One level up is not available either: CONTRL AHB 1.0 Kap. 3, SG1 UCM table
/// (S. 26), gives `UCM` DE 0085 and DE 0013 only under „[2] Wenn Syntaxfehler in
/// UNH vorhanden" ∨ „[3] Wenn Syntaxfehler in UNT vorhanden", and makes `SG2`
/// „Muss [9] Wenn SG1 UCM DE0013 nicht vorhanden". A `UCM` for a fault in the
/// message body therefore cannot stand without a `UCS`, and the fault is reported
/// in the Übertragungsdatei column instead — `UCI` DE 0085 (S. 25).
///
/// Errors of the edi-energy layer — the message type or release in `UNH` names
/// nothing this side reads — are `UCM` DE 0085 `12` „Ungültiger Wert" on `UNH`:
/// `locate` knows only [`edifact_rs::EdifactError`]s.
fn fehlerort(
    recovered: &RecoveredInterchange<'_>,
    faults: &[(&edifact_rs::EdifactError, ContrlLocation)],
    message: usize,
    error: &edi_energy::Error,
) -> Option<Fehlerort> {
    match error {
        edi_energy::Error::UnknownMessageType { .. }
        | edi_energy::Error::FeatureNotEnabled { .. }
        | edi_energy::Error::MissingRelease
        | edi_energy::Error::InvalidRelease(_)
        | edi_energy::Error::ProfileNotFound { .. } => Some(Fehlerort::Nachricht {
            code: edifact_rs::contrl::SyntaxError::InvalidValue
                .code()
                .to_owned(),
            service_segment: "UNH".to_owned(),
        }),
        edi_energy::Error::Parse(inner) => {
            let location = faults
                .iter()
                .find(|(fault, at)| *fault == inner && at.message() == Some(message))
                .map(|(_, at)| at.clone())
                .or_else(|| recovered.locate(inner, Some(message)))?;
            ort_of(recovered, message, &location)
        }
        _ => None,
    }
}

/// `location` as the `SG1 UCM` / `SG2` of message `message`; see [`fehlerort`].
fn ort_of(
    recovered: &RecoveredInterchange<'_>,
    message: usize,
    location: &ContrlLocation,
) -> Option<Fehlerort> {
    use edifact_rs::contrl::ReportingLevel as L;
    if location.message() != Some(message) {
        return None;
    }
    let code = location.error().code();
    match location.level() {
        L::Message => {
            let service_segment = location.service_segment()?;
            UCM_0085.contains(&code).then(|| Fehlerort::Nachricht {
                code: code.to_owned(),
                service_segment: service_segment.to_owned(),
            })
        }
        level @ (L::Segment | L::DataElement) => {
            let position = location.segment_position()?;
            if !location.is_exact() && !tag_is_unique(recovered, message, position) {
                return None;
            }
            if level == L::Segment {
                return UCS_0085.contains(&code).then(|| Fehlerort::Segment {
                    position,
                    code: Some(code.to_owned()),
                    datenelement: None,
                });
            }
            let element = location.element_position()?;
            UCD_0085.contains(&code).then(|| Fehlerort::Segment {
                position,
                code: None,
                datenelement: Some(Datenelement {
                    code: code.to_owned(),
                    position: element,
                    komponente: location.component_position(),
                }),
            })
        }
        _ => None,
    }
}

/// Whether the segment at DE 0096 `position` of message `message` is the only
/// one with its tag in that message — the case where a tag search is exact.
fn tag_is_unique(recovered: &RecoveredInterchange<'_>, message: usize, position: u32) -> bool {
    let Some(window) = recovered.messages().get(message) else {
        return false;
    };
    let Some(last) = window.last() else {
        return false;
    };
    let (Ok(first), Ok(last), Ok(position)) = (
        usize::try_from(window.first()),
        usize::try_from(last),
        usize::try_from(position),
    ) else {
        return false;
    };
    let Some(items) = recovered.items().get(first..=last) else {
        return false;
    };
    let Some(tag) = position
        .checked_sub(1)
        .and_then(|i| items.get(i))
        .and_then(edifact_rs::recover::Scanned::tag)
    else {
        return false;
    };
    items.iter().filter(|item| item.tag() == Some(tag)).count() == 1
}

/// Every fault of the scan with its location, in wire order; `None` when a
/// fault has no location, so the two lists cannot be paired.
fn located_faults<'r>(
    recovered: &'r RecoveredInterchange<'_>,
) -> Option<Vec<(&'r edifact_rs::EdifactError, ContrlLocation)>> {
    let faults: Vec<_> = recovered
        .faults()
        .map(edifact_rs::recover::Fault::error)
        .collect();
    let locations = recovered.fault_locations();
    (faults.len() == locations.len()).then(|| faults.into_iter().zip(locations).collect())
}

/// `raw` in the repertoire the parser reads it in.
///
/// edi-energy transcodes the interchange its `UNB` declares before parsing, so
/// the byte offsets in its errors are offsets into the transcoded bytes; the
/// scan runs over the same bytes so that they agree. A repertoire that does not
/// decode leaves the raw bytes — the parse failed on it, and the scan reports
/// what it can.
fn decoded(raw: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    edifact_rs::decode_interchange(raw).unwrap_or(std::borrow::Cow::Borrowed(raw))
}

/// The `UNB` of a raw interchange: (sender, recipient, control reference).
///
/// Read through the recovering scan rather than the parser, because the parser
/// refuses an interchange whose messages do not tokenize — and a CONTRL for such
/// an interchange still has to be addressed. `None` when there is no `UNB`
/// with all three (CONTRL AHB 1.0 Kap. 2.2.2.1: then no CONTRL can be built).
#[must_use]
pub fn unb_parties(raw: &[u8]) -> Option<(String, String, String)> {
    let decoded = decoded(raw);
    let recovered = RecoveredInterchange::scan(&decoded, &RecoveryConfig::default());
    let address = recovered.address()?;
    Some((
        address.sender_id().to_owned(),
        address.recipient_id().to_owned(),
        address.control_ref().to_owned(),
    ))
}

// ── ContrlAckService ─────────────────────────────────────────────────────────

/// CONTRL Empfangsbestätigung emitter for Gas interchanges.
///
/// Thread-safe; share via `Arc`.  All methods are non-blocking for the caller:
/// the `emit_for_interchange` method awaits only the outbox `enqueue` call and
/// never panics.
///
/// Uses [`SlateDbStore`] directly (not a trait object) because async-fn-in-trait
/// methods are not dyn-compatible.
pub struct ContrlAckService {
    /// Shared store: the CONTRL message and its 6-hour delivery deadline
    /// (CONTRL AHB 1.0 §2.3.1) are written in **one** transaction via
    /// [`SlateDbStore::enqueue_outbox_with_deadlines`], so a crash can never
    /// queue the message without its escalation deadline.
    outbox: Arc<SlateDbStore>,
    tenant_id: TenantId,
    /// Own-party registry. Resolves the inbound interchange's recipient MP-ID
    /// (UNB DE0010) to its [`RoleSparte`] — the authoritative Sparte signal for
    /// the Gas-only CONTRL obligation — and supplies the CONTRL `sender` field
    /// (the addressed own MP-ID, correct even in a multi-Sparte deployment).
    mp_id_registry: Arc<MpIdRegistry>,
}

impl ContrlAckService {
    /// Construct a new service.
    ///
    /// - `outbox`: shared `SlateDbStore` — enqueues the CONTRL message and
    ///   registers its 6h deadline (CONTRL AHB 1.0 §2.3.1) atomically.
    /// - `tenant_id`: the active tenant identifier.
    /// - `mp_id_registry`: the own-party registry, used to resolve the recipient
    ///   MP-ID to its Sparte and to pick the CONTRL sender MP-ID.
    #[must_use]
    pub fn new(
        outbox: Arc<SlateDbStore>,
        tenant_id: TenantId,
        mp_id_registry: Arc<MpIdRegistry>,
    ) -> Self {
        Self {
            outbox,
            tenant_id,
            mp_id_registry,
        }
    }

    /// Emit a CONTRL Empfangsbestätigung for a successfully-parsed Gas interchange
    /// or Gas APERAK receipt.
    ///
    /// **Regulatory basis:**
    /// - CONTRL AHB 1.0 §2.3.1: "Der Empfänger der Übertragungsdatei **oder APERAK**
    ///   teilt dem Absender unverzüglich, jedoch spätestens **6 Stunden** nach Erhalt
    ///   der Übertragungsdatei oder APERAK, das Ergebnis seiner syntaktischen Prüfung
    ///   mittels der Nachricht CONTRL mit."
    /// - APERAK AHB 1.1 §2.3: "Auf eine APERAK ist immer eine CONTRL zu senden."
    ///
    /// This means: we MUST send CONTRL for both Gas interchanges AND Gas APERAKs we
    /// receive.  Only CONTRL-on-CONTRL is forbidden (§2.2.2.2).
    ///
    /// `interchange_ref` is the UNB DE0020 interchange control reference.  Pass
    /// `pi.header.control_ref.as_ref()` from the parsed interchange.  An empty
    /// string is accepted when the control reference is unavailable (e.g. for
    /// bare UNH…UNT messages without a UNB envelope).  The CONTRL renderer treats
    /// an empty `interchange_ref` as absent.
    ///
    /// `recipient_mp_id` is the UNB DE0010 receiver MP-ID — the own MP-ID the
    /// interchange was addressed to. It determines the Sparte (each own party is
    /// exactly one Sparte) and becomes the CONTRL sender. Pass
    /// `pi.header.receiver_id.as_ref()`.
    ///
    /// Passes silently when:
    /// - The interchange is not Gas (recipient MP-ID resolves to Strom, or — for a
    ///   sparte-neutral / unknown recipient — no message carries a Gas signal).
    /// - All messages are CONTRL (§2.2.2.2 exception: no CONTRL-on-CONTRL).
    /// - No sender MP-ID can be extracted from any acknowledgeable message.
    ///
    /// `messages` should contain every successfully-parsed message from one
    /// UNB…UNZ interchange.  Syntax-error messages (parse failures) are not
    /// passed here: a parse failure owes a CONTRL Syntaxfehlermeldung (UCI=4),
    /// which is a different message on a different path.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the atomic outbox+deadline write fails. The
    /// caller must make that durable (dead-letter): the CONTRL is the only proof
    /// of receipt the Gas sender gets, and nothing else would ever retry it.
    pub async fn emit_for_interchange(
        &self,
        messages: &[&AnyMessage],
        interchange_ref: &str,
        recipient_mp_id: &str,
    ) -> Result<(), EngineError> {
        // ── Determine the interchange Sparte ───────────────────────────────────
        // The CONTRL Empfangsbestätigung obligation (CONTRL AHB 1.0 §2.3.1) is a
        // property of the *Übertragungsdatei*, keyed purely on Sparte — every
        // inbound Gas interchange gets a CONTRL; Strom does not.
        //
        // Primary signal: the recipient MP-ID (UNB DE0010) is one of our own
        // parties, and every `[[party]]` covers exactly one Sparte (BDEW §2.13).
        // This is authoritative — unlike PID/release heuristics, which fail for
        // INVOIC/ORDERS/MSCONS (no Sparte prefix in the release code, and NAD
        // DE3055 agency 293 is shared across both sectors in modern MaKo).
        //
        // Fallback (recipient is a sparte-neutral own party or not one of ours):
        // the message-level Gas heuristic — an unambiguous Gas-only PID or a Gas
        // UTILMD release track.
        let is_gas_interchange = match self.mp_id_registry.sparte_of(recipient_mp_id) {
            Some(RoleSparte::Gas) => true,
            Some(RoleSparte::Strom) => false,
            Some(RoleSparte::Both) | None => {
                messages.iter().any(|m| !is_contrl(m) && message_is_gas(m))
            }
        };

        if !is_gas_interchange {
            return Ok(());
        }

        // §2.2.2.2 exception: no CONTRL in response to CONTRL. APERAK is NOT
        // excluded (CONTRL AHB §2.3.1 + APERAK AHB §2.3 mandate a CONTRL even for
        // inbound Gas APERAKs). An interchange of only CONTRL is skipped here.
        let ackable: Vec<&AnyMessage> =
            messages.iter().copied().filter(|m| !is_contrl(m)).collect();
        if ackable.is_empty() {
            return Ok(());
        }

        // Extract sender MP-ID (the CONTRL recipient) from the first message with one.
        let Some(sender_mp_id) = ackable.iter().find_map(|m| sender_mp_id(m)) else {
            tracing::warn!(
                message_count = ackable.len(),
                "CONTRL ack: Gas interchange received but no sender MP-ID found \
                 in any message — Empfangsbestätigung NOT enqueued (regulatory gap)"
            );
            return Ok(());
        };

        self.enqueue(
            sender_mp_id.as_ref(),
            interchange_ref,
            recipient_mp_id,
            Verdict::Empfangsbestaetigung,
            // The Empfangsbestätigung is Gas-only, and §2.3.1 shortens only the
            // ALOCAT — which rides DVGW, not this path.
            mako_fristen::ContrlAnlass::Regelfall,
        )
        .await
    }

    /// Enqueue the Empfangsbestätigung for a **DVGW** gas-transport interchange.
    ///
    /// The obligation is a property of the Übertragungsdatei and keyed on Sparte
    /// (CONTRL AHB 1.0 §2.3.1), and a DVGW interchange is Gas by definition — the
    /// DVGW formats *are* the gas transport layer — so neither of the two
    /// decisions the BDEW path makes from its messages applies here: the Sparte
    /// is not in question, and a DVGW message is never a CONTRL, so the
    /// no-CONTRL-on-CONTRL exception cannot fire.
    ///
    /// What is left is the sender, which the caller reads from `NAD+MS`.
    ///
    /// # Errors
    ///
    /// As [`emit_for_interchange`](Self::emit_for_interchange).
    /// `ist_alocat` shortens the window to 45 minutes: CONTRL AHB 1.0 §2.3.1
    /// singles out „der Prozess der ALOCAT-Übermittlung vom NB an den MGV nach
    /// GABi Gas". It is the caller's to state because the document code that
    /// decides it is in the ingest report, not in the envelope.
    pub async fn emit_for_dvgw_interchange(
        &self,
        sender_mp_id: &str,
        interchange_ref: &str,
        recipient_mp_id: &str,
        ist_alocat: bool,
    ) -> Result<(), EngineError> {
        if sender_mp_id.is_empty() {
            tracing::warn!(
                interchange_ref,
                "CONTRL ack: DVGW interchange has no NAD+MS sender — \
                 Empfangsbestätigung NOT enqueued (regulatory gap)"
            );
            return Ok(());
        }
        self.enqueue(
            sender_mp_id,
            interchange_ref,
            recipient_mp_id,
            Verdict::Empfangsbestaetigung,
            if ist_alocat {
                mako_fristen::ContrlAnlass::GasAlocat
            } else {
                mako_fristen::ContrlAnlass::Regelfall
            },
        )
        .await
    }

    /// Emit the **Syntaxfehlermeldung** for an interchange that cannot be
    /// processed — `UCI` DE 0083 = `4` with the DE 0085 code in `fehler`.
    ///
    /// This is the half of the CONTRL that both Sparten owe. Where the
    /// Empfangsbestätigung is Gas-only, CONTRL AHB 1.0 §2.4 is explicit that in
    /// Strom „wird die CONTRL **ausschließlich** als Syntaxfehlermeldung
    /// eingesetzt" — so there is no Sparte gate here, and the `messages` the
    /// interchange was *meant* to carry decide only the window (§2.4.1 gives a
    /// UTILMD or ORDERS 15 minutes rather than 6 hours).
    ///
    /// Sending it says two things at once (§2.3.2 / §2.4.2): the Übertragungsdatei
    /// arrived, and it „wird nicht weiterbearbeitet". This is the variant for a
    /// fault that cannot be placed on a message; one that can is reported with
    /// [`emit_message_syntax_errors`](Self::emit_message_syntax_errors).
    ///
    /// # When no CONTRL can be sent at all
    ///
    /// §2.2.2.1: the CONTRL's Muss-Datenelemente are copied out of the subject
    /// interchange, so a `UNB` that is itself missing or invalid makes a
    /// syntactically correct CONTRL impossible — „Der Fehler muss dann durch
    /// andere Mittel als durch die CONTRL mitgeteilt werden." The callers
    /// dead-letter that case instead, which is why this takes an already-parsed
    /// interchange reference and sender rather than raw bytes.
    ///
    /// # Errors
    ///
    /// As [`emit_for_interchange`](Self::emit_for_interchange).
    pub async fn emit_syntax_error(
        &self,
        interchange: &[u8],
        interchange_ref: &str,
        recipient_mp_id: &str,
        sender_mp_id: &str,
        fehler: SyntaxFehler,
    ) -> Result<(), EngineError> {
        if sender_mp_id.is_empty() {
            tracing::warn!(
                interchange_ref,
                "CONTRL: no UNB sender on a rejected interchange — \
                 Syntaxfehlermeldung NOT enqueued (regulatory gap)"
            );
            return Ok(());
        }
        let types = unh_message_types(interchange);
        // §2.2.2.2: never a CONTRL in answer to a CONTRL, however broken.
        if !types.is_empty() && types.iter().all(|t| t == "CONTRL") {
            return Ok(());
        }
        let anlass = syntaxfehler_anlass(self.mp_id_registry.sparte_of(recipient_mp_id), &types);
        self.enqueue(
            sender_mp_id,
            interchange_ref,
            recipient_mp_id,
            Verdict::Syntaxfehler(fehler),
            anlass,
        )
        .await
    }

    /// Emit the **Syntaxfehlermeldung in der Nachricht** for an Übertragungsdatei
    /// whose faults [`syntaxpruefung`] located inside its messages.
    ///
    /// One CONTRL for the file (CONTRL AHB 1.0 Kap. 1.3), `UCI` DE 0083 = `4`
    /// without DE 0085, and per broken message an `SG1 UCM` with DE 0083 = `4`
    /// and the `SG2 UCS` / `UCD` naming the segment and data element (Kap. 3).
    /// Owed in both Sparten, under the same windows as
    /// [`emit_syntax_error`](Self::emit_syntax_error); in Gas it is the CONTRL
    /// the file gets instead of an Empfangsbestätigung.
    ///
    /// # Errors
    ///
    /// As [`emit_for_interchange`](Self::emit_for_interchange).
    pub async fn emit_message_syntax_errors(
        &self,
        interchange: &[u8],
        interchange_ref: &str,
        recipient_mp_id: &str,
        sender_mp_id: &str,
        fehler: &[NachrichtenFehler],
    ) -> Result<(), EngineError> {
        if sender_mp_id.is_empty() {
            tracing::warn!(
                interchange_ref,
                "CONTRL: no UNB sender on a rejected interchange — \
                 Syntaxfehlermeldung NOT enqueued (regulatory gap)"
            );
            return Ok(());
        }
        let types = unh_message_types(interchange);
        if !types.is_empty() && types.iter().all(|t| t == "CONTRL") {
            return Ok(());
        }
        let anlass = syntaxfehler_anlass(self.mp_id_registry.sparte_of(recipient_mp_id), &types);
        self.enqueue_with(
            sender_mp_id,
            interchange_ref,
            recipient_mp_id,
            Verdict::SyntaxfehlerInNachrichten,
            anlass,
            fehler,
        )
        .await
    }

    /// Queue the CONTRL and its delivery deadline, atomically.
    ///
    /// `verdict` is what the UCI states about the interchange, and `anlass` the
    /// window it has to be delivered inside — the two are independent: a
    /// Syntaxfehlermeldung on a Strom UTILMD is due in 15 minutes, on a Gas
    /// MSCONS in 6 hours.
    async fn enqueue(
        &self,
        sender_mp_id: &str,
        interchange_ref: &str,
        recipient_mp_id: &str,
        verdict: Verdict,
        anlass: mako_fristen::ContrlAnlass,
    ) -> Result<(), EngineError> {
        self.enqueue_with(
            sender_mp_id,
            interchange_ref,
            recipient_mp_id,
            verdict,
            anlass,
            &[],
        )
        .await
    }

    /// [`enqueue`](Self::enqueue) carrying the per-message report.
    async fn enqueue_with(
        &self,
        sender_mp_id: &str,
        interchange_ref: &str,
        recipient_mp_id: &str,
        verdict: Verdict,
        anlass: mako_fristen::ContrlAnlass,
        fehler: &[NachrichtenFehler],
    ) -> Result<(), EngineError> {
        // CONTRL sender = the own MP-ID the interchange was addressed to (the
        // Sparte-correct MP-ID, even in a multi-Sparte deployment). Fall back to the
        // primary MP-ID when the recipient was resolved only by the heuristic.
        let contrl_sender: &str = if self.mp_id_registry.is_own_mp_id(recipient_mp_id) {
            recipient_mp_id
        } else {
            self.mp_id_registry.primary_mp_id()
        };

        // Construct a synthetic OutboxMessage.
        //
        // This message is not produced by a workflow event — it is an interchange-level
        // protocol obligation.  We use freshly-generated IDs for process/stream/event
        // since there is no domain process associated with the acknowledgement.
        let process_id = ProcessId::new();
        let msg = OutboxMessage::new(
            StreamId::for_process(self.tenant_id, &process_id),
            process_id,
            self.tenant_id,
            CorrelationId::new(),
            ConversationId::new(),
            EventId::new(),
            "CONTRL",
            sender_mp_id,
            serde_json::json!({
                "sender":          contrl_sender,
                "receiver":        sender_mp_id,
                "accepted":        verdict.accepted(),
                // DE 0085, only on a Syntaxfehlermeldung.
                "syntax_error":    verdict.syntax_error(),
                // UNB DE0020 interchange control reference.
                // Surfaced from the parsed interchange header; the CONTRL
                // renderer uses this to populate UCI reference fields.
                "interchange_ref": interchange_ref,
                // SG1 UCM / SG2 UCS / UCD, on a Syntaxfehlermeldung in der
                // Nachricht.
                "messages":        fehler,
            }),
        );

        // The CONTRL delivery deadline (CONTRL AHB 1.0 §2.3.1 / §2.4.1), whose
        // width `anlass` decides.
        //
        // `OutboxWorker::discharge_delivery_window` retires this deadline as
        // soon as the CONTRL is delivered, so it only ever reaches the scheduler
        // when the message did *not* go out — which is what lets
        // `deadline_dispatch` treat a fired `contrl-ack-obligation` as a
        // regulatory violation without re-checking anything.
        //
        // The format version is the latest known FV from the release registry.
        // `contrl-ack-obligation` is not a domain workflow; the FV is used
        // only as a WorkflowId discriminator in the deadline store.
        let fv = crate::adapters::known_fvs()
            .into_iter()
            .max()
            .unwrap_or_else(|| {
                mako_engine::version::FormatVersion::parse("FV2025-10-01")
                    .expect("FV2025-10-01 is a valid fallback format version")
            });
        let due_at = mako_fristen::contrl_due_at(time::OffsetDateTime::now_utc(), anlass);
        let deadline = Deadline::new(
            StreamId::for_process(self.tenant_id, &process_id),
            process_id,
            self.tenant_id,
            WorkflowId::new("contrl-ack-obligation", fv.as_str()),
            mako_fristen::CONTRL_FRIST_LABEL,
            due_at,
        );

        // Message and deadline land in ONE transaction: a crash between two
        // separate writes would queue a CONTRL with no escalation deadline —
        // the exact loss the atomic write path exists to prevent.
        match self
            .outbox
            .enqueue_outbox_with_deadlines(&[msg], &[deadline])
            .await
        {
            Ok(()) => {
                tracing::debug!(
                    sender_mp_id,
                    verdict = verdict.label(),
                    due_at  = %due_at,
                    "CONTRL: message + delivery deadline enqueued atomically",
                );
                Ok(())
            }
            Err(e) => {
                // Log at error: a missing CONTRL triggers §1.3 clarification
                // obligations on the counterparty side.
                tracing::error!(
                    error      = %e,
                    sender_mp_id,
                    verdict = verdict.label(),
                    "CONTRL: atomic outbox+deadline enqueue failed — the regulatory \
                     CONTRL window is at risk (CONTRL AHB 1.0 §2.3.1 / §2.4.1)",
                );
                Err(e)
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// The window a Syntaxfehlermeldung earns, from the recipient's Sparte and the
/// message types the rejected interchange carried.
///
/// §2.3.1 gives the ALOCAT 45 minutes for „die zugehörige CONTRL" without
/// distinguishing the two verdicts, so a broken ALOCAT owes the same window as
/// an accepted one. It is decided first because it names the message and not the
/// Sparte, and an ALOCAT interchange carries no UTILMD or ORDERS to compete
/// with it.
///
/// §2.4.1's 15-minute window sits in the **Strom** chapter, so it applies only
/// where the recipient is a Strom party. A sparte-neutral or unknown recipient
/// keeps the Regelfall — the longer window, and the one that cannot be missed by
/// having been applied wrongly.
fn syntaxfehler_anlass(sparte: Option<RoleSparte>, types: &[String]) -> mako_fristen::ContrlAnlass {
    if types.iter().any(|t| t == "ALOCAT") {
        mako_fristen::ContrlAnlass::GasAlocat
    } else if matches!(sparte, Some(RoleSparte::Strom))
        && types.iter().any(|t| t == "UTILMD" || t == "ORDERS")
    {
        mako_fristen::ContrlAnlass::StromUtilmdOderOrders
    } else {
        mako_fristen::ContrlAnlass::Regelfall
    }
}

/// `true` when the DVGW interchange carried an ALOCAT, which shortens the CONTRL
/// window to 45 minutes (CONTRL AHB 1.0 §2.3.1).
///
/// Read off the ingest report rather than the envelope: the document code that
/// decides it is in the message body.
#[must_use]
pub fn dvgw_report_has_alocat(report: &crate::dvgw_ingest::DvgwIngestReport) -> bool {
    report.messages.iter().any(|m| {
        m.document
            .is_some_and(|d| d.message_type() == dvgw_edi::DvgwMessageType::Alocat)
    })
}

/// The `UNH` DE 0065 message-type code of every message in a raw interchange.
///
/// Read off the wire rather than off a parsed model, because the case that needs
/// it is the one where nothing parsed: a Syntaxfehlermeldung on an unreadable
/// UTILMD still owes the 15-minute window of §2.4.1, and the message type sits
/// in the `UNH` whether or not the body behind it is valid.
///
/// Read through the recovering scan, which reads a `UNH` the parser refused as
/// well; a `UNH` without a readable DE 0062 / DE 0065 contributes nothing, and
/// the Regelfall window that results is the longer one.
fn unh_message_types(interchange: &[u8]) -> Vec<String> {
    let decoded = decoded(interchange);
    RecoveredInterchange::scan(&decoded, &RecoveryConfig::default())
        .messages()
        .iter()
        .filter_map(edifact_rs::recover::MessageWindow::identity)
        .map(|identity| identity.message_type().to_owned())
        .filter(|code| !code.is_empty())
        .collect()
}

/// Returns `true` when the message is a CONTRL.
///
/// Per CONTRL AHB 1.0 §2.2.2.2: "Als Antwort auf eine empfangene CONTRL-Nachricht
/// darf weder eine CONTRL-Nachricht noch eine andere UN/EDIFACT-Nachricht gesendet
/// werden."  No CONTRL-on-CONTRL, ever.
///
/// Note: APERAKs are NOT excluded here — CONTRL AHB §2.3.1 and APERAK AHB §2.3
/// explicitly require a CONTRL reply when a Gas APERAK is received.
fn is_contrl(msg: &AnyMessage) -> bool {
    matches!(msg, AnyMessage::Contrl(_))
}

/// Message-level Gas heuristic — the **fallback** used only when the recipient
/// MP-ID does not resolve to a single Sparte (a sparte-neutral own party, or an
/// interchange not addressed to one of our own MP-IDs).
///
/// The authoritative signal is [`MpIdRegistry::sparte_of`] on the recipient
/// MP-ID; this heuristic exists purely as a best-effort backstop and is
/// deliberately conservative (only *unambiguous* Gas signals return `true`):
///
/// 1. **Unambiguous Gas-only PID** (UTILMD G, INSRPT Gas, INVOIC WiM/GaBi/AWH Gas).
///    These PIDs exist only in Gas profiles; no Strom message can carry them.
///
/// 2. **UTILMD release track** — UTILMD is the only message type whose UNH S009
///    release code carries a Sparte prefix (`G…` = Gas, `S…` = Strom). INVOIC,
///    ORDERS, MSCONS, IFTSTA and INSRPT releases have no Sparte prefix, so the
///    release fallback cannot classify them — those rely on the recipient MP-ID.
///
/// Genuinely ambiguous PIDs (INVOIC NN/MMM/MSB 31001/31002/31005/31006/31009,
/// ORDERS Sperrung 17115–17117) are therefore *not* resolvable by this heuristic
/// alone; they are resolved by the recipient MP-ID in [`emit_for_interchange`].
fn message_is_gas(msg: &AnyMessage) -> bool {
    // Strategy 1: unambiguous Gas-only PID.
    if let Ok(pid) = msg.detect_pruefidentifikator() {
        if is_unambiguous_gas_pid(pid.as_u32()) {
            return true;
        }
        // Strom-only PIDs are not Gas; ambiguous PIDs fall through to strategy 2.
        if is_strom_only_pid(pid.as_u32()) {
            return false;
        }
    }

    // Strategy 2: UTILMD release track (only UTILMD carries a G/S prefix).
    msg.detect_release()
        .ok()
        .map(|r| r.as_ref().starts_with('G'))
        .unwrap_or(false)
}

/// Gas-only PID ranges (cannot appear in Strom interchanges).
///
/// | Range        | Sparte | Message type                             |
/// |--------------|--------|------------------------------------------|
/// | 44001–44053  | Gas    | UTILMD G (GeLi Gas, WiM Gas)             |
/// | 44168–44170  | Gas    | UTILMD G (WiM Gas extensions)            |
/// | 21028        | Gas    | IFTSTA Informationsmeldung (GeLi Gas 2.0, MSB → NB) |
/// | 31007, 31008 | Gas    | INVOIC GaBi Gas Aggreg. MMM-Rechnung (NB → MGV) |
/// | 31010        | Gas    | INVOIC Kapazitätsrechnung (NB → KN)      |
///
/// # What this list must not contain
///
/// A PID belongs here only when **every** row of the BDEW Anwendungsübersicht
/// der Prüfidentifikatoren 4.0 (`PID_4_0_info_20260401.xlsx`, sheet „Prüf-ID
/// Prozessschritt") that carries it has „Sparte Strom" empty. Four did not, and
/// each one made this heuristic send a Gas CONTRL to a Strom counterparty that
/// expects none:
///
/// * **31003** WiM-Rechnung — Strom under *WiM Strom Teil 1* (MSBA → MSBN)
///   as well as Gas under *AWH WiM Gas 2.0*.
/// * **31011** Rechnung sonstige Leistung — Strom under *GPKE Teil 2* as well
///   as Gas under *AWH Sperrprozesse Gas*, both NB → LF. The overview lists the
///   PID twice for exactly this reason.
/// * **23005 / 23009** INSRPT Informationsmeldung — Strom under *WiM Strom
///   Teil 2* as well as Gas under *AWH WiM Gas 2.0*.
///
/// **Not 31004:** the Stornorechnung is a Sparte-neutral universal Storno (INVOIC
/// AHB §3.1.2) — the same PID is used for Strom *and* Gas across GPKE/MMM/WiM/
/// Kapazität/AWH/GeLi. Like the other Sparte-agnostic PIDs it is resolved by
/// recipient MP-ID in [`emit_for_interchange`], never forced to Gas here.
fn is_unambiguous_gas_pid(pid: u32) -> bool {
    matches!(
        pid,
        44001..=44053 | 44168..=44170 | 21028 | 31007 | 31008 | 31010
    )
}

/// Strom-only PID ranges (cannot appear in Gas interchanges).
///
/// Returning `true` here short-circuits the release-track fallback, preventing a
/// Strom message with an ambiguous release code from being misclassified as Gas.
///
/// **Not listed here:** the INVOIC PIDs 31001 (Abschlag), 31002 (NN-Rechnung)
/// and 31005/31006 (MMM). The Anwendungsübersicht 4.0 carries each of them once
/// for Strom and once for Gas — the same Prüfidentifikator, with the Sparte in
/// the message content. Classifying them as Strom-only would suppress the
/// mandatory CONTRL for a Gas NN/MMM invoice. Their Sparte is resolved by the
/// recipient MP-ID in [`emit_for_interchange`].
///
/// **31009 (MSB-Rechnung) is not one of them.** All seven rows the overview
/// carries for it — *GPKE Teil 3* (MSB → NB, MSB → LF), *WiM Strom Teil 1*,
/// *WiM Strom Teil 2* (MSB → ESA, the ESA-Rechnung) and *AWH Prozesse zur
/// Änderung der Technik an Lokationen* — are Strom, and none is Gas: the Gas
/// MSB bills on 31003.
///
/// **21028 is not a Strom PID** either, though it sits inside the IFTSTA range:
/// the overview carries it once, as a GeLi Gas 2.0 Informationsmeldung
/// (MSB → NB), so it is in [`is_unambiguous_gas_pid`] instead. 21024 and 21026
/// appear nowhere in the 4.0 overview at all; the range keeps them because a
/// PID that no longer exists cannot arrive.
fn is_strom_only_pid(pid: u32) -> bool {
    matches!(
        pid,
        // GPKE UTILMD Strom (Lieferbeginn, Lieferende, Kündigung, …)
        55001..=55557
            // GPKE / WiM Strom IFTSTA (Vollzugsmeldung, Ablehnung der Anfrage)
            | 21024..=21027 | 21033 | 21035 | 21045 | 21047
            // INVOIC MSB-Rechnung Strom (GPKE Teil 3 / WiM Strom Teil 1 und 2)
            | 31009
            // MaBiS MSCONS / IFTSTA
            | 13003 | 21000..=21005
    )
}

/// Extract the NAD+MS sender MP-ID from a parsed EDIFACT message.
///
/// Returns `None` when the message has no NAD section (e.g. CONTRL) or when
/// the party_id field is absent or empty.
fn sender_mp_id(msg: &AnyMessage) -> Option<Box<str>> {
    let nad = match msg {
        AnyMessage::Utilmd(m) => m.sender()?,
        AnyMessage::Mscons(m) => m.sender()?,
        AnyMessage::Invoic(m) => m.sender()?,
        AnyMessage::Insrpt(m) => m.sender()?,
        AnyMessage::Orders(m) => m.sender()?,
        AnyMessage::Ordrsp(m) => m.sender()?,
        AnyMessage::Partin(m) => m.sender()?,
        AnyMessage::Iftsta(m) => m.sender()?,
        AnyMessage::Remadv(m) => m.sender()?,
        _ => return None,
    };
    let mp_id = nad.party_id.as_deref().filter(|s| !s.is_empty())?;
    Some(mp_id.into())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{SyntaxFehler, Verdict};

    /// The two `UCI` DE 0083 values CONTRL AHB 1.0 Kap. 3 admits, and the
    /// DE 0085 codes that ride the rejecting one.
    #[test]
    fn the_verdict_carries_the_ahb_code_pair() {
        assert!(Verdict::Empfangsbestaetigung.accepted());
        assert_eq!(
            Verdict::Empfangsbestaetigung.syntax_error(),
            None,
            "an Empfangsbestätigung reports no error, and an empty DE 0085 is \
             not the same as an absent one"
        );

        for (fehler, code) in [
            (SyntaxFehler::UngueltigerWert, "12"),
            (SyntaxFehler::TestKennzeichen, "25"),
            (SyntaxFehler::FalscherEmpfaenger, "7"),
        ] {
            let verdict = Verdict::Syntaxfehler(fehler);
            assert!(!verdict.accepted());
            assert_eq!(verdict.syntax_error(), Some(code), "{fehler:?}");
        }
    }

    /// Every code mako can emit is one Kap. 3 lists in the `UCI` DE 0085 column.
    #[test]
    fn every_emitted_code_is_admitted_at_interchange_level() {
        // CONTRL AHB 1.0 Kap. 3, „Syntaxfehlermeldung in der Übertragungsdatei".
        const UCI_0085: &[&str] = &[
            "2", "7", "12", "13", "16", "20", "21", "23", "25", "26", "28", "29", "32",
        ];
        for fehler in [
            SyntaxFehler::UngueltigerWert,
            SyntaxFehler::TestKennzeichen,
            SyntaxFehler::FalscherEmpfaenger,
        ] {
            assert!(
                UCI_0085.contains(&fehler.de0085()),
                "{fehler:?} emits DE 0085 {} which the AHB does not admit on the UCI",
                fehler.de0085()
            );
        }
    }

    /// §2.4.1 is the Strom chapter, so its short window follows both the Sparte
    /// and the message type. This is the rule itself, decided without a clock:
    /// the window it names is 15 minutes or 6 hours depending on the weekday,
    /// and that arithmetic belongs to `mako_fristen`.
    #[test]
    fn the_short_window_needs_both_strom_and_a_utilmd_or_orders() {
        use super::{RoleSparte as S, syntaxfehler_anlass};
        use mako_fristen::ContrlAnlass as A;

        let utilmd = vec!["UTILMD".to_owned()];
        let orders = vec!["ORDERS".to_owned()];
        let mscons = vec!["MSCONS".to_owned()];

        assert_eq!(
            syntaxfehler_anlass(Some(S::Strom), &utilmd),
            A::StromUtilmdOderOrders
        );
        assert_eq!(
            syntaxfehler_anlass(Some(S::Strom), &orders),
            A::StromUtilmdOderOrders
        );
        // The same message types toward a Gas recipient: §2.3.1, six hours.
        assert_eq!(syntaxfehler_anlass(Some(S::Gas), &utilmd), A::Regelfall);
        // A Strom recipient, and a message type §2.4.1 does not name.
        assert_eq!(syntaxfehler_anlass(Some(S::Strom), &mscons), A::Regelfall);
        // Unknown or sparte-neutral: the longer window.
        assert_eq!(syntaxfehler_anlass(None, &utilmd), A::Regelfall);
        assert_eq!(syntaxfehler_anlass(Some(S::Both), &utilmd), A::Regelfall);

        // §2.3.1 gives „die zugehörige CONTRL" on an ALOCAT 45 minutes and does
        // not exempt the Syntaxfehlermeldung, so the rejected ALOCAT is owed the
        // same window as the accepted one — and it is a DVGW message, so the
        // recipient's Sparte adds nothing to it.
        let alocat = vec!["ALOCAT".to_owned()];
        assert_eq!(syntaxfehler_anlass(Some(S::Gas), &alocat), A::GasAlocat);
        assert_eq!(syntaxfehler_anlass(None, &alocat), A::GasAlocat);
    }

    /// The message type comes off the `UNH` of the raw interchange, because the
    /// Syntaxfehlermeldung is owed exactly when nothing parsed.
    #[test]
    fn the_message_type_is_read_off_the_wire() {
        let raw = b"UNA:+.? 'UNB+UNOC:3+9900123456789:500+9900987654321:500+260912:0900+IC4711'\
                    UNH+1+UTILMD:D:11B:UN:S2.1'BGM+E01+DOC1+9'UNT+3+1'UNZ+1+IC4711'";
        assert_eq!(super::unh_message_types(raw), vec!["UTILMD".to_owned()]);

        // A CONTRL interchange, which §2.2.2.2 forbids answering.
        let contrl = b"UNB+UNOC:3+A+B+260912:0900+IC1'UNH+1+CONTRL:D:3:UN:2.0b'UNT+2+1'UNZ+1+IC1'";
        assert_eq!(super::unh_message_types(contrl), vec!["CONTRL".to_owned()]);

        // Nothing readable contributes nothing, and the caller keeps the
        // Regelfall window.
        assert!(super::unh_message_types(b"garbage without segments").is_empty());
    }

    /// One UTILMD with two `DTM` segments (DE 0096 positions 3 and 4).
    const TWO_DTM: &str = "UNB+UNOC:3+4012345000023:500+9900357000004:500+261001:0700+IC1'\
                           UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E01+X'\
                           DTM+137:202610010000?+00:303'DTM+163:202610012200?+00:303'\
                           UNT+5+1'UNZ+1+IC1'";

    fn parse_error(error: edifact_rs::EdifactError) -> Vec<Result<AnyMessage, edi_energy::Error>> {
        vec![Err(edi_energy::Error::Parse(error))]
    }

    /// A fault carrying its byte offset is placed on the segment that contains
    /// it — the second `DTM` here, not the first segment with that tag.
    #[test]
    fn an_offset_fault_in_the_second_dtm_names_the_second_dtm() {
        let at = TWO_DTM.find("DTM+163").expect("second DTM") + "DTM+163:".len();
        let results = parse_error(edifact_rs::EdifactError::InvalidText { offset: at });
        let Syntaxpruefung::Nachrichten(fehler) = syntaxpruefung(TWO_DTM.as_bytes(), &results)
        else {
            panic!("an exact fault is reported on the message");
        };
        assert_eq!(fehler.len(), 1);
        assert_eq!(fehler[0].message_ref, "1");
        assert_eq!(fehler[0].message_type, "UTILMD");
        assert_eq!(fehler[0].association.as_deref(), Some("S2.1"));
        assert_eq!(
            fehler[0].fehler,
            Fehlerort::Segment {
                position: 4,
                code: None,
                datenelement: Some(Datenelement {
                    // `21` „Ungültige(s) Zeichen"; C507 is element 2, DE 2380
                    // its second component.
                    code: "21".to_owned(),
                    position: 2,
                    komponente: Some(2),
                }),
            }
        );
    }

    /// An error without a byte position names only its tag. With two `DTM`s in
    /// the message no `UCS` position is certain, and the `UCM` cannot carry a
    /// body fault without one (CONTRL AHB 1.0 Kap. 3, S. 26, [2]/[3]/[9]) — so
    /// the file is reported at the `UCI`, never at the first `DTM`.
    #[test]
    fn a_tag_only_fault_in_a_repeated_segment_is_not_given_a_ucs_position() {
        let results = parse_error(edifact_rs::EdifactError::MissingRequiredElement {
            tag: "DTM".into(),
            element_index: 1,
        });
        assert_eq!(
            syntaxpruefung(TWO_DTM.as_bytes(), &results),
            Syntaxpruefung::Uebertragungsdatei(SyntaxFehler::UngueltigerWert)
        );
    }

    /// The same error on a tag the message carries once is exact after all.
    #[test]
    fn a_tag_only_fault_in_a_unique_segment_is_placed_on_it() {
        let results = parse_error(edifact_rs::EdifactError::MissingRequiredElement {
            tag: "BGM".into(),
            element_index: 1,
        });
        let Syntaxpruefung::Nachrichten(fehler) = syntaxpruefung(TWO_DTM.as_bytes(), &results)
        else {
            panic!("a unique tag places the fault");
        };
        assert_eq!(
            fehler[0].fehler,
            Fehlerort::Segment {
                position: 2,
                code: None,
                datenelement: Some(Datenelement {
                    code: "13".to_owned(),
                    position: 3,
                    komponente: None,
                }),
            }
        );
    }

    /// A message type this side does not read is `UCM` `12` on `UNH`.
    #[test]
    fn an_unknown_message_type_is_reported_on_unh() {
        let results = vec![Err(edi_energy::Error::MissingRelease)];
        let Syntaxpruefung::Nachrichten(fehler) = syntaxpruefung(TWO_DTM.as_bytes(), &results)
        else {
            panic!("reported on the message");
        };
        assert_eq!(
            fehler[0].fehler,
            Fehlerort::Nachricht {
                code: "12".to_owned(),
                service_segment: "UNH".to_owned(),
            }
        );
    }

    /// The `UNB` parties are read although a message behind it is broken.
    #[test]
    fn the_unb_parties_survive_a_broken_message() {
        let raw = TWO_DTM.replacen("BGM+", "BGM", 1);
        assert_eq!(
            unb_parties(raw.as_bytes()),
            Some((
                "4012345000023".to_owned(),
                "9900357000004".to_owned(),
                "IC1".to_owned()
            ))
        );
        assert_eq!(unb_parties(b"UNH+1+UTILMD:D:11A:UN'UNT+2+1'"), None);
    }

    /// The Sparte classification must agree with the published Anwendungsübersicht.
    ///
    /// Both predicates below were hand-maintained lists, and **five PIDs were
    /// wrong at once** — 31011, 31009, 31003, 23005/23009 and 21028. Each error
    /// is silent and one-directional: a PID wrongly called Gas sends a CONTRL
    /// into a Strom interchange that expects none, and a PID wrongly called
    /// Strom-only short-circuits the fallback so a mandatory Gas CONTRL is never
    /// emitted at all. Nothing downstream complains either way.
    ///
    /// The source of truth is BDEW's own „Sparte Strom" / „Sparte Gas" columns,
    /// carried into `pid-overview.json` by `cargo xtask import-pid-overview`. A
    /// PID counts as running in a Sparte when **any** of its Prozessschritt rows
    /// marks it — which is exactly the question here: can this Prüfidentifikator
    /// arrive in an interchange of that Sparte?
    ///
    /// The file is tracked, so this gates without `regulatories/`.
    #[test]
    fn the_sparte_lists_match_the_published_overview() {
        #[derive(serde::Deserialize)]
        struct Sparten {
            strom: bool,
            gas: bool,
        }
        #[derive(serde::Deserialize)]
        struct Overview {
            sparten: std::collections::BTreeMap<String, Sparten>,
        }

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/edi-energy/profiles/pid-overview.json"
        );
        let raw = std::fs::read_to_string(path).expect("pid-overview.json is tracked");
        let overview: Overview = serde_json::from_str(&raw).expect("pid-overview.json parses");
        assert!(
            overview.sparten.len() > 400,
            "only {} PIDs carry Sparte data — re-run `cargo xtask import-pid-overview`",
            overview.sparten.len()
        );

        let mut wrong_gas = Vec::new();
        let mut wrong_strom = Vec::new();
        for (pid, sparten) in &overview.sparten {
            let Ok(pid) = pid.parse::<u32>() else {
                continue;
            };

            // Claiming Gas-only for a PID the overview also runs in Strom.
            if is_unambiguous_gas_pid(pid) && sparten.strom {
                wrong_gas.push(pid);
            }
            // Claiming Strom-only for a PID the overview also runs in Gas.
            if is_strom_only_pid(pid) && sparten.gas {
                wrong_strom.push(pid);
            }
        }

        assert!(
            wrong_gas.is_empty(),
            "is_unambiguous_gas_pid claims these run only in Gas, but the \
             Anwendungsuebersicht marks them Sparte Strom too, so a CONTRL would \
             be emitted into a Strom interchange: {wrong_gas:?}"
        );
        assert!(
            wrong_strom.is_empty(),
            "is_strom_only_pid claims these run only in Strom, but the \
             Anwendungsuebersicht marks them Sparte Gas too, so the Gas CONTRL \
             would be short-circuited and never sent: {wrong_strom:?}"
        );
    }

    use super::*;

    #[test]
    fn unambiguous_gas_pids() {
        assert!(is_unambiguous_gas_pid(44001));
        assert!(is_unambiguous_gas_pid(44021));
        assert!(is_unambiguous_gas_pid(44022));
        assert!(is_unambiguous_gas_pid(44053));
        assert!(is_unambiguous_gas_pid(44168));
        assert!(is_unambiguous_gas_pid(44170));
        // IFTSTA 21028 — the Anwendungsübersicht 4.0 carries it once, as a
        // GeLi Gas 2.0 Informationsmeldung (MSB → NB).
        assert!(is_unambiguous_gas_pid(21028));
        assert!(is_unambiguous_gas_pid(31007));
        assert!(is_unambiguous_gas_pid(31008));
        assert!(is_unambiguous_gas_pid(31010));
        // 31004 (Stornorechnung) is Sparte-neutral — NOT unambiguously Gas.
        assert!(!is_unambiguous_gas_pid(31004));
    }

    /// The PIDs this list claimed as Gas-only that the Anwendungsübersicht
    /// carries for **both** Sparten.
    ///
    /// Every one of them made the fallback heuristic answer „Gas" for a Strom
    /// message and send a CONTRL to a counterparty that expects none — Strom
    /// has no CONTRL. Each is a Strom row of `PID_4_0_info_20260401.xlsx`
    /// („Sparte Strom" = X) as well as a Gas one, so the PID alone cannot say
    /// which; only the recipient MP-ID can.
    #[test]
    fn a_pid_used_in_both_sparten_is_not_unambiguously_gas() {
        for (pid, strom_prozess) in [
            (31_003_u32, "WiM Strom Teil 1 (MSBA → MSBN)"),
            (31_011, "GPKE Teil 2 Rechnung sonstige Leistung (NB → LF)"),
            (23_005, "WiM Strom Teil 2 Informationsmeldung"),
            (23_009, "WiM Strom Teil 2 Informationsmeldung"),
        ] {
            assert!(
                !is_unambiguous_gas_pid(pid),
                "PID {pid} also runs in Strom ({strom_prozess}), so forcing it \
                 to Gas sends a CONTRL into a Strom interchange"
            );
            assert!(
                !is_strom_only_pid(pid),
                "PID {pid} also runs in Gas, so suppressing its CONTRL breaches \
                 the 6-hour window"
            );
        }
    }

    #[test]
    fn strom_pids_not_gas() {
        assert!(!is_unambiguous_gas_pid(55001));
        assert!(!is_unambiguous_gas_pid(55039));
        assert!(!is_unambiguous_gas_pid(21024));
        assert!(!is_unambiguous_gas_pid(13003));
        assert!(!is_unambiguous_gas_pid(31001));
        assert!(!is_unambiguous_gas_pid(31002));
    }

    #[test]
    fn strom_only_pids_are_genuinely_strom_only() {
        // Gas-only INVOIC PIDs must NOT be Strom-only.
        assert!(!is_strom_only_pid(31007));
        assert!(!is_strom_only_pid(31008));
        // Genuine Strom-only PIDs: UTILMD Strom, IFTSTA Strom, MaBiS.
        assert!(is_strom_only_pid(55001));
        assert!(is_strom_only_pid(21024));
        assert!(is_strom_only_pid(13003));
        // …and the Gas Informationsmeldung sitting inside the IFTSTA range is
        // not one of them.
        assert!(!is_strom_only_pid(21028));
    }

    /// 31009 is the MSB-Rechnung, and it is Strom.
    ///
    /// All seven rows the Anwendungsübersicht 4.0 carries for 31009 are Strom
    /// (GPKE Teil 3 MSB → NB / MSB → LF, WiM Strom Teil 1, WiM Strom Teil 2
    /// MSB → ESA, AWH Änderung der Technik); the Gas MSB bills on 31003, which
    /// is why 31003 is the one with rows in both Sparten. Calling 31009 Gas
    /// would send a Gas CONTRL into a Strom interchange that expects none.
    #[test]
    fn the_msb_rechnung_is_strom() {
        assert!(is_strom_only_pid(31_009));
        assert!(!is_unambiguous_gas_pid(31_009));
    }

    #[test]
    fn invoic_nne_and_mmm_pids_are_sparte_agnostic() {
        // The Anwendungsübersicht carries 31001 (Abschlag), 31002
        // (NN-Rechnung) and 31005/31006 (MMM) once for Strom and once for Gas —
        // the same PID, Sparte in the content. They must be in NEITHER list, so
        // an inbound Gas NN/MMM invoice is resolved by the recipient MP-ID and
        // not wrongly suppressed as "Strom-only".
        for pid in [31001, 31002, 31005, 31006] {
            assert!(!is_strom_only_pid(pid), "PID {pid} must not be Strom-only");
            assert!(
                !is_unambiguous_gas_pid(pid),
                "PID {pid} must not be unambiguous-Gas"
            );
        }
    }

    /// No PID is in both lists.
    ///
    /// `message_is_gas` asks the Gas list first, so a PID in both would answer
    /// „Gas" and the Strom membership would be silently unreachable.
    #[test]
    fn the_two_lists_are_disjoint() {
        for pid in (13_000..14_000)
            .chain(17_000..18_000)
            .chain(21_000..22_000)
            .chain(23_000..24_000)
            .chain(31_000..32_000)
            .chain(44_000..45_000)
            .chain(55_000..56_000)
        {
            assert!(
                !(is_unambiguous_gas_pid(pid) && is_strom_only_pid(pid)),
                "PID {pid} is in both Sparte lists"
            );
        }
    }

    #[test]
    fn ambiguous_orders_pid_not_in_either_list() {
        // ORDERS 17115/17117 are used by both Gas and Strom Sperrung, and the
        // ORDERS release code carries no Sparte prefix — disambiguation is by the
        // recipient MP-ID (MpIdRegistry::sparte_of) at runtime.
        assert!(!is_unambiguous_gas_pid(17115));
        assert!(!is_unambiguous_gas_pid(17117));
        assert!(!is_strom_only_pid(17115));
        assert!(!is_strom_only_pid(17117));
    }
}
