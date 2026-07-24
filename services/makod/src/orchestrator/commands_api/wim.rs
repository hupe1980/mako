//! WiM Strom command wrappers and dispatchers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

pub(super) fn cmd_wim_geraetewechsel_beauftragen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_geraetewechsel_beauftragen(s, p))
}

pub(super) fn cmd_wim_geraetewechsel_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_aperak(s, p, true))
}

pub(super) fn cmd_wim_geraetewechsel_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_aperak(s, p, false))
}

pub(super) fn cmd_wim_preisanfrage_angebot_senden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_preisanfrage_angebot_senden(s, p))
}

pub(super) fn cmd_wim_steuerungsauftrag_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_steuerungsauftrag_endantwort(s, p, true))
}

pub(super) fn cmd_wim_steuerungsauftrag_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_steuerungsauftrag_endantwort(s, p, false))
}

pub(super) fn cmd_wim_rechnung_annehmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let invoice_ref = extract_invoice_ref(p)?;
        dispatch_to_process::<WimRechnungWorkflow, _>(s, &invoice_ref, "wim-rechnung", || {
            WimRechnungCommand::Settle
        })
        .await
    })
}

pub(super) fn cmd_wim_rechnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let invoice_ref = extract_invoice_ref(p)?;
        let reason = p
            .get("ablehnungsgrund")
            .and_then(|v| v.as_str())
            .unwrap_or("Automatisch ermittelte Abweichung — WiM 31009")
            .to_owned();
        dispatch_to_process::<WimRechnungWorkflow, _>(s, &invoice_ref, "wim-rechnung", || {
            WimRechnungCommand::Dispute { reason }
        })
        .await
    })
}

/// Spawn an outbound WiM MSB-Wechsel order (UTILMD 55039/55042/55051/55168).
///
/// **Roles: NB or MSB** — the PID decides the direction:
///
/// | PID   | Process                              | Von  | An   | Antwortfrist |
/// |-------|--------------------------------------|------|------|--------------|
/// | 55039 | Kündigung MSB                        | MSBN | MSBA | 3 WT |
/// | 55042 | Anmeldung MSB                        | MSBN | NB   | 5 WT |
/// | 55051 | Ende MSB (Abmeldung)                 | MSBA | NB   | 7 WT |
/// | 55168 | Verpflichtungsanfrage / Aufforderung | NB   | gMSB | 1 WT |
///
/// The caller supplies `melo_id`, `process_date`, and the counterparty GLN
/// (`receiver_mp_id`). Business key = `melo_id`.
///
/// Deadline: 5 Werktage for the counterparty's answer (WiM BK6-24-174).
pub(super) async fn dispatch_wim_geraetewechsel_beauftragen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let melo_id = extract_melo_id(payload)?;

    let pid_code = payload
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .map(|n| n as u32)
        .unwrap_or(55_042);

    if !mako_wim::DEVICE_CHANGE_PIDS.contains(&pid_code) {
        return Err(DispatchError::InvalidPayload(format!(
            "unsupported WiM MSB-Wechsel PID {pid_code}; expected one of {:?}",
            mako_wim::DEVICE_CHANGE_PIDS
        )));
    }

    let process_date = payload
        .get("process_date")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"process_date\" (YYYYMMDD in German local time)".to_owned(),
            )
        })?
        .to_owned();

    // The counterparty GLN cannot be derived from the MeLo alone: for 55039/55042
    // it is the NB, for 55051/55168 it is the nMSB. The ERP knows which.
    let receiver_mp_id = payload
        .get("receiver_mp_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"receiver_mp_id\" (13-digit GLN of the counterparty: \
                 the MSBA for 55039, the NB for 55042 and 55051, the gMSB for 55168)"
                    .to_owned(),
            )
        })?
        .to_owned();

    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;
    let message_ref = MessageRef::new(format!("WIM-GW-{}", uuid::Uuid::new_v4()));

    let domain_cmd = DeviceChangeCommand::InitiateDeviceChange {
        pid,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: MarktpartnerCode::new(receiver_mp_id),
        melo_id: melo_id.clone(),
        process_date,
        message_ref,
    };

    // Idempotency: one active device-change process per MeLo.
    let existing = state
        .store
        .as_process_registry()
        .lookup_correlated(state.tenant_id, melo_id.as_str())
        .await
        .map_err(DispatchError::Engine)?;
    if let Some(first) = existing
        .into_iter()
        .find(|id| id.workflow_id.name.as_ref() == mako_wim::WORKFLOW_NAME)
    {
        let dup_id = first.process_id;
        tracing::warn!(
            melo_id = %melo_id,
            process_id = %dup_id,
            pid = pid_code,
            "wim.geraetewechsel.beauftragen refused: active device-change process already exists",
        );
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: melo_id.into(),
        });
    }

    let workflow_id = WorkflowId::new(mako_wim::WORKFLOW_NAME, latest_format_version());
    let process = mako_engine::process::Process::<
        WimDeviceChangeWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();

    // Antwortfrist per process — NOT a flat 5 WT (BK6-24-174 WiM Teil 1):
    // 55039 → 3 WT · 55042 → 5 WT · 55051 → 7 WT · 55168 → 1 WT.
    let frist_wt = mako_wim::antwort_frist_werktage(pid_code).ok_or_else(|| {
        DispatchError::InvalidPayload(format!("no Antwortfrist defined for PID {pid_code}"))
    })?;
    let due_at = mako_engine::fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        frist_wt,
        mako_engine::fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id,
        mako_wim::AUFTRAG_ANTWORT_WINDOW_LABEL,
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await?;

    let identity = process.identity();
    let _ = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, melo_id.as_str(), process_id, identity)
        .await;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch `DeviceChangeCommand::DispatchAperak` to an existing
/// `WimDeviceChangeWorkflow` process looked up by `melo_id`.
///
/// Called for `wim.geraetewechsel.bestaetigen` and `wim.geraetewechsel.ablehnen`.
pub(super) async fn dispatch_wim_aperak(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    positive: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let melo_id = extract_melo_id(payload)?;

    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    // WiM uses melo_id as the business key (not malo_id).
    dispatch_to_process::<WimDeviceChangeWorkflow, _>(
        state,
        melo_id.as_str(),
        mako_wim::WORKFLOW_NAME,
        move || DeviceChangeCommand::DispatchAperak {
            positive,
            reason: reason.clone(),
        },
    )
    .await
}

/// Dispatch `PreisanfrageCommand::SendAngebot` to an existing
/// `WimPreisanfrageWorkflow` process looked up by `melo_id`.
///
/// Called for `wim.preisanfrage.angebot-senden` — the aMSB answers an inbound
/// REQOTE Preisanfrage (35001–35005) with the QUOTES Angebot (15001–15005).
/// The response PID is derived inside the workflow from the stored REQOTE PID;
/// the price content comes from the aMSB's current PreisblattMessung.
pub(super) async fn dispatch_wim_preisanfrage_angebot_senden(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let melo_id = extract_melo_id(payload)?;
    dispatch_to_process::<WimPreisanfrageWorkflow, _>(
        state,
        melo_id.as_str(),
        mako_wim::PREISANFRAGE_WORKFLOW_NAME,
        move || PreisanfrageCommand::SendAngebot {
            message_ref: MessageRef::new(format!("WIM-QUOTES-{}", uuid::Uuid::new_v4())),
        },
    )
    .await
}

/// Dispatch `wim.steuerungsauftrag.bestaetigen` / `.ablehnen` to an existing
/// `WimSteuerungsauftragWorkflow` process looked up by `tx_id`.
///
/// The `tx_id` is the transaction ID that arrived in the original
/// `POST /steuerbefehl/konfiguration/` or `/initialZustand/` REST request.
/// It is the natural business key for the process registry.
pub(super) async fn dispatch_steuerungsauftrag_endantwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    positive: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let tx_id = payload
        .get("tx_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"tx_id\" (transaction ID from the original \
             konfiguration/initialZustand request)"
                    .into(),
            )
        })?
        .to_owned();

    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    if positive {
        let reference_id = payload
            .get("reference_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // ── M1: Konfigurationsprodukt eligibility guard ───────────────────────
        // Before dispatching the positive ORDRSP, verify that the SR has the
        // requested `produkt_code` in its contracted `konfigurationsprodukte`
        // list in `marktd`.  If the product is not contracted, auto-dispatch
        // `ablehnen` with ERC A99 instead of the requested `bestaetigen`.
        //
        // Only runs when:
        //   1. `state.marktd_client` is configured (optional — disabled in dev mode)
        //   2. The process state is `Received` with a `produkt_code` set
        //   3. The `location_id` is a SteuerbareRessource (SR ID starts with "C")
        if let Some(ref marktd) = state.marktd_client {
            use mako_wim::steuerungsauftrag::{LocationId, SteuerungsauftragState};
            // Look up the process identity for this tx_id.
            let registry = state.store.as_process_registry();
            if let Ok(identities) = registry.lookup_correlated(state.tenant_id, &tx_id).await {
                let maybe_identity = identities.into_iter().find(|id| {
                    id.workflow_id.name.as_ref() == mako_wim::steuerungsauftrag::WORKFLOW_NAME
                });

                if let Some(identity) = maybe_identity {
                    let proc =
                        mako_engine::process::Process::<
                            WimSteuerungsauftragWorkflow,
                            Arc<mako_engine::store_slatedb::SlateDbStore>,
                        >::from_identity(Arc::clone(&state.store), identity);

                    #[allow(clippy::collapsible_match)]
                    if let Ok(SteuerungsauftragState::Received(ref data)) =
                        proc.state_with_snapshot(&state.snapshot_store).await
                    {
                        #[allow(clippy::collapsible_match)]
                        if let (LocationId::Sr(sr_id), Some(produkt_code)) =
                            (&data.location_id, &data.produkt_code)
                        {
                            match marktd.get_konfigurationsprodukte(sr_id.as_ref()).await {
                                Ok(Some(products)) => {
                                    let contracted = products.iter().any(|p| {
                                        p.get("produktCode")
                                            .or_else(|| p.get("produkt_code"))
                                            .and_then(|v| v.as_str())
                                            .map(|code| code == produkt_code.as_str())
                                            .unwrap_or(false)
                                    });
                                    if !contracted {
                                        let reject_reason = format!(
                                            "ERC A99: Konfigurationsprodukt '{}' is not in the \
                                             contracted konfigurationsprodukte list for SR {}",
                                            produkt_code, sr_id
                                        );
                                        tracing::warn!(
                                            tx_id = %tx_id,
                                            sr_id = %sr_id,
                                            produkt_code = %produkt_code,
                                            "M1: Konfigurationsprodukt not contracted — auto-dispatching ablehnen (ERC A99)"
                                        );
                                        return dispatch_to_process::<WimSteuerungsauftragWorkflow, _>(
                                            state,
                                            &tx_id,
                                            mako_wim::steuerungsauftrag::WORKFLOW_NAME,
                                            move || mako_wim::steuerungsauftrag::SteuerungsauftragCommand::SendEndantwortNegativ {
                                                reason: Some(reject_reason),
                                            },
                                        )
                                        .await;
                                    }
                                }
                                Ok(None) => {
                                    tracing::warn!(
                                        tx_id = %tx_id,
                                        sr_id = %sr_id,
                                        "M1: SR not found in marktd — skipping Konfigurationsprodukt guard"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        tx_id = %tx_id,
                                        sr_id = %sr_id,
                                        error = %e,
                                        "M1: marktd request failed — skipping Konfigurationsprodukt guard (fail-open)"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        dispatch_to_process::<WimSteuerungsauftragWorkflow, _>(
            state,
            &tx_id,
            mako_wim::steuerungsauftrag::WORKFLOW_NAME,
            move || SteuerungsauftragCommand::SendEndantwortPositiv {
                reference_id: reference_id.clone(),
            },
        )
        .await
    } else {
        dispatch_to_process::<WimSteuerungsauftragWorkflow, _>(
            state,
            &tx_id,
            mako_wim::steuerungsauftrag::WORKFLOW_NAME,
            move || SteuerungsauftragCommand::SendEndantwortNegativ {
                reason: reason.clone(),
            },
        )
        .await
    }
}
