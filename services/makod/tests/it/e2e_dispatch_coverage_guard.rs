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
//! Every registration it cannot exercise is a Prüfidentifikator with no AHB profile entry — the
//! `KNOWN_PROFILE_GAPS` set from `e2e_ahb_rule_coverage_guard` — plus the
//! send-only entries below. That is one root cause with three downstream
//! effects: no profile entry means validation passes vacuously,
//! `generate-fixtures` skips the PID, and its dispatch arm goes unexercised
//! here. Closing the AHB coverage gap closes all three.
//!
//! The ratchet below therefore measures reach over the **profiled** subset: a
//! PID with no profile cannot have a fixture, so counting it would let
//! unprofiled PIDs dilute a guard that exists to detect fixture loss.
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
/// This list must only ever shrink, and [`SEND_ONLY_CEILING`] holds it there.
/// Implementing a receiver makes the PID reach an arm, and the test then fails
/// until the entry is removed; an entry whose `(pid, workflow)` the router does
/// not register fails `every_send_only_entry_is_a_registration` — so it cannot
/// go stale in either direction.
const SEND_ONLY_PIDS: &[(u32, &str, &str)] = &[
    // No MaBiS MSCONS PID is send-only any more.
    //
    // A participant that *files* a Summenzeitreihe also receives one — the
    // BIKO forwards it, and the BKV, NB and ÜNB each sit on the receiving end
    // of some series in Tabelle 1 — so filing 13003 / 13020 / 13023 as outbound
    // would drop every inbound version without a trace. 13010–13012 likewise:
    // the LF and the MSB receive the normierte Profile and may answer with an
    // ORDERS 17211 Reklamation, which is what `mabis-profile` exists for.
    // ── MMM Allokationsliste Anforderungen ───────────────────────────────────
    // ORDERS fv20260401 (AHB 1.1a Kap. 4.2.2, S. 28, NB an ÜNB) publishes it;
    // fv20261001 (AHB 1.1b, 01.10.2026) drops it — `RETIRED_PIDS` in xtask
    // names that release. It stays registered while a process created under
    // fv20260401 can still be open; then the registration and this entry go.
    // It is not implemented: the receiver is the ÜNB.
    (
        17114,
        "gpke-allokationsliste",
        "Anforderung bilanzierte Menge (NB → ÜNB) — dropped by ORDERS fv20261001, do not implement",
    ),
    // ── MaBiS Listenabgleich replies addressed to the ÜNB ────────────────────
    // The NB answers the ÜNB's Bilanzierungsgebietsclearingliste 55195 and
    // DZÜ-Liste 55223 with these (Anwendungsübersicht 4.0 lfd. Nr. 7340 / 8170,
    // NB an ÜNB). Their receiver is the ÜNB that distributed the list, and mako
    // deploys no ÜNB. 55066 / 55202 resume the NB's own list (`SendListe`).
    (
        55196,
        "mabis-listenabgleich",
        "Antwort auf Bilanzierungsgebietsclearingliste (NB → ÜNB) — receiver is a ÜNB",
    ),
    (
        55224,
        "mabis-listenabgleich",
        "Antwort auf DZÜ-Liste (NB → ÜNB) — receiver is a ÜNB",
    ),
    // ── MaBiS ZP-lifecycle Weiterleitungen ───────────────────────────────────
    // Prozessschritt 4 is addressed to the **BKV**: „Der BIKO leitet nur den
    // nicht abgelehnten MaBiS-ZP an den BKV … weiter" (BK6-24-174 Anlage 3
    // Kap. 10.4.2 / 17.3.3.1.2 Nr. 4). mako stands as the NB, which is neither
    // the BIKO that forwards nor the BKV that receives, so these codes have no
    // inbound leg here at all — they stay registered because the answering side
    // of this workflow *emits* them.
    //
    // The Antwort codes are not on this list: 55064, 55204, 55207, 55210,
    // 55213 and 55237 answer an Anfrage mako sent and resume its process.
    (
        55205,
        "mabis-zp-lifecycle",
        "Weiterleitung monatliche AAÜZ, BKV des LF — addressed to the BKV, not to this deployment",
    ),
    (
        55208,
        "mabis-zp-lifecycle",
        "Weiterleitung monatliche AAÜZ, BKV des LF (Deaktivierung) — addressed to the BKV, not to this deployment",
    ),
    (
        55211,
        "mabis-zp-lifecycle",
        "Weiterleitung monatliche AAÜZ, BKV des anfNB — addressed to the BKV, not to this deployment",
    ),
    (
        55214,
        "mabis-zp-lifecycle",
        "Weiterleitung monatliche AAÜZ, BKV des anfNB (Deaktivierung) — addressed to the BKV, not to this deployment",
    ),
];

/// The most entries [`SEND_ONLY_PIDS`] may hold. Lower it with every entry a
/// receiver removes; it never rises.
const SEND_ONLY_CEILING: usize = 20;

#[test]
fn send_only_pids_do_not_grow() {
    assert!(
        SEND_ONLY_PIDS.len() <= SEND_ONLY_CEILING,
        "SEND_ONLY_PIDS holds {} entries, above its ceiling of {SEND_ONLY_CEILING}. A PID \
         a counterparty sends mako is owed a receiver, not an entry here.",
        SEND_ONLY_PIDS.len()
    );
}

/// An entry names a `(pid, workflow)` the router registers. One that does not
/// is never exercised by the guard below and can only hide a regression.
#[test]
fn every_send_only_entry_is_a_registration() {
    let pairs = registered_pairs();
    let stale: Vec<String> = SEND_ONLY_PIDS
        .iter()
        .filter(|(pid, wf, _)| !pairs.iter().any(|(p, w)| p == pid && w == wf))
        .map(|(pid, wf, _)| format!("  PID {pid} → {wf}"))
        .collect();
    assert!(
        stale.is_empty(),
        "SEND_ONLY_PIDS names registrations the router does not make — delete them:\n{}",
        stale.join("\n")
    );
}

/// A conformant message carrying `pid`, addressed to this tenant: the newest
/// profile's own skeleton of the Anwendungsfall, so every registered
/// Prüfidentifikator has a message and the guard exercises every arm.
fn fixture(pid: u32) -> Option<String> {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).ok()?;
    let profile = edi_energy::ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())?;
    let af = profile.anwendungsfall(pid)?;
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: "4012345000023".to_owned(),
                receiver: OWN_MP.to_owned(),
            },
        )
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// A synthetic reply-family message, for PIDs with no fixture on disk.
///
/// Each carries a `LOC` and an `RFF+ON`/`Z13` so both the MaLo and the
/// order-reference correlation paths have something to read.
fn synthetic_reply(pid: u32) -> Option<String> {
    let msg = match pid {
        15000..=15999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:500+230101:0000+1'\
UNH+1+QUOTES:D:10A:UN:1.3c'BGM+310+000{pid}'DTM+137:202301010000?+00:303'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'LOC+172+{LOC}'UNT+8+1'UNZ+1+1'"
        ),
        19000..=19999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:500+230101:0000+1'\
UNH+1+ORDRSP:D:10A:UN:1.4c'BGM+7+000{pid}'DTM+137:202301010000?+00:303'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'LOC+172+{LOC}'UNT+8+1'UNZ+1+1'"
        ),
        21000..=21999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:500+230101:0000+1'\
UNH+1+IFTSTA:D:18A:UN:2.1'BGM+Z03+000{pid}'DTM+137:202301010000?+00:303'RFF+ON:REF'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'LOC+172+{LOC}'STS+Z21+Z05'UNT+9+1'UNZ+1+1'"
        ),
        29000..=29999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:500+250101:0000+1'\
UNH+1+COMDIS:D:17A:UN:1.0g'BGM+739+ABL{pid}'RFF+Z13:{pid}'DTM+137:202501010000?+00:303'\
NAD+MS+4012345000023::293'NAD+MR+9900357000004::293'AJT+Z01'UNT+8+1'UNZ+1+1'"
        ),
        33000..=33999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:500+250101:0000+1'\
UNH+1+REMADV:D:05A:UN:2.9f'BGM+239+000{pid}'DTM+137:202501010000?+00:303'RFF+Z13:REF'\
NAD+MS+4012345000023::293'CUX+2:EUR:9'MOA+9:100.00:EUR'UNS+D'MOA+9:100.00:EUR'AJT+Z01'UNT+11+1'UNZ+1+1'"
        ),
        39000..=39999 => format!(
            "UNB+UNOC:3+4012345000023:14+9900357000004:500+230101:0000+1'\
UNH+1+ORDCHG:D:20B:UN:1.1'BGM+Z51+000{pid}'DTM+137:202301010000?+00:303'RFF+ON:REF'\
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
/// Every other reason (`no_correlation_key`, `process_not_found`) means the PID
/// *did* reach its arm and the arm made a domain decision.
///
/// A reason naming the arm's own intent — "this PID only resumes" — is not one
/// of those: it describes what the arm would do, and an arm that returns a skip
/// did not do it. Such a reason has to carry the `pid_not_in_` prefix like any
/// other drop, or the guard is satisfied by the wording rather than by the
/// behaviour.
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
    // The one production list — see `makod::startup::production_modules`. Never
    // restate the stack here: a guard with its own copy silently stops seeing a
    // module the daemon registers.
    let modules: Vec<Box<dyn EngineModule>> = makod::startup::production_modules();
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
            dispatcher.dispatch(&msg, &wf, pid, time::OffsetDateTime::now_utc()).await,
            Ok(IngestOutcome::Skipped { reason, .. }) if is_silent_drop(reason)
        );

        match (dropped, allowed.contains(&(pid, wf.as_str()))) {
            (true, false) => gaps.push(format!("  PID {pid} → {wf}")),
            (false, true) => now_covered.push(format!("  PID {pid} → {wf}")),
            _ => {}
        }
    }

    // Ratchet on the registrations exercised. A drop means fixtures moved or
    // the lookup broke, which would hollow the guard out silently.
    assert!(
        exercised >= 440,
        "only {exercised} PIDs were exercised (expected >= 440) — fixture \
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
         Add the dispatch arm: a registered PID a counterparty sends is owed a \
         receiver.",
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
/// nothing to find. Counting those would let unprofiled PIDs dilute the ratio:
/// registering a batch of unprofiled PIDs would trip this guard with every
/// fixture still in place, which is the opposite of what it watches for.
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
            .any(|prof| prof.has_anwendungsfall(p))
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

/// The routed PIDs whose own profile skeleton an adapter refuses, each with
/// why. An adapter that requires a segment its column does not define refuses
/// every conformant message of that PID — 13003 required an `SG10 CCI` its
/// column lacks — and that shows up here as a new entry. The list only
/// shrinks: a fixed refusal fails the test until its entry goes.
const KNOWN_REFUSALS: &[(u32, &str, &str)] = &[
    // State, not shape: a version or a Deaktivierung names its series through
    // the MaBiS-ZP's activation, which a fresh store does not hold.
    (
        13003,
        "mabis-billing",
        "no recorded Aktivierung of the MaBiS-ZP",
    ),
    (
        13020,
        "mabis-billing",
        "no recorded Aktivierung of the MaBiS-ZP",
    ),
    (
        55063,
        "mabis-zp-lifecycle",
        "no recorded Aktivierung of the MaBiS-ZP",
    ),
    // Skeleton values, not adapter requirements.
    (
        21002,
        "mabis-billing",
        "an Abweisung resumes a settlement a fresh store does not hold",
    ),
    (
        21003,
        "mabis-billing",
        "skeleton STS+Z04 DE 4405 placeholder is no Datenstatus code",
    ),
    (
        21004,
        "mabis-billing",
        "skeleton STS+Z04 DE 4405 placeholder is no Datenstatus code",
    ),
    (
        55064,
        "mabis-zp-lifecycle",
        "skeleton STS code placeholder is no E_0009 code",
    ),
    (
        55204,
        "mabis-zp-lifecycle",
        "skeleton STS code placeholder is no E_0071 code",
    ),
    (
        55207,
        "mabis-zp-lifecycle",
        "skeleton STS code placeholder is no E_0072 code",
    ),
    (
        55210,
        "mabis-zp-lifecycle",
        "skeleton STS code placeholder is no E_0078 code",
    ),
    (
        55213,
        "mabis-zp-lifecycle",
        "skeleton STS code placeholder is no E_0079 code",
    ),
    (
        55237,
        "mabis-zp-lifecycle",
        "skeleton STS code placeholder is no E_0102 code",
    ),
    (
        29001,
        "gabi-gas-invoic",
        "the newest profile carrying 29001 is not COMDIS",
    ),
    (
        29001,
        "gpke-abrechnung",
        "the newest profile carrying 29001 is not COMDIS",
    ),
    (
        29001,
        "wim-invoic",
        "the newest profile carrying 29001 is not COMDIS",
    ),
    // No requester side: an answer with no open Anfrage to resume.
    (
        44138,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44142,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44151,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44152,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44157,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44163,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44167,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44181,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44182,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    // An answer to an Anfrage or Änderungsmeldung mako does not send.
    (
        44021,
        "geli-gas-supplier-change",
        "answers a 44020 mako does not send",
    ),
    (
        44145,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44146,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44149,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
    (
        44164,
        "geli-gas-stammdatenaenderung",
        "Stammdatenanfrage answer, no requester side",
    ),
];

/// Every routed PID's profile skeleton is accepted by its adapter, or its
/// refusal is a [`KNOWN_REFUSALS`] entry — in both directions.
#[tokio::test]
async fn every_conformant_skeleton_is_accepted_or_its_refusal_named() {
    let mut refused: Vec<(u32, String, String)> = Vec::new();
    for (pid, wf) in registered_pairs() {
        let Some(edi) = fixture(pid) else { continue };
        let Ok(msg) = edi_energy::parse(edi.as_bytes()) else {
            continue;
        };
        let dispatcher = make_dispatcher().await;
        if let Err(e) = dispatcher
            .dispatch(&msg, &wf, pid, time::OffsetDateTime::now_utc())
            .await
        {
            refused.push((pid, wf, e.to_string()));
        }
    }
    let known: std::collections::BTreeSet<(u32, &str)> =
        KNOWN_REFUSALS.iter().map(|(p, w, _)| (*p, *w)).collect();
    let neu: Vec<String> = refused
        .iter()
        .filter(|(p, w, _)| !known.contains(&(*p, w.as_str())))
        .map(|(p, w, e)| format!("  {p} → {w}: {e}"))
        .collect();
    assert!(
        neu.is_empty(),
        "an adapter refuses a conformant message — does it require a segment its \
         column does not define?\n{}",
        neu.join("\n")
    );
    // A state refusal must name the missing activation — not a segment the
    // column lacks, which is what 13003's `SG10 CCI` requirement looked like.
    for (pid, wf, e) in &refused {
        let Some((_, _, grund)) = KNOWN_REFUSALS.iter().find(|(p, w, _)| p == pid && w == wf)
        else {
            continue;
        };
        if grund.starts_with("no recorded Aktivierung") {
            assert!(
                e.contains("Aktivierung"),
                "{pid} → {wf} is refused for something other than its activation: {e}"
            );
        }
    }
    let behoben: Vec<String> = KNOWN_REFUSALS
        .iter()
        .filter(|(p, w, _)| !refused.iter().any(|(rp, rw, _)| rp == p && rw == w))
        .map(|(p, w, _)| format!("  {p} → {w}"))
        .collect();
    assert!(
        behoben.is_empty(),
        "these skeletons are accepted now — remove their KNOWN_REFUSALS entry:\n{}",
        behoben.join("\n")
    );
}
