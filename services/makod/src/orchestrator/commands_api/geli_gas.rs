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
    Box::pin(dispatch_geli_lf_anmeldung(s, p, 44004))
}

/// **44016** Kündigung beim Altlieferanten, LFN → LFA.
///
/// The Gas twin of `gpke.kuendigung.anmelden`. GeLi Gas 3.0 § 3.1 makes the
/// electronic Kündigung the Neulieferant's own message to the Altlieferant —
/// the grid operator is not in it — and forbids the Altlieferant from refusing
/// it for want of a contractually agreed form.
pub(super) fn cmd_geli_kuendigung_anmelden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_lf_anmeldung(s, p, 44016))
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

/// Dispatch `geli.datenabruf.anfragen` — LF requests Gas quality data from NB.
///
/// Spawns a new [`GeliGasDatenabrufWorkflow`] and sends ORDERS 17103 outbound
/// to the GNB requesting Abrechnungsbrennwert and Zustandszahl. The GNB's
/// answer window is registered in the same write: ten Werktage after the
/// Anfrage, but not before the 10. Werktag of the month after the Zeitspanne
/// ends (GeLi Gas 3.0 Kap. 4.2).
///
/// ## Required payload fields
///
/// | Field     | Type   | Notes |
/// |-----------|--------|-------|
/// | `malo_id` | string | Gas Marktlokations-ID (11-digit EIC) |
/// | `zeitspanne_bis` | string | Last day of the requested Zeitspanne, `YYYY-MM-DD` |
pub(super) async fn dispatch_geli_gas_datenabruf_anfragen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;
    // The Zeitspanne's end sets the earliest day the GNB may answer, so the
    // window cannot be computed without it — refused, not defaulted.
    let zeitspanne_bis = payload
        .get("zeitspanne_bis")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "zeitspanne_bis (YYYY-MM-DD) is required — the last day of the Zeitspanne \
                 the Brennwert is asked for (GeLi Gas 3.0 Kap. 4.2)"
                    .to_owned(),
            )
        })
        .and_then(|s| {
            time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]"))
                .map_err(|e| DispatchError::InvalidPayload(format!("zeitspanne_bis {s:?}: {e}")))
        })?;

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
                "MaLo {malo_id} has no network_operator — GNB MP-ID cannot be resolved",
            ))
        })?;

    let pid = Pruefidentifikator::new(17103).map_err(DispatchError::InvalidPayload)?;
    let message_ref = MessageRef::new(format!("DATENABRUF-{}", uuid::Uuid::new_v4()));
    let domain_cmd = GeliGasDatenabrufCommand::InitiateAnfrage {
        pid,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: gnb_mp_id,
        message_ref,
        zeitspanne_bis,
    };

    let workflow_id = WorkflowId::new(GELI_GAS_DATENABRUF_WORKFLOW_NAME, latest_format_version());

    let due_at = mako_fristen::antwort::brennwert_zustandszahl_due(
        time::OffsetDateTime::now_utc(),
        zeitspanne_bis,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    let process_id = spawn_process::<GeliGasDatenabrufWorkflow>(
        &state.store,
        state.tenant_id,
        workflow_id,
        domain_cmd,
        &[(mako_geli_gas::datenabruf::ANTWORT_WINDOW_LABEL, due_at)],
        &[malo_id.as_str()],
    )
    .await?;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch `geli.eog.anmelden` — GNB registers a Gas MaLo into Ersatz-/
/// Grundversorgung by sending UTILMD G **44013** (EoG Anmeldung) to the E/G
/// Lieferant. This is the Gas twin of `gpke.eog.anmelden` (Strom 55013).
///
/// Spawns a [`GeliGasSupplierChangeWorkflow`] in its GNB-initiator role
/// ([`GasSupplierChangeCommand::InitiateGnbProcess`]) and registers the E/G's
/// answer window atomically — „spätestens bis zum Ablauf des 2. Werktages"
/// (GeLi Gas 3.0 Kap. 3.3), out of `mako_fristen::antwort`. The window matters
/// beyond alerting: „Meldet sich der Ersatz-/ Grundversorger nicht
/// fristgerecht, ordnet der Netzbetreiber die Marktlokation zu dem von ihm
/// gemeldeten Termin dem Ersatz-/ Grundversorger zu", so its lapse completes
/// the Zuordnung.
///
/// The process is keyed by [`mako_geli_gas::gnb_anfrage_key`], not the bare
/// Marktlokation: on a deployment that is GNB and LF at once the 44013 returns
/// through the loopback and opens the LF's own process on that Marktlokation,
/// under the same workflow name.
///
/// The 44013 carries the Marktlokation's Stammdaten whole (UTILMD AHB Gas 1.2
/// Kap. 5.9; GeLi Gas 3.0 Kap. 3.3.2). They are read from marktd for the
/// Zuordnungsbeginn ([`super::eog_stammdaten`]) — the caller sends none, and a
/// fact marktd does not hold refuses the command naming it, as does a marktd
/// that does not answer. Before the spawn the 44013 is rendered and checked
/// against its AHB profile — the check the pre-send gate runs — so a command
/// whose message lacks a Muss part is refused here, naming it, rather than
/// written as a message the gate dead-letters.
///
/// ## Payload — [`mako_markt::commands::GeliEogAnmelden`]
///
/// | Field          | Type   | Notes |
/// |----------------|--------|-------|
/// | `malo_id`      | string | Gas Marktlokations-ID |
/// | `gv_mp_id`     | string | MP-ID of the E/G Lieferant (message receiver) |
/// | `process_date` | string | Zuordnungsbeginn `SG4 DTM+92`, ISO-8601 or `YYYYMMDD` |
/// | `bilanzierungsbeginn` | string | `SG4 DTM+158`, ISO-8601 or `YYYYMMDD` |
/// | `transaktionsgrund` | string | `SG4 STS+7` DE 9013 (`E06`/`Z02`/`Z36`/`Z37`/`Z39`/`ZC6`/`ZC7`/`ZT6`/`ZT7`/`ZZD`) |
///
/// Any other key is refused.
pub(super) async fn dispatch_geli_eog_anmelden(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    const EOG_GAS_PID: u32 = 44013;

    let body: mako_markt::commands::GeliEogAnmelden =
        mako_service::json_from_value(payload.clone()).map_err(|e| {
            DispatchError::InvalidPayload(format!(
                "geli.eog.anmelden payload: {e} — Muss on the Anmeldung EoG 44013 \
                 (UTILMD AHB Gas 1.2 Kap. 5.9)"
            ))
        })?;
    let malo_id = extract_malo_id(&serde_json::json!({ "malo_id": body.malo_id }))?;
    let nonempty = |v: &str, field: &str| {
        let v = v.trim();
        if v.is_empty() {
            Err(DispatchError::InvalidPayload(format!(
                "\"{field}\" must not be empty — Muss on the Anmeldung EoG 44013 \
                 (UTILMD AHB Gas 1.2 Kap. 5.9)"
            )))
        } else {
            Ok(v.to_owned())
        }
    };
    let transaktionsgrund = nonempty(&body.transaktionsgrund, "transaktionsgrund")?;
    let bilanzierungsbeginn = nonempty(&body.bilanzierungsbeginn, "bilanzierungsbeginn")?;
    let gv_mp_id = nonempty(&body.gv_mp_id, "gv_mp_id")?;
    let process_date = body.process_date;
    let stichtag = super::eog_stammdaten::stichtag(&process_date)?;
    let record = super::eog_stammdaten::lesen(state, malo_id.as_str(), stichtag).await?;
    let stammdaten = super::eog_stammdaten::gas_44013(&record, &transaktionsgrund)?;

    let today = mako_fristen::heute();
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
        transaktionsgrund: Some(transaktionsgrund),
        bilanzierungsbeginn: Some(bilanzierungsbeginn),
        stammdaten,
    };
    {
        use mako_engine::workflow::Workflow as _;
        let out = GeliGasSupplierChangeWorkflow::handle(
            &mako_geli_gas::GasSupplierChangeState::New,
            domain_cmd.clone(),
        )
        .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
        super::eog_stammdaten::refuse_what_the_gate_refuses(state, &out.outbox)?;
    }

    // Duplicate guard: refuse only while an Anfrage this GNB sent for the
    // Gas-MaLo is still running. An LF rejection is terminal and must not retire
    // the MaLo. See `find_occupying_process`.
    let gnb_key = mako_geli_gas::gnb_anfrage_key(malo_id.as_str());
    let occupancy = find_occupying_process::<GeliGasSupplierChangeWorkflow>(
        state,
        &gnb_key,
        mako_geli_gas::WORKFLOW_NAME,
    )
    .await?;
    if let Some(dup_id) = occupancy.process {
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

    let due_at =
        mako_fristen::antwort::antwort_deadline(EOG_GAS_PID, time::OffsetDateTime::now_utc())
            .ok_or_else(|| {
                DispatchError::InvalidPayload(format!(
                    "PID {EOG_GAS_PID} has no published answer window in mako_fristen::antwort"
                ))
            })?;
    let process_id = spawn_process::<GeliGasSupplierChangeWorkflow>(
        &state.store,
        state.tenant_id,
        workflow_id,
        domain_cmd,
        &[(mako_geli_gas::LIEFERBEGINN_RESPONSE_WINDOW_LABEL, due_at)],
        &[&gnb_key],
    )
    .await?;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch `geli.stornierung.initiieren` — LFN/LFA initiates a Gas
/// supply-change cancellation (UTILMD G 44022 outbound to GNB).
///
/// ## Required payload fields
///
/// | Field         | Type   | Notes |
/// |---------------|--------|-------|
/// | `malo_id`     | string | Gas Marktlokations-ID (11-digit EIC) |
/// | `bgm_qualifier` | string (opt.) | `E01`=Kündigung (default), `E02`=Rücktritt, `E35`=Sperrung |
///
/// Spawns a new [`GeliGasLfStornierungWorkflow`] keyed on `malo_id`. The
/// GNB's answer is due within the **original** message's Antwortfrist: a
/// Stornierung is admissible only while that message is unanswered (GeLi Gas
/// 3.0 Kap. 2.7), and no Festlegung gives the Stornierung a window of its own.
/// The window is therefore the one the running Anmeldung, Abmeldung or
/// Kündigung registered; with none running, the Stornierung is refused.
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
                "MaLo {malo_id} has no network_operator entry — GNB MP-ID cannot be resolved",
            ))
        })?;

    let pid = Pruefidentifikator::new(44022).map_err(DispatchError::InvalidPayload)?;
    let domain_cmd = LfStornierungCommand::InitiateStornierung {
        pid,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: gnb_mp_id,
        malo: malo_id.clone(),
        bgm_qualifier,
    };

    // The message being cancelled: the running LF process on this MaLo. Its
    // answer window bounds the Stornierung's.
    let original = original_antwortfrist(state, malo_id.as_str()).await?;

    // Duplicate guard — only a Stornierung still awaiting the GNB blocks a new
    // one. See `find_occupying_process`.
    let occupancy = find_occupying_process::<GeliGasLfStornierungWorkflow>(
        state,
        malo_id.as_str(),
        STORNIERUNG_LF_WORKFLOW_NAME,
    )
    .await?;
    if let Some(dup_id) = occupancy.process {
        tracing::warn!(
            malo_id = %malo_id,
            process_id = %dup_id,
            "geli.stornierung.initiieren refused: active stornierung already exists for this MaLo",
        );
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: malo_id.into(),
        });
    }

    let workflow_id = WorkflowId::new(STORNIERUNG_LF_WORKFLOW_NAME, latest_format_version());

    let deadlines: Vec<(&'static str, time::OffsetDateTime)> = original
        .map(|due_at| (mako_geli_gas::STORNIERUNG_LF_RESPONSE_WINDOW_LABEL, due_at))
        .into_iter()
        .collect();
    let process_id = spawn_process::<GeliGasLfStornierungWorkflow>(
        &state.store,
        state.tenant_id,
        workflow_id,
        domain_cmd,
        &deadlines,
        &[malo_id.as_str()],
    )
    .await?;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch a GeLi Gas LFN-side Anmeldung (44001), Abmeldung (44004) or
/// Kündigung beim Altlieferanten (44016).
///
/// Spawns a new [`GeliGasLfAnmeldungWorkflow`] and atomically registers the
/// **published** answer deadline for that PID from `mako_fristen::antwort` —
/// 4 Werktage for the Anmeldung, 3 for the Abmeldung and the Kündigung
/// (GeLi Gas 3.0 Kap. 3.1 / 3.2.3 / 3.2.2).
///
/// ## Required payload fields
///
/// | Field             | Type   | Notes |
/// |-------------------|--------|-------|
/// | `malo_id`         | string | Gas Marktlokations-ID → `SG5 LOC+Z16` |
/// | `zaehlpunkt`      | string | Zählpunktbezeichnung |
/// | `process_date`    | string | Lieferbeginn / Lieferende / Kündigungstermin (YYYYMMDD) |
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

    // Resolve GNB MP-ID from MaLo cache.
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
                "MaLo {malo_id} has no network_operator — GNB MP-ID cannot be resolved",
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
        // `SG10 CCI+Z19` DE 7037 — Muss on a 44001. GeLi Gas states the
        // Bilanzkreis in one segment; the Strom `SG8 SEQ+Z79` Produktpaket does
        // not exist here.
        bilanzkreis: payload
            .get("bilanzkreis")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        received_at: time::OffsetDateTime::now_utc(),
    };

    // Duplicate guard — a GNB rejection is terminal and must not retire the
    // Gas-MaLo. See `find_occupying_process`.
    let occupancy = find_occupying_process::<GeliGasLfAnmeldungWorkflow>(
        state,
        malo_id.as_str(),
        GELI_LF_ANMELDUNG_WF,
    )
    .await?;
    if let Some(dup_id) = occupancy.process {
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
    // The counterparty's published answer window, from the one table four
    // services share. It is **not** 10 Werktage: that is the LFN's own
    // Vorlauffrist before Lieferbeginn, and sizing the answer queue by it
    // reports a lapsed Frist as still running for six Werktage.
    let deadlines: Vec<(&'static str, time::OffsetDateTime)> =
        mako_fristen::antwort::antwort_deadline(pid_code, time::OffsetDateTime::now_utc())
            .map(|due_at| (mako_geli_gas::LF_ANMELDUNG_RESPONSE_WINDOW_LABEL, due_at))
            .into_iter()
            .collect();
    let process_id = spawn_process::<GeliGasLfAnmeldungWorkflow>(
        &state.store,
        state.tenant_id,
        workflow_id,
        domain_cmd,
        &deadlines,
        &[malo_id.as_str()],
    )
    .await?;

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

    // `SG4 STS+E01` is Muss on every Antwortnachricht, and the Gas Codelisten
    // are a different alphabet from Strom's: `A16` where `E_0622` says `A02`,
    // `ZC5` where it says `A06`, `E17` where it says `A07`. The code is resolved
    // inside the tree the caller names — the Gas MIG does not carry DE 1131, so
    // `antwort_tree` is the key and nothing rides on the wire beside the code.
    let tree = payload
        .get("antwort_tree")
        .and_then(|v| v.as_str())
        .unwrap_or(default_gas_tree(response_pid_code));
    let antwort_code = payload
        .get("antwort_code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "payload must contain \"antwort_code\" — the {tree} Antwortcode for \
                 SG4 STS+E01, which the AHB marks Muss on every Antwortnachricht"
            ))
        })?
        .to_owned();
    let entry = mako_pruefung::codes::lookup(tree, &antwort_code).ok_or_else(|| {
        DispatchError::InvalidPayload(format!(
            "Antwortcode {antwort_code:?} is not published by {tree}. Only codes from \
             that Codeliste are admissible."
        ))
    })?;
    // `None` means the code's tree is not on the agreement axis at all, which no
    // Gas Anmeldung/Abmeldung Codeliste is — refuse rather than guess.
    let Some(zustimmung) = entry.ist_zustimmung() else {
        return Err(DispatchError::InvalidPayload(format!(
            "Antwortcode {antwort_code:?} belongs to {tree}, whose Cluster is \"{}\" — not \
             the Zustimmung/Ablehnung pair this command derives its answer PID from.",
            entry.cluster.label(),
        )));
    };
    if zustimmung != positive {
        return Err(DispatchError::InvalidPayload(format!(
            "Antwortcode {antwort_code:?} is a{} in {tree}, but the command asks for a{}. \
             The published Cluster decides the answer PID.",
            if zustimmung {
                " Zustimmung"
            } else {
                "n Ablehnung"
            },
            if positive {
                " Bestätigung"
            } else {
                "n Ablehnung"
            },
        )));
    }

    let bemerkung = payload
        .get("bemerkung")
        .or_else(|| payload.get("reason"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    if entry.braucht_bemerkung && bemerkung.is_none() {
        return Err(DispatchError::InvalidPayload(format!(
            "Antwortcode {antwort_code:?} requires a written Erläuterung in FTX+ACB — \
             pass \"bemerkung\""
        )));
    }

    // The tree the code was resolved in rides the event, so the Verlauf
    // reads its wording: a Lieferbeginn is answered from two.
    let antwort_ebd = tree.to_owned();
    dispatch_to_process::<GeliGasSupplierChangeWorkflow, _>(
        state,
        malo_id.as_str(),
        mako_geli_gas::WORKFLOW_NAME,
        move || GasSupplierChangeCommand::SendAntwort {
            accepted: positive,
            antwort_code: antwort_code.clone(),
            antwort_ebd: Some(antwort_ebd.clone()),
            bemerkung: bemerkung.clone(),
            obligations: vec![],
            stammdaten: mako_engine::payload::Stammdaten::default(),
        },
    )
    .await
}

/// The Gas Codeliste that governs an answer PID, when the caller does not name
/// one. An Anmeldung is answered out of `E_3005`/`E_3007`, an Abmeldung out of
/// `E_3019`.
const fn default_gas_tree(response_pid: u32) -> &'static str {
    match response_pid {
        44_005 | 44_006 => mako_pruefung::codes::EBD_ABMELDUNG_GAS_NB,
        _ => mako_pruefung::codes::EBD_LIEFERBEGINN_GAS,
    }
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
    let message_ref = remadv_message_ref(payload);
    let antwort = remadv_antwort(payload);
    dispatch_to_process::<GeliGasSperrprozesseInvoicWorkflow, _>(
        state,
        &invoice_ref,
        GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME,
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

/// Issue an outbound INVOIC 31011 (GNB/VNB issuer role).
///
/// `netzbilanzd` settles the AWH positions; this builds the INVOIC from the
/// payload's `rechnung` and spawns the process the payer's inbound REMADV
/// correlates back to ([`super::invoic_stellen`]). Mirrors
/// `wim.msb-rechnung.stellen`, which does the same for PID 31009.
async fn dispatch_geli_gas_awh_send_invoic(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let sender = payload
        .get("sender_mp_id")
        .or_else(|| payload.get("nb_mp_id"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    let recipient = payload
        .get("recipient_mp_id")
        .or_else(|| payload.get("lf_mp_id"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    super::e_rechnung::edi_pruefen(state.marktd_client.as_deref(), payload, &recipient).await?;
    super::invoic_stellen::invoic_stellen::<GeliGasSperrprozesseInvoicWorkflow>(
        state,
        payload,
        super::invoic_stellen::Ausstellung {
            workflow_name: GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME,
            pid: 31011,
            sender: &sender,
            recipient: &recipient,
        },
    )
    .await
}

pub(super) fn cmd_sonstige_leistung_rechnung_stellen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_gas_awh_send_invoic(s, p))
}

pub(super) fn cmd_sonstige_leistung_rechnung_annehmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_gas_invoic(s, p, true))
}

pub(super) fn cmd_sonstige_leistung_rechnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_gas_invoic(s, p, false))
}

// ── The answers the Gas supplier owes ─────────────────────────────────────────
//
// One dispatcher per process, both commands of a pair landing on it: whether
// the Bestätigungs- or the Ablehnungs-PID goes out is decided by the
// Antwortcode's published Cluster, not by the command name the ERP called.
// `Z01` „Zustimmung mit Terminänderung" reads like a refusal and is a
// Zustimmung; a name-driven split would send it as the wrong message.

/// Dispatch the supplier's answer to a **Abmeldung NN vom NB** (44008/44009).
pub(super) async fn dispatch_geli_abmeldung_nb_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    dispatch_geli_lf_antwort(state, payload, mako_pruefung::codes::EBD_ABMELDUNG_GAS).await
}

pub(super) fn cmd_geli_abmeldung_nb_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_abmeldung_nb_antwort(s, p))
}

pub(super) fn cmd_geli_abmeldung_nb_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_abmeldung_nb_antwort(s, p))
}

/// Dispatch the supplier's answer to a **Abmeldungsanfrage des NB** (44011/44012).
pub(super) async fn dispatch_geli_abmeldungsanfrage_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    dispatch_geli_lf_antwort(
        state,
        payload,
        mako_pruefung::codes::EBD_ABMELDUNGSANFRAGE_GAS,
    )
    .await
}

pub(super) fn cmd_geli_abmeldungsanfrage_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_abmeldungsanfrage_antwort(s, p))
}

pub(super) fn cmd_geli_abmeldungsanfrage_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_abmeldungsanfrage_antwort(s, p))
}

/// Dispatch the supplier's answer to a **Kündigung beim alten Lieferanten** (44017/44018).
pub(super) async fn dispatch_geli_kuendigung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    dispatch_geli_lf_antwort(state, payload, mako_pruefung::codes::EBD_KUENDIGUNG_GAS).await
}

pub(super) fn cmd_geli_kuendigung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_kuendigung_antwort(s, p))
}

pub(super) fn cmd_geli_kuendigung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_kuendigung_antwort(s, p))
}

/// Dispatch the E/G supplier's answer to a Gas **EoG-Anmeldung** (44014/44015).
///
/// `E_3008`; the Gas twin of `gpke.eog.bestaetigen`. An Ablehnung 44015 is
/// the bare answer. A Bestätigung **44014** is read as
/// [`mako_markt::commands::GeliEogBestaetigen`], refusing a key it does not
/// declare. It carries the Marktlokation's record whole (UTILMD AHB Gas 1.2
/// Kap. 5.9) — read from marktd on the process's Zuordnungsbeginn, as the
/// 44013 was ([`super::eog_stammdaten`]) — and two facts of the LF's own the
/// command must state:
///
/// | Field | Type | Notes |
/// |---|---|---|
/// | `bilanzkreis` | string | `SG10 CCI+Z19` Bilanzkreis (Muss with `DTM+158`) |
/// | `abrechnungsintervall_monate` | `1`, `3`, `6` or `12` | `SG6 DTM+Z20` Abrechnungsintervall des LF — the four entries UTILMD MIG Gas G1.2 S. 55 admits |
///
/// A missing one is refused naming it, as is a fact marktd does not hold; the
/// rendered 44014 is checked against its profile before anything is written.
pub(super) async fn dispatch_geli_eog_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let ebd = mako_pruefung::codes::EBD_ANMELDUNG_EOG_GAS;
    let (antwort_code, zustimmung) = geli_antwortcode(payload, ebd)?;
    if !zustimmung {
        return answer_gas_supplier_change(state, payload, false, &antwort_code, Some(ebd)).await;
    }
    let body: mako_markt::commands::GeliEogBestaetigen =
        mako_service::json_from_value(payload.clone()).map_err(|e| {
            DispatchError::InvalidPayload(format!(
                "geli.eog.bestaetigen payload: {e} (Bestätigung EoG 44014, UTILMD AHB Gas 1.2 \
                 Kap. 5.9)"
            ))
        })?;
    let refuse = |field: &str, what: &str| {
        DispatchError::InvalidPayload(format!(
            "\"{field}\" is required — {what}, Muss on the Bestätigung EoG 44014 \
             (UTILMD AHB Gas 1.2 Kap. 5.9)"
        ))
    };
    let bilanzkreis = body
        .bilanzkreis
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| refuse("bilanzkreis", "the LF's Bilanzkreis (SG10 CCI+Z19)"))?
        .to_owned();
    let abrechnungsintervall_monate = body
        .abrechnungsintervall_monate
        .filter(|n| mako_markt::commands::ABRECHNUNGSINTERVALLE_MONATE.contains(n))
        .ok_or_else(|| {
            refuse(
                "abrechnungsintervall_monate",
                "the LF's Abrechnungsintervall in Monaten, 1, 3, 6 or 12 (SG6 DTM+Z20, \
                 DE 2379 802; UTILMD MIG Gas G1.2 S. 55)",
            )
        })?;
    let malo_id = extract_malo_id(payload)?;
    let bemerkung = body.bemerkung.or(body.reason);

    let current = super::current_state::<GeliGasSupplierChangeWorkflow>(
        state,
        malo_id.as_str(),
        mako_geli_gas::WORKFLOW_NAME,
    )
    .await?;
    let mako_geli_gas::GasSupplierChangeState::ValidationPassed(data) = &current else {
        return Err(DispatchError::InvalidPayload(format!(
            "the Gas process on {malo_id} owes no answer (state {})",
            current.status_str()
        )));
    };
    let transaktionsgrund = data.vorgang.transaktionsgrund.clone().unwrap_or_default();
    let stichtag = super::eog_stammdaten::stichtag(&data.process_date)?;
    let record = super::eog_stammdaten::lesen(state, malo_id.as_str(), stichtag).await?;
    let stammdaten = super::eog_stammdaten::gas_44014(
        &record,
        &transaktionsgrund,
        &super::eog_stammdaten::LfEogFakten {
            bilanzkreis: &bilanzkreis,
            abrechnungsintervall_monate,
        },
    )?;

    dispatch_to_process_with_state::<GeliGasSupplierChangeWorkflow, _>(
        state,
        malo_id.as_str(),
        mako_geli_gas::WORKFLOW_NAME,
        move |current| {
            use mako_engine::workflow::Workflow as _;
            let cmd = GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                antwort_code,
                antwort_ebd: Some(ebd.to_owned()),
                bemerkung,
                obligations: vec![],
                stammdaten,
            };
            let out = GeliGasSupplierChangeWorkflow::handle(current, cmd.clone())
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
            super::eog_stammdaten::refuse_what_the_gate_refuses(state, &out.outbox)?;
            Ok(cmd)
        },
    )
    .await
}

/// Resolve `antwort_code` against its Gas Codeliste: the code and whether its
/// Cluster is a Zustimmung.
fn geli_antwortcode(
    payload: &serde_json::Value,
    ebd: &str,
) -> Result<(String, bool), DispatchError> {
    let antwort_code = payload
        .get("antwort_code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "payload must contain \"antwort_code\" — a code from the {ebd} Codeliste, \
                 rendered into SG4 STS+E01 of the answering UTILMD"
            ))
        })?;
    let entry = mako_pruefung::codes::lookup(ebd, antwort_code).ok_or_else(|| {
        DispatchError::InvalidPayload(format!(
            "Antwortcode {antwort_code:?} is not published by {ebd}"
        ))
    })?;
    let zustimmung = entry.ist_zustimmung().ok_or_else(|| {
        DispatchError::InvalidPayload(format!(
            "Antwortcode {antwort_code:?} belongs to {ebd}, whose Cluster is \"{}\" — not the \
             Zustimmung/Ablehnung pair this answer's PID is derived from.",
            entry.cluster.label(),
        ))
    })?;
    Ok((antwort_code.to_owned(), zustimmung))
}

pub(super) fn cmd_geli_eog_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_eog_antwort(s, p))
}

pub(super) fn cmd_geli_eog_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_eog_antwort(s, p))
}

/// Shared body: resolve the Antwortcode against its Gas Codeliste, then answer.
async fn dispatch_geli_lf_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    ebd: &str,
) -> Result<DispatchOutcome, DispatchError> {
    let (antwort_code, zustimmung) = geli_antwortcode(payload, ebd)?;
    answer_gas_supplier_change(state, payload, zustimmung, &antwort_code, Some(ebd)).await
}

/// `ebd` is the tree the code was resolved in, which the event names so the
/// Verlauf reads its wording; `None` leaves it to the process's only tree.
async fn answer_gas_supplier_change(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    accepted: bool,
    antwort_code: &str,
    ebd: Option<&str>,
) -> Result<DispatchOutcome, DispatchError> {
    let antwort_code = antwort_code.to_owned();
    let antwort_ebd = ebd.map(str::to_owned);
    let malo_id = extract_malo_id(payload)?;
    let reason = payload
        .get("bemerkung")
        .or_else(|| payload.get("reason"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    dispatch_to_process::<GeliGasSupplierChangeWorkflow, _>(
        state,
        malo_id.as_str(),
        mako_geli_gas::WORKFLOW_NAME,
        move || GasSupplierChangeCommand::SendAntwort {
            accepted,
            antwort_code: antwort_code.clone(),
            antwort_ebd: antwort_ebd.clone(),
            bemerkung: reason.clone(),
            obligations: vec![],
            stammdaten: mako_engine::payload::Stammdaten::default(),
        },
    )
    .await
}

// ── GeLi Gas Informationsmeldungen (44036 / 44037 / 44038) ────────────────────

pub(super) fn cmd_geli_zuordnung_informieren<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_zuordnungsmeldung(
        s,
        p,
        mako_geli_gas::zuordnungsmeldung::Zuordnungsmeldung::Information,
    ))
}

pub(super) fn cmd_geli_zuordnung_beenden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_zuordnungsmeldung(
        s,
        p,
        mako_geli_gas::zuordnungsmeldung::Zuordnungsmeldung::Beendigung,
    ))
}

pub(super) fn cmd_geli_zuordnung_aufheben<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_zuordnungsmeldung(
        s,
        p,
        mako_geli_gas::zuordnungsmeldung::Zuordnungsmeldung::Aufhebung,
    ))
}

/// Dispatch one of the GNB's three Informationsmeldungen.
///
/// The Gas twin of `dispatch_gpke_zuordnungsmeldung`, with two payload
/// differences: there is no `tranche` flag — every Gas Vorgang names a
/// `SG5 LOC+172` Meldepunkt — and `bilanzierungsende` rides `SG4 DTM+159`,
/// Soll on 44037/44038 „wenn eine Bilanzierung stattfindet" (Bedingung `[29]`).
async fn dispatch_geli_zuordnungsmeldung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    meldung: mako_geli_gas::zuordnungsmeldung::Zuordnungsmeldung,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;
    let receiver = require_payload_str(payload, "empfaenger_mp_id")?;
    let transaktionsgrund = require_payload_str(payload, "transaktionsgrund")?;

    let domain_cmd = mako_geli_gas::zuordnungsmeldung::ZuordnungsmeldungCommand::Senden {
        meldung,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: MarktpartnerCode::new(receiver),
        location_id: malo_id.clone(),
        transaktionsgrund,
        process_date: payload
            .get("process_date")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned),
        bilanzierungsende: payload
            .get("bilanzierungsende")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned),
        beteiligte: beteiligte_marktpartner(payload),
        referenz_vorgangsnummer: payload
            .get("referenz_vorgangsnummer")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned),
    };

    spawn_meldung_process::<mako_geli_gas::GeliGasZuordnungsmeldungWorkflow>(
        state,
        mako_geli_gas::GAS_ZUORDNUNGSMELDUNG_WORKFLOW_NAME,
        &malo_id,
        domain_cmd,
    )
    .await
}

/// The Antwortfrist of the message a Gas Stornierung cancels — the GNB answer
/// window the running Anmeldung, Abmeldung or Kündigung on `malo_id`
/// registered. `None` when that message's PID has no published window.
///
/// # Errors
///
/// [`DispatchError::InvalidPayload`] when no such message is running: a
/// Stornierung of a message already answered is not possible (GeLi Gas 3.0
/// Kap. 2.7), only a manual Rückabwicklung.
async fn original_antwortfrist(
    state: &CommandsApiState,
    malo_id: &str,
) -> Result<Option<time::OffsetDateTime>, DispatchError> {
    use mako_engine::deadline::DeadlineStore as _;
    use mako_geli_gas::{GeliGasLfAnmeldungWorkflow, lf_anmeldung::WORKFLOW_NAME};

    let occupancy =
        find_occupying_process::<GeliGasLfAnmeldungWorkflow>(state, malo_id, WORKFLOW_NAME).await?;
    let Some(process_id) = occupancy.process else {
        return Err(DispatchError::InvalidPayload(format!(
            "für die Gas-MaLo {malo_id} läuft keine unbeantwortete Anmeldung, Abmeldung oder \
             Kündigung — eine Stornierung ist nicht mehr möglich (GeLi Gas 3.0 Kap. 2.7)"
        )));
    };
    let stream = mako_engine::ids::StreamId::for_process(state.tenant_id, &process_id);
    let registered = state
        .store
        .as_deadline_store()
        .for_stream(&stream)
        .await
        .map_err(DispatchError::Engine)?;
    Ok(registered
        .iter()
        .find(|d| d.label() == mako_geli_gas::LF_ANMELDUNG_RESPONSE_WINDOW_LABEL)
        .map(mako_engine::deadline::Deadline::due_at))
}
