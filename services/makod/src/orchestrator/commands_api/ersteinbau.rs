//! WiM Strom Teil 1 Kap. 3.5 — Ersteinbau eines iMS in eine bestehende
//! Messlokation, the two outbound legs (IFTSTA 21029, 21030/21031).
//!
//! The gMSB opens the Vorgang with its Vorabinformation 21029 to the wMSB and
//! waits for the `E_0233` answer; the wMSB answers an inbound 21029 with 21030
//! (Zustimmung) or 21031 (Bestandsschutz / Eigenausbau). Both are keyed on the
//! **Messlokation** — the `LOC+172` the inbound leg is resumed by
//! (`extract_malo_from_msg`).

use mako_wim::ersteinbau::{ErsteinbauCommand, WimErsteinbauWorkflow};

use super::*;

/// Body of `wim.ersteinbau.vorabinformation-senden`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VorabinformationBody {
    /// The Messlokation the iMS is to be fitted at.
    melo_id: String,
    /// MP-ID of the wettbewerblicher MSB operating it.
    wmsb_mp_id: String,
    /// The planned Umstellungszeitpunkt, `YYYY-MM-DD` or `YYYYMMDD`.
    umstellungszeitpunkt: String,
}

/// Body of `wim.ersteinbau.beantworten`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AntwortBody {
    /// The Messlokation the Vorabinformation named.
    melo_id: String,
    /// The `E_0233` code — `A03` rides 21030, `A01`/`A02`/`A04` ride 21031.
    antwortcode: String,
}

fn parse_body<T: serde::de::DeserializeOwned>(
    payload: &serde_json::Value,
) -> Result<T, DispatchError> {
    serde_json::from_value(payload.clone())
        .map_err(|e| DispatchError::InvalidPayload(format!("payload: {e}")))
}

fn melo(raw: &str) -> Result<MeLo, DispatchError> {
    extract_melo_id(&serde_json::json!({ "melo_id": raw }))
}

/// `YYYY-MM-DD` or `YYYYMMDD` in, the date out.
fn umstellungsdatum(raw: &str) -> Result<time::Date, DispatchError> {
    let compact: String = raw.chars().filter(|c| *c != '-').collect();
    time::Date::parse(
        &compact,
        time::macros::format_description!("[year][month][day]"),
    )
    .map_err(|e| {
        DispatchError::InvalidPayload(format!(
            "\"umstellungszeitpunkt\" {raw:?} is not a date as YYYY-MM-DD or YYYYMMDD: {e}"
        ))
    })
}

pub(super) fn cmd_wim_ersteinbau_vorabinformation_senden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_ersteinbau_vorabinformation(s, p))
}

pub(super) fn cmd_wim_ersteinbau_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_ersteinbau_antwort(s, p))
}

/// The gMSB sends IFTSTA 21029 „Vorabinformation zum Gerätewechsel" to the
/// wMSB and opens the Vorgang the wMSB's 21030/21031 resumes.
///
/// The Vorlauffrist — „spätester ÜT liegt 3 Monate und 3 WT vor dem geplanten
/// Zeitpunkt der Ausstattung der Messlokation" (Kap. 3.5.2 Nr. 1) — is checked
/// here, against today's Berlin date, because the workflow cannot know the
/// date. A Vorabinformation sent too late is refused rather than sent: the wMSB
/// could not lawfully be held to an answer window that ends after the rollout.
///
/// The wMSB's answer window (3 WT nach dem ÜT, Kap. 3.5.2 Nr. 2) is registered
/// in the same write, so the gMSB sees an unanswered Vorabinformation lapse.
async fn dispatch_wim_ersteinbau_vorabinformation(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: VorabinformationBody = parse_body(payload)?;
    let melo_id = melo(&body.melo_id)?;
    let wmsb = body.wmsb_mp_id.trim();
    if wmsb.is_empty() {
        return Err(DispatchError::InvalidPayload(
            "\"wmsb_mp_id\" must name the wettbewerblicher MSB".to_owned(),
        ));
    }
    let umstellung = umstellungsdatum(&body.umstellungszeitpunkt)?;

    let obligation = mako_fristen::vorlauf::vorlauf("wim.vorabinformation-ersteinbau-ims")
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "the Vorabinformation zum Ersteinbau has no Vorlauffrist in \
                 mako_fristen::vorlauf"
                    .to_owned(),
            )
        })?;
    if let mako_fristen::vorlauf::VorlaufVerdict::TooLate {
        earliest_possible, ..
    } = obligation.shape.check(
        mako_fristen::heute(),
        umstellung,
        mako_fristen::HolidayCalendar::BdewMaKo,
    ) {
        return Err(DispatchError::InvalidPayload(format!(
            "the Vorabinformation for an Umstellungszeitpunkt on {umstellung} is late — {} \
             ({}); the earliest Umstellungszeitpunkt still reachable is {earliest_possible}",
            obligation.name, obligation.source
        )));
    }

    let occupancy = find_occupying_process::<WimErsteinbauWorkflow>(
        state,
        melo_id.as_str(),
        mako_wim::ersteinbau::WORKFLOW_NAME,
    )
    .await?;
    if let Some(process_id) = occupancy.process {
        return Err(DispatchError::DuplicateProcess {
            process_id,
            malo_id: melo_id.as_str().to_owned(),
        });
    }

    let now = time::OffsetDateTime::now_utc();
    let due_at =
        mako_fristen::antwort::antwort_deadline(mako_wim::ersteinbau::VORABINFORMATION_PID, now)
            .ok_or_else(|| {
                DispatchError::InvalidPayload(
                    "IFTSTA 21029 has no published answer window in mako_fristen::antwort"
                        .to_owned(),
                )
            })?;
    let command = ErsteinbauCommand::SendVorabinformation {
        gmsb: MarktpartnerCode::new(state.sender_party_id.clone()),
        wmsb: MarktpartnerCode::new(wmsb),
        melo_id: melo_id.clone(),
        umstellungszeitpunkt: format!(
            "{:04}{:02}{:02}",
            umstellung.year(),
            u8::from(umstellung.month()),
            umstellung.day()
        ),
        message_ref: MessageRef::new(format!("WIM-ERSTEINBAU-{}", uuid::Uuid::new_v4())),
    };
    let process_id = spawn_process::<WimErsteinbauWorkflow>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(mako_wim::ersteinbau::WORKFLOW_NAME, latest_format_version()),
        command,
        &[(mako_wim::ersteinbau::ANTWORT_WINDOW_LABEL, due_at)],
        &[melo_id.as_str()],
    )
    .await?;
    drop(occupancy);
    Ok(DispatchOutcome::Spawned { process_id })
}

/// The wMSB answers an inbound 21029 with its `E_0233` code; the code's
/// cluster, not the caller, picks 21030 or 21031.
async fn dispatch_wim_ersteinbau_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: AntwortBody = parse_body(payload)?;
    let melo_id = melo(&body.melo_id)?;
    let antwort_code = body.antwortcode.trim().to_owned();
    dispatch_to_process::<WimErsteinbauWorkflow, _>(
        state,
        melo_id.as_str(),
        mako_wim::ersteinbau::WORKFLOW_NAME,
        move || ErsteinbauCommand::DispatchAntwort { antwort_code },
    )
    .await
}
