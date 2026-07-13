//! Axum webhook handler for inbound `MarktEvent` CloudEvents.
//!
//! ## Inbound event routing
//!
//! | `ce_type`                    | `data.pid`      | Action                       |
//! |------------------------------|-----------------|------------------------------|
//! | `de.mako.process.initiated`  | INVOIC PID set  | Run plausibility check       |
//! | *(anything else)*            | *(any)*         | 204 No Content (ignored)     |
//!
//! ### INVOIC PID routing table
//!
//! | PID(s) | Domain crate | Price sheet | Commands |
//! |--------|-------------|-------------|---------|
//! | 31001, 31002, 31005, 31006 | mako-gpke | PreisblattNetznutzung | gpke.abrechnung.annehmen / ablehnen |
//! | 31003, 31011 | mako-wim-gas / mako-geli-gas | PreisblattNetznutzung Gas | wim.gas.rechnung.annehmen / wim.geli.gas.rechnung.annehmen |
//! | 31004 | mako-wim-gas (Stornorechnung) | — (auto-accept) | wim.gas.stornorechnung.annehmen |
//! | 31007, 31008 | mako-gabi-gas | PreisblattNetznutzung Gas + MMM check | gabi.gas.mmm.rechnung.annehmen / ablehnen |
//! | 31009 | mako-wim | PreisblattMessung (MSB) | wim.rechnung.annehmen / ablehnen |

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use invoic_checker::{CheckConfig, CheckOutcome, InvoicCheckEngine};
use mako_markt::{
    cloudevents::verify_signature,
    makod_client::{ForwardCommand, MakodClient},
};
use rubo4e::current::Rechnung;
use secrecy::{ExposeSecret, SecretString};
use time::OffsetDateTime;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::pg;

/// GPKE INVOIC PIDs that embed a `Rechnung` BO4E object in `ProcessInitiated`.
///
/// Only these PIDs are handled via the embedded-rechnung fast path.  All others
/// fall back to the makod GET endpoint or use dedicated ingestors.
///
/// Source: PID ownership table, BK6-24-174.
const INVOIC_PIDS: &[u32] = &[31001, 31002, 31005, 31006];

/// Gas billing PIDs (WiM Gas + GeLi Gas + GaBi Gas) that embed a `Rechnung`
/// in `ProcessInitiated` following the same pattern as GPKE billing PIDs.
///
/// | PID   | Description                                | Crate           |
/// |-------|--------------------------------------------|-----------------|
/// | 31003 | WiM Gas Rechnung (Gas NNE, NB → LF)        | mako-wim-gas    |
/// | 31007 | GaBi Gas MMM-Rechnung (NB → MGV)           | mako-gabi-gas   |
/// | 31008 | GaBi Gas selbst ausgest. MMM-Rechnung      | mako-gabi-gas   |
/// | 31011 | GeLi Gas Rechnung sonstige Leistung (AWH)  | mako-geli-gas   |
///
/// PID 31004 (WiM Gas Stornorechnung) is handled separately because it
/// auto-approves without a tariff check.
const GAS_INVOIC_PIDS: &[u32] = &[31003, 31007, 31008, 31011];

/// GaBi Gas MMM PIDs — these need the additional MMM settlement price check
/// (check 6) against `marktd` MMMA Gas prices.
const GABI_GAS_MMM_PIDS: &[u32] = &[31007, 31008];

/// Strom MMM PIDs — these need check 6 against `marktd` MMMA Strom prices.
const STROM_MMM_PIDS: &[u32] = &[31002, 31005];

/// Shared application state for the webhook handler.
#[derive(Clone)]
pub struct HandlerState {
    pub preisblatt_client: mako_markt::marktd_client::MarktdClient,
    pub makod: MakodClient,
    pub check_config: Arc<CheckConfig>,
    pub inbound_secret: Arc<Option<SecretString>>,
    /// When `total_net_invoic` (in EUR-cents) exceeds this value, a `Warn`
    /// outcome is escalated to a `Dispute` instead of automatic approval.
    /// `0` means `Warn` is always approved.
    pub auto_dispute_threshold_eur_cents: i64,
    /// PostgreSQL pool for persisting receipts (§22 MessZV compliance).
    /// `None` in development mode — receipts are NOT persisted.
    pub pool: Option<sqlx::PgPool>,
    /// Operator tenant identifier written to every receipt row.
    pub tenant: String,
    /// Optional ERP webhook URL for outbound payment CloudEvents.
    pub erp_webhook_url: Option<String>,
    /// `edmd` base URL for `MeterBillingPeriod` queries in selbstausstellen.
    /// When `None`, PID 31006 selbstausstellen returns 503.
    pub edmd_url: Option<String>,
    /// `edmd` API key (Bearer token).
    pub edmd_api_key: Option<secrecy::SecretString>,
    /// Optional HMAC-SHA256 secret for signing outbound ERP webhook requests.
    ///
    /// When set, every outbound ERP POST includes an `X-Mako-Signature: sha256=<hex>`
    /// header so the ERP can verify authenticity.
    pub erp_hmac_secret: Option<SecretString>,
    /// HTTP client for ERP payment event delivery.
    pub http_client: reqwest::Client,
}

/// `POST /webhook` — receive a `MarktEvent` from `marktd`.
pub async fn handle_webhook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── 1. Verify signature if configured ────────────────────────────────────
    if let Some(secret) = (*state.inbound_secret).as_ref() {
        let provided = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !verify_signature(secret.expose_secret().as_bytes(), &body, provided) {
            warn!("invoicd: webhook signature mismatch");
            return (StatusCode::UNAUTHORIZED, "signature mismatch").into_response();
        }
    }

    // ── 2. Parse JSON body ────────────────────────────────────────────────────
    // `MarktEvent` implements only `Serialize`; parse as a generic JSON value to
    // avoid coupling to an internal `Deserialize` impl.
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(err) => {
            warn!(%err, "invoicd: failed to parse MarktEvent");
            return (StatusCode::BAD_REQUEST, "invalid JSON").into_response();
        }
    };

    let ce_type = event["type"].as_str().unwrap_or("").to_owned();
    let data = &event["data"];
    let pid = extract_pid(data);

    debug!(ce_type, pid, "invoicd: received event");

    // ── 3. Route by ce_type + pid ─────────────────────────────────────────────
    if ce_type == "de.mako.process.initiated" && INVOIC_PIDS.contains(&pid) {
        let subject = event["subject"].as_str().unwrap_or("").to_owned();
        handle_invoic_initiated(state, subject, data.clone()).await;
    } else if ce_type == "de.mako.process.initiated" && GAS_INVOIC_PIDS.contains(&pid) {
        let subject = event["subject"].as_str().unwrap_or("").to_owned();
        handle_gas_invoic_initiated(state, subject, pid, data.clone()).await;
    } else if ce_type == "de.mako.process.initiated" && pid == 31004 {
        // WiM Gas Stornorechnung: auto-accept without tariff check.
        let subject = event["subject"].as_str().unwrap_or("").to_owned();
        handle_gas_stornorechnung(state, subject, data.clone()).await;
    } else if ce_type == "de.mako.process.initiated" && pid == 31009 {
        // WiM MSB-Rechnung (PID 31009): uses PreisblattMessung, not NNE.
        let subject = event["subject"].as_str().unwrap_or("").to_owned();
        handle_wim_31009_initiated(state, subject, data.clone()).await;
    } else {
        debug!(ce_type, pid, "invoicd: event ignored");
    }

    StatusCode::NO_CONTENT.into_response()
}

// ── INVOIC plausibility check ─────────────────────────────────────────────────

async fn handle_invoic_initiated(state: HandlerState, subject: String, data: serde_json::Value) {
    let process_id = match subject.parse::<Uuid>() {
        Ok(id) => id,
        Err(_) => {
            warn!(
                subject,
                "invoicd: process.initiated event has no parseable UUID subject"
            );
            return;
        }
    };

    // Extract fields from the event data payload.
    let pid = extract_pid(&data);
    let sender_mp_id = data["sender_mp_id"].as_str().unwrap_or("").to_owned();
    // `invoice_ref` is the EDIFACT INVOIC message-reference used as the
    // business key in `dispatch_to_process`.  It must be forwarded to makod
    // so the command can be routed to the correct billing process.
    let invoice_ref = data["invoice_ref"].as_str().unwrap_or("").to_owned();
    if invoice_ref.is_empty() {
        warn!(
            pid,
            "invoicd: invoice_ref missing from process.initiated payload — cannot dispatch command"
        );
        return;
    }
    let rechnung_value = data["rechnung"].clone();
    // GLN of the invoice receiver (tenant = our own GLN for all Inbound PIDs).
    let receiver_gln = data["receiver_gln"]
        .as_str()
        .unwrap_or(&state.tenant)
        .to_owned();

    // Deserialize the Rechnung BO4E object embedded by GpkeAbrechnungWorkflow.
    let rechnung: Rechnung = match serde_json::from_value(rechnung_value.clone()) {
        Ok(r) => r,
        Err(err) => {
            warn!(%err, pid, "invoicd: could not deserialize Rechnung from event payload — skipping check");
            return;
        }
    };

    let received_at = OffsetDateTime::now_utc();

    // Extract malo_id at ingest time for the indexed DB column (avoids JSONB scan in zahlungsstatus).
    let malo_id: Option<String> = rechnung
        .marktlokation
        .as_ref()
        .and_then(|ml| ml.marktlokations_id.as_ref())
        .map(|id| id.to_string())
        .or_else(|| data["malo_id"].as_str().map(str::to_owned));

    // Use billing_period() start or invoice document date for price-sheet lookup.
    let billing_date: time::Date = rechnung
        .billing_period()
        .map(|(start, _)| start)
        .or(rechnung.rechnungsdatum)
        .unwrap_or(time::macros::date!(2025 - 01 - 01));

    // Detect Stornierung: when ist_storno=true, use arithmetic-only check to
    // avoid false TariffDeviation disputes on negated cancellation amounts.
    // invoic_checker::is_stornierung() checks rechnung.ist_storno == Some(true).
    let storno = invoic_checker::is_stornierung(&rechnung);

    // Run the stateless plausibility check.
    let report = if storno {
        // Stornierung: stages 0–3 only (Storno ref + period + arithmetic + total).
        InvoicCheckEngine::check_storno(pid, &rechnung, &state.check_config)
    } else {
        let sheet = state
            .preisblatt_client
            .get_preisblatt(&sender_mp_id, billing_date)
            .await
            .ok()
            .flatten();
        let preisblatt_store = {
            use invoic_checker::tariff::InMemoryPreisblattStore;
            let mut store = InMemoryPreisblattStore::new();
            if let Some(s) = sheet {
                store.insert(sender_mp_id.clone(), s);
            }
            store
        };
        InvoicCheckEngine::check(
            pid,
            &sender_mp_id,
            &rechnung,
            &preisblatt_store,
            &state.check_config,
        )
    };

    // ── Check 6 (MMM settlement prices — Strom only) ─────────────────────────
    // For Strom MMM PIDs (31002/31005), validate that Mehrmengen/Mindermengen
    // position prices match the MMMA reference stored in `marktd`.
    // Gas MMM PIDs (31007/31008) are handled by handle_gas_invoic_initiated.
    let report = {
        if !storno && STROM_MMM_PIDS.contains(&pid) {
            let billing_date = rechnung
                .billing_period()
                .map(|(s, _)| s)
                .or(rechnung.rechnungsdatum)
                .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
            let (y, m) = (billing_date.year(), billing_date.month() as u8);

            // Strom MMM prices: sender IS the NB (ÜNB per §22 StromNZV).
            let mmm_prices = state
                .preisblatt_client
                .get_mmm_strom(y, m, &sender_mp_id)
                .await
                .ok()
                .flatten()
                .map(|r| (r.mehr_ct_kwh, r.minder_ct_kwh));

            if let Some((mehr_ct, minder_ct)) = mmm_prices {
                let mmm_findings = InvoicCheckEngine::check_mmm_settlement(
                    &rechnung,
                    mehr_ct,
                    minder_ct,
                    &state.check_config,
                );
                if !mmm_findings.is_empty() {
                    use invoic_checker::CheckOutcome;
                    let extra_outcome = mmm_findings
                        .iter()
                        .map(|f| {
                            if f.is_dispute {
                                CheckOutcome::Dispute
                            } else {
                                CheckOutcome::Warn
                            }
                        })
                        .max()
                        .unwrap_or(CheckOutcome::Ok);
                    let merged_outcome = report.outcome.max(extra_outcome);
                    let mut merged = report;
                    merged.findings.extend(mmm_findings);
                    merged.outcome = merged_outcome;
                    merged
                } else {
                    report
                }
            } else {
                // MMMA prices not yet imported for this month — skip check 6.
                // Logged at debug level to avoid noise in NB deployments without MMMA data.
                tracing::debug!(
                    pid,
                    year = y,
                    month = m,
                    "invoicd: MMMA prices not found in marktd — MMM settlement check skipped"
                );
                report
            }
        } else {
            report
        }
    };

    let checked_at = OffsetDateTime::now_utc();

    info!(
        process_id = %process_id,
        pid,
        outcome = ?report.outcome,
        line_items = report.line_items_checked,
        findings = report.findings.len(),
        "invoicd: INVOIC check complete"
    );

    // Decide action based on outcome.
    let should_dispute = match report.outcome {
        CheckOutcome::Ok => false,
        CheckOutcome::Warn => {
            // Escalate if the invoice total exceeds the configured threshold.
            state.auto_dispute_threshold_eur_cents > 0
                && report
                    .total_net_invoic
                    .map(|t| t.to_raw() > state.auto_dispute_threshold_eur_cents)
                    .unwrap_or(false)
        }
        CheckOutcome::Dispute => true,
    };

    let outcome_str = match (report.outcome, should_dispute, storno) {
        (_, _, true) if !should_dispute => "AcceptedPartial",
        (CheckOutcome::Ok, _, _) => "Ok",
        (CheckOutcome::Warn, false, _) => "Warn",
        _ => "Dispute",
    };

    // ── §22 MessZV: persist receipt BEFORE dispatching ────────────────────────
    //
    // The receipt must be written before the REMADV/COMDIS command is sent.
    // If persistence fails we log an error but still dispatch — the REMADV
    // deadline is regulatory; a DB failure is an operational incident.
    // Operators must monitor `invoic_receipts WHERE dispatched_at IS NULL`.
    if let Some(pool) = &state.pool {
        let findings_json =
            serde_json::to_value(&report.findings).unwrap_or(serde_json::Value::Array(vec![]));
        // Extract Zahlungsziel (DTM+92) from the Rechnung for the pay_by column.
        // rubo4e v0.5 `Rechnung.faelligkeitsdatum` is `Option<time::Date>`;
        // the DB column is TIMESTAMPTZ — store as midnight UTC.
        let pay_by: Option<time::OffsetDateTime> = rechnung
            .faelligkeitsdatum
            .map(|d| d.with_time(time::Time::MIDNIGHT).assume_utc());
        let row = pg::ReceiptRow {
            process_id,
            pid: pid as i16,
            direction: "Inbound".to_owned(),
            sender_mp_id: sender_mp_id.clone(),
            receiver_gln,
            malo_id: malo_id.clone(),
            rechnung: rechnung_value,
            bo4e_version: "v202607.0.0".to_owned(),
            outcome: outcome_str.to_owned(),
            findings: findings_json,
            pay_by,
            received_at,
            checked_at,
            dispatched_at: None,
            tenant: state.tenant.clone(),
        };
        if let Err(err) = pg::upsert_receipt(pool, &row).await {
            warn!(
                %err,
                process_id = %process_id,
                pid,
                "invoicd: failed to persist receipt — §22 MessZV compliance gap; continuing with dispatch"
            );
        }
    } else {
        warn!(
            process_id = %process_id,
            pid,
            "invoicd: no database configured — receipt NOT persisted (§22 MessZV violation in production)"
        );
    }

    // ── Dispatch REMADV or COMDIS to makod ────────────────────────────────────
    if should_dispute {
        let reason = dispute_reason(&report.findings);
        warn!(
            process_id = %process_id,
            pid,
            reason = %reason,
            "invoicd: disputing invoice"
        );
        let idempotency_key = Uuid::new_v5(&process_id, b"dispute").to_string();
        let cmd = ForwardCommand {
            marktrolle: None,
            command: "gpke.abrechnung.ablehnen".to_owned(),
            malo_id: None,
            melo_id: None,
            payload: serde_json::json!({ "invoice_ref": invoice_ref, "ablehnungsgrund": reason }),
        };
        match state.makod.post_command(&idempotency_key, &cmd).await {
            Ok(_) => {
                if let Some(pool) = &state.pool
                    && let Err(err) =
                        pg::receipts::mark_dispatched(pool, process_id, OffsetDateTime::now_utc())
                            .await
                {
                    warn!(%err, process_id = %process_id, "invoicd: failed to mark receipt as dispatched");
                }
            }
            Err(err) => {
                warn!(%err, process_id = %process_id, "invoicd: failed to submit dispute command");
            }
        }
    } else {
        info!(process_id = %process_id, pid, invoice_ref = %invoice_ref, "invoicd: approving invoice");
        let idempotency_key = Uuid::new_v5(&process_id, b"settle").to_string();
        let cmd = ForwardCommand {
            marktrolle: None,
            command: "gpke.abrechnung.annehmen".to_owned(),
            malo_id: None,
            melo_id: None,
            payload: serde_json::json!({ "invoice_ref": invoice_ref }),
        };
        match state.makod.post_command(&idempotency_key, &cmd).await {
            Ok(_) => {
                if let Some(pool) = &state.pool
                    && let Err(err) =
                        pg::receipts::mark_dispatched(pool, process_id, OffsetDateTime::now_utc())
                            .await
                {
                    warn!(%err, process_id = %process_id, "invoicd: failed to mark receipt as dispatched");
                }
            }
            Err(err) => {
                warn!(%err, process_id = %process_id, "invoicd: failed to submit settle command");
            }
        }
        // Notify ERP of payment outcome (best-effort).
        emit_payment_event(
            &state,
            PaymentEventCtx {
                process_id,
                pid,
                direction: "Inbound",
                sender_mp_id: &sender_mp_id,
                outcome: outcome_str,
                pay_by: rechnung.faelligkeitsdatum,
                findings_count: report.findings.len(),
            },
        )
        .await;
    }
}

// ── WiM 31009 ingestor ────────────────────────────────────────────────────────

/// Handle a `de.mako.process.initiated` event for PID 31009 (WiM MSB-Rechnung).
///
/// # Design note
///
/// The `wim-rechnung` workflow embeds the `Rechnung` BO4E object directly in the
/// `ProcessInitiated` outbox payload (same pattern as GPKE abrechnung, since
/// `FV2025-10-01` / workspace 0.8.0). This function extracts it and runs the
/// same validation pipeline as `handle_invoic_initiated`.
///
/// **Fallback**: if the rechnung is absent from the payload (e.g. a process
/// started before the cutover), we fall back to calling
/// `GET /api/v1/invoic/{process_id}/rechnung` on `makod`. If that also returns
/// nothing, we write a DLQ entry so the operator can reconcile manually.
///
/// Source: WiM AHB BK6-24-174 PID 31009 (MSB-Rechnung, MSB → LF).
async fn handle_wim_31009_initiated(state: HandlerState, subject: String, data: serde_json::Value) {
    let process_id = match subject.parse::<Uuid>() {
        Ok(id) => id,
        Err(_) => {
            warn!(
                subject,
                "invoicd: WiM 31009 event has no parseable UUID subject"
            );
            return;
        }
    };

    let sender_mp_id = data["sender_mp_id"].as_str().unwrap_or("").to_owned();
    let invoice_ref = data["invoice_ref"].as_str().unwrap_or("").to_owned();
    let receiver_gln = data["receiver_gln"]
        .as_str()
        .unwrap_or(&state.tenant)
        .to_owned();

    // ── 1. Try the embedded rechnung (present since workspace 0.8.0) ──────────
    let rechnung_value = if data["rechnung"].is_object() {
        data["rechnung"].clone()
    } else {
        // ── 2. Fallback: fetch from makod rechnung endpoint ───────────────────
        info!(
            process_id = %process_id,
            "invoicd: WiM 31009 rechnung not in payload — fetching from makod"
        );
        match state.makod.get_invoic_rechnung(process_id).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                warn!(
                    process_id = %process_id,
                    "invoicd: WiM 31009 rechnung not available from makod — writing DLQ"
                );
                write_31009_dlq(&state, &data, process_id).await;
                return;
            }
            Err(e) => {
                warn!(
                    %e,
                    process_id = %process_id,
                    "invoicd: WiM 31009 makod rechnung fetch failed — writing DLQ"
                );
                write_31009_dlq(&state, &data, process_id).await;
                return;
            }
        }
    };

    // ── 3. Deserialize Rechnung ───────────────────────────────────────────────
    let rechnung: Rechnung = match serde_json::from_value(rechnung_value.clone()) {
        Ok(r) => r,
        Err(err) => {
            warn!(%err, process_id = %process_id, pid = 31009,
                "invoicd: WiM 31009 could not deserialize Rechnung — writing DLQ");
            write_31009_dlq(&state, &data, process_id).await;
            return;
        }
    };

    let received_at = OffsetDateTime::now_utc();
    let billing_date: time::Date = rechnung
        .billing_period()
        .map(|(start, _)| start)
        .or(rechnung.rechnungsdatum)
        .unwrap_or(time::macros::date!(2025 - 01 - 01));

    // ── 4. Plausibility check — PID 31009 uses PreisblattMessung + AufAbschlag ─
    //
    // The MSB-Rechnung (31009) is validated against `PreisblattMessung` (MSB
    // metering service price sheet), NOT `PreisblattNetznutzung` (NNE tariff).
    //
    // Check 6 (AufAbschlag validation): discount positions are validated against
    // contracted AufAbschlag entries from the PreisblattMessung.  This prevents
    // the MSB from adding undocumented discount lines to the invoice.
    // Source: WiM AHB BK6-24-174, PRICAT 27001–27003 (MSB AufAbschlag).
    let report = {
        let preisblatt_messung = state
            .preisblatt_client
            .get_preisblatt_messung(&sender_mp_id, billing_date)
            .await
            .ok()
            .flatten();

        // Extract contracted AufAbschlag names from PreisblattMessung.
        // `auf_abschlaege` is an extension field — extract from JSON extension data.
        let aufabschlag_names: Vec<String> = preisblatt_messung
            .as_ref()
            .and_then(|pm| {
                use rubo4e::json::Bo4eExtensionData as _;
                pm.extension_data()
                    .get("auf_abschlaege")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| {
                                e.get("name").and_then(|n| n.as_str()).map(str::to_owned)
                            })
                            .collect()
                    })
            })
            .unwrap_or_default();

        InvoicCheckEngine::check_msb_rechnung_with_aufabschlaege(
            &sender_mp_id,
            &rechnung,
            preisblatt_messung.as_ref(),
            &aufabschlag_names,
            &state.check_config,
        )
    };

    let checked_at = OffsetDateTime::now_utc();

    info!(
        process_id = %process_id,
        pid = 31009,
        outcome = ?report.outcome,
        "invoicd: WiM 31009 check complete"
    );

    let should_dispute = match report.outcome {
        CheckOutcome::Ok => false,
        CheckOutcome::Warn => {
            state.auto_dispute_threshold_eur_cents > 0
                && report
                    .total_net_invoic
                    .map(|t| t.to_raw() > state.auto_dispute_threshold_eur_cents)
                    .unwrap_or(false)
        }
        CheckOutcome::Dispute => true,
    };

    let outcome_str = match (report.outcome, should_dispute) {
        (CheckOutcome::Ok, _) => "Ok",
        (CheckOutcome::Warn, false) => "Warn",
        _ => "Dispute",
    };

    // ── 5. §22 MessZV: persist receipt BEFORE dispatching ────────────────────
    if let Some(pool) = &state.pool {
        let findings_json =
            serde_json::to_value(&report.findings).unwrap_or(serde_json::Value::Array(vec![]));
        let pay_by: Option<time::OffsetDateTime> = rechnung
            .faelligkeitsdatum
            .map(|d| d.with_time(time::Time::MIDNIGHT).assume_utc());
        let malo_id_31009: Option<String> = rechnung
            .marktlokation
            .as_ref()
            .and_then(|ml| ml.marktlokations_id.as_ref())
            .map(|id| id.to_string())
            .or_else(|| data["malo_id"].as_str().map(str::to_owned));
        let row = pg::ReceiptRow {
            process_id,
            pid: 31009_i16,
            direction: "Inbound".to_owned(),
            sender_mp_id: sender_mp_id.clone(),
            receiver_gln,
            malo_id: malo_id_31009,
            rechnung: rechnung_value,
            bo4e_version: "v202607.0.0".to_owned(),
            outcome: outcome_str.to_owned(),
            findings: findings_json,
            pay_by,
            received_at,
            checked_at,
            dispatched_at: None,
            tenant: state.tenant.clone(),
        };
        if let Err(err) = pg::upsert_receipt(pool, &row).await {
            warn!(
                %err, process_id = %process_id, pid = 31009,
                "invoicd: WiM 31009 failed to persist receipt — §22 MessZV gap; continuing"
            );
        }
    }

    // ── 6. Dispatch Settle or Dispute command to makod ────────────────────────
    if should_dispute {
        let reason = dispute_reason(&report.findings);
        warn!(process_id = %process_id, pid = 31009, reason = %reason,
            "invoicd: WiM 31009 disputing invoice");
        let idem = Uuid::new_v5(&process_id, b"wim31009-dispute").to_string();
        let cmd = ForwardCommand {
            marktrolle: None,
            command: "wim.rechnung.ablehnen".to_owned(),
            malo_id: None,
            melo_id: None,
            payload: serde_json::json!({ "invoice_ref": invoice_ref, "ablehnungsgrund": reason }),
        };
        match state.makod.post_command(&idem, &cmd).await {
            Ok(_) => {
                if let Some(pool) = &state.pool {
                    let _ =
                        pg::receipts::mark_dispatched(pool, process_id, OffsetDateTime::now_utc())
                            .await;
                }
            }
            Err(e) => {
                warn!(%e, process_id = %process_id, "invoicd: WiM 31009 dispute dispatch failed")
            }
        }
    } else {
        info!(process_id = %process_id, pid = 31009, invoice_ref = %invoice_ref,
            "invoicd: WiM 31009 approving invoice");
        let idem = Uuid::new_v5(&process_id, b"wim31009-settle").to_string();
        let cmd = ForwardCommand {
            marktrolle: None,
            command: "wim.rechnung.annehmen".to_owned(),
            malo_id: None,
            melo_id: None,
            payload: serde_json::json!({ "invoice_ref": invoice_ref }),
        };
        match state.makod.post_command(&idem, &cmd).await {
            Ok(_) => {
                if let Some(pool) = &state.pool {
                    let _ =
                        pg::receipts::mark_dispatched(pool, process_id, OffsetDateTime::now_utc())
                            .await;
                }
            }
            Err(e) => {
                warn!(%e, process_id = %process_id, "invoicd: WiM 31009 settle dispatch failed")
            }
        }
    }
}

// ── Gas INVOIC ingestor (PIDs 31003, 31007, 31008, 31011) ────────────────────

/// Handle a `de.mako.process.initiated` event for Gas billing PIDs.
///
/// Covers:
/// - PID 31003 — WiM Gas Rechnung (Gas NNE, NB → LF, `mako-wim-gas`)
/// - PID 31007 — GaBi Gas MMM-Rechnung (NB → MGV, `mako-gabi-gas`) + MMM check 6
/// - PID 31008 — GaBi Gas selbst ausgest. MMM-Rechnung + MMM check 6
/// - PID 31011 — GeLi Gas Rechnung sonstige Leistung / AWH Sperrprozesse (NB → LF, `mako-geli-gas`)
///
/// Uses the same embedded-rechnung fast path as the GPKE handler.
/// PIDs 31007/31008 additionally run the MMM Gas settlement price check (check 6).
async fn handle_gas_invoic_initiated(
    state: HandlerState,
    subject: String,
    pid: u32,
    data: serde_json::Value,
) {
    let process_id = match subject.parse::<Uuid>() {
        Ok(id) => id,
        Err(_) => {
            warn!(
                subject,
                pid, "invoicd: Gas invoice event has no parseable UUID subject"
            );
            return;
        }
    };

    let sender_mp_id = data["sender_mp_id"].as_str().unwrap_or("").to_owned();
    let invoice_ref = data["invoice_ref"].as_str().unwrap_or("").to_owned();
    if invoice_ref.is_empty() {
        warn!(pid, "invoicd: Gas invoice invoice_ref missing from payload");
        return;
    }
    let receiver_gln = data["receiver_gln"]
        .as_str()
        .unwrap_or(&state.tenant)
        .to_owned();
    let rechnung_value = data["rechnung"].clone();

    let rechnung: Rechnung = match serde_json::from_value(rechnung_value.clone()) {
        Ok(r) => r,
        Err(err) => {
            warn!(%err, pid, "invoicd: Gas invoice could not deserialize Rechnung — writing DLQ");
            write_gas_dlq(&state, &data, process_id, pid).await;
            return;
        }
    };

    let received_at = OffsetDateTime::now_utc();
    let malo_id: Option<String> = rechnung
        .marktlokation
        .as_ref()
        .and_then(|ml| ml.marktlokations_id.as_ref())
        .map(|id| id.to_string())
        .or_else(|| data["malo_id"].as_str().map(str::to_owned));

    let billing_date: time::Date = rechnung
        .billing_period()
        .map(|(start, _)| start)
        .or(rechnung.rechnungsdatum)
        .unwrap_or(time::macros::date!(2025 - 01 - 01));

    // Standard 5-check plausibility (PreisblattNetznutzung Gas).
    let report = {
        let sheet = state
            .preisblatt_client
            .get_preisblatt(&sender_mp_id, billing_date)
            .await
            .ok()
            .flatten();
        let preisblatt_store = {
            use invoic_checker::tariff::InMemoryPreisblattStore;
            let mut store = InMemoryPreisblattStore::new();
            if let Some(s) = sheet {
                store.insert(sender_mp_id.clone(), s);
            }
            store
        };
        InvoicCheckEngine::check(
            pid,
            &sender_mp_id,
            &rechnung,
            &preisblatt_store,
            &state.check_config,
        )
    };

    // ── Check 6 (MMM Gas settlement prices for GaBi Gas PIDs 31007/31008) ────
    let report = if GABI_GAS_MMM_PIDS.contains(&pid) {
        let billing_date = rechnung
            .billing_period()
            .map(|(s, _)| s)
            .or(rechnung.rechnungsdatum)
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
        let (y, m) = (billing_date.year(), billing_date.month() as u8);

        // Gas MMM prices: Trading Hub Europe (THE) is the single Gas MGV.
        let mmm_prices = state
            .preisblatt_client
            .get_mmma_gas(y, m, "THE")
            .await
            .ok()
            .flatten()
            .map(|r| (r.mehr_ct_kwh, r.minder_ct_kwh));

        if let Some((mehr_ct, minder_ct)) = mmm_prices {
            let mmm_findings = InvoicCheckEngine::check_mmm_settlement(
                &rechnung,
                mehr_ct,
                minder_ct,
                &state.check_config,
            );
            if !mmm_findings.is_empty() {
                use invoic_checker::CheckOutcome;
                let extra_outcome = mmm_findings
                    .iter()
                    .map(|f| {
                        if f.is_dispute {
                            CheckOutcome::Dispute
                        } else {
                            CheckOutcome::Warn
                        }
                    })
                    .max()
                    .unwrap_or(CheckOutcome::Ok);
                let merged_outcome = report.outcome.max(extra_outcome);
                let mut merged = report;
                merged.findings.extend(mmm_findings);
                merged.outcome = merged_outcome;
                merged
            } else {
                report
            }
        } else {
            tracing::debug!(
                pid,
                year = y,
                month = m,
                "invoicd: Gas MMMA prices not found in marktd — MMM Gas check skipped"
            );
            report
        }
    } else {
        report
    };

    let checked_at = OffsetDateTime::now_utc();

    info!(
        process_id = %process_id, pid,
        outcome = ?report.outcome, findings = report.findings.len(),
        "invoicd: Gas INVOIC check complete"
    );

    let should_dispute = match report.outcome {
        invoic_checker::CheckOutcome::Ok => false,
        invoic_checker::CheckOutcome::Warn => {
            state.auto_dispute_threshold_eur_cents > 0
                && report
                    .total_net_invoic
                    .map(|t| t.to_raw() > state.auto_dispute_threshold_eur_cents)
                    .unwrap_or(false)
        }
        invoic_checker::CheckOutcome::Dispute => true,
    };

    let outcome_str = match (report.outcome, should_dispute) {
        (invoic_checker::CheckOutcome::Ok, _) => "Ok",
        (invoic_checker::CheckOutcome::Warn, false) => "Warn",
        _ => "Dispute",
    };

    // §22 MessZV: persist receipt BEFORE dispatching.
    if let Some(pool) = &state.pool {
        let findings_json =
            serde_json::to_value(&report.findings).unwrap_or(serde_json::Value::Array(vec![]));
        let pay_by: Option<time::OffsetDateTime> = rechnung
            .faelligkeitsdatum
            .map(|d| d.with_time(time::Time::MIDNIGHT).assume_utc());
        let row = pg::ReceiptRow {
            process_id,
            pid: pid as i16,
            direction: "Inbound".to_owned(),
            sender_mp_id: sender_mp_id.clone(),
            receiver_gln,
            malo_id: malo_id.clone(),
            rechnung: rechnung_value,
            bo4e_version: "v202607.0.0".to_owned(),
            outcome: outcome_str.to_owned(),
            findings: findings_json,
            pay_by,
            received_at,
            checked_at,
            dispatched_at: None,
            tenant: state.tenant.clone(),
        };
        if let Err(err) = pg::upsert_receipt(pool, &row).await {
            warn!(%err, process_id = %process_id, pid,
                "invoicd: Gas invoice failed to persist receipt — §22 MessZV gap; continuing");
        }
    }

    // Dispatch command — routing depends on PID domain.
    let (accept_cmd, reject_cmd) = gas_billing_commands(pid);
    if should_dispute {
        let reason = dispute_reason(&report.findings);
        warn!(process_id = %process_id, pid, reason = %reason, "invoicd: Gas invoice disputed");
        let idem = Uuid::new_v5(&process_id, b"gas-dispute").to_string();
        let cmd = mako_markt::makod_client::ForwardCommand {
            marktrolle: None,
            command: reject_cmd.to_owned(),
            malo_id: None,
            melo_id: None,
            payload: serde_json::json!({ "invoice_ref": invoice_ref, "ablehnungsgrund": reason }),
        };
        match state.makod.post_command(&idem, &cmd).await {
            Ok(_) => {
                if let Some(pool) = &state.pool {
                    let _ =
                        pg::receipts::mark_dispatched(pool, process_id, OffsetDateTime::now_utc())
                            .await;
                }
            }
            Err(e) => {
                warn!(%e, process_id = %process_id, pid, "invoicd: Gas dispute dispatch failed")
            }
        }
    } else {
        info!(process_id = %process_id, pid, "invoicd: Gas invoice approved");
        let idem = Uuid::new_v5(&process_id, b"gas-accept").to_string();
        let cmd = mako_markt::makod_client::ForwardCommand {
            marktrolle: None,
            command: accept_cmd.to_owned(),
            malo_id: None,
            melo_id: None,
            payload: serde_json::json!({ "invoice_ref": invoice_ref }),
        };
        match state.makod.post_command(&idem, &cmd).await {
            Ok(_) => {
                if let Some(pool) = &state.pool {
                    let _ =
                        pg::receipts::mark_dispatched(pool, process_id, OffsetDateTime::now_utc())
                            .await;
                }
            }
            Err(e) => {
                warn!(%e, process_id = %process_id, pid, "invoicd: Gas accept dispatch failed")
            }
        }
        emit_payment_event(
            &state,
            PaymentEventCtx {
                process_id,
                pid,
                direction: "Inbound",
                sender_mp_id: &sender_mp_id,
                outcome: outcome_str,
                pay_by: rechnung.faelligkeitsdatum,
                findings_count: report.findings.len(),
            },
        )
        .await;
    }
}

/// Map a Gas billing PID to its (accept, reject) command names.
fn gas_billing_commands(pid: u32) -> (&'static str, &'static str) {
    match pid {
        31003 => ("wim.gas.rechnung.annehmen", "wim.gas.rechnung.ablehnen"),
        31007 | 31008 => (
            "gabi.gas.mmm.rechnung.annehmen",
            "gabi.gas.mmm.rechnung.ablehnen",
        ),
        31011 => ("geli.gas.rechnung.annehmen", "geli.gas.rechnung.ablehnen"),
        _ => ("invoic.annehmen", "invoic.ablehnen"), // safe fallback
    }
}

/// Handle PID 31004 — WiM Gas Stornorechnung (cancellation invoice).
///
/// A Stornorechnung cancels a previously issued PID 31003 invoice.  It carries a
/// negative `gesamtbrutto`.  Arithmetic and period checks still run; tariff checks
/// are skipped (cancellations don't carry tariff positions).  The outcome is always
/// `AcceptedPartial` unless arithmetic fails.
async fn handle_gas_stornorechnung(state: HandlerState, subject: String, data: serde_json::Value) {
    let process_id = match subject.parse::<Uuid>() {
        Ok(id) => id,
        Err(_) => {
            warn!(
                subject,
                "invoicd: WiM Gas 31004 event has no parseable UUID subject"
            );
            return;
        }
    };

    let sender_mp_id = data["sender_mp_id"].as_str().unwrap_or("").to_owned();
    let invoice_ref = data["invoice_ref"].as_str().unwrap_or("").to_owned();
    let receiver_gln = data["receiver_gln"]
        .as_str()
        .unwrap_or(&state.tenant)
        .to_owned();
    let rechnung_value = data["rechnung"].clone();

    let rechnung: Rechnung = match serde_json::from_value(rechnung_value.clone()) {
        Ok(r) => r,
        Err(err) => {
            warn!(%err, "invoicd: WiM Gas 31004 Stornorechnung deserialization failed — writing DLQ");
            write_gas_dlq(&state, &data, process_id, 31004).await;
            return;
        }
    };

    let received_at = OffsetDateTime::now_utc();
    let malo_id: Option<String> = rechnung
        .marktlokation
        .as_ref()
        .and_then(|ml| ml.marktlokations_id.as_ref())
        .map(|id| id.to_string());

    // Stornorechnung: run arithmetic + period checks only (no tariff check).
    let storno_config = invoic_checker::CheckConfig {
        require_tariff: false,
        ..(*state.check_config).clone()
    };
    let report = {
        let empty_store = invoic_checker::tariff::InMemoryPreisblattStore::new();
        InvoicCheckEngine::check(
            31004,
            &sender_mp_id,
            &rechnung,
            &empty_store,
            &storno_config,
        )
    };

    let checked_at = OffsetDateTime::now_utc();
    let should_dispute = matches!(report.outcome, invoic_checker::CheckOutcome::Dispute);
    let outcome_str = if should_dispute {
        "Dispute"
    } else {
        "AcceptedPartial"
    };

    info!(process_id = %process_id, pid = 31004, outcome = outcome_str,
        "invoicd: WiM Gas Stornorechnung check complete");

    if let Some(pool) = &state.pool {
        let findings_json =
            serde_json::to_value(&report.findings).unwrap_or(serde_json::Value::Array(vec![]));
        let pay_by: Option<time::OffsetDateTime> = rechnung
            .faelligkeitsdatum
            .map(|d| d.with_time(time::Time::MIDNIGHT).assume_utc());
        let row = pg::ReceiptRow {
            process_id,
            pid: 31004_i16,
            direction: "Inbound".to_owned(),
            sender_mp_id: sender_mp_id.clone(),
            receiver_gln,
            malo_id,
            rechnung: rechnung_value,
            bo4e_version: "v202607.0.0".to_owned(),
            outcome: outcome_str.to_owned(),
            findings: findings_json,
            pay_by,
            received_at,
            checked_at,
            dispatched_at: None,
            tenant: state.tenant.clone(),
        };
        if let Err(err) = pg::upsert_receipt(pool, &row).await {
            warn!(%err, process_id = %process_id, "invoicd: Gas 31004 persist failed — §22 MessZV gap");
        }
    }

    let cmd_name = if should_dispute {
        "wim.gas.stornorechnung.ablehnen"
    } else {
        "wim.gas.stornorechnung.annehmen"
    };
    let idem = Uuid::new_v5(&process_id, b"gas31004").to_string();
    let payload = if should_dispute {
        serde_json::json!({ "invoice_ref": invoice_ref, "ablehnungsgrund": dispute_reason(&report.findings) })
    } else {
        serde_json::json!({ "invoice_ref": invoice_ref })
    };
    let cmd = mako_markt::makod_client::ForwardCommand {
        marktrolle: None,
        command: cmd_name.to_owned(),
        malo_id: None,
        melo_id: None,
        payload,
    };
    match state.makod.post_command(&idem, &cmd).await {
        Ok(_) => {
            if let Some(pool) = &state.pool {
                let _ = pg::receipts::mark_dispatched(pool, process_id, OffsetDateTime::now_utc())
                    .await;
            }
        }
        Err(e) => warn!(%e, process_id = %process_id, "invoicd: Gas 31004 dispatch failed"),
    }
}

/// Write a DLQ entry for a Gas invoice that could not be processed.
async fn write_gas_dlq(state: &HandlerState, data: &serde_json::Value, process_id: Uuid, pid: u32) {
    if let Some(pool) = &state.pool {
        let reason = format!(
            "PID {pid}: rechnung unavailable or unparseable — manual reconciliation required"
        );
        let _ = sqlx::query(
            r"INSERT INTO invoic_dlq (malo_id, raw_event, failure_reason, failed_at, tenant)
              VALUES (NULL, $1, $2, now(), $3)
              ON CONFLICT DO NOTHING",
        )
        .bind(data)
        .bind(&reason)
        .bind(&state.tenant)
        .execute(pool)
        .await
        .inspect_err(|e| {
            warn!(%e, process_id = %process_id, pid, "invoicd: failed to write Gas DLQ entry");
        });
    }
}

/// Write a DLQ entry for a 31009 event that could not be processed.
async fn write_31009_dlq(state: &HandlerState, data: &serde_json::Value, process_id: Uuid) {
    if let Some(pool) = &state.pool {
        let _ = sqlx::query(
            r"INSERT INTO invoic_dlq (malo_id, raw_event, failure_reason, failed_at)
              VALUES (NULL, $1, $2, now())
              ON CONFLICT DO NOTHING",
        )
        .bind(data)
        .bind("WiM 31009: rechnung unavailable — manual reconciliation required")
        .execute(pool)
        .await
        .inspect_err(|e| {
            warn!(%e, process_id = %process_id, "invoicd: failed to write 31009 DLQ entry");
        });
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Context for [`emit_payment_event`].
pub struct PaymentEventCtx<'a> {
    process_id: uuid::Uuid,
    pid: u32,
    direction: &'a str,
    sender_mp_id: &'a str,
    outcome: &'a str,
    pay_by: Option<time::Date>,
    findings_count: usize,
}

/// Emit a payment CloudEvent to the configured ERP webhook.
///
/// CloudEvents types:
/// - `de.invoic.receipt.settled`   — outcome `Ok` / `AcceptedPartial` / `Warn`
/// - `de.invoic.receipt.disputed`  — outcome `Dispute`
/// - `de.invoic.receipt.dispatched`— outbound 31006 sent (awaiting NB REMADV)
///
/// ## Delivery guarantee
///
/// Delivery is **durable at-least-once**:
/// - This function makes the initial attempt (inline, within the handler task).
/// - On success (HTTP 2xx): marks `erp_notified_at` in `invoic_receipts`.
/// - On 5xx / transport error: increments `erp_attempts` and schedules a retry
///   via `erp_next_attempt_at`; the background outbox worker picks it up.
/// - On 4xx: permanent failure — log with response body; mark as dead-lettered
///   (ERP rejected the request; retrying would not help).
/// - REMADV dispatch is always completed BEFORE this function is called; ERP
///   notification never blocks the regulatory obligation.
///
/// ## Request signing
///
/// When `state.erp_hmac_secret` is configured, the POST body is signed with
/// HMAC-SHA256 and the signature is sent as `X-Mako-Signature: sha256=<hex>`.
pub async fn emit_payment_event(state: &HandlerState, ctx: PaymentEventCtx<'_>) {
    let Some(url) = &state.erp_webhook_url else {
        return;
    };

    let ce_type = match ctx.outcome {
        "Dispute" => "de.invoic.receipt.disputed",
        "Dispatched" => "de.invoic.receipt.dispatched",
        _ => "de.invoic.receipt.settled",
    };

    let payload = serde_json::json!({
        "specversion": "1.0",
        "id":          uuid::Uuid::new_v4().to_string(),
        "source":      format!("urn:invoicd:tenant:{}", state.tenant),
        "type":        ce_type,
        "time":        time::OffsetDateTime::now_utc()
                            .format(&time::format_description::well_known::Rfc3339)
                            .unwrap_or_default(),
        "subject":     ctx.process_id.to_string(),
        "datacontenttype": "application/json",
        "data": {
            "process_id":     ctx.process_id.to_string(),
            "pid":            ctx.pid,
            "direction":      ctx.direction,
            "sender_mp_id":   ctx.sender_mp_id,
            "outcome":        ctx.outcome,
            "pay_by":         ctx.pay_by.map(|d| d.to_string()),
            "findings_count": ctx.findings_count,
        },
    });

    let body = match serde_json::to_vec(&payload) {
        Ok(b) => b,
        Err(e) => {
            warn!(%e, process_id = %ctx.process_id, "invoicd: failed to serialize ERP payment event");
            return;
        }
    };

    let process_id = ctx.process_id;
    let pid = ctx.pid;

    // Sign the request body if an HMAC secret is configured.
    let signature = state.erp_hmac_secret.as_ref().map(|s| {
        format!(
            "sha256={}",
            mako_service::webhook::hmac_hex(s.expose_secret().as_bytes(), &body)
        )
    });

    let mut req = state
        .http_client
        .post(url)
        .header("Content-Type", "application/cloudevents+json");
    if let Some(sig) = signature {
        req = req.header("X-Mako-Signature", sig);
    }

    match req.body(body).send().await {
        Err(e) => {
            // Transport-level failure (DNS, connection refused, timeout, etc.)
            warn!(
                %process_id, pid, ce_type, erp_url = %url, error = %e,
                "invoicd: ERP payment webhook transport error — background worker will retry"
            );
            if let Some(pool) = &state.pool {
                let _ = crate::pg::receipts::record_erp_failure(pool, process_id, 0).await;
            }
        }
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                debug!(%process_id, pid, ce_type, %status, "invoicd: ERP payment event delivered");
                if let Some(pool) = &state.pool {
                    let _ = crate::pg::receipts::mark_erp_notified(
                        pool,
                        process_id,
                        time::OffsetDateTime::now_utc(),
                    )
                    .await;
                }
            } else if status.is_client_error() {
                // 4xx: permanent failure — retrying will not help.
                let body_preview = resp
                    .text()
                    .await
                    .unwrap_or_default()
                    .chars()
                    .take(256)
                    .collect::<String>();
                warn!(
                    %process_id, pid, ce_type, %status, erp_url = %url,
                    response_body = %body_preview,
                    "invoicd: ERP payment webhook rejected (4xx) — dead-lettered; check ERP webhook config"
                );
                // Record as permanently failed (erp_attempts >= 5 prevents background retry)
                if let Some(pool) = &state.pool {
                    for _ in 0..5i16 {
                        let _ = crate::pg::receipts::record_erp_failure(pool, process_id, 5).await;
                    }
                }
            } else {
                // 5xx: transient server error — background worker will retry.
                warn!(
                    %process_id, pid, ce_type, %status, erp_url = %url,
                    "invoicd: ERP payment webhook 5xx — background worker will retry"
                );
                if let Some(pool) = &state.pool {
                    let _ = crate::pg::receipts::record_erp_failure(pool, process_id, 0).await;
                }
            }
        }
    }
}

fn extract_pid(data: &serde_json::Value) -> u32 {
    data["pid"].as_u64().unwrap_or(0) as u32
}

/// Build a human-readable dispute reason from plausibility check findings.
///
/// Falls back to a generic message when no individual findings triggered a
/// `Dispute` outcome (e.g. the invoice was escalated solely by the
/// auto-dispute monetary threshold).
fn dispute_reason(findings: &[invoic_checker::Finding]) -> String {
    let specific: Vec<&str> = findings
        .iter()
        .filter(|f| f.is_dispute)
        .map(|f| f.message.as_str())
        .collect();
    if specific.is_empty() {
        "Automatische Ablehnung: Rechnungsbetrag überschreitet Freigabegrenze".into()
    } else {
        specific.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use invoic_checker::{Finding, FindingKind};

    use super::*;

    /// INVOIC_PIDS must be exactly the four GPKE billing PIDs that embed a
    /// `Rechnung` BO4E in the `ProcessInitiated` outbox payload.
    #[test]
    fn invoic_pids_are_gpke_only() {
        assert_eq!(INVOIC_PIDS, &[31001u32, 31002, 31005, 31006]);
        // PIDs belonging to other workflows must NOT be in the Strom list.
        for forbidden in [31003u32, 31004, 31007, 31008, 31009, 31011] {
            assert!(
                !INVOIC_PIDS.contains(&forbidden),
                "PID {forbidden} must not be in INVOIC_PIDS (wrong workflow)"
            );
        }
    }

    /// Gas billing PIDs are routed separately from Strom billing PIDs.
    #[test]
    fn gas_invoic_pids_are_separate() {
        assert_eq!(GAS_INVOIC_PIDS, &[31003u32, 31007, 31008, 31011]);
        // No overlap with GPKE Strom PIDs.
        for pid in GAS_INVOIC_PIDS {
            assert!(
                !INVOIC_PIDS.contains(pid),
                "PID {pid} must not appear in both INVOIC_PIDS and GAS_INVOIC_PIDS"
            );
        }
        // PID 31004 (Stornorechnung) is handled separately — not in either list.
        assert!(!GAS_INVOIC_PIDS.contains(&31004u32));
        // PID 31009 (WiM MSB-Rechnung) uses PreisblattMessung — not Gas.
        assert!(!GAS_INVOIC_PIDS.contains(&31009u32));
    }

    /// MMM PID sets must be non-overlapping and correct.
    #[test]
    fn mmm_pid_sets_are_disjoint() {
        for strom_pid in STROM_MMM_PIDS {
            assert!(
                !GABI_GAS_MMM_PIDS.contains(strom_pid),
                "PID {strom_pid} must not be in both STROM_MMM_PIDS and GABI_GAS_MMM_PIDS"
            );
        }
        // Strom MMM: 31002 (MMM-Rechnung), 31005 (MMM selbst ausgest.)
        assert!(STROM_MMM_PIDS.contains(&31002u32));
        assert!(STROM_MMM_PIDS.contains(&31005u32));
        // Gas MMM: 31007 (GaBi Gas MMM), 31008 (selbst ausgest.)
        assert!(GABI_GAS_MMM_PIDS.contains(&31007u32));
        assert!(GABI_GAS_MMM_PIDS.contains(&31008u32));
    }

    /// Gas billing command routing must cover all Gas PIDs.
    #[test]
    fn gas_billing_commands_cover_all_gas_pids() {
        for &pid in GAS_INVOIC_PIDS {
            let (accept, reject) = gas_billing_commands(pid);
            assert!(!accept.is_empty(), "PID {pid} has no accept command");
            assert!(!reject.is_empty(), "PID {pid} has no reject command");
            // Commands must use the correct domain prefix.
            let domain = match pid {
                31003 => "wim.gas",
                31007 | 31008 => "gabi.gas",
                31011 => "geli.gas",
                _ => unreachable!(),
            };
            assert!(
                accept.starts_with(domain),
                "PID {pid} accept cmd must start with '{domain}'"
            );
            assert!(
                reject.starts_with(domain),
                "PID {pid} reject cmd must start with '{domain}'"
            );
        }
    }

    #[test]
    fn extract_pid_happy() {
        let data = serde_json::json!({ "pid": 31001 });
        assert_eq!(extract_pid(&data), 31001u32);
    }

    #[test]
    fn extract_pid_missing_returns_zero() {
        let data = serde_json::json!({ "other": 99 });
        assert_eq!(extract_pid(&data), 0u32);
    }

    #[test]
    fn dispute_reason_empty_findings_returns_fallback() {
        let reason = dispute_reason(&[]);
        assert!(!reason.is_empty(), "fallback message must not be empty");
        assert!(
            reason.contains("Automatische Ablehnung"),
            "fallback must mention automatic rejection"
        );
    }

    #[test]
    fn dispute_reason_with_findings_joins_them() {
        let findings = vec![
            Finding {
                kind: FindingKind::TariffDeviation,
                is_dispute: true,
                message: "Einzelpreis weicht ab".into(),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            },
            Finding {
                kind: FindingKind::PeriodInvalid,
                is_dispute: true,
                message: "Abrechnungszeitraum falsch".into(),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            },
        ];
        let reason = dispute_reason(&findings);
        assert!(reason.contains("Einzelpreis weicht ab"));
        assert!(reason.contains("Abrechnungszeitraum falsch"));
    }
}
