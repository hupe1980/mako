//! Axum webhook handler for inbound `MarktEvent` CloudEvents from `marktd`.
//!
//! ## Event routing
//!
//! | `ce_type`                    | `makopid` | Action |
//! |------------------------------|-----------|--------|
//! | `de.mako.process.completed`  | MSCONS set | Store `MeterDataReceipt` |
//! | `de.mako.process.initiated`  | 23001 (INSRPT Störungsmeldung) | M2: auto-create `INSRPT_STOERUNG` reading order |
//! | *(anything else)*            | *(any)*   | 204 No Content (ignored) |
//!
//! ## M2 — INSRPT → reading-order automation
//!
//! When an INSRPT Störungsmeldung (PID 23001, LF → MSB) arrives, §18 MessZV
//! mandates a Sonderablesung.  `edmd` auto-creates an `ablese_auftraege` row
//! with `anlass = 'INSRPT_STOERUNG'` so field-service scheduling is never
//! blocked on manual ERP input.
//!
//! The reading order is idempotent on `insrpt_process_id` — re-delivery of the
//! same event produces a no-op.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_edm::{
    domain::{GAS_QUALITY_PIDS, MSCONS_PIDS, MeterDataReceipt},
    repository::TimeSeriesRepository,
};
use mako_markt::cloudevents::verify_signature;
use secrecy::{ExposeSecret, SecretString};
use tracing::{debug, info, warn};
use uuid::Uuid;

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

    // ── M2: INSRPT Störungsmeldung → auto-create INSRPT_STOERUNG reading order ─
    //
    // §18 MessZV mandates a Sonderablesung when an INSRPT (PID 23001) is received.
    // Auto-creating the reading order here ensures the field-service scheduler
    // never blocks on a manual ERP trigger — eliminating billing gaps after
    // device swaps.
    //
    // Idempotent: ON CONFLICT (insrpt_process_id) DO NOTHING.
    if ce_type == "de.mako.process.initiated" && pid == 23001 {
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
                "edmd M2: INSRPT 23001 missing malo_id or process_id — skipping"
            );
            return StatusCode::NO_CONTENT.into_response();
        }

        // §18 MessZV / WiM AHB BK6-24-174: Sonderablesung within 5 Werktage.
        // `geplant_am` = next working day; `ausfuehrt_bis` = +7 calendar days
        // (covers 5 Werktage reliably including Saturdays = Werktag).
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
               VALUES ($1, $2, $3, 'INSRPT_STOERUNG', 'MSB', $4, $5, $6, $7)
               ON CONFLICT DO NOTHING"#,
        )
        .bind(&malo_id)
        .bind(&melo_id)
        .bind(&state.tenant)
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
                    geplant_am = %geplant_am,
                    "edmd M2: auto-created INSRPT_STOERUNG reading order (§18 MessZV)"
                );
            }
            Ok(_) => {
                debug!(
                    malo_id = %malo_id,
                    process_id = %process_id_str,
                    "edmd M2: INSRPT_STOERUNG reading order already exists — idempotent"
                );
            }
            Err(e) => {
                warn!(error = %e, malo_id = %malo_id, "edmd M2: failed to create INSRPT_STOERUNG reading order");
            }
        }
        return StatusCode::NO_CONTENT.into_response();
    }

    // 3. Route: only process.completed events for known MSCONS PIDs.
    if ce_type == "de.mako.process.completed" && MSCONS_PIDS.contains(&pid) {
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
            tenant_id: None,
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
                warn!(%err, process_id = %process_id, "edmd: failed to store receipt");
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
                    .update_gas_quality(&malo_id, brennwert.as_deref(), zustandszahl.as_deref())
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
    } else {
        debug!(ce_type, pid, "edmd: event ignored");
    }

    StatusCode::NO_CONTENT.into_response()
}
