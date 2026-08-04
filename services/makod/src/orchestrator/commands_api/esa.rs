//! ESA Wertebestellung — ESA origination side and MSB-side answers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

pub(super) fn cmd_esa_werteanfrage_stellen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_esa_werteanfrage(s, p))
}

pub(super) fn cmd_esa_bestellung_beauftragen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_esa_bestellung(s, p))
}

pub(super) fn cmd_esa_stornierung_beauftragen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_esa_stornierung(s, p))
}

pub(super) fn cmd_esa_abbestellung_beauftragen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_esa_abbestellung(s, p))
}

pub(super) fn cmd_wim_wertebestellung_liefern<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_wertebestellung_liefern(s, p))
}

pub(super) fn cmd_wim_wertebestellung_anbieten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_wertebestellung_anbieten(s, p))
}

pub(super) fn cmd_wim_wertebestellung_anfrage_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_wertebestellung_anfrage_ablehnen(s, p))
}

pub(super) fn cmd_wim_wertebestellung_bestellung_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_wertebestellung_bestellung_beantworten(s, p))
}

pub(super) fn cmd_wim_wertebestellung_stornierung_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_wertebestellung_stornierung_beantworten(s, p))
}

pub(super) fn cmd_wim_wertebestellung_abbestellung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_wertebestellung_abbestellung_bestaetigen(s, p))
}

// ── ESA Wertebestellung — ESA origination side ────────────────────────────────
//
// This deployment *is* the ESA. It originates the order handshake and is gated
// by the consent registry: §49 Abs. 2 Nr. 9 MsbG makes the ESA a consent-derived
// role, so it may request a location's values only while it holds a valid
// GDPR-Art.-7 Einwilligung. The gate uses the strict `esa_outbound` perspective
// (a missing consent record is no lawful basis). The Abbestellung (the Art. 7(3)
// revocation path) is deliberately *not* gated — it is the act of stopping.

/// Infer the location level from the identifier length (as the wire adapter does).
pub(super) fn esa_ebene(location: &str) -> mako_wim::esa_wertebestellung::Lokationsebene {
    use mako_wim::esa_wertebestellung::Lokationsebene;
    match location.len() {
        33 => Lokationsebene::Messlokation,
        11 => Lokationsebene::Marktlokation,
        _ => Lokationsebene::Netzlokation,
    }
}

/// Enforce the strict `esa_outbound` consent gate before the ESA originates a
/// request. Disabled (allows) when no marktd client is configured.
///
/// Thin boundary wrapper: the fail-closed policy (a blocked decision *and* a
/// failed lookup both reject) lives in [`mako_wim::consent::gate_outbound`].
pub(super) async fn esa_outbound_consent_gate(
    state: &CommandsApiState,
    esa: &str,
    msb: &str,
    location: &str,
) -> Result<(), DispatchError> {
    let Some(marktd) = &state.marktd_client else {
        return Ok(());
    };
    mako_wim::consent::gate_outbound(
        &mako_engine::types::MarktpartnerCode::new(esa),
        &mako_engine::types::MarktpartnerCode::new(msb),
        &mako_engine::types::MaLo::new(location),
        &crate::ingest_dispatcher::MarktdConsentGate { client: marktd },
    )
    .await
    .map_err(DispatchError::InvalidPayload)
}

fn parse_iso_date(s: &str) -> Option<time::Date> {
    time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT).ok()
}

/// How a Werteanfrage's MSB is determined — decided purely from the payload.
#[derive(Debug, PartialEq, Eq)]
enum MsbResolution {
    /// An explicit `msb_mp_id` was supplied — direct address.
    Explicit(String),
    /// Resolve from the per-Messlokation dated MSB timeline for the given period.
    FromTimeline {
        melo: String,
        von: time::Date,
        bis: Option<time::Date>,
    },
}

/// Pure decision (no I/O): how should the Werteanfrage's MSB be determined?
///
/// An explicit `msb_mp_id` wins (direct address). Otherwise — the `WiM` Teil 2
/// UC 4.1.1 **historical** Werteanfrage — the MSB is resolved from marktd's
/// per-Messlokation dated timeline for the requested period. The Messlokation comes from
/// `melo_id`, or from the location itself when it is a Messlokation (33-char ZPB);
/// the period start from `zeitraum_von` (alias `von`), the optional end from
/// `zeitraum_bis` (alias `bis`).
fn plan_werteanfrage_msb(
    payload: &serde_json::Value,
    location: &str,
) -> Result<MsbResolution, DispatchError> {
    if let Some(explicit) = payload.get("msb_mp_id").and_then(|v| v.as_str()) {
        return Ok(MsbResolution::Explicit(explicit.to_owned()));
    }
    let melo = payload
        .get("melo_id")
        .and_then(|v| v.as_str())
        .or_else(|| {
            (esa_ebene(location) == mako_wim::esa_wertebestellung::Lokationsebene::Messlokation)
                .then_some(location)
        })
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "melo_id is required to resolve the responsible MSB from the timeline".to_owned(),
            )
        })?
        .to_owned();
    let von = payload
        .get("zeitraum_von")
        .or_else(|| payload.get("von"))
        .and_then(|v| v.as_str())
        .and_then(parse_iso_date)
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "zeitraum_von (period start, YYYY-MM-DD) is required to resolve the MSB for a \
                 historical Werteanfrage"
                    .to_owned(),
            )
        })?;
    let bis = payload
        .get("zeitraum_bis")
        .or_else(|| payload.get("bis"))
        .and_then(|v| v.as_str())
        .and_then(parse_iso_date);
    Ok(MsbResolution::FromTimeline { melo, von, bis })
}

/// Resolve the Messstellenbetreiber a Werteanfrage is addressed to, doing the
/// marktd timeline I/O for the [`MsbResolution::FromTimeline`] case.
///
/// Resolves at the **start** of the requested period so a request for a past
/// interval reaches the MSB that operated the Messlokation then rather than
/// today's MSB. When `bis` is supplied and the timeline shows a different MSB at
/// the end, the interval spans an MSB change and the caller must split the
/// Werteanfrage per MSB period — resolving to a single MSB would silently
/// mis-address part of the request.
async fn resolve_werteanfrage_msb(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    location: &str,
) -> Result<String, DispatchError> {
    let (melo, von, bis) = match plan_werteanfrage_msb(payload, location)? {
        MsbResolution::Explicit(msb) => return Ok(msb),
        MsbResolution::FromTimeline { melo, von, bis } => (melo, von, bis),
    };
    let Some(marktd) = &state.marktd_client else {
        return Err(DispatchError::InvalidPayload(
            "msb_mp_id is required (no marktd client configured to resolve it from the per-Messlokation \
             MSB timeline)"
                .to_owned(),
        ));
    };
    let msb = marktd
        .get_melo_msb_at(&melo, von)
        .await
        .map_err(|e| DispatchError::InvalidPayload(format!("MSB timeline lookup failed: {e}")))?
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!("no MSB on record for MeLo {melo} at {von}"))
        })?;
    if let Some(bis) = bis {
        let msb_at_bis = marktd.get_melo_msb_at(&melo, bis).await.map_err(|e| {
            DispatchError::InvalidPayload(format!("MSB timeline lookup failed: {e}"))
        })?;
        if msb_at_bis.as_deref() != Some(msb.as_str()) {
            return Err(DispatchError::InvalidPayload(format!(
                "the requested period {von}..={bis} spans an MSB change for MeLo {melo} (start MSB \
                 {msb}, end MSB {}); split the Werteanfrage per MSB period",
                msb_at_bis.as_deref().unwrap_or("none")
            )));
        }
    }
    Ok(msb)
}

/// `esa.werteanfrage.stellen` — originate REQOTE 35003 (UC 4.1 Nr. 1).
pub(super) async fn dispatch_esa_werteanfrage(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = payload
        .get("malo_id")
        .or_else(|| payload.get("lokations_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::InvalidPayload("malo_id is required".to_owned()))?
        .to_owned();
    let msb = resolve_werteanfrage_msb(state, payload, &location).await?;
    let esa = state.sender_party_id.clone();

    esa_outbound_consent_gate(state, &esa, &msb, &location).await?;

    // Duplicate guard — an order that was cancelled, ended or refused is
    // terminal, so the ESA may place a new one. See `find_occupying_process`.
    if let Some(dup_id) =
        find_occupying_process::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow>(
            state,
            &location,
            mako_wim::esa_wertebestellung::WORKFLOW_NAME,
        )
        .await?
    {
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: location,
        });
    }

    let message_ref = MessageRef::new(format!("ESA-WA-{}", uuid::Uuid::new_v4()));
    let domain_cmd = mako_wim::esa_wertebestellung::EsaWertebestellungCommand::SendWerteanfrage {
        esa: MarktpartnerCode::new(esa),
        msb: MarktpartnerCode::new(msb),
        ebene: esa_ebene(&location),
        lokations_id: location.clone(),
        message_ref,
    };

    let workflow_id = WorkflowId::new(
        mako_wim::esa_wertebestellung::WORKFLOW_NAME,
        latest_format_version(),
    );
    let process = mako_engine::process::Process::<
        mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();

    let due_at = mako_engine::fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        mako_wim::wertebestellung::ANGEBOT_FRIST_WT,
        mako_engine::fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id,
        mako_wim::esa_wertebestellung::ANGEBOT_WINDOW_LABEL,
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await?;

    let identity = process.identity();
    let _ = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, &location, process_id, identity)
        .await;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// `esa.bestellung.beauftragen` — originate ORDERS 17007 (UC 4.1 Nr. 3).
pub(super) async fn dispatch_esa_bestellung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    // Re-gate: consent can be withdrawn between Anfrage and Bestellung. The
    // parties are optional here (the process already holds them), but when the
    // caller supplies them we enforce the strict gate.
    if let (Some(msb), esa) = (
        payload.get("msb_mp_id").and_then(|v| v.as_str()),
        state.sender_party_id.clone(),
    ) {
        esa_outbound_consent_gate(state, &esa, msb, &location).await?;
    }
    // The Belegnummer must equal the wire UNH reference the renderer emits, so
    // the MSB's ORDRSP answer (which echoes it in RFF+ACW) correlates back here.
    let message_ref =
        crate::edifact_renderer::msg_ref_from_uuid(&format!("ESABE{}", uuid::Uuid::new_v4()));
    dispatch_to_process_keyed::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow, _>(
        state,
        &location,
        mako_wim::esa_wertebestellung::WORKFLOW_NAME,
        &[message_ref.as_str()],
        || mako_wim::esa_wertebestellung::EsaWertebestellungCommand::SendBestellung {
            message_ref: MessageRef::new(message_ref.clone()),
        },
    )
    .await
}

/// `esa.stornierung.beauftragen` — originate ORDCHG 39002 (UC 4.1 Nr. 5).
pub(super) async fn dispatch_esa_stornierung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let message_ref =
        crate::edifact_renderer::msg_ref_from_uuid(&format!("ESAST{}", uuid::Uuid::new_v4()));
    dispatch_to_process_keyed::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow, _>(
        state,
        &location,
        mako_wim::esa_wertebestellung::WORKFLOW_NAME,
        // The ORDRSP 19013/19014 Storno-Antwort echoes this ORDCHG's Belegnummer.
        &[message_ref.as_str()],
        || mako_wim::esa_wertebestellung::EsaWertebestellungCommand::SendStornierung {
            message_ref: MessageRef::new(message_ref.clone()),
        },
    )
    .await
}

/// `wim.wertebestellung.liefern` — the MSB delivers Typ-2 values to the ESA as
/// outbound MSCONS 13027 (UC 4.2 / §60 Abs. 1 MsbG delivery duty).
///
/// Resumes the MSB-side Wertebestellung process for the MaLo and runs
/// `LiefereWerte`. The workflow refuses the command unless the process holds a
/// confirmed Bestellung (`lieferung_erlaubt`) — so an MSB can neither accept an
/// order it cannot fulfil nor deliver without one. `ProcessNotFound` here means
/// there is no active subscription for the MaLo.
pub(super) async fn dispatch_wim_wertebestellung_liefern(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let reads = payload
        .get("reads")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if reads.as_array().is_none_or(Vec::is_empty) {
        return Err(DispatchError::InvalidPayload(
            "reads must be a non-empty array of interval values".to_owned(),
        ));
    }
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &location,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        move || mako_wim::wertebestellung::WertebestellungCommand::LiefereWerte {
            message_ref: MessageRef::new(format!("ESA-WERTE-{}", uuid::Uuid::new_v4())),
            reads,
        },
    )
    .await
}

// ── MSB-side Wertebestellung answers (drive the MSB half of the handshake) ─────
//
// The ESA originates via the `esa.*` commands; these let an MSB deployment
// answer, so a self-contained loopback (mako as both roles) can run end to end.
// Each resumes the MSB-side `wim-wertebestellung` process for the MaLo.

pub(super) fn esa_answer_reason(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("reason")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `wim.wertebestellung.anbieten` — MSB sends the QUOTES 15003 Angebot (UC 4.1
/// Nr. 2), carrying its Bindungsfrist. `bindungsfrist` (RFC3339) is optional;
/// it defaults to 14 days out.
pub(super) async fn dispatch_wim_wertebestellung_anbieten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let bindungsfrist = payload
        .get("bindungsfrist")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
        .unwrap_or_else(|| time::OffsetDateTime::now_utc() + time::Duration::days(14));
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &location,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        move || mako_wim::wertebestellung::WertebestellungCommand::SendAngebot {
            message_ref: MessageRef::new(format!("MSB-ANG-{}", uuid::Uuid::new_v4())),
            bindungsfrist,
        },
    )
    .await
}

/// `wim.wertebestellung.anfrage-ablehnen` — MSB refuses the Werteanfrage
/// (QUOTES 15003 Ablehnung). `reason` is required.
pub(super) async fn dispatch_wim_wertebestellung_anfrage_ablehnen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let reason = esa_answer_reason(payload)
        .ok_or_else(|| DispatchError::InvalidPayload("reason is required".to_owned()))?;
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &location,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        move || mako_wim::wertebestellung::WertebestellungCommand::RejectAnfrage { reason },
    )
    .await
}

/// `wim.wertebestellung.bestellung-beantworten` — MSB confirms or refuses the
/// Bestellung (ORDRSP 19011/19012, UC 4.1 Nr. 4). `accept` is required; a
/// refusal needs a `reason`.
pub(super) async fn dispatch_wim_wertebestellung_bestellung_beantworten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let accept = payload
        .get("accept")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| DispatchError::InvalidPayload("accept (bool) is required".to_owned()))?;
    let reason = esa_answer_reason(payload);
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &location,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        move || mako_wim::wertebestellung::WertebestellungCommand::AnswerBestellung {
            accept,
            message_ref: MessageRef::new(format!("MSB-RSP-{}", uuid::Uuid::new_v4())),
            reason,
        },
    )
    .await
}

/// `wim.wertebestellung.stornierung-beantworten` — MSB confirms or refuses the
/// Stornierung (ORDRSP 19013/19014, UC 4.1 Nr. 6).
pub(super) async fn dispatch_wim_wertebestellung_stornierung_beantworten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let accept = payload
        .get("accept")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| DispatchError::InvalidPayload("accept (bool) is required".to_owned()))?;
    let reason = esa_answer_reason(payload);
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &location,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        move || mako_wim::wertebestellung::WertebestellungCommand::AnswerStornierung {
            accept,
            message_ref: MessageRef::new(format!("MSB-STO-{}", uuid::Uuid::new_v4())),
            reason,
        },
    )
    .await
}

/// `wim.wertebestellung.abbestellung-bestaetigen` — MSB confirms the Abbestellung
/// (ORDRSP 19011, UC 4.3 Nr. 2).
pub(super) async fn dispatch_wim_wertebestellung_abbestellung_bestaetigen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &location,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        || mako_wim::wertebestellung::WertebestellungCommand::AnswerAbbestellung {
            message_ref: MessageRef::new(format!("MSB-ABB-{}", uuid::Uuid::new_v4())),
        },
    )
    .await
}

/// `esa.abbestellung.beauftragen` — originate ORDERS 17008 (UC 4.3 Nr. 1), the
/// GDPR Art. 7(3) revocation path. Fired by marktd on Widerruf. **Not** gated.
pub(super) async fn dispatch_esa_abbestellung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let grund = payload
        .get("grund")
        .and_then(|v| v.as_str())
        .unwrap_or("einwilligung_widerrufen")
        .to_owned();
    // Delivery stops as soon as the market allows; the ORDRSP confirms the date.
    let beendigung_zum = time::OffsetDateTime::now_utc();
    let message_ref =
        crate::edifact_renderer::msg_ref_from_uuid(&format!("ESAAB{}", uuid::Uuid::new_v4()));
    let key = message_ref.clone();
    dispatch_to_process_keyed::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow, _>(
        state,
        &location,
        mako_wim::esa_wertebestellung::WORKFLOW_NAME,
        // The ORDRSP 19011 confirming the Abbestellung echoes this Belegnummer.
        &[key.as_str()],
        move || mako_wim::esa_wertebestellung::EsaWertebestellungCommand::SendAbbestellung {
            message_ref: MessageRef::new(message_ref),
            beendigung_zum,
            grund,
        },
    )
    .await
}

/// Extract the location (MaLo/MeLo/NeLo) an ESA follow-up command targets.
pub(super) fn extract_esa_location(payload: &serde_json::Value) -> Result<String, DispatchError> {
    payload
        .get("malo_id")
        .or_else(|| payload.get("melo_id"))
        .or_else(|| payload.get("lokations_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| DispatchError::InvalidPayload("malo_id is required".to_owned()))
}

#[cfg(test)]
mod werteanfrage_msb_tests {
    use serde_json::json;

    use super::{MsbResolution, plan_werteanfrage_msb};
    use crate::orchestrator::commands_api::types::DispatchError;

    const MALO: &str = "51238696780"; // 11 chars → Marktlokation
    const MELO: &str = "DE0001234567890123456789012345678"; // 33 chars → Messlokation

    fn date(s: &str) -> time::Date {
        time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT).unwrap()
    }

    /// An explicit `msb_mp_id` short-circuits — no timeline resolution.
    #[test]
    fn explicit_msb_is_honoured() {
        let p = json!({ "malo_id": MALO, "msb_mp_id": "9903666000009" });
        assert_eq!(
            plan_werteanfrage_msb(&p, MALO).unwrap(),
            MsbResolution::Explicit("9903666000009".to_owned())
        );
    }

    /// Without `msb_mp_id`, an explicit `melo_id` + period resolves from the timeline.
    #[test]
    fn melo_and_period_resolve_from_timeline() {
        let p = json!({ "malo_id": MALO, "melo_id": MELO, "zeitraum_von": "2025-03-01", "zeitraum_bis": "2025-03-31" });
        assert_eq!(
            plan_werteanfrage_msb(&p, MALO).unwrap(),
            MsbResolution::FromTimeline {
                melo: MELO.to_owned(),
                von: date("2025-03-01"),
                bis: Some(date("2025-03-31")),
            }
        );
    }

    /// A Messlokation location doubles as the Messlokation when `melo_id` is omitted;
    /// `von` is the alias for `zeitraum_von`.
    #[test]
    fn melo_location_is_used_when_melo_id_absent() {
        let p = json!({ "lokations_id": MELO, "von": "2024-11-15" });
        assert_eq!(
            plan_werteanfrage_msb(&p, MELO).unwrap(),
            MsbResolution::FromTimeline {
                melo: MELO.to_owned(),
                von: date("2024-11-15"),
                bis: None,
            }
        );
    }

    /// A MaLo location with no `melo_id` cannot address the per-Messlokation timeline.
    #[test]
    fn malo_location_without_melo_id_is_rejected() {
        let p = json!({ "malo_id": MALO, "zeitraum_von": "2025-03-01" });
        assert!(matches!(
            plan_werteanfrage_msb(&p, MALO),
            Err(DispatchError::InvalidPayload(_))
        ));
    }

    /// Resolution needs a period start.
    #[test]
    fn missing_period_start_is_rejected() {
        let p = json!({ "melo_id": MELO });
        assert!(matches!(
            plan_werteanfrage_msb(&p, MALO),
            Err(DispatchError::InvalidPayload(_))
        ));
    }
}
