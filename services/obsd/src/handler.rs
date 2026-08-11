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
    let inbound_secret = (*state.inbound_secret)
        .as_ref()
        .map(|s| s.expose_secret().as_bytes().to_vec());
    if let Err(code) =
        mako_service::webhook::verify_request(inbound_secret.as_deref(), &headers, &body)
    {
        warn!("obsd: webhook signature mismatch");
        return code.into_response();
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

    // 3. Only project de.mako.* events — via the canonical catalog matcher so a
    //    `de.mako.*` namespace change stays single-sourced in `mako-events`.
    if !mako_events::matches("de.mako.*", &ce_type) {
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

    // Look up existing projection to preserve started_at. A read failure is not
    // "no row": treating it as one re-anchors the deadline on this event.
    let (started_at, existing_deadline) = match state.repo.get(process_id).await {
        Ok(Some(existing)) => (existing.started_at, existing.deadline_at),
        Ok(None) => (event_time, None),
        Err(err) => {
            warn!(%err, process_id = %process_id, "obsd: projection read failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
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

    // A swallowed failure would let marktd's fan-out mark the event delivered:
    // a lost `process.initiated` leaves no projection and no deadline, and the
    // process can then breach in silence. Fail loudly so the fan-out retries.
    if let Err(err) = state.repo.upsert(&projection).await {
        warn!(%err, process_id = %process_id, "obsd: failed to upsert projection");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    info!(
        process_id = %process_id,
        pid,
        ce_type,
        state = ?state_val,
        "obsd: upserted process projection"
    );

    StatusCode::NO_CONTENT.into_response()
}

/// Derive the process family from workflow name or PID range.
///
/// Source: BDEW PID table 3.3/4.0, BK6-24-174, BK7-24-01-009, BK7-24-01-008.
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
        31003 => "wim-gas",         // INVOIC WiM Gas Rechnung
        31004 => "invoic-storno", // INVOIC Stornorechnung — Sparte-neutral, cross-process (AHB §3.1.2)
        // ── GaBi Gas — Bilanzierung Gas (BK7-24-01-008) ──────────────────────────
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
/// | WiM Strom | **per PID** — 3 / 5 / 7 / 1 Werktage | BK6-24-174 Teil 1 Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2 |
/// | GeLi Gas | 10 Werktage | BK7-24-01-009 §5 |
/// | WiM Gas  | 10 Werktage | BK7-24-01-009 §5 |
/// | MABIS  | 1 Werktag | BK6-24-174 §13.8 |
///
/// Werktage windows are computed **exactly**, with the same BdewMaKo calendar
/// `processd`/`mako-engine` use, so an obsd alert and the engine's own deadline
/// agree on the instant.
///
/// This used to be a calendar-day approximation described as "always
/// conservative". It was not. WiM Strom was flattened to one 7-day window when
/// its four PIDs run 3 / 5 / 7 / 1 Werktage, so the Abmeldung (55051, 7 WT)
/// raised breaches while the counterparty still had days in hand. No fixed
/// day-count could have been correct anyway: `deadline_at_werktage` resolves to
/// a 17:00 local cutoff and public holidays shift it further, so any midnight
/// day-count bound can land before the true deadline.
pub fn compute_deadline(
    pid: u32,
    started_at: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    use mako_engine::fristen::{HolidayCalendar, deadline_at_werktage};
    use time::Duration;

    // Werktage windows per family. GPKE is the exception: 24 wall-clock hours,
    // not Werktage (BK6-22-024 §5).
    let werktage = match pid {
        55001..=55018 | 55022..=55024 | 55555 | 55607..=55609 => {
            return Some(started_at + Duration::hours(24));
        }
        // WiM Strom — per PID; mako-wim is the single source of truth.
        55039 | 55042 | 55051 | 55168 => mako_wim::antwort_frist_werktage(pid)
            .expect("the match arm restricts this to the MSB-Wechsel family"),
        // GeLi Gas / WiM Gas — 10 Werktage (BK7-24-01-009 §5)
        44001..=44024 | 44039..=44053 | 44168..=44170 => 10,
        // MABIS Prüfmitteilung — 1 Werktag (BK6-24-174 §13.8)
        13003 => 1,
        // All other PIDs: billing, PARTIN, INSRPT — no per-process deadline
        _ => return None,
    };
    Some(deadline_at_werktage(
        started_at,
        werktage,
        HolidayCalendar::BdewMaKo,
    ))
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

    /// WiM Strom is per PID — a flat window raised false breaches on the
    /// Abmeldung (7 WT) and hid real ones on the Verpflichtungsanfrage (1 WT).
    #[test]
    fn compute_deadline_wim_strom_is_per_pid() {
        let started = datetime!(2026-07-14 08:00 UTC);
        let exact = |wt| {
            mako_engine::fristen::deadline_at_werktage(
                started,
                wt,
                mako_engine::fristen::HolidayCalendar::BdewMaKo,
            )
        };
        assert_eq!(compute_deadline(55_039, started).unwrap(), exact(3));
        assert_eq!(compute_deadline(55_042, started).unwrap(), exact(5));
        assert_eq!(compute_deadline(55_051, started).unwrap(), exact(7));
        assert_eq!(compute_deadline(55_168, started).unwrap(), exact(1));

        // The four must not collapse onto one instant.
        let all =
            [55_039_u32, 55_042, 55_051, 55_168].map(|p| compute_deadline(p, started).unwrap());
        assert_eq!(
            all.iter().collect::<std::collections::BTreeSet<_>>().len(),
            4,
            "each WiM Strom PID carries its own Frist"
        );
    }

    /// obsd and the engine must agree on the instant, or an alert contradicts
    /// the deadline the process actually carries.
    #[test]
    fn obsd_agrees_with_the_engine_on_every_werktage_window() {
        for (pid, wt) in [
            (55_039_u32, 3_u32),
            (55_042, 5),
            (55_051, 7),
            (55_168, 1),
            (44_001, 10),
            (44_039, 10),
            (13_003, 1),
        ] {
            let mut day = datetime!(2026-01-01 09:00 UTC);
            for _ in 0..365 {
                let exact = mako_engine::fristen::deadline_at_werktage(
                    day,
                    wt,
                    mako_engine::fristen::HolidayCalendar::BdewMaKo,
                );
                assert_eq!(
                    compute_deadline(pid, day).unwrap(),
                    exact,
                    "PID {pid} at {day} disagrees with the engine"
                );
                day += time::Duration::days(1);
            }
        }
    }

    #[test]
    fn compute_deadline_geli_gas_10_werktage() {
        let started = datetime!(2026-07-01 00:00 UTC);
        let d = compute_deadline(44001, started).unwrap();
        // 10 WT, resolved on the BdewMaKo calendar to the 17:00 CEST cutoff.
        assert_eq!(d, datetime!(2026-07-15 17:00 +2));
    }

    #[test]
    fn compute_deadline_wim_gas_10_werktage() {
        let started = datetime!(2026-07-01 00:00 UTC);
        let d = compute_deadline(44039, started).unwrap();
        assert_eq!(d, datetime!(2026-07-15 17:00 +2));
    }

    #[test]
    fn compute_deadline_mabis_1_werktag() {
        let started = datetime!(2026-07-14 08:00 UTC); // a Tuesday
        let d = compute_deadline(13003, started).unwrap();
        assert_eq!(d, datetime!(2026-07-15 17:00 +2));
    }

    #[test]
    fn compute_deadline_billing_pid_returns_none() {
        // INVOIC, PARTIN, INSRPT — no per-process response deadline
        assert!(compute_deadline(31001, datetime!(2026-07-14 00:00 UTC)).is_none());
        assert!(compute_deadline(37000, datetime!(2026-07-14 00:00 UTC)).is_none());
        assert!(compute_deadline(23001, datetime!(2026-07-14 00:00 UTC)).is_none());
    }
}
