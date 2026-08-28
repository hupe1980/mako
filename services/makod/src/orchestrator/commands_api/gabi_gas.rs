//! GaBi Gas command wrappers and dispatchers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

/// Settle or dispute a GaBi Gas billing INVOIC — the whole family, PIDs 31007,
/// 31008 and 31010.
///
/// One workflow answers all three, so one command pair does too. `invoicd`
/// dispatches it for 31007/31008 after the plausibility check (including MMM
/// Gas price check 6 against Trading Hub Europe MMMA prices); the
/// Kapazitätsrechnung 31010 has no price basis to check against and is answered
/// by an operator through the same command.
///
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
        .unwrap_or("Automatisch ermittelte Abweichung — GaBi Gas Rechnung")
        .to_owned();
    let message_ref = remadv_message_ref(payload);
    let antwort = remadv_antwort(payload);
    dispatch_to_process::<GaBiGasInvoicWorkflow, _>(
        state,
        &invoice_ref,
        GABI_GAS_INVOIC_WORKFLOW_NAME,
        move || {
            if settle {
                InvoicCommand::SettleInvoice {
                    message_ref: message_ref.clone(),
                }
            } else {
                InvoicCommand::DisputeInvoice {
                    message_ref: message_ref.clone(),
                    reason: reason.clone(),
                    antwort: antwort.clone(),
                }
            }
        },
    )
    .await
}

pub(super) fn cmd_gabi_gas_rechnung_annehmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gabi_gas_invoic(s, p, true))
}

pub(super) fn cmd_gabi_gas_rechnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gabi_gas_invoic(s, p, false))
}
