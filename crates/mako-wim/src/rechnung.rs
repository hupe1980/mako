//! WiM Rechnung — INVOIC-based billing processes for WiM Strom (BK6-24-174).
//!
//! Covers the WiM-Strom **MSB-Rechnung (PID 31009)**: the Messstellenbetreiber
//! (MSB) invoices the Netzbetreiber, Lieferant or Einspeise-Abrechnung (ESA) for
//! metering-point operation. The workflow hosts **both** sides of the exchange —
//! the deployment's market role selects which commands it issues:
//!
//! - **MSB (invoicer / sender):** [`WimRechnungCommand::SendInvoic`] records the
//!   outbound 31009, then awaits the payer's REMADV (33001–33004) / may reject it
//!   with a COMDIS.
//! - **NB/LF/ESA (payer / recipient):** [`WimRechnungCommand::ReceiveInvoic`]
//!   ingests the inbound 31009, then `Settle`/`Dispute` and returns a REMADV.
//!
//! # Covered Prüfidentifikatoren (INVOIC AHB 1.0 / FV2025-10-01)
//!
//! | PID   | Process variant       | Party direction     |
//! |-------|-----------------------|---------------------|
//! | 31009 | MSB-Rechnung          | **MSB → NB/LF/ESA** |
//!
//! **PID 31009 belongs exclusively to the WiM domain.** It must not be registered
//! by `mako-gpke` (see `crates/mako-gpke/src/abrechnung.rs` `GPKE_INVOIC_PIDS` for the
//! explicit exclusion). The Gas twin — WiM-Rechnung 31003 (gMSB → NB) — lives in
//! `mako-wim-gas` (`crates/mako-wim-gas/src/invoic.rs`), duplicated per Sparte.
//!
//! # Regulatory basis
//!
//! - **BDEW WiM** — Wechselprozesse im Messwesen Strom (BDEW BK6-24-174)
//! - **INVOIC AHB 1.0** — EDI@Energy invoice message format (valid FV2025-10-01)
//! - **CONTRL / APERAK** — Acknowledgement (5 Werktage Frist per BK6-24-174)
//!
//! # Implementation status
//!
//! This module implements the full billing workflow state machine:
//!
//! 1. Registers PID 31009 in the PID router (preventing dead-letter routing).
//! 2. Sends the outbound INVOIC (`SendInvoic`, MSB role) or accepts an inbound one
//!    (`ReceiveInvoic`, payer role).
//! 3. Transitions to `PendingSettlement`/`InvoicSent` and registers a 5-Werktage deadline.
//! 4. Accepts `Settle` or `Dispute` commands to close the invoice lifecycle.
//! 5. Accepts inbound REMADV (`ReceiveRemadv`, 33001–33004) and COMDIS (`ReceiveComdis`).
//! 6. Transitions to terminal states: `Settled`, `Disputed`, `PaymentConfirmed`,
//!    `PaymentDisputed`, or `ComdisRejected`.
//!
//! **Not implemented in the application layer (`deadline_dispatch.rs`):**
//! automatic outbound REMADV generation when the 5-Werktage deadline fires without
//! an explicit `Settle`/`Dispute` command. The workflow satisfies the AS4
//! acknowledgement obligation (BDEW AS4-Profile §5) and records the deadline, but
//! `DeadlineExpired` does not itself emit a REMADV.

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// WiM billing Prüfidentifikatoren handled by this workflow (INVOIC AHB 1.0).
///
/// | PID   | Name                                          |
/// |-------|-----------------------------------------------|
/// | 31009 | MSB-Rechnung (MSB → NB/LF/ESA)                |
///
/// **PID 31003** (WiM-Rechnung Gas) belongs to `mako-wim-gas` per
/// `site/content/docs/regulatory/pid-reference.md`. It must not be registered here.
pub const WIM_INVOIC_PIDS: &[u32] = &[31009];

/// REMADV PIDs for WiM Strom billing (inbound payment advice, MSB invoicer role).
///
/// WiM Strom billing uses the same REMADV format as GPKE, including the
/// **itemized (positionsscharf) Strom rejections** — a WiM Strom MSB-Rechnung
/// (31009) can be rejected header+total (33003) or per line item (33004), not
/// only non-itemized (33002). Settlement is „ganz oder gar nicht" (no
/// Teilzahlung), so 33002/33003/33004 are all Abweisungen.
///
/// | PID   | Name                                                        |
/// |-------|-------------------------------------------------------------|
/// | 33001 | Bestätigung (payment confirmed)                             |
/// | 33002 | Abweisung (non-itemized)                                     |
/// | 33003 | Strom Abweisung Kopf und Summe (itemized header+total)       |
/// | 33004 | Strom Abweisung Position (itemized line item)                |
///
/// 33003/33004 are **Strom-only**; the Gas twin (`mako-wim-gas`) keeps 33001/33002.
/// Inbound REMADV routing is by correlation (RFF+Z13 → original 31009 message
/// reference), so this set governs which PIDs the workflow *accepts*, not routing.
///
/// Source: REMADV AHB 1.0a §3, WiM Strom Teil 1, BK6-24-174.
pub const WIM_REMADV_PIDS: &[u32] = &[33001, 33002, 33003, 33004];

/// COMDIS PID for inbound Ablehnung REMADV in WiM (payer role).
///
/// Source: COMDIS AHB 1.0, WiM Strom Teil 1, BK6-24-174.
pub const WIM_COMDIS_ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(29001);

/// Workflow key for WiM billing processes.
pub const WORKFLOW_NAME: &str = "wim-rechnung";

/// Deadline label for the INVOIC settlement response window.
///
/// Per BDEW WiM BK6-24-174, the NB must respond within **5 Werktage** of receipt.
pub const WIM_RECHNUNG_WINDOW_LABEL: &str = "wim-invoic-settlement-deadline";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimRechnungEvent {
    /// INVOIC received and CONTRL acknowledgement enqueued.
    InvoicReceived {
        /// EDIFACT message reference of the INVOIC.
        invoice_ref: MessageRef,
        /// GLN of the billing party (sender).
        sender: MarktpartnerCode,
        /// GLN of the receiving party.
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// BDEW Prüfidentifikator (31009 — MSB-Rechnung).
        ///
        /// PID 31003 (WiM-Rechnung) belongs to `mako-wim-gas`; it is not handled here.
        pruefidentifikator: Pruefidentifikator,
        /// BO4E `Rechnung` object (`rubo4e::current`, schema v202607).
        ///
        /// Serialised at the transport boundary by the EDIFACT adapter and
        /// embedded here so that `invoicd` can run plausibility checks
        /// directly from the `ProcessInitiated` webhook payload without
        /// re-fetching the original EDIFACT interchange.
        rechnung: serde_json::Value,
    },
    /// Outbound INVOIC recorded (MSB invoicer role — the MSB sent the 31009 to the
    /// NB/LF/ESA payer and now awaits a REMADV). State-only, mirroring the GPKE
    /// invoicer side: the billed EDIFACT with amounts is rendered by the billing
    /// module (`netzbilanzd`); this event tracks the process for REMADV correlation.
    InvoicSent {
        /// BDEW Prüfidentifikator of the outbound INVOIC (31009).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the sender (MSB invoicer).
        sender: MarktpartnerCode,
        /// GLN of the recipient (NB/LF/ESA payer).
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference of the outbound INVOIC (REMADV correlation key).
        invoice_ref: MessageRef,
    },
    /// INVOIC rejected immediately due to AHB validation failure.
    ///
    /// A CONTRL with error code is enqueued. No further processing occurs.
    Rejected {
        /// Human-readable rejection reason (from AHB validation issues).
        reason: String,
    },
    /// Settlement deadline expired before a response was issued.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// Invoice settled — CONTRL acknowledgement was dispatched.
    Settled,
    /// Invoice disputed — negative CONTRL or APERAK was dispatched.
    Disputed {
        /// Human-readable dispute reason.
        reason: String,
    },
    /// Inbound REMADV received (MSB invoicer role — payer confirms or disputes).
    ///
    /// 33001 = full payment confirmed; 33002 = non-itemized Abweisung;
    /// 33003 = itemized Kopf+Summe Abweisung; 33004 = itemized Position Abweisung.
    RemadvReceived {
        /// REMADV Prüfidentifikator (33001–33004).
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// GLN of the REMADV sender (payer).
        sender: MarktpartnerCode,
        /// `true` only for 33001 (full payment confirmed); 33002/33003/33004 dispute.
        is_confirmed: bool,
    },
    /// Inbound COMDIS 29001 received (payer role — invoicer rejects our REMADV).
    ComdisAbLehnungReceived {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
}

impl EventPayload for WimRechnungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::InvoicReceived { .. } => "WimRechnungInvoicReceived",
            Self::InvoicSent { .. } => "WimRechnungInvoicSent",
            Self::Rejected { .. } => "WimRechnungRejected",
            Self::DeadlineExpired { .. } => "WimRechnungDeadlineExpired",
            Self::Settled => "WimRechnungSettled",
            Self::Disputed { .. } => "WimRechnungDisputed",
            Self::RemadvReceived { .. } => "WimRechnungRemadvReceived",
            Self::ComdisAbLehnungReceived { .. } => "WimRechnungComdisAbLehnungReceived",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands accepted by the WiM billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimRechnungCommand {
    /// Receive an inbound INVOIC from a WiM market participant.
    ///
    /// The transport layer is responsible for parsing and validating the raw
    /// EDIFACT bytes **before** constructing this command. Pass
    /// `validation_passed: false` and `validation_errors` if AHB validation
    /// found errors; the workflow will emit `Rejected` and enqueue a negative
    /// CONTRL.
    ReceiveInvoic {
        /// EDIFACT message reference from the UNH segment.
        invoice_ref: MessageRef,
        /// GLN of the sender (billing party).
        sender: MarktpartnerCode,
        /// GLN of the recipient.
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD from BGM+DTM).
        document_date: String,
        /// BDEW Prüfidentifikator (31009 — MSB-Rechnung).
        ///
        /// PID 31003 (WiM-Rechnung) belongs to `mako-wim-gas`; it is not handled here.
        pruefidentifikator: Pruefidentifikator,
        /// `true` if AHB profile validation found no errors.
        validation_passed: bool,
        /// Validation error descriptions (empty when `validation_passed`).
        validation_errors: Vec<String>,
        /// BO4E `Rechnung` object (`rubo4e::current`, schema v202607).
        ///
        /// Built by the EDIFACT adapter from the raw INVOIC segments.  Stored
        /// in the `InvoicReceived` event and forwarded in the `ProcessInitiated`
        /// webhook payload so that `invoicd` can validate without a separate
        /// makod API round-trip.
        rechnung: serde_json::Value,
    },
    /// MSB invoicer role: record an outbound INVOIC (31009) sent to the payer.
    ///
    /// The billing module (`netzbilanzd`) renders and dispatches the billed
    /// EDIFACT with amounts; this command records the process so an inbound
    /// REMADV (33001–33004) from the payer correlates back to it. Mirrors the
    /// GPKE invoicer side, duplicated into the WiM crate for a clean Sparte/domain
    /// boundary (no reuse of `GpkeAbrechnungWorkflow`).
    SendInvoic {
        /// BDEW Prüfidentifikator of the outbound INVOIC (must be 31009).
        pid: Pruefidentifikator,
        /// GLN of the sender (MSB invoicer).
        sender: MarktpartnerCode,
        /// GLN of the recipient (NB/LF/ESA payer).
        recipient: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference of the outbound INVOIC (REMADV correlation key).
        invoice_ref: MessageRef,
    },
    /// Settle the invoice — a positive CONTRL will be dispatched.
    Settle,
    /// Dispute the invoice — a negative CONTRL / APERAK will be dispatched.
    Dispute {
        /// Human-readable dispute reason.
        reason: String,
    },
    /// The settlement deadline fired before a response was issued.
    ///
    /// Fired by the `DeadlineScheduler` when the `wim-invoic-settlement-deadline`
    /// deadline expires. The workflow emits `DeadlineExpired` and the outbox
    /// worker sends a late-notice APERAK.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
    /// MSB invoicer role: inbound REMADV received from the payer.
    ///
    /// PIDs 33001–33004 (REMADV AHB 1.0a §3, WiM Strom Teil 1, BK6-24-174);
    /// 33003/33004 are the itemized Strom Abweisungen.
    ReceiveRemadv {
        /// REMADV Prüfidentifikator (33001–33004).
        pid: Pruefidentifikator,
        /// EDIFACT message reference of the REMADV.
        remadv_ref: MessageRef,
        /// GLN of the REMADV sender (payer).
        sender: MarktpartnerCode,
    },
    /// Payer role: inbound COMDIS 29001 received (invoicer rejects our REMADV).
    ///
    /// COMDIS PID 29001 (Ablehnung REMADV, COMDIS AHB 1.0, WiM BK6-24-174).
    ReceiveComdis {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
}

impl CommandPayload for WimRechnungCommand {}

// ── Workflow state ─────────────────────────────────────────────────────────────

/// Internal state of the WiM billing workflow.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum WimRechnungState {
    /// No INVOIC received yet.
    #[default]
    New,
    /// INVOIC received; awaiting settlement or dispute action.
    PendingSettlement {
        /// Invoice reference for correlation.
        invoice_ref: MessageRef,
        /// BDEW Prüfidentifikator (31009).
        pruefidentifikator: Pruefidentifikator,
        /// BO4E `Rechnung` object — retained in state so it survives replay
        /// and is accessible to `GET /api/v1/invoic/{process_id}/rechnung`.
        rechnung: serde_json::Value,
    },
    /// Outbound INVOIC recorded (MSB invoicer role); awaiting the payer's REMADV.
    InvoicSent {
        /// Invoice reference for REMADV correlation.
        invoice_ref: MessageRef,
        /// BDEW Prüfidentifikator (31009).
        pruefidentifikator: Pruefidentifikator,
    },
    /// Invoice was accepted and settled.
    Settled,
    /// Invoice was disputed.
    Disputed {
        /// Human-readable dispute reason.
        reason: String,
    },
    /// Invoice was rejected due to AHB validation failure or deadline expiry.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// Payment confirmed by payer (REMADV 33001 received).
    PaymentConfirmed,
    /// Payment disputed by payer (REMADV 33002 received).
    PaymentDisputed {
        /// REMADV PID (33002).
        remadv_pid: Pruefidentifikator,
    },
    /// Invoicer rejected our REMADV (COMDIS 29001 received, payer role).
    ComdisRejected,
}

// ── Workflow implementation ───────────────────────────────────────────────────

/// WiM Strom billing workflow for INVOIC PID 31009 (MSB-Rechnung).
pub struct WimRechnungWorkflow;

impl Workflow for WimRechnungWorkflow {
    type Command = WimRechnungCommand;
    type Event = WimRechnungEvent;
    type State = WimRechnungState;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            WimRechnungEvent::InvoicReceived {
                invoice_ref,
                pruefidentifikator,
                rechnung,
                ..
            } => WimRechnungState::PendingSettlement {
                invoice_ref: invoice_ref.clone(),
                pruefidentifikator: *pruefidentifikator,
                rechnung: rechnung.clone(),
            },
            WimRechnungEvent::InvoicSent {
                invoice_ref,
                pruefidentifikator,
                ..
            } => WimRechnungState::InvoicSent {
                invoice_ref: invoice_ref.clone(),
                pruefidentifikator: *pruefidentifikator,
            },
            WimRechnungEvent::Rejected { reason } => WimRechnungState::Rejected {
                reason: reason.clone(),
            },
            WimRechnungEvent::Settled => WimRechnungState::Settled,
            WimRechnungEvent::Disputed { reason } => WimRechnungState::Disputed {
                reason: reason.clone(),
            },
            WimRechnungEvent::DeadlineExpired { label, .. } => match state {
                // Terminal states — do not overwrite with deadline expiry.
                WimRechnungState::Settled
                | WimRechnungState::Disputed { .. }
                | WimRechnungState::Rejected { .. }
                | WimRechnungState::PaymentConfirmed
                | WimRechnungState::PaymentDisputed { .. }
                | WimRechnungState::ComdisRejected => state,
                _ => WimRechnungState::Rejected {
                    reason: format!("settlement deadline expired: {label}"),
                },
            },
            WimRechnungEvent::RemadvReceived {
                pid, is_confirmed, ..
            } => {
                if *is_confirmed {
                    WimRechnungState::PaymentConfirmed
                } else {
                    WimRechnungState::PaymentDisputed { remadv_pid: *pid }
                }
            }
            WimRechnungEvent::ComdisAbLehnungReceived { .. } => WimRechnungState::ComdisRejected,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            WimRechnungCommand::ReceiveInvoic {
                invoice_ref,
                sender,
                recipient,
                document_date,
                pruefidentifikator,
                validation_passed,
                validation_errors,
                rechnung,
            } => {
                if !matches!(state, WimRechnungState::New) {
                    return Err(WorkflowError::invalid_state("New", format!("{state:?}")));
                }
                if !WIM_INVOIC_PIDS.contains(&pruefidentifikator.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a WiM INVOIC PID (31009), got {pruefidentifikator}"
                    )));
                }
                if validation_passed {
                    // Notify `invoicd` that a validated WiM INVOIC is ready for
                    // plausibility checking.  The `Rechnung` BO4E object is
                    // embedded so that `invoicd` can run checks directly from
                    // the webhook payload (same pattern as GPKE abrechnung).
                    let outbox = vec![
                        PendingOutbox::new(
                            "ProcessInitiated",
                            recipient.as_str(),
                            serde_json::json!({
                                "pid":          pruefidentifikator.as_u32(),
                                "invoice_ref":  invoice_ref.as_str(),
                                "sender_mp_id": sender.as_str(),
                                "rechnung":     rechnung,
                            }),
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox(
                        vec![WimRechnungEvent::InvoicReceived {
                            invoice_ref,
                            sender,
                            recipient,
                            document_date,
                            pruefidentifikator,
                            rechnung: outbox[0]
                                .payload
                                .get("rechnung")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        }],
                        outbox,
                    ))
                } else {
                    Ok(WorkflowOutput::events(vec![WimRechnungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    }]))
                }
            }

            WimRechnungCommand::SendInvoic {
                pid,
                sender,
                recipient,
                document_date,
                invoice_ref,
            } => {
                if !matches!(state, WimRechnungState::New) {
                    return Err(WorkflowError::invalid_state("New", format!("{state:?}")));
                }
                if !WIM_INVOIC_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a WiM INVOIC PID (31009), got {pid}"
                    )));
                }
                Ok(WorkflowOutput::events(vec![WimRechnungEvent::InvoicSent {
                    pruefidentifikator: pid,
                    sender,
                    recipient,
                    document_date,
                    invoice_ref,
                }]))
            }

            WimRechnungCommand::Settle => {
                if !matches!(state, WimRechnungState::PendingSettlement { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                let (pid, invoice_ref) = match &state {
                    WimRechnungState::PendingSettlement {
                        pruefidentifikator,
                        invoice_ref,
                        ..
                    } => (pruefidentifikator.as_u32(), invoice_ref.to_string()),
                    _ => (0, String::new()),
                };
                Ok(WorkflowOutput::with_outbox(
                    vec![WimRechnungEvent::Settled],
                    vec![PendingOutbox::new(
                        "ProcessCompleted",
                        "",
                        serde_json::json!({
                            "pid": pid,
                            "invoice_ref": invoice_ref,
                            "outcome": "settled",
                        }),
                    )],
                ))
            }

            WimRechnungCommand::Dispute { reason } => {
                if !matches!(state, WimRechnungState::PendingSettlement { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                let (pid, invoice_ref) = match &state {
                    WimRechnungState::PendingSettlement {
                        pruefidentifikator,
                        invoice_ref,
                        ..
                    } => (pruefidentifikator.as_u32(), invoice_ref.to_string()),
                    _ => (0, String::new()),
                };
                Ok(WorkflowOutput::with_outbox(
                    vec![WimRechnungEvent::Disputed {
                        reason: reason.clone(),
                    }],
                    vec![PendingOutbox::new(
                        "ProcessCompleted",
                        "",
                        serde_json::json!({
                            "pid": pid,
                            "invoice_ref": invoice_ref,
                            "outcome": "disputed",
                            "reason": reason,
                        }),
                    )],
                ))
            }

            WimRechnungCommand::TimeoutExpired { deadline_id, label } => {
                if !matches!(state, WimRechnungState::PendingSettlement { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                Ok(WorkflowOutput::events(vec![
                    WimRechnungEvent::DeadlineExpired { deadline_id, label },
                ]))
            }

            WimRechnungCommand::ReceiveRemadv {
                pid,
                remadv_ref,
                sender,
            } => {
                if !matches!(
                    state,
                    WimRechnungState::Settled
                        | WimRechnungState::PendingSettlement { .. }
                        | WimRechnungState::InvoicSent { .. }
                ) {
                    return Err(WorkflowError::invalid_state(
                        "Settled|PendingSettlement|InvoicSent",
                        format!("{state:?}"),
                    ));
                }
                if !WIM_REMADV_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a WiM REMADV PID (33001–33004), got {pid}",
                    )));
                }
                let is_confirmed = pid.as_u32() == 33001;
                Ok(WorkflowOutput::events(vec![
                    WimRechnungEvent::RemadvReceived {
                        pid,
                        remadv_ref,
                        sender,
                        is_confirmed,
                    },
                ]))
            }

            WimRechnungCommand::ReceiveComdis { comdis_ref } => {
                if matches!(
                    state,
                    WimRechnungState::New
                        | WimRechnungState::Rejected { .. }
                        | WimRechnungState::ComdisRejected
                ) {
                    return Err(WorkflowError::invalid_state(
                        "Settled|PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                Ok(WorkflowOutput::events(vec![
                    WimRechnungEvent::ComdisAbLehnungReceived { comdis_ref },
                ]))
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).expect("valid PID")
    }

    fn mp(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }

    fn send_cmd(p: u32) -> WimRechnungCommand {
        WimRechnungCommand::SendInvoic {
            pid: pid(p),
            sender: mp("9900123456789"),    // MSB (invoicer)
            recipient: mp("9900987654321"), // NB/LF/ESA (payer)
            document_date: "20260731".into(),
            invoice_ref: MessageRef::new("MSB-RE-001"),
        }
    }

    fn remadv_cmd(p: u32) -> WimRechnungCommand {
        WimRechnungCommand::ReceiveRemadv {
            pid: pid(p),
            remadv_ref: MessageRef::new("REMADV-001"),
            sender: mp("9900987654321"),
        }
    }

    // ── SendInvoic (MSB invoicer side) ──────────────────────────────────────────

    #[test]
    fn send_invoic_31009_from_new_emits_invoic_sent() {
        let out = WimRechnungWorkflow::handle(&WimRechnungState::New, send_cmd(31009))
            .expect("valid 31009 send must succeed");
        assert_eq!(out.events.len(), 1);
        assert!(matches!(out.events[0], WimRechnungEvent::InvoicSent { .. }));
        // State transitions to InvoicSent (awaiting REMADV).
        let state = WimRechnungWorkflow::apply(WimRechnungState::New, &out.events[0]);
        assert!(matches!(state, WimRechnungState::InvoicSent { .. }));
    }

    #[test]
    fn send_invoic_rejects_non_31009_pid() {
        // 31003 is the Gas twin (mako-wim-gas), not a Strom WiM INVOIC PID.
        let err = WimRechnungWorkflow::handle(&WimRechnungState::New, send_cmd(31003))
            .expect_err("31003 must be rejected by the Strom WiM workflow");
        assert!(format!("{err}").contains("31009"));
    }

    #[test]
    fn send_invoic_from_wrong_state_errors() {
        let state = WimRechnungState::Settled;
        let err = WimRechnungWorkflow::handle(&state, send_cmd(31009))
            .expect_err("send from a non-New state must be rejected");
        assert!(format!("{err}").contains("New"));
    }

    // ── REMADV after send (itemized Strom rejections 33003/33004) ───────────────

    #[test]
    fn remadv_33001_confirms_payment_after_send() {
        let state = WimRechnungState::InvoicSent {
            invoice_ref: MessageRef::new("MSB-RE-001"),
            pruefidentifikator: pid(31009),
        };
        let out = WimRechnungWorkflow::handle(&state, remadv_cmd(33001))
            .expect("33001 REMADV must be accepted after send");
        let new_state = WimRechnungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(new_state, WimRechnungState::PaymentConfirmed));
    }

    #[test]
    fn remadv_33003_itemized_kopf_summe_rejection_after_send() {
        let state = WimRechnungState::InvoicSent {
            invoice_ref: MessageRef::new("MSB-RE-001"),
            pruefidentifikator: pid(31009),
        };
        let out = WimRechnungWorkflow::handle(&state, remadv_cmd(33003))
            .expect("33003 itemized rejection must be accepted");
        assert!(matches!(
            out.events[0],
            WimRechnungEvent::RemadvReceived {
                is_confirmed: false,
                ..
            }
        ));
        let new_state = WimRechnungWorkflow::apply(state, &out.events[0]);
        match new_state {
            WimRechnungState::PaymentDisputed { remadv_pid } => {
                assert_eq!(remadv_pid.as_u32(), 33003);
            }
            other => panic!("expected PaymentDisputed(33003), got {other:?}"),
        }
    }

    #[test]
    fn remadv_33004_itemized_position_rejection_after_send() {
        let state = WimRechnungState::InvoicSent {
            invoice_ref: MessageRef::new("MSB-RE-001"),
            pruefidentifikator: pid(31009),
        };
        let out = WimRechnungWorkflow::handle(&state, remadv_cmd(33004))
            .expect("33004 itemized rejection must be accepted");
        let new_state = WimRechnungWorkflow::apply(state, &out.events[0]);
        assert!(matches!(
            new_state,
            WimRechnungState::PaymentDisputed { remadv_pid } if remadv_pid.as_u32() == 33004
        ));
    }

    #[test]
    fn remadv_unknown_pid_is_rejected() {
        let state = WimRechnungState::InvoicSent {
            invoice_ref: MessageRef::new("MSB-RE-001"),
            pruefidentifikator: pid(31009),
        };
        let err = WimRechnungWorkflow::handle(&state, remadv_cmd(33099))
            .expect_err("an out-of-range REMADV PID must be rejected");
        assert!(format!("{err}").contains("33001"));
    }
}
