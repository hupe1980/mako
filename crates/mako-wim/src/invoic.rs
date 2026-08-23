//! WiM Rechnung — the INVOIC billing processes of WiM Strom and WiM Gas.
//!
//! Three Prüfidentifikatoren, listed in [`WIM_INVOIC_PIDS`]. The workflow hosts
//! **both** sides of each exchange — the deployment's Marktrolle selects which
//! commands it issues:
//!
//! - **MSB (invoicer):** [`WimInvoicCommand::SendInvoic`] records the outbound
//!   invoice, then awaits the payer's REMADV (33001–33004) and may refuse it
//!   with a COMDIS.
//! - **Payer:** [`WimInvoicCommand::ReceiveInvoic`] ingests it, then
//!   `SettleInvoice`/`DisputeInvoice` returns the REMADV.
//!
//! **31009 belongs exclusively to the WiM domain** and must not be registered
//! by `mako-gpke` — see `GPKE_INVOIC_PIDS` there for the explicit exclusion.
//!
//! # Answer windows
//!
//! Every WiM invoice is answered against the **Zahlungsziel it carries**
//! (`SG8 DTM+265`), never a flat Werktage count from arrival. Where the answer
//! sits relative to that date depends on who pays:
//!
//! | Rechnung | Zahler | Spätester ÜT der Antwort | Fundstelle |
//! |---|---|---|---|
//! | MSB-Rechnung 31009 | NB | **4. WT vor** dem Zahlungsziel | WiM Teil 1 Kap. 6.2 Nr. 2 |
//! | MSB-Rechnung 31009 | LF · ESA | zum Zahlungsziel | Kap. 3.6.3.8.2 Nr. 2/4 |
//! | WiM-Rechnung 31003 | NB · MSBN | zum Zahlungsziel | Kap. 3.7.2 Nr. 2/4 |
//!
//! The MSB's Mitteilung that a refused invoice was correct after all (COMDIS
//! 29001) is due by the **2. WT vor** dem Zahlungsziel (Kap. 6.2 Nr. 3), and
//! the Zahlungsziel itself may not fall short of 10 Werktage after receipt.
//! [`mako_fristen::vorlauf`] holds all four as one table; `makod` registers the
//! process deadline from it.
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-22-024 Anlage 2a** — WiM Strom Teil 1, Kap. 3.6.3.8 / 3.7 / 6
//! - **AWH WiM Gas 2.0** — Kap. 4.7 (Abrechnung von Dienstleistungen)
//! - **INVOIC AHB 1.0b** — EDI@Energy invoice message format

use std::collections::HashMap;

use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// WiM billing Prüfidentifikatoren handled by this workflow (INVOIC AHB 1.0b),
/// in **both Sparten**.
///
/// | PID | Name | Empfänger | Sparte | Fundstelle |
/// |---|---|---|---|---|
/// | 31009 | MSB-Rechnung | NB · LF · ESA | Strom | GPKE Teil 3, WiM Strom Teil 1/2, AWH Änd. Technik |
/// | 31003 | WiM-Rechnung (Abrechnung von Dienstleistungen im Messwesen) | NB · MSBN | **beide** | WiM Strom Teil 1 Kap. 3.7, AWH WiM Gas 2.0 Kap. 4.7 |
/// | 31004 | Stornorechnung | wie die Ursprungsrechnung | **neutral** | INVOIC AHB §3.1.2 |
///
/// **31003 is not the Gas twin of 31009.** They are different Abrechnungen:
/// 31009 bills the *Messstellenbetrieb* to the NB, LF or ESA and exists only in
/// Strom; 31003 bills the *Dienstleistungen* between the abgebender and the
/// aufnehmender MSB — the temporäre Fortführung, the Geräteübernahme and a
/// Zwischen- oder Kontrollablesung — and exists in both Sparten.
///
/// The Gas Ablehnung splits by **who refuses whose invoice**, not by PID
/// (EBD 4.3 Kap. 14.7) — [`gas_ablehnungs_ebd`] resolves it.
pub const WIM_INVOIC_PIDS: &[u32] = &[31009, 31003, 31004];

/// Who refused the Gas invoice, and what it invoiced — the pair that picks the
/// Ablehnungs-Entscheidungsbaum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GasAblehnung {
    /// The **NB** refuses a Rechnung that names a Marktlokation — `E_2014`.
    NbRechnung,
    /// The **MSBN** refuses a Rechnung — `E_2015`.
    MsbnRechnung,
    /// The **NB** refuses a Rechnung that names only a Messlokation — `E_2016`.
    NbMesslokationsRechnung,
    /// The **NB** refuses a Stornorechnung — `E_2018`.
    NbStorno,
    /// The **MSBN** refuses a Stornorechnung — `E_2019`.
    MsbnStorno,
}

/// The Gas Ablehnungs-Entscheidungsbaum for a refusal.
///
/// EBD 4.3 Kap. 14.7 splits one INVOIC family across five trees, and the PID is
/// not what tells them apart: `E_2014`/`E_2016` are the NB's, `E_2015` the
/// MSBN's, and the two Storno trees repeat that split. `E_2017`
/// („Nichtzahlungsavis prüfen") has no tree, „da keine Antwort gegeben wird",
/// so a Zahlungsavis carries no `AJT`.
#[must_use]
pub const fn gas_ablehnungs_ebd(ablehnung: GasAblehnung) -> &'static str {
    match ablehnung {
        GasAblehnung::NbRechnung => mako_pruefung::codes::EBD_WIM_RECHNUNG_NB_GAS,
        GasAblehnung::MsbnRechnung => mako_pruefung::codes::EBD_WIM_RECHNUNG_MSBN_GAS,
        GasAblehnung::NbMesslokationsRechnung => mako_pruefung::codes::EBD_WIM_RECHNUNG_MELO_GAS,
        GasAblehnung::NbStorno => mako_pruefung::codes::EBD_WIM_STORNO_GAS,
        GasAblehnung::MsbnStorno => mako_pruefung::codes::EBD_WIM_STORNO_MSBN_GAS,
    }
}

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
/// 33003/33004 are **Strom-only**: the Gas WiM-Rechnung 31003 is rejected with
/// 33002 alone (REMADV AHB 1.0a; PID-Übersicht 4.0 rows 39780–39910).
/// Inbound REMADV routing is by correlation (RFF+Z13 → original 31009 message
/// reference), so this set governs which PIDs the workflow *accepts*, not routing.
///
/// Source: REMADV AHB 1.0a §3, WiM Strom Teil 1 (BK6-22-024).
pub const WIM_REMADV_PIDS: &[u32] = &[33001, 33002, 33003, 33004];

/// COMDIS PID for inbound Ablehnung REMADV in WiM (payer role).
///
/// Source: COMDIS AHB 1.0, WiM Strom Teil 1 (BK6-22-024).
pub const WIM_COMDIS_ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(29001);

/// Workflow key for WiM billing processes.
pub const WORKFLOW_NAME: &str = "wim-invoic";

/// Deadline label for the INVOIC settlement response window.
///
/// The window itself is
/// [`mako_fristen::vorlauf::rechnung_antwort_spaetester_uet`] — it is anchored
/// on the Zahlungsziel the invoice carries and on the payer's Marktrolle, so
/// the workflow labels the deadline and `makod` dates it.
pub const SETTLEMENT_WINDOW_LABEL: &str = "wim-invoic-settlement-deadline";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimInvoicEvent {
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
        /// One of [`WIM_INVOIC_PIDS`] — 31009 (Strom), 31003 (Gas) or 31004.
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
    InvoiceSettled,
    /// Invoice disputed — negative CONTRL or APERAK was dispatched.
    InvoiceDisputed {
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

impl EventPayload for WimInvoicEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::InvoicReceived { .. } => "WimInvoicReceived",
            Self::InvoicSent { .. } => "WimInvoicSent",
            Self::Rejected { .. } => "WimInvoicRejected",
            Self::DeadlineExpired { .. } => "WimInvoicDeadlineExpired",
            Self::InvoiceSettled => "WimInvoicSettled",
            Self::InvoiceDisputed { .. } => "WimInvoicDisputed",
            Self::RemadvReceived { .. } => "WimInvoicRemadvReceived",
            Self::ComdisAbLehnungReceived { .. } => "WimInvoicComdisAbLehnungReceived",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands accepted by the WiM billing workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimInvoicCommand {
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
        /// One of [`WIM_INVOIC_PIDS`] — 31009 (Strom), 31003 (Gas) or 31004.
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
    SettleInvoice,
    /// Dispute the invoice — a negative CONTRL / APERAK will be dispatched.
    DisputeInvoice {
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
    /// PIDs 33001–33004 (REMADV AHB 1.0a §3, WiM Strom Teil 1, BK6-22-024);
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
    /// COMDIS PID 29001 (Ablehnung REMADV, COMDIS AHB 1.0, WiM BK6-22-024).
    ReceiveComdis {
        /// EDIFACT message reference of the COMDIS.
        comdis_ref: MessageRef,
    },
}

impl CommandPayload for WimInvoicCommand {}

// ── Workflow state ─────────────────────────────────────────────────────────────

/// Internal state of the WiM billing workflow.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum WimInvoicState {
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
pub struct WimInvoicWorkflow;

impl Workflow for WimInvoicWorkflow {
    type Command = WimInvoicCommand;
    type Event = WimInvoicEvent;
    type State = WimInvoicState;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            WimInvoicEvent::InvoicReceived {
                invoice_ref,
                pruefidentifikator,
                rechnung,
                ..
            } => WimInvoicState::PendingSettlement {
                invoice_ref: invoice_ref.clone(),
                pruefidentifikator: *pruefidentifikator,
                rechnung: rechnung.clone(),
            },
            WimInvoicEvent::InvoicSent {
                invoice_ref,
                pruefidentifikator,
                ..
            } => WimInvoicState::InvoicSent {
                invoice_ref: invoice_ref.clone(),
                pruefidentifikator: *pruefidentifikator,
            },
            WimInvoicEvent::Rejected { reason } => WimInvoicState::Rejected {
                reason: reason.clone(),
            },
            WimInvoicEvent::InvoiceSettled => WimInvoicState::Settled,
            WimInvoicEvent::InvoiceDisputed { reason } => WimInvoicState::Disputed {
                reason: reason.clone(),
            },
            WimInvoicEvent::DeadlineExpired { label, .. } => match state {
                // Terminal states — do not overwrite with deadline expiry.
                WimInvoicState::Settled
                | WimInvoicState::Disputed { .. }
                | WimInvoicState::Rejected { .. }
                | WimInvoicState::PaymentConfirmed
                | WimInvoicState::PaymentDisputed { .. }
                | WimInvoicState::ComdisRejected => state,
                _ => WimInvoicState::Rejected {
                    reason: format!("settlement deadline expired: {label}"),
                },
            },
            WimInvoicEvent::RemadvReceived {
                pid, is_confirmed, ..
            } => {
                if *is_confirmed {
                    WimInvoicState::PaymentConfirmed
                } else {
                    WimInvoicState::PaymentDisputed { remadv_pid: *pid }
                }
            }
            WimInvoicEvent::ComdisAbLehnungReceived { .. } => WimInvoicState::ComdisRejected,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            WimInvoicCommand::ReceiveInvoic {
                invoice_ref,
                sender,
                recipient,
                document_date,
                pruefidentifikator,
                validation_passed,
                validation_errors,
                rechnung,
            } => {
                if !matches!(state, WimInvoicState::New) {
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
                        vec![WimInvoicEvent::InvoicReceived {
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
                    Ok(WorkflowOutput::events(vec![WimInvoicEvent::Rejected {
                        reason: validation_errors.join("; "),
                    }]))
                }
            }

            WimInvoicCommand::SendInvoic {
                pid,
                sender,
                recipient,
                document_date,
                invoice_ref,
            } => {
                if !matches!(state, WimInvoicState::New) {
                    return Err(WorkflowError::invalid_state("New", format!("{state:?}")));
                }
                if !WIM_INVOIC_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a WiM INVOIC PID (31009), got {pid}"
                    )));
                }
                Ok(WorkflowOutput::events(vec![WimInvoicEvent::InvoicSent {
                    pruefidentifikator: pid,
                    sender,
                    recipient,
                    document_date,
                    invoice_ref,
                }]))
            }

            WimInvoicCommand::SettleInvoice => {
                if !matches!(state, WimInvoicState::PendingSettlement { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                let (pid, invoice_ref) = match &state {
                    WimInvoicState::PendingSettlement {
                        pruefidentifikator,
                        invoice_ref,
                        ..
                    } => (pruefidentifikator.as_u32(), invoice_ref.to_string()),
                    _ => (0, String::new()),
                };
                Ok(WorkflowOutput::with_outbox(
                    vec![WimInvoicEvent::InvoiceSettled],
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

            WimInvoicCommand::DisputeInvoice { reason } => {
                if !matches!(state, WimInvoicState::PendingSettlement { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                let (pid, invoice_ref) = match &state {
                    WimInvoicState::PendingSettlement {
                        pruefidentifikator,
                        invoice_ref,
                        ..
                    } => (pruefidentifikator.as_u32(), invoice_ref.to_string()),
                    _ => (0, String::new()),
                };
                Ok(WorkflowOutput::with_outbox(
                    vec![WimInvoicEvent::InvoiceDisputed {
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

            WimInvoicCommand::TimeoutExpired { deadline_id, label } => {
                if !matches!(state, WimInvoicState::PendingSettlement { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                Ok(WorkflowOutput::events(vec![
                    WimInvoicEvent::DeadlineExpired { deadline_id, label },
                ]))
            }

            WimInvoicCommand::ReceiveRemadv {
                pid,
                remadv_ref,
                sender,
            } => {
                if !matches!(
                    state,
                    WimInvoicState::Settled
                        | WimInvoicState::PendingSettlement { .. }
                        | WimInvoicState::InvoicSent { .. }
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
                    WimInvoicEvent::RemadvReceived {
                        pid,
                        remadv_ref,
                        sender,
                        is_confirmed,
                    },
                ]))
            }

            WimInvoicCommand::ReceiveComdis { comdis_ref } => {
                if matches!(
                    state,
                    WimInvoicState::New
                        | WimInvoicState::Rejected { .. }
                        | WimInvoicState::ComdisRejected
                ) {
                    return Err(WorkflowError::invalid_state(
                        "Settled|PendingSettlement",
                        format!("{state:?}"),
                    ));
                }
                Ok(WorkflowOutput::events(vec![
                    WimInvoicEvent::ComdisAbLehnungReceived { comdis_ref },
                ]))
            }
        }
    }
}

// ── Read-model projection ──────────────────────────────────────────────────────

/// Read-model record for a single WiM Strom INVOIC billing process stream.
#[derive(Debug)]
pub struct WimInvoicRecord {
    /// Current lifecycle status label.
    pub status: &'static str,
    /// BDEW Prüfidentifikator once the INVOIC is received.
    pub pruefidentifikator: Option<Pruefidentifikator>,
    /// Total events processed for this stream.
    pub event_count: usize,
}

impl Default for WimInvoicRecord {
    fn default() -> Self {
        Self {
            status: "New",
            pruefidentifikator: None,
            event_count: 0,
        }
    }
}

/// In-process read model tracking all WiM Strom INVOIC billing process streams.
#[derive(Debug, Default)]
pub struct WimInvoicProjection {
    /// All known billing process records keyed by stream ID.
    pub records: HashMap<String, WimInvoicRecord>,
    /// Sequence number of the last event applied.
    pub last_seq: u64,
}

impl Projection for WimInvoicProjection {
    fn name(&self) -> &'static str {
        "WimInvoicProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        let Ok(event) = envelope.decode::<WimInvoicEvent>() else {
            return;
        };

        match event {
            WimInvoicEvent::InvoicReceived {
                pruefidentifikator, ..
            } => {
                record.status = "PendingSettlement";
                record.pruefidentifikator = Some(pruefidentifikator);
            }
            WimInvoicEvent::InvoicSent {
                pruefidentifikator, ..
            } => {
                record.status = "InvoicSent";
                record.pruefidentifikator = Some(pruefidentifikator);
            }
            WimInvoicEvent::InvoiceSettled => {
                record.status = "Settled";
            }
            WimInvoicEvent::InvoiceDisputed { .. } => {
                record.status = "Disputed";
            }
            WimInvoicEvent::Rejected { .. } | WimInvoicEvent::DeadlineExpired { .. } => {
                record.status = "Rejected";
            }
            WimInvoicEvent::RemadvReceived { is_confirmed, .. } => {
                record.status = if is_confirmed {
                    "PaymentConfirmed"
                } else {
                    "PaymentDisputed"
                };
            }
            WimInvoicEvent::ComdisAbLehnungReceived { .. } => {
                record.status = "ComdisRejected";
            }
        }
    }

    fn last_sequence(&self) -> Option<u64> {
        if self.last_seq == 0 {
            None
        } else {
            Some(self.last_seq)
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

    fn send_cmd(p: u32) -> WimInvoicCommand {
        WimInvoicCommand::SendInvoic {
            pid: pid(p),
            sender: mp("9900123456789"),    // MSB (invoicer)
            recipient: mp("9900987654321"), // NB/LF/ESA (payer)
            document_date: "20260731".into(),
            invoice_ref: MessageRef::new("MSB-RE-001"),
        }
    }

    fn remadv_cmd(p: u32) -> WimInvoicCommand {
        WimInvoicCommand::ReceiveRemadv {
            pid: pid(p),
            remadv_ref: MessageRef::new("REMADV-001"),
            sender: mp("9900987654321"),
        }
    }

    // ── SendInvoic (MSB invoicer side) ──────────────────────────────────────────

    #[test]
    fn send_invoic_31009_from_new_emits_invoic_sent() {
        let out = WimInvoicWorkflow::handle(&WimInvoicState::New, send_cmd(31009))
            .expect("valid 31009 send must succeed");
        assert_eq!(out.events.len(), 1);
        assert!(matches!(out.events[0], WimInvoicEvent::InvoicSent { .. }));
        // State transitions to InvoicSent (awaiting REMADV).
        let state = WimInvoicWorkflow::apply(WimInvoicState::New, &out.events[0]);
        assert!(matches!(state, WimInvoicState::InvoicSent { .. }));
    }

    /// The WiM-Rechnung in both Sparten, and the Sparte-neutral Storno, all run
    /// this workflow: 31009 (Strom, WiM Teil 1 Kap. 3.6/4), 31003 (Gas, AWH WiM
    /// Gas 2.0 Kap. 4.7) and 31004 (Stornorechnung, INVOIC AHB §3.1.2).
    #[test]
    fn send_invoic_accepts_every_wim_rechnung_pid() {
        for pid in [31_009_u32, 31_003, 31_004] {
            let out = WimInvoicWorkflow::handle(&WimInvoicState::New, send_cmd(pid))
                .unwrap_or_else(|e| panic!("PID {pid} must be accepted: {e}"));
            assert!(matches!(out.events[0], WimInvoicEvent::InvoicSent { .. }));
        }
    }

    /// A PID from another family never opens a WiM billing process. 31001 is
    /// the Abschlagsrechnung and 31002 the NN-Rechnung — both GPKE.
    #[test]
    fn send_invoic_rejects_a_foreign_invoice_pid() {
        for pid in [31_001_u32, 31_002] {
            let err = WimInvoicWorkflow::handle(&WimInvoicState::New, send_cmd(pid))
                .unwrap_err_or_else_msg(pid);
            assert!(err.contains("31009"), "PID {pid}: {err}");
        }
    }

    trait UnwrapErrMsg {
        fn unwrap_err_or_else_msg(self, pid: u32) -> String;
    }
    impl<T: std::fmt::Debug> UnwrapErrMsg for Result<T, mako_engine::error::WorkflowError> {
        fn unwrap_err_or_else_msg(self, pid: u32) -> String {
            match self {
                Err(e) => e.to_string(),
                Ok(v) => panic!("PID {pid} must be rejected, got {v:?}"),
            }
        }
    }

    #[test]
    fn send_invoic_from_wrong_state_errors() {
        let state = WimInvoicState::Settled;
        let err = WimInvoicWorkflow::handle(&state, send_cmd(31009))
            .expect_err("send from a non-New state must be rejected");
        assert!(format!("{err}").contains("New"));
    }

    // ── REMADV after send (itemized Strom rejections 33003/33004) ───────────────

    #[test]
    fn remadv_33001_confirms_payment_after_send() {
        let state = WimInvoicState::InvoicSent {
            invoice_ref: MessageRef::new("MSB-RE-001"),
            pruefidentifikator: pid(31009),
        };
        let out = WimInvoicWorkflow::handle(&state, remadv_cmd(33001))
            .expect("33001 REMADV must be accepted after send");
        let new_state = WimInvoicWorkflow::apply(state, &out.events[0]);
        assert!(matches!(new_state, WimInvoicState::PaymentConfirmed));
    }

    #[test]
    fn remadv_33003_itemized_kopf_summe_rejection_after_send() {
        let state = WimInvoicState::InvoicSent {
            invoice_ref: MessageRef::new("MSB-RE-001"),
            pruefidentifikator: pid(31009),
        };
        let out = WimInvoicWorkflow::handle(&state, remadv_cmd(33003))
            .expect("33003 itemized rejection must be accepted");
        assert!(matches!(
            out.events[0],
            WimInvoicEvent::RemadvReceived {
                is_confirmed: false,
                ..
            }
        ));
        let new_state = WimInvoicWorkflow::apply(state, &out.events[0]);
        match new_state {
            WimInvoicState::PaymentDisputed { remadv_pid } => {
                assert_eq!(remadv_pid.as_u32(), 33003);
            }
            other => panic!("expected PaymentDisputed(33003), got {other:?}"),
        }
    }

    #[test]
    fn remadv_33004_itemized_position_rejection_after_send() {
        let state = WimInvoicState::InvoicSent {
            invoice_ref: MessageRef::new("MSB-RE-001"),
            pruefidentifikator: pid(31009),
        };
        let out = WimInvoicWorkflow::handle(&state, remadv_cmd(33004))
            .expect("33004 itemized rejection must be accepted");
        let new_state = WimInvoicWorkflow::apply(state, &out.events[0]);
        assert!(matches!(
            new_state,
            WimInvoicState::PaymentDisputed { remadv_pid } if remadv_pid.as_u32() == 33004
        ));
    }

    #[test]
    fn remadv_unknown_pid_is_rejected() {
        let state = WimInvoicState::InvoicSent {
            invoice_ref: MessageRef::new("MSB-RE-001"),
            pruefidentifikator: pid(31009),
        };
        let err = WimInvoicWorkflow::handle(&state, remadv_cmd(33099))
            .expect_err("an out-of-range REMADV PID must be rejected");
        assert!(format!("{err}").contains("33001"));
    }

    // ── Projection ─────────────────────────────────────────────────────────────

    #[test]
    fn projection_defaults_to_new() {
        let proj = WimInvoicProjection::default();
        assert!(proj.records.is_empty());
        assert_eq!(proj.last_sequence(), None);
    }

    /// Build an envelope carrying `event` at `seq` on a fixed stream.
    fn envelope(event: &WimInvoicEvent, seq: u64) -> EventEnvelope {
        use mako_engine::ids::{ConversationId, CorrelationId, ProcessId, StreamId, TenantId};
        use mako_engine::version::WorkflowId;

        EventEnvelope::from_new(
            mako_engine::envelope::NewEvent::new(
                CorrelationId::new(),
                None,
                ConversationId::new(),
                ProcessId::new(),
                TenantId::new(),
                WorkflowId::new(WORKFLOW_NAME, "FV2026-04-01"),
                event.event_type(),
                1,
                serde_json::to_value(event).expect("event serialises"),
            ),
            StreamId::new("wim-invoic-test"),
            seq,
            time::OffsetDateTime::UNIX_EPOCH,
        )
    }

    /// Every event must drive the record to its documented status label.
    ///
    /// The REMADV arm is the one worth pinning: 33001 confirms payment while
    /// 33002/33003/33004 dispute it, so a projection that ignored
    /// `is_confirmed` would silently report every itemized Abweisung as paid.
    #[test]
    fn projection_maps_each_event_to_its_status() {
        let cases: Vec<(WimInvoicEvent, &str)> = vec![
            (
                WimInvoicEvent::InvoicSent {
                    pruefidentifikator: pid(31009),
                    sender: MarktpartnerCode::new("9900000000001"),
                    recipient: MarktpartnerCode::new("9900000000002"),
                    document_date: "20260401".to_owned(),
                    invoice_ref: MessageRef::new("MSB-RE-001"),
                },
                "InvoicSent",
            ),
            (WimInvoicEvent::InvoiceSettled, "Settled"),
            (
                WimInvoicEvent::InvoiceDisputed {
                    reason: "Preisabweichung".to_owned(),
                },
                "Disputed",
            ),
            (
                WimInvoicEvent::Rejected {
                    reason: "AHB".to_owned(),
                },
                "Rejected",
            ),
            (
                WimInvoicEvent::RemadvReceived {
                    pid: pid(33001),
                    remadv_ref: MessageRef::new("R-1"),
                    sender: MarktpartnerCode::new("9900000000002"),
                    is_confirmed: true,
                },
                "PaymentConfirmed",
            ),
            (
                WimInvoicEvent::RemadvReceived {
                    pid: pid(33004),
                    remadv_ref: MessageRef::new("R-2"),
                    sender: MarktpartnerCode::new("9900000000002"),
                    is_confirmed: false,
                },
                "PaymentDisputed",
            ),
            (
                WimInvoicEvent::ComdisAbLehnungReceived {
                    comdis_ref: MessageRef::new("C-1"),
                },
                "ComdisRejected",
            ),
        ];

        for (seq, (event, expected)) in cases.into_iter().enumerate() {
            let mut proj = WimInvoicProjection::default();
            let seq = seq as u64 + 1;
            proj.handle_event(&envelope(&event, seq));
            let record = proj
                .records
                .get("wim-invoic-test")
                .expect("record created for the stream");
            assert_eq!(record.status, expected, "event {event:?}");
            assert_eq!(record.event_count, 1);
            assert_eq!(proj.last_sequence(), Some(seq));
        }
    }
}

#[cfg(test)]
mod gas_ablehnung_tests {
    use super::*;

    /// Each of the five Gas refusal trees is reachable and publishes codes.
    #[test]
    fn every_gas_ablehnungsbaum_resolves_to_a_published_tree() {
        for a in [
            GasAblehnung::NbRechnung,
            GasAblehnung::MsbnRechnung,
            GasAblehnung::NbMesslokationsRechnung,
            GasAblehnung::NbStorno,
            GasAblehnung::MsbnStorno,
        ] {
            let ebd = gas_ablehnungs_ebd(a);
            let codes = mako_pruefung::codes::CODELISTEN
                .iter()
                .find(|(id, _)| *id == ebd)
                .map(|(_, codes)| *codes)
                .unwrap_or_else(|| panic!("{ebd} is registered in CODELISTEN"));
            assert!(!codes.is_empty(), "{ebd} publishes no codes");
            assert!(
                codes
                    .iter()
                    .all(|c| c.cluster == mako_pruefung::Cluster::Ablehnung),
                "{ebd} must publish Ablehnungscodes only — the Gas Zahlungsavis carries no AJT"
            );
            assert!(
                mako_pruefung::codes::wire_codeliste(ebd, mako_pruefung::Cluster::Ablehnung)
                    .is_some_and(|c| c.starts_with("G_")),
                "{ebd} must name a Gas Codeliste in DE 1082"
            );
        }
    }

    /// The NB's and the MSBN's trees are different trees, even though they
    /// spell the same alphabet.
    #[test]
    fn the_nb_and_the_msbn_refuse_from_different_trees() {
        assert_ne!(
            gas_ablehnungs_ebd(GasAblehnung::NbRechnung),
            gas_ablehnungs_ebd(GasAblehnung::MsbnRechnung)
        );
        assert_ne!(
            gas_ablehnungs_ebd(GasAblehnung::NbStorno),
            gas_ablehnungs_ebd(GasAblehnung::MsbnStorno)
        );
    }

    /// Only the Messlokations-Abrechnung names a Messlokation alone in code 14.
    #[test]
    fn code_14_names_the_marktlokation_except_on_the_melo_abrechnung() {
        let name_of = |a| {
            mako_pruefung::codes::lookup(gas_ablehnungs_ebd(a), "14")
                .expect("code 14 is published")
                .bedeutung
        };
        assert!(name_of(GasAblehnung::NbRechnung).contains("Marktlokation"));
        assert!(!name_of(GasAblehnung::NbMesslokationsRechnung).contains("Marktlokation"));
    }
}
