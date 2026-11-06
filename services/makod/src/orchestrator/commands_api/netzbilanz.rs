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

/// Issue an NB invoice: `invoic.nne-abschlag.stellen` (PID 31001),
/// `invoic.nne.stellen` (31002, NN-Rechnung Strom and Gas — the Sparte is
/// carried in the `rechnung`, not the PID) and `invoic.mmm.stellen` (31005).
///
/// Builds the INVOIC from the payload's `rechnung` and spawns the
/// `gpke-abrechnung` process in the invoicer role, keyed by `invoice_ref` —
/// which `netzbilanzd` sets to the `rechnungsnummer` — so the Lieferant's
/// REMADV correlates back to it ([`super::invoic_stellen`]).
pub(super) async fn dispatch_nne_send_invoic(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    pid: u32,
) -> Result<DispatchOutcome, DispatchError> {
    // `sender_mp_id` / `recipient_mp_id` are what the settlement resolved and
    // what `netzbilanzd` stores; `nb_mp_id` / `lf_mp_id` are the older names and
    // stay accepted. Reading only the old pair left both parties empty, and an
    // INVOIC with no sender and no recipient is not a message anyone can route.
    let nb_mp_id = party(payload, "sender_mp_id", "nb_mp_id");
    let lf_mp_id = party(payload, "recipient_mp_id", "lf_mp_id");
    super::e_rechnung::edi_pruefen(state.marktd_client.as_deref(), payload, &lf_mp_id).await?;
    super::invoic_stellen::invoic_stellen::<GpkeAbrechnungWorkflow>(
        state,
        payload,
        super::invoic_stellen::Ausstellung {
            workflow_name: "gpke-abrechnung",
            pid,
            sender: &nb_mp_id,
            recipient: &lf_mp_id,
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
    // The Mindermenge is the 31005, the Mehrmenge the NB's selbst ausgestellte
    // 31006 (INVOIC AHB 1.0b Kap. 3, S. 5) — which one the Rechnung says.
    let selbst = p
        .get("rechnung")
        .and_then(|r| r.get("netznutzungrechnungsart"))
        .and_then(|v| v.as_str())
        == Some("SELBSTAUSGESTELLT");
    Box::pin(dispatch_nne_send_invoic(
        s,
        p,
        if selbst { 31006 } else { 31005 },
    ))
}

/// Issue the WiM MSB-Rechnung (PID 31009) through the **dedicated** WiM billing
/// workflow ([`WimInvoicWorkflow`]) — not `GpkeAbrechnungWorkflow`, which
/// rejects 31009 (its `GPKE_INVOIC_PIDS` are GPKE-only). Spawns the WiM process
/// in the MSB invoicer role so the payer's inbound REMADV (33001–33004)
/// correlates back to it ([`super::invoic_stellen`]).
async fn dispatch_wim_msb_send_invoic(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
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
    super::e_rechnung::edi_pruefen(state.marktd_client.as_deref(), payload, &recipient).await?;
    super::invoic_stellen::invoic_stellen::<WimInvoicWorkflow>(
        state,
        payload,
        super::invoic_stellen::Ausstellung {
            workflow_name: mako_wim::invoic::WORKFLOW_NAME,
            pid: 31009,
            sender: &sender,
            recipient: &recipient,
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
