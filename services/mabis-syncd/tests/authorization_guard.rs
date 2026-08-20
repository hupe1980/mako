//! Every Cedar action the handlers check is one the policy can permit.
//!
//! # Why this is a test
//!
//! Cedar is deny-by-default, so an action a handler names and the policy file
//! does not mention is a **permanent 403**: no token, role or configuration
//! change lifts it, and the response says only "forbidden". Nothing fails to
//! compile, because the action is a string on one side and a policy document on
//! the other.
//!
//! The second half matters as much. `POST /api/v1/datenstatus` and
//! `POST /api/v1/pruefmitteilung` only *record* what the BIKO said, so they
//! must not sit behind `trigger-mabis-run` — that would give the ingest
//! identity the power to file a binding Summenzeitreihe in the tenant's name.
//! An over-broad grant is not a 403, and only a per-route check sees it.

const POLICY: &str = include_str!("../policies/mabis-syncd.cedar");
const SERVER: &str = include_str!("../src/server.rs");

/// Actions named in a `permit` head, ignoring comment lines.
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
    out.sort();
    out.dedup();
    out
}

/// Actions passed to `deny(…)` in the router.
fn checked_actions() -> Vec<String> {
    let mut out: Vec<String> = SERVER
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('"') && l.ends_with("\",") && l.contains('-'))
        .map(|l| l.trim_matches(|c| c == '"' || c == ',').to_owned())
        .filter(|a| a.starts_with("read-") || a.starts_with("trigger-") || a.starts_with("record-"))
        .collect();
    // The MCP surface is gated by `McpAuth`, which checks `use-mcp`.
    out.push("use-mcp".to_owned());
    out.sort();
    out.dedup();
    out
}

/// The parsers must find something — otherwise every assertion is vacuous.
#[test]
fn the_parses_are_not_vacuous() {
    assert!(
        permitted_actions().len() >= 3,
        "policy actions: {:?}",
        permitted_actions()
    );
    assert!(
        checked_actions().len() >= 3,
        "checked actions: {:?}",
        checked_actions()
    );
}

/// Every action a handler checks is one the policy permits.
#[test]
fn every_checked_action_is_permittable() {
    let permitted = permitted_actions();
    for action in checked_actions() {
        assert!(
            permitted.contains(&action),
            "a handler checks {action:?} but policies/mabis-syncd.cedar never permits it — \
             that route is a permanent 403. Permitted: {permitted:?}"
        );
    }
}

/// And the policy grants nothing no handler checks: a stale `permit` reads as
/// coverage that is not there.
#[test]
fn the_policy_grants_nothing_unused() {
    let checked = checked_actions();
    for action in permitted_actions() {
        assert!(
            checked.contains(&action),
            "policies/mabis-syncd.cedar permits {action:?} but no handler checks it"
        );
    }
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
            .find(&format!("async fn {route}("))
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

/// Filing one *does* require it, and the policy restricts that to the roles
/// with standing to aggregate a Bilanzierungsgebiet.
#[test]
fn filing_a_summenzeitreihe_stays_restricted_to_the_grid_roles() {
    for route in ["trigger_sync", "retry_run"] {
        let start = SERVER
            .find(&format!("async fn {route}("))
            .unwrap_or_else(|| panic!("{route} exists"));
        assert!(
            SERVER[start..start + 400].contains("\"trigger-mabis-run\""),
            "{route} files a binding Summenzeitreihe and must authorise as trigger-mabis-run"
        );
    }
    let grant = POLICY
        .split("Action::\"trigger-mabis-run\"")
        .nth(1)
        .expect("the policy grants trigger-mabis-run");
    assert!(grant.contains("\"NB\""), "NB must be able to file");
    assert!(grant.contains("\"UENB\""), "UENB must be able to file");
}
