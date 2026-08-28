//! WiM Strom command wrappers and dispatchers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use mako_fristen::antwort::Messtechnik;
use mako_wim::{StoerungsmeldungCommand, WimInsrptWorkflow};

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
    Box::pin(dispatch_wim_antwort(s, p, true))
}

pub(super) fn cmd_wim_geraetewechsel_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_antwort(s, p, false))
}

pub(super) fn cmd_wim_geraetewechsel_aperak<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    let positive = p
        .get("positiv")
        .or_else(|| p.get("positive"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    Box::pin(dispatch_wim_aperak(s, p, positive))
}

pub(super) fn cmd_wim_gesamtvorgang_melden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_gesamtvorgang(s, p))
}

pub(super) fn cmd_wim_zuordnung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_zuordnung(s, p, true))
}

pub(super) fn cmd_wim_zuordnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_zuordnung(s, p, false))
}

pub(super) fn cmd_wim_weiterverpflichtung_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_weiterverpflichtung_beantworten(s, p))
}

pub(super) fn cmd_wim_stoerung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_stoerung_antwort(s, p, true))
}

pub(super) fn cmd_wim_stoerung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_stoerung_antwort(s, p, false))
}

pub(super) fn cmd_wim_stoerung_ergebnis_melden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_stoerung_ergebnisbericht(s, p))
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
        let message_ref = remadv_message_ref(p);
        dispatch_to_process::<WimInvoicWorkflow, _>(s, &invoice_ref, "wim-invoic", || {
            InvoicCommand::SettleInvoice {
                message_ref: message_ref.clone(),
            }
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
        // `SG7 AJT` is Muss on a Nicht-Zahlungsavis, and the tree it names
        // depends on who received the invoice: 31009 is `E_0264` toward an ESA,
        // `E_0566` toward an NB and `E_0210` toward an LF. `invoicd` resolves
        // all three and puts them in the payload.
        let message_ref = remadv_message_ref(p);
        let antwort = remadv_antwort(p);
        dispatch_to_process::<WimInvoicWorkflow, _>(s, &invoice_ref, "wim-invoic", || {
            InvoicCommand::DisputeInvoice {
                message_ref: message_ref.clone(),
                reason: reason.clone(),
                antwort: antwort.clone(),
            }
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
/// The caller supplies `melo_id`, `process_date`, and the counterparty MP-ID
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

    // The counterparty MP-ID cannot be derived from the MeLo alone: for 55039/55042
    // it is the NB, for 55051/55168 it is the nMSB. The ERP knows which.
    let receiver_mp_id = payload
        .get("receiver_mp_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"receiver_mp_id\" (Marktpartner-ID of the counterparty: \
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

    // Duplicate guard — a meter can be changed more than once, so only a
    // device change still in flight blocks. See `find_occupying_process`.
    if let Some(dup_id) = find_occupying_process::<WimDeviceChangeWorkflow>(
        state,
        melo_id.as_str(),
        mako_wim::WORKFLOW_NAME,
    )
    .await?
    {
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
    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        frist_wt,
        mako_fristen::HolidayCalendar::BdewMaKo,
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

/// Dispatch the **business** Bestätigung or Ablehnung on a WiM MSB-Wechsel
/// process, looked up by `melo_id`.
///
/// Called for `wim.geraetewechsel.bestaetigen` and `wim.geraetewechsel.ablehnen`.
///
/// | Payload field | Required | Meaning |
/// |---|---|---|
/// | `melo_id` | yes | Business key of the process being answered |
/// | `antwortcode` | on Ablehnung | `SG4 STS+E01` DE 9013, from the process's EBD |
/// | `bemerkung` | no | `FTX+ACB` free text |
/// | `abweichender_termin` | with `Z01`/`Z12`/`Z14` | The date the answer confirms |
///
/// The **technical** APERAK is a separate command on a separate clock
/// (`wim.geraetewechsel.aperak`, 45 minutes): it says the message could be
/// processed and decides nothing.
///
/// On a Bestätigung the Antwortcode defaults to the tree's unconditional
/// Zustimmung (`E15` for the three UTILMD trees, `Z13` for the ORDRSP ones); an
/// Ablehnung has no defensible default and must name its ground.
pub(super) async fn dispatch_wim_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    bestaetigt: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let melo_id = extract_melo_id(payload)?;

    // `antwortcode` is the field name every other answering command in this
    // API uses; `reason` stays accepted as the Bemerkung because processd and
    // the ERP already send it.
    let antwortcode = payload
        .get("antwortcode")
        .or_else(|| payload.get("antwort_code"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let bemerkung = payload
        .get("bemerkung")
        .or_else(|| payload.get("reason"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let abweichender_termin = payload
        .get("abweichender_termin")
        .and_then(|v| v.as_str())
        .map(normalise_process_date);

    if !bestaetigt && antwortcode.is_none() {
        return Err(DispatchError::InvalidPayload(
            "an Ablehnung must carry \"antwortcode\" — the ground is a code from the              process's Entscheidungsbaum (E_0200 / E_0201 / E_0202 / E_0240), and no              default is defensible"
                .to_owned(),
        ));
    }

    // The default Zustimmung depends on which process is being answered, and
    // only the process knows its PID — so resolve it inside the closure
    // against the loaded state rather than guessing here.
    dispatch_to_process_with_state::<WimDeviceChangeWorkflow, _>(
        state,
        melo_id.as_str(),
        mako_wim::WORKFLOW_NAME,
        move |st| {
            let code = match antwortcode.clone() {
                Some(c) => c,
                None => default_zustimmung_for(st).ok_or_else(|| {
                    DispatchError::InvalidPayload(
                        "the process is not in a state that owes an answer, so no default                          Antwortcode could be resolved"
                            .to_owned(),
                    )
                })?,
            };
            Ok(DeviceChangeCommand::DispatchAntwort {
                bestaetigt,
                antwort_code: code,
                bemerkung: bemerkung.clone(),
                abweichender_termin: abweichender_termin.clone(),
            })
        },
    )
    .await
}

/// The unconditional Zustimmung code for whichever MSB-Wechsel process this
/// stream is running.
fn default_zustimmung_for(state: &mako_wim::DeviceChangeState) -> Option<String> {
    use mako_wim::DeviceChangeState as S;
    let data = match state {
        S::ValidationPassed(d) | S::AperakSent(d) => d,
        _ => return None,
    };
    let ebd = mako_wim::geraetewechsel::wim_ebd(data.pruefidentifikator.as_u32())?;
    // Named on the tree rather than inferred: `E_0202` publishes both `E15`
    // and `Z01` as Muss with no Bedingung, and picking `Z01` for a plain
    // acceptance asserts a Terminänderung that did not happen.
    let code = match ebd {
        mako_pruefung::codes::EBD_WEITERVERPFLICHTUNG => "Z13",
        _ => "E15",
    };
    mako_pruefung::codes::lookup(ebd, code).map(|c| c.code.to_owned())
}

/// `YYYY-MM-DD` or `YYYYMMDD` in, `YYYYMMDD` out.
fn normalise_process_date(raw: &str) -> String {
    raw.chars().filter(char::is_ascii_digit).collect()
}

/// Report the outcome of the Gesamtvorgang as the **MSBN**
/// (`wim.gesamtvorgang.melden`, IFTSTA 21010 / 21009).
///
/// | Payload field | Required | Meaning |
/// |---|---|---|
/// | `melo_id` | yes | Business key of the Beginn-Messstellenbetrieb process |
/// | `erfolgreich` | no (default `true`) | 21010 vs. 21009 |
/// | `zuordnungsbeginn` | on success | `SG15 DTM+2380`, the day the MSBN takes over |
///
/// The date must lie inside the ±9-Werktage Realisierungskorridor around the
/// Zuordnungsbeginn the NB confirmed; the workflow refuses one that does not.
pub(super) async fn dispatch_wim_gesamtvorgang(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let melo_id = extract_melo_id(payload)?;
    let erfolgreich = payload
        .get("erfolgreich")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let zuordnungsbeginn = payload
        .get("zuordnungsbeginn")
        .and_then(|v| v.as_str())
        .map(normalise_process_date);
    if erfolgreich && zuordnungsbeginn.is_none() {
        return Err(DispatchError::InvalidPayload(
            "an erfolgreicher Gesamtvorgang must carry \"zuordnungsbeginn\" — the NB assigns \
             the MSBN from that day, 00:00 Uhr (WiM Teil 1 Kap. 2.1.1)"
                .to_owned(),
        ));
    }
    dispatch_to_process::<WimDeviceChangeWorkflow, _>(
        state,
        melo_id.as_str(),
        mako_wim::WORKFLOW_NAME,
        move || DeviceChangeCommand::MeldeGesamtvorgang {
            erfolgreich,
            zuordnungsbeginn: zuordnungsbeginn.clone(),
        },
    )
    .await
}

/// Decide the Zuordnung as the **NB** (IFTSTA 21012 / 21011).
///
/// This is the constitutive step: on `zugeordnet` the MSBN is assigned from the
/// date it reported and the MSBA's assignment ends at the same instant.
pub(super) async fn dispatch_wim_zuordnung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    zugeordnet: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let melo_id = extract_melo_id(payload)?;
    dispatch_to_process::<WimDeviceChangeWorkflow, _>(
        state,
        melo_id.as_str(),
        mako_wim::WORKFLOW_NAME,
        move || DeviceChangeCommand::DispatchZuordnung { zugeordnet },
    )
    .await
}

/// Dispatch the technical APERAK on a WiM MSB-Wechsel process.
///
/// Separate from [`dispatch_wim_antwort`] on purpose: two messages, two
/// Fristen (45 min vs. 3/5/7/1 Werktage), two decisions.
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
/// REQOTE Preisanfrage (35001/35002/35004/35005) with the QUOTES Angebot (15001/15002/15004/15005).
/// The response PID is derived inside the workflow from the stored REQOTE PID;
/// the price content comes from the aMSB's current PreisblattMessung.
/// Dispatch the MSBA's ORDRSP answer to an inbound Weiterverpflichtungsauftrag
/// (ORDERS 17002).
///
/// | Payload field | Required | Meaning |
/// |---|---|---|
/// | `melo_id` | yes | Business key of the process being answered |
/// | `bestaetigtes_zuordnungsende` | for the cap check | The Zuordnungsende the NB confirmed on the Abmeldung |
/// | `abmeldegrund` | for the cap check | `anschlussnutzerwechsel` (3 Monate) · `vertragsende` · `ausserbetriebnahme` (1 Monat) |
/// | `bereits_ausgeschoepft` | no | `true` on a *further* order after the maximum was reached |
/// | `antwortcode` | when the cap check cannot run | `Z13` · `Z14` · `Z22` from the Sparte's tree |
/// | `abweichender_termin` | with `Z14`/`Z22` | The corrected date the answer names |
///
/// The Antwortcode is a computation, not a choice:
/// [`mako_pruefung::msb::pruefe_weiterverpflichtung`] measures the requested
/// date against „längstens drei Monate" resp. „längstens einen Monat" from the
/// confirmed Zuordnungsende (WiM Teil 1 Kap. 2.4.2 Nr. 4). `Z13` asserts the
/// request is inside that cap, so the command refuses to answer without either
/// the cap inputs or an explicit code.
pub(super) async fn dispatch_wim_weiterverpflichtung_beantworten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let melo_id = extract_melo_id(payload)?;
    let explicit = payload
        .get("antwortcode")
        .or_else(|| payload.get("antwort_code"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let explicit_termin = payload
        .get("abweichender_termin")
        .and_then(|v| v.as_str())
        .map(normalise_process_date);
    let bestaetigtes_ende = payload
        .get("bestaetigtes_zuordnungsende")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let grund = abmeldegrund_from(payload)?;
    let bereits_ausgeschoepft = payload
        .get("bereits_ausgeschoepft")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    dispatch_to_process_with_state::<mako_wim::WimWeiterverpflichtungWorkflow, _>(
        state,
        melo_id.as_str(),
        mako_wim::weiterverpflichtung::WORKFLOW_NAME,
        move |st| {
            let mako_wim::weiterverpflichtung::WeiterverpflichtungState::AuftragEmpfangen(data) =
                st
            else {
                return Err(DispatchError::InvalidPayload(format!(
                    "the Weiterverpflichtung process is in state {} and owes no answer",
                    st.label()
                )));
            };
            match (bestaetigtes_ende.as_deref(), grund) {
                (Some(ende), Some(grund)) => {
                    let auftrag = mako_pruefung::msb::types::WeiterverpflichtungAuftrag {
                        melo_id: data.melo_id.as_str().to_owned(),
                        bestaetigtes_zuordnungsende: parse_process_date(ende)?,
                        verschobenes_zuordnungsende: parse_process_date(
                            &data.verschobenes_zuordnungsende,
                        )?,
                        sparte: match data.sparte {
                            mako_engine::types::Sparte::Gas => {
                                mako_pruefung::msb::types::Sparte::Gas
                            }
                            mako_engine::types::Sparte::Strom => {
                                mako_pruefung::msb::types::Sparte::Strom
                            }
                        },
                        grund,
                        bereits_ausgeschoepft,
                    };
                    let entscheidung = mako_pruefung::msb::pruefe_weiterverpflichtung(&auftrag);
                    let (code, termin) = weiterverpflichtung_antwort(&entscheidung);
                    Ok(
                        mako_wim::weiterverpflichtung::WeiterverpflichtungCommand::DispatchAntwort {
                            antwort_code: code,
                            abweichender_termin: termin,
                        },
                    )
                }
                _ => {
                    let code = explicit.clone().ok_or_else(|| {
                        DispatchError::InvalidPayload(
                            "supply \"bestaetigtes_zuordnungsende\" and \"abmeldegrund\" so the \
                             Weiterverpflichtungszeitraum can be measured, or name the \
                             \"antwortcode\" (Z13 / Z14 / Z22) explicitly — Z13 is a claim that \
                             the request is inside the cap"
                                .to_owned(),
                        )
                    })?;
                    Ok(
                        mako_wim::weiterverpflichtung::WeiterverpflichtungCommand::DispatchAntwort {
                            antwort_code: code,
                            abweichender_termin: explicit_termin.clone(),
                        },
                    )
                }
            }
        },
    )
    .await
}

/// The Antwortcode and the corrected date a Weiterverpflichtung decision names.
fn weiterverpflichtung_antwort(
    entscheidung: &mako_pruefung::MsbEntscheidung,
) -> (String, Option<String>) {
    let antwort = match entscheidung {
        mako_pruefung::MsbEntscheidung::Accept(d) => Some(d),
        mako_pruefung::MsbEntscheidung::Reject(r) => Some(&r.antwort),
        // The cap is arithmetic on two dates that are both present here, so the
        // tree has no Klärfall to escalate into.
        mako_pruefung::MsbEntscheidung::Escalate { .. } => None,
    };
    antwort.map_or_else(
        || ("Z14".to_owned(), None),
        |a| {
            (
                a.antwortcode.clone(),
                a.abweichender_termin
                    .map(|d| format!("{:04}{:02}{:02}", d.year(), u8::from(d.month()), d.day())),
            )
        },
    )
}

/// Read the Abmeldegrund of the Ende Messstellenbetrieb this Weiterverpflichtung
/// follows — it is what caps the period at three months or one.
fn abmeldegrund_from(
    payload: &serde_json::Value,
) -> Result<Option<mako_pruefung::msb::types::Abmeldegrund>, DispatchError> {
    use mako_pruefung::msb::types::Abmeldegrund;
    match payload.get("abmeldegrund").and_then(|v| v.as_str()) {
        None => Ok(None),
        Some("anschlussnutzerwechsel") => Ok(Some(Abmeldegrund::AnschlussnutzerWechsel)),
        Some("vertragsende") => Ok(Some(Abmeldegrund::VertragsEnde)),
        Some("ausserbetriebnahme") => Ok(Some(Abmeldegrund::Ausserbetriebnahme)),
        Some(other) => Err(DispatchError::InvalidPayload(format!(
            "\"abmeldegrund\" must be one of anschlussnutzerwechsel, vertragsende, \
             ausserbetriebnahme; got {other:?}"
        ))),
    }
}

/// `YYYY-MM-DD` or `YYYYMMDD` in, a `Date` out.
fn parse_process_date(raw: &str) -> Result<time::Date, DispatchError> {
    let compact = normalise_process_date(raw);
    let bytes = compact.as_bytes();
    if bytes.len() != 8 {
        return Err(DispatchError::InvalidPayload(format!(
            "expected a date as YYYY-MM-DD or YYYYMMDD, got {raw:?}"
        )));
    }
    let num = |from: usize, to: usize| compact[from..to].parse::<u32>().ok();
    let (Some(y), Some(m), Some(d)) = (num(0, 4), num(4, 6), num(6, 8)) else {
        return Err(DispatchError::InvalidPayload(format!(
            "expected a date as YYYY-MM-DD or YYYYMMDD, got {raw:?}"
        )));
    };
    let month = u8::try_from(m)
        .ok()
        .and_then(|m| time::Month::try_from(m).ok())
        .ok_or_else(|| DispatchError::InvalidPayload(format!("{raw:?} names no month")))?;
    i32::try_from(y)
        .ok()
        .and_then(|y| u8::try_from(d).ok().map(|d| (y, d)))
        .and_then(|(y, d)| time::Date::from_calendar_date(y, month, d).ok())
        .ok_or_else(|| DispatchError::InvalidPayload(format!("{raw:?} is not a calendar date")))
}

/// Dispatch the MSB's answer to an inbound INSRPT Störungsmeldung.
///
/// `messtechnik` is the caller's, because it is the MSB's own device registry
/// that knows it and it is what sizes the Ergebnisfrist the Bestätigung opens
/// (WiM Strom Teil 2 Kap. 1.2 Nr. 7). Absent, the fastest branch applies: a
/// window that closes early is visible, one that closes late is not.
pub(super) async fn dispatch_wim_stoerung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    bestaetigung: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let melo_id = extract_melo_id(payload)?;
    let status_code = payload
        .get("status_code")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let messtechnik = messtechnik_from(payload)?;
    let pid = Pruefidentifikator::new(if bestaetigung { 23_004 } else { 23_003 })
        .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
    dispatch_to_process::<WimInsrptWorkflow, _>(
        state,
        melo_id.as_str(),
        mako_wim::insrpt::WORKFLOW_NAME,
        move || StoerungsmeldungCommand::DispatchAntwort {
            pid,
            status_code,
            message_ref: MessageRef::new(format!("WIM-INSRPT-{}", uuid::Uuid::new_v4())),
            sent_at: time::OffsetDateTime::now_utc(),
            messtechnik,
        },
    )
    .await
}

/// Dispatch the MSB's INSRPT 23008 Ergebnisbericht, which closes the Use-Case.
pub(super) async fn dispatch_wim_stoerung_ergebnisbericht(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let melo_id = extract_melo_id(payload)?;
    let status_code = payload
        .get("status_code")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    dispatch_to_process::<WimInsrptWorkflow, _>(
        state,
        melo_id.as_str(),
        mako_wim::insrpt::WORKFLOW_NAME,
        move || StoerungsmeldungCommand::DispatchErgebnisbericht {
            message_ref: MessageRef::new(format!("WIM-INSRPT-{}", uuid::Uuid::new_v4())),
            status_code,
        },
    )
    .await
}

/// Read the Messtechnik at the Messlokation from the payload.
///
/// Accepted values are the three WiM Teil 2 branches; anything else is refused
/// rather than defaulted, because guessing picks a Frist.
fn messtechnik_from(payload: &serde_json::Value) -> Result<Messtechnik, DispatchError> {
    match payload.get("messtechnik").and_then(|v| v.as_str()) {
        None | Some("rlm-oder-ims-ms-hs") => Ok(Messtechnik::RlmOderImsMsHs),
        Some("kme-ohne-rlm") => Ok(Messtechnik::KmeOhneRlm),
        Some("rlm-oder-ims-ns") => Ok(Messtechnik::RlmOderImsNs),
        Some(other) => Err(DispatchError::InvalidPayload(format!(
            "\"messtechnik\" must be one of kme-ohne-rlm, rlm-oder-ims-ns, \
             rlm-oder-ims-ms-hs; got {other:?}"
        ))),
    }
}

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
                                        // BO4E `Konfigurationsprodukt.produktcode`; the
                                        // camelCase/snake_case variants are legacy fallbacks.
                                        p.get("produktcode")
                                            .or_else(|| p.get("produktCode"))
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

// ── WiM Rechnungsabwicklung MSB über LF ──────────────────────────────────────

pub(super) fn cmd_wim_rechnungsabwicklung_beenden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_rechnungsabwicklung_beenden(s, p))
}

pub(super) fn cmd_wim_rechnungsabwicklung_zustimmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_rechnungsabwicklung_antwort(s, p, true))
}

pub(super) fn cmd_wim_rechnungsabwicklung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_rechnungsabwicklung_antwort(s, p, false))
}

/// Spawn an outbound Beendigung Rechnungsabwicklung (ORDERS 17006).
///
/// Either side of the arrangement may end it (AWH Aktivitätsdiagramme WiM V1.3
/// §§2.9/2.11), so the sending role is whatever this deployment is; the
/// counterparty MP-ID comes from the ERP, which knows whom the arrangement is
/// with. The counterparty answers with ORDRSP 19009/19010, which resumes this
/// process by MaLo.
pub(super) async fn dispatch_wim_rechnungsabwicklung_beenden(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_wim::{RechnungsabwicklungCommand, WimRechnungsabwicklungWorkflow};

    let malo_id = extract_malo_id(payload)?;
    let counterparty = payload
        .get("counterparty_mp_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"counterparty_mp_id\" (Marktpartner-ID of the \
                 other side of the arrangement — the MSB when the LF ends it, \
                 the LF when the MSB does)"
                    .to_owned(),
            )
        })?
        .to_owned();

    let domain_cmd = RechnungsabwicklungCommand::SendBeendigung {
        counterparty: MarktpartnerCode::new(counterparty),
        location_id: malo_id.to_string(),
        message_ref: MessageRef::new(format!("WIM-RA-{}", uuid::Uuid::new_v4())),
    };

    // Duplicate guard — one Beendigung in flight per MaLo. A settled process
    // (Bestellt/Beendet/Rejected) does not block; see `find_occupying_process`.
    if let Some(dup_id) = find_occupying_process::<WimRechnungsabwicklungWorkflow>(
        state,
        malo_id.as_str(),
        mako_wim::RECHNUNGSABWICKLUNG_WORKFLOW_NAME,
    )
    .await?
    {
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: malo_id.into(),
        });
    }

    let workflow_id = WorkflowId::new(
        mako_wim::RECHNUNGSABWICKLUNG_WORKFLOW_NAME,
        latest_format_version(),
    );
    let process = mako_engine::process::Process::<
        WimRechnungsabwicklungWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();

    // The counterparty's answer window: the WiM Teil 1 process window the
    // sibling workflows use (5 Werktage, BK6-24-174).
    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        5,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id,
        mako_wim::RECHNUNGSABWICKLUNG_DEADLINE_LABEL,
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

/// Answer a received Beendigung (ORDRSP 19009 Zustimmung / 19010 Ablehnung).
///
/// Called for `wim.rechnungsabwicklung.zustimmen` / `.ablehnen` — the decision
/// the counterparty's EBD (`E_0206`/`E_0209`) checks is the operator's to
/// make, so it arrives here rather than being auto-echoed.
pub(super) async fn dispatch_wim_rechnungsabwicklung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    zustimmung: bool,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_wim::{RechnungsabwicklungCommand, WimRechnungsabwicklungWorkflow};

    let malo_id = extract_malo_id(payload)?;
    dispatch_to_process::<WimRechnungsabwicklungWorkflow, _>(
        state,
        malo_id.as_str(),
        mako_wim::RECHNUNGSABWICKLUNG_WORKFLOW_NAME,
        move || RechnungsabwicklungCommand::SendAntwort {
            zustimmung,
            message_ref: MessageRef::new(format!("WIM-RA-RSP-{}", uuid::Uuid::new_v4())),
        },
    )
    .await
}
