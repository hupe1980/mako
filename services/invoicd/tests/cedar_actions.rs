//! Every Cedar action the code checks is one the policy can permit.
//!
//! # Why this is a test
//!
//! Cedar is deny-by-default. An action a handler names and the policy file does
//! not mention is a **permanent 403**: no token, no role and no configuration
//! change can lift it, and the response says only "forbidden", so it reads like
//! a permissions problem on the caller's side.
//!
//! Nothing fails to compile, because the action is a string on one side and a
//! policy document on the other. This test is the join the type system cannot
//! make.

/// The policy, as compiled into the binary.
const POLICY: &str = include_str!("../policies/invoicd.cedar");

/// Actions named in a `permit` head, ignoring comment lines.
///
/// The parse is deliberately literal — `Action::"name"` — rather than a real
/// Cedar parse: a stricter reader that silently matched nothing would make this
/// test pass by finding no actions at all.
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

/// The parser must actually find the policy's actions — otherwise every
/// assertion below is vacuous.
#[test]
fn the_policy_parse_is_not_vacuous() {
    let permitted = permitted_actions();
    assert!(
        permitted.len() >= 5,
        "expected the policy to permit several actions, found {permitted:?}"
    );
    assert!(permitted.contains(&"read-receipt".to_owned()));
}

/// Every action a handler checks appears in the policy.
#[test]
fn every_checked_action_is_permittable() {
    let permitted = permitted_actions();
    for action in invoicd::server::CEDAR_ACTIONS {
        assert!(
            permitted.contains(&(*action).to_owned()),
            "handlers check {action:?} but policies/invoicd.cedar never permits it — \
             that endpoint is a permanent 403. Permitted: {permitted:?}"
        );
    }
}

/// And nothing in the policy grants an action no handler checks: a stale
/// `permit` is a grant nobody reviews, and reads as coverage that is not there.
#[test]
fn the_policy_grants_nothing_unused() {
    for action in permitted_actions() {
        assert!(
            invoicd::server::CEDAR_ACTIONS.contains(&action.as_str()),
            "policies/invoicd.cedar permits {action:?} but no handler checks it"
        );
    }
}

/// The declared list must match what the handlers actually pass.
///
/// `CEDAR_ACTIONS` is hand-maintained, so it can drift from the `authorize(…)`
/// calls it claims to describe — and then the two tests above check a list
/// against itself.
#[test]
fn the_declared_list_matches_the_authorize_calls() {
    let source = include_str!("../src/server.rs");
    let mut called: Vec<&str> = source
        .match_indices("\", &state.tenant)")
        .filter_map(|(i, _)| {
            let before = &source[..i];
            let start = before.rfind('"')?;
            Some(&before[start + 1..])
        })
        .collect();
    // The selbstausstellen handler lives in its own module.
    called.push("dispatch-selbstausstellen");
    // The MCP surface is gated by `McpAuth`, which checks `use-mcp`.
    called.push("use-mcp");
    called.sort_unstable();
    called.dedup();

    let mut declared: Vec<&str> = invoicd::server::CEDAR_ACTIONS.to_vec();
    declared.sort_unstable();
    assert_eq!(
        called, declared,
        "CEDAR_ACTIONS does not match the actions the handlers check"
    );
}
