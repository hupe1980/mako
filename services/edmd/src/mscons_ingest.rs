//! The MSCONS `de.mako.process.completed` door: decode the whole delivery,
//! then write it.
//!
//! Every refusal — an absent or unknown Sparte, an unknown origin label, a
//! Zählerstand that names no register, a Messlokation `marktd` cannot place —
//! is decided before anything is written. The receipt is written last, so a
//! `meter_data_receipts` row means the delivery was taken: a refused one leaves
//! none, and one whose store write failed is redelivered and receipted then.

use axum::{http::StatusCode, response::IntoResponse};
use mako_service::ApiError;
use tracing::{info, warn};
use uuid::Uuid;

use crate::domain::{
    AblesungHerkunft, ESA_TYP2_PIDS, GAS_QUALITY_PIDS, IngestionSource, MeterDataReceipt,
    MeterRead, MeterReading, Sparte, Typ2DeliveryPath, Typ2Read,
    repository::{TimeSeriesRepository, Typ2Repository},
};
use crate::handler::{HandlerState, ingest_refusal, json_decimal, month_of};

/// A Messlokations-ID: 33 characters, against the 11 digits of a
/// Marktlokations-ID — `SG6 LOC+172` takes either.
fn is_melo(id: &str) -> bool {
    id.len() == 33
}

/// Store one MSCONS `process.completed` event.
pub(crate) async fn ingest(
    state: &HandlerState,
    event: &serde_json::Value,
    pid: u32,
) -> axum::response::Response {
    let subject = event["subject"].as_str().unwrap_or("").to_owned();
    let process_id: Uuid = match subject.parse() {
        Ok(id) => id,
        Err(_) => {
            warn!(subject, "edmd: subject is not a valid UUID — skipping");
            return StatusCode::NO_CONTENT.into_response();
        }
    };

    let data = &event["data"];
    let location = data["malo_id"]
        .as_str()
        .or_else(|| data["location_id"].as_str())
        .unwrap_or("")
        .to_owned();
    if location.is_empty() {
        warn!(process_id = %process_id, pid, "edmd: no malo_id in event data — skipping");
        return StatusCode::NO_CONTENT.into_response();
    }

    let sender_mp_id = data["sender"]
        .as_str()
        .or_else(|| data["sender_mp_id"].as_str())
        .or_else(|| data["partner_mp_id"].as_str())
        .unwrap_or("")
        .to_owned();
    let sender = (!sender_mp_id.is_empty()).then(|| sender_mp_id.clone());
    let message_ref = data["message_ref"].as_str().map(str::to_owned);
    let received_at = event["time"]
        .as_str()
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
        .unwrap_or_else(time::OffsetDateTime::now_utc);

    let reads_array = data["reads"]
        .as_array()
        .filter(|a| !a.is_empty() && !crate::domain::STORNO_PIDS.contains(&pid));
    let staende_array = data["zaehlerstaende"]
        .as_array()
        .filter(|a| !a.is_empty() && !crate::domain::STORNO_PIDS.contains(&pid));

    // ── Decode: every refusal before any write ────────────────────────────
    let decoded = if reads_array.is_some() || staende_array.is_some() {
        match decode(
            state,
            process_id,
            pid,
            data,
            &location,
            sender.as_ref(),
            reads_array.map(Vec::as_slice).unwrap_or_default(),
            staende_array.map(Vec::as_slice).unwrap_or_default(),
        )
        .await
        {
            Ok(d) => Some(d),
            Err(refusal) => return refusal.into_response(),
        }
    } else {
        None
    };
    // The Marktlokation the receipt is filed under: the resolved one when the
    // delivery named a Messlokation.
    let malo_id = decoded
        .as_ref()
        .map_or_else(|| location.clone(), |d| d.malo_id.clone());

    let receipt = MeterDataReceipt {
        process_id,
        pid,
        malo_id: malo_id.clone(),
        sender_mp_id,
        message_ref,
        received_at,
        tenant: state.tenant.clone(),
    };

    // ── PID 13007: record the Gasbeschaffenheit delivery ───────────────────
    // The ProcessCompleted payload carries `brennwert_kwh_per_m3` and
    // `zustandszahl` extracted by the makod adapter from `QTY+Z08`/`QTY+Z10`,
    // plus the period they apply to. Both factors are period-scoped: the gas
    // grid operator publishes an Abrechnungsbrennwert per supply area per
    // month, so a value without its period cannot be matched to a
    // consumption month. When the payload omits the period, the delivery is
    // filed against the calendar month it arrived in — the publication
    // cadence — rather than being applied to the MaLo's whole history.
    if GAS_QUALITY_PIDS.contains(&pid) {
        let decimal = |key: &str| json_decimal(&data[key]);
        let brennwert = decimal("brennwert_kwh_per_m3");
        let zustandszahl = decimal("zustandszahl");
        if brennwert.is_some() || zustandszahl.is_some() {
            let (default_from, default_to) = month_of(mako_fristen::berlin_date(received_at));
            let date = |key: &str| {
                data[key].as_str().and_then(|s| {
                    time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE).ok()
                })
            };
            let period_from = date("period_from").unwrap_or(default_from);
            let period_to = date("period_to")
                .filter(|t| *t >= period_from)
                .unwrap_or(default_to.max(period_from));

            let record = crate::domain::GasQualityRecord {
                tenant: state.tenant.clone(),
                malo_id: malo_id.clone(),
                period_from,
                period_to,
                brennwert_kwh_per_m3: brennwert,
                zustandszahl,
                source_pid: Some(pid),
            };
            match state.repo.record_gas_quality(&record).await {
                Ok(n) => info!(
                    process_id = %process_id, pid, malo_id = %malo_id,
                    %period_from, %period_to, billing_periods_backfilled = n,
                    "edmd: recorded Gasbeschaffenheit (Brennwert/Zustandszahl)"
                ),
                Err(err) => warn!(%err, process_id = %process_id, pid,
                    "edmd: failed to record gas quality"),
            }
        }
    }

    // ── PID 13006: Messwert Storno ────────────────────────────────────────
    // A Storno withdraws values delivered earlier; it carries no new
    // measurements. Storing whatever `reads` array it happens to include
    // would book the *cancelled* quantities as freshly measured ones — the
    // opposite of what the message says — so the receipt is recorded and the
    // payload is not. Applying the cancellation to the referenced delivery
    // needs the reference the ProcessCompleted payload does not carry yet.
    if crate::domain::STORNO_PIDS.contains(&pid) {
        warn!(
            process_id = %process_id, pid, malo_id = %malo_id,
            message_ref = ?receipt.message_ref,
            "edmd: MSCONS Messwert Storno received — receipt recorded, no values \
             stored; the referenced delivery must be withdrawn by an operator \
             correction (POST /api/v1/corrections/{{malo_id}})"
        );
        return record_receipt(state, &receipt).await;
    }

    let Some(decoded) = decoded else {
        return record_receipt(state, &receipt).await;
    };

    // ── ESA "Werte nach Typ 2" (PID 13027) fork ───────────────────────────
    // These are non-authoritative (Codeliste 1.4 Kap. 4.6 · WiM Teil 2
    // §4). They must never enter `meter_reads` — a billing query would
    // sum them by omission. Route them to the separate Typ-2 store, with
    // no validation/substitution machinery: a Typ-2 value is stored as
    // delivered and never reconciled or corrected.
    if ESA_TYP2_PIDS.contains(&pid) {
        // `SG1 RFF+AGI` — which ESA subscription these values belong
        // to. A Meldepunkt may carry several (a subscription is the
        // (Meldepunkt, Messprodukt) pair), so without it a Typ-2 gap
        // could only be reported against a register, never against the
        // subscription that has actually stopped.
        let bestellung_ref = data
            .get("bestellung_ref")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if bestellung_ref.is_none() {
            warn!(
                process_id = %process_id, pid, malo_id = %malo_id,
                "edmd: ESA Typ-2 delivery without SG1 RFF+AGI — the subscription it \
                 belongs to cannot be named (MSCONS AHB 3.2 §11.2 hint [574])"
            );
        }
        let typ2: Vec<Typ2Read> = decoded
            .reads
            .into_iter()
            .map(|m| Typ2Read {
                malo_id: m.malo_id,
                melo_id: m.melo_id,
                dtm_from: m.dtm_from,
                dtm_to: m.dtm_to,
                quantity_kwh: m.quantity_kwh,
                quality: m.quality,
                pid: m.pid,
                sparte: m.sparte,
                obis_code: m.obis_code,
                tenant: m.tenant,
                delivery_path: Typ2DeliveryPath::MsconsBackend,
                sender_mp_id: m.sender_mp_id,
                bestellung_ref: bestellung_ref.clone(),
                received_at: None,
            })
            .collect();
        let stored = typ2.len();
        if let Err(err) = state.typ2_repo.store_typ2_reads(&typ2).await {
            return ingest_refusal(
                err,
                format!("ESA Typ-2 values of PID {pid} for MaLo {malo_id} not stored"),
            )
            .into_response();
        }
        info!(
            process_id = %process_id, pid, malo_id = %malo_id, stored,
            "edmd: stored ESA Typ-2 values (non-authoritative, separate store)"
        );
        return record_receipt(state, &receipt).await;
    }

    if !decoded.reads.is_empty()
        && let Some(response) = store_intervals(state, process_id, pid, &decoded).await
    {
        return response;
    }

    if !decoded.zaehlerstaende.is_empty() {
        if let Err(err) = state.repo.store_readings(&decoded.zaehlerstaende).await {
            return ingest_refusal(
                err,
                format!("MSCONS Zählerstände of PID {pid} for MaLo {malo_id} not stored"),
            )
            .into_response();
        }
        info!(
            process_id = %process_id, pid, malo_id = %malo_id,
            stored = decoded.zaehlerstaende.len(),
            "edmd: stored MSCONS Zählerstände"
        );
    }

    record_receipt(state, &receipt).await
}

/// Write the receipt — the last write of a delivery.
async fn record_receipt(
    state: &HandlerState,
    receipt: &MeterDataReceipt,
) -> axum::response::Response {
    match state.repo.store_receipt(receipt).await {
        Ok(()) => {
            info!(
                process_id = %receipt.process_id, pid = receipt.pid,
                malo_id = %receipt.malo_id, "edmd: stored MSCONS receipt"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        // `marktd` treats 2xx as delivered and will not redeliver, so a
        // failed receipt write must surface rather than being logged and
        // forgotten — as 5xx when another attempt could work, as 422 when the
        // delivery itself is what the store refused.
        Err(err) => ingest_refusal(
            err,
            format!("receipt of process {} not stored", receipt.process_id),
        )
        .into_response(),
    }
}

/// A delivery decoded and placed, ready to write.
struct Decoded {
    malo_id: String,
    sparte: Sparte,
    max_plant_power_kw: Option<rust_decimal::Decimal>,
    reads: Vec<MeterRead>,
    zaehlerstaende: Vec<MeterReading>,
}

/// The origin a value states, or the refusal naming an unknown label.
fn herkunft(raw: Option<&str>, what: &str) -> Result<Option<AblesungHerkunft>, ApiError> {
    match raw {
        None => Ok(None),
        Some(raw) => AblesungHerkunft::parse(raw).map(Some).ok_or_else(|| {
            ApiError::unprocessable_with(
                format!("MSCONS payload names an unknown herkunft {raw:?} — refusing {what}"),
                serde_json::json!({ "expected": AblesungHerkunft::CODES }),
            )
        }),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the parts of one event the decode reads; a struct would exist for this call alone"
)]
async fn decode(
    state: &HandlerState,
    process_id: Uuid,
    pid: u32,
    data: &serde_json::Value,
    location: &str,
    sender: Option<&String>,
    reads_array: &[serde_json::Value],
    staende_array: &[serde_json::Value],
) -> Result<Decoded, ApiError> {
    use time::format_description::well_known::Rfc3339;
    let what = format!("the batch (process_id {process_id}, pid {pid}, location {location})");

    // A payload naming no commodity, or an unknown one, is a decode fault
    // upstream, not an electricity delivery: a guessed Sparte is stored in
    // the wrong unit — gas settles in kWh_Hs, water in m³. Refused as 422,
    // because redelivering the same payload can never succeed.
    let Some(sparte) = data["sparte"]
        .as_str()
        .and_then(crate::domain::parse_sparte)
    else {
        return Err(ApiError::unprocessable(format!(
            "MSCONS payload names no known sparte ({:?}) — refusing {what}",
            data["sparte"]
        )));
    };

    // The MSCONS correction version the operator assigned, if the decode
    // carried it up. It is what resolution orders by; absent, `store.rs`
    // falls back to arrival time. Accepted as number or string because a
    // ≥14-digit label is routinely quoted.
    let mscons_version = data
        .get("mscons_version")
        .or_else(|| data.get("version"))
        .and_then(|v| {
            v.as_u64()
                .map(u128::from)
                .or_else(|| v.as_str().and_then(|s| s.parse::<u128>().ok()))
        });
    // The metered plant's physical capacity, when the decode carried it up.
    // It is what makes `CAPACITY` fireable.
    let max_plant_power_kw = data.get("max_plant_power_kw").and_then(json_decimal);

    let parse_at = |v: &serde_json::Value| {
        v.as_str()
            .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok())
    };
    // The day the delivery's values fall on — what `marktd`'s
    // Lokationszuordnung is read at.
    let stichtag = reads_array
        .iter()
        .filter_map(|r| parse_at(&r["dtm_from"]))
        .chain(staende_array.iter().filter_map(|z| parse_at(&z["at"])))
        .min()
        .map_or_else(mako_fristen::heute, mako_fristen::berlin_date);

    // `SG6 LOC+172` names a Marktlokation or a Messlokation. edmd files
    // every value under a Marktlokation, so a Messlokation is placed at
    // `marktd` — on the Lokationszuordnung valid on the values' day.
    let (malo_id, location_melo) = if is_melo(location) {
        let malo = zugeordnet(state, location, "melo", "MALO", stichtag).await?;
        (malo, Some(location.to_owned()))
    } else {
        (location.to_owned(), None)
    };

    let mut reads = Vec::with_capacity(reads_array.len());
    let mut skipped = 0usize;
    for r in reads_array {
        let (Some(from), Some(to)) = (parse_at(&r["dtm_from"]), parse_at(&r["dtm_to"])) else {
            skipped += 1;
            continue;
        };
        // An unparseable quantity is dropped rather than defaulted to zero: a
        // zero-kWh interval is a billable assertion that no energy flowed,
        // which a decode failure does not establish.
        let Some(kwh) = json_decimal(&r["quantity_kwh"]) else {
            skipped += 1;
            continue;
        };
        if from >= to {
            skipped += 1;
            continue;
        }
        reads.push(MeterRead {
            malo_id: malo_id.clone(),
            melo_id: r["melo_id"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| location_melo.clone()),
            dtm_from: from,
            dtm_to: to,
            quantity_kwh: kwh,
            quality: crate::domain::quality_from_label(r["quality"].as_str()),
            pid,
            sparte,
            obis_code: r["obis_code"].as_str().map(str::to_owned),
            tenant: state.tenant.clone(),
            source: IngestionSource::Mscons,
            push_session: Some(process_id.to_string()),
            quality_warnings: None,
            sender_mp_id: sender.cloned(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: Some(time::OffsetDateTime::now_utc()),
            mscons_version,
            ablesung_herkunft: herkunft(r["herkunft"].as_str(), &what)?,
        });
    }

    // A Zählerstand belongs to one register of one meter, and the register
    // store keys on both. A delivery addressed to the Marktlokation names no
    // meter, so it is placed at `marktd` too.
    let mut zaehlerstaende = Vec::with_capacity(staende_array.len());
    let mut malo_melo: Option<String> = None;
    for z in staende_array {
        let (Some(at), Some(value)) = (parse_at(&z["at"]), json_decimal(&z["value"])) else {
            skipped += 1;
            continue;
        };
        let Some(obis_code) = z["obis_code"].as_str().map(str::to_owned) else {
            return Err(ApiError::unprocessable(format!(
                "a Zählerstand names no register (SG9 PIA) — refusing {what}"
            )));
        };
        let melo_id = match z["melo_id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| location_melo.clone())
        {
            Some(m) => m,
            None => match &malo_melo {
                Some(m) => m.clone(),
                None => {
                    let m = zugeordnet(state, &malo_id, "malo", "MELO", stichtag).await?;
                    malo_melo = Some(m.clone());
                    m
                }
            },
        };
        zaehlerstaende.push(MeterReading {
            malo_id: malo_id.clone(),
            read_at: at,
            zaehlerstand: value,
            quality: crate::domain::quality_from_label(z["quality"].as_str()),
            sparte,
            obis_code: Some(obis_code),
            melo_id: Some(melo_id),
            tenant: state.tenant.clone(),
            source: IngestionSource::Mscons,
            sender_mp_id: sender.cloned(),
            push_session: Some(process_id.to_string()),
            ablesung_herkunft: herkunft(z["herkunft"].as_str(), &what)?,
        });
    }

    if skipped > 0 {
        warn!(
            process_id = %process_id, pid, malo_id = %malo_id, skipped,
            "edmd: MSCONS values dropped as undecodable"
        );
    }

    Ok(Decoded {
        malo_id,
        sparte,
        max_plant_power_kw,
        reads,
        zaehlerstaende,
    })
}

/// The one location of `want` type (`MALO`/`MELO`) `marktd`'s
/// Lokationszuordnung links to `id` on `stichtag`.
///
/// None, or more than one, is a delivery edmd cannot file without guessing,
/// and is refused by name; an unreachable `marktd` is a `500`, so the delivery
/// comes back once it answers.
async fn zugeordnet(
    state: &HandlerState,
    id: &str,
    typ: &str,
    want: &str,
    stichtag: time::Date,
) -> Result<String, ApiError> {
    let edges = state
        .marktd
        .get_lokationen(id, typ, Some(&stichtag.to_string()))
        .await
        .map_err(|e| {
            ApiError::internal(anyhow::anyhow!(
                "marktd Lokationszuordnung of {id} on {stichtag}: {e}"
            ))
        })?;
    let mut found: Vec<String> = edges
        .iter()
        .flat_map(|e| {
            [
                (serde_json::to_value(e.von_typ).ok(), &e.von_id),
                (serde_json::to_value(e.nach_typ).ok(), &e.nach_id),
            ]
        })
        .filter(|(t, other)| t.as_ref().and_then(|t| t.as_str()) == Some(want) && *other != id)
        .map(|(_, other)| other.clone())
        .collect();
    found.sort();
    found.dedup();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(ApiError::unprocessable(format!(
            "marktd links {id} to no {want} on {stichtag} — the values cannot be filed"
        ))),
        many => Err(ApiError::unprocessable(format!(
            "marktd links {id} to {} {want}s on {stichtag} ({}) — the values cannot be \
             filed under one",
            many.len(),
            many.join(", ")
        ))),
    }
}

/// Validate, grade and store the interval values; `Some` is the response
/// that ends the delivery.
async fn store_intervals(
    state: &HandlerState,
    process_id: Uuid,
    pid: u32,
    decoded: &Decoded,
) -> Option<axum::response::Response> {
    let malo_id = &decoded.malo_id;
    // Warnings attach to the intervals they name, in the same statement as
    // the readings.
    let (validated, validation) = crate::domain::ValidatedReads::validate(
        decoded.reads.clone(),
        crate::domain::IngestContext::new("MSCONS_VALIDATION", malo_id)
            .with_capacity_kw(decoded.max_plant_power_kw),
    );
    let stored = validated.len();
    // Captured before the batch moves into the store, for the alert below.
    let (period_from, period_to) = crate::domain::batch_period(validated.as_slice());
    // The quality grade and its `quality_assessments` row, as on every other
    // door: the § 147 AO history is only as complete as its least-covered
    // path. Graded from the borrow, recorded once the readings are stored.
    let hampel = crate::server::score_batch(validated.as_slice());
    if let Err(err) = state.repo.store_reads(validated).await {
        // A 5xx makes `marktd` redeliver; answering 204 would mark the
        // process delivered while the readings were never stored. A refused
        // delivery gets 422 instead — redelivering a batch the store will
        // always refuse never terminates.
        return Some(
            ingest_refusal(
                err,
                format!("MSCONS intervals of PID {pid} for MaLo {malo_id} not stored"),
            )
            .into_response(),
        );
    }
    if let Some(q) = &hampel {
        q.record(state.repo.pool(), &state.tenant, malo_id).await;
    }
    info!(
        process_id = %process_id, pid, malo_id = %malo_id, stored,
        issue_count = validation.issue_count,
        "edmd: stored MSCONS intervals"
    );

    // Same emitter, same payload schema and same trigger as every other
    // ingest door — a recipient must not have to special-case MSCONS.
    let alert = crate::server::quality_alert::QualityAlert {
        malo_id,
        door: "mscons",
        correlation_id: &process_id.to_string(),
        causation_id: &process_id.to_string(),
        sparte: Some(decoded.sparte.as_str()),
        period_from,
        period_to,
        validation: &validation,
        hampel: hampel
            .as_ref()
            .map(|q| crate::server::hampel_summary(&q.report)),
    };
    if alert.is_warning() {
        warn!(
            process_id = %process_id, pid, malo_id = %malo_id,
            issue_count = validation.issue_count,
            billing_block_count = validation.billing_block_count,
            "edmd: MSCONS ingest validation issues (§ 60 Abs. 1 MsbG — substitute values may be required)"
        );
    }
    crate::server::quality_alert::raise_quality_warning(state.repo.pool(), &state.tenant, &alert)
        .await;
    None
}
