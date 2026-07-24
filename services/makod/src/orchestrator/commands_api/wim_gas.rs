//! WiM Gas command wrappers and dispatchers (Anmeldung, APERAK, INVOIC).
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

// ── WiM Gas Anmeldung ─────────────────────────────────────────────────────────

pub(super) fn cmd_wim_gas_anmeldung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_aperak(
        s,
        p,
        WIM_GAS_ANMELDUNG_WORKFLOW_NAME,
        true,
    ))
}

pub(super) fn cmd_wim_gas_anmeldung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_aperak(
        s,
        p,
        WIM_GAS_ANMELDUNG_WORKFLOW_NAME,
        false,
    ))
}

pub(super) fn cmd_wim_gas_kuendigung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_aperak(
        s,
        p,
        WIM_GAS_KUENDIGUNG_WORKFLOW_NAME,
        true,
    ))
}

pub(super) fn cmd_wim_gas_kuendigung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_aperak(
        s,
        p,
        WIM_GAS_KUENDIGUNG_WORKFLOW_NAME,
        false,
    ))
}

pub(super) fn cmd_wim_gas_stornierung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_stornierung_aperak(s, p, true))
}

pub(super) fn cmd_wim_gas_stornierung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_stornierung_aperak(s, p, false))
}

// ── WiM Gas APERAK helpers ────────────────────────────────────────────────────

/// Dispatch `WimGasAnmeldungCommand::DispatchAperak` or
/// `WimGasKuendigungCommand::DispatchAperak` to an existing WiM Gas process.
///
/// Both workflows use `malo_id` as the registry business key.
/// `workflow_name` selects whether to route to `WimGasAnmeldungWorkflow` or
/// `WimGasKuendigungWorkflow`.
pub(super) async fn dispatch_wim_gas_aperak(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    workflow_name: &'static str,
    positive: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?.to_string();
    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    match workflow_name {
        WIM_GAS_ANMELDUNG_WORKFLOW_NAME => {
            dispatch_to_process::<WimGasAnmeldungWorkflow, _>(
                state,
                malo_id.as_str(),
                WIM_GAS_ANMELDUNG_WORKFLOW_NAME,
                move || WimGasAnmeldungCommand::DispatchAperak {
                    positive,
                    reason: reason.clone(),
                },
            )
            .await
        }
        WIM_GAS_KUENDIGUNG_WORKFLOW_NAME => {
            dispatch_to_process::<WimGasKuendigungWorkflow, _>(
                state,
                malo_id.as_str(),
                WIM_GAS_KUENDIGUNG_WORKFLOW_NAME,
                move || WimGasKuendigungCommand::DispatchAperak {
                    positive,
                    reason: reason.clone(),
                },
            )
            .await
        }
        _ => Err(DispatchError::NotImplemented(workflow_name.to_owned())),
    }
}

/// Dispatch `WimGasStornierungCommand::DispatchAperak` to an existing
/// `WimGasStornierungWorkflow` process looked up by `vorgang_id`.
///
/// The `vorgang_id` is the Vorgangsnummer from the original PID 44022 message
/// (IDE+24 segment). Pass it as `vorgang_id` in the ERP payload.
pub(super) async fn dispatch_wim_gas_stornierung_aperak(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    positive: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let vorgang_id = payload
        .get("vorgang_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"vorgang_id\" (Vorgangsnummer from the PID 44022 message)"
                    .into(),
            )
        })?
        .to_owned();

    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    dispatch_to_process::<WimGasStornierungWorkflow, _>(
        state,
        &vorgang_id,
        WIM_GAS_STORNIERUNG_WORKFLOW_NAME,
        move || WimGasStornierungCommand::DispatchAperak {
            positive,
            reason: reason.clone(),
        },
    )
    .await
}
// ── WiM Gas Invoic dispatch functions ─────────────────────────────────────────

/// Settle or dispute a WiM Gas INVOIC (PIDs 31003 / 31004 Stornorechnung).
///
/// Dispatched by `invoicd` after the plausibility check completes.
/// Business key = `invoice_ref` (EDIFACT message reference from the inbound INVOIC).
pub(super) async fn dispatch_wim_gas_invoic(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    settle: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let invoice_ref = extract_invoice_ref(payload)?;
    let reason = payload
        .get("ablehnungsgrund")
        .and_then(|v| v.as_str())
        .unwrap_or("Automatisch ermittelte Abweichung — WiM Gas 31003/31004")
        .to_owned();
    dispatch_to_process::<WimGasInvoicWorkflow, _>(
        state,
        &invoice_ref,
        WIM_GAS_INVOIC_WORKFLOW_NAME,
        move || {
            if settle {
                WimGasInvoicCommand::SettleInvoice
            } else {
                WimGasInvoicCommand::DisputeInvoice {
                    reason: reason.clone(),
                }
            }
        },
    )
    .await
}

pub(super) fn cmd_wim_gas_rechnung_annehmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_invoic(s, p, true))
}

pub(super) fn cmd_wim_gas_rechnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_invoic(s, p, false))
}

pub(super) fn cmd_wim_gas_stornorechnung_annehmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_invoic(s, p, true))
}

pub(super) fn cmd_wim_gas_stornorechnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gas_invoic(s, p, false))
}
