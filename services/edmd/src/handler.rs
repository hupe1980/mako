//! Axum webhook handler for inbound `MarktEvent` CloudEvents from `marktd`.
//!
//! ## Event routing
//!
//! | `ce_type`                    | `makopid` | Action |
//! |------------------------------|-----------|--------|
//! | `de.mako.process.completed`  | MSCONS set | Store `MeterDataReceipt` |
//! | `de.mako.process.initiated`  | 23001 (INSRPT Störungsmeldung) | Auto-create `INSRPT_STOERUNG` reading order (WiM Störungsmeldung) |
//! | `de.mako.process.initiated`  | 23003/23008 (INSRPT Technische Änderung/Gerätebefund) | Auto-create `SONDERABLESUNG` reading order |
//! | `de.mako.process.initiated`  | 23005/23009 (WiM Gas INSRPT) | Auto-create `SONDERABLESUNG` reading order |
//! | `de.mako.process.completed`  | 55001 (GPKE Lieferbeginn) | Auto-create `LIEFERBEGINN` reading order |
//! | `de.mako.process.completed`  | 55004/55007 (GPKE Abmeldung / Beendigung der Zuordnung) | Auto-create `LIEFERENDE` reading order |
//! | *(anything else)*            | *(any)*   | 204 No Content (ignored) |
//!
//! ## INSRPT → reading-order automation
//!
//! When an INSRPT Störungsmeldung (PID 23001, LF → MSB) arrives, WiM Störungsmeldung
//! mandates a Sonderablesung.  `edmd` auto-creates an `ablese_auftraege` row
//! with `anlass = 'INSRPT_STOERUNG'` so field-service scheduling is never
//! blocked on manual ERP input.
//!
//! PIDs 23003/23008 (Technische Änderung / Gerätebefund) and WiM Gas PIDs
//! 23005/23009 trigger `SONDERABLESUNG` orders for similar reasons.
//!
//! PIDs 55001 (Lieferbeginn) and 55004/55007 (Lieferende) completions trigger reading
//! orders to capture the meter reading at the supply handover boundary —
//! required for accurate Mehr-/Mindermengensaldo calculation.

use std::sync::Arc;

/// The status an ingest door answers when the store refused a batch.
///
/// `marktd` treats 2xx as delivered and redelivers on 5xx, so the choice is
/// whether this delivery should come back. A transient failure — a lost
/// connection, a lock the statement declined to queue for — should: the same
/// bytes will store once the condition clears. A **refused** one must not: an
/// overlapping span, a restated value under an existing version or a second
/// network operator on one reading is a statement about the message, and
/// redelivering it is a loop that runs until the retry budget is exhausted
/// while an operator watches a 5xx that never resolves.
///
/// 422 is the honest answer for the second: the delivery was received and
/// understood, and it cannot be stored as sent.
fn ingest_status(err: &crate::domain::error::EdmError) -> StatusCode {
    if err.is_retryable() {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    }
}

use crate::domain::{
    ALL_MSCONS_PIDS, ESA_TYP2_PIDS, GAS_QUALITY_PIDS, IngestionSource, MeterDataReceipt, MeterRead,
    Sparte as EdmSparte, Typ2DeliveryPath, Typ2Read,
    repository::{TimeSeriesRepository, Typ2Repository},
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use secrecy::{ExposeSecret, SecretString};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::store::{MeterStoreTimeSeriesRepository, MeterStoreTyp2Repository};

/// The calendar month `day` falls in, as an inclusive date range.
///
/// The default period for a Gasbeschaffenheit delivery that states none: the
/// grid operator publishes the Abrechnungsbrennwert monthly, so the month is the
/// narrowest honest guess. Applying it to the MaLo's whole history — the former
/// behaviour — silently repriced every past period.
fn month_of(day: time::Date) -> (time::Date, time::Date) {
    let first = day.replace_day(1).unwrap_or(day);
    let last = first
        .replace_day(first.month().length(first.year()))
        .unwrap_or(day);
    (first, last)
}

/// Shared application state for the webhook handler.
#[derive(Clone)]
pub struct HandlerState {
    /// Authoritative meter-data store (hot Postgres + cold Iceberg via meterstore),
    /// plus the edmd business-table pool it exposes through `repo.pool()`.
    pub repo: MeterStoreTimeSeriesRepository,
    /// Separate store for ESA "Werte nach Typ 2" (non-authoritative; never billing).
    pub typ2_repo: MeterStoreTyp2Repository,
    pub inbound_secret: Arc<Option<SecretString>>,
    /// Tenant identifier — used as Cedar resource_tenant for REST queries.
    pub tenant: String,
    /// `marktd` base URL — used by the Jahresablesung campaign to enumerate SLP MaLos.
    pub marktd_url: String,
    /// `marktd` bearer token.
    pub marktd_api_key: secrecy::SecretString,
    /// ERP webhook URL for outbound CloudEvents from direct push and quality warnings.
    pub erp_webhook_url: Option<String>,
    /// Optional secret signing outbound CloudEvents (Standard Webhooks).
    pub erp_webhook_secret: Option<secrecy::SecretString>,
    /// §14a SMGW/CLS compliance thresholds, so the synchronous checks on the
    /// upsert and audit endpoints use the same numbers as the daily sweep. They
    /// were hardcoded `30, 2` at four call sites while the docs called them
    /// configurable.
    pub smgw: crate::config::SmgwConfig,
    /// Delivery-surveillance thresholds, shared by the worker and the on-demand
    /// scan endpoint so both judge by the same numbers.
    pub surveillance: crate::config::SurveillanceConfig,
    /// Whether a real cold tier is configured and its maintenance loop running.
    ///
    /// `false` means meterstore is hot-only against an in-memory warehouse:
    /// nothing is ever archived and the settled history has nowhere to go.
    /// `GET /api/v1/archive/status` reports this rather than a constant, so it
    /// cannot read as "archival is working" on the deployment where it is not.
    pub cold_tier_enabled: bool,
}

impl HandlerState {
    /// The outbound-webhook HMAC secret as bytes, if one is configured. Passed to
    /// `post_ce_with_retry` so every emitted CloudEvent is signed the same way.
    pub(crate) fn webhook_secret_bytes(&self) -> Option<&[u8]> {
        use secrecy::ExposeSecret;
        self.erp_webhook_secret
            .as_ref()
            .map(|s| s.expose_secret().as_bytes())
    }
}

/// `POST /webhook` — receive a `MarktEvent` from `marktd`.
pub async fn handle_webhook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // 1. Verify the HMAC signature — fail closed. This webhook stores meter
    //    readings and auto-creates reading orders, so accepting it without a
    //    configured secret would let anything that can reach the port inject data.
    //    An unconfigured secret is rejected, mirroring edmd's fail-closed OIDC
    //    posture, rather than waved through.
    match (*state.inbound_secret).as_ref() {
        Some(secret) => {
            // The shared verifier, which also refuses a stale
            // `webhook-timestamp`: a replayed reading would be stored twice.
            if let Err(err) = mako_service::webhook::verify_request(
                Some(secret.expose_secret().as_bytes()),
                &headers,
                &body,
            ) {
                warn!(%err, "edmd: inbound webhook refused");
                return (StatusCode::from(err), err.to_string()).into_response();
            }
        }
        None => {
            warn!(
                "edmd: /webhook rejected — no [webhook].inbound_secret configured; refusing \
                 unauthenticated reading ingestion"
            );
            return (
                StatusCode::UNAUTHORIZED,
                "webhook authentication not configured",
            )
                .into_response();
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
    // PID 23001: Störungsmeldung (LF→MSB) → INSRPT_STOERUNG (WiM Störungsmeldung)
    // PID 23003: Technische Änderung / Geräteübernahme → SONDERABLESUNG
    // PID 23005: WiM Gas INSRPT → SONDERABLESUNG
    // PID 23008: Gerätebefund (device inspection) → SONDERABLESUNG
    // PID 23009: WiM Gas INSRPT → SONDERABLESUNG
    if ce_type == mako_events::mako::PROCESS_INITIATED
        && matches!(pid, 23001 | 23003 | 23004 | 23005 | 23008 | 23009)
    {
        let (anlass, description) = match pid {
            23001 => ("INSRPT_STOERUNG", "WiM Störungsmeldung Störungsmeldung"),
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
    // When a GPKE Anmeldung (PID 55001) completes, supply starts; when an
    // Abmeldung (PID 55004, LF-initiated) or a Beendigung der Zuordnung
    // (PID 55007, NB-initiated) completes, supply ends. Both boundaries need a
    // reading order to capture the meter reading at the supply handover —
    // required for an accurate Mehr-/Mindermengensaldo. (PID 55009 is the
    // *Ablehnung* of an Abmeldung — supply continues, no reading is due.)
    //
    // Legal basis: GPKE (BK6-24-174) Teil 1 — Ablesung bei Lieferbeginn/-ende.
    // Not BK6-22-024: GPKE Teil 1–3 were reissued under BK6-24-174, and what
    // stayed behind there is GPKE Teil 4 and WiM Strom Teil 1/2.
    if ce_type == mako_events::mako::PROCESS_COMPLETED && matches!(pid, 55001 | 55004 | 55007) {
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

        // The reading date is the Lieferbeginndatum / Lieferendedatum from the event.
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
                        "edmd: auto-created {label} reading order (GPKE Beginn-/Schlussablesung)"
                    );
                }
                Ok(_) => debug!(malo_id = %malo_id, "edmd: {label} reading order already exists"),
                Err(e) => {
                    warn!(error = %e, malo_id = %malo_id, "edmd: failed to create {label} reading order")
                }
            }
        }
        // Fall through to MSCONS handling (55001/55004/55007 are NOT MSCONS PIDs — returns NO_CONTENT)
        return StatusCode::NO_CONTENT.into_response();
    }

    // 3. Route: only process.completed events for known MSCONS PIDs (Messwesen + Redispatch 2.0).
    //
    // `MSCONS_PIDS` = Messwesen PIDs (13005–13027, excl. 13003/13013).
    // `ALL_MSCONS_PIDS` = MSCONS_PIDS + REDISPATCH_MSCONS_PIDS (13020–13023, 13026).
    // Redispatch 2.0 Ausfallarbeit/meteorological data (PIDs 13020–13023, 13026) must also be stored
    // in `edmd` for OLAP aggregation and archive, even though `mako-redispatch` handles the
    // workflow routing (the two concerns are orthogonal).
    if ce_type == mako_events::mako::PROCESS_COMPLETED && ALL_MSCONS_PIDS.contains(&pid) {
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
                // failed receipt write must surface rather than being logged and
                // forgotten — as 5xx when another attempt could work, as 422
                // when the delivery itself is what the store refused.
                let status = ingest_status(&err);
                error!(
                    %err, process_id = %process_id, retryable = err.is_retryable(),
                    "edmd: failed to store receipt"
                );
                return status.into_response();
            }
        }

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
            let decimal = |key: &str| {
                data[key]
                    .as_str()
                    .and_then(|s| s.parse::<rust_decimal::Decimal>().ok())
                    .or_else(|| {
                        data[key]
                            .as_f64()
                            .and_then(rust_decimal::Decimal::from_f64_retain)
                    })
            };
            let brennwert = decimal("brennwert_kwh_per_m3");
            let zustandszahl = decimal("zustandszahl");
            if brennwert.is_some() || zustandszahl.is_some() {
                let (default_from, default_to) = month_of(received_at.date());
                let date = |key: &str| {
                    data[key].as_str().and_then(|s| {
                        time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE)
                            .ok()
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
        // ── PID 13006: Messwert Storno ────────────────────────────────────
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
            return StatusCode::NO_CONTENT.into_response();
        }

        // ── Typed MSCONS interval ingest ──────────────────────────────────
        // The reads carried by a ProcessCompleted event are the primary source
        // of metered data in German MaKo. They are validated (V01–V09/V11/V12) and then
        // stored through the same batched path as every other ingest family, so
        // a MSCONS reading lands with the same key, unit and quality record as
        // one that arrived by direct push.
        if let Some(reads_array) = data["reads"].as_array().filter(|a| !a.is_empty()) {
            // A payload naming an unknown commodity is a decode fault upstream,
            // not an electricity delivery. `marktd` redelivers on a 5xx, so the
            // batch is refused rather than stored under a guessed Sparte — and a
            // guessed one is stored in the wrong unit: gas settles in kWh_Hs,
            // water in m³.
            let sparte = match data["sparte"].as_str() {
                None => EdmSparte::Strom,
                Some(raw) => match crate::domain::parse_sparte(raw) {
                    Some(s) => s,
                    None => {
                        error!(
                            process_id = %process_id, pid, malo_id = %malo_id, sparte = raw,
                            "edmd: MSCONS payload names an unknown Sparte — refusing the batch"
                        );
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                },
            };

            // The MSCONS correction version the operator assigned, if the
            // decode carried it up. It is what resolution *should* order by;
            // when absent, `store.rs` falls back to arrival time, which
            // reverses a correction that is later replayed against. Accepted as
            // number or string because a ≥14-digit label is routinely quoted.
            let mscons_version = data
                .get("mscons_version")
                .or_else(|| data.get("version"))
                .and_then(|v| {
                    v.as_u64()
                        .map(u128::from)
                        .or_else(|| v.as_str().and_then(|s| s.parse::<u128>().ok()))
                });

            // The metered plant's physical capacity, when the decode carried it
            // up. It is what makes V12 (`ImplausiblePower`) fireable: without a
            // ceiling there is nothing for an average power to be impossible
            // against, and the rule is inert.
            let max_plant_power_kw = data.get("max_plant_power_kw").and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<rust_decimal::Decimal>().ok())
                    .or_else(|| v.as_f64().and_then(rust_decimal::Decimal::from_f64_retain))
            });

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
                    quality: crate::domain::quality_from_label(r["quality"].as_str()),
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
                    mscons_version,
                });
            }

            if skipped > 0 {
                warn!(
                    process_id = %process_id, pid, malo_id = %malo_id, skipped,
                    "edmd: MSCONS intervals dropped as undecodable"
                );
            }

            // ── ESA "Werte nach Typ 2" (PID 13027) fork ───────────────────────
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
                let typ2: Vec<Typ2Read> = batch
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
                    let status = ingest_status(&err);
                    error!(
                        %err, process_id = %process_id, pid, malo_id = %malo_id,
                        retryable = err.is_retryable(),
                        "edmd: ESA Typ-2 store failed"
                    );
                    return status.into_response();
                }
                info!(
                    process_id = %process_id, pid, malo_id = %malo_id, stored,
                    "edmd: stored ESA Typ-2 values (non-authoritative, separate store)"
                );
                return StatusCode::NO_CONTENT.into_response();
            }

            // Warnings attach to the intervals they name, in the same statement
            // as the readings.
            let (validated, validation) = crate::domain::ValidatedReads::validate(
                batch,
                crate::domain::IngestContext::new("MSCONS_VALIDATION", &malo_id)
                    .with_capacity_kw(max_plant_power_kw),
            );

            let stored = validated.len();
            // Captured before the batch moves into the store, for the alert below.
            let (period_from, period_to) = crate::domain::batch_period(validated.as_slice());
            // The Hampel grade and its `quality_assessments` row, as on every
            // other door: the § 147 AO history is only as complete as its
            // least-covered path. Graded from the borrow, recorded once the
            // readings are stored.
            let hampel = crate::server::score_batch(validated.as_slice());
            if let Err(err) = state.repo.store_reads(validated).await {
                // A 5xx makes `marktd` redeliver; answering 204 would mark the
                // process delivered while the readings were never stored. A
                // refused delivery gets 422 instead — redelivering a batch the
                // store will always refuse never terminates.
                let status = ingest_status(&err);
                error!(
                    %err, process_id = %process_id, pid, malo_id = %malo_id,
                    retryable = err.is_retryable(),
                    "edmd: MSCONS interval store failed"
                );
                return status.into_response();
            }

            if let Some(q) = &hampel {
                q.record(state.repo.pool(), &state.tenant, &malo_id).await;
            }

            info!(
                process_id = %process_id, pid, malo_id = %malo_id, stored,
                issue_count = validation.issue_count,
                "edmd: stored MSCONS intervals"
            );

            // Same emitter, same payload schema and same trigger as every other
            // ingest door — a recipient must not have to special-case MSCONS.
            let alert = crate::server::quality_alert::QualityAlert {
                malo_id: &malo_id,
                door: "mscons",
                correlation_id: &process_id.to_string(),
                causation_id: &process_id.to_string(),
                sparte: Some(sparte.as_str()),
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
                    "edmd: MSCONS ingest validation issues (§ 60 Abs. 2 MsbG — substitute values may be required)"
                );
            }
            crate::server::quality_alert::raise_quality_warning(
                state.erp_webhook_url.as_deref(),
                state.webhook_secret_bytes(),
                &state.tenant,
                &alert,
            )
            .await;
        }
    } else {
        debug!(ce_type, pid, "edmd: event ignored");
    }

    StatusCode::NO_CONTENT.into_response()
}
