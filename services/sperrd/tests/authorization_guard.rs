//! Guards on `sperrd`'s authentication and authorization surface.
//!
//! Source-level, because both defects they pin — a handler with no `Claims`
//! extractor, and one that authenticates without authorizing — are silent at
//! compile time and invisible until a request arrives.

use std::collections::BTreeSet;

const POLICY: &str = include_str!("../policies/sperrd.cedar");
const HANDLERS: &str = include_str!("../src/handlers.rs");
const MCP: &str = include_str!("../src/mcp_server.rs");

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
        .expect("sperrd.cedar must parse — the service refuses to start otherwise");
}

#[test]
fn the_policy_permits_every_action_the_code_checks() {
    let used = actions_used_in_code();
    assert!(!used.is_empty(), "the extractor found no actions");
    let missing: Vec<_> = used
        .difference(&actions_permitted_in_policy())
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "checked in code but named in no policy — Cedar is default-deny, so these \
         endpoints 403 for every caller: {missing:?}"
    );
}

#[test]
fn the_policy_grants_no_action_the_code_never_checks() {
    let dead: Vec<_> = actions_permitted_in_policy()
        .difference(&actions_used_in_code())
        .cloned()
        .collect();
    assert!(
        dead.is_empty(),
        "granted by policy but checked nowhere — either an endpoint lost its guard \
         or the grant is dead: {dead:?}"
    );
}

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

/// Routes that legitimately carry no `Claims`, each with its reason.
fn unauthenticated_by_design(handler: &str) -> Option<&'static str> {
    match handler {
        "ingest_webhook" => Some(
            "the market inbox is authenticated by the inbound Standard Webhooks signature; \
             makod holds no bearer token for this service",
        ),
        _ => None,
    }
}

#[test]
fn every_handler_authenticates_and_authorizes() {
    let sigs = handler_signatures();
    assert!(
        sigs.len() >= 7,
        "the signature parser found only {} handlers — it has drifted and the guards \
         below are vacuous",
        sigs.len()
    );
    let mut offenders = Vec::new();
    for (name, sig) in sigs {
        if unauthenticated_by_design(&name).is_some() {
            continue;
        }
        if !sig.contains("Claims") {
            offenders.push(format!(
                "{name}: no Claims extractor — served without a token"
            ));
            continue;
        }
        let start = HANDLERS
            .find(&format!("\npub async fn {name}("))
            .expect("handler found by the parser must be findable");
        let tail = &HANDLERS[start + 1..];
        let end = tail.find("\npub async fn ").map_or(tail.len(), |i| i + 1);
        if !tail[..end].contains("cedar\n") && !tail[..end].contains("cedar.check(") {
            offenders.push(format!("{name}: authenticates but never authorizes"));
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}

#[test]
fn the_mcp_surface_stays_read_only() {
    // A model drives this surface and model output is untrusted, so the mutating
    // decision stays with an operator on the REST routes.
    assert!(
        !MCP.contains("destructive_hint = true"),
        "an MCP tool is annotated destructive — sperrd's MCP is read-only by design"
    );
    for forbidden in ["cancel_order_pg", "create_order_pg", "report_outcome"] {
        assert!(
            !MCP.contains(forbidden),
            "mcp_server.rs calls `{forbidden}`, a mutating operation. The MCP surface \
             is read-only; mutations belong on the authenticated REST routes."
        );
    }
}

/// No invented citation may appear.
///
/// The execution window is **6 Werktage** — BK6-24-174 GPKE Teil 2 § 3.5.1.2
/// Nr. 1: „Die Sperrung der Marktlokation ist durch den NB spätestens innerhalb
/// von 6 WT nach dem frühestmöglichen Sperrtermin durchzuführen." This guard
/// forbids the three claims that contradict it: a „2 Werktage" window,
/// BK6-22-024 §3.4 / §9 (neither exists), and the assertion that GPKE fixes no
/// window at all.
#[test]
fn no_fabricated_regulatory_citations_remain() {
    let sources: Vec<(&str, String)> = ["src/handlers.rs", "src/mcp_server.rs", "src/pg.rs"]
        .iter()
        .map(|p| {
            (
                *p,
                std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(p))
                    .expect("read source"),
            )
        })
        .chain(std::iter::once((
            "migrations/0001_schema.sql",
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_schema.sql"),
            )
            .expect("read schema"),
        )))
        .collect();

    for (path, src) in &sources {
        // Match against the text with Markdown emphasis stripped: a `**no**`
        // inside a phrase would otherwise slip past a literal `contains`.
        let src = &src.replace('*', "");

        for phrase in ["BK6-22-024 §3.4", "BK6-22-024 §9", "older_than_werktage"] {
            assert!(
                !src.contains(phrase),
                "{path} still cites {phrase:?}, which appears in no BNetzA or BDEW document"
            );
        }
        for phrase in ["2 Werktage", "2-Werktage"] {
            assert!(
                !src.contains(phrase),
                "{path} names a {phrase:?} execution window; GPKE Teil 2 § 3.5.1.2 \
                 Nr. 1 fixes 6 Werktage"
            );
        }
        for phrase in [
            "no execution deadline in Werktagen",
            "kein Werktage-Fenster",
            "GPKE defines none",
        ] {
            assert!(
                !src.contains(phrase),
                "{path} claims GPKE fixes no execution window; § 3.5.1.2 Nr. 1 fixes \
                 6 Werktage after the frühestmöglicher Sperrtermin"
            );
        }
    }
}
