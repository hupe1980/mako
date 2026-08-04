//! GeLi Gas command wrappers and dispatchers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

pub(super) fn cmd_geli_lieferbeginn_anmelden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_lf_anmeldung(s, p, 44001))
}

pub(super) fn cmd_geli_lieferende_anmelden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_lf_anmeldung(s, p, 44002))
}

pub(super) fn cmd_geli_lieferbeginn_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let pid = p
            .get("response_pid")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(44003);
        dispatch_geli_gas_antwort(s, p, true, pid).await
    })
}

pub(super) fn cmd_geli_lieferbeginn_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let pid = p
            .get("response_pid")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(44004);
        dispatch_geli_gas_antwort(s, p, false, pid).await
    })
}

pub(super) fn cmd_geli_lieferende_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let pid = p
            .get("response_pid")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(44005);
        dispatch_geli_gas_antwort(s, p, true, pid).await
    })
}

pub(super) fn cmd_geli_lieferende_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let pid = p
            .get("response_pid")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(44006);
        dispatch_geli_gas_antwort(s, p, false, pid).await
    })
}

pub(super) fn cmd_geli_gas_stornierung_initiieren<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_gas_stornierung_initiieren(s, p))
}

pub(super) fn cmd_geli_gas_datenabruf_anfragen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_gas_datenabruf_anfragen(s, p))
}

pub(super) fn cmd_geli_eog_anmelden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_eog_anmelden(s, p))
}

/// Dispatch `geli.gas.datenabruf.anfragen` — LF requests Gas quality data from NB.
///
/// Spawns a new [`GeliGasDatanabrufWorkflow`] and sends ORDERS 17103 outbound
/// to the GNB requesting Abrechnungsbrennwert and Zustandszahl.
/// 10-Werktage response deadline registered atomically (BK7-24-01-009).
///
/// ## Required payload fields
///
/// | Field     | Type   | Notes |
/// |-----------|--------|-------|
/// | `malo_id` | string | Gas Marktlokations-ID (11-digit EIC) |
pub(super) async fn dispatch_geli_gas_datenabruf_anfragen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;

    let malo_record = state
        .malo_cache
        .get(&state.tenant_id.to_string(), malo_id.as_str())
        .await
        .map_err(|e| DispatchError::Engine(mako_engine::error::EngineError::store(e.to_string())))?
        .ok_or_else(|| DispatchError::MaloNotFound(malo_id.to_string()))?;

    let gnb_mp_id = malo_record
        .data_market_location
        .data_market_location_network_operators
        .iter()
        .max_by_key(|p| (p.execution_time_until.is_none(), &p.execution_time_from))
        .map(|p| MarktpartnerCode::new(format!("{:013}", p.market_partner_id)))
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "MaLo {malo_id} has no network_operator — GNB GLN cannot be resolved",
            ))
        })?;

    let pid = Pruefidentifikator::new(17103).map_err(DispatchError::InvalidPayload)?;
    let message_ref = MessageRef::new(format!("DATENABRUF-{}", uuid::Uuid::new_v4()));
    let domain_cmd = GeliGasDatanabrufCommand::InitiateAnfrage {
        pid,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: gnb_mp_id,
        message_ref,
    };

    let workflow_id = WorkflowId::new(GELI_GAS_DATENABRUF_WORKFLOW_NAME, latest_format_version());
    let process = mako_engine::process::Process::<
        GeliGasDatanabrufWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();

    let due_at = mako_engine::fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        10,
        mako_engine::fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id,
        mako_geli_gas::datenabruf::ANTWORT_WINDOW_LABEL,
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await?;

    let identity = process.identity();
    let _ = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, malo_id.as_str(), process_id, identity)
        .await;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch `geli.eog.anmelden` — GNB registers a Gas MaLo into Ersatz-/
/// Grundversorgung by sending UTILMD G **44013** (EoG Anmeldung) to the E/G
/// Lieferant. This is the Gas twin of `gpke.eog.anmelden` (Strom 55013).
///
/// Spawns a [`GeliGasSupplierChangeWorkflow`] in its GNB-initiator role
/// ([`GasSupplierChangeCommand::InitiateGnbProcess`]) and registers the
/// 10-Werktage response window (BK7-24-01-009) atomically.
///
/// ## Required payload fields
///
/// | Field          | Type   | Notes |
/// |----------------|--------|-------|
/// | `malo_id`      | string | Gas Marktlokations-ID |
/// | `gv_mp_id`     | string | MP-ID of the E/G Lieferant (message receiver) |
/// | `process_date` | string | Zuordnungsbeginn, ISO-8601 or `YYYYMMDD` |
pub(super) async fn dispatch_geli_eog_anmelden(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    const EOG_GAS_PID: u32 = 44013;

    let malo_id = extract_malo_id(payload)?;
    let gv_mp_id = payload
        .get("gv_mp_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"gv_mp_id\" (MP-ID of the E/G Lieferant)".into(),
            )
        })?
        .to_owned();
    let process_date = payload
        .get("process_date")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"process_date\" (Zuordnungsbeginn, ISO-8601)".into(),
            )
        })?
        .to_owned();

    let today = time::OffsetDateTime::now_utc().date();
    let document_date = format!(
        "{:04}{:02}{:02}",
        today.year(),
        u8::from(today.month()),
        today.day()
    );

    let pid = Pruefidentifikator::new(EOG_GAS_PID).map_err(DispatchError::InvalidPayload)?;
    let message_ref = MessageRef::new(format!("EOG-GAS-{}", uuid::Uuid::new_v4()));
    let domain_cmd = GasSupplierChangeCommand::InitiateGnbProcess {
        pid,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: MarktpartnerCode::new(gv_mp_id),
        malo_id: malo_id.clone(),
        document_date,
        process_date,
        message_ref,
    };

    // Duplicate guard: refuse only while a supplier-change process is still
    // running for this Gas-MaLo. A GNB rejection is terminal and must not retire
    // the MaLo — the corrected Anmeldung that follows is a normal GeLi Gas flow.
    // See `find_occupying_process`.
    if let Some(dup_id) = find_occupying_process::<GeliGasSupplierChangeWorkflow>(
        state,
        malo_id.as_str(),
        mako_geli_gas::WORKFLOW_NAME,
    )
    .await?
    {
        tracing::warn!(
            malo_id    = %malo_id,
            process_id = %dup_id,
            "geli.eog.anmelden refused: a supplier-change process is still running for this Gas-MaLo",
        );
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: malo_id.into(),
        });
    }

    let workflow_id = WorkflowId::new(mako_geli_gas::WORKFLOW_NAME, latest_format_version());
    let process = mako_engine::process::Process::<
        GeliGasSupplierChangeWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();

    let due_at = mako_engine::fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        10,
        mako_engine::fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id,
        mako_geli_gas::LIEFERBEGINN_RESPONSE_WINDOW_LABEL,
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await?;

    let identity = process.identity();
    if let Err(e) = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, malo_id.as_str(), process_id, identity)
        .await
    {
        tracing::warn!(
            process_id = %process_id,
            malo_id    = %malo_id,
            error      = %e,
            "geli.eog.anmelden: business-key registration failed (non-fatal)",
        );
    }

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch `geli.gas.stornierung.initiieren` — LFN/LFA initiates a Gas
/// supply-change cancellation (UTILMD G 44022 outbound to GNB).
///
/// ## Required payload fields
///
/// | Field         | Type   | Notes |
/// |---------------|--------|-------|
/// | `malo_id`     | string | Gas Marktlokations-ID (11-digit EIC) |
/// | `bgm_qualifier` | string (opt.) | `E01`=Kündigung (default), `E02`=Rücktritt, `E35`=Sperrung |
///
/// Spawns a new [`GeliGasLfStornierungWorkflow`] keyed on `malo_id`.
/// 10-Werktage response deadline (BK7-24-01-009) registered atomically.
pub(super) async fn dispatch_geli_gas_stornierung_initiieren(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_geli_gas::{
        GeliGasLfStornierungWorkflow, LfStornierungCommand, STORNIERUNG_LF_WORKFLOW_NAME,
    };

    let malo_id = extract_malo_id(payload)?;
    let bgm_qualifier = payload
        .get("bgm_qualifier")
        .and_then(|v| v.as_str())
        .unwrap_or("E01")
        .to_owned();

    let malo_record = state
        .malo_cache
        .get(&state.tenant_id.to_string(), malo_id.as_str())
        .await
        .map_err(|e| DispatchError::Engine(mako_engine::error::EngineError::store(e.to_string())))?
        .ok_or_else(|| DispatchError::MaloNotFound(malo_id.to_string()))?;

    let gnb_mp_id = malo_record
        .data_market_location
        .data_market_location_network_operators
        .iter()
        .max_by_key(|p| (p.execution_time_until.is_none(), &p.execution_time_from))
        .map(|p| MarktpartnerCode::new(format!("{:013}", p.market_partner_id)))
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "MaLo {malo_id} has no network_operator entry — GNB GLN cannot be resolved",
            ))
        })?;

    let pid = Pruefidentifikator::new(44022).map_err(DispatchError::InvalidPayload)?;
    let domain_cmd = LfStornierungCommand::InitiateStornierung {
        pid,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: gnb_mp_id,
        vorgang_id: malo_id.clone(),
        bgm_qualifier,
    };

    // Duplicate guard — only a Stornierung still awaiting the GNB blocks a new
    // one. See `find_occupying_process`.
    if let Some(dup_id) = find_occupying_process::<GeliGasLfStornierungWorkflow>(
        state,
        malo_id.as_str(),
        STORNIERUNG_LF_WORKFLOW_NAME,
    )
    .await?
    {
        tracing::warn!(
            malo_id = %malo_id,
            process_id = %dup_id,
            "geli.gas.stornierung.initiieren refused: active stornierung already exists for this MaLo",
        );
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: malo_id.into(),
        });
    }

    let workflow_id = WorkflowId::new(STORNIERUNG_LF_WORKFLOW_NAME, latest_format_version());
    let process = mako_engine::process::Process::<
        GeliGasLfStornierungWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();

    let due_at = mako_engine::fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        10,
        mako_engine::fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id,
        mako_geli_gas::STORNIERUNG_LF_RESPONSE_WINDOW_LABEL,
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await?;

    let identity = process.identity();
    let _ = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, malo_id.as_str(), process_id, identity)
        .await;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch a GeLi Gas LFN-side Lieferbeginn (PID 44001) or Lieferende (PID 44002).
///
/// Spawns a new [`GeliGasLfAnmeldungWorkflow`] and atomically registers a
/// 10-Werktage GNB-response deadline (BK7-24-01-009).
///
/// ## Required payload fields
///
/// | Field             | Type   | Notes |
/// |-------------------|--------|-------|
/// | `malo_id`         | string | Gas Marktlokations-ID (11-digit EIC, IDE+Z19) |
/// | `zaehlpunkt`      | string | Zählpunktbezeichnung (RFF+Z13) — mandatory per AHB |
/// | `process_date`    | string | Lieferbeginn/Lieferende date (YYYYMMDD, German local) |
pub(super) async fn dispatch_geli_lf_anmeldung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    pid_code: u32,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_geli_gas::lf_anmeldung::WORKFLOW_NAME as GELI_LF_ANMELDUNG_WF;

    if !LF_ANMELDUNG_ANFRAGE_PIDS.contains(&pid_code) {
        return Err(DispatchError::InvalidPayload(format!(
            "geli.lieferbeginn.anmelden: unsupported PID {pid_code}"
        )));
    }

    let malo_id = extract_malo_id(payload)?;

    let zaehlpunkt = payload
        .get("zaehlpunkt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"zaehlpunkt\" (Zählpunktbezeichnung, RFF+Z13)".to_owned(),
            )
        })?
        .to_owned();

    let process_date = payload
        .get("process_date")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"process_date\" (YYYYMMDD in German local time)".to_owned(),
            )
        })?
        .to_owned();

    // Resolve GNB GLN from MaLo cache.
    let malo_record = state
        .malo_cache
        .get(&state.tenant_id.to_string(), malo_id.as_str())
        .await
        .map_err(|e| DispatchError::Engine(mako_engine::error::EngineError::store(e.to_string())))?
        .ok_or_else(|| DispatchError::MaloNotFound(malo_id.to_string()))?;

    let gnb_mp_id = malo_record
        .data_market_location
        .data_market_location_network_operators
        .iter()
        .max_by_key(|p| (p.execution_time_until.is_none(), &p.execution_time_from))
        .map(|p| MarktpartnerCode::new(format!("{:013}", p.market_partner_id)))
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "MaLo {malo_id} has no network_operator — GNB GLN cannot be resolved",
            ))
        })?;

    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;
    let domain_cmd = GeliGasLfAnmeldungCommand::InitiateAnmeldung {
        pid,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: gnb_mp_id,
        malo_id: malo_id.clone(),
        zaehlpunkt,
        process_date,
        // SG4 STS Transaktionsgrund — E01/E02 permit the 6-week retroactive
        // window for SLP metering (AWH GeLi Gas 2.0 Kap. 2.2); E03 is
        // future-only. Rendered as the outbound STS segment when supplied.
        transaktionsgrund: payload
            .get("transaktionsgrund")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        received_at: time::OffsetDateTime::now_utc(),
    };

    // Duplicate guard — a GNB rejection is terminal and must not retire the
    // Gas-MaLo. See `find_occupying_process`.
    if let Some(dup_id) = find_occupying_process::<GeliGasLfAnmeldungWorkflow>(
        state,
        malo_id.as_str(),
        GELI_LF_ANMELDUNG_WF,
    )
    .await?
    {
        tracing::warn!(
            malo_id = %malo_id,
            process_id = %dup_id,
            "geli.lieferbeginn.anmelden refused: active process already registered for this Gas-MaLo",
        );
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: malo_id.into(),
        });
    }

    let workflow_id = WorkflowId::new(GELI_LF_ANMELDUNG_WF, latest_format_version());
    let process = mako_engine::process::Process::<
        GeliGasLfAnmeldungWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();

    // 10-Werktage GNB response deadline (BK7-24-01-009).
    let due_at = mako_engine::fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        10,
        mako_engine::fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id,
        mako_geli_gas::LF_ANMELDUNG_RESPONSE_WINDOW_LABEL,
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await?;

    let identity = process.identity();
    let _ = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, malo_id.as_str(), process_id, identity)
        .await;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch a GeLi Gas GNB→LFG Antwort to an existing
/// `GeliGasSupplierChangeWorkflow` process looked up by gas `malo_id`.
///
/// Called for `geli.lieferbeginn.bestaetigen`, `geli.lieferbeginn.ablehnen`,
/// `geli.lieferende.bestaetigen`, `geli.lieferende.ablehnen`.
pub(super) async fn dispatch_geli_gas_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    positive: bool,
    response_pid_code: u32,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;

    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    // GeLi Gas uses SendAntwort — the GNB/LFA sends the UTILMD G Antwort.
    let _ = response_pid_code; // recorded in audit log, not in SendAntwort
    dispatch_to_process::<GeliGasSupplierChangeWorkflow, _>(
        state,
        malo_id.as_str(),
        mako_geli_gas::WORKFLOW_NAME,
        move || GasSupplierChangeCommand::SendAntwort {
            accepted: positive,
            reason: reason.clone(),
            obligations: vec![],
        },
    )
    .await
}

/// Settle or dispute a GeLi Gas AWH Sperrprozesse INVOIC (PID 31011).
///
/// Dispatched by `invoicd` after the plausibility check completes.
/// Business key = `invoice_ref` (EDIFACT message reference from the inbound INVOIC).
pub(super) async fn dispatch_geli_gas_invoic(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    settle: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let invoice_ref = extract_invoice_ref(payload)?;
    let reason = payload
        .get("ablehnungsgrund")
        .and_then(|v| v.as_str())
        .unwrap_or("Automatisch ermittelte Abweichung — GeLi Gas 31011")
        .to_owned();
    dispatch_to_process::<GeliGasSperrprozesseInvoicWorkflow, _>(
        state,
        &invoice_ref,
        GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME,
        move || {
            if settle {
                GeliGasSperrprozesseInvoicCommand::SettleInvoice
            } else {
                GeliGasSperrprozesseInvoicCommand::DisputeInvoice {
                    reason: reason.clone(),
                }
            }
        },
    )
    .await
}

pub(super) fn cmd_geli_gas_rechnung_annehmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_gas_invoic(s, p, true))
}

pub(super) fn cmd_geli_gas_rechnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_gas_invoic(s, p, false))
}
