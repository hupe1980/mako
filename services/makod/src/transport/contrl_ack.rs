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
//! send a CONTRL Empfangsbestätigung (UCI DE0083 = 7) back to the sender within
//! **6 wall-clock hours**.  Only CONTRL-on-CONTRL is forbidden (§2.2.2.2).
//!
//! For **Strom** interchanges no Empfangsbestätigung is required (only UCI = 4
//! Syntaxfehlermeldung on parse failure, which is handled separately).
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

        self.enqueue(sender_mp_id.as_ref(), interchange_ref, recipient_mp_id)
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
    pub async fn emit_for_dvgw_interchange(
        &self,
        sender_mp_id: &str,
        interchange_ref: &str,
        recipient_mp_id: &str,
    ) -> Result<(), EngineError> {
        if sender_mp_id.is_empty() {
            tracing::warn!(
                interchange_ref,
                "CONTRL ack: DVGW interchange has no NAD+MS sender — \
                 Empfangsbestätigung NOT enqueued (regulatory gap)"
            );
            return Ok(());
        }
        self.enqueue(sender_mp_id, interchange_ref, recipient_mp_id)
            .await
    }

    /// Queue the CONTRL and its 6-hour delivery deadline, atomically.
    async fn enqueue(
        &self,
        sender_mp_id: &str,
        interchange_ref: &str,
        recipient_mp_id: &str,
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
                "accepted":        true,
                // UNB DE0020 interchange control reference.
                // Surfaced from the parsed interchange header; the CONTRL
                // renderer uses this to populate UCI reference fields.
                "interchange_ref": interchange_ref,
            }),
        );

        // The 6-hour CONTRL delivery deadline (CONTRL AHB 1.0 §2.3.1): the
        // Empfangsbestätigung must be delivered within 6 wall-clock hours.
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
        let due_at = mako_fristen::contrl_due_at(time::OffsetDateTime::now_utc());
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
                    "CONTRL ack: Empfangsbestätigung + 6h deadline enqueued atomically",
                );
                Ok(())
            }
            Err(e) => {
                // Log at error: a missing CONTRL triggers §1.3 clarification
                // obligations on the counterparty side (6h deadline violation).
                tracing::error!(
                    error      = %e,
                    sender_mp_id,
                    "CONTRL ack: atomic outbox+deadline enqueue failed — regulatory \
                     6h CONTRL window at risk (CONTRL AHB 1.0 §1.2 / APERAK AHB 1.0 §1.2)",
                );
                Err(e)
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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
/// MSB bills on 31003. Four other sites in this workspace already treated it as
/// Strom-only; only the comment and test here said otherwise.
///
/// **21028 is not a Strom PID** either, though it sits inside the range that
/// used to be written `21024..=21028`: the overview carries it once, as a GeLi
/// Gas 2.0 Informationsmeldung (MSB → NB). It is in [`is_unambiguous_gas_pid`]
/// instead. 21024 and 21026 appear nowhere in the 4.0 overview at all; the
/// range keeps them because a PID that no longer exists cannot arrive.
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
        // …and the Gas Informationsmeldung that used to sit inside the IFTSTA
        // range is not one of them.
        assert!(!is_strom_only_pid(21028));
    }

    /// 31009 is the MSB-Rechnung, and it is Strom.
    ///
    /// This module used to assert the opposite — „used for BOTH Strom and Gas" —
    /// against four other sites in the workspace that treat it as Strom-only.
    /// All seven rows the Anwendungsübersicht 4.0 carries for 31009 are Strom
    /// (GPKE Teil 3 MSB → NB / MSB → LF, WiM Strom Teil 1, WiM Strom Teil 2
    /// MSB → ESA, AWH Änderung der Technik); the Gas MSB bills on 31003, which
    /// is why 31003 is the one with rows in both Sparten.
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
