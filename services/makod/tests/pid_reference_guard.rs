//! Guard: the published PID reference must not claim a PID that mako never routes.
//!
//! `site/content/docs/regulatory/pid-reference.md` names a crate and workflow in
//! its last column for each Prüfidentifikator it considers implemented. An
//! operator reads that as "an inbound message with this PID is handled".
//!
//! The column cannot distinguish a PID mako *receives* from one it only
//! *sends*, so an outbound-only response PID and a genuinely unimplemented one
//! read the same. A message carrying an unrouted PID is dead-lettered as
//! `UnknownPid` — audited and alertable — while the documentation says it is
//! handled.
//!
//! This test pins the two sets together. Every PID the reference credits to a
//! workflow must either be routed, or appear in [`NOT_ROUTED_BY_DESIGN`] with a
//! stated reason. Adding a row to the table without one of those fails here.
//!
//! Uses only committed sources: the markdown file and the compiled modules.

use std::collections::{BTreeMap, BTreeSet};

use mako_engine::builder::EngineModule;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;

/// PIDs the reference credits to a workflow that the `PidRouter` deliberately
/// does not register.
///
/// Two distinct reasons, and the difference matters — the first group is
/// correct as-is, the second is a real gap:
///
/// * **Outbound-only.** mako *generates* the PID as a response; it never arrives
///   inbound, so there is nothing to route.
/// * **Not yet wired.** The constants exist in the domain crate but no module
///   registers them, or the PID falls outside the band its named workflow
///   actually covers.
const NOT_ROUTED_BY_DESIGN: &[(u32, &str)] = &[
    // The MSBA's ORDRSP answering a Weiterverpflichtung (`E_0203`). mako holds
    // the MSBA side; the NB side — sending 17002 and awaiting the answer — is
    // not implemented, so these never arrive inbound.
    (19_003, "outbound-only: MSBA response to ORDERS 17002"),
    (19_004, "outbound-only: MSBA response to ORDERS 17002"),
    // „Ablehnung Anforderung von Werten" answers a Werteanforderung
    // (ORDRSP AHB 1.1b § 4.13); no workflow covers that leg.
    (
        19_007,
        "Werteanforderung answer; no workflow covers that leg",
    ),
    // ── Not yet wired: constants exist, nothing registers them.
    (17_134, "konfiguration::ORDERS_PIDS exists; not registered"),
    (17_135, "konfiguration::ORDERS_PIDS exists; not registered"),
    // **44170 does not exist under FV2026-10-01.** PID-Übersicht 4.0 publishes
    // the Gas Verpflichtungsanfrage as 44168 → 44169 and no Ablehnungs-PID; the
    // 44170 of PID 3.3 was withdrawn. `E_2006` still publishes the Ablehnungs-
    // Codeliste `G_0071`, so the codes exist with no carrier — mako escalates
    // rather than emitting a Prüfidentifikator the market rejects.
    (
        44_170,
        "withdrawn with FV2026-10-01; no Ablehnungs-PID exists for 44168",
    ),
    // ── Credited to `gpke-stammdatenaenderung`, but outside STAMMDATEN_PAIRS
    //    (55615–55694, 55109/55110). These are GDA-Antwort and individual-order
    //    processes, not Stammdatenänderung.
    (55_035, "GDA-Antwort; outside STAMMDATEN_PAIRS"),
    (55_060, "GDA-Antwort; outside STAMMDATEN_PAIRS"),
    (55_095, "GDA-Antwort; outside STAMMDATEN_PAIRS"),
    (55_173, "outside STAMMDATEN_PAIRS"),
    (55_175, "outside STAMMDATEN_PAIRS"),
    (55_177, "outside STAMMDATEN_PAIRS"),
    (55_180, "outside STAMMDATEN_PAIRS"),
    (55_194, "outside STAMMDATEN_PAIRS"),
    (55_225, "outside STAMMDATEN_PAIRS"),
    (55_227, "outside STAMMDATEN_PAIRS"),
    (55_553, "individuelle Bestellung; outside STAMMDATEN_PAIRS"),
    // The one PID the Anwendungsübersicht credits to *AWH NBW* alone. It is
    // imported, profiled and validates — and `makod` registers no NBW module
    // at all (it has no dependency on `mako-nbw`, which is a domain crate
    // with no PID family of its own). Note this is NOT the 55691/55692 case:
    // those ride `gpke-stammdatenaenderung` and minting an NBW family would
    // double-register them. Nothing else claims 55690.
    (55_690, "AWH NBW; makod registers no NBW module"),
];

/// Every PID the reference table credits to a crate/workflow.
fn pids_claimed_by_the_reference() -> BTreeSet<u32> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../site/content/docs/regulatory/pid-reference.md"
    );
    let src = std::fs::read_to_string(path).expect("pid-reference.md is readable");

    let mut claimed = BTreeSet::new();
    for line in src.lines() {
        if !line.starts_with("| ") {
            continue;
        }
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() < 10 {
            continue;
        }
        let Ok(pid) = cells[0].parse::<u32>() else {
            continue;
        };
        // A backtick in the last column means a crate/workflow is named.
        if cells[cells.len() - 1].contains('`') {
            claimed.insert(pid);
        }
    }
    claimed
}

/// Every PID the compiled modules register, with all Marktrollen active.
fn routed_pids() -> BTreeMap<u32, String> {
    // The one production list — see `makod::startup::production_modules`. Never
    // restate the stack here: a guard with its own copy silently stops seeing a
    // module the daemon registers.
    let modules: Vec<Box<dyn EngineModule>> = makod::startup::production_modules();
    let roles = DeploymentRoles::all();

    let mut routed = BTreeMap::new();
    for m in &modules {
        // One router per module: the deliberate cross-module PID overlaps would
        // otherwise collide.
        let mut router = PidRouter::new();
        m.register_pids_with_roles(&mut router, &roles);
        for pid in router.registered_pids() {
            if let Some(wf) = router.route(pid) {
                routed.entry(pid).or_insert_with(|| wf.to_owned());
            }
        }
        for (pid, _sparte, wf) in router.registered_commodity_entries() {
            routed.entry(pid).or_insert_with(|| wf.to_owned());
        }
    }
    routed
}

#[test]
fn the_reference_does_not_credit_a_workflow_with_a_pid_it_never_receives() {
    let claimed = pids_claimed_by_the_reference();
    let routed = routed_pids();
    let exempt: BTreeMap<u32, &str> = NOT_ROUTED_BY_DESIGN.iter().copied().collect();

    assert!(
        claimed.len() > 300,
        "reference parse yielded only {}",
        claimed.len()
    );
    assert!(
        routed.len() > 300,
        "module registration yielded only {}",
        routed.len()
    );

    let unexplained: Vec<u32> = claimed
        .iter()
        .copied()
        .filter(|pid| !routed.contains_key(pid) && !exempt.contains_key(pid))
        .collect();

    assert!(
        unexplained.is_empty(),
        "pid-reference.md credits a crate/workflow to {} PID(s) that no module routes:\n  {:?}\n\
         An inbound message with one of these is dead-lettered as UnknownPid, so the table \
         promises handling that does not exist.\n\
         Either register the PID, or add it to NOT_ROUTED_BY_DESIGN with the reason \
         (outbound-only vs not-yet-wired).",
        unexplained.len(),
        unexplained
    );
}

/// The exemption list must not rot: an entry that becomes routed should be
/// removed, or it hides the fact that the gap was closed.
#[test]
fn no_exemption_is_stale() {
    let routed = routed_pids();
    let now_routed: Vec<u32> = NOT_ROUTED_BY_DESIGN
        .iter()
        .map(|(pid, _)| *pid)
        .filter(|pid| routed.contains_key(pid))
        .collect();

    assert!(
        now_routed.is_empty(),
        "NOT_ROUTED_BY_DESIGN lists {:?}, but these are routed now — delete the entries",
        now_routed
    );
}

/// The workflow the reference names must be the workflow that routes the PID.
///
/// The check above asks only whether a PID is routed *at all*, so a row naming
/// any workflow would pass it. An operator reads the column to know which
/// process to look at when a message arrives, so a wrong name sends them to the
/// wrong event stream — `gpke-supplier-change` for a Kündigung that
/// `gpke-kuendigung` handles, say.
///
/// A row may name several workflows: a few PIDs are deliberately claimed by
/// more than one family (the shared REMADV/COMDIS replies), and the engine
/// resolves one owner. The routed owner must be among those named.
#[test]
fn the_reference_names_the_workflow_that_actually_routes_each_pid() {
    use mako_engine::builder::EngineBuilder;
    use mako_engine::event_store::InMemoryEventStore;
    use std::sync::Arc;

    // Resolve through a real engine build, not per-module routers: cross-module
    // PID overlaps have an ownership rule, and a guard with its own resolution
    // would disagree with the daemon.
    let mut builder = EngineBuilder::new().with_event_store(Arc::new(InMemoryEventStore::new()));
    for module in makod::startup::production_modules() {
        builder = builder.register(module);
    }
    let ctx = builder.build();

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../site/content/docs/regulatory/pid-reference.md"
    );
    let src = std::fs::read_to_string(path).expect("pid-reference.md is readable");

    let mut wrong: Vec<String> = Vec::new();
    for line in src.lines() {
        if !line.starts_with("| ") {
            continue;
        }
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() < 10 {
            continue;
        }
        let Ok(pid) = cells[0].parse::<u32>() else {
            continue;
        };
        let last = cells[cells.len() - 1];
        // Workflow names are the backticked segments containing a hyphen that
        // are not crate names (`mako-gpke`, …).
        let named: Vec<&str> = last
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|t| !t.starts_with("mako-") && t.contains('-'))
            .collect();
        if named.is_empty() {
            continue;
        }
        let Some(actual) = ctx.pid_router().route(pid) else {
            continue; // the routed/claimed check above owns this case
        };
        if !named.contains(&actual) {
            wrong.push(format!(
                "  {pid}: reference names {named:?}, router says `{actual}`"
            ));
        }
    }

    assert!(
        wrong.is_empty(),
        "these pid-reference rows credit a workflow that does not route the PID:\n{}\n\
         Correct the last column to the workflow the router resolves.",
        wrong.join("\n"),
    );
}

/// Rows the reference names no workflow for **and** that do not yet say why.
///
/// The workflow column of an unrouted PID should carry its reason inline, the
/// way `19301` does — `— (Herkunftsnachweisregister, not implemented)` — so a
/// reader meets an explanation rather than a silence they have to read as
/// either "deliberate" or "someone forgot".
///
/// [`NOT_ROUTED_BY_DESIGN`] cannot cover these: it is checked against the PIDs
/// the reference *credits* to a workflow, and a row naming none is never in
/// that set. So an unrouted PID could be added to the table with a bare `—` and
/// no guard would see it.
///
/// This list is that blind spot, made explicit. It is a **backlog, not a
/// blessing** — every entry wants a one-line reason in the table, after which
/// it is deleted from here. The guard below refuses a *new* bare `—` row, so
/// the list can only shrink.
const UNCLAIMED_WITHOUT_A_REASON: &[u32] = &[
    55_008, 55_009, 55_074, 55_075, 55_076, 55_126, 55_218, 55_602, 55_603, 55_604, 55_605, 55_608,
    55_609, 55_613, 55_614, 55_672, 55_674, 55_675, 44_035, 44_060, 44_101, 44_102, 44_103, 44_104,
    44_105, 44_183, 17_101, 17_126, 17_301, 19_007, 21_040, 13_028, 29_002,
];

/// A row that names no workflow must say why — or be a known one that does not.
///
/// Adding an unrouted PID to the reference with a bare `—` fails here. Writing
/// its reason into the table fails here too, until the entry is removed from
/// [`UNCLAIMED_WITHOUT_A_REASON`] — which is the point: the list is meant to
/// empty out.
#[test]
fn an_unclaimed_row_states_its_reason() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../site/content/docs/regulatory/pid-reference.md"
    );
    let src = std::fs::read_to_string(path).expect("pid-reference.md is readable");

    let mut bare = BTreeSet::new();
    for line in src.lines() {
        if !line.starts_with("| ") {
            continue;
        }
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() < 10 {
            continue;
        }
        let Ok(pid) = cells[0].parse::<u32>() else {
            continue;
        };
        let workflow = cells[cells.len() - 1];
        // A backtick means a workflow is named; that row is the other guard's.
        if workflow.contains('`') {
            continue;
        }
        if workflow == "—" {
            bare.insert(pid);
        }
    }

    assert!(
        !bare.is_empty(),
        "no unclaimed rows found at all — this guard has stopped parsing the table"
    );

    let known: BTreeSet<u32> = UNCLAIMED_WITHOUT_A_REASON.iter().copied().collect();

    let undocumented: Vec<u32> = bare.difference(&known).copied().collect();
    assert!(
        undocumented.is_empty(),
        "these PIDs name no workflow and give no reason: {undocumented:?} — \
         either state why in the table's last column (e.g. \"— (not implemented)\") \
         or add them to UNCLAIMED_WITHOUT_A_REASON"
    );

    let now_explained: Vec<u32> = known.difference(&bare).copied().collect();
    assert!(
        now_explained.is_empty(),
        "UNCLAIMED_WITHOUT_A_REASON lists {now_explained:?}, but the table now \
         explains or routes them — delete the entries"
    );
}
