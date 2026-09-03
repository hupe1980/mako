//! GaBi Gas command wrappers and dispatchers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

/// Settle or dispute a GaBi Gas billing INVOIC — the whole family, PIDs 31007,
/// 31008 and 31010.
///
/// One workflow answers all three, so one command pair does too. `invoicd`
/// dispatches it for 31007/31008 after the plausibility check (including MMM
/// Gas price check 6 against Trading Hub Europe MMMA prices); the
/// Kapazitätsrechnung 31010 has no price basis to check against and is answered
/// by an operator through the same command.
///
/// Business key = `invoice_ref`.
pub(super) async fn dispatch_gabi_gas_invoic(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    settle: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let invoice_ref = extract_invoice_ref(payload)?;
    let reason = payload
        .get("ablehnungsgrund")
        .and_then(|v| v.as_str())
        .unwrap_or("Automatisch ermittelte Abweichung — GaBi Gas Rechnung")
        .to_owned();
    let message_ref = remadv_message_ref(payload);
    let antwort = remadv_antwort(payload);
    dispatch_to_process::<GaBiGasInvoicWorkflow, _>(
        state,
        &invoice_ref,
        GABI_GAS_INVOIC_WORKFLOW_NAME,
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

pub(super) fn cmd_gabi_gas_rechnung_annehmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gabi_gas_invoic(s, p, true))
}

pub(super) fn cmd_gabi_gas_rechnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gabi_gas_invoic(s, p, false))
}

// ── DVGW sending side: nomination, its answer, Mehr-/Mindermengen ────────────

/// One `LIN` position of a nomination, out of the command payload.
///
/// The wire states a **rate** per period (`QTY` in `KW1`, kWh/h), so a
/// position carries its `mengen` with their bounds rather than a single
/// figure — an energy without the period it applies to cannot be nominated.
fn nomination_positions(
    payload: &serde_json::Value,
) -> Result<Vec<mako_gabi_gas::NominationPosition>, DispatchError> {
    let bad = |what: &str| DispatchError::InvalidPayload(what.to_owned());
    let entries = payload
        .get("positionen")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| {
            bad(
                "payload must contain a non-empty \"positionen\" array — a nomination \
                 states at least one point",
            )
        })?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let field = |name: &str| {
            entry
                .get(name)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        };
        let ort = field("ort").ok_or_else(|| bad("every Position needs an \"ort\""))?;
        let mut mengen = Vec::new();
        for m in entry
            .get("mengen")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let instant = |name: &str| -> Result<time::OffsetDateTime, DispatchError> {
                let raw = m
                    .get(name)
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| bad(&format!("every Menge needs \"{name}\" (RFC 3339)")))?;
                time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
                    .map_err(|e| bad(&format!("\"{name}\" {raw:?} is not RFC 3339: {e}")))
            };
            let kwh_pro_h = m
                .get("kwh_pro_h")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    bad(
                        "every Menge needs \"kwh_pro_h\" as a decimal string — a rate in \
                         kWh/h, and a JSON float would carry a binary rounding error into \
                         what is settled",
                    )
                })?
                .parse::<rust_decimal::Decimal>()
                .map_err(|e| bad(&format!("\"kwh_pro_h\" is not a decimal: {e}")))?;
            mengen.push(mako_gabi_gas::NominationMenge {
                von: instant("von")?,
                bis: instant("bis")?,
                kwh_pro_h,
            });
        }
        if mengen.is_empty() {
            return Err(bad("every Position needs a non-empty \"mengen\" array"));
        }
        out.push(mako_gabi_gas::NominationPosition {
            ort_qualifier: field("ort_qualifier").unwrap_or("Z19").to_owned(),
            ort: ort.to_owned(),
            richtung: field("richtung")
                .ok_or_else(|| {
                    bad("every Position needs a \"richtung\" — Z02 Einspeisung or Z03 Ausspeisung")
                })?
                .to_owned(),
            bilanzkreis_intern: field("bilanzkreis_intern")
                .ok_or_else(|| bad("every Position needs a \"bilanzkreis_intern\""))?
                .to_owned(),
            bilanzkreis_extern: field("bilanzkreis_extern").map(str::to_owned),
            mengen,
        });
    }
    Ok(out)
}

fn gas_day_of(payload: &serde_json::Value, field: &str) -> Result<GasDay, DispatchError> {
    let raw = payload.get(field).and_then(|v| v.as_str()).ok_or_else(|| {
        DispatchError::InvalidPayload(format!("payload must contain \"{field}\" (YYYY-MM-DD)"))
    })?;
    GasDay::parse(raw).map_err(|e| {
        DispatchError::InvalidPayload(format!(
            "\"{field}\" {raw:?} is not a gas day (YYYY-MM-DD): {e}"
        ))
    })
}

fn message_ref_of(payload: &serde_json::Value, field: &str) -> mako_engine::types::MessageRef {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map_or_else(
            || mako_engine::types::MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
            mako_engine::types::MessageRef::new,
        )
}

/// Nominate a gas day: the NOMINT a Transportkunde sends its NB or the MGV.
///
/// Business key = the nomination's own Zuordnung (Gastag, Ort, Bilanzkreis
/// intern, Bilanzkreis extern), built through `dvgw-edi` so the NOMRES that
/// answers it — which carries no reference back — resolves to the same
/// process.
pub(super) async fn dispatch_gabi_nominierung_senden(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let pruefidentifikator = payload
        .get("pruefidentifikator")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"pruefidentifikator\" — 70030 at a physical point, \
                 70031 at the Virtueller Handelspunkt, 70032 Flexibilitätsübertragung, \
                 70033 gebündelt, 70034 Weitergabe zwischen Netzbetreibern"
                    .into(),
            )
        })?;
    let gas_day = gas_day_of(payload, "gastag")?;
    let positions = nomination_positions(payload)?;
    let nomination_ref = message_ref_of(payload, "nomination_ref");
    let receiver_eic = payload
        .get("empfaenger")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"empfaenger\" — the NB or MGV the nomination is \
                 addressed to"
                    .into(),
            )
        })?
        .to_owned();
    let sender_eic = state.sender_party_id.clone();
    let corrects = match payload.get("korrigiert") {
        None => None,
        Some(c) => {
            let reference = c
                .get("nomination_ref")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    DispatchError::InvalidPayload(
                        "\"korrigiert\" needs the \"nomination_ref\" of the nomination it \
                         corrects"
                            .into(),
                    )
                })?;
            let raw = c
                .get("processed_at")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    DispatchError::InvalidPayload(
                        "\"korrigiert\" needs \"processed_at\" (RFC 3339) — NOMINT marks the \
                     DTM+9 beside RFF+AGO Erforderlich"
                            .into(),
                    )
                })?;
            let processed_at =
                time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
                    .map_err(|e| {
                        DispatchError::InvalidPayload(format!(
                            "\"processed_at\" is not RFC 3339: {e}"
                        ))
                    })?;
            Some(mako_gabi_gas::Renominierung {
                nomination_ref: mako_engine::types::MessageRef::new(reference),
                processed_at,
            })
        }
    };
    let business_key = mako_gabi_gas::nomination_process_key(gas_day, &positions);
    dispatch_to_process::<GaBiGasNominationWorkflow, _>(
        state,
        &business_key,
        GABI_GAS_NOMINATION_WORKFLOW_NAME,
        move || NominationCommand::SendNomination {
            pruefidentifikator,
            sender_eic,
            receiver_eic,
            gas_day,
            nomination_ref,
            positions,
            corrects,
        },
    )
    .await
}

/// Answer a nomination addressed to this tenant: the NOMRES the NB or MGV owes.
///
/// Business key = the nomination's Zuordnung, so the answer resumes the
/// process the inbound NOMINT opened.
pub(super) async fn dispatch_gabi_nominierung_beantworten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let pruefidentifikator = payload
        .get("pruefidentifikator")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"pruefidentifikator\" — 70035 Matching, 70036 \
                 Bestätigung, 70037 VHP-Matching, 70038 VHP-Bestätigung, 70039 \
                 Bestätigung Flexibilitätsübertragung"
                    .into(),
            )
        })?;
    let gas_day = gas_day_of(payload, "gastag")?;
    // The answer resumes the process the NOMINT opened, so the key comes from
    // the nomination's own positions.
    let nominated = nomination_positions(payload)?;
    let acceptance = match payload.get("entscheidung").and_then(|v| v.as_str()) {
        Some("bestaetigt") => NomresAcceptance::Accepted,
        Some("teilweise") => NomresAcceptance::PartiallyAccepted,
        Some("abgelehnt") => NomresAcceptance::Rejected,
        other => {
            return Err(DispatchError::InvalidPayload(format!(
                "unknown \"entscheidung\" {other:?}; valid: bestaetigt, teilweise, abgelehnt"
            )));
        }
    };
    // A curtailment states the positions the match produced; a confirmation
    // restates the nomination's own.
    let confirmed = match payload.get("bestaetigte_positionen") {
        Some(_) => Some(nomination_positions_from(
            payload,
            "bestaetigte_positionen",
        )?),
        None => None,
    };
    let nomres_ref = message_ref_of(payload, "nomres_ref");
    let business_key = mako_gabi_gas::nomination_process_key(gas_day, &nominated);
    dispatch_to_process::<GaBiGasNominationWorkflow, _>(
        state,
        &business_key,
        GABI_GAS_NOMINATION_WORKFLOW_NAME,
        move || NominationCommand::SendNomres {
            pruefidentifikator,
            nomres_ref,
            acceptance,
            confirmed,
        },
    )
    .await
}

/// [`nomination_positions`] reading a differently named array.
fn nomination_positions_from(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Vec<mako_gabi_gas::NominationPosition>, DispatchError> {
    let lifted =
        serde_json::json!({ "positionen": payload.get(field).cloned().unwrap_or_default() });
    nomination_positions(&lifted)
}

/// Report a Netzkonto's Mehr-/Mindermenge: the SSQNOT the NB sends the MGV.
///
/// Business key = the published 2-Tupel (Netzkonto, Netzbetreiber) plus the
/// Abrechnungszeitraum, so a later report for the same period resumes the
/// process holding the earlier one.
pub(super) async fn dispatch_gabi_mehrmindermengen_melden(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let bad = |what: &str| DispatchError::InvalidPayload(what.to_owned());
    let str_field = |name: &'static str| {
        payload
            .get(name)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| DispatchError::InvalidPayload(format!("payload must contain {name:?}")))
    };
    let date = |name: &'static str| -> Result<time::Date, DispatchError> {
        let raw = str_field(name)?;
        time::Date::parse(&raw, &time::format_description::well_known::Iso8601::DATE)
            .map_err(|e| DispatchError::InvalidPayload(format!("{name:?} is not a date: {e}")))
    };
    let decimal = |name: &'static str| -> Result<rust_decimal::Decimal, DispatchError> {
        let raw = payload.get(name).and_then(|v| v.as_str()).unwrap_or("0");
        raw.parse::<rust_decimal::Decimal>()
            .map_err(|e| DispatchError::InvalidPayload(format!("{name:?} is not a decimal: {e}")))
    };
    let verfahren = match payload.get("verfahren").and_then(|v| v.as_str()) {
        Some("slp") => mako_gabi_gas::MmmVerfahren::Slp,
        Some("rlm") => mako_gabi_gas::MmmVerfahren::Rlm,
        other => {
            return Err(DispatchError::InvalidPayload(format!(
                "unknown \"verfahren\" {other:?}; valid: slp, rlm (rlm only for Zeiträume \
                 before {})",
                dvgw_edi::SSQNOT_RLM_CUTOFF
            )));
        }
    };
    let data = mako_gabi_gas::MehrMindermengenData {
        pruefidentifikator: match verfahren {
            mako_gabi_gas::MmmVerfahren::Slp => 70_095,
            mako_gabi_gas::MmmVerfahren::Rlm => 70_096,
        },
        netzbetreiber: state.sender_party_id.clone(),
        marktgebietsverantwortlicher: str_field("marktgebietsverantwortlicher")?,
        netzkonto: str_field("netzkonto")?,
        zeitraum_von: date("zeitraum_von")?,
        zeitraum_bis: date("zeitraum_bis")?,
        verfahren,
        mehrmenge_kwh: decimal("mehrmenge_kwh")?,
        mindermenge_kwh: decimal("mindermenge_kwh")?,
        message_ref: message_ref_of(payload, "message_ref"),
    };
    if data.netzkonto.is_empty() {
        return Err(bad("\"netzkonto\" must not be empty"));
    }
    let business_key = format!(
        "{}|{}..{}",
        dvgw_edi::CorrelationKey {
            zuordnung: dvgw_edi::Zuordnung::MehrMindermengen,
            elements: vec![data.netzkonto.clone(), data.netzbetreiber.clone()],
        },
        data.zeitraum_von,
        data.zeitraum_bis
    );
    dispatch_to_process::<GaBiGasMehrMindermengenWorkflow, _>(
        state,
        &business_key,
        mako_gabi_gas::MEHR_MINDERMENGEN_WORKFLOW_NAME,
        move || MehrMindermengenCommand::Melden(data.clone()),
    )
    .await
}

pub(super) fn cmd_gabi_nominierung_senden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gabi_nominierung_senden(s, p))
}

pub(super) fn cmd_gabi_nominierung_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gabi_nominierung_beantworten(s, p))
}

pub(super) fn cmd_gabi_mehrmindermengen_melden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gabi_mehrmindermengen_melden(s, p))
}
