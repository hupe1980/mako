//! Guard: every workflow the router routes to has a command that moves it.
//!
//! A routed workflow spawns a process from an inbound message. If no command
//! names that workflow, nothing outside the ingest door can ever move the
//! process on: it looks healthy while its Frist runs out. A source scan cannot
//! decide this — the dispatch functions name workflows through constants
//! re-exported under aliases — so each [`CommandDescriptor`] declares its
//! `workflow`, and this guard takes the set difference against the router.
//!
//! A workflow with no command passes only on [`RECORD_ONLY`] (nothing is owed
//! back) or [`OHNE_KOMMANDO`] (something is owed and no command sends it yet),
//! each with its reason. Both lists are refused when stale: an entry whose
//! workflow gained a command, or is no longer routed.

use std::collections::BTreeSet;

use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;

use super::registry::COMMAND_REGISTRY;
use super::types::CommandDescriptor;

/// Routed workflows that owe no answer a command could send, and why.
const RECORD_ONLY: &[(&str, &str)] = &[
    (
        "gabi-gas-allocation",
        "an ALOCAT Allokation is recorded; the receiver answers it with no business message",
    ),
    (
        "geli-gas-mscons",
        "Gas Messwerte are recorded; the receiver owes no business answer",
    ),
    (
        "geli-gas-partin",
        "PARTIN Kommunikationsdaten are recorded; PARTIN carries no business answer",
    ),
    (
        "gpke-partin",
        "PARTIN Kommunikationsdaten are recorded; PARTIN carries no business answer",
    ),
    (
        "gpke-messwerte",
        "Strom Messwerte are recorded; the receiver owes no business answer",
    ),
    (
        "gpke-utilts",
        "UTILTS Konfigurationsdaten are recorded; the receiver owes no business answer",
    ),
    (
        "mabis-clearingliste",
        "55067 / 55069 / 55070 / 55073 are distributed and nothing is owed back",
    ),
    (
        "wim-preisliste",
        "a PRICAT Preisblatt is recorded; the receiver owes no business answer",
    ),
];

/// Routed workflows that owe an answer no command sends yet — a burn-down
/// list: it only shrinks.
const OHNE_KOMMANDO: &[(&str, &str)] = &[
    (
        "emob-abmeldung",
        "`ModellwechselCommand::SendAntwort` has no door",
    ),
    (
        "emob-anmeldung",
        "`ModellwechselCommand::SendAntwort` has no door",
    ),
    (
        "emob-zuordnungsende",
        "`ModellwechselCommand::SendAntwort` has no door",
    ),
    (
        "geli-gas-sperrung-lf",
        "the LF's Gas Sperrauftrag and its Stornierung have no door",
    ),
    (
        "geli-gas-sperrung-nb",
        "the GNB's `BestaetigueSperrung` has no door",
    ),
    (
        "gpke-allokationsliste",
        "the LF's Anforderung 17110 (`SendAnforderung`) has no door",
    ),
    (
        "gpke-anfrage-bestellung",
        "the MSB's Bearbeitungsstand (`DispatchResponse`) has no door",
    ),
    (
        "gpke-stammdatenaenderung",
        "an A02 Rückmeldung and the Verantwortlicher's own Änderung have no door",
    ),
    (
        "mabis-anforderung",
        "the Anforderung (`SendAnforderung`) has no door",
    ),
    (
        "mabis-profile",
        "the Reklamation 17211 out of `E_0100` (`SendReklamation`) has no door",
    ),
    (
        "mabis-zp-lifecycle",
        "the MaBiS-ZP Anfrage and Antwort (`SendAnfrage` / `SendAntwort`) have no door",
    ),
    (
        "redispatch-aktivierung",
        "the ACR / AAR answers and the Kaskade to the ANB have no door",
    ),
    (
        "wim-geraeteubernahme",
        "the MSBA's ORDRSP 19015 / 19016 (`DispatchAntwort`) has no door",
    ),
];

/// Every workflow the production modules route a PID to, under every role.
fn routed_workflows() -> BTreeSet<String> {
    let roles = DeploymentRoles::all();
    let mut routed = BTreeSet::new();
    for module in crate::startup::production_modules() {
        // One router per module: the deliberate cross-module PID overlaps would
        // otherwise collide.
        let mut router = PidRouter::new();
        module.register_pids_with_roles(&mut router, &roles);
        for pid in router.registered_pids() {
            if let Some(wf) = router.route(pid) {
                routed.insert(wf.to_owned());
            }
        }
        for (_, _, wf) in router.registered_commodity_entries() {
            routed.insert(wf.to_owned());
        }
    }
    routed
}

/// The routed workflows no command in `registry` names and no exemption
/// covers.
fn ohne_tuer(
    routed: &BTreeSet<String>,
    registry: &[CommandDescriptor],
    exempt: &[&[(&str, &str)]],
) -> Vec<String> {
    let commanded: BTreeSet<&str> = registry.iter().filter_map(|c| c.workflow).collect();
    routed
        .iter()
        .filter(|wf| !commanded.contains(wf.as_str()))
        .filter(|wf| !exempt.iter().any(|l| l.iter().any(|(w, _)| w == wf)))
        .cloned()
        .collect()
}

#[test]
fn every_routed_workflow_has_a_command() {
    let routed = routed_workflows();
    assert!(routed.len() > 50, "only {} routed workflows", routed.len());
    let gaps = ohne_tuer(&routed, COMMAND_REGISTRY, &[RECORD_ONLY, OHNE_KOMMANDO]);
    assert!(
        gaps.is_empty(),
        "these workflows route inbound messages and no command moves their \
         process — add the command, or name the workflow in RECORD_ONLY with \
         why nothing is owed back:\n  {}",
        gaps.join("\n  ")
    );
}

#[test]
fn no_exemption_is_stale() {
    let routed = routed_workflows();
    let commanded: BTreeSet<&str> = COMMAND_REGISTRY.iter().filter_map(|c| c.workflow).collect();
    for (wf, _) in RECORD_ONLY.iter().chain(OHNE_KOMMANDO) {
        assert!(routed.contains(*wf), "{wf} is exempt but not routed");
        assert!(
            !commanded.contains(wf),
            "{wf} is exempt but a command names it — delete the entry"
        );
    }
}

#[test]
fn every_declared_workflow_exists() {
    let drivers: BTreeSet<&str> = crate::startup::production_modules()
        .iter()
        .flat_map(|m| m.drivers())
        .map(|d| d.name())
        .collect();
    for c in COMMAND_REGISTRY {
        if let Some(wf) = c.workflow {
            assert!(
                drivers.contains(wf),
                "{} declares workflow {wf}, which no module registers",
                c.name
            );
        }
    }
}

/// The guard fails on a workflow whose only command is removed.
#[test]
fn removing_a_workflows_only_command_fails_the_guard() {
    let routed = routed_workflows();
    let ohne: Vec<CommandDescriptor> = COMMAND_REGISTRY
        .iter()
        .filter(|c| c.workflow != Some("gpke-neuanlage"))
        .cloned()
        .collect();
    assert_eq!(
        ohne_tuer(&routed, &ohne, &[RECORD_ONLY, OHNE_KOMMANDO]),
        vec!["gpke-neuanlage".to_owned()]
    );
}
