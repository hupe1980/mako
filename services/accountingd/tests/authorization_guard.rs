//! Guards on `accountingd`'s authentication and authorization surface.
//!
//! Source-level, because all three defects they pin are silent at compile time
//! and invisible until a request arrives:
//!
//! 1. **A handler with no `Claims` extractor is unauthenticated.** There is no
//!    global auth middleware — `Claims` is a `FromRequestParts` extractor, so a
//!    handler that does not name it is served to anyone.
//! 2. **`Claims` alone is authentication, not authorization.** Without a Cedar
//!    check any valid token from any tenant is accepted.
//! 3. **A Cedar action checked in code but named in no policy is a permanent
//!    403,** because Cedar is default-deny. The reverse is a dead grant, and
//!    usually means an endpoint lost its guard.

use std::collections::BTreeSet;

const POLICY: &str = include_str!("../policies/accountingd.cedar");
const HANDLERS: &str = include_str!("../src/handlers.rs");

/// Every `"…"` literal passed as the action argument of `cedar.check(..)`.
fn actions_used_in_code() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = HANDLERS;
    while let Some(idx) = rest.find(".check(") {
        rest = &rest[idx + ".check(".len()..];
        let Some(open) = rest.find('"') else { break };
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        found.insert(after[..close].to_owned());
        rest = &after[close..];
    }
    found
}

/// Every `Action::"…"` named in the policy.
fn actions_permitted_in_policy() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = POLICY;
    while let Some(idx) = rest.find("Action::\"") {
        rest = &rest[idx + "Action::\"".len()..];
        let Some(close) = rest.find('"') else { break };
        found.insert(rest[..close].to_owned());
        rest = &rest[close..];
    }
    found
}

#[test]
fn the_policy_parses() {
    mako_service::cedar::CedarEnforcer::from_policy_str(POLICY)
        .expect("accountingd.cedar must parse — the service refuses to start otherwise");
}

#[test]
fn the_policy_permits_every_action_the_code_checks() {
    let used = actions_used_in_code();
    assert!(
        !used.is_empty(),
        "the extractor found no actions — it has drifted from the call shape"
    );
    let permitted = actions_permitted_in_policy();
    let missing: Vec<_> = used.difference(&permitted).cloned().collect();
    assert!(
        missing.is_empty(),
        "these Cedar actions are checked in code but appear in no policy, so Cedar's \
         default-deny makes those endpoints return 403 for every caller: {missing:?}"
    );
}

#[test]
fn the_policy_grants_no_action_the_code_never_checks() {
    let used = actions_used_in_code();
    let permitted = actions_permitted_in_policy();
    let dead: Vec<_> = permitted.difference(&used).cloned().collect();
    assert!(
        dead.is_empty(),
        "these Cedar actions are granted by policy but checked nowhere — either the \
         endpoint lost its check (and is now unauthorized) or the grant is dead: {dead:?}"
    );
}

/// Handlers that legitimately carry no `Claims` extractor, each with its reason.
fn unauthenticated_by_design(handler: &str) -> Option<&'static str> {
    match handler {
        "ingest_webhook" => {
            Some("authenticated by the inbound X-Mako-Signature HMAC, not by a bearer token")
        }
        "metrics" => Some("Prometheus scrape target; aggregates only, no per-customer data"),
        _ => None,
    }
}

/// Every `pub async fn` in `handlers.rs` paired with its parameter list.
fn handler_signatures() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = HANDLERS;
    while let Some(idx) = rest.find("\npub async fn ") {
        rest = &rest[idx + "\npub async fn ".len()..];
        let Some(paren) = rest.find('(') else { break };
        let name = rest[..paren].trim().to_owned();
        let Some(end) = rest.find(") ->") else { break };
        out.push((name, rest[paren..end].to_owned()));
        rest = &rest[end..];
    }
    out
}

#[test]
fn every_handler_authenticates() {
    let offenders: Vec<String> = handler_signatures()
        .into_iter()
        .filter(|(name, sig)| !sig.contains("Claims") && unauthenticated_by_design(name).is_none())
        .map(|(name, _)| name)
        .collect();
    assert!(
        offenders.is_empty(),
        "these handlers name no `Claims` extractor, so they are served to any caller \
         without a token: {offenders:?}. Add `claims: Claims` and a Cedar check, or \
         list the handler in `unauthenticated_by_design` with its reason."
    );
}

#[test]
fn every_authenticated_handler_authorizes() {
    // A handler that takes `Claims` but never reaches Cedar has decided *who* is
    // calling and then ignored the answer.
    let mut offenders = Vec::new();
    for (name, sig) in handler_signatures() {
        if !sig.contains("Claims") {
            continue;
        }
        let Some(start) = HANDLERS.find(&format!("\npub async fn {name}(")) else {
            continue;
        };
        // The body runs to the next top-level `pub async fn`, or to EOF.
        let tail = &HANDLERS[start + 1..];
        let end = tail.find("\npub async fn ").map_or(tail.len(), |i| i + 1);
        if !tail[..end].contains("cedar.check(") {
            offenders.push(name);
        }
    }
    assert!(
        offenders.is_empty(),
        "these handlers authenticate but never authorize — any valid token from any \
         tenant is accepted: {offenders:?}"
    );
}

#[test]
fn the_handler_count_has_not_silently_shrunk() {
    // A guard that finds no handlers passes vacuously. accountingd had 55 when
    // this suite was written; the exact number matters less than the parser still
    // finding roughly that many.
    let n = handler_signatures().len();
    assert!(
        n >= 50,
        "the signature parser found only {n} handlers — it has drifted from the \
         source shape and the guards above are now vacuous"
    );
}
