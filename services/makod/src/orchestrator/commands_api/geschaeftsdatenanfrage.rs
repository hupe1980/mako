//! GPKE Teil 4 Kap. 3 — Geschäftsdatenanfrage Stammdaten Strom, the three
//! outbound legs (ORDERS 17132, UTILMD 55035/55095/55060, ORDRSP 19101).
//!
//! The LF or MSB opens the process with its ORDERS 17132 and waits for the NB's
//! answer; the NB answers an inbound 17132 with the Stammdaten (UTILMD) or
//! refuses it (ORDRSP 19101). Every message names the Anfrage by the ORDERS'
//! `BGM` DE 1004 — the Anfrage-Nummer — so the two sides are keyed on it:
//! [`anfragender_key`] for the Anfrage this party sent, [`netzbetreiber_key`]
//! (with the Anfragender's MP-ID, because the number is unique only per
//! sender) for one it received.

use mako_gpke::geschaeftsdatenanfrage::{
    ANTWORT_PIDS, Anfragender, GdaAnfrage, GdaCommand, GdaState,
    GpkeGeschaeftsdatenanfrageWorkflow, WORKFLOW_NAME, anfragender_key, netzbetreiber_key,
};

use super::*;

/// Body of `gpke.geschaeftsdatenanfrage.anfragen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnfrageBody {
    /// The Markt- or Messlokation asked about — `SG2 LOC+172`.
    lokation_id: String,
    /// MP-ID of the Netzbetreiber asked.
    nb_mp_id: String,
    /// The Anfrage-Nummer (`BGM` DE 1004, `an..35`); generated when absent.
    #[serde(default)]
    anfrage_nummer: Option<String>,
}

/// Body of `gpke.geschaeftsdatenanfrage.beantworten`. The Stammdaten are
/// marktd's, read at this boundary; the body names the Anfrage and the answer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AntwortBody {
    /// MP-ID of the Anfragender — the 17132's `NAD+MS`.
    anfragender_mp_id: String,
    /// The 17132's `BGM` DE 1004.
    anfrage_nummer: String,
    /// `55035` (verbrauchende MaLo, to a LF), `55095` (erzeugende MaLo, to a
    /// LF) or `55060` (to a MSB).
    antwort_pid: u32,
    /// The day the Stammdaten are read for (`YYYY-MM-DD`); absent = the
    /// Berlin day the answer is sent, which is the day whose Stammdaten the
    /// answer states.
    #[serde(default)]
    stichtag: Option<String>,
}

/// Body of `gpke.geschaeftsdatenanfrage.ablehnen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AblehnungBody {
    /// MP-ID of the Anfragender — the 17132's `NAD+MS`.
    anfragender_mp_id: String,
    /// The 17132's `BGM` DE 1004.
    anfrage_nummer: String,
    /// `LF` or `MSB` — picks `E_0441` or `E_0443`.
    anfragender: String,
    /// The Ablehnungscode of that EBD.
    antwortcode: String,
}

fn parse_body<T: serde::de::DeserializeOwned>(
    payload: &serde_json::Value,
) -> Result<T, DispatchError> {
    serde_json::from_value(payload.clone())
        .map_err(|e| DispatchError::InvalidPayload(format!("payload: {e}")))
}

fn required<'a>(raw: &'a str, field: &str) -> Result<&'a str, DispatchError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DispatchError::InvalidPayload(format!(
            "{field:?} must not be empty"
        )));
    }
    Ok(trimmed)
}

/// The Anfrage-Nummer rides `BGM` DE 1004, `an..35` (ORDERS MIG 1.4c).
fn anfrage_nummer(raw: &str) -> Result<&str, DispatchError> {
    let nummer = required(raw, "anfrage_nummer")?;
    if nummer.chars().count() > 35 {
        return Err(DispatchError::InvalidPayload(format!(
            "\"anfrage_nummer\" {nummer:?} exceeds BGM DE 1004's an..35"
        )));
    }
    Ok(nummer)
}

/// `SG2 LOC+172` DE 3225 — a Marktlokations-ID (`[950]`) or a
/// Zählpunktbezeichnung (`[951]`), ORDERS AHB 1.1b Kap. 4.1.1.1.
fn lokation(raw: &str) -> Result<String, DispatchError> {
    let id = raw.trim();
    if let Ok(malo) = extract_malo_id(&serde_json::json!({ "malo_id": id })) {
        return Ok(malo.as_str().to_owned());
    }
    extract_melo_id(&serde_json::json!({ "melo_id": id }))
        .map(|melo| melo.as_str().to_owned())
        .map_err(|_| {
            DispatchError::InvalidPayload(format!(
                "\"lokation_id\" {id:?} is neither a Marktlokations-ID nor a \
                 Zählpunktbezeichnung (ORDERS AHB 1.1b Kap. 4.1.1.1, [950] ⊻ [951])"
            ))
        })
}

pub(super) fn cmd_gpke_geschaeftsdatenanfrage_anfragen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_anfrage(s, p))
}

pub(super) fn cmd_gpke_geschaeftsdatenanfrage_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_antwort(s, p))
}

pub(super) fn cmd_gpke_geschaeftsdatenanfrage_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_ablehnung(s, p))
}

/// The LF or MSB sends ORDERS 17132 to the NB and opens the process the NB's
/// UTILMD or ORDRSP 19101 resumes.
///
/// The Anfragender holds no Frist of its own here: GPKE Teil 4 Kap. 3.2 Nr. 2
/// binds the NB's answer, and the NB's process registers it.
async fn dispatch_anfrage(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: AnfrageBody = parse_body(payload)?;
    let lokation_id = lokation(&body.lokation_id)?;
    let nb = required(&body.nb_mp_id, "nb_mp_id")?;
    let nummer = match body.anfrage_nummer.as_deref() {
        Some(raw) => anfrage_nummer(raw)?.to_owned(),
        // `GDA` + 32 hex digits: 35 characters, BGM DE 1004's whole width.
        None => format!("GDA{}", uuid::Uuid::new_v4().simple()),
    };
    let key = anfragender_key(&nummer);

    let occupancy =
        find_occupying_process::<GpkeGeschaeftsdatenanfrageWorkflow>(state, &key, WORKFLOW_NAME)
            .await?;
    if let Some(process_id) = occupancy.process {
        return Err(DispatchError::DuplicateProcess {
            process_id,
            malo_id: nummer,
        });
    }

    let command = GdaCommand::SendAnfrage {
        anfrage: GdaAnfrage {
            anfragender: MarktpartnerCode::new(state.sender_party_id.clone()),
            netzbetreiber: MarktpartnerCode::new(nb),
            lokation_id,
            anfrage_nummer: MessageRef::new(nummer),
        },
    };
    let process_id = spawn_process::<GpkeGeschaeftsdatenanfrageWorkflow>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(WORKFLOW_NAME, latest_format_version()),
        command,
        &[],
        &[key.as_str()],
    )
    .await?;
    drop(occupancy);
    Ok(DispatchOutcome::Spawned { process_id })
}

/// The NB answers an inbound 17132 with the Stammdaten marktd holds.
///
/// The Anfrage names the Marktlokation; its UTILMD Stammdaten and its
/// Lokationsbündel are read from marktd on the Stichtag and set down as the
/// answer's column lists them (`gda_stammdaten`). marktd unreadable is a 503,
/// a fact it does not hold a 422 naming it — in both cases nothing is
/// written, the process stays `Offen` and its 1-WT window keeps running.
async fn dispatch_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: AntwortBody = parse_body(payload)?;
    let key = netzbetreiber_key(
        required(&body.anfragender_mp_id, "anfragender_mp_id")?,
        anfrage_nummer(&body.anfrage_nummer)?,
    );
    let antwort_pid = Pruefidentifikator::new(body.antwort_pid).map_err(|e| {
        DispatchError::InvalidPayload(format!("\"antwort_pid\" {}: {e}", body.antwort_pid))
    })?;
    if !ANTWORT_PIDS.contains(&antwort_pid.as_u32()) {
        return Err(DispatchError::InvalidPayload(format!(
            "\"antwort_pid\" {antwort_pid} is no UTILMD answer to a Geschäftsdatenanfrage \
             (one of {ANTWORT_PIDS:?})"
        )));
    }
    let stichtag = match body.stichtag.as_deref() {
        Some(raw) => super::eog_stammdaten::stichtag(raw)?,
        None => mako_fristen::heute(),
    };

    let current =
        super::current_state::<GpkeGeschaeftsdatenanfrageWorkflow>(state, &key, WORKFLOW_NAME)
            .await?;
    let GdaState::Offen(anfrage) = current.offen() else {
        return Err(DispatchError::InvalidPayload(format!(
            "the Geschäftsdatenanfrage {} of {} is not open for an answer (state {})",
            body.anfrage_nummer,
            body.anfragender_mp_id,
            current.status_str()
        )));
    };
    let malo_id =
        extract_malo_id(&serde_json::json!({ "malo_id": anfrage.lokation_id })).map_err(|_| {
            DispatchError::NotImplemented(format!(
                "the Anfrage names the Messlokation {}: reading the Lokationsbündel of a \
                 Messlokation's Marktlokation for the answer is not assembled",
                anfrage.lokation_id
            ))
        })?;

    let sd = super::eog_stammdaten::lesen(state, malo_id.as_str(), stichtag).await?;
    let lb = super::gda_stammdaten::lesen(state, malo_id.as_str(), stichtag).await?;
    let antwort = super::gda_stammdaten::antwort(antwort_pid.as_u32(), &sd, &lb)?;
    let cmd = GdaCommand::SendAntwort {
        antwort_pid,
        message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
        transaktionsgrund_ergaenzung: antwort.ergaenzung.map(str::to_owned),
        stammdaten: antwort.stammdaten,
    };
    // The answer is checked against its profile before anything is written —
    // the check the pre-send gate runs.
    dispatch_to_process_in_version::<GpkeGeschaeftsdatenanfrageWorkflow, _>(
        state,
        &key,
        WORKFLOW_NAME,
        move |current, fv| {
            use mako_engine::workflow::Workflow as _;
            let out = GpkeGeschaeftsdatenanfrageWorkflow::handle(current, cmd.clone())
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
            super::eog_stammdaten::refuse_what_the_gate_refuses(state, fv, &out.outbox)?;
            Ok(cmd)
        },
    )
    .await
}

/// The NB refuses an inbound 17132 with ORDRSP 19101.
async fn dispatch_ablehnung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: AblehnungBody = parse_body(payload)?;
    let key = netzbetreiber_key(
        required(&body.anfragender_mp_id, "anfragender_mp_id")?,
        anfrage_nummer(&body.anfrage_nummer)?,
    );
    let anfragender = match body.anfragender.trim().to_uppercase().as_str() {
        "LF" => Anfragender::Lf,
        "MSB" => Anfragender::Msb,
        other => {
            return Err(DispatchError::InvalidPayload(format!(
                "\"anfragender\" must be LF or MSB; got {other:?}"
            )));
        }
    };
    let antwortcode = required(&body.antwortcode, "antwortcode")?.to_owned();
    dispatch_to_process::<GpkeGeschaeftsdatenanfrageWorkflow, _>(
        state,
        &key,
        WORKFLOW_NAME,
        move || GdaCommand::SendAblehnung {
            anfragender,
            antwortcode,
            message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
        },
    )
    .await
}
