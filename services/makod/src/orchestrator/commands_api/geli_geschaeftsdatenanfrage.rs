//! GeLi Gas Geschäftsdatenanfrage (BK7-24-01-009 Anlage Kap. 4.4) — the three
//! outbound legs: ORDERS 17101 / 17126, UTILMD 44035 / 44060, ORDRSP 19101.
//!
//! Keyed as on the Strom side ([`super::geschaeftsdatenanfrage`]): the
//! Anfragender's process on the Anfrage-Nummer it issued, the NB's on that
//! number with the Anfragender's MP-ID.

use mako_geli_gas::geschaeftsdatenanfrage::{
    Anfragender, GdaGasAnfrage, GdaGasCommand, GdaGasState, GeliGasGeschaeftsdatenanfrageWorkflow,
    WORKFLOW_NAME, anfragender_key, netzbetreiber_key,
};

use super::*;

/// Body of `geli.geschaeftsdatenanfrage.anfragen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnfrageBody {
    /// The Markt- or Messlokation asked about — `SG2 LOC+172`.
    lokation_id: String,
    /// MP-ID of the Gasnetzbetreiber asked.
    nb_mp_id: String,
    /// `LF` (ORDERS 17101) or `MSB` (ORDERS 17126).
    anfragender: String,
    /// The Anfrage-Nummer (`BGM` DE 1004, `an..35`); generated when absent.
    #[serde(default)]
    anfrage_nummer: Option<String>,
}

/// Body of `geli.geschaeftsdatenanfrage.beantworten`: the NB's Stammdaten,
/// carried whole as it holds them; the AHB profile checks what renders.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AntwortBody {
    /// MP-ID of the Anfragender — the ORDERS' `NAD+MS`.
    anfragender_mp_id: String,
    /// The ORDERS' `BGM` DE 1004.
    anfrage_nummer: String,
    /// The Stammdaten the UTILMD 44035 / 44060 transmits.
    stammdaten: mako_engine::payload::Stammdaten,
}

/// Body of `geli.geschaeftsdatenanfrage.ablehnen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AblehnungBody {
    /// MP-ID of the Anfragender — the ORDERS' `NAD+MS`.
    anfragender_mp_id: String,
    /// The ORDERS' `BGM` DE 1004.
    anfrage_nummer: String,
    /// The Ablehnungscode — `G_0049` to a LF, `E_3004` to a MSB.
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

pub(super) fn cmd_geli_geschaeftsdatenanfrage_anfragen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_anfrage(s, p))
}

pub(super) fn cmd_geli_geschaeftsdatenanfrage_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_antwort(s, p))
}

pub(super) fn cmd_geli_geschaeftsdatenanfrage_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_ablehnung(s, p))
}

/// The LF or MSB sends ORDERS 17101 / 17126 to the Gasnetzbetreiber and opens
/// the process the UTILMD 44035 / 44060 or the ORDRSP 19101 resumes. The
/// Anfragender holds no Frist of its own: Kap. 4.4 binds the NB's answer.
async fn dispatch_anfrage(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: AnfrageBody = parse_body(payload)?;
    let lokation_id = required(&body.lokation_id, "lokation_id")?.to_owned();
    let nb = required(&body.nb_mp_id, "nb_mp_id")?;
    let rolle = match body.anfragender.trim().to_uppercase().as_str() {
        "LF" => Anfragender::Lf,
        "MSB" => Anfragender::Msb,
        other => {
            return Err(DispatchError::InvalidPayload(format!(
                "\"anfragender\" must be LF (ORDERS 17101) or MSB (ORDERS 17126); got {other:?}"
            )));
        }
    };
    let nummer = match body.anfrage_nummer.as_deref() {
        Some(raw) => anfrage_nummer(raw)?.to_owned(),
        None => format!("GDA{}", uuid::Uuid::new_v4().simple()),
    };
    let key = anfragender_key(&nummer);

    let occupancy =
        find_occupying_process::<GeliGasGeschaeftsdatenanfrageWorkflow>(state, &key, WORKFLOW_NAME)
            .await?;
    if let Some(process_id) = occupancy.process {
        return Err(DispatchError::DuplicateProcess {
            process_id,
            malo_id: nummer,
        });
    }

    let command = GdaGasCommand::SendAnfrage {
        anfrage: GdaGasAnfrage {
            anfragender_rolle: rolle,
            anfragender: MarktpartnerCode::new(state.sender_party_id.clone()),
            netzbetreiber: MarktpartnerCode::new(nb),
            lokation_id,
            anfrage_nummer: MessageRef::new(nummer),
        },
    };
    let process_id = spawn_process::<GeliGasGeschaeftsdatenanfrageWorkflow>(
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

/// The Gasnetzbetreiber answers an inbound 17101 / 17126 with the Stammdaten
/// it holds. The answer is checked against its AHB profile before anything is
/// written — the check the pre-send gate runs.
async fn dispatch_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: AntwortBody = parse_body(payload)?;
    let key = netzbetreiber_key(
        required(&body.anfragender_mp_id, "anfragender_mp_id")?,
        anfrage_nummer(&body.anfrage_nummer)?,
    );
    let current =
        super::current_state::<GeliGasGeschaeftsdatenanfrageWorkflow>(state, &key, WORKFLOW_NAME)
            .await?;
    if !matches!(current.offen(), GdaGasState::Offen(_)) {
        return Err(DispatchError::InvalidPayload(format!(
            "the Geschäftsdatenanfrage {} of {} is not open for an answer (state {})",
            body.anfrage_nummer,
            body.anfragender_mp_id,
            current.status_str()
        )));
    }
    let cmd = GdaGasCommand::SendAntwort {
        message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
        stammdaten: body.stammdaten,
    };
    dispatch_to_process_in_version::<GeliGasGeschaeftsdatenanfrageWorkflow, _>(
        state,
        &key,
        WORKFLOW_NAME,
        move |current, fv| {
            use mako_engine::workflow::Workflow as _;
            let out = GeliGasGeschaeftsdatenanfrageWorkflow::handle(current, cmd.clone())
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
            super::eog_stammdaten::refuse_what_the_gate_refuses(state, fv, &out.outbox)?;
            Ok(cmd)
        },
    )
    .await
}

/// The Gasnetzbetreiber refuses an inbound 17101 / 17126 with ORDRSP 19101;
/// the Codeliste follows from the Anfrage (`G_0049` / `E_3004`).
async fn dispatch_ablehnung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: AblehnungBody = parse_body(payload)?;
    let key = netzbetreiber_key(
        required(&body.anfragender_mp_id, "anfragender_mp_id")?,
        anfrage_nummer(&body.anfrage_nummer)?,
    );
    let antwortcode = required(&body.antwortcode, "antwortcode")?.to_owned();
    dispatch_to_process::<GeliGasGeschaeftsdatenanfrageWorkflow, _>(
        state,
        &key,
        WORKFLOW_NAME,
        move || GdaGasCommand::SendAblehnung {
            antwortcode,
            message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
        },
    )
    .await
}
