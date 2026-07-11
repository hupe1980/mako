//! Axum webhook handler for inbound `MarktEvent` CloudEvents from `marktd`.
//!
//! ## Event routing
//!
//! | `ce_type`                    | `makopid` | Action |
//! |------------------------------|-----------|--------|
//! | `de.mako.process.completed`  | MSCONS set | Store `MeterDataReceipt` |
//! | *(anything else)*            | *(any)*   | 204 No Content (ignored) |

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

use crate::pg::PgTimeSeriesRepository;

/// Shared application state for the webhook handler.
#[derive(Clone)]
pub struct HandlerState {
    pub repo: PgTimeSeriesRepository,
    pub inbound_secret: Arc<Option<SecretString>>,
    /// Tenant identifier — used as Cedar resource_tenant for REST queries.
    pub tenant: String,
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
