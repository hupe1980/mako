//! Axum webhook handler for inbound `MarktEvent` CloudEvents from `marktd`.
//!
//! ## Event routing
//!
//! | `ce_type`                    | `makopid` | Action |
//! |------------------------------|-----------|--------|
//! | `de.mako.process.completed`  | MSCONS set | Store `MeterDataReceipt` |
//! | `de.mako.process.initiated`  | 23001 (INSRPT Störungsmeldung) | Auto-create `INSRPT_STOERUNG` reading order (§18 MessZV) |
//! | `de.mako.process.initiated`  | 23003/23008 (INSRPT Technische Änderung/Gerätebefund) | Auto-create `SONDERABLESUNG` reading order |
//! | `de.mako.process.initiated`  | 23005/23009 (WiM Gas INSRPT) | Auto-create `SONDERABLESUNG` reading order |
//! | `de.mako.process.completed`  | 55001 (GPKE Lieferbeginn) | Auto-create `LIEFERBEGINN` reading order |
//! | `de.mako.process.completed`  | 55009 (GPKE Lieferende) | Auto-create `LIEFERENDE` reading order |
//! | *(anything else)*            | *(any)*   | 204 No Content (ignored) |
//!
//! ## M2 — INSRPT → reading-order automation
//!
//! When an INSRPT Störungsmeldung (PID 23001, LF → MSB) arrives, §18 MessZV
//! mandates a Sonderablesung.  `edmd` auto-creates an `ablese_auftraege` row
//! with `anlass = 'INSRPT_STOERUNG'` so field-service scheduling is never
//! blocked on manual ERP input.
//!
//! PIDs 23003/23008 (Technische Änderung / Gerätebefund) and WiM Gas PIDs
//! 23005/23009 trigger `SONDERABLESUNG` orders for similar reasons.
//!
//! PIDs 55001/55009 (Lieferbeginn/Lieferende completion) trigger reading
//! orders to capture the meter reading at the supply handover boundary —
//! required for accurate Mehr-/Mindermengensaldo calculation.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_edm::{
    domain::{
        ALL_MSCONS_PIDS, GAS_QUALITY_PIDS, IngestionSource, MeterDataReceipt, MeterRead,
        QualityFlag, Sparte as EdmSparte,
    },
    repository::TimeSeriesRepository,
};
use mako_markt::cloudevents::verify_signature;
use secrecy::{ExposeSecret, SecretString};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Map an MSCONS quality token onto the domain quality flag.
///
/// An unrecognised or absent token becomes `Unknown`, which the billing
/// aggregates exclude — a reading whose status we cannot interpret must not be
/// settled as if it were measured.
fn quality_from_mscons(status: Option<&str>) -> QualityFlag {
    match status.unwrap_or_default() {
        "MEASURED" => QualityFlag::Measured,
        "ESTIMATED" => QualityFlag::Estimated,
        "SUBSTITUTED" => QualityFlag::Substituted,
        "CALCULATED" => QualityFlag::Calculated,
        "CORRECTED" => QualityFlag::Corrected,
        "PRELIMINARY" => QualityFlag::Preliminary,
        "FAULTY" => QualityFlag::Faulty,
        _ => QualityFlag::Unknown,
    }
}

use crate::iceberg::query::OlapEngine;
use crate::pg::PgTimeSeriesRepository;

/// Shared application state for the webhook handler.
#[derive(Clone)]
pub struct HandlerState {
    pub repo: PgTimeSeriesRepository,
    pub inbound_secret: Arc<Option<SecretString>>,
    /// Tenant identifier — used as Cedar resource_tenant for REST queries.
    pub tenant: String,
    /// DataFusion OLAP engine for Iceberg/S3 queries.
    /// `None` when archival is disabled.
    pub olap_engine: Option<Arc<OlapEngine>>,
    /// `marktd` base URL — used by the Jahresablesung campaign to enumerate SLP MaLos.
    pub marktd_url: String,
    /// `marktd` bearer token.
    pub marktd_api_key: secrecy::SecretString,
    /// ERP webhook URL for outbound CloudEvents from direct push (M4) and quality warnings (M7).
    pub erp_webhook_url: Option<String>,
}

/// `POST /webhook` — receive a `MarktEvent` from `marktd`.
pub async fn handle_webhook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // 1. Verify signature if configured.
    if let Some(secret) = (*state.inbound_secret).as_ref() {
        let provided = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !verify_signature(secret.expose_secret().as_bytes(), &body, provided) {
            warn!("edmd: webhook signature mismatch");
            return (StatusCode::UNAUTHORIZED, "signature mismatch").into_response();
        }
    }

    // 2. Parse JSON body.
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(err) => {
            warn!(%err, "edmd: failed to parse MarktEvent");
            return (StatusCode::BAD_REQUEST, "invalid JSON").into_response();
        }
    };

    let ce_type = event["type"].as_str().unwrap_or("").to_owned();
    // Prefer the forwarded makopid extension; fall back to data["pid"].
    let pid = event["makopid"]
        .as_u64()
        .or_else(|| event["data"]["pid"].as_u64())
        .unwrap_or(0) as u32;

    debug!(ce_type, pid, "edmd: received event");

    // ── M2+: INSRPT → auto-create reading orders ──────────────────────────────
    //
    // PID 23001: Störungsmeldung (LF→MSB) → INSRPT_STOERUNG (§18 MessZV)
    // PID 23003: Technische Änderung / Geräteübernahme → SONDERABLESUNG
    // PID 23005: WiM Gas INSRPT → SONDERABLESUNG
    // PID 23008: Gerätebefund (device inspection) → SONDERABLESUNG
    // PID 23009: WiM Gas INSRPT → SONDERABLESUNG
    if ce_type == "de.mako.process.initiated"
        && matches!(pid, 23001 | 23003 | 23004 | 23005 | 23008 | 23009)
    {
        let (anlass, description) = match pid {
            23001 => ("INSRPT_STOERUNG", "§18 MessZV Störungsmeldung"),
            23003 => ("SONDERABLESUNG", "INSRPT Technische Änderung (PID 23003)"),
            23004 => (
                "SONDERABLESUNG",
                "INSRPT Bestätigung Gerätebefund (PID 23004)",
            ),
            23005 => ("SONDERABLESUNG", "WiM Gas INSRPT (PID 23005)"),
            23008 => ("SONDERABLESUNG", "INSRPT Gerätebefund (PID 23008)"),
            23009 => ("SONDERABLESUNG", "WiM Gas INSRPT (PID 23009)"),
            _ => unreachable!(),
        };

        let process_id_str = event["subject"].as_str().unwrap_or("").to_owned();
        let data = &event["data"];
        let malo_id = data["malo_id"]
            .as_str()
            .or_else(|| data["location_id"].as_str())
            .unwrap_or("")
            .to_owned();
        let melo_id = data["melo_id"].as_str().map(str::to_owned);
        let msb_mp_id = data["msb_mp_id"]
            .as_str()
            .or_else(|| data["receiver"].as_str())
            .map(str::to_owned);

        if malo_id.is_empty() || process_id_str.is_empty() {
            warn!(
                pid,
                "edmd M2+: INSRPT missing malo_id or process_id — skipping"
            );
            return StatusCode::NO_CONTENT.into_response();
        }

        let today = time::OffsetDateTime::now_utc().date();
        let geplant_am = today.next_day().unwrap_or(today);
        let ausfuehrt_bis = geplant_am
            .checked_add(time::Duration::days(7))
            .unwrap_or(geplant_am);

        let pool = state.repo.pool();
        let result = sqlx::query(
            r#"INSERT INTO ablese_auftraege
               (malo_id, melo_id, tenant, anlass, auftraggeber_rolle, ausfuehrender_msb,
                geplant_am, ausfuehrt_bis, insrpt_process_id)
               VALUES ($1, $2, $3, $4, 'MSB', $5, $6, $7, $8)
               ON CONFLICT DO NOTHING"#,
        )
        .bind(&malo_id)
        .bind(&melo_id)
        .bind(&state.tenant)
        .bind(anlass)
        .bind(&msb_mp_id)
        .bind(geplant_am)
        .bind(ausfuehrt_bis)
        .bind(&process_id_str)
        .execute(pool)
        .await;

        match result {
            Ok(r) if r.rows_affected() > 0 => {
                info!(
                    malo_id = %malo_id,
                    process_id = %process_id_str,
                    anlass,
                    geplant_am = %geplant_am,
                    "edmd: auto-created {description} reading order"
                );
            }
            Ok(_) => {
                debug!(
                    malo_id = %malo_id,
                    process_id = %process_id_str,
                    "edmd: {description} reading order already exists — idempotent"
                );
            }
            Err(e) => {
                warn!(error = %e, malo_id = %malo_id, "edmd: failed to create {description} reading order");
            }
        }
        return StatusCode::NO_CONTENT.into_response();
    }

    // ── Lieferbeginn / Lieferende → reading orders ────────────────────────────
    //
    // When a GPKE Lieferbeginn (PID 55001) or Lieferende (PID 55009) process
    // completes, create a reading order to capture the meter reading at the
    // supply handover boundary. This is required for accurate Mehr-/Mindermengensaldo.
    //
    // Legal basis: GPKE BK6-22-024 §3; §9 MessZV Ablesung bei Lieferbeginn/-ende.
    if ce_type == "de.mako.process.completed" && matches!(pid, 55001 | 55009) {
        let (anlass, label) = if pid == 55001 {
            ("LIEFERBEGINN", "Lieferbeginn")
        } else {
            ("LIEFERENDE", "Lieferende")
        };

        let data = &event["data"];
        let malo_id = data["malo_id"]
            .as_str()
            .or_else(|| data["location_id"].as_str())
            .unwrap_or("")
            .to_owned();

        // The reading date is the Lieferbeginndatum / Lieferenclatum from the event.
        // Fall back to today when the field is absent.
        let reading_date_str = data["lieferbeginn_datum"]
            .as_str()
            .or_else(|| data["lieferende_datum"].as_str())
            .or_else(|| data["wechseldatum"].as_str());

        let geplant_am = reading_date_str
            .and_then(|s| {
                use time::format_description::well_known::Iso8601;
                time::Date::parse(s, &Iso8601::DEFAULT).ok()
            })
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());

        let ausfuehrt_bis = geplant_am
            .checked_add(time::Duration::days(3))
            .unwrap_or(geplant_am);

        if !malo_id.is_empty() {
            let process_id_str = event["subject"].as_str().unwrap_or("").to_owned();
            let pool = state.repo.pool();
            let result = sqlx::query(
                r#"INSERT INTO ablese_auftraege
                   (malo_id, tenant, anlass, auftraggeber_rolle,
                    geplant_am, ausfuehrt_bis, insrpt_process_id)
                   VALUES ($1, $2, $3, 'LF', $4, $5, $6)
                   ON CONFLICT DO NOTHING"#,
            )
            .bind(&malo_id)
            .bind(&state.tenant)
            .bind(anlass)
            .bind(geplant_am)
            .bind(ausfuehrt_bis)
            .bind(if process_id_str.is_empty() {
                None
            } else {
                Some(process_id_str.clone())
            })
            .execute(pool)
            .await;

            match result {
                Ok(r) if r.rows_affected() > 0 => {
                    info!(
                        malo_id = %malo_id,
                        anlass,
                        geplant_am = %geplant_am,
                        "edmd: auto-created {label} reading order (§9 MessZV)"
                    );
                }
                Ok(_) => debug!(malo_id = %malo_id, "edmd: {label} reading order already exists"),
                Err(e) => {
                    warn!(error = %e, malo_id = %malo_id, "edmd: failed to create {label} reading order")
                }
            }
        }
        // Fall through to MSCONS handling (55001/55009 are NOT MSCONS PIDs — returns NO_CONTENT)
        return StatusCode::NO_CONTENT.into_response();
    }

    // 3. Route: only process.completed events for known MSCONS PIDs (Messwesen + Redispatch 2.0).
    //
    // `MSCONS_PIDS` = Messwesen PIDs (13005–13027, excl. 13003/13013).
    // `ALL_MSCONS_PIDS` = MSCONS_PIDS + REDISPATCH_MSCONS_PIDS (13020–13026).
    // Redispatch 2.0 Ausfallarbeit/meteorological data (PIDs 13020–13026) must also be stored
    // in `edmd` for OLAP aggregation and archive, even though `mako-redispatch` handles the
    // workflow routing (the two concerns are orthogonal).
    if ce_type == "de.mako.process.completed" && ALL_MSCONS_PIDS.contains(&pid) {
        let subject = event["subject"].as_str().unwrap_or("").to_owned();
        let process_id: Uuid = match subject.parse() {
            Ok(id) => id,
            Err(_) => {
                warn!(subject, "edmd: subject is not a valid UUID — skipping");
                return StatusCode::NO_CONTENT.into_response();
            }
        };

        let data = &event["data"];
        let malo_id = data["malo_id"]
            .as_str()
            .or_else(|| data["location_id"].as_str())
            .unwrap_or("")
            .to_owned();

        if malo_id.is_empty() {
            warn!(process_id = %process_id, pid, "edmd: no malo_id in event data — skipping");
            return StatusCode::NO_CONTENT.into_response();
        }

        let sender_mp_id = data["sender"]
            .as_str()
            .or_else(|| data["sender_mp_id"].as_str())
            .or_else(|| data["partner_mp_id"].as_str())
            .unwrap_or("")
            .to_owned();

        let message_ref = data["message_ref"].as_str().map(str::to_owned);

        let received_at = event["time"]
            .as_str()
            .and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .unwrap_or_else(time::OffsetDateTime::now_utc);

        let receipt = MeterDataReceipt {
            process_id,
            pid,
            malo_id: malo_id.clone(),
            sender_mp_id,
            message_ref,
            received_at,
            tenant: state.tenant.clone(),
        };

        match state.repo.store_receipt(&receipt).await {
            Ok(()) => {
                info!(
                    process_id = %process_id,
                    pid,
                    malo_id = %malo_id,
                    "edmd: stored MSCONS receipt"
                );
            }
            Err(err) => {
                // `marktd` treats 2xx as delivered and will not redeliver, so a
                // failed receipt write must surface as 5xx rather than being
                // logged and forgotten.
                error!(
                    %err, process_id = %process_id,
                    "edmd: failed to store receipt — signalling redelivery"
                );
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }

        // ── PID 13007: update meter_billing_periods with gas quality data ──────
        // The ProcessCompleted payload carries `brennwert_kwh_per_m3` and
        // `zustandszahl` extracted by the makod adapter from `QTY+Z08`/`QTY+Z10`.
        if GAS_QUALITY_PIDS.contains(&pid) {
            let brennwert = data["brennwert_kwh_per_m3"].as_str().map(str::to_owned);
            let zustandszahl = data["zustandszahl"].as_str().map(str::to_owned);
            if brennwert.is_some() || zustandszahl.is_some() {
                match state
                    .repo
                    .update_gas_quality(
                        &state.tenant,
                        &malo_id,
                        brennwert.as_deref(),
                        zustandszahl.as_deref(),
                    )
                    .await
                {
                    Ok(n) => info!(
                        process_id = %process_id, pid, malo_id = %malo_id,
                        rows_updated = n,
                        "edmd: updated gas quality (Brennwert/Zustandszahl) in meter_billing_periods"
                    ),
                    Err(err) => warn!(%err, process_id = %process_id, pid,
                        "edmd: failed to update gas quality"),
                }
            }
        }
        // ── Typed MSCONS interval ingest ──────────────────────────────────
        // The reads carried by a ProcessCompleted event are the primary source
        // of metered data in German MaKo. They are validated (V01–V10) and then
        // stored through the same batched path as every other ingest family, so
        // a MSCONS reading lands with the same key, unit and quality record as
        // one that arrived by direct push.
        if let Some(reads_array) = data["reads"].as_array().filter(|a| !a.is_empty()) {
            let sparte = match data["sparte"]
                .as_str()
                .unwrap_or("STROM")
                .to_uppercase()
                .as_str()
            {
                "GAS" => EdmSparte::Gas,
                "WAERME" | "WÄRME" => EdmSparte::Waerme,
                "WASSER" => EdmSparte::Wasser,
                _ => EdmSparte::Strom,
            };

            let mut batch: Vec<MeterRead> = Vec::with_capacity(reads_array.len());
            let mut skipped = 0usize;
            for r in reads_array {
                use time::format_description::well_known::Rfc3339;
                let (Some(from), Some(to)) = (
                    r["dtm_from"]
                        .as_str()
                        .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok()),
                    r["dtm_to"]
                        .as_str()
                        .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok()),
                ) else {
                    skipped += 1;
                    continue;
                };
                // An unparseable quantity is dropped rather than defaulted to
                // zero: a zero-kWh interval is a billable assertion that no
                // energy flowed, which a decode failure does not establish.
                let Some(kwh) = r["quantity_kwh"]
                    .as_str()
                    .and_then(|s| s.parse::<rust_decimal::Decimal>().ok())
                    .or_else(|| {
                        r["quantity_kwh"]
                            .as_f64()
                            .and_then(rust_decimal::Decimal::from_f64_retain)
                    })
                else {
                    skipped += 1;
                    continue;
                };
                if from >= to {
                    skipped += 1;
                    continue;
                }

                batch.push(MeterRead {
                    malo_id: malo_id.clone(),
                    melo_id: r["melo_id"].as_str().map(str::to_owned),
                    dtm_from: from,
                    dtm_to: to,
                    quantity_kwh: kwh,
                    quality: quality_from_mscons(r["quality"].as_str()),
                    pid,
                    sparte,
                    obis_code: r["obis_code"].as_str().map(str::to_owned),
                    tenant: state.tenant.clone(),
                    source: IngestionSource::Mscons,
                    push_session: Some(process_id.to_string()),
                    quality_warnings: None,
                    sender_mp_id: (!receipt.sender_mp_id.is_empty())
                        .then(|| receipt.sender_mp_id.clone()),
                    allocation_version: "INITIAL".to_owned(),
                    valid_from_tx: Some(time::OffsetDateTime::now_utc()),
                });
            }

            if skipped > 0 {
                warn!(
                    process_id = %process_id, pid, malo_id = %malo_id, skipped,
                    "edmd: MSCONS intervals dropped as undecodable"
                );
            }

            // Warnings attach to the intervals they name, in the same statement
            // as the readings.
            let validation =
                crate::server::validate_and_annotate(&mut batch, "MSCONS_VALIDATION", &malo_id);

            let stored = batch.len();
            if let Err(err) = state.repo.store_reads(&batch).await {
                // A 5xx makes `marktd` redeliver. Answering 204 here would mark
                // the process delivered while the readings were never stored.
                error!(
                    %err, process_id = %process_id, pid, malo_id = %malo_id,
                    "edmd: MSCONS interval store failed — signalling redelivery"
                );
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            info!(
                process_id = %process_id, pid, malo_id = %malo_id, stored,
                issue_count = validation.issue_count,
                "edmd: stored MSCONS intervals"
            );

            if !validation.is_clean() {
                warn!(
                    process_id = %process_id, pid, malo_id = %malo_id,
                    issue_count = validation.issue_count,
                    billing_block_count = validation.billing_block_count,
                    "edmd: MSCONS ingest validation issues (§17 MessZV — substitute values may be required)"
                );

                if let Some(ref webhook_url) = state.erp_webhook_url {
                    let payload = serde_json::json!({
                        "specversion": "1.0",
                        "type": "de.edmd.reading.quality.warning",
                        "source": format!("urn:edmd:tenant:{}:{}", state.tenant, malo_id),
                        "id": uuid::Uuid::new_v4().to_string(),
                        "subject": malo_id,
                        "tenant": state.tenant,
                        "datacontenttype": "application/json",
                        "data": {
                            "malo_id": malo_id, "pid": pid,
                            "process_id": process_id.to_string(),
                            "issue_count": validation.issue_count,
                            "billing_block_count": validation.billing_block_count,
                            "rules": validation.rules,
                        }
                    });
                    // Retry up to 3 times with exponential backoff.
                    let client = mako_service::http::default_client();
                    for attempt in 0u32..3 {
                        match client.post(webhook_url).json(&payload).send().await {
                            Ok(r) if r.status().is_success() => break,
                            Ok(r) => {
                                tracing::warn!(
                                    attempt, status = %r.status(),
                                    "edmd: quality warning webhook non-success"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(attempt, error = %e, "edmd: quality warning webhook failed");
                            }
                        }
                        if attempt < 2 {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                200 * (1 << attempt),
                            ))
                            .await;
                        }
                    }
                }
            }
        }
    } else {
        debug!(ce_type, pid, "edmd: event ignored");
    }

    StatusCode::NO_CONTENT.into_response()
}
