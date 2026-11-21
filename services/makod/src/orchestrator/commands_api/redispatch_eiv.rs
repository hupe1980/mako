//! The Einsatzverantwortliche's doors — Redispatch 2.0 from the plant's side.
//!
//! Each sending door takes the document the EIV's own systems produced, parses
//! and validates it through `redispatch-xml`, holds its roles against the
//! Anwendungstabelle — sent by the EIV (`A27`) to the Data Provider (`A39`) —
//! and records the sending with the DP's acknowledgement window armed in the
//! same write. The body is `{ "dokument": "<xml>" }`; the answer names the
//! process.
//!
//! The release door answers an activation. It is the only way an activation
//! is answered, and it refuses a caller that is not a person: the command's
//! actor is stamped on the event, and an [`Akteur::System`] actor is refused
//! before anything is written.

use super::*;

use mako_redispatch::eiv;
use mako_redispatch::eiv::abruf::RUECKMELDUNG_ACK_WINDOW_LABEL as EIV_RUECKMELDUNG_ACK_WINDOW;

/// The validated document a door was given, with the facts the workflow
/// records.
struct EivDokument {
    sendung: eiv::Sendung,
    receiver: String,
}

/// Parse and validate `payload["dokument"]` as a document of `erwartet`,
/// sent by the EIV to the Data Provider.
fn eiv_dokument(
    payload: &serde_json::Value,
    erwartet: redispatch_xml::DocumentType,
    felder: &[&str],
) -> Result<(redispatch_xml::Document, EivDokument), DispatchError> {
    let xml = payload
        .get("dokument")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| DispatchError::InvalidPayload("dokument (XML) is required".to_owned()))?;
    if let Some(feld) = payload
        .as_object()
        .and_then(|o| o.keys().find(|k| !felder.contains(&k.as_str())))
    {
        return Err(DispatchError::InvalidPayload(format!(
            "unknown field {feld:?}"
        )));
    }
    let doc = redispatch_xml::parse_and_validate(xml.as_bytes())
        .map_err(|e| DispatchError::InvalidPayload(format!("dokument: {e}")))?;
    if doc.document_type() != erwartet {
        return Err(DispatchError::InvalidPayload(format!(
            "dokument is a {:?}, this command sends a {erwartet:?}",
            doc.document_type()
        )));
    }
    if doc.sender_role() != eiv::rollen::EIV {
        return Err(DispatchError::InvalidPayload(format!(
            "dokument names SenderRole {}, the EIV sends as {}",
            doc.sender_role(),
            eiv::rollen::EIV
        )));
    }
    if doc.receiver_role() != eiv::rollen::DATA_PROVIDER {
        return Err(DispatchError::InvalidPayload(format!(
            "dokument names ReceiverRole {}, the EIV sends to the Data Provider ({})",
            doc.receiver_role(),
            eiv::rollen::DATA_PROVIDER
        )));
    }
    let sendung = eiv::Sendung {
        mrid: doc.mrid().to_owned(),
        dokumenttyp: doc.type_code().unwrap_or_default().to_owned(),
        sender: doc.sender_id().to_owned(),
        empfaenger: doc.receiver_id().to_owned(),
        gesendet_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    };
    let receiver = sendung.empfaenger.clone();
    Ok((doc, EivDokument { sendung, receiver }))
}

/// Record one sending under its own mRID, with the DP's acknowledgement window.
async fn sende<W>(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    erwartet: redispatch_xml::DocumentType,
    workflow_name: &'static str,
    ack_label: &'static str,
) -> Result<DispatchOutcome, DispatchError>
where
    W: mako_engine::workflow::Workflow<Command = eiv::SendungCommand>,
    Arc<mako_engine::store_slatedb::SlateDbStore>: mako_engine::event_store::AtomicAppend,
{
    let (_, dok) = eiv_dokument(payload, erwartet, &["dokument"])?;
    tracing::info!(
        workflow = workflow_name,
        mrid = %dok.sendung.mrid,
        dp = %dok.receiver,
        "redispatch EIV: sending recorded",
    );
    let mrid = dok.sendung.mrid.clone();
    let process_id = spawn_process::<W>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(workflow_name, latest_format_version()),
        eiv::SendungCommand::Senden(dok.sendung),
        &[(
            ack_label,
            time::OffsetDateTime::now_utc() + mako_redispatch::fristen::ACK_FRIST,
        )],
        &[mrid.as_str()],
    )
    .await?;
    Ok(DispatchOutcome::Spawned { process_id })
}

macro_rules! sende_door {
    ($cmd:ident, $dispatch:ident, $workflow:ty, $module:ident, $doctype:ident) => {
        pub(super) fn $cmd<'a>(
            s: &'a CommandsApiState,
            p: &'a serde_json::Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin($dispatch(s, p))
        }

        async fn $dispatch(
            state: &CommandsApiState,
            payload: &serde_json::Value,
        ) -> Result<DispatchOutcome, DispatchError> {
            sende::<$workflow>(
                state,
                payload,
                redispatch_xml::DocumentType::$doctype,
                eiv::$module::WORKFLOW_NAME,
                eiv::$module::ACK_WINDOW_LABEL,
            )
            .await
        }
    };
}

sende_door!(
    cmd_redispatch_eiv_planungsdaten_senden,
    dispatch_eiv_planungsdaten,
    eiv::PlanungsdatenSendungWorkflow,
    planungsdaten,
    PlannedResourceSchedule
);
sende_door!(
    cmd_redispatch_eiv_nichtbeanspruchbarkeit_senden,
    dispatch_eiv_nichtbeanspruchbarkeit,
    eiv::NichtbeanspruchbarkeitSendungWorkflow,
    nichtbeanspruchbarkeit,
    Unavailability
);
sende_door!(
    cmd_redispatch_eiv_stammdaten_senden,
    dispatch_eiv_stammdaten,
    eiv::StammdatenSendungWorkflow,
    stammdaten,
    Stammdaten
);
sende_door!(
    cmd_redispatch_eiv_kostenblatt_senden,
    dispatch_eiv_kostenblatt,
    eiv::KostenblattSendungWorkflow,
    kostenblatt,
    Kostenblatt
);

pub(super) fn cmd_redispatch_eiv_abruf_freigeben<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_eiv_abruf_freigeben(s, p))
}

/// The principal of a person's command; an unattended caller is refused.
fn freigebende_person() -> Result<String, DispatchError> {
    match befehls_akteur()? {
        Akteur::Person { principal } | Akteur::Freigabe { principal, .. } => Ok(principal),
        Akteur::System => Err(DispatchError::InvalidPayload(
            "an activation is answered only on a person's release — this command came \
             with no person as its actor"
                .to_owned(),
        )),
    }
}

/// Release the Rückmeldung zur Umsetzbarkeit for the activation
/// `payload["abruf_mrid"]`: `payload["dokument"]` is the `ActivationDocument`
/// `A41` the person reviewed.
async fn dispatch_eiv_abruf_freigeben(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let principal = freigebende_person()?;
    let abruf_mrid = payload
        .get("abruf_mrid")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| DispatchError::InvalidPayload("abruf_mrid is required".to_owned()))?
        .to_owned();
    let (doc, dok) = eiv_dokument(
        payload,
        redispatch_xml::DocumentType::Activation,
        &["dokument", "abruf_mrid"],
    )?;
    if doc.type_code() != Some(eiv::abruf::RUECKMELDUNG_DOKUMENTTYP) {
        return Err(DispatchError::InvalidPayload(format!(
            "dokument is DocumentType {:?}; the Rückmeldung zur Umsetzbarkeit is {}",
            doc.type_code().unwrap_or_default(),
            eiv::abruf::RUECKMELDUNG_DOKUMENTTYP
        )));
    }
    let current =
        current_state::<eiv::AbrufWorkflow>(state, &abruf_mrid, eiv::abruf::WORKFLOW_NAME).await?;
    let Some(akte) = current.akte() else {
        return Err(DispatchError::ProcessNotFound {
            business_key: abruf_mrid,
            workflow_name: eiv::abruf::WORKFLOW_NAME,
        });
    };
    // The answer goes back the way the activation came: from the EIV it was
    // addressed to, to the DP that sent it.
    if dok.sendung.sender != akte.eingang.empfaenger || dok.receiver != akte.eingang.sender {
        return Err(DispatchError::InvalidPayload(format!(
            "the Rückmeldung runs {} → {}, the activation came {} → {}",
            dok.sendung.sender, dok.receiver, akte.eingang.sender, akte.eingang.empfaenger
        )));
    }

    let identity = find_identity(state, &abruf_mrid, eiv::abruf::WORKFLOW_NAME).await?;
    let process_id = identity.process_id;
    let process = mako_engine::process::Process::<
        eiv::AbrufWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::from_identity(Arc::clone(&state.store), identity.clone());
    let command = eiv::AbrufCommand::Freigeben(eiv::Freigabe {
        rueckmeldung_mrid: dok.sendung.mrid.clone(),
        freigegeben_von: principal,
        freigegeben_at: dok.sendung.gesendet_at.clone(),
    });
    if probe::active() {
        return rehearse_resume::<eiv::AbrufWorkflow>(
            &current,
            &identity.workflow_id.format_version,
            command,
        );
    }
    // The DP's acknowledgement of the Rückmeldung finds this process by the
    // Rückmeldung's mRID, so the key lands in the same write as the release.
    let keys = [mako_engine::event_store::CorrelationEntry {
        tenant_id: state.tenant_id,
        tag: dok.sendung.mrid.clone(),
        process_id,
        identity: identity.clone(),
    }];
    let deadlines = [mako_engine::deadline::Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        identity.workflow_id.clone(),
        EIV_RUECKMELDUNG_ACK_WINDOW,
        time::OffsetDateTime::now_utc() + mako_redispatch::fristen::ACK_FRIST,
    )];
    process
        .run(
            command,
            befehls_optionen()?
                .deadlines(&deadlines)
                .correlations(&keys)
                .snapshot(&state.snapshot_store, state.snapshot_interval),
        )
        .await?;
    Ok(DispatchOutcome::Dispatched { process_id })
}
