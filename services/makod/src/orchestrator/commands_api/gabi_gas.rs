//! GaBi Gas command wrappers and dispatchers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

/// Settle or dispute a GaBi Gas MMM-Rechnung INVOIC (PIDs 31007 / 31008).
///
/// Dispatched by `invoicd` after the plausibility check (including MMM Gas price
/// check 6 against Trading Hub Europe MMMA prices) completes.
/// Business key = `invoice_ref`.
pub(super) async fn dispatch_gabi_gas_invoic(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    settle: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let invoice_ref = extract_invoice_ref(payload)?;
    let reason = payload
        .get("ablehnungsgrund")
        .and_then(|v| v.as_str())
        .unwrap_or("Automatisch ermittelte Abweichung — GaBi Gas 31007/31008")
        .to_owned();
    dispatch_to_process::<GaBiGasInvoicWorkflow, _>(
        state,
        &invoice_ref,
        GABI_GAS_INVOIC_WORKFLOW_NAME,
        move || {
            if settle {
                GaBiGasInvoicCommand::SettleInvoice
            } else {
                GaBiGasInvoicCommand::DisputeInvoice {
                    reason: reason.clone(),
                }
            }
        },
    )
    .await
}

pub(super) fn cmd_gabi_gas_mmm_rechnung_annehmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gabi_gas_invoic(s, p, true))
}

pub(super) fn cmd_gabi_gas_mmm_rechnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gabi_gas_invoic(s, p, false))
}
