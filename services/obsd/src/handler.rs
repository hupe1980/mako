//! Axum webhook handler for inbound `MarktEvent` CloudEvents — all event types.
//!
//! Projects every `de.mako.*` event into a [`ProcessProjection`] row.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::cloudevents::verify_signature;
use mako_obs::{
    domain::{DeadlineRisk, ProcessProjection, ProcessState},
    repository::ProcessProjectionRepository,
};
use secrecy::{ExposeSecret, SecretString};
use time::OffsetDateTime;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::pg::PgProcessProjectionRepository;

/// Shared application state for the webhook handler.
#[derive(Clone)]
pub struct HandlerState {
    pub repo: PgProcessProjectionRepository,
    pub inbound_secret: Arc<Option<SecretString>>,
    /// Tenant identifier — used as Cedar resource_tenant for REST queries.
    pub tenant: String,
}

/// `POST /webhook` — receive any `MarktEvent` from `marktd`.
pub async fn handle_webhook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // 1. Verify signature.
    if let Some(secret) = (*state.inbound_secret).as_ref() {
        let provided = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !verify_signature(secret.expose_secret().as_bytes(), &body, provided) {
            warn!("obsd: webhook signature mismatch");
            return (StatusCode::UNAUTHORIZED, "signature mismatch").into_response();
        }
    }

    // 2. Parse JSON body.
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(err) => {
            warn!(%err, "obsd: failed to parse MarktEvent");
            return (StatusCode::BAD_REQUEST, "invalid JSON").into_response();
        }
    };

    let ce_type = event["type"].as_str().unwrap_or("").to_owned();

    // 3. Only project de.mako.* events.
    if !ce_type.starts_with("de.mako.") {
        debug!(ce_type, "obsd: non-mako event ignored");
        return StatusCode::NO_CONTENT.into_response();
    }

    let Some(state_val) = ProcessState::from_ce_type(&ce_type) else {
        debug!(ce_type, "obsd: unrecognised mako event type, skipping");
        return StatusCode::NO_CONTENT.into_response();
    };

    let subject = event["subject"].as_str().unwrap_or("").to_owned();
    let process_id: Uuid = match subject.parse() {
        Ok(id) => id,
        Err(_) => {
            debug!(subject, "obsd: subject is not a valid UUID, skipping");
            return StatusCode::NO_CONTENT.into_response();
        }
    };

    let event_time = event["time"]
        .as_str()
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);

    let data = &event["data"];

    // Extract fields — prefer forwarded CE extensions, fall back to data payload.
    let pid = event["makopid"]
        .as_u64()
        .or_else(|| data["pid"].as_u64())
        .unwrap_or(0) as u32;

    let workflow_name = event["makoworkflow"]
        .as_str()
        .or_else(|| data["workflow_name"].as_str())
        .unwrap_or("")
        .to_owned();

    let family = derive_family(&workflow_name, pid);

    let malo_id = data["malo_id"]
        .as_str()
        .or_else(|| data["location_id"].as_str())
        .map(str::to_owned);

    let partner_mp_id = data["partner_mp_id"]
        .as_str()
        .or_else(|| data["sender"].as_str())
        .or_else(|| data["sender_mp_id"].as_str())
        .map(str::to_owned);

    let mdm_role = event["marktrole"].as_str().map(str::to_owned);

    let erc_code = event["makoerc"]
        .as_str()
        .or_else(|| data["error_code"].as_str())
        .map(str::to_owned);

    // Look up existing projection to preserve started_at.
    let (started_at, existing_deadline) = match state.repo.get(process_id).await {
        Ok(Some(existing)) => (existing.started_at, existing.deadline_at),
        _ => (event_time, None),
    };

    let deadline_at = existing_deadline;
    let deadline_risk = deadline_at
        .map(|d| DeadlineRisk::classify(d, event_time))
        .unwrap_or(DeadlineRisk::Green);

    let projection = ProcessProjection {
        process_id,
        pid,
        family,
        workflow_name,
        state: state_val,
        malo_id,
        partner_mp_id,
        mdm_role,
        deadline_at,
        deadline_risk,
        started_at,
        last_event_at: event_time,
        erc_code,
        tenant_id: None,
    };

    match state.repo.upsert(&projection).await {
        Ok(()) => {
            info!(
                process_id = %process_id,
                pid,
                ce_type,
                state = ?state_val,
                "obsd: upserted process projection"
            );
        }
        Err(err) => {
            warn!(%err, process_id = %process_id, "obsd: failed to upsert projection");
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Derive the process family from workflow name or PID range.
fn derive_family(workflow_name: &str, pid: u32) -> String {
    if !workflow_name.is_empty() {
        // Extract prefix before first '-' hyphen-segment after known families.
        if workflow_name.starts_with("gpke") {
            return "gpke".into();
        }
        if workflow_name.starts_with("geli-gas") {
            return "geli-gas".into();
        }
        if workflow_name.starts_with("wim-gas") {
            return "wim-gas".into();
        }
        if workflow_name.starts_with("wim") {
            return "wim".into();
        }
        if workflow_name.starts_with("gabi-gas") {
            return "gabi-gas".into();
        }
        if workflow_name.starts_with("mabis") {
            return "mabis".into();
        }
    }
    // Fall back to PID range.
    // Note: ranges must not overlap — individual values covered by a range are omitted.
    match pid {
        44001..=44024 => "geli-gas".into(),
        37008..=37014 => "geli-gas".into(),
        44039..=44053 | 44168..=44170 => "wim-gas".into(),
        13003 => "mabis".into(),
        13013 | 17110 | 19110 => "gabi-gas".into(),
        _ => "unknown".into(),
    }
}
