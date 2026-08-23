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
    /// All operator MP-IDs for § 7a Abs. 5 EnWG `initiator_is_affiliate` detection.
    ///
    /// Tested against the Lieferant on the processes the network arm answers.
    /// That set is derived from the Antwortfrist table rather than from a
    /// literal PID list — see `counterparty_is_affiliate`.
    /// `Arc<HashSet>` for O(1) lookup without clone.
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
    // The shared verifier also refuses a stale `webhook-timestamp`, so a
    // captured POST cannot be replayed into the projection.
    if let Err(err) =
        mako_service::webhook::verify_request(inbound_secret.as_deref(), &headers, &body)
    {
        warn!(%err, "obsd: inbound webhook refused");
        return StatusCode::from(err).into_response();
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
    let (started_at, existing) = match state.repo.get(process_id).await {
        Ok(Some(existing)) => (existing.started_at, Some(existing)),
        Ok(None) => (event_time, None),
        Err(err) => {
            warn!(%err, process_id = %process_id, "obsd: projection read failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let existing_deadline = existing.as_ref().and_then(|e| e.deadline_at);
    let existing_source = existing.as_ref().and_then(|e| e.deadline_source.clone());

    // The Frist is anchored once and never moved. The fan-out is at-least-once,
    // so a redelivered `process.initiated` must not recompute it from its own
    // arrival — that would silently extend a regulatory window. The upsert
    // refuses to overwrite a stored `deadline_at`, but `deadline_risk` is
    // written unconditionally, so the guard has to be here too.
    let frist = if existing_deadline.is_some() {
        None
    } else if matches!(state_val, ProcessState::Initiated) {
        answer_frist(pid, event_time)
    } else {
        // A projection with no prior row: events arrived out of order, so the
        // clock starts at the earliest instant seen for this process.
        answer_frist(pid, started_at)
    };
    let (deadline_at, deadline_source) = match frist {
        Some(f) => (Some(f.due_at), Some(f.source.to_owned())),
        None => (existing_deadline, existing_source),
    };
    // No published Frist is `Unknown`, never `Green`: "we have not read that
    // Festlegung" and "there is time" must not render the same on a dashboard.
    let deadline_risk = DeadlineRisk::classify_opt(deadline_at, event_time);

    // § 7a Abs. 5 EnWG Gleichbehandlung: detect affiliate initiators.
    //
    // Which PIDs count is read off the Antwortfrist table rather than hard-coded:
    // an initiating Lieferant is named in `new_supplier` on exactly the PIDs that
    // start a supply relationship, and those are the ones the Festlegung gives an
    // answer window. A literal list drifts from it — 55016 is a Kündigung whose
    // payload names no new supplier, and 55077 (erzeugende MaLo) is easy to omit.
    let initiator_is_affiliate = counterparty_is_affiliate(
        pid,
        data["new_supplier"].as_str(),
        partner_mp_id.as_deref(),
        &state.own_mp_ids,
    );

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
        deadline_source,
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

/// Whether this process belongs in the § 7a Abs. 5 EnWG parity comparison, and
/// whether its counterparty is inside the operator's own undertaking.
///
/// **The comparison is over the processes the operator's *network* arm answers
/// for a Lieferant** — those are the ones where the network operator is the
/// party doing the treating, and the only ones a Gleichbehandlungsbericht can
/// say anything about. The set is read off the Antwortfrist table
/// (`answered_by == "NB"` in the GPKE and GeLi Gas families) rather than typed
/// out, so a Festlegung change moves both the deadline and the report.
///
/// A literal PID list gets this wrong in both directions: the Kündigung (55016)
/// is answered by the *old supplier* and belongs to no NB report, while 55077
/// (Anmeldung erzeugende Marktlokation) is an NB-answered Lieferbeginn that is
/// easy to omit.
///
/// The MP-ID compared is `new_supplier` where the message carries one and the
/// counterparty otherwise: an Abmeldung names no new supplier, but the Lieferant
/// being treated is still the sender.
fn counterparty_is_affiliate(
    pid: u32,
    new_supplier: Option<&str>,
    partner_mp_id: Option<&str>,
    own_mp_ids: &HashSet<String>,
) -> bool {
    if own_mp_ids.is_empty() || !is_nb_answered_lieferanten_process(pid) {
        return false;
    }
    new_supplier
        .or(partner_mp_id)
        .is_some_and(|mp| own_mp_ids.contains(mp))
}

/// PIDs whose answer the operator's network arm owes a Lieferant.
///
/// Derived from the Antwortfrist table, so it cannot drift from the Festlegung.
fn is_nb_answered_lieferanten_process(pid: u32) -> bool {
    use mako_fristen::antwort::Family;
    mako_fristen::antwort::antwort_obligation(pid).is_some_and(|o| {
        o.answered_by == "NB" && matches!(o.family, Family::Gpke | Family::GeliGas)
    })
}

/// Derive the process family for a projection row.
///
/// Three sources, in order of authority:
///
/// 1. **The Antwortfrist table** — when a PID carries a published window, the
///    Festlegung that states it *is* the family. One fact, not a second guess.
/// 2. **The workflow name** `makod` put on the event, for PIDs with no window.
/// 3. **A PID range**, for events that carry neither.
///
/// The ranges are the weakest source and are kept only because billing, PARTIN
/// and INSRPT PIDs have no Antwortfrist and often no workflow name.
fn derive_family(workflow_name: &str, pid: u32) -> String {
    if let Some(f) = mako_fristen::antwort::antwort_obligation(pid) {
        return f.family.as_str().to_owned();
    }
    // The Gas-only Prüfidentifikatoren outrank the workflow name.
    //
    // WiM runs one workflow for both Sparten — `wim-device-change` carries the
    // Strom 55039/55042/55051/55168 and the Gas 44039/44042/44051/44168 alike —
    // so the name says „wim" for a process the BNetzA reports under Gas. The
    // PID is what carries the Sparte, and where it does it is the better
    // source. (Both Sparten' MSB-Wechsel PIDs have an Antwortfrist, so they are
    // already answered above; this covers the two INSRPT PIDs that have none.)
    //
    // 31003 is **not** in this set: the WiM-Rechnung über Dienstleistungen im
    // Messwesen exists in both Sparten (WiM Strom Teil 1 Kap. 3.7, AWH WiM Gas
    // 2.0 Kap. 4.7), so bucketing it as Gas mislabels every Strom invoice.
    if matches!(pid, 23_005 | 23_009) {
        return "wim-gas".to_owned();
    }
    if !workflow_name.is_empty() {
        // Longest prefix first: `wim-gas` must not be swallowed by `wim`.
        for prefix in ["gpke", "geli-gas", "wim-gas", "wim", "gabi-gas", "mabis"] {
            if workflow_name.starts_with(prefix) {
                return prefix.to_owned();
            }
        }
    }
    match pid {
        // ── GPKE — Lieferwechsel Strom (BK6-24-174) ───────────────────────────
        55001..=55018 | 55022..=55024 | 55077 | 55080 | 55555 | 55607..=55609 => "gpke",
        17115..=17117 => "gpke",                 // ORDERS Sperrung Strom
        17134 | 17135 => "gpke",                 // ORDERS/ORDRSP Konfiguration Strom
        19001 | 19002 => "gpke", // ORDRSP Konfiguration / Geräteübernahme (multi-domain, gpke bucket)
        37000..=37006 => "gpke", // PARTIN Strom Kommunikationsdaten
        31001 | 31002 | 31005 | 31006 => "gpke", // INVOIC NNE/MMM/selbst ausgest. Strom
        // ── WiM — Messstellenbetrieb Strom (BK6-24-174) ───────────────────────
        55039..=55044 | 55051..=55053 | 55168..=55170 => "wim",
        17001..=17011 => "wim", // ORDERS Geräteübernahme (nMSB)
        23001 | 23003 | 23004 | 23008 | 23011 | 23012 => "wim", // INSRPT, beide Sparten
        27001..=27003 => "wim", // PRICAT Preisliste
        31009 => "wim",         // INVOIC MSB-Rechnung (Strom)
        31003 => "wim",         // INVOIC WiM-Rechnung Dienstleistungen, beide Sparten
        35001..=35005 | 15001..=15005 => "wim", // REQOTE/QUOTES Preisanfrage
        // ── GeLi Gas — Lieferbeginn/-ende Gas (BK7-24-01-009) ─────────────────
        44001..=44024 => "geli-gas", // UTILMD G incl. 44022-44024 role-conditional
        37008..=37014 => "geli-gas", // PARTIN Gas Kommunikationsdaten
        31011 => "geli-gas",         // INVOIC AWH Sperrprozesse Gas (GNB→LFG)
        // ── WiM Gas — Messstellenbetrieb Gas (AWH WiM Gas 2.0) ───────────────
        44039..=44053 | 44168 | 44169 | 44183 => "wim-gas",
        23005 | 23009 => "wim-gas", // INSRPT Gas-only variants
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

/// The regulatory answer deadline for a process, and the citation behind it.
///
/// Single-sourced from [`mako_fristen::antwort`] — the same table `makod`
/// registers the deadline from and `processd` sizes its operator queue by, so
/// an obsd breach alert and the process's own deadline are one number.
///
/// Returns `None` for a PID whose window no Festlegung this codebase has read
/// quantifies (billing, PARTIN, INSRPT, and the GeLi Gas processes whose Frist
/// is set per Netzbetreiber under Kap. 2.6). That is **unknown**, not
/// unbounded: obsd registers no deadline, so the process never appears in a
/// breach sweep on an instant nobody can cite.
///
/// No flat window is correct here: GPKE Teil 2 states clock times on the 1.
/// Werktag after the ÜT, and the GeLi Gas „10 Werktage" is the *supplier's*
/// Vorlauffrist rather than the Netzbetreiber's 4-Werktage answer window.
/// `obsd_agrees_with_the_table_on_every_published_window` reads the PID list off
/// the table, so it cannot pass by only checking the families that agree.
pub fn answer_frist(
    pid: u32,
    started_at: time::OffsetDateTime,
) -> Option<mako_fristen::antwort::Antwortfrist> {
    mako_fristen::antwort::antwortfrist(pid, started_at)
}

/// The deadline instant alone.
#[must_use]
pub fn compute_deadline(
    pid: u32,
    started_at: time::OffsetDateTime,
) -> Option<time::OffsetDateTime> {
    answer_frist(pid, started_at).map(|f| f.due_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    // ── derive_family ─────────────────────────────────────────────────────────

    /// A PID with a published Frist takes its family from the Festlegung that
    /// states it — one fact, not a second guess beside the first.
    #[test]
    fn a_pid_with_a_published_frist_takes_its_family_from_the_table() {
        assert_eq!(derive_family("", 55_001), "gpke");
        assert_eq!(derive_family("", 44_001), "geli-gas");
        assert_eq!(derive_family("", 55_039), "wim");
        assert_eq!(derive_family("", 44_042), "wim-gas");
        // …even when the event's workflow name says otherwise.
        assert_eq!(derive_family("geli-gas-lieferbeginn", 55_001), "gpke");
    }

    #[test]
    fn the_workflow_name_decides_when_no_frist_is_published() {
        assert_eq!(derive_family("gpke-supplier-change", 31_001), "gpke");
        assert_eq!(derive_family("mabis-billing", 13_003), "mabis");
    }

    /// `wim-gas` must not be swallowed by the shorter `wim` prefix.
    #[test]
    fn the_longer_workflow_prefix_wins() {
        assert_eq!(derive_family("wim-gas-anmeldung", 23_005), "wim-gas");
        // 31003 exists in both Sparten and must not be bucketed as Gas.
        assert_eq!(derive_family("", 31_003), "wim");
        assert_eq!(derive_family("wim-insrpt", 23_001), "wim");
    }

    #[test]
    fn the_pid_range_is_the_last_resort() {
        assert_eq!(derive_family("", 31_009), "wim"); // MSB-Rechnung
        assert_eq!(derive_family("", 37_008), "geli-gas"); // PARTIN Gas
        assert_eq!(derive_family("", 31_011), "geli-gas"); // AWH Sperrprozesse Gas
        assert_eq!(derive_family("", 13_013), "gabi-gas"); // MSCONS MMMA
        assert_eq!(derive_family("", 31_004), "invoic-storno");
        assert_eq!(derive_family("", 99_999), "unknown");
        assert_eq!(derive_family("", 0), "unknown");
    }

    // ── The Antwortfrist ──────────────────────────────────────────────────────

    /// A GPKE Anmeldung is due at a clock time on the next Werktag.
    ///
    /// This replaces `compute_deadline_gpke_24h`, which asserted
    /// `started + 24 h` and therefore pinned the defect: BK6-24-174 Teil 2
    /// states 11:00 Uhr des 1. WT nach dem ÜT, so a Friday arrival is
    /// answerable until Monday and a Tuesday-evening one has under sixteen
    /// hours. obsd breached the first on Saturday and called the second healthy
    /// nine hours late.
    #[test]
    fn a_gpke_anmeldung_is_not_twenty_four_hours() {
        let started = datetime!(2026-07-17 14:00 UTC); // Friday
        let f = answer_frist(55_001, started).expect("published");
        assert_ne!(f.due_at, started + time::Duration::hours(24));
        assert_eq!(f.due_at.date(), time::macros::date!(2026 - 07 - 20));
        assert!(f.source.contains("BK6-24-174"));
    }

    /// A Gas Anmeldung is four Werktage, not the supplier's ten.
    #[test]
    fn a_gas_anmeldung_is_four_werktage() {
        let started = datetime!(2026-03-02 09:00 UTC); // Monday
        let f = answer_frist(44_001, started).expect("published");
        assert_eq!(f.due_at.date(), time::macros::date!(2026 - 03 - 06));
    }

    /// WiM Strom keeps four distinct windows.
    #[test]
    fn wim_strom_is_per_pid() {
        let started = datetime!(2026-07-14 08:00 UTC);
        let all: std::collections::BTreeSet<_> = [55_039_u32, 55_042, 55_051, 55_168]
            .into_iter()
            .map(|p| compute_deadline(p, started).expect("published"))
            .collect();
        assert_eq!(all.len(), 4);
    }

    /// obsd and the engine resolve the same instant, for every published PID,
    /// on every day of a year.
    ///
    /// The test this replaces made the same claim and only ever checked the
    /// families that already agreed, so the flat GPKE and Gas windows passed it.
    /// Reading the PID list off the table is what makes it exhaustive.
    #[test]
    fn obsd_agrees_with_the_table_on_every_published_window() {
        let mut day = datetime!(2026-01-01 09:00 UTC);
        for _ in 0..365 {
            for o in mako_fristen::antwort::all() {
                assert_eq!(
                    compute_deadline(o.trigger_pid, day),
                    mako_fristen::antwort::antwort_deadline(o.trigger_pid, day),
                    "PID {} at {day}",
                    o.trigger_pid
                );
            }
            day += time::Duration::days(1);
        }
    }

    /// A PID with no published window gets no deadline — unknown, not
    /// The ESA Wertebestellung is monitored like any other WiM process: its
    /// four inbound PIDs carry published windows, so a Werteanfrage or
    /// Bestellung that goes unanswered is visible as a breach rather than as a
    /// process with no deadline at all.
    #[test]
    fn the_esa_wertebestellung_is_monitored() {
        let received = datetime!(2026-03-02 08:00 UTC); // Monday
        for pid in [35_003_u32, 17_007, 17_008, 39_002] {
            let f = answer_frist(pid, received)
                .unwrap_or_else(|| panic!("PID {pid} must carry a published window"));
            assert!(f.due_at > received, "PID {pid}");
            assert!(
                f.source.contains("Teil 2"),
                "PID {pid} draws its window from WiM Teil 2, got {:?}",
                f.source
            );
            assert_eq!(derive_family("", pid), "wim", "PID {pid}");
        }
    }

    /// The §7a Abs. 5 Gleichbehandlung report covers the processes the
    /// operator's **network** arm answers for a Lieferant. An ESA order is
    /// answered by the MSB and names no Lieferant, so it must stay out.
    #[test]
    fn an_esa_order_is_not_in_the_affiliate_report() {
        let own: HashSet<String> = ["9900357000004".to_owned()].into_iter().collect();
        for pid in [35_003_u32, 17_007, 17_008, 39_002] {
            assert!(
                !counterparty_is_affiliate(pid, None, Some("9900357000004"), &own),
                "PID {pid} is an MSB obligation, not an NB one"
            );
        }
    }

    /// unbounded, and never measured against an instant nobody can cite.
    #[test]
    fn an_unpublished_window_yields_no_deadline() {
        let t = datetime!(2026-07-14 00:00 UTC);
        // 44020 Änderungsmeldung zur Bestandsliste is the GeLi Gas process
        // whose Frist really is per-Netzbetreiber. 44010 sat in this list until
        // the AWH was read: Kap. 2.5.2 Nr. 4 quantifies it at Ablauf des 3. WT,
        // so obsd raised no breach alert for the supplier's own answer window.
        for pid in [31_001_u32, 37_000, 23_001, 44_020, 13_003] {
            assert!(compute_deadline(pid, t).is_none(), "PID {pid}");
        }
        for pid in [44_007_u32, 44_010] {
            assert!(
                compute_deadline(pid, t).is_some(),
                "PID {pid} has a published Antwortfrist"
            );
        }
    }

    /// No deadline is not the same as plenty of time.
    #[test]
    fn no_deadline_classifies_as_unknown_risk() {
        let now = datetime!(2026-07-14 00:00 UTC);
        assert_eq!(DeadlineRisk::classify_opt(None, now), DeadlineRisk::Unknown);
    }

    // ── § 7a Abs. 5 EnWG parity scope ─────────────────────────────────────────

    fn own(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    /// The parity set is the NB-answered Lieferanten processes, taken from the
    /// Antwortfrist table.
    #[test]
    fn the_parity_set_is_the_nb_answered_lieferanten_processes() {
        for pid in [55_001_u32, 55_077, 55_004, 44_001, 44_004] {
            assert!(
                is_nb_answered_lieferanten_process(pid),
                "PID {pid} is answered by the NB for a Lieferant"
            );
        }
    }

    /// **55016 is the Kündigung, answered by the old supplier — never the NB.**
    ///
    /// It was in the literal list the report used, so an NB's own parity figures
    /// counted a process the NB does not handle. Its payload also names no
    /// `new_supplier`, so the flag could never be set on it: a bucket that
    /// silently contributed only denominators.
    #[test]
    fn the_kuendigung_is_not_in_the_nb_parity_set() {
        assert!(!is_nb_answered_lieferanten_process(55_016));
        assert!(!is_nb_answered_lieferanten_process(44_016));
        // 55007 is NB-*initiated* and answered by the LF — also out of scope.
        assert!(!is_nb_answered_lieferanten_process(55_007));
        // 44013 is answered by the Ersatz-/Grundversorger, not the NB.
        assert!(!is_nb_answered_lieferanten_process(44_013));
        // WiM is an MSB process, not a Lieferanten one.
        assert!(!is_nb_answered_lieferanten_process(55_039));
    }

    #[test]
    fn an_affiliate_initiator_is_flagged_and_a_third_party_is_not() {
        let ours = own(&["9900357000004", "9800357000004"]);
        assert!(counterparty_is_affiliate(
            55_001,
            Some("9900357000004"),
            None,
            &ours
        ));
        assert!(!counterparty_is_affiliate(
            55_001,
            Some("9912345678901"),
            None,
            &ours
        ));
    }

    /// An Abmeldung names no new supplier; the Lieferant being treated is still
    /// the sender, so the counterparty MP-ID carries the comparison.
    #[test]
    fn a_message_without_a_new_supplier_falls_back_to_the_counterparty() {
        let ours = own(&["9900357000004"]);
        assert!(counterparty_is_affiliate(
            55_004,
            None,
            Some("9900357000004"),
            &ours
        ));
    }

    /// A deployment that configured no MP-IDs flags nothing, rather than
    /// flagging everything or nothing-by-accident.
    #[test]
    fn an_unconfigured_deployment_flags_nothing() {
        assert!(!counterparty_is_affiliate(
            55_001,
            Some("9900357000004"),
            None,
            &HashSet::new()
        ));
    }
}
