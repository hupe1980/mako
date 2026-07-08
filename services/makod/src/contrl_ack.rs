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
//! interchange, passing all messages contained in the UNB…UNZ.  The service
//! enqueues a single [`OutboxMessage`] of type `"CONTRL"` which the
//! [`OutboxWorker`] renders via `edifact_renderer::render_contrl` and delivers
//! to the counterparty's AS4 endpoint.
//!
//! Failures are logged at `error` level and do NOT propagate to the caller —
//! the HTTP / AS4 response is unaffected by CONTRL enqueue failures.
//!
//! [`OutboxWorker`]: mako_engine::builder::OutboxWorker

use std::sync::Arc;

use edi_energy::{AnyMessage, EdiEnergyMessage as _};
use mako_engine::{
    deadline::{Deadline, DeadlineStore as _},
    ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId},
    outbox::{OutboxMessage, OutboxStore as _},
    store_slatedb::{SlateDbDeadlineStore, SlateDbStore},
    version::WorkflowId,
};
use time::Duration;

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
    outbox: Arc<SlateDbStore>,
    /// Deadline store for registering the mandatory 6-hour CONTRL delivery window.
    ///
    /// CONTRL AHB 1.0 §2.3.1 requires delivery within 6 wall-clock hours.
    /// Registering a deadline ensures an escalation fires if the `OutboxWorker`
    /// is delayed beyond 6 hours (e.g. due to a network outage or worker crash).
    deadline_store: SlateDbDeadlineStore,
    tenant_id: TenantId,
    /// The tenant's own market-participant identifier (GLN), emitted as the
    /// CONTRL `sender` field (the party acknowledging the inbound interchange).
    own_mp_id: Box<str>,
}

impl ContrlAckService {
    /// Construct a new service.
    ///
    /// - `outbox`: shared `SlateDbStore` for enqueuing the CONTRL message.
    /// - `deadline_store`: persists the 6h CONTRL delivery deadline (CONTRL AHB 1.0 §2.3.1).
    /// - `tenant_id`: the active tenant identifier.
    /// - `own_mp_id`: the tenant's market-participant code (BDEW GLN, 13 digits).
    #[must_use]
    pub fn new(
        outbox: Arc<SlateDbStore>,
        deadline_store: SlateDbDeadlineStore,
        tenant_id: TenantId,
        own_mp_id: impl Into<Box<str>>,
    ) -> Self {
        Self {
            outbox,
            deadline_store,
            tenant_id,
            own_mp_id: own_mp_id.into(),
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
    /// Passes silently when:
    /// - No Gas message is present in `messages`.
    /// - All messages are CONTRL (§2.2.2.2 exception: no CONTRL-on-CONTRL).
    /// - No sender GLN can be extracted from any Gas message.
    ///
    /// `messages` should contain every successfully-parsed message from one
    /// UNB…UNZ interchange.  Syntax-error messages (parse failures) are not
    /// passed here — they should trigger a CONTRL Syntaxfehlermeldung (UCI=4)
    /// via a separate path (not yet implemented, tracked as part of F-033).
    pub async fn emit_for_interchange(&self, messages: &[&AnyMessage], interchange_ref: &str) {
        // §2.2.2.2 exception: no CONTRL in response to CONTRL.
        // APERAK is NOT excluded: CONTRL AHB §2.3.1 + APERAK AHB §2.3 mandate
        // a CONTRL reply even for inbound Gas APERAKs.
        let gas_messages: Vec<&AnyMessage> = messages
            .iter()
            .copied()
            .filter(|m| !is_contrl(m) && is_gas(m))
            .collect();

        if gas_messages.is_empty() {
            return;
        }

        // Extract sender GLN from the first Gas message that has one.
        let Some(sender_mp_id) = gas_messages.iter().find_map(|m| sender_mp_id(m)) else {
            tracing::warn!(
                message_count = gas_messages.len(),
                "CONTRL ack: Gas interchange received but no sender GLN found \
                 in any message — Empfangsbestätigung NOT enqueued (regulatory gap)"
            );
            return;
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
                "sender":          self.own_mp_id.as_ref(),
                "receiver":        sender_mp_id.as_ref(),
                "accepted":        true,
                // UNB DE0020 interchange control reference.
                // Surfaced from the parsed interchange header; the CONTRL
                // renderer uses this to populate UCI reference fields.
                "interchange_ref": interchange_ref,
            }),
        );

        match self.outbox.enqueue(&[msg]).await {
            Ok(()) => {
                // Register the 6-hour CONTRL delivery deadline.
                //
                // CONTRL AHB 1.0 §2.3.1: the Empfangsbestätigung must be delivered
                // within 6 wall-clock hours.  The deadline scheduler fires a
                // `contrl-ack-obligation` event if the OutboxWorker has not cleared
                // the message by then.  The `deadline_dispatch` module logs a
                // regulatory alert for any fired `contrl-ack-obligation` deadlines.
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
                let due_at = time::OffsetDateTime::now_utc() + Duration::hours(6);
                let deadline = Deadline::new(
                    StreamId::for_process(self.tenant_id, &process_id),
                    process_id,
                    self.tenant_id,
                    WorkflowId::new("contrl-ack-obligation", fv.as_str()),
                    "contrl-6h-delivery-window",
                    due_at,
                );
                if let Err(e) = self.deadline_store.register(&deadline).await {
                    tracing::error!(
                        error      = %e,
                        sender_mp_id = sender_mp_id.as_ref(),
                        "CONTRL ack: failed to register 6h deadline — escalation \
                         will not fire if OutboxWorker is delayed (CONTRL AHB 1.0 §2.3.1)",
                    );
                }
                tracing::debug!(
                    sender_mp_id = sender_mp_id.as_ref(),
                    "CONTRL ack: Empfangsbestätigung enqueued for Gas interchange",
                );
            }
            Err(e) => {
                // Log at error: a missing CONTRL triggers §1.3 clarification
                // obligations on the counterparty side (6h deadline violation).
                tracing::error!(
                    error      = %e,
                    sender_mp_id = sender_mp_id.as_ref(),
                    "CONTRL ack: outbox enqueue failed — regulatory 6h CONTRL window \
                     at risk (CONTRL AHB 1.0 §1.2 / APERAK AHB 1.0 §1.2)",
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

/// Returns `true` when the message belongs to the **Gas sparte**.
///
/// Detection uses two complementary strategies:
///
/// 1. **Unambiguous Gas PIDs** (UTILMD G, INSRPT Gas, INVOIC Gas) — checked first.
///    These PIDs exist only in Gas profiles; no Strom message can carry them.
///
/// 2. **UNH S009 release-code prefix** — fallback for messages with no PID or
///    messages whose PID is shared between Gas and Strom (e.g. ORDERS 17115/17117
///    used by both GPKE Sperrung Strom and GeLi Gas Sperrung Gas).
///    Gas release codes start with `"G"` (e.g. `"G1.1"`, `"G3.0"`);
///    Strom release codes start with `"S"` (e.g. `"S2.1"`, `"S2.2"`).
fn is_gas(msg: &AnyMessage) -> bool {
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

    // Strategy 2: release-code prefix.
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
/// | 31003, 31004 | Gas    | INVOIC WiM Gas                           |
/// | 31007, 31008 | Gas    | INVOIC GaBi Gas Aggreg. MMM-Rechnung (NB → MGV) |
/// | 31010, 31011 | Gas    | INVOIC GaBi Gas / GeLi Gas AWH           |
fn is_unambiguous_gas_pid(pid: u32) -> bool {
    matches!(
        pid,
        44001..=44053 | 44168..=44170 | 23005 | 23009 | 31003 | 31004 | 31007 | 31008 | 31010 | 31011
    )
}

/// Strom-only PID ranges (cannot appear in Gas interchanges).
///
/// Returning `true` here short-circuits strategy 2, preventing a Strom
/// message with an ambiguous release code from being misclassified as Gas.
fn is_strom_only_pid(pid: u32) -> bool {
    matches!(
        pid,
        // GPKE UTILMD Strom (Lieferbeginn, Lieferende, Kündigung, …)
        55001..=55557
            // GPKE IFTSTA Strom (Vollzugsmeldung)
            | 21024..=21028 | 21033 | 21035 | 21045 | 21047
            // Strom INVOIC (GPKE, WiM Strom) — 31007/31008 are Gas-only (BK7-14-020)
            | 31001 | 31002 | 31005 | 31006 | 31009
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
        assert!(is_unambiguous_gas_pid(31004));
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
    fn strom_only_excludes_gas_invoic_pids() {
        // 31007 and 31008 are Gas-only (BK7-14-020, Aggreg. MMM-Rechnung Gas, NB → MGV)
        // They must NOT appear in is_strom_only_pid even though 31005–31009 is a natural range.
        assert!(!is_strom_only_pid(31007));
        assert!(!is_strom_only_pid(31008));
        // Confirm Strom INVOIC PIDs are still classified correctly.
        assert!(is_strom_only_pid(31001));
        assert!(is_strom_only_pid(31002));
        assert!(is_strom_only_pid(31005));
        assert!(is_strom_only_pid(31006));
        assert!(is_strom_only_pid(31009));
    }

    #[test]
    fn ambiguous_orders_pid_not_in_either_list() {
        // ORDERS 17115/17117 are used by both Gas and Strom Sperrung —
        // disambiguation falls through to release-code check at runtime.
        assert!(!is_unambiguous_gas_pid(17115));
        assert!(!is_unambiguous_gas_pid(17117));
        assert!(!is_strom_only_pid(17115));
        assert!(!is_strom_only_pid(17117));
    }
}
