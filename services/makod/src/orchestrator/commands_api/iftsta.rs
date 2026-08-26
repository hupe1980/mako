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
/// - `sender_mp_id`    — Sender party MP-ID
/// - `receiver_mp_id`  — Receiver party MP-ID
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
/// Constructs [`DeviceChangeCommand::ReceiveInformation`] and executes it on the
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
        move || DeviceChangeCommand::ReceiveInformation {
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
/// Both carry an inbound MaBiS IFTSTA over ERP REST instead of AS4.
///
/// | Command | PIDs accepted | Required payload |
/// |---|---|---|
/// | `mabis.datenstatus.empfangen` | 21003, 21004 | `datenstatus` |
/// | `mabis.iftsta.empfangen` | 21002 | `abweisungsgrund` |
///
/// **Both Datenstatus PIDs are accepted**, and which one applies follows from
/// the participant's role: the BIKO sends 21003 to an NB or ÜNB and 21004 to a
/// BKV. Forcing the PID to 21004 — as this handler used to — silently relabels
/// every Datenstatus an NB receives.
///
/// 21000, 21001 and 21005 are refused: they are this participant's **own
/// outbound** Prüfmitteilungen, so accepting one as an arrival would record a
/// check nobody performed.
///
/// Expected payload fields: `stream_id`, `pid`, `version`, `message_ref`, plus
/// `datenstatus` or `abweisungsgrund` per the table above.
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

    let pid_code = payload
        .get("pid")
        .and_then(|v| v.as_u64())
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        .ok_or_else(|| DispatchError::InvalidPayload("\"pid\" (u32) is required".into()))?;

    if MABIS_IFTSTA_PRUEFMITTEILUNG_PIDS.contains(&pid_code) {
        return Err(DispatchError::InvalidPayload(format!(
            "pid {pid_code} is an outbound Prüfmitteilung of this participant and cannot \
             be recorded as an arrival"
        )));
    }
    let erwartet_datenstatus = MABIS_IFTSTA_DATENSTATUS_PIDS.contains(&pid_code);
    if erwartet_datenstatus != is_datenstatus {
        return Err(DispatchError::InvalidPayload(if is_datenstatus {
            format!(
                "pid {pid_code} carries no Datenstatus; \
                 valid: {MABIS_IFTSTA_DATENSTATUS_PIDS:?}"
            )
        } else {
            format!("pid {pid_code} carries a Datenstatus — use mabis.datenstatus.empfangen")
        }));
    }
    if !erwartet_datenstatus && pid_code != MABIS_IFTSTA_ABWEISUNG_PID {
        return Err(DispatchError::InvalidPayload(format!(
            "pid {pid_code} is not an inbound MaBiS IFTSTA; \
             valid: {MABIS_IFTSTA_DATENSTATUS_PIDS:?} or {MABIS_IFTSTA_ABWEISUNG_PID}"
        )));
    }

    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;

    // The version of a Summenzeitreihe is its Erstellungszeitpunkt, carried in
    // IFTSTA `SG4 RFF+AUU` (17 characters). It is the key both ends match on,
    // so it is required and validated rather than defaulted.
    let version = payload
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "\"version\" (Erstellungszeitpunkt, RFF+AUU) is required".into(),
            )
        })
        .and_then(|v| {
            SzrVersion::new(v).map_err(|e| DispatchError::InvalidPayload(e.to_string()))
        })?;

    let message_ref = MessageRef::new(
        payload
            .get("message_ref")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );

    let (datenstatus, abweisungsgrund) = if erwartet_datenstatus {
        let code = payload
            .get("datenstatus")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                DispatchError::InvalidPayload(
                    "\"datenstatus\" is required for mabis.datenstatus.empfangen".into(),
                )
            })?;
        // The STS+Z04 codelist (A01/A02/A03/A04/A06) is accepted verbatim, and
        // so are the snake_case names, so an ERP can send either.
        let ds = Datenstatus::from_code(code)
            .or(match code {
                "pruefdaten" => Some(Datenstatus::Pruefdaten),
                "abrechnungsdaten" => Some(Datenstatus::Abrechnungsdaten),
                "abrechnungsdaten_kbka" => Some(Datenstatus::AbrechnungsdatenKbka),
                "abgerechnete_daten" => Some(Datenstatus::AbgerechneteDaten),
                "abgerechnete_daten_kbka" => Some(Datenstatus::AbgerechneteDatenKbka),
                _ => None,
            })
            .ok_or_else(|| {
                DispatchError::InvalidPayload(format!(
                    "unknown datenstatus \"{code}\"; valid: A01/A02/A03/A04/A06 or \
                     pruefdaten, abrechnungsdaten, abrechnungsdaten_kbka, \
                     abgerechnete_daten, abgerechnete_daten_kbka"
                ))
            })?;
        (Some(ds), None)
    } else {
        let grund = payload
            .get("abweisungsgrund")
            .and_then(|v| v.as_str())
            .filter(|g| !g.trim().is_empty())
            .ok_or_else(|| {
                DispatchError::InvalidPayload(
                    "\"abweisungsgrund\" is required for an Abweisung (PID 21002)".into(),
                )
            })?
            .to_owned();
        (None, Some(grund))
    };

    dispatch_to_process::<MabisBillingWorkflow, _>(state, &stream_id, "mabis-billing", move || {
        BillingCommand::ReceiveIftsta {
            pid,
            version,
            datenstatus,
            abweisungsgrund,
            message_ref,
        }
    })
    .await
}
