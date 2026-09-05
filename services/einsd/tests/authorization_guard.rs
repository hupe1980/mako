//! Guards on the authentication and authorization surface.
//!
//! `einsd` settles money. A route here registers an EEG or KWKG plant, runs a
//! month's Vergütung, corrects a settlement that has already been paid, or
//! records a § 52 EEG Pflichtzahlung against an operator. None of that is safe
//! to serve to whoever can reach the port, and **nothing in the type system
//! notices when a handler forgets to say so**: `Claims` is a
//! `FromRequestParts` extractor a signature can simply omit, and a
//! `CedarEnforcer` can be injected into a router that no handler ever consults.
//!
//! At the time this guard was written every routed handler was correct. That is
//! precisely when a guard is worth writing — it is here to keep the 36th
//! handler from being the exception, not to fix a defect.
//!
//! Three failure classes, none of which the compiler can see:
//!
//! 1. **A handler with no `Claims` extractor is unauthenticated.** There is no
//!    global auth middleware; a handler that does not name `Claims` is served
//!    to anyone who can reach the port.
//! 2. **A handler that takes `Claims` and never authorizes is authenticated and
//!    unguarded** — any token the verifier accepts reaches every route. This is
//!    the shape that shipped in two sibling services: 42 handlers took
//!    `_claims: Claims`, extracted it, and discarded it.
//! 3. **A Cedar action checked in code but named in no policy is a permanent
//!    403**, because Cedar is default-deny. The reverse — a policy action
//!    nothing checks — is a dead grant, which means either an endpoint lost its
//!    check or the grant describes a route that is not mounted.

use std::collections::BTreeSet;

const POLICY: &str = include_str!("../policies/einsd.cedar");
const ROUTES: &str = include_str!("../src/routes.rs");
const HANDLERS: &str = include_str!("../src/handlers.rs");

/// Actions enforced somewhere other than a handler body.
///
/// `use-mcp` gates the whole MCP surface from the transport: `main.rs` builds
/// `McpAuth::from_auth_config_oidc(.., Some(cedar.clone()), ..)`, so the check
/// runs in middleware before any tool is dispatched. A per-tool check would be
/// redundant, so its absence from `handlers.rs` is correct and not a dead
/// grant.
const ENFORCED_BY_MIDDLEWARE: [&str; 1] = ["use-mcp"];

/// Every handler the router actually mounts.
fn routed_handlers() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = ROUTES;
    while let Some(i) = rest.find("handlers::") {
        rest = &rest[i + "handlers::".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            found.insert(name);
        }
    }
    found
}

/// `handlers.rs` split into `pub async fn` bodies, keyed by name.
///
/// Crude on purpose: it must not depend on rustfmt's line breaking, because a
/// guard that stops matching when a signature wraps stops guarding in silence.
fn handler_bodies() -> Vec<(String, String)> {
    const MARKER: &str = "\npub async fn ";
    let mut out = Vec::new();
    let mut rest = HANDLERS;
    while let Some(i) = rest.find(MARKER) {
        rest = &rest[i + MARKER.len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let end = rest.find(MARKER).unwrap_or(rest.len());
        out.push((name, rest[..end].to_owned()));
    }
    out
}

/// The Cedar actions named in `Action::"…"` positions of the policy.
fn policy_actions() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = POLICY;
    while let Some(i) = rest.find("Action::\"") {
        rest = &rest[i + "Action::\"".len()..];
        if let Some(end) = rest.find('"') {
            found.insert(rest[..end].to_owned());
        }
    }
    found
}

/// The actions the handlers actually check, as the second argument to `check`.
fn code_actions() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (_, body) in handler_bodies() {
        let mut rest = body.as_str();
        while let Some(i) = rest.find("check(") {
            rest = &rest[i + "check(".len()..];
            // The action is the first string literal in the call. Scanning to
            // it rather than matching the whole call keeps this working when
            // rustfmt wraps the arguments across lines.
            let Some(q) = rest.find('"') else { break };
            let after = &rest[q + 1..];
            let Some(end) = after.find('"') else { break };
            let lit = &after[..end];
            if !lit.is_empty()
                && lit
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
            {
                found.insert(lit.to_owned());
            }
        }
    }
    found
}

#[test]
fn every_routed_handler_authenticates_and_authorizes() {
    let routed = routed_handlers();
    assert!(
        routed.len() > 20,
        "only {} routed handlers found — this guard has stopped parsing routes.rs",
        routed.len()
    );

    let bodies: std::collections::BTreeMap<_, _> = handler_bodies().into_iter().collect();

    let mut unauthenticated = Vec::new();
    let mut unauthorized = Vec::new();
    for name in &routed {
        let Some(body) = bodies.get(name) else {
            panic!(
                "routes.rs mounts handlers::{name}, which is not a `pub async fn` in handlers.rs"
            );
        };
        let signature = &body[..body.find('{').unwrap_or(body.len())];
        // `_claims` counts as absent on purpose: extracting the token and
        // discarding it is the exact defect this catches.
        if !signature.contains("claims: Claims") || signature.contains("_claims: Claims") {
            unauthenticated.push(name.clone());
        }
        if !body.contains("check(") {
            unauthorized.push(name.clone());
        }
    }

    assert!(
        unauthenticated.is_empty(),
        "these routed handlers do not extract `claims: Claims`, so they are served \
         to anyone who can reach the port: {unauthenticated:?}"
    );
    assert!(
        unauthorized.is_empty(),
        "these routed handlers authenticate but never authorize, so any accepted \
         token reaches them: {unauthorized:?}"
    );
}

#[test]
fn the_policy_permits_every_action_the_code_checks() {
    let policy = policy_actions();
    let code = code_actions();
    assert!(
        !code.is_empty(),
        "no checked actions found — this guard has stopped parsing handlers.rs"
    );

    let missing: Vec<_> = code.difference(&policy).cloned().collect();
    assert!(
        missing.is_empty(),
        "these actions are checked in code but named in no policy — Cedar is \
         default-deny, so each is a permanent 403: {missing:?}"
    );
}

#[test]
fn no_policy_grant_is_dead() {
    let policy = policy_actions();
    let mut code = code_actions();
    for a in ENFORCED_BY_MIDDLEWARE {
        code.insert(a.to_owned());
    }

    let dead: Vec<_> = policy.difference(&code).cloned().collect();
    assert!(
        dead.is_empty(),
        "these policy actions are checked nowhere — either an endpoint lost its \
         check, or the grant describes a route that is not mounted: {dead:?}"
    );
}

#[test]
fn the_policy_parses() {
    mako_service::cedar::CedarEnforcer::from_policy_str(POLICY)
        .expect("policies/einsd.cedar must parse — it is included at startup");
}
