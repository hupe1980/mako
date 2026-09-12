//! CONTRL Empfangsbestätigung service — CONTRL AHB 1.0 §2.3 / APERAK AHB 1.0 §2.3.
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
//! > — APERAK AHB 1.0 §2.3 (Gas rules)
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
//! and `4` „Diese Ebene und alle tieferen Ebenen zurückgewiesen", the latter
//! carrying a DE 0085 [`SyntaxFehler`]. The Sparte decides whether a *clean*
//! interchange is acknowledged; it never decides whether a broken one is
//! reported, which both Sparten owe.
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
}

impl Verdict {
    const fn accepted(self) -> bool {
        matches!(self, Self::Empfangsbestaetigung)
    }

    const fn syntax_error(self) -> Option<&'static str> {
        match self {
            Self::Empfangsbestaetigung => None,
            Self::Syntaxfehler(code) => Some(code.de0085()),
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Empfangsbestaetigung => "Empfangsbestätigung",
            Self::Syntaxfehler(_) => "Syntaxfehlermeldung",
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

// ── ContrlAckService ─────────────────────────────────────────────────────────

/// CONTRL Empfangsbestätigung emitter for Gas interchanges.
///
/// Thread-safe; share via `Arc`.  All methods are non-blocking for the caller:
/// the `emit_for_interchange` method awaits only the outbox `enqueue` call and
/// never panics.
///
/// Uses [`SlateDbStore`] directly (not a trait object) because async-fn-in-trait
/// methods are not yet dyn-compatible in Rust 1.89.
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
    /// - APERAK AHB 1.0 §2.3: "Auf eine APERAK ist immer eine CONTRL zu senden."
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
    /// arrived, and it „wird nicht weiterbearbeitet". Call it only where that is
    /// true of the whole file — a fault inside one message of an otherwise
    /// readable interchange is reported per message, which needs the segment
    /// positions the ingest loop does not carry.
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
/// Deliberately a scan and not a parse. It reads what it can and returns what it
/// found; a garbled `UNH` simply contributes nothing, and the Regelfall window
/// that results is the longer one.
fn unh_message_types(interchange: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(interchange);
    let mut out = Vec::new();
    for segment in text.split('\'') {
        let segment = segment.trim_start_matches(['\n', '\r', ' ']);
        let Some(rest) = segment.strip_prefix("UNH+") else {
            continue;
        };
        // `UNH+<ref>+<type>:<version>:…`
        if let Some(after_ref) = rest.split_once('+').map(|(_, r)| r) {
            let code = after_ref
                .split([':', '+'])
                .next()
                .unwrap_or_default()
                .trim();
            if !code.is_empty() {
                out.push(code.to_owned());
            }
        }
    }
    out
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
