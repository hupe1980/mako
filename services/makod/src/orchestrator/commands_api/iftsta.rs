//! IFTSTA REST command dispatch (GPKE / WiM / MaBiS Vollzugs- und Statusmeldungen).
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

pub(super) fn cmd_gpke_vollzugsmeldung_empfangen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_iftsta(s, p))
}

pub(super) fn cmd_wim_iftsta_empfangen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_iftsta(s, p))
}

pub(super) fn cmd_mabis_iftsta_empfangen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_mabis_iftsta(s, p, false))
}

pub(super) fn cmd_mabis_datenstatus_empfangen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_mabis_iftsta(s, p, true))
}

// ── IFTSTA REST command dispatch helpers ──────────────────────────────────────

/// Dispatch a `gpke.vollzugsmeldung.empfangen` REST command.
///
/// Constructs [`SupplierChangeCommand::ReceiveVollzugsmeldung`] and executes
/// it on the existing `gpke-supplier-change` process identified by
/// `stream_id` in the payload.
///
/// Expected payload fields:
/// - `stream_id`     — Process stream ID (UUID)
/// - `pid`           — IFTSTA Prüfidentifikator (21024–21033)
/// - `sender_mp_id`    — Sender party GLN
/// - `receiver_mp_id`  — Receiver party GLN
/// - `message_ref`   — EDIFACT message reference string
pub(super) async fn dispatch_gpke_iftsta(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let stream_id = payload
        .get("stream_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::InvalidPayload("\"stream_id\" is required".into()))?
        .to_owned();
    let pid_code = payload
        .get("pid")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .ok_or_else(|| DispatchError::InvalidPayload("\"pid\" (u32) is required".into()))?;
    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;
    let sender = MarktpartnerCode::new(
        payload
            .get("sender_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let receiver = MarktpartnerCode::new(
        payload
            .get("receiver_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let message_ref = MessageRef::new(
        payload
            .get("message_ref")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    dispatch_to_process::<GpkeSupplierChangeWorkflow, _>(
        state,
        &stream_id,
        "gpke-supplier-change",
        move || SupplierChangeCommand::ReceiveVollzugsmeldung {
            pid,
            sender,
            receiver,
            message_ref,
            // IFTSTA arrives via ERP REST, not AS4; no EDIFACT AHB profile applies.
            // The ERP operator is responsible for providing a conformant IFTSTA payload.
            validation_passed: true,
            validation_errors: vec![],
        },
    )
    .await
}

/// Dispatch a `wim.iftsta.empfangen` REST command.
///
/// Constructs [`DeviceChangeCommand::ReceiveIftsta`] and executes it on the
/// existing `wim-device-change` process identified by `stream_id` in the payload.
///
/// Expected payload fields: `stream_id`, `pid`, `sender_mp_id`, `receiver_mp_id`,
/// `message_ref`.
pub(super) async fn dispatch_wim_iftsta(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let stream_id = payload
        .get("stream_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::InvalidPayload("\"stream_id\" is required".into()))?
        .to_owned();
    let pid_code = payload
        .get("pid")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .ok_or_else(|| DispatchError::InvalidPayload("\"pid\" (u32) is required".into()))?;
    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;
    let sender = MarktpartnerCode::new(
        payload
            .get("sender_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let receiver = MarktpartnerCode::new(
        payload
            .get("receiver_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let message_ref = MessageRef::new(
        payload
            .get("message_ref")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    dispatch_to_process::<WimDeviceChangeWorkflow, _>(
        state,
        &stream_id,
        "wim-device-change",
        move || DeviceChangeCommand::ReceiveIftsta {
            pid,
            sender,
            receiver,
            message_ref,
            // IFTSTA arrives via ERP REST, not AS4; no EDIFACT AHB profile applies.
            // The ERP operator is responsible for providing a conformant IFTSTA payload.
            validation_passed: true,
            validation_errors: vec![],
        },
    )
    .await
}

/// Dispatch `mabis.iftsta.empfangen` or `mabis.datenstatus.empfangen`.
///
/// When `is_datenstatus = true` (`mabis.datenstatus.empfangen`), the payload
/// must include `data_status` (`"abrechnungsdaten"` / `"abgerechnete_daten"` /
/// `"abgerechnete_daten_kbka"`). The PID is forced to 21004.
///
/// When `is_datenstatus = false` (`mabis.iftsta.empfangen`), `data_status` is
/// `None` and `pid` must be a non-21004 MaBiS IFTSTA PID from
/// [`MABIS_IFTSTA_PIDS`].
///
/// Expected payload fields: `stream_id`, `pid` (optional for datenstatus),
/// `sender_mp_id`, `receiver_mp_id`, `message_ref`. For datenstatus: `data_status`.
pub(super) async fn dispatch_mabis_iftsta(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    is_datenstatus: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let stream_id = payload
        .get("stream_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::InvalidPayload("\"stream_id\" is required".into()))?
        .to_owned();
    let pid_code = if is_datenstatus {
        IFTSTA_DATENSTATUS_PID.as_u32()
    } else {
        let code = payload
            .get("pid")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .ok_or_else(|| DispatchError::InvalidPayload("\"pid\" (u32) is required".into()))?;
        if !MABIS_IFTSTA_PIDS.contains(&code) || code == IFTSTA_DATENSTATUS_PID.as_u32() {
            return Err(DispatchError::InvalidPayload(format!(
                "pid {code} is not a valid non-datenstatus MaBiS IFTSTA PID; \
                 use mabis.datenstatus.empfangen for PID {IFTSTA_DATENSTATUS_PID}"
            )));
        }
        code
    };
    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;
    let sender = MarktpartnerCode::new(
        payload
            .get("sender_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let receiver = MarktpartnerCode::new(
        payload
            .get("receiver_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let message_ref = MessageRef::new(
        payload
            .get("message_ref")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let data_status = if is_datenstatus {
        let code = payload
            .get("data_status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                DispatchError::InvalidPayload(
                    "\"data_status\" is required for mabis.datenstatus.empfangen".into(),
                )
            })?;
        let ds = match code {
            "abrechnungsdaten" => DataStatus::Abrechnungsdaten,
            "abgerechnete_daten" => DataStatus::AbgerechtneteDaten,
            "abgerechnete_daten_kbka" => DataStatus::AbgerechtneteDatenKbka,
            other => {
                return Err(DispatchError::InvalidPayload(format!(
                    "unknown data_status \"{other}\"; valid values: \
                     abrechnungsdaten, abgerechnete_daten, abgerechnete_daten_kbka"
                )));
            }
        };
        Some(ds)
    } else {
        None
    };
    dispatch_to_process::<MabisBillingWorkflow, _>(state, &stream_id, "mabis-billing", move || {
        BillingCommand::ReceiveIftsta {
            pid,
            sender,
            receiver,
            message_ref,
            // IFTSTA arrives via ERP REST, not AS4; no EDIFACT AHB profile applies.
            // The ERP operator is responsible for providing a conformant IFTSTA payload.
            validation_passed: true,
            validation_errors: vec![],
            data_status,
        }
    })
    .await
}
