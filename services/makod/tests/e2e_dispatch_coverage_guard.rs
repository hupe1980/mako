//! Dispatch-coverage guard: every registered PID must reach a dispatch arm.
//!
//! Regression guard for the "registered-but-not-dispatched" bug class. A PID is
//! registered in the router — so `resolve_workflow` returns its workflow and
//! makod claims to handle the message — but the ingest `match pid` arm has no
//! branch for it, so the inbound message is silently dropped
//! (`Skipped { pid_not_in_* }`). Nothing errors; the message simply vanishes.
//!
//! For every PID each domain module registers, this test dispatches a real
//! message and asserts the outcome is **not** `pid_not_in_*`. Dispatch arms
//! match on the passed PID rather than message content, so any parseable
//! message of the right type suffices: `process_not_found` / `no_correlation_key` /
//! adapter `Err` all mean "the PID reached its arm" — only `pid_not_in_*` is
//! the bug.
//!
//! Messages come from the `edi-energy` fixtures (`valid/` first, then the
//! generated `gen/` minimals, then any fixture of the same message type whose
//! content carries the PID), falling back to a synthetic template for reply
//! families that have no fixture at all.
//!
//! Reach today: **318 of 432 registrations**. Every registration it cannot
//! exercise is a Prüfidentifikator with no AHB profile entry — the
//! `KNOWN_PROFILE_GAPS` set from `e2e_ahb_rule_coverage_guard` — plus the
//! send-only entries below. That is one root cause with three downstream
//! effects: no profile entry means validation passes vacuously,
//! `generate-fixtures` skips the PID, and its dispatch arm goes unexercised
//! here. Closing the AHB coverage gap closes all three.
//!
//! The ratchet below therefore measures reach over the **profiled** subset: a
//! PID with no profile cannot have a fixture, so counting it would let the AHB
//! backlog dilute a guard that exists to detect fixture loss.
//!
//! Router is built **per module** — a combined all-roles router would panic on
//! the deliberate geli-gas ↔ wim-gas 44022–44024 `register_with_module`
//! conflict.

use std::sync::Arc;

use mako_engine::builder::EngineModule;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const LOC: &str = "51238696012";

/// PIDs mako registers but deliberately does not receive, with the reason.
///
/// Every entry is a process where **mako implements the initiating side and
/// sends this PID**; the counterparty role that would receive it has no
/// workflow yet. They stay registered so `validate_dispatch_completeness` and
/// the AHB-coverage guard still see the process, and so the outbound render
/// path keeps a name to resolve.
///
/// This list must only ever shrink. Implementing a receiver makes the PID reach
/// an arm, and the test then fails until the entry is removed — so it cannot go
/// stale in the quiet direction.
const SEND_ONLY_PIDS: &[(u32, &str, &str)] = &[
    // ── Registered, received, but not answerable ─────────────────────────────
    // 55557 (Änderung MSB-Abrechnungsdaten, GPKE Teil 4) has no Antwort mapping
    // in `response_pid_for`, so `gpke-supplier-change` cannot carry it to
    // completion: spawning it yields a process the registered deadline
    // turns into a false `Rejected`. It stays registered so the router resolves
    // it; the receiving implementation is the missing piece.
    (
        55557,
        "gpke-supplier-change",
        "Änderung MSB-Abr.-Daten (GPKE Teil 4) — no Antwort mapping, receiver not implemented",
    ),
    // ── MaBiS MSCONS series (NB → MSB · NB → LF) ─────────────────────────────
    // Built by the aggregation layer (`startup.rs`) and rendered outbound;
    // makod never ingests one.
    (
        13003,
        "mabis-billing",
        "Abrechnungssummenzeitreihe — aggregation layer",
    ),
    (13010, "mabis-billing", "normiertes Profil — outbound only"),
    (13011, "mabis-billing", "Profilschar — outbound only"),
    (
        13012,
        "mabis-billing",
        "TEP vergl. Werte Referenzmessung — outbound only",
    ),
    // ── GPKE Datenabruf ORDERS Anfragen (→ MSB) ──────────────────────────────
    // mako is the requester; it ingests only the ORDRSP answers. An MSB-side
    // receiver for these does not exist.
    (
        17004,
        "gpke-datenabruf",
        "Anforderung von Werten — MSB receiver not implemented",
    ),
    (
        17102,
        "gpke-datenabruf",
        "Anfrage Werte — MSB receiver not implemented",
    ),
    (
        17113,
        "gpke-datenabruf",
        "Reklamation Werte — MSB receiver not implemented",
    ),
    // ── MMM Allokationsliste Anforderungen ───────────────────────────────────
    (
        17110,
        "gpke-allokationsliste",
        "Anforderung Allokationsliste (LF → NB) — NB receiver not implemented",
    ),
    // Retired in ORDERS AHB 1.1b (01.04.2026): absent from the current
    // `fv20260401` profile and from the PID overview 4.0 (see `RETIRED_PIDS` in
    // xtask). Still registered so messages from the `fv20251001` window resolve,
    // but no counterparty can send one under the current release — this entry is
    // to be dropped, not implemented.
    (
        17114,
        "gpke-allokationsliste",
        "Anforderung bilanzierte Menge (NB → ÜNB) — RETIRED 01.04.2026, do not implement",
    ),
    // ── Sperrprozesse ────────────────────────────────────────────────────────
    (
        17116,
        "gpke-sperrung",
        "Anfrage Sperrung (NB → MSB) — MSB receiver not implemented",
    ),
    (
        17116,
        "geli-gas-sperrung-nb",
        "Anfrage Sperrung (NB → MSB) — MSB receiver not implemented",
    ),
    // ── GPKE Teil 3 Konfigurationsänderung ORDERS Anfragen (LF → NB/MSB) ─────
    // mako implements the LF side: it sends these and ingests the ORDRSP /
    // IFTSTA answers. The NB/MSB receiving side is a separate workflow that
    // does not exist yet.
    (
        17120,
        "gpke-konfiguration-aenderung",
        "Bestellung Änderung Prognosegrundlage — NB receiver not implemented",
    ),
    (
        17121,
        "gpke-konfiguration-aenderung",
        "Bestellung Änderung (NB → MSB) — MSB receiver not implemented",
    ),
    (
        17122,
        "gpke-konfiguration-aenderung",
        "Reklamation einer Definition — NB receiver not implemented",
    ),
    (
        17123,
        "gpke-konfiguration-aenderung",
        "Bestellung Änderung Zählzeitdefinition — NB receiver not implemented",
    ),
    (
        17128,
        "gpke-konfiguration-aenderung",
        "Reklamation einer Konfiguration — MSB receiver not implemented",
    ),
    (
        17129,
        "gpke-konfiguration-aenderung",
        "Bestellung Beendigung Konfiguration — MSB receiver not implemented",
    ),
    (
        17130,
        "gpke-konfiguration-aenderung",
        "Bestellung Konfiguration (ohne Angebot) — MSB receiver not implemented",
    ),
    (
        17131,
        "gpke-konfiguration-aenderung",
        "Bestellung Angebot Konfiguration — MSB receiver not implemented",
    ),
    (
        17133,
        "gpke-konfiguration-aenderung",
        "Bestellung Änderung Abrechnungsdaten — NB receiver not implemented",
    ),
    // ── WiM Technikänderung ORDERS Anfragen ──────────────────────────────────
    // mako implements the requester side: it sends these and ingests the
    // ORDRSP 19003–19007 answers. The MSB receiving side has no command —
    // `TechnikAenderungCommand` models `SendAuftrag`, not `ReceiveAuftrag`.
    (
        17011,
        "wim-technik-aenderung",
        "Beauftragung Änderung der Technik (LF/NB → MSB) — MSB receiver not implemented",
    ),
    (
        17118,
        "wim-technik-aenderung",
        "Bestellung Konfigurationsänderung (MSB → MSB) — MSB receiver not implemented",
    ),
];

/// The EDIFACT message directory a PID's band belongs to.
fn fixture_dir(pid: u32) -> Option<&'static str> {
    Some(match pid / 1000 {
        13 => "mscons",
        15 => "quotes",
        17 => "orders",
        19 => "ordrsp",
        21 => "iftsta",
        23 => "insrpt",
        25 => "utilts",
        27 => "pricat",
        29 => "aperak",
        31 => "invoic",
        33 => "remadv",
        35 => "reqote",
        37 => "partin",
        39 => "ordchg",
        44 | 55 => "utilmd",
        _ => return None,
    })
}

/// A real fixture carrying `pid`, preferring the curated file over the
/// generated one, then any fixture of the same message type whose content
/// carries the PID.
///
/// The content scan matters: 28 Prüfidentifikatoren have no file named after
/// them but do appear in another fixture's `BGM` DE1004 or `RFF+Z13` — which is
/// exactly how `validate-pruefids` and `generate-fixtures` count coverage. A
/// filename-only lookup silently skips those, leaving their dispatch arms
/// unguarded.
fn fixture(pid: u32) -> Option<String> {
    let dir = fixture_dir(pid)?;
    let base = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/edi-energy/tests/fixtures"
    );
    let named = [
        format!("{base}/{dir}/valid/pid_{pid}.edi"),
        format!("{base}/{dir}/gen/pid_{pid}.gen.edi"),
    ]
    .into_iter()
    .find_map(|p| std::fs::read_to_string(p).ok());
    if named.is_some() {
        return named;
    }

    // Fall back to any fixture of this message type that carries the PID.
    for sub in ["valid", "gen"] {
        let Ok(entries) = std::fs::read_dir(format!("{base}/{dir}/{sub}")) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if carries_pid(&text, pid) {
                return Some(text);
            }
        }
    }
    None
}

/// `true` when the interchange announces `pid` in `BGM` DE1004 or `RFF+Z13`.
///
/// Both carry the Prüfidentifikator with optional leading zeros, which is why
/// this trims rather than comparing the raw token.
fn carries_pid(text: &str, pid: u32) -> bool {
    let want = pid.to_string();
    for seg in text.split('\'') {
        let seg = seg.trim();
        if let Some(rest) = seg.strip_prefix("RFF+Z13:")
            && rest.trim_start_matches('0').starts_with(&want)
        {
            return true;
        }
        if seg.starts_with("BGM+") && seg.split('+').any(|f| f.trim_start_matches('0') == want) {
            return true;
        }
    }
    false
}

/// A synthetic reply-family message, for PIDs with no fixture on disk.
///
/// Each carries a `LOC` and an `RFF+ON`/`Z13` so both the MaLo and the
/// order-reference correlation paths have something to read.
fn synthetic_reply(pid: u32) -> Option<String> {
    let msg = match pid {
        15000..=15999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+QUOTES:D:10A:UN:1.3c'BGM+310+000{pid}'DTM+137:20230101:102'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'LOC+172+{LOC}'UNT+8+1'UNZ+1+1'"
        ),
        19000..=19999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+ORDRSP:D:10A:UN:1.4c'BGM+7+000{pid}'DTM+137:20230101:102'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'LOC+172+{LOC}'UNT+8+1'UNZ+1+1'"
        ),
        21000..=21999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+IFTSTA:D:18A:UN:2.1'BGM+Z03+000{pid}'DTM+137:20230101:102'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'LOC+172+{LOC}'STS+Z21+Z05'UNT+9+1'UNZ+1+1'"
        ),
        29000..=29999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+250101:0000+1'\
UNH+1+COMDIS:D:17A:UN:1.0g'BGM+739+ABL{pid}'RFF+Z13:{pid}'DTM+137:20250101:102'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'AJT+Z01'UNT+8+1'UNZ+1+1'"
        ),
        33000..=33999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+250101:0000+1'\
UNH+1+REMADV:D:05A:UN:2.9f'BGM+239+000{pid}'DTM+137:20250101:102'RFF+Z13:REF'\
NAD+MS+4012345000023::293'CUX+2:EUR:9'MOA+9:100.00:EUR'UNS+D'MOA+9:100.00:EUR'AJT+Z01'UNT+11+1'UNZ+1+1'"
        ),
        39000..=39999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+ORDCHG:D:20B:UN:1.1'BGM+Z51+000{pid}'DTM+137:20230101:102'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'UNS+D'UNT+8+1'UNZ+1+1'"
        ),
        _ => return None,
    };
    Some(msg)
}

/// `true` when a `Skipped` reason means **nothing handled the message**.
///
/// Two distinct shapes drop a message silently, and both must count:
///
/// - `pid_not_in_*` — the workflow has an arm, but its inner `match pid` has no
///   branch for this Prüfidentifikator.
/// - `phase2_dispatch_not_yet_implemented` / `workflow_not_in_dispatch_table` —
///   the *whole workflow* is a stub, so every one of its registered PIDs is
///   dropped at once. This is the wider gap of the two, and checking only the
///   first shape hid seven `wim-technik-aenderung` PIDs behind a single stub
///   arm.
///
/// Every other reason (`no_correlation_key`, `process_not_found`, `*_resumes_only`)
/// means the PID *did* reach its arm and the arm made a domain decision.
fn is_silent_drop(reason: &str) -> bool {
    reason.starts_with("pid_not_in_")
        || reason == "phase2_dispatch_not_yet_implemented"
        || reason == "workflow_not_in_dispatch_table"
}

async fn make_dispatcher() -> EdifactIngestDispatcher {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant = TenantId::from_party_id(OWN_MP);
    EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    )
}

/// Every registered `(pid, workflow)` pair, across every domain module.
fn registered_pairs() -> Vec<(u32, String)> {
    let modules: Vec<Box<dyn EngineModule>> = vec![
        Box::new(mako_gpke::GpkeModule),
        Box::new(mako_wim::WimModule),
        Box::new(mako_geli_gas::GeliGasModule),
        Box::new(mako_gabi_gas::GaBiGasModule),
        Box::new(mako_mabis::MabisModule),
        Box::new(mako_redispatch::RedispatchModule),
    ];
    let roles = DeploymentRoles::all();

    let mut pairs: Vec<(u32, String)> = Vec::new();
    for m in &modules {
        let mut router = PidRouter::new();
        m.register_pids_with_roles(&mut router, &roles);
        for pid in router.registered_pids() {
            if let Some(wf) = router.route(pid) {
                pairs.push((pid, wf.to_owned()));
            }
        }
        for (pid, _sparte, wf) in router.registered_commodity_entries() {
            pairs.push((pid, wf.to_owned()));
        }
    }
    pairs.sort();
    pairs.dedup();
    pairs
}

#[tokio::test]
async fn every_registered_pid_reaches_a_dispatch_arm() {
    let dispatcher = make_dispatcher().await;
    let pairs = registered_pairs();
    assert!(!pairs.is_empty(), "expected registered PIDs");

    let allowed: std::collections::HashSet<(u32, &str)> = SEND_ONLY_PIDS
        .iter()
        .map(|(pid, wf, _)| (*pid, *wf))
        .collect();

    let mut gaps: Vec<String> = Vec::new();
    let mut now_covered: Vec<String> = Vec::new();
    let mut exercised = 0usize;

    for (pid, wf) in pairs {
        let Some(edi) = fixture(pid).or_else(|| synthetic_reply(pid)) else {
            // No message available for this PID — coverage is reported by
            // `dispatch_coverage_is_not_silently_shrinking` below.
            continue;
        };
        let Ok(msg) = edi_energy::parse(edi.as_bytes()) else {
            // An unparseable fixture is a fixture problem, not a coverage gap.
            continue;
        };
        exercised += 1;

        let dropped = matches!(
            dispatcher.dispatch(&msg, &wf, pid).await,
            Ok(IngestOutcome::Skipped { reason, .. }) if is_silent_drop(reason)
        );

        match (dropped, allowed.contains(&(pid, wf.as_str()))) {
            (true, false) => gaps.push(format!("  PID {pid} → {wf}")),
            (false, true) => now_covered.push(format!("  PID {pid} → {wf}")),
            _ => {}
        }
    }

    // Ratchet: 318 registrations are exercised today. A drop means fixtures
    // moved or the lookup broke, which would hollow the guard out silently.
    assert!(
        exercised >= 310,
        "only {exercised} PIDs were exercised (expected >= 310) — fixture \
         lookup is probably broken, and the coverage assertion below would \
         then be verifying almost nothing"
    );

    assert!(
        now_covered.is_empty(),
        "these PIDs now reach a dispatch arm — remove them from SEND_ONLY_PIDS \
         so the list keeps shrinking:\n{}",
        now_covered.join("\n")
    );

    assert!(
        gaps.is_empty(),
        "these PIDs are registered in the router but never reach a dispatch \
         arm, so an inbound message carrying one is silently dropped:\n{}\n\n\
         Either add the dispatch arm, or — if mako only ever *sends* this PID — \
         add it to SEND_ONLY_PIDS with the role whose receiver is missing.",
        gaps.join("\n")
    );
}

/// Guards the guard: most *exercisable* registered PIDs must be exercised.
///
/// The coverage check above can only catch a gap for a PID it has a message
/// for. If fixtures were moved or renamed it would quietly verify almost
/// nothing, so pin the ratio.
///
/// The denominator counts only PIDs that **could** have a fixture. A PID with
/// no AHB profile entry cannot: `generate-fixtures` skips it, so there is
/// nothing to find. Counting those would let the AHB backlog dilute the ratio —
/// registering a batch of unprofiled PIDs would trip this guard even though not
/// a single fixture had been lost, which is the opposite of what it watches for.
#[tokio::test]
async fn dispatch_coverage_is_not_silently_shrinking() {
    let platform = edi_energy::Platform::with_all_profiles();
    // Asked across every shipped profile rather than via a PID→message-type
    // band map: the bands genuinely overlap (29xxx is both APERAK and COMDIS),
    // so only the profiles can answer whether a PID was ever imported.
    let has_profile = |pid: u32| {
        let Ok(p) = edi_energy::Pruefidentifikator::new(pid) else {
            return false;
        };
        platform
            .registry()
            .all_profiles()
            .iter()
            .any(|prof| prof.ahb_rule_pack(Some(p)).name() != "unknown-pid")
    };

    let pairs: Vec<_> = registered_pairs()
        .into_iter()
        .filter(|(pid, _)| has_profile(*pid))
        .collect();
    let with_message = pairs
        .iter()
        .filter(|(pid, _)| fixture(*pid).is_some() || synthetic_reply(*pid).is_some())
        .count();

    // If `has_profile` ever broke, the denominator would collapse and the
    // percentage below would pass vacuously (or divide by zero).
    assert!(
        pairs.len() >= 300,
        "only {} registered PIDs resolve to a shipped profile — the profile \
         lookup is broken, not the fixtures",
        pairs.len()
    );

    let pct = with_message * 100 / pairs.len();
    assert!(
        pct >= 75,
        "only {with_message}/{} profiled registered PIDs ({pct}%) have a message \
         to dispatch — the coverage guard is running near-empty. Add fixtures \
         under crates/edi-energy/tests/fixtures/<type>/valid/pid_<pid>.edi",
        pairs.len()
    );
}
