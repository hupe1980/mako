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
//! Failures are logged at `error` level and do NOT propagate to the caller —
//! the HTTP / AS4 response is unaffected by CONTRL enqueue failures.
//!
//! [`OutboxWorker`]: mako_engine::builder::OutboxWorker

use std::sync::Arc;

use edi_energy::{AnyMessage, EdiEnergyMessage as _};
use mako_engine::{
    deadline::Deadline,
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
    ///   MP-ID to its Sparte and to pick the CONTRL sender GLN.
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
    /// `recipient_mp_id` is the UNB DE0010 receiver GLN — the own MP-ID the
    /// interchange was addressed to. It determines the Sparte (each own party is
    /// exactly one Sparte) and becomes the CONTRL sender. Pass
    /// `pi.header.receiver_id.as_ref()`.
    ///
    /// Passes silently when:
    /// - The interchange is not Gas (recipient MP-ID resolves to Strom, or — for a
    ///   sparte-neutral / unknown recipient — no message carries a Gas signal).
    /// - All messages are CONTRL (§2.2.2.2 exception: no CONTRL-on-CONTRL).
    /// - No sender GLN can be extracted from any acknowledgeable message.
    ///
    /// `messages` should contain every successfully-parsed message from one
    /// UNB…UNZ interchange.  Syntax-error messages (parse failures) are not
    /// passed here — they should trigger a CONTRL Syntaxfehlermeldung (UCI=4)
    /// via a separate path (not yet implemented, tracked as part of F-033).
    pub async fn emit_for_interchange(
        &self,
        messages: &[&AnyMessage],
        interchange_ref: &str,
        recipient_mp_id: &str,
    ) {
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
            return;
        }

        // §2.2.2.2 exception: no CONTRL in response to CONTRL. APERAK is NOT
        // excluded (CONTRL AHB §2.3.1 + APERAK AHB §2.3 mandate a CONTRL even for
        // inbound Gas APERAKs). An interchange of only CONTRL is skipped here.
        let ackable: Vec<&AnyMessage> =
            messages.iter().copied().filter(|m| !is_contrl(m)).collect();
        if ackable.is_empty() {
            return;
        }

        // Extract sender GLN (the CONTRL recipient) from the first message with one.
        let Some(sender_mp_id) = ackable.iter().find_map(|m| sender_mp_id(m)) else {
            tracing::warn!(
                message_count = ackable.len(),
                "CONTRL ack: Gas interchange received but no sender GLN found \
                 in any message — Empfangsbestätigung NOT enqueued (regulatory gap)"
            );
            return;
        };

        // CONTRL sender = the own MP-ID the interchange was addressed to (the
        // Sparte-correct GLN, even in a multi-Sparte deployment). Fall back to the
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
            sender_mp_id.as_ref(),
            serde_json::json!({
                "sender":          contrl_sender,
                "receiver":        sender_mp_id.as_ref(),
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
        let due_at = mako_engine::fristen::contrl_due_at(time::OffsetDateTime::now_utc());
        let deadline = Deadline::new(
            StreamId::for_process(self.tenant_id, &process_id),
            process_id,
            self.tenant_id,
            WorkflowId::new("contrl-ack-obligation", fv.as_str()),
            mako_engine::fristen::CONTRL_FRIST_LABEL,
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
                    sender_mp_id = sender_mp_id.as_ref(),
                    "CONTRL ack: Empfangsbestätigung + 6h deadline enqueued atomically",
                );
            }
            Err(e) => {
                // Log at error: a missing CONTRL triggers §1.3 clarification
                // obligations on the counterparty side (6h deadline violation).
                tracing::error!(
                    error      = %e,
                    sender_mp_id = sender_mp_id.as_ref(),
                    "CONTRL ack: atomic outbox+deadline enqueue failed — regulatory \
                     6h CONTRL window at risk (CONTRL AHB 1.0 §1.2 / APERAK AHB 1.0 §1.2)",
                );
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
/// | 23005, 23009 | Gas    | INSRPT Gas-only variants                 |
/// | 31003        | Gas    | INVOIC WiM Gas Rechnung                  |
/// | 31007, 31008 | Gas    | INVOIC GaBi Gas Aggreg. MMM-Rechnung (NB → MGV) |
/// | 31010, 31011 | Gas    | INVOIC GaBi Gas / GeLi Gas AWH           |
///
/// **Not 31004:** the Stornorechnung is a Sparte-neutral universal Storno (INVOIC
/// AHB §3.1.2) — the same PID is used for Strom *and* Gas across GPKE/MMM/WiM/
/// Kapazität/AWH/GeLi. Like the other Sparte-agnostic INVOIC PIDs it is resolved by
/// recipient MP-ID in [`emit_for_interchange`], never forced to Gas here.
fn is_unambiguous_gas_pid(pid: u32) -> bool {
    matches!(
        pid,
        44001..=44053 | 44168..=44170 | 23005 | 23009 | 31003 | 31007 | 31008 | 31010 | 31011
    )
}

/// Strom-only PID ranges (cannot appear in Gas interchanges).
///
/// Returning `true` here short-circuits the release-track fallback, preventing a
/// Strom message with an ambiguous release code from being misclassified as Gas.
///
/// **Not listed here:** the INVOIC PIDs 31001 (Abschlag), 31002 (NN-Rechnung),
/// 31005/31006 (MMM) and 31009 (MSB-Rechnung). Per the BDEW INVOIC AHB these are
/// **Sparte-agnostic** — the same Prüfidentifikator is used for Strom *and* Gas,
/// with the Sparte carried in the message content. Classifying them as Strom-only
/// would suppress the mandatory CONTRL for a Gas NN/MMM/MSB invoice. Their Sparte
/// is resolved by the recipient MP-ID in [`emit_for_interchange`].
fn is_strom_only_pid(pid: u32) -> bool {
    matches!(
        pid,
        // GPKE UTILMD Strom (Lieferbeginn, Lieferende, Kündigung, …)
        55001..=55557
            // GPKE IFTSTA Strom (Vollzugsmeldung)
            | 21024..=21028 | 21033 | 21035 | 21045 | 21047
            // MaBiS MSCONS / IFTSTA
            | 13003 | 21000..=21005
    )
}

/// Extract the NAD+MS sender GLN from a parsed EDIFACT message.
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
    use super::*;

    #[test]
    fn unambiguous_gas_pids() {
        assert!(is_unambiguous_gas_pid(44001));
        assert!(is_unambiguous_gas_pid(44021));
        assert!(is_unambiguous_gas_pid(44022));
        assert!(is_unambiguous_gas_pid(44053));
        assert!(is_unambiguous_gas_pid(44168));
        assert!(is_unambiguous_gas_pid(44170));
        assert!(is_unambiguous_gas_pid(23005));
        assert!(is_unambiguous_gas_pid(23009));
        assert!(is_unambiguous_gas_pid(31003));
        // 31004 (Stornorechnung) is Sparte-neutral — NOT unambiguously Gas.
        assert!(!is_unambiguous_gas_pid(31004));
        assert!(is_unambiguous_gas_pid(31007));
        assert!(is_unambiguous_gas_pid(31008));
        assert!(is_unambiguous_gas_pid(31010));
        assert!(is_unambiguous_gas_pid(31011));
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
    }

    #[test]
    fn invoic_nne_mmm_msb_pids_are_sparte_agnostic() {
        // Per the BDEW INVOIC AHB, 31001 (Abschlag), 31002 (NN-Rechnung),
        // 31005/31006 (MMM) and 31009 (MSB) are used for BOTH Strom and Gas —
        // the same PID, Sparte in the content. They must be in NEITHER PID list,
        // so an inbound Gas NN/MMM/MSB invoice is resolved by the recipient MP-ID
        // (not wrongly suppressed as "Strom-only").
        for pid in [31001, 31002, 31005, 31006, 31009] {
            assert!(!is_strom_only_pid(pid), "PID {pid} must not be Strom-only");
            assert!(
                !is_unambiguous_gas_pid(pid),
                "PID {pid} must not be unambiguous-Gas"
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
