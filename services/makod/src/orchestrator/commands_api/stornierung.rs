//! The NB's answer to an inbound Anfrage nach Stornierung — UTILMD Strom
//! 55023/55024 on a 55022, UTILMD Gas 44023/44024 on a 44022.
//!
//! The answer is a UTILMD of its own, „zurück an den Absender" (UTILMD AHB
//! Strom 2.2 Kap. 9.6; UTILMD AHB Gas 1.2 Kap. 5.2). The Bestätigung carries
//! `E15`, the only code of `S_0086` / `G_0004`; the Ablehnung one of the three
//! codes of `S_0087` / `G_0003` — `E14` „Ablehnung Sonstiges" with its `FTX+ACB`
//! Erläuterung, `Z14` „Ablehnung (Doppelmeldung)", `ZE1` „zu stornierender
//! Vorgang wurde schon beantwortet".
//!
//! The process is found by the key the ingest spawned it under: the 55022 /
//! 44022 carries no `SG5 LOC`, so that is its `SG4 IDE+24` Vorgangsnummer.

use super::*;

/// Body of `gpke.stornierung.bestaetigen` / `geli.stornierung.bestaetigen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BestaetigungBody {
    /// The 55022's / 44022's `SG4 IDE+24` Vorgangsnummer.
    vorgang_id: String,
}

/// Body of `gpke.stornierung.ablehnen` / `geli.stornierung.ablehnen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AblehnungBody {
    /// The 55022's / 44022's `SG4 IDE+24` Vorgangsnummer.
    vorgang_id: String,
    /// `E14`, `Z14` or `ZE1`.
    antwortcode: String,
    /// The `FTX+ACB` Erläuterung — required with `E14`, refused otherwise.
    #[serde(default)]
    bemerkung: Option<String>,
}

/// One of the three Ablehnungsgründe both Codelisten publish, read Sparte-free.
enum Grund {
    Sonstiges(String),
    Doppelmeldung,
    SchonBeantwortet,
}

fn parse_body<T: serde::de::DeserializeOwned>(
    payload: &serde_json::Value,
) -> Result<T, DispatchError> {
    serde_json::from_value(payload.clone())
        .map_err(|e| DispatchError::InvalidPayload(format!("payload: {e}")))
}

fn vorgang_id(raw: &str) -> Result<&str, DispatchError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DispatchError::InvalidPayload(
            "\"vorgang_id\" must name the Anfrage's SG4 IDE+24 Vorgangsnummer".to_owned(),
        ));
    }
    Ok(trimmed)
}

/// Resolve the Ablehnungsgrund.
///
/// A code outside the Codeliste is refused, and so is a `bemerkung` beside a
/// code that carries no `FTX`: it would be dropped on the wire while the caller
/// believes it was sent.
fn grund(body: &AblehnungBody, codeliste: &str) -> Result<Grund, DispatchError> {
    let bemerkung = body
        .bemerkung
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty());
    match (body.antwortcode.trim(), bemerkung) {
        ("E14", Some(b)) => Ok(Grund::Sonstiges(b.to_owned())),
        ("E14", None) => Err(DispatchError::InvalidPayload(format!(
            "E14 „Ablehnung Sonstiges\" needs its FTX+ACB Erläuterung in \"bemerkung\" \
             ({codeliste})"
        ))),
        ("Z14" | "ZE1", Some(_)) => Err(DispatchError::InvalidPayload(format!(
            "\"bemerkung\" rides only E14 — {} carries no FTX+ACB ({codeliste})",
            body.antwortcode.trim()
        ))),
        ("Z14", None) => Ok(Grund::Doppelmeldung),
        ("ZE1", None) => Ok(Grund::SchonBeantwortet),
        (other, _) => Err(DispatchError::InvalidPayload(format!(
            "\"antwortcode\" must be E14, Z14 or ZE1 ({codeliste}); got {other:?}"
        ))),
    }
}

pub(super) fn cmd_gpke_stornierung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_stornierung_antwort(s, p, false))
}

pub(super) fn cmd_gpke_stornierung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_stornierung_antwort(s, p, true))
}

pub(super) fn cmd_geli_stornierung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_stornierung_antwort(s, p, false))
}

pub(super) fn cmd_geli_stornierung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_stornierung_antwort(s, p, true))
}

/// Answer an inbound 55022 with UTILMD 55023 (Bestätigung) or 55024
/// (Ablehnung, Codeliste `S_0087`).
async fn dispatch_gpke_stornierung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    ablehnen: bool,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_gpke::{GpkeStornierungCommand, GpkeStornierungWorkflow, StornoAblehnung};
    let (key, ablehnung) = if ablehnen {
        let body: AblehnungBody = parse_body(payload)?;
        let ablehnung = match grund(&body, mako_gpke::stornierung::ABLEHNUNG_CODELISTE)? {
            Grund::Sonstiges(bemerkung) => StornoAblehnung::Sonstiges { bemerkung },
            Grund::Doppelmeldung => StornoAblehnung::Doppelmeldung,
            Grund::SchonBeantwortet => StornoAblehnung::SchonBeantwortet,
        };
        (vorgang_id(&body.vorgang_id)?.to_owned(), Some(ablehnung))
    } else {
        let body: BestaetigungBody = parse_body(payload)?;
        (vorgang_id(&body.vorgang_id)?.to_owned(), None)
    };
    dispatch_to_process::<GpkeStornierungWorkflow, _>(
        state,
        &key,
        mako_gpke::STORNIERUNG_GPKE_WORKFLOW_NAME,
        move || GpkeStornierungCommand::SendAntwort { ablehnung },
    )
    .await
}

/// Answer an inbound 44022 with UTILMD 44023 (Bestätigung) or 44024
/// (Ablehnung, Codeliste `G_0003`).
async fn dispatch_geli_stornierung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    ablehnen: bool,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_geli_gas::{GeliGasStornierungCommand, GeliGasStornierungWorkflow, StornoAblehnung};
    let (key, ablehnung) = if ablehnen {
        let body: AblehnungBody = parse_body(payload)?;
        let ablehnung = match grund(&body, mako_geli_gas::stornierung::ABLEHNUNG_CODELISTE)? {
            Grund::Sonstiges(bemerkung) => StornoAblehnung::Sonstiges { bemerkung },
            Grund::Doppelmeldung => StornoAblehnung::Doppelmeldung,
            Grund::SchonBeantwortet => StornoAblehnung::SchonBeantwortet,
        };
        (vorgang_id(&body.vorgang_id)?.to_owned(), Some(ablehnung))
    } else {
        let body: BestaetigungBody = parse_body(payload)?;
        (vorgang_id(&body.vorgang_id)?.to_owned(), None)
    };
    dispatch_to_process::<GeliGasStornierungWorkflow, _>(
        state,
        &key,
        mako_geli_gas::STORNIERUNG_WORKFLOW_NAME,
        move || GeliGasStornierungCommand::SendAntwort { ablehnung },
    )
    .await
}
