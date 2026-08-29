//! Guard: every catalogued Meldepflicht is routed, or declared missing with a
//! reason.
//!
//! A **Meldepflicht** ([`mako_fristen::meldung`]) is a message a Festlegung
//! obliges a party to send with no answer expected back — so a missing one
//! produces no timeout, no dead letter and no alert, and surfaces months later
//! as a counterparty holding a stale view of who supplies a Marktlokation.
//! Nothing in the ordinary machinery notices; this file is what does.
//!
//! Six sit around the Lieferbeginn, three per Sparte, and all six now route:
//! `gpke-zuordnungsmeldung` (55036/55037/55038) and
//! `geli-gas-zuordnungsmeldung` (44036/44037/44038). The catalogue states what
//! is owed and by when; this states what mako can actually send, so the two
//! cannot drift.

use std::collections::{BTreeMap, BTreeSet};

use mako_engine::builder::EngineModule;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;
use mako_fristen::meldung;

/// Catalogued Meldepflichten mako cannot yet send, and why.
///
/// Empty is the goal, not the norm: entries land here as the catalogue grows
/// ahead of the workflows. **No deadline is registered for anything listed
/// here** — a deadline on an unrenderable message can only ever fire, which is
/// what `deadline_labels.rs` exists to prevent.
const NOT_YET_SENDABLE: &[(u32, &str)] = &[];

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

/// Every catalogued Meldepflicht is either routed or named as a known gap.
#[test]
fn every_meldepflicht_is_routed_or_declared_missing() {
    let declared: BTreeMap<u32, &str> = NOT_YET_SENDABLE.iter().copied().collect();
    let router = router();
    let mut unrouted = Vec::new();
    for m in meldung::all() {
        if router.route(m.pid).is_none() && !declared.contains_key(&m.pid) {
            unrouted.push(format!(
                "{} ({}, {} → {})",
                m.pid, m.name, m.sent_by, m.sent_to
            ));
        }
    }
    assert!(
        unrouted.is_empty(),
        "these Meldepflichten are catalogued but neither routed nor declared \
         missing in NOT_YET_SENDABLE:\n  {}",
        unrouted.join("\n  ")
    );
}

/// Each Sparte's three Meldungen route to that Sparte's own workflow.
///
/// Not cosmetic: the two differ on the wire — `BGM+E01`/`E02` against `E44`,
/// `LOC+Z16`/`Z21` against `LOC+172`, and three Gründe Gas does not define
/// (`ZD9`, `ZG5`, `ZG6`). A Gas Meldung answered by the Strom workflow would be
/// rendered against the wrong AHB throughout.
#[test]
fn each_sparte_routes_to_its_own_zuordnungsmeldung_workflow() {
    let router = router();
    for (pids, expected) in [
        (
            mako_gpke::ZUORDNUNGSMELDUNG_PIDS,
            mako_gpke::ZUORDNUNGSMELDUNG_WORKFLOW_NAME,
        ),
        (
            mako_geli_gas::GAS_ZUORDNUNGSMELDUNG_PIDS,
            mako_geli_gas::GAS_ZUORDNUNGSMELDUNG_WORKFLOW_NAME,
        ),
    ] {
        for &pid in pids {
            assert_eq!(
                router.route(pid),
                Some(expected),
                "PID {pid} must route to {expected}"
            );
        }
    }
}

/// Nothing is declared missing that is no longer missing.
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
         their NOT_YET_SENDABLE entries: {stale:?}"
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
