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

use mako_engine::payload::{Stammdaten, StammdatenBlock, StammdatenLokation, StammdatenSegment};
use mako_gpke::geschaeftsdatenanfrage::{
    Anfragender, GdaAnfrage, GdaCommand, GpkeGeschaeftsdatenanfrageWorkflow, WORKFLOW_NAME,
    anfragender_key, netzbetreiber_key,
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

/// Body of `gpke.geschaeftsdatenanfrage.beantworten`.
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
    /// `SG4 STS+7` Transaktionsgrundergänzung on 55035 (`ZW4` / `ZAP`).
    #[serde(default)]
    transaktionsgrund_ergaenzung: Option<String>,
    /// The Stammdaten the answer transmits.
    stammdaten: StammdatenBody,
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

/// The Stammdaten of an answer, segment by segment.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StammdatenBody {
    /// `SG5 LOC`, in the MIG's order.
    lokationen: Vec<LokationBody>,
    /// `SG8` blocks.
    bloecke: Vec<BlockBody>,
    /// `SG12` segments.
    #[serde(default)]
    sg12: Vec<SegmentBody>,
}

/// One `SG5 LOC`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LokationBody {
    /// DE 3227.
    qualifier: String,
    /// DE 3225.
    id: String,
}

/// One `SG8` block.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockBody {
    /// `SEQ` DE 1229.
    seq: String,
    /// The segments under it, in order.
    segmente: Vec<SegmentBody>,
}

/// One segment.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SegmentBody {
    /// Segment tag.
    tag: String,
    /// Data elements, each a list of components.
    elemente: Vec<Vec<String>>,
}

impl From<SegmentBody> for StammdatenSegment {
    fn from(s: SegmentBody) -> Self {
        Self {
            tag: s.tag,
            elemente: s.elemente,
        }
    }
}

impl From<StammdatenBody> for Stammdaten {
    fn from(b: StammdatenBody) -> Self {
        Self {
            lokationen: b
                .lokationen
                .into_iter()
                .map(|l| StammdatenLokation {
                    qualifier: l.qualifier,
                    id: l.id,
                })
                .collect(),
            bloecke: b
                .bloecke
                .into_iter()
                .map(|blk| StammdatenBlock {
                    seq: blk.seq,
                    segmente: blk.segmente.into_iter().map(Into::into).collect(),
                })
                .collect(),
            sg12: b.sg12.into_iter().map(Into::into).collect(),
        }
    }
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

/// The NB answers an inbound 17132 with the Stammdaten.
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
    let transaktionsgrund_ergaenzung = body
        .transaktionsgrund_ergaenzung
        .map(|e| e.trim().to_owned())
        .filter(|e| !e.is_empty());
    let stammdaten = Stammdaten::from(body.stammdaten);
    dispatch_to_process::<GpkeGeschaeftsdatenanfrageWorkflow, _>(
        state,
        &key,
        WORKFLOW_NAME,
        move || GdaCommand::SendAntwort {
            antwort_pid,
            message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
            transaktionsgrund_ergaenzung,
            stammdaten,
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
