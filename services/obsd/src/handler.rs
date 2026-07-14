//! Axum webhook handler for inbound `MarktEvent` CloudEvents — all event types.
//!
//! Projects every `de.mako.*` event into a [`ProcessProjection`] row.

use std::collections::HashSet;
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
    /// All operator MP-IDs for §20 EnWG `initiator_is_affiliate` detection.
    ///
    /// Membership test against `data.new_supplier` on Lieferbeginn events
    /// (PIDs 55001, 55016, 44001).  `Arc<HashSet>` for O(1) lookup without clone.
    pub own_mp_ids: Arc<HashSet<String>>,
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

    let deadline_at = match state_val {
        // Compute a fresh deadline when the process is first initiated.
        ProcessState::Initiated => compute_deadline(pid, event_time),
        // For all subsequent events preserve the existing deadline.
        // If the projection is brand-new (no prior row), fall back to a computed deadline
        // so events arriving out-of-order still get a usable deadline.
        _ => existing_deadline.or_else(|| compute_deadline(pid, started_at)),
    };
    let deadline_risk = deadline_at
        .map(|d| DeadlineRisk::classify(d, event_time))
        .unwrap_or(DeadlineRisk::Green);

    // §20 EnWG Diskriminierungsfreiheitspflicht: detect affiliate initiators.
    // For Lieferbeginn PIDs (55001, 55016, 44001) the event data carries
    // `new_supplier` (the initiating LF's MP-ID).  A match against any of
    // own_mp_ids means the LF is a subsidiary of the operating NB/GNB.
    // Covers both Strom (BDEW 99…) and Gas (DVGW 98…) in one check.
    let initiator_is_affiliate = matches!(pid, 55001 | 55016 | 44001)
        && !state.own_mp_ids.is_empty()
        && data["new_supplier"]
            .as_str()
            .is_some_and(|s| state.own_mp_ids.contains(s));

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
        initiator_is_affiliate,
        tenant: state.tenant.clone(),
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
///
/// Source: BDEW PID table 3.3/4.0, BK6-24-174, BK7-24-01-009, BK7-14-020.
fn derive_family(workflow_name: &str, pid: u32) -> String {
    if !workflow_name.is_empty() {
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
    match pid {
        // ── GPKE — Lieferwechsel Strom (BK6-22-024 / BK6-24-174) ──────────────
        55001..=55018 | 55022..=55024 | 55555 | 55607..=55609 => "gpke",
        17115..=17117 => "gpke",                 // ORDERS Sperrung Strom
        17134 | 17135 => "gpke",                 // ORDERS/ORDRSP Konfiguration Strom
        19001 | 19002 => "gpke", // ORDRSP Konfiguration / Geräteübernahme (multi-domain, gpke bucket)
        37000..=37006 => "gpke", // PARTIN Strom Kommunikationsdaten
        31001 | 31002 | 31005 | 31006 => "gpke", // INVOIC NNE/MMM/selbst ausgest. Strom
        // ── WiM — Messstellenbetrieb Strom (BK6-24-174) ───────────────────────
        55039 | 55042 | 55051 | 55168 => "wim",
        17001..=17011 => "wim", // ORDERS Geräteübernahme (nMSB)
        23001 | 23003 | 23004 | 23008 => "wim", // INSRPT Strom
        27001..=27003 => "wim", // PRICAT Preisliste
        31009 => "wim",         // INVOIC MSB-Rechnung
        35001..=35005 => "wim", // REQOTE/QUOTES Preisanfrage
        // ── GeLi Gas — Lieferbeginn/-ende Gas (BK7-24-01-009) ─────────────────
        44001..=44024 => "geli-gas", // UTILMD G incl. 44022-44024 role-conditional
        37008..=37014 => "geli-gas", // PARTIN Gas Kommunikationsdaten
        31011 => "geli-gas",         // INVOIC AWH Sperrprozesse Gas (GNB→LFG)
        // ── WiM Gas — Messstellenbetrieb Gas (BK7-24-01-009) ──────────────────
        44039..=44053 | 44168..=44170 => "wim-gas",
        23005 | 23009 => "wim-gas", // INSRPT Gas-only variants
        31003 | 31004 => "wim-gas", // INVOIC WiM Gas Rechnung / Stornorechnung
        // ── GaBi Gas — Bilanzierung Gas (BK7-14-020) ──────────────────────────
        31007 | 31008 | 31010 => "gabi-gas", // INVOIC MMM-Rechnung / Kapazitätsrechnung
        13013 => "gabi-gas",                 // MSCONS Allokationsliste Gas (MMMA)
        17110 | 19110 => "gabi-gas",         // ORDERS/ORDRSP Allokationsliste Gas
        // ── MABIS — Bilanzkreisabrechnung Strom (BK6-24-174) ──────────────────
        13003 => "mabis",
        _ => "unknown",
    }
    .into()
}

/// Compute the regulatory response deadline for a process based on its PID.
///
/// Returns `None` for PIDs without a defined per-process deadline (billing PIDs,
/// PARTIN, etc.).
///
/// ## Deadline sources
/// | Family | Deadline | Source |
/// |--------|----------|--------|
/// | GPKE   | 24 wall-clock hours | BK6-22-024 §5 |
/// | WiM    | 5 Werktage ≈ 7 calendar days | BK6-24-174 |
/// | GeLi Gas | 10 Werktage ≈ 14 calendar days | BK7-24-01-009 §5 |
/// | WiM Gas  | 10 Werktage ≈ 14 calendar days | BK7-24-01-009 §5 |
/// | MABIS  | 1 Werktag ≈ 2 calendar days | BK6-24-174 §13.8 |
///
/// Calendar-day approximations are **always conservative**: 7 calendar days ≥ 5
/// Werktage, so obsd never marks a process as overdue before its true deadline.
/// Exact Werktage arithmetic (accounting for Samstag + public holidays) is
/// performed by `processd`/`mako-engine`; `obsd` uses the coarser approximation
/// for alerting purposes only.
pub fn compute_deadline(
    pid: u32,
    started_at: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    use time::Duration;
    let d = match pid {
        // GPKE — 24 wall-clock hours exact
        55001..=55018 | 55022..=55024 | 55555 | 55607..=55609 => Duration::hours(24),
        // WiM — 5 Werktage (7 calendar days conservative)
        55039 | 55042 | 55051 | 55168 => Duration::days(7),
        // GeLi Gas — 10 Werktage (14 calendar days conservative)
        44001..=44024 => Duration::days(14),
        // WiM Gas — 10 Werktage (14 calendar days conservative)
        44039..=44053 | 44168..=44170 => Duration::days(14),
        // MABIS Prüfmitteilung — 1 Werktag (2 calendar days conservative)
        13003 => Duration::days(2),
        // All other PIDs: billing, PARTIN, INSRPT — no per-process deadline
        _ => return None,
    };
    Some(started_at + d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    // ── derive_family ─────────────────────────────────────────────────────────

    #[test]
    fn derive_family_gpke_by_workflow() {
        assert_eq!(derive_family("gpke-lf-anmeldung", 55001), "gpke");
        assert_eq!(derive_family("gpke-nb-lieferende", 55008), "gpke");
    }

    #[test]
    fn derive_family_gpke_by_pid() {
        assert_eq!(derive_family("", 55001), "gpke");
        assert_eq!(derive_family("", 55016), "gpke");
        assert_eq!(derive_family("", 55555), "gpke");
        assert_eq!(derive_family("", 55607), "gpke");
    }

    #[test]
    fn derive_family_wim_by_pid() {
        assert_eq!(derive_family("", 55039), "wim");
        assert_eq!(derive_family("", 55042), "wim");
        assert_eq!(derive_family("", 55051), "wim");
        assert_eq!(derive_family("", 55168), "wim");
        assert_eq!(derive_family("", 31009), "wim"); // MSB-Rechnung
    }

    #[test]
    fn derive_family_geli_gas_by_pid() {
        assert_eq!(derive_family("", 44001), "geli-gas");
        assert_eq!(derive_family("", 44021), "geli-gas");
        assert_eq!(derive_family("", 37008), "geli-gas"); // PARTIN Gas
        assert_eq!(derive_family("", 31011), "geli-gas"); // AWH Sperrprozesse Gas
    }

    #[test]
    fn derive_family_wim_gas_by_pid() {
        assert_eq!(derive_family("", 44039), "wim-gas");
        assert_eq!(derive_family("", 44053), "wim-gas");
        assert_eq!(derive_family("", 44168), "wim-gas");
        assert_eq!(derive_family("", 23005), "wim-gas"); // INSRPT Gas-only
    }

    #[test]
    fn derive_family_gabi_gas_by_pid() {
        assert_eq!(derive_family("", 31007), "gabi-gas");
        assert_eq!(derive_family("", 31010), "gabi-gas"); // Kapazitätsrechnung
        assert_eq!(derive_family("", 13013), "gabi-gas"); // MSCONS MMMA
        assert_eq!(derive_family("", 17110), "gabi-gas"); // ORDERS Allokation
    }

    #[test]
    fn derive_family_mabis_by_pid() {
        assert_eq!(derive_family("", 13003), "mabis");
    }

    #[test]
    fn derive_family_unknown_pid() {
        assert_eq!(derive_family("", 99999), "unknown");
        assert_eq!(derive_family("", 0), "unknown");
    }

    #[test]
    fn derive_family_workflow_wins_over_pid() {
        // Even when PID says "geli-gas", workflow prefix takes priority
        assert_eq!(derive_family("gpke-supplier-change", 44001), "gpke");
    }

    // ── compute_deadline ──────────────────────────────────────────────────────

    #[test]
    fn compute_deadline_gpke_24h() {
        let started = datetime!(2026-07-14 10:00 UTC);
        let d = compute_deadline(55001, started).unwrap();
        assert_eq!(d, datetime!(2026-07-15 10:00 UTC));
    }

    #[test]
    fn compute_deadline_wim_7_days() {
        let started = datetime!(2026-07-14 00:00 UTC);
        let d = compute_deadline(55039, started).unwrap();
        assert_eq!(d, datetime!(2026-07-21 00:00 UTC));
    }

    #[test]
    fn compute_deadline_geli_gas_14_days() {
        let started = datetime!(2026-07-01 00:00 UTC);
        let d = compute_deadline(44001, started).unwrap();
        assert_eq!(d, datetime!(2026-07-15 00:00 UTC));
    }

    #[test]
    fn compute_deadline_wim_gas_14_days() {
        let started = datetime!(2026-07-01 00:00 UTC);
        let d = compute_deadline(44039, started).unwrap();
        assert_eq!(d, datetime!(2026-07-15 00:00 UTC));
    }

    #[test]
    fn compute_deadline_mabis_2_days() {
        let started = datetime!(2026-07-14 08:00 UTC);
        let d = compute_deadline(13003, started).unwrap();
        assert_eq!(d, datetime!(2026-07-16 08:00 UTC));
    }

    #[test]
    fn compute_deadline_billing_pid_returns_none() {
        // INVOIC, PARTIN, INSRPT — no per-process response deadline
        assert!(compute_deadline(31001, datetime!(2026-07-14 00:00 UTC)).is_none());
        assert!(compute_deadline(37000, datetime!(2026-07-14 00:00 UTC)).is_none());
        assert!(compute_deadline(23001, datetime!(2026-07-14 00:00 UTC)).is_none());
    }
}
