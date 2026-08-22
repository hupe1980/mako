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

pub(super) fn cmd_wim_wertebestellung_abbestellung_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_wertebestellung_abbestellung_beantworten(s, p))
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

/// Read the ordered Messprodukt, Wunschtermin and Abo mode out of an
/// `esa.werteanfrage.stellen` payload.
///
/// These are the substance of the request, not decoration: the Messprodukt is
/// `SG27 PIA+5` (restricted by REQOTE AHB 1.2 §4.3 condition `[41]` to Codeliste
/// der Konfigurationen Kapitel 4.6), the Wunschtermin is the `DTM+76` **Muss**,
/// and the Abo mode is the `IMD+7081` **Muss** on the ORDERS that follows.
///
/// `zeitraum_von` doubles as the Wunschtermin: WiM Teil 2 UC 4.1.1 bounds a
/// request to the period the Anschlussnutzer held the location, and that
/// period's start is the earliest the delivery can begin.
fn parse_gegenstand(
    payload: &serde_json::Value,
) -> Result<Box<mako_wim::esa::Bestellgegenstand>, DispatchError> {
    let messprodukt = payload
        .get("messprodukt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "messprodukt is required — a Messprodukt-Code from the Codeliste der \
                 Konfigurationen 1.4 Kapitel 4.6 (e.g. \"9991 00000 305 6\")"
                    .to_owned(),
            )
        })?;
    let wunschtermin = payload
        .get("wunschtermin")
        .or_else(|| payload.get("zeitraum_von"))
        .or_else(|| payload.get("von"))
        .and_then(|v| v.as_str())
        .and_then(parse_iso_date)
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "wunschtermin (or zeitraum_von, YYYY-MM-DD) is required — DTM+76 is Muss on \
                 the REQOTE 35003"
                    .to_owned(),
            )
        })?;
    let zeitraum_bis = payload
        .get("zeitraum_bis")
        .or_else(|| payload.get("bis"))
        .and_then(|v| v.as_str())
        .and_then(parse_iso_date);
    // `IMD+7081`: a subscription (`Z01`) unless the caller asks for a single
    // transmission. Historical requests are naturally one-shots, but the caller
    // states it — guessing from `zeitraum_bis` would silently change which
    // termination path applies (Stornierung vs Abbestellung).
    let abonnement = match payload.get("abonnement").and_then(|v| v.as_str()) {
        None => mako_wim::esa::Abonnement::StartAbo,
        Some(code) => mako_wim::esa::Abonnement::from_imd_code(code)
            .or(match code {
                "abo" | "start_abo" => Some(mako_wim::esa::Abonnement::StartAbo),
                "einmalig" | "ohne_abo" => Some(mako_wim::esa::Abonnement::OhneAbo),
                _ => None,
            })
            .ok_or_else(|| {
                DispatchError::InvalidPayload(format!(
                    "abonnement {code:?} is not an IMD DE 7081 code (Z01 Start Abo, \
                     Z03 ohne Abo)"
                ))
            })?,
    };
    let smgw = match payload.get("smgw").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| {
            DispatchError::InvalidPayload(format!("smgw target is malformed: {e}"))
        })?),
    };
    Ok(Box::new(mako_wim::esa::Bestellgegenstand {
        messprodukt: mako_wim::esa::normalize_code(messprodukt),
        wunschtermin,
        zeitraum_bis,
        abonnement,
        smgw,
    }))
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
    // Parse (and thereby validate) the order before any I/O — a Messprodukt
    // outside Kapitel 4.6 is not orderable by this Marktrolle at all.
    let gegenstand = parse_gegenstand(payload)?;
    let msb = resolve_werteanfrage_msb(state, payload, &location).await?;
    let esa = state.sender_party_id.clone();

    esa_outbound_consent_gate(state, &esa, &msb, &location).await?;

    // Duplicate guard, keyed on the (location, Messprodukt) pair: several
    // Kapitel-4.6 products exist for the same Marktlokation and an ESA may
    // subscribe to more than one. An order that was cancelled, ended or refused
    // is terminal, so the ESA may place a new one for the same pair.
    let subscription_key = mako_wim::esa::business_key(&location, &gegenstand.messprodukt);
    if let Some(dup_id) =
        find_occupying_process::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow>(
            state,
            &subscription_key,
            mako_wim::esa_wertebestellung::WORKFLOW_NAME,
        )
        .await?
    {
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: location,
        });
    }

    // The Belegnummer must equal the wire UNH reference the renderer emits:
    // the MSB's QUOTES echoes it in `RFF+AAV`, which is how the Angebot finds
    // its way back to this process.
    let message_ref =
        crate::edifact_renderer::msg_ref_from_uuid(&format!("ESAWA{}", uuid::Uuid::new_v4()));
    let anfrage_key = message_ref.clone();
    let message_ref = MessageRef::new(message_ref);
    let domain_cmd = mako_wim::esa_wertebestellung::EsaWertebestellungCommand::SendWerteanfrage {
        esa: MarktpartnerCode::new(esa),
        msb: MarktpartnerCode::new(msb),
        ebene: esa_ebene(&location),
        lokations_id: location.clone(),
        gegenstand,
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

    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        mako_wim::wertebestellung::ANGEBOT_FRIST_WT,
        mako_fristen::HolidayCalendar::BdewMaKo,
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

    // Index the process under the location *and* the REQOTE's Belegnummer.
    // Only the opening REQOTE is keyed on a location (`ZO-T17`); every answer
    // from here on references a Belegnummer instead.
    let identity = process.identity();
    let registry = state.store.as_process_registry();
    for key in [
        subscription_key.as_str(),
        location.as_str(),
        anfrage_key.as_str(),
    ] {
        let _ = registry
            .register_correlated(state.tenant_id, key, process_id, identity.clone())
            .await;
    }

    Ok(DispatchOutcome::Spawned { process_id })
}

/// `esa.bestellung.beauftragen` — originate ORDERS 17007 (UC 4.1 Nr. 3).
pub(super) async fn dispatch_esa_bestellung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let process_key = esa_process_key(payload, &location);
    // Re-gate: consent can be withdrawn between Anfrage and Bestellung. The
    // parties are optional here (the process already holds them), but when the
    // caller supplies them we enforce the strict gate.
    if let (Some(msb), esa) = (
        payload.get("msb_mp_id").and_then(|v| v.as_str()),
        state.sender_party_id.clone(),
    ) {
        esa_outbound_consent_gate(state, &esa, msb, &location).await?;
    }
    // The Belegnummer must equal the wire UNH reference the renderer emits: the
    // MSB's ORDRSP 19011/19012 echoes it in `RFF+ON` (`ZG-T14`), and a later
    // ORDCHG Stornierung references it in `RFF+ON` too (`ZG-T51`).
    let message_ref =
        crate::edifact_renderer::msg_ref_from_uuid(&format!("ESABE{}", uuid::Uuid::new_v4()));
    dispatch_to_process_keyed::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow, _>(
        state,
        &process_key,
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
    let process_key = esa_process_key(payload, &location);
    let message_ref =
        crate::edifact_renderer::msg_ref_from_uuid(&format!("ESAST{}", uuid::Uuid::new_v4()));
    dispatch_to_process_keyed::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow, _>(
        state,
        &process_key,
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
    let process_key = esa_process_key(payload, &location);
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
        &process_key,
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
    let process_key = esa_process_key(payload, &location);
    let bindungsfrist = payload
        .get("bindungsfrist")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
        .unwrap_or_else(|| time::OffsetDateTime::now_utc() + time::Duration::days(14));
    // `DTM+469` — the earliest start the MSB offers. Defaults inside the
    // workflow to the ESA's Wunschtermin when the MSB can meet it.
    let fruehester_start = payload
        .get("fruehester_start")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        });
    // The Angebot's Belegnummer must equal the wire UNH reference: the ESA's
    // ORDERS echoes it in `RFF+AAG` (`ZG-T24`), so the process is indexed under
    // it for the Bestellung to come.
    let message_ref =
        crate::edifact_renderer::msg_ref_from_uuid(&format!("MSBANG{}", uuid::Uuid::new_v4()));
    let key = message_ref.clone();
    dispatch_to_process_keyed::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &process_key,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        &[key.as_str()],
        move || mako_wim::wertebestellung::WertebestellungCommand::SendAngebot {
            message_ref: MessageRef::new(message_ref),
            bindungsfrist,
            fruehester_start,
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
    let process_key = esa_process_key(payload, &location);
    let reason = esa_answer_reason(payload)
        .ok_or_else(|| DispatchError::InvalidPayload("reason is required".to_owned()))?;
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &process_key,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        move || mako_wim::wertebestellung::WertebestellungCommand::RejectAnfrage { reason },
    )
    .await
}

/// Read the `AJT` DE 4465 Antwortcode an MSB answer command must carry.
///
/// The ESA answers are ORDRSP, where `SG2 AJT` is **Muss** and its code must
/// sit in the named EBD's Zustimmungs- or Ablehnungs-Cluster (ORDRSP AHB 1.1b
/// §4.15, conditions `[17]`/`[18]`). The **cluster picks the answer PID**, so this
/// replaces the old `accept: bool` rather than accompanying it — the two could
/// disagree, and an answer to the market is a binding statement.
///
/// Run [`mako_pruefung::msb::esa`] to obtain the code from the process facts.
fn esa_antwort_code(payload: &serde_json::Value, tree: &str) -> Result<String, DispatchError> {
    payload
        .get("antwort_code")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "antwort_code is required — an AJT DE 4465 code published by {tree}"
            ))
        })
}

/// `wim.wertebestellung.bestellung-beantworten` — MSB answers the Bestellung
/// (ORDRSP 19011/19012, UC 4.1 Nr. 4).
///
/// `antwort_code` is required and comes from `E_0256`
/// ([`mako_pruefung::msb::esa::pruefe_bestellung`]); its Cluster decides
/// whether the answer rides 19011 or 19012.
pub(super) async fn dispatch_wim_wertebestellung_bestellung_beantworten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let process_key = esa_process_key(payload, &location);
    let antwort_code = esa_antwort_code(payload, mako_pruefung::codes::EBD_ESA_BESTELLUNG)?;
    let reason = esa_answer_reason(payload);
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &process_key,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        move || mako_wim::wertebestellung::WertebestellungCommand::AnswerBestellung {
            antwort_code,
            message_ref: MessageRef::new(format!("MSB-RSP-{}", uuid::Uuid::new_v4())),
            reason,
        },
    )
    .await
}

/// `wim.wertebestellung.stornierung-beantworten` — MSB answers the Stornierung
/// (ORDRSP 19013/19014, UC 4.1 Nr. 6). Code from `E_0257`.
pub(super) async fn dispatch_wim_wertebestellung_stornierung_beantworten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let process_key = esa_process_key(payload, &location);
    let antwort_code = esa_antwort_code(payload, mako_pruefung::codes::EBD_ESA_STORNIERUNG)?;
    let reason = esa_answer_reason(payload);
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &process_key,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        move || mako_wim::wertebestellung::WertebestellungCommand::AnswerStornierung {
            antwort_code,
            message_ref: MessageRef::new(format!("MSB-STO-{}", uuid::Uuid::new_v4())),
            reason,
        },
    )
    .await
}

/// `wim.wertebestellung.abbestellung-beantworten` — MSB answers the
/// Abbestellung (ORDRSP 19011/19012, UC 4.3 Nr. 2). Code from `E_0254`.
///
/// Not "bestätigen": `E_0254` publishes four refusals, and one of them (`A01`,
/// „es handelte sich um eine einmalige Übermittlung") is the normal answer to
/// an ESA that used the wrong termination path.
pub(super) async fn dispatch_wim_wertebestellung_abbestellung_beantworten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let location = extract_esa_location(payload)?;
    let process_key = esa_process_key(payload, &location);
    let antwort_code = esa_antwort_code(payload, mako_pruefung::codes::EBD_ESA_BEENDIGUNG)?;
    let reason = esa_answer_reason(payload);
    dispatch_to_process::<mako_wim::wertebestellung::WimWertebestellungWorkflow, _>(
        state,
        &process_key,
        mako_wim::wertebestellung::WORKFLOW_NAME,
        move || mako_wim::wertebestellung::WertebestellungCommand::AnswerAbbestellung {
            antwort_code,
            message_ref: MessageRef::new(format!("MSB-ABB-{}", uuid::Uuid::new_v4())),
            reason,
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
    let process_key = esa_process_key(payload, &location);
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
        &process_key,
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

/// The process key an ESA follow-up command targets.
///
/// A subscription is the (Meldepunkt, Messprodukt) pair, so a location alone is
/// ambiguous once an ESA holds more than one product at a location. Supplying
/// `messprodukt` addresses one of them; omitting it keeps the plain location
/// key, which is unambiguous while only one subscription exists there.
pub(super) fn esa_process_key(payload: &serde_json::Value, location: &str) -> String {
    payload
        .get("messprodukt")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map_or_else(
            || location.to_owned(),
            |m| mako_wim::esa::business_key(location, m),
        )
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

    const MALO: &str = "51238696012"; // 11 chars → Marktlokation
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
