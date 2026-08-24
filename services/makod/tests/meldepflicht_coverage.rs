//! Guard: the Lieferbeginn notification obligations stay visible until they ship.
//!
//! A **Meldepflicht** ([`mako_fristen::meldung`]) is a message a Festlegung
//! obliges a party to send with no answer expected back — so a missing one
//! produces no timeout, no dead letter and no alert, and surfaces months later
//! as a counterparty holding a stale view of who supplies a Marktlokation.
//!
//! Six sit around the Lieferbeginn, three per Sparte, and mako sends none.
//! The catalogue states what is owed and by when; this states what is not built,
//! so the two cannot drift. See `ROADMAP.md` for the work itself.

use std::collections::{BTreeMap, BTreeSet};

use mako_engine::builder::EngineModule;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;
use mako_fristen::meldung;

/// Catalogued Meldepflichten mako cannot yet send, and why.
///
/// All six are blocked on the same thing: `edi-energy` carries no UTILMD AHB
/// rules for the Prüfidentifikator, so there is nothing to render or validate
/// against. **No deadline is registered for any of them** — a deadline on an
/// unrenderable message can only ever fire, which is what `deadline_labels.rs`
/// exists to prevent.
const NOT_YET_SENDABLE: &[(u32, &str)] = &[
    (
        55_036,
        "no UTILMD AHB Strom profile rules; NB→LFN Identität des LFA",
    ),
    (
        55_037,
        "no UTILMD AHB Strom profile rules; NB→LFA Zuordnungsende",
    ),
    (
        55_038,
        "no UTILMD AHB Strom profile rules; NB→LFZ Aufhebung",
    ),
    (
        44_036,
        "no UTILMD AHB Gas profile rules; NB→LFN Identität des LFA",
    ),
    (
        44_037,
        "no UTILMD AHB Gas profile rules; NB→LFA Zuordnungsende",
    ),
    (44_038, "no UTILMD AHB Gas profile rules; NB→LFZ Aufhebung"),
];

fn router() -> PidRouter {
    let mut router = PidRouter::new();
    let roles = DeploymentRoles::all();
    for module in modules() {
        module.register_pids_with_roles(&mut router, &roles);
    }
    router
}

fn modules() -> Vec<Box<dyn EngineModule>> {
    vec![
        Box::new(mako_gpke::GpkeModule),
        Box::new(mako_geli_gas::GeliGasModule),
    ]
}

/// Every catalogued Meldepflicht is either sendable or named as a known gap.
#[test]
fn every_meldepflicht_is_implemented_or_declared_missing() {
    let declared: BTreeMap<u32, &str> = NOT_YET_SENDABLE.iter().copied().collect();
    let mut undeclared = Vec::new();
    for m in meldung::all() {
        if !declared.contains_key(&m.pid) {
            undeclared.push(format!(
                "{} ({}, {} → {})",
                m.pid, m.name, m.sent_by, m.sent_to
            ));
        }
    }
    assert!(
        undeclared.is_empty(),
        "these Meldepflichten are catalogued but neither implemented nor declared \
         missing in NOT_YET_SENDABLE:\n  {}",
        undeclared.join("\n  ")
    );
}

/// Nothing is declared missing that is no longer missing.
///
/// The router is the operational answer to „does mako handle this PID". A
/// declared-missing Meldepflicht that turns up routed means the entry is stale
/// and the gap statement in `ROADMAP.md` is now wrong.
#[test]
fn no_declared_gap_is_actually_routed() {
    let router = router();
    let stale: Vec<u32> = NOT_YET_SENDABLE
        .iter()
        .map(|(pid, _)| *pid)
        .filter(|pid| router.route(*pid).is_some())
        .collect();
    assert!(
        stale.is_empty(),
        "these PIDs are declared missing but the router registers them — delete \
         their NOT_YET_SENDABLE entries and update ROADMAP.md: {stale:?}"
    );
}

/// Every declared gap is still catalogued — no entry for an obligation that
/// `mako_fristen::meldung` no longer states.
#[test]
fn no_declared_gap_has_lost_its_catalogue_entry() {
    let catalogued: BTreeSet<u32> = meldung::all().map(|m| m.pid).collect();
    let orphaned: Vec<u32> = NOT_YET_SENDABLE
        .iter()
        .map(|(pid, _)| *pid)
        .filter(|pid| !catalogued.contains(pid))
        .collect();
    assert!(
        orphaned.is_empty(),
        "declared missing but no longer catalogued in mako_fristen::meldung: {orphaned:?}"
    );
}

/// Each gap entry says *why*, not just *that*.
#[test]
fn every_declared_gap_states_a_reason() {
    for (pid, reason) in NOT_YET_SENDABLE {
        assert!(
            reason.len() > 20,
            "PID {pid} needs a reason a reader can act on, got {reason:?}"
        );
    }
}
