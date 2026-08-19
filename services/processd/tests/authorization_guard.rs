//! Guards on the authentication and authorization surface.
//!
//! processd does not merely report decisions, it makes them: approving a queue
//! entry dispatches the market answer, and `start-supply` / `end-supply`
//! initiate and terminate a supply relationship on the market. Every one of
//! those routes was reachable without a token — a Cedar policy existed and the
//! enforcer was injected into the router, but no handler ever called `check`.
//! Nothing in the type system notices that, so it is pinned here.
//!
//! The same three failure classes as `marktd`, for the same reasons:
//!
//! 1. **A handler with no `Claims` extractor is unauthenticated.** There is no
//!    global auth middleware; `Claims` is a `FromRequestParts` extractor, so a
//!    handler that does not name it is served to anyone.
//! 2. **A Cedar action checked in code but in no policy is a permanent 403**,
//!    because Cedar is default-deny.
//! 3. **A policy action nothing checks is a dead grant** — either the endpoint
//!    lost its check, or the grant describes an endpoint that no longer exists
//!    (`read-parity` named `GET /api/v1/parity`, which was never routed).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const POLICY: &str = include_str!("../policies/processd.cedar");

fn src(file: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read src/{file}: {e}"))
}

/// Every `"…"` literal passed as the action argument of an authorization call.
fn actions_used_in_code() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for file in ["server.rs", "mcp_server.rs", "handler.rs"] {
        let text = src(file);
        for call in ["authorize(", ".check(", ".authorize("] {
            let mut rest = text.as_str();
            while let Some(idx) = rest.find(call) {
                rest = &rest[idx + call.len()..];
                // The action is the first quoted literal in the call, and calls
                // never span more than a few lines.
                let window = &rest[..rest.len().min(200)];
                if let Some(open) = window.find('"')
                    && let Some(close) = window[open + 1..].find('"')
                {
                    found.insert(window[open + 1..open + 1 + close].to_owned());
                }
            }
        }
    }
    // Only Cedar action names, not arbitrary strings that happened to follow.
    found.retain(|a| a.contains('-') && a.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
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
    // `use-mcp` is the blanket gate the shared MCP middleware applies
    // (`mako_service::mcp_auth::McpAuth::authenticate`), one crate over.
    let enforced_elsewhere: BTreeSet<String> = ["use-mcp".to_owned()].into();
    let dead: Vec<_> = permitted
        .difference(&used)
        .filter(|a| !enforced_elsewhere.contains(*a))
        .cloned()
        .collect();
    assert!(
        dead.is_empty(),
        "these Cedar actions are granted by policy but checked nowhere — either the \
         endpoint lost its check (and is now unauthorized) or the grant is dead: {dead:?}"
    );
}

/// Every route the domain router mounts, paired with whether its handler both
/// takes a `Claims` extractor and authorizes.
#[test]
fn every_rest_handler_authenticates_and_authorizes() {
    let server = src("server.rs");

    // The handlers reachable from the router, excluding the webhook (which is
    // authenticated by the marktd HMAC, not by a bearer) and the MCP surface
    // (gated by the shared middleware).
    let routed: Vec<&str> = server
        .lines()
        .filter_map(|l| l.split_once("rest::"))
        .map(|(_, rest)| rest.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_'))
        .collect();
    assert!(
        !routed.is_empty(),
        "no `rest::` routes found — the extractor has drifted"
    );

    let mut offenders = Vec::new();
    for name in routed {
        let Some(start) = server.find(&format!("pub async fn {name}(")) else {
            offenders.push(format!("{name}: routed but not defined"));
            continue;
        };
        // The signature plus the first statements of the body.
        let body = &server[start..server.len().min(start + 1_400)];
        if !body.contains("claims: Claims") {
            offenders.push(format!(
                "{name}: no `Claims` extractor — reachable without a bearer token"
            ));
        }
        if !body.contains("authorize(&enforcer, &claims,") {
            offenders.push(format!("{name}: does not call `authorize`"));
        }
    }

    assert!(
        offenders.is_empty(),
        "REST handlers missing authentication or authorization:\n  {}",
        offenders.join("\n  ")
    );
}

/// `Claims` only exists as a `FromRequestParts` extractor; nothing inserts it
/// into request extensions, so `Extension<Claims>` is a guaranteed 500.
#[test]
fn no_handler_extracts_claims_as_a_request_extension() {
    for file in ["server.rs", "handler.rs", "mcp_server.rs"] {
        assert!(
            !src(file).contains("Extension<Claims>"),
            "src/{file} extracts `Extension<Claims>`, but no layer inserts it, so every \
             request to those handlers returns 500 — take `claims: Claims` instead"
        );
    }
}
