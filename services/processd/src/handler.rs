//! Axum webhook handler for `processd`.
//!
//! Receives `de.mako.process.*` CloudEvents from `marktd` (HMAC-signed).
//!
//! ## Event routing
//!
//! ## Event routing, by the role that owes the answer
//!
//! | Trigger | Module | Cargo feature |
//! |---|---|---|
//! | `process.initiated` 55001, 55077, 44001 | NB Anmeldung STP | `role-nb-*` |
//! | `process.initiated` 55004, 44004 | NB Abmeldung STP (EBD `E_0607`) | `role-nb-*` |
//! | `process.initiated` 55042, 55051 | MSB-Wechsel the **NB** answers | `role-nb-*` |
//! | `versorgung.gap-detected` / `.eog-begonnen` / `.changed` | EoG gap closure | `role-nb-*` |
//! | `process.initiated` 55007, 55010 | LF answers to NB-initiated GPKE | `role-lf-*` |
//! | `process.initiated` 55039, 55168 | MSB-Wechsel the **MSB** answers | `role-msb-*` |
//! | `process.initiated` REQOTE PIDs | REQOTE → auto QUOTES | `role-msb-*` |
//! | `makoworkflow = wim-steuerungsauftrag` | §14a auto-ORDRSP | `role-msb-*` |
//! | *(anything else)* | *(ignored)* | — |
//!
//! The role split is load-bearing, not cosmetic: a Kündigung MSB (55039) is
//! MSBN → MSBA and never reaches the NB, and a Steuerungsauftrag ORDRSP is the
//! MSB's answer. Compiling either into an NB binary inverts the § 7 EnWG
//! separation the features exist for.

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use secrecy::ExposeSecret;
use tracing::{debug, warn};

#[cfg(any(
    feature = "role-nb-strom",
    feature = "role-nb-gas",
    feature = "role-msb-strom"
))]
use crate::msb_module;
use crate::server::ProcessdState;

/// Every `process.initiated` PID this build answers.
///
/// Derived from the same `cfg` gates the router uses, so it *is* the § 7 EnWG
/// separation rather than a description of it: a `nb-only` binary that listed
/// an LF or MSB PID here would be listing one it actually answers.
/// `tests/role_separation.rs` asserts the set per role build.
#[must_use]
pub fn answerable_pids() -> Vec<u32> {
    let mut pids: Vec<u32> = Vec::new();

    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    {
        // GPKE / GeLi Gas Anmeldung and Abmeldung STP.
        pids.extend(crate::nb_module::answered_pids());
        // MSB-Wechsel PIDs the NB owes an answer to.
        pids.extend_from_slice(crate::msb_module::NB_ANSWERED_PIDS);
    }
    #[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
    pids.extend(crate::lf_module::lf_antwort_processes().map(|p| p.trigger_pid));
    #[cfg(feature = "role-msb-strom")]
    {
        pids.extend_from_slice(crate::msb_module::MSB_ANSWERED_PIDS);
        pids.extend_from_slice(mako_wim::preisanfrage::REQOTE_PIDS);
    }

    pids.sort_unstable();
    pids.dedup();
    pids
}

/// WiM Steuerungsauftrag confirmation window in Werktage (BK6-22-024) — the
/// Frist `makod` registers as `mako_wim::STEUERUNGSAUFTRAG_DEADLINE_LABEL`.
#[cfg(feature = "role-msb-strom")]
const STEUERUNGSAUFTRAG_ANTWORT_FRIST_WT: u32 = 5;

/// `POST /webhook` — receive a `de.mako.*` event from `marktd`.
pub async fn handle_webhook(
    State(state): State<ProcessdState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── 1. Verify HMAC signature ──────────────────────────────────────────
    let inbound_secret = (*state.inbound_secret)
        .as_ref()
        .map(|s| s.expose_secret().as_bytes().to_vec());
    // The shared verifier also refuses a stale `webhook-timestamp`, so a
    // captured POST cannot be replayed into the projection.
    if let Err(err) =
        mako_service::webhook::verify_request(inbound_secret.as_deref(), &headers, &body)
    {
        warn!(%err, "processd: inbound webhook refused");
        return StatusCode::from(err).into_response();
    }

    // ── 2. Parse JSON body ────────────────────────────────────────────────
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(err) => {
            warn!(%err, "processd: failed to parse CloudEvent");
            return (StatusCode::BAD_REQUEST, "invalid JSON").into_response();
        }
    };

    let ce_type = event["type"].as_str().unwrap_or("").to_owned();

    // ── 3. Route by event type ────────────────────────────────────────────
    // 3a. EoG gap-closure automation (§36/§38 EnWG, NB role) — consumes the
    //     de.markt.versorgung.* triggers before the process.initiated guard.
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    if ce_type == mako_events::markt::VERSORGUNG_GAP_DETECTED
        || ce_type == mako_events::markt::VERSORGUNG_EOG_BEGONNEN
        || ce_type == mako_events::markt::VERSORGUNG_CHANGED
    {
        use crate::eog_module;
        return match eog_module::handle_versorgung_event(
            &event,
            &state.eog,
            &state.marktd,
            &state.makod,
            &state.pool,
            &state.tenant,
            &state.own_mp_id,
        )
        .await
        {
            Ok(_) => StatusCode::OK.into_response(),
            Err(e) => {
                warn!(error = %e, "processd EoG: event handling failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        };
    }

    if ce_type != mako_events::mako::PROCESS_INITIATED {
        debug!(ce_type, "processd: non-initiated event ignored");
        return StatusCode::NO_CONTENT.into_response();
    }

    // ── 4a. NB Neuanlage (55600/55601) ────────────────────────────────────
    //
    // Ahead of the NB module because a Neuanlage is not a one-shot decision:
    // `E_0608` runs a 60-Werktage identification Prüflauf, so the event opens a
    // case rather than producing an answer.
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    {
        use crate::neuanlage_module;
        if let Some(ref nb) = state.nb {
            match neuanlage_module::handle_process_initiated(
                &event,
                &state.neuanlage,
                &state.pool,
                &nb.makod,
            )
            .await
            {
                Ok(true) => return StatusCode::OK.into_response(),
                Ok(false) => {}
                Err(e) => {
                    warn!(error = %e, "processd Neuanlage: evaluation error");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    }

    // ── 4b. NB module ─────────────────────────────────────────────────────
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    {
        use crate::nb_module;
        if let Some(ref nb) = state.nb {
            match nb_module::evaluate_and_decide(
                &event,
                &nb.config,
                &nb.reader,
                nb.einsd.as_ref(),
                &nb.makod,
                &nb.repo,
                &nb.queue,
            )
            .await
            {
                Ok(true) => return StatusCode::OK.into_response(),
                Ok(false) => {} // not an NB PID, fall through to LF module
                Err(e) => {
                    warn!(error = %e, "processd NB: evaluation error");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    }

    // ── 5. LF module ──────────────────────────────────────────────────────
    #[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
    {
        use crate::lf_module;
        if let Some(ref lf) = state.lf {
            match lf_module::process_lf_antwort(
                &event, &lf.config, &lf.reader, &lf.makod, &lf.queue,
            )
            .await
            {
                Ok(true) => return StatusCode::OK.into_response(),
                Ok(false) => {} // not an LF PID
                Err(e) => {
                    warn!(error = %e, "processd LF: evaluation error");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    }
    // ── 6. MSB-Wechsel STP ──────────────────────────────────────────────────
    //
    // Which PIDs this build answers depends on the role it was compiled for:
    // the NB owes 55042 (Anmeldung, MSBN → NB) and 55051 (Ende MSB, MSBA → NB);
    // the MSB owes 55039 (Kündigung, MSBN → MSBA) and 55168
    // (Verpflichtungsanfrage, NB → gMSB).
    #[cfg(any(
        feature = "role-nb-strom",
        feature = "role-nb-gas",
        feature = "role-msb-strom"
    ))]
    {
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| event["data"].get("pid").and_then(|v| v.as_u64()))
            .unwrap_or(0) as u32;

        let mut answerable: Vec<u32> = Vec::new();
        #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
        answerable.extend_from_slice(msb_module::NB_ANSWERED_PIDS);
        #[cfg(feature = "role-msb-strom")]
        answerable.extend_from_slice(msb_module::MSB_ANSWERED_PIDS);

        // The recipient check only applies to the PIDs addressed *to the NB*
        // (55042 / 55051): those carry `grid_operator`, so a message for
        // another operator on a shared bus can be told apart. The MSB-answered
        // PIDs (55039 MSBN → MSBA, 55168 NB → gMSB) name the NB as the *sender*
        // or not at all, so filtering them on `nb_mp_id` would drop every one
        // this deployment legitimately receives.
        let addressed_here = |p: &msb_module::MsbWechselPayload| {
            !msb_module::NB_ANSWERED_PIDS.contains(&p.pid)
                || p.nb_mp_id.is_empty()
                || p.nb_mp_id == state.own_mp_id
        };

        if answerable.contains(&pid)
            && let Some(payload) =
                msb_module::MsbWechselPayload::parse(&event).filter(addressed_here)
        {
            let queue = crate::pg::PgApprovalQueue::new(state.pool.clone());
            return match msb_module::handle_msb_wechsel(
                &msb_module::MsbModuleConfig::for_state(
                    &state.own_mp_id,
                    &state.tenant,
                    state.msb_auto_accept,
                    state.msb_auto_preisanfrage,
                    state.vertragd_url.clone(),
                    state.vertragd_api_key.clone(),
                ),
                payload,
                &state.marktd,
                &state.makod,
                &queue,
            )
            .await
            {
                Ok(()) => StatusCode::OK.into_response(),
                Err(e) => {
                    warn!(error = %e, "processd MSB: STP evaluation failed — fan-out will redeliver");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            };
        }
    }

    // ── 7. §14a Steuerungsauftrag auto-ORDRSP ─────────────────────────────
    //
    // When mako acts as MSB and receives a wim-steuerungsauftrag initiation,
    // auto-confirm if SteuerbareRessource.istFernschaltbar=true AND the
    // dispatched produktcode is in the contracted konfigurationsprodukte.
    //
    // BK6-24-174 §4.3: MSB MUST only confirm a Steuerungsauftrag for
    // products that are under contract.  Uncontracted produktcode → ablehnen.
    #[cfg(feature = "role-msb-strom")]
    if event
        .get("makoworkflow")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        == "wim-steuerungsauftrag"
    {
        let process_id = event["subject"].as_str().unwrap_or("");
        let data = event.get("data").unwrap_or(&serde_json::Value::Null);
        let sr_id = data.get("sr_id").and_then(|v| v.as_str()).unwrap_or("");
        // The dispatched produktcode is in the payload; empty means uncoded command.
        let dispatched_produktcode = data
            .get("produktcode")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Fetch SR + contracted konfigurationsprodukte in parallel.
        let (sr_result, kp_result) = tokio::join!(
            state.marktd.get_steuerbare_ressource(sr_id),
            state.marktd.get_konfigurationsprodukte(sr_id),
        );

        // A marktd outage is not a business finding: only a genuine *absence* of
        // the SteuerbareRessource may escalate. A transport error answers 5xx so
        // the fan-out redelivers.
        if let Err(e) = &sr_result {
            warn!(sr_id, process_id, error = %e, "processd: marktd SteuerbareRessource lookup failed — fan-out will redeliver");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        if let Err(e) = &kp_result {
            warn!(sr_id, process_id, error = %e, "processd: marktd Konfigurationsprodukte lookup failed — fan-out will redeliver");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }

        let is_fernschaltbar: Option<bool> = if sr_id.is_empty() {
            None
        } else {
            sr_result.ok().flatten().and_then(|sr| {
                sr.get("ist_fernschaltbar")
                    .or_else(|| sr.get("istFernschaltbar"))
                    .and_then(|v| v.as_bool())
            })
        };

        // Check whether the dispatched produktcode is contracted.
        // If no konfigurationsprodukte are stored yet, we cannot confirm.
        // If konfigurationsprodukte is an empty array, no products are contracted.
        let contracted: Option<Vec<serde_json::Value>> = kp_result.ok().flatten();
        let produktcode_contracted = if dispatched_produktcode.is_empty() {
            // No produktcode in event — legacy: accept only if there's at least
            // one contracted product (non-empty konfigurationsprodukte).
            contracted.as_ref().is_some_and(|a| !a.is_empty())
        } else {
            contracted.as_ref().is_some_and(|arr| {
                arr.iter().any(|item| {
                    item.get("produktcode")
                        .and_then(|v| v.as_str())
                        .map(|code| code == dispatched_produktcode)
                        .unwrap_or(false)
                })
            })
        };

        // A swallowed dispatch failure drops the ORDRSP the AHB mandates, and the
        // fan-out has already marked the event delivered. Answer 5xx instead.
        let dispatch = |command: &'static str, key: &str, payload: serde_json::Value| {
            let cmd = mako_markt::makod_client::ForwardCommand {
                command: command.to_owned(),
                marktrolle: None,
                malo_id: None,
                melo_id: None,
                payload,
            };
            let idem_key = format!("{key}-{process_id}");
            async move { state.makod.post_command(&idem_key, &cmd).await }
        };

        // Escalations get an approval-queue row — a warn! alone leaves the
        // operator no surface and the ORDRSP unanswered.
        let sa_pid = event["makopid"]
            .as_u64()
            .or_else(|| event["data"]["pid"].as_u64())
            .unwrap_or(0) as i32;
        // The operator window is the WiM Steuerungsauftrag confirmation Frist —
        // 5 Werktage (BK6-22-024, the clock makod registers as
        // `mako_wim::STEUERUNGSAUFTRAG_DEADLINE_LABEL`) — less an hour of
        // headroom. An escalation must not expire before its own process.
        let sa_expires_at = {
            use mako_fristen::{HolidayCalendar, deadline_at_werktage};
            let received_at = event["time"]
                .as_str()
                .and_then(|s| {
                    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .unwrap_or_else(time::OffsetDateTime::now_utc);
            deadline_at_werktage(
                received_at,
                STEUERUNGSAUFTRAG_ANTWORT_FRIST_WT,
                HolidayCalendar::BdewMaKo,
            ) - time::Duration::hours(1)
        };
        let escalate = |reason: String| {
            // The CloudEvent subject is the process UUID. A non-UUID is a broken
            // producer contract: it must surface as an error, since acking it
            // would drop the escalation and the AHB-mandated ORDRSP with it.
            let entry = process_id.parse().map(|pid_uuid| {
                crate::pg::approval::ApprovalQueueEntry::pending(
                    pid_uuid,
                    sa_pid,
                    None,
                    reason,
                    sa_expires_at,
                    state.tenant.clone(),
                )
                .with_commands(
                    mako_markt::commands::WIM_STEUERUNGSAUFTRAG_BESTAETIGEN,
                    mako_markt::commands::WIM_STEUERUNGSAUFTRAG_ABLEHNEN,
                    Some("MSB"),
                )
            });
            let queue = crate::pg::PgApprovalQueue::new(state.pool.clone());
            async move {
                match entry {
                    Ok(e) => queue.enqueue(&e).await.map_err(|e| e.to_string()),
                    Err(e) => Err(format!(
                        "Steuerungsauftrag CloudEvent subject {process_id:?} is not a \
                         process UUID ({e}) — cannot queue the escalation"
                    )),
                }
            }
        };

        let result: Result<(), String> = match (is_fernschaltbar, produktcode_contracted) {
            (Some(true), true) => {
                // Auto-confirm: SR is remote-switchable and produktcode is contracted.
                dispatch(
                    mako_markt::commands::WIM_STEUERUNGSAUFTRAG_BESTAETIGEN,
                    "steuerungsauftrag-bestaetigen",
                    serde_json::json!({
                        "process_id": process_id,
                        "auto_ordrsp": true,
                        "produktcode": dispatched_produktcode,
                    }),
                )
                .await
                .map(|_| {
                    debug!(
                        sr_id,
                        process_id,
                        produktcode = dispatched_produktcode,
                        "processd: Steuerungsauftrag auto-confirmed (istFernschaltbar=true, produktcode contracted)"
                    );
                })
                .map_err(|e| e.to_string())
            }
            (Some(true), false) => {
                // SR is remote-switchable but produktcode is NOT contracted — must ablehnen.
                // BK6-24-174 §4.3: dispatch only for contracted products.
                warn!(
                    sr_id,
                    process_id,
                    produktcode = dispatched_produktcode,
                    "processd: Steuerungsauftrag ablehnen — produktcode not in contracted konfigurationsprodukte (BK6-24-174 §4.3)"
                );
                dispatch(
                    mako_markt::commands::WIM_STEUERUNGSAUFTRAG_ABLEHNEN,
                    "steuerungsauftrag-ablehnen",
                    serde_json::json!({
                        "process_id": process_id,
                        "reason": "produktcode not in contracted konfigurationsprodukte (BK6-24-174 §4.3)",
                        "produktcode": dispatched_produktcode,
                    }),
                )
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
            }
            (Some(false), _) => {
                // SR is not remote-switchable — escalate to operator.
                warn!(
                    sr_id,
                    process_id,
                    "processd: Steuerungsauftrag escalated — istFernschaltbar=false; manual ORDRSP required"
                );
                escalate(format!(
                    "SteuerbareRessource {sr_id} has istFernschaltbar=false — manual ORDRSP required"
                ))
                .await
            }
            (None, _) => {
                // Unknown SR or marktd unavailable — escalate.
                warn!(
                    sr_id,
                    process_id,
                    "processd: Steuerungsauftrag escalated — SR not found in marktd or ist_fernschaltbar unknown"
                );
                escalate(format!(
                    "SteuerbareRessource {sr_id} not found in marktd or istFernschaltbar unknown"
                ))
                .await
            }
        };

        return match result {
            Ok(()) => StatusCode::OK.into_response(),
            Err(e) => {
                warn!(sr_id, process_id, error = %e, "processd: Steuerungsauftrag handling failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        };
    }

    // ── 8. REQOTE Preisanfrage → auto QUOTES ──────────────────────────────
    //
    // The `auto_preisanfrage` switch is inside the handler rather than here:
    // turning automation off means "an operator quotes", not "nobody answers",
    // so the disabled path still has to produce an approval-queue entry with
    // the WiM Antwortfrist on it.
    #[cfg(feature = "role-msb-strom")]
    {
        let queue = crate::pg::PgApprovalQueue::new(state.pool.clone());
        match msb_module::handle_preisanfrage_reqote(
            &event,
            &msb_module::MsbModuleConfig::for_state(
                &state.own_mp_id,
                &state.tenant,
                state.msb_auto_accept,
                state.msb_auto_preisanfrage,
                state.vertragd_url.clone(),
                state.vertragd_api_key.clone(),
            ),
            &state.marktd,
            &state.makod,
            &queue,
        )
        .await
        {
            Ok(true) => return StatusCode::OK.into_response(),
            Ok(false) => {} // not a REQOTE PID
            Err(e) => {
                warn!(error = %e, "processd MSB: REQOTE handling failed — fan-out will redeliver");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod frist_tests {
    use mako_fristen::antwort::operator_window;
    use time::{Date, Month, OffsetDateTime, Time};

    fn utc(y: i32, m: Month, d: u8, h: u8) -> OffsetDateTime {
        OffsetDateTime::new_utc(
            Date::from_calendar_date(y, m, d).expect("valid date"),
            Time::from_hms(h, 0, 0).expect("valid time"),
        )
    }

    /// Every PID a compiled role can answer must resolve to a *regulatory*
    /// window — an operating-convention fallback on a process this deployment
    /// actually runs means an unread Festlegung, not an acceptable default.
    #[test]
    fn every_answerable_pid_has_a_published_frist() {
        let received = utc(2026, Month::March, 2, 9);
        let unknown: Vec<u32> = super::answerable_pids()
            .into_iter()
            .filter(|p| !operator_window(*p, received).is_regulatory)
            .collect();
        assert!(
            unknown.is_empty(),
            "these PIDs are answered but have no published Antwortfrist: {unknown:?}"
        );
    }

    /// The headroom must never invert the window.
    #[test]
    fn the_queue_expires_before_the_deadline() {
        let received = utc(2026, Month::March, 2, 9);
        for pid in super::answerable_pids() {
            let w = operator_window(pid, received);
            assert!(w.expires_at < w.deadline, "PID {pid}");
            assert!(
                w.expires_at > received,
                "PID {pid} expires before it arrives"
            );
        }
    }

    /// A Friday Gas Anmeldung: four Werktage, not ten, and not 24 hours.
    #[test]
    fn a_gas_anmeldung_is_four_werktage() {
        let w = operator_window(44_001, utc(2026, Month::March, 2, 9));
        assert!(w.is_regulatory);
        assert_eq!(
            w.deadline.date(),
            Date::from_calendar_date(2026, Month::March, 6).expect("valid date")
        );
    }

    #[test]
    fn an_unknown_pid_still_expires() {
        let received = utc(2026, Month::March, 2, 9);
        let w = operator_window(99_999, received);
        assert!(!w.is_regulatory);
        assert!(w.expires_at > received);
    }
}
