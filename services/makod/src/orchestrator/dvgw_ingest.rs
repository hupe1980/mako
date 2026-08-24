//! DVGW gas-transport ingest.
//!
//! Both families arrive over the same transports and route through the same
//! [`PidRouter`] — DVGW allocates Prüfidentifikatoren from 70000–79999 and BDEW
//! does not — but they need different parsers: a DVGW message rides `ORDERS` or
//! `ORDRSP`, both of which are also real BDEW message types, so the BDEW parser
//! accepts an ALOCAT as a well-formed `ORDRSP`.
//!
//! [`try_ingest`] is the first thing every ingest path calls. It sniffs `BGM`
//! DE 1001 — the only field that separates the families — and either handles the
//! interchange or hands the bytes back untouched.
//!
//! [`PidRouter`]: mako_engine::pid_router::PidRouter

use dvgw_edi::{DvgwDocument, DvgwPlatform};

use crate::api::edifact_api::EdifactApiState;
use crate::ingest_dispatcher::IngestOutcome;
use mako_engine::dead_letter::{AuditContext, DeadLetterReason};

/// What happened to one DVGW message.
#[derive(Debug, Clone)]
pub struct DvgwMessageResult {
    /// `BGM` DE 1001 — the document-name code that identified the message.
    ///
    /// `None` when the message could not be parsed far enough to read it.
    pub document: Option<DvgwDocument>,
    /// `SG1 RFF+Z13` Prüfidentifikator, when the message carried a valid one.
    pub pruefidentifikator: Option<u32>,
    /// The workflow the Prüfidentifikator routed to.
    pub workflow: Option<String>,
    /// The process the message reached, when it reached one.
    pub process_id: Option<String>,
    /// Why the message did not reach a process.
    pub skipped: Option<&'static str>,
    /// A hard failure — a parse error, or a workflow that rejected the command.
    pub error: Option<String>,
}

/// The outcome of a DVGW interchange.
#[derive(Debug, Clone, Default)]
pub struct DvgwIngestReport {
    /// One entry per `UNH`…`UNT` window, in wire order.
    pub messages: Vec<DvgwMessageResult>,
    /// `NAD+MS` of the first message that carried one.
    ///
    /// The CONTRL Empfangsbestätigung is addressed to it. Kept on the report so
    /// the transport can discharge that obligation without re-parsing.
    pub sender_mp_id: Option<String>,
    /// The UNB DE 0010 recipient — the own MP-ID the interchange was sent to.
    pub recipient_mp_id: String,
    /// The UNB DE 0020 Datenaustauschreferenz, which a CONTRL acknowledges.
    pub interchange_ref: String,
}

impl DvgwIngestReport {
    /// How many messages were accepted.
    ///
    /// Matches the BDEW path's table: a message that parsed and carries a
    /// Prüfidentifikator counts even when no workflow claimed it, because that
    /// case is dead-lettered and auditable. Only a parse failure or a missing
    /// Prüfidentifikator is a rejection — neither leaves anything routable.
    #[must_use]
    pub fn accepted(&self) -> usize {
        self.messages.len() - self.rejected()
    }

    /// How many messages could not be accepted at all.
    #[must_use]
    pub fn rejected(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.error.is_some() || m.pruefidentifikator.is_none())
            .count()
    }
}

/// The UNB DE 0010 recipient MP-ID and DE 0020 control reference, read from the
/// head of the interchange.
///
/// Only reached once the sniff has claimed the bytes, so it is on the DVGW path
/// alone — which is what keeps a BDEW interchange paying for the sniff and
/// nothing else.
fn interchange_envelope(body: &[u8]) -> (String, String) {
    edifact_rs::from_bytes_owned_with_config(body, edifact_rs::ReaderConfig::default())
        .take_while(Result::is_ok)
        .map_while(Result::ok)
        .take(2)
        .find(|s| s.tag == "UNB")
        .map(|unb| {
            (
                unb.component_str(2, 0).unwrap_or_default().to_owned(),
                unb.element_str(4).unwrap_or_default().to_owned(),
            )
        })
        .unwrap_or_default()
}

/// Handle `body` if it is a DVGW interchange; return `None` if it is not.
///
/// `None` means "not mine" — the caller continues with the BDEW path unchanged.
/// `Some` means the interchange was DVGW and every message in it has an entry in
/// the report, including the ones that failed.
pub async fn try_ingest(state: &EdifactApiState, body: &[u8]) -> Option<DvgwIngestReport> {
    // `sniff` reads `BGM` out of the head of the interchange and stops. Nothing
    // above this line may parse the body, or a BDEW interchange pays for a full
    // parse it does not use.
    dvgw_edi::sniff(body)?;

    // The recipient drives the same Sparte-aware workflow resolution the BDEW
    // path applies; the control reference is what a CONTRL acknowledges.
    let (recipient_mp_id, interchange_ref) = interchange_envelope(body);
    let interchange_ref_for_audit = interchange_ref.clone();

    let platform = DvgwPlatform::default();
    let mut report = DvgwIngestReport {
        recipient_mp_id: recipient_mp_id.clone(),
        interchange_ref,
        ..DvgwIngestReport::default()
    };

    for parsed in platform.parse_interchange(body) {
        let msg = match parsed {
            Ok(msg) => msg,
            Err(e) => {
                tracing::warn!(error = %e, "DVGW ingest: message parse error");
                // The sniff already claimed this interchange, so a failure here
                // is a defect in it rather than a signal to retry as BDEW.
                // Dead-lettered on the same terms as the BDEW path (§ 147 AO /
                // GoBD): the AS4 receipt still goes out, so an unrecorded
                // message is acknowledged and gone.
                state.dl_sink.reject(&DeadLetterReason::ProcessingError {
                    message: format!("dvgw_parse_error: {e}"),
                    context: AuditContext::now()
                        .with_message_ref(interchange_ref_for_audit.as_str())
                        .with_receiver_eic(recipient_mp_id.as_str())
                        .with_tenant_id(state.tenant_id.to_string()),
                });
                report.messages.push(DvgwMessageResult {
                    document: None,
                    pruefidentifikator: None,
                    workflow: None,
                    process_id: None,
                    skipped: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };

        // Conformance findings are recorded, not fatal: a message that violates
        // a `Muss` row still has to be auditable.
        let validation = DvgwPlatform::validate_message(&msg);
        if !validation.is_valid() {
            for issue in validation.errors() {
                tracing::warn!(
                    document = msg.document.code(),
                    pid = msg
                        .pruefidentifikator
                        .map(dvgw_edi::Pruefidentifikator::as_u32),
                    rule = issue.rule_id.unwrap_or("—"),
                    "DVGW ingest: conformance error: {}",
                    issue.message,
                );
            }
        }

        if report.sender_mp_id.is_none() {
            report.sender_mp_id = msg
                .sender()
                .map(|p| p.id.clone())
                .filter(|id| !id.is_empty());
        }

        let pid = msg
            .pruefidentifikator
            .map(dvgw_edi::Pruefidentifikator::as_u32);
        let workflow = pid
            .and_then(|p| state.resolve_workflow(p, &recipient_mp_id))
            .map(str::to_owned);

        let mut result = DvgwMessageResult {
            document: Some(msg.document),
            pruefidentifikator: pid,
            workflow: workflow.clone(),
            process_id: None,
            skipped: None,
            error: None,
        };

        // Unroutable messages are dead-lettered on the same path as BDEW ones
        // (§ 147 AO / GoBD): the message is just as lost, and a silent accept is
        // worse than a rejection because nothing signals it.
        let Some((pid, workflow)) = pid.zip(workflow) else {
            let mut ctx = AuditContext::now()
                .with_message_type(msg.document.code())
                .with_message_ref(msg.message_ref.clone())
                .with_tenant_id(state.tenant_id.to_string());
            if let Some(sender) = msg.sender() {
                ctx = ctx.with_sender_eic(sender.id.clone());
            }
            if let Some(receiver) = msg.receiver() {
                ctx = ctx.with_receiver_eic(receiver.id.clone());
            }
            let dead_pid = pid
                .and_then(mako_engine::ids::Pid::from_u32)
                .unwrap_or(mako_engine::ids::Pid::new(1));
            mako_engine::metrics::EngineMetrics::global()
                .inbound_received(dead_pid.as_u32(), "unknown_pid");
            state.dl_sink.reject(&DeadLetterReason::UnknownPid {
                pid: dead_pid,
                context: ctx,
            });
            result.skipped = Some(if pid.is_none() {
                "no_pruefidentifikator"
            } else {
                "unknown_pid"
            });
            report.messages.push(result);
            continue;
        };

        tracing::info!(
            document = msg.document.code(),
            message_type = %msg.message_type,
            pid,
            workflow = %workflow,
            gas_day = ?msg.gas_day(),
            "DVGW message received",
        );

        let Some(dispatcher) = state.dispatcher.as_deref() else {
            // Classification-only deployment (Phase 1) — same as the BDEW path.
            result.skipped = Some("no_dispatcher");
            report.messages.push(result);
            continue;
        };

        match dispatcher.dispatch_dvgw(&msg, &workflow, pid).await {
            Ok(
                IngestOutcome::Spawned { process_id, .. }
                | IngestOutcome::Dispatched { process_id, .. },
            ) => {
                result.process_id = Some(process_id.to_string());
            }
            Ok(IngestOutcome::Skipped { reason, .. }) => result.skipped = Some(reason),
            Err(e) => {
                tracing::warn!(pid, workflow = %workflow, error = %e, "DVGW ingest: dispatch failed");
                // A regulated message that did not land; the BDEW path
                // dead-letters the same case.
                state.dl_sink.reject(&DeadLetterReason::ProcessingError {
                    message: format!("dvgw_dispatch_failed: {e}"),
                    context: AuditContext::now()
                        .with_message_type(msg.document.code())
                        .with_message_ref(msg.message_ref.clone())
                        .with_receiver_eic(recipient_mp_id.as_str())
                        .with_tenant_id(state.tenant_id.to_string())
                        .with_pid(
                            mako_engine::ids::Pid::from_u32(pid)
                                .unwrap_or(mako_engine::ids::Pid::new(1)),
                        ),
                });
                result.error = Some(e.to_string());
            }
        }
        report.messages.push(result);
    }

    Some(report)
}
