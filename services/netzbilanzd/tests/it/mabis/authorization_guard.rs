//! Every Cedar action the MaBiS handlers check is one the policy can permit,
//! and each grants exactly the power its route needs.
//!
//! # Why this is a test
//!
//! Cedar is deny-by-default, so an action a handler names and the policy file
//! does not mention is a **permanent 403**: no token, role or configuration
//! change lifts it, and the response says only "forbidden". Nothing fails to
//! compile, because the action is a string on one side and a policy document on
//! the other.
//!
//! The second half matters as much. `POST /api/v1/mabis/datenstatus` and
//! `POST /api/v1/mabis/pruefmitteilung` only *record* what the BIKO said, so
//! they must not sit behind `trigger-mabis-run` — that would give the ingest
//! identity the power to file a binding Summenzeitreihe in the tenant's name.
//! An over-broad grant is not a 403, and only a per-route check sees it.

use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

const POLICY: &str = include_str!("../../../policies/netzbilanzd.cedar");
const SERVER: &str = include_str!("../../../src/mabis/server.rs");

const TENANT: &str = "9900357000004";

/// Whether `action` is one of the MaBiS module's.
fn is_mabis(action: &str) -> bool {
    action.ends_with("-mabis-run") || action == "record-biko-response"
}

/// MaBiS actions named in a `permit` head, ignoring comment lines.
fn permitted_actions() -> Vec<String> {
    let mut out = Vec::new();
    for line in POLICY.lines() {
        let line = line.trim();
        if line.starts_with("//") {
            continue;
        }
        let mut rest = line;
        while let Some(i) = rest.find("Action::\"") {
            rest = &rest[i + "Action::\"".len()..];
            if let Some(end) = rest.find('"') {
                out.push(rest[..end].to_owned());
                rest = &rest[end + 1..];
            } else {
                break;
            }
        }
    }
    out.retain(|a| is_mabis(a));
    out.sort();
    out.dedup();
    out
}

/// Actions passed to `authorize(…)` in the MaBiS handlers.
fn checked_actions() -> Vec<String> {
    let needle = "authorize(&cedar, &claims, \"";
    let mut out = Vec::new();
    let mut rest = SERVER;
    while let Some(i) = rest.find(needle) {
        rest = &rest[i + needle.len()..];
        let Some(end) = rest.find('"') else { break };
        out.push(rest[..end].to_owned());
    }
    out.sort();
    out.dedup();
    out
}

/// The parsers must find something — otherwise every assertion is vacuous.
#[test]
fn the_parses_are_not_vacuous() {
    assert_eq!(
        permitted_actions().len(),
        3,
        "policy actions: {:?}",
        permitted_actions()
    );
    assert_eq!(
        checked_actions().len(),
        3,
        "checked actions: {:?}",
        checked_actions()
    );
}

/// Every action a handler checks is one the policy permits, and the policy
/// grants no MaBiS action no handler checks — a stale `permit` reads as
/// coverage that is not there.
#[test]
fn the_checked_and_the_permitted_actions_are_the_same_set() {
    assert_eq!(
        checked_actions(),
        permitted_actions(),
        "a handler checking an action policies/netzbilanzd.cedar never permits is a \
         permanent 403; a permit nothing checks is a dead grant"
    );
}

/// Recording an inbound BIKO response is not the power to file one.
///
/// The two ingest routes must not sit behind `trigger-mabis-run`: that action
/// is what sends a Summenzeitreihe the BIKO cannot un-settle, and relaying an
/// IFTSTA needs none of it.
#[test]
fn recording_a_biko_response_does_not_require_the_power_to_file_one() {
    for route in ["post_datenstatus", "post_pruefmitteilung"] {
        let start = SERVER
            .find(&format!("pub async fn {route}("))
            .unwrap_or_else(|| panic!("{route} exists"));
        let body = &SERVER[start..start + 600];
        assert!(
            body.contains("\"record-biko-response\""),
            "{route} must authorise as record-biko-response"
        );
        assert!(
            !body.contains("\"trigger-mabis-run\""),
            "{route} only records what arrived; it must not require the power to file a \
             binding Summenzeitreihe"
        );
    }
}

/// Filing one *does* require it.
#[test]
fn filing_a_summenzeitreihe_authorises_as_trigger_mabis_run() {
    for route in ["trigger_sync", "retry_run"] {
        let start = SERVER
            .find(&format!("pub async fn {route}("))
            .unwrap_or_else(|| panic!("{route} exists"));
        assert!(
            SERVER[start..start + 400].contains("\"trigger-mabis-run\""),
            "{route} files a binding Summenzeitreihe and must authorise as trigger-mabis-run"
        );
    }
}

fn principal(tenant: &str, roles: &[&str]) -> CedarPrincipal {
    CedarPrincipal {
        sub: "caller".to_owned(),
        tenant: tenant.to_owned(),
        roles: roles.iter().map(|r| (*r).to_owned()).collect(),
    }
}

/// The policy decides the three powers as three: NB and ÜNB file, any caller
/// of the tenant reads and records, and nobody of another tenant does anything.
#[test]
fn the_policy_grants_each_power_to_whom_it_belongs() {
    let cedar = CedarEnforcer::from_policy_str(POLICY).expect("netzbilanzd.cedar parses");
    let allowed = |p: &CedarPrincipal, action: &str| cedar.check(p, action, TENANT).is_ok();

    for role in ["NB", "UENB"] {
        let p = principal(TENANT, &[role]);
        assert!(allowed(&p, "trigger-mabis-run"), "{role} files");
    }
    for roles in [&["LF"][..], &["MSB"], &["ESA"], &[]] {
        let p = principal(TENANT, roles);
        assert!(
            !allowed(&p, "trigger-mabis-run"),
            "{roles:?} has no standing to file a Summenzeitreihe"
        );
        assert!(allowed(&p, "read-mabis-run"), "{roles:?} reads run state");
        assert!(
            allowed(&p, "record-biko-response"),
            "{roles:?} relays an inbound IFTSTA"
        );
    }

    let fremd = principal("9900012345678", &["NB", "UENB"]);
    for action in [
        "trigger-mabis-run",
        "read-mabis-run",
        "record-biko-response",
    ] {
        assert!(
            !allowed(&fremd, action),
            "`{action}` is reachable across the tenant boundary"
        );
    }
}
