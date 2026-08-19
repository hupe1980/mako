//! Guards on the authentication and authorization surface.
//!
//! These are source-level tests rather than request-level ones on purpose: the
//! two defects they pin are both *silent at compile time and invisible until a
//! request arrives in production*, and both had shipped.
//!
//! 1. **A handler with no `Claims` extractor is unauthenticated.** marktd has no
//!    global auth middleware — `Claims` is a `FromRequestParts` extractor, so a
//!    handler that simply does not name it is served to anyone. The §42 EnWG
//!    Energiemix endpoints were reachable without a token this way.
//!
//! 2. **A Cedar action that appears in code but in no policy is a permanent
//!    403.** Cedar is default-deny, so `enforcer.check(.., "read-grundversorger", ..)`
//!    against a policy that never mentions that action denies every caller —
//!    the §36 Abs. 2 Grundversorger endpoints and the whole `/admin/fanout/dlq`
//!    surface were dead this way. The reverse (a policy action nothing checks)
//!    is a dead grant and is pinned too.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const POLICY: &str = include_str!("../policies/marktd.cedar");

fn handlers_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/handlers")
}

fn handler_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(handlers_dir()).expect("read src/handlers") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let name = path
            .file_stem()
            .expect("file stem")
            .to_string_lossy()
            .into_owned();
        out.push((name, std::fs::read_to_string(&path).expect("read handler")));
    }
    out.sort();
    out
}

/// Every `"…"` literal passed as the second argument of `enforcer.check(..)`.
fn actions_used_in_code() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut sources: Vec<String> = handler_sources().into_iter().map(|(_, s)| s).collect();
    sources.push(
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mcp_server.rs"))
            .expect("read mcp_server.rs"),
    );

    for src in sources {
        // `.check(` … the action is the first quoted literal after it.
        let mut rest = src.as_str();
        while let Some(idx) = rest.find(".check(") {
            rest = &rest[idx + ".check(".len()..];
            let Some(open) = rest.find('"') else { break };
            let after = &rest[open + 1..];
            let Some(close) = after.find('"') else { break };
            found.insert(after[..close].to_owned());
            rest = &after[close..];
        }
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
    // (`mako_service::mcp_auth::McpAuth::authenticate`), so it is granted here
    // but checked one crate over rather than in a marktd handler.
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

/// Modules whose handlers legitimately carry no `Claims` extractor, each with
/// the reason it is safe.
fn unauthenticated_by_design(module: &str) -> Option<&'static str> {
    match module {
        "health" => Some("liveness/readiness probes must answer before auth is usable"),
        "metrics" => Some("Prometheus scrape target; no personal or tenant data"),
        "event_ingest" => Some("authenticated by the makod X-Mako-Signature HMAC, not by a bearer"),
        "mod" => Some("shared helpers, no handlers"),
        _ => None,
    }
}

#[test]
fn every_handler_module_authenticates_and_authorizes() {
    let mut offenders = Vec::new();

    for (module, src) in handler_sources() {
        let handlers = src.matches("\npub async fn ").count();
        if handlers == 0 {
            continue;
        }
        if let Some(_reason) = unauthenticated_by_design(&module) {
            continue;
        }

        // One `claims: Claims` parameter and one `.check(` per handler. Counting
        // rather than parsing keeps the guard blunt on purpose: a handler added
        // without either trips it, and the fix is to add them, not to tune this.
        let authenticated = src.matches("claims: Claims,").count();
        let authorized = src.matches(".check(").count();

        if authenticated < handlers {
            offenders.push(format!(
                "{module}: {handlers} handlers but only {authenticated} take a `Claims` \
                 extractor — the remainder are reachable without a bearer token"
            ));
        }
        if authorized < handlers {
            offenders.push(format!(
                "{module}: {handlers} handlers but only {authorized} call \
                 `enforcer.check(..)` — the remainder skip Cedar entirely"
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "handler modules missing authentication or authorization:\n  {}",
        offenders.join("\n  ")
    );
}

/// `Claims` only exists as a `FromRequestParts` extractor; nothing inserts it
/// into request extensions. `Extension(claims): Extension<Claims>` therefore
/// fails at runtime with a 500 on every request — which is exactly what the
/// Lokationszuordnung graph API and nine device endpoints did.
#[test]
fn no_handler_extracts_claims_as_a_request_extension() {
    let offenders: Vec<String> = handler_sources()
        .into_iter()
        .filter(|(_, src)| src.contains("Extension<Claims>"))
        .map(|(module, _)| module)
        .collect();

    assert!(
        offenders.is_empty(),
        "these modules extract `Extension<Claims>`, but no layer ever inserts it, so every \
         request to them returns 500: {offenders:?} — take `claims: Claims` instead"
    );
}
