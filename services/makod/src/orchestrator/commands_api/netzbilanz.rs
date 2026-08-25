//! netzbilanzd NNE/MMM invoice generation (NB role, outbound INVOIC).
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

// ── netzbilanzd NNE/MMM invoice generation (NB role, outbound INVOIC) ─────────

/// Read a party MP-ID, preferring the role-neutral name over the legacy one.
fn party(payload: &serde_json::Value, preferred: &str, legacy: &str) -> String {
    payload
        .get(preferred)
        .or_else(|| payload.get(legacy))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned()
}

/// Helper: dispatch `InvoicCommand::SendInvoic` for a given PID.
///
/// Called by `invoic.nne.stellen` and `invoic.nne.stellen`
/// (both PID 31002, NN-Rechnung — the Sparte is carried in the payload, not the
/// PID) and `invoic.mmm.stellen` (PID 31005, MMM-Rechnung).
///
/// Spawns a new `GpkeAbrechnungWorkflow` in the invoicer role so that inbound
/// REMADV responses from the LF are routed back to the correct process.
///
/// The `invoice_ref` in the payload (set by `netzbilanzd`) becomes the business
/// key — it must match the `rechnungsnummer` in the Rechnung BO4E so the REMADV
/// correlation works.
pub(super) async fn dispatch_nne_send_invoic(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    pid: u32,
) -> Result<DispatchOutcome, DispatchError> {
    let invoice_ref_str = extract_invoice_ref(payload)?;
    // `sender_mp_id` / `recipient_mp_id` are what the settlement resolved and
    // what `netzbilanzd` stores; `nb_mp_id` / `lf_mp_id` are the older names and
    // stay accepted. Reading only the old pair left both parties empty, and an
    // INVOIC with no sender and no recipient is not a message anyone can route.
    let nb_mp_id = party(payload, "sender_mp_id", "nb_mp_id");
    let lf_mp_id = party(payload, "recipient_mp_id", "lf_mp_id");
    let invoice_ref_clone = invoice_ref_str.clone();
    dispatch_to_process::<GpkeAbrechnungWorkflow, _>(
        state,
        &invoice_ref_str,
        "gpke-abrechnung",
        move || InvoicCommand::SendInvoic {
            pid: mako_engine::types::Pruefidentifikator::new(pid).expect("valid NNE PID"),
            sender: mako_engine::types::MarktpartnerCode::new(nb_mp_id.as_str()),
            recipient: mako_engine::types::MarktpartnerCode::new(lf_mp_id.as_str()),
            invoice_ref: mako_engine::types::MessageRef::new(invoice_ref_clone.clone()),
            document_date: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Iso8601::DEFAULT)
                .unwrap_or_default(),
        },
    )
    .await
}

pub(super) fn cmd_invoic_nne_abschlag_stellen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_nne_send_invoic(s, p, 31001))
}

pub(super) fn cmd_invoic_nne_stellen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_nne_send_invoic(s, p, 31002))
}

pub(super) fn cmd_invoic_mmm_stellen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_nne_send_invoic(s, p, 31005))
}

/// Dispatch the WiM MSB-Rechnung (PID 31009) through the **dedicated** WiM billing
/// workflow ([`WimInvoicWorkflow::SendInvoic`]) — not `GpkeAbrechnungWorkflow`,
/// which rejects 31009 (its `GPKE_INVOIC_PIDS` are GPKE-only). Spawns the WiM process
/// in the MSB invoicer role so the payer's inbound REMADV (33001–33004) correlates
/// back to it. The billed EDIFACT with amounts is rendered by `netzbilanzd`; this
/// command records the process state.
async fn dispatch_wim_msb_send_invoic(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let invoice_ref_str = extract_invoice_ref(payload)?;
    // Sender = MSB invoicer, recipient = NB/LF/ESA payer. netzbilanzd supplies the
    // party MP-IDs; fall back to the legacy nb/lf fields for compatibility.
    let sender = payload
        .get("msb_mp_id")
        .or_else(|| payload.get("sender_mp_id"))
        .or_else(|| payload.get("nb_mp_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let recipient = payload
        .get("recipient_mp_id")
        .or_else(|| payload.get("lf_mp_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let invoice_ref_clone = invoice_ref_str.clone();
    dispatch_to_process::<WimInvoicWorkflow, _>(
        state,
        &invoice_ref_str,
        mako_wim::invoic::WORKFLOW_NAME,
        move || InvoicCommand::SendInvoic {
            pid: mako_engine::types::Pruefidentifikator::new(31009)
                .expect("valid MSB-Rechnung PID"),
            sender: mako_engine::types::MarktpartnerCode::new(sender.as_str()),
            recipient: mako_engine::types::MarktpartnerCode::new(recipient.as_str()),
            invoice_ref: mako_engine::types::MessageRef::new(invoice_ref_clone.clone()),
            document_date: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Iso8601::DEFAULT)
                .unwrap_or_default(),
        },
    )
    .await
}

/// WiM MSB-Rechnung dispatch (PID 31009 outbound). See [`dispatch_wim_msb_send_invoic`].
pub(super) fn cmd_wim_msb_rechnung_stellen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_msb_send_invoic(s, p))
}
