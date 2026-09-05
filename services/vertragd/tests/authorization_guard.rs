//! Guards on the authentication and authorization surface.
//!
//! `vertragd` is the only service an **end customer's own token** reaches:
//! `portald` forwards it verbatim to the portal authorization check, because
//! the identity that check is about is the customer's. That makes a route which
//! authenticates and then forgets to authorize a privilege escalation, not
//! merely a missing check — the customer holds a first-class `Claims` for every
//! route in the service, and the tenant the extractor pins is satisfied by
//! construction.
//!
//! Nothing in the type system notices a discarded `Claims`, so it is pinned
//! here, in the four failure classes this repo has met before:
//!
//! 1. **A handler with no `Claims` extractor is unauthenticated.** There is no
//!    global auth middleware; `Claims` is a `FromRequestParts` extractor, so a
//!    handler that does not name it is served to anyone.
//! 2. **A handler that extracts `Claims` and never authorizes** is reachable by
//!    every principal that holds any token for this tenant — a customer's
//!    included.
//! 3. **A Cedar action checked in code but in no policy is a permanent 403**,
//!    because Cedar is default-deny.
//! 4. **A policy action nothing checks is a dead grant** — either the endpoint
//!    lost its check, or the grant describes an endpoint that is not routed.
//!
//! The last test is the substance rather than the shape: a role-less principal
//! — which is what a portal customer's token produces — is refused on every
//! operator action and admitted on exactly the two customer-scoped ones.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

const POLICY: &str = include_str!("../policies/vertragd.cedar");

const TENANT: &str = "9900357000004";

/// The handler modules the router mounts REST routes from. `inbound` is
/// excluded: its two routes are authenticated by the `inbound_secret` HMAC and
/// carry no token at all.
const HANDLER_MODULES: [&str; 4] = ["kunden", "vertraege", "rahmenvertraege", "stammdaten"];

/// The MCP surface's blanket gate, applied by the shared middleware rather than
/// by a handler in this crate.
const MCP_GATE: &str = "use-mcp";

/// The first `n` characters of `text` — a slice by byte index splits the German
/// prose in these files mid-character.
fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// One item's own source, from `start` to the next top-level item.
///
/// A fixed-length window would run past the end of a short handler into the
/// next one, and a missing `authorize` there would be masked by its neighbour's.
fn item_source(text: &str, start: usize) -> &str {
    let rest = &text[start..];
    // Past the `pub async fn …(` this starts on, to the next item's `pub`.
    let after_header = rest.find(") ->").map_or(0, |i| i + 4);
    match rest[after_header..].find("\npub ") {
        Some(end) => &rest[..after_header + end],
        None => rest,
    }
}

/// `text` with every space, tab and newline removed.
///
/// The checks below look for call shapes, and rustfmt wraps a call across lines
/// as soon as it is long enough. Matching the wrapped form would make the guard
/// pass or fail on the length of an action name.
fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn src(file: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read src/{file}: {e}"))
}

/// Every `"…"` literal passed as the action argument of an `authorize` call.
fn actions_used_in_code() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for module in HANDLER_MODULES {
        let text = src(&format!("handlers/{module}.rs"));
        let mut rest = text.as_str();
        while let Some(idx) = rest.find("authorize(") {
            rest = &rest[idx + "authorize(".len()..];
            // The action is the first quoted literal in the call, and a call
            // never spans more than one line.
            let window = head(rest, 200);
            if let Some(open) = window.find('"')
                && let Some(close) = window[open + 1..].find('"')
            {
                found.insert(window[open + 1..open + 1 + close].to_owned());
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

/// The `module::handler` pairs `handlers::router` mounts, minus the webhooks.
fn routed_handlers() -> Vec<(String, String)> {
    let router = src("handlers/mod.rs");
    let body = router
        .split_once("pub fn router(")
        .expect("handlers::router is defined")
        .1;
    let mut out = Vec::new();
    for module in HANDLER_MODULES {
        // `get(` / `post(` / `.route(` — the path prefix keeps `vertraege::`
        // from also matching inside `rahmenvertraege::`.
        for needle in [format!("({module}::"), format!(" {module}::")] {
            let mut rest = body;
            while let Some(idx) = rest.find(&needle) {
                rest = &rest[idx + needle.len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.push((module.to_owned(), name));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
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

/// Every routed REST handler both authenticates and authorizes.
#[test]
fn every_rest_handler_authenticates_and_authorizes() {
    let routed = routed_handlers();
    assert!(
        routed.len() > 40,
        "only {} routes found — the extractor has drifted from the router",
        routed.len()
    );

    let mut offenders = Vec::new();
    for (module, name) in routed {
        let text = src(&format!("handlers/{module}.rs"));
        let Some(start) = text.find(&format!("pub async fn {name}(")) else {
            offenders.push(format!("{module}::{name}: routed but not defined"));
            continue;
        };
        let body = compact(item_source(&text, start));
        // `(claims:Claims` / `,claims:Claims` and never `_claims`: a discarded
        // extractor is the shape this whole file exists to catch.
        if !body.contains("(claims:Claims") && !body.contains(",claims:Claims") {
            offenders.push(format!(
                "{module}::{name}: no `Claims` extractor, or one bound to `_claims` and \
                 discarded — the token is verified and then ignored"
            ));
        }
        if !body.contains("authorize(&enforcer,&claims,") {
            offenders.push(format!(
                "{module}::{name}: does not call `authorize` — every holder of a token for \
                 this tenant reaches it, a portal customer's included"
            ));
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
    for module in HANDLER_MODULES {
        let file = format!("handlers/{module}.rs");
        assert!(
            !src(&file).contains("Extension<Claims>"),
            "src/{file} extracts `Extension<Claims>`, but no layer inserts it, so every \
             request to those handlers returns 500 — take `claims: Claims` instead"
        );
    }
}

// ── The customer-vs-operator split ────────────────────────────────────────────

fn enforcer() -> CedarEnforcer {
    CedarEnforcer::from_policy_str(POLICY).expect("vertragd.cedar parses")
}

/// A portal customer: a verified token for this tenant, carrying no market role.
fn customer() -> CedarPrincipal {
    CedarPrincipal {
        sub: "portal-user-4711".to_owned(),
        tenant: TENANT.to_owned(),
        roles: vec![],
    }
}

/// The supplier's own identity — staff, or a peer service's credential.
fn operator() -> CedarPrincipal {
    CedarPrincipal {
        sub: "svc-portald".to_owned(),
        tenant: TENANT.to_owned(),
        roles: vec!["LF".to_owned()],
    }
}

/// The two routes `portald` calls on a customer's behalf, and nothing else.
const CUSTOMER_SCOPED: [&str; 2] = ["authenticate-portal-identity", "read-own-portal-identity"];

#[test]
fn a_customer_token_reaches_only_the_two_customer_scoped_actions() {
    let enforcer = enforcer();
    let customer = customer();
    for action in CUSTOMER_SCOPED {
        assert!(
            enforcer.check(&customer, action, TENANT).is_ok(),
            "{action} is the portal check itself — a customer token must reach it"
        );
    }
    for action in actions_permitted_in_policy() {
        if CUSTOMER_SCOPED.contains(&action.as_str()) {
            continue;
        }
        assert!(
            enforcer.check(&customer, &action, TENANT).is_err(),
            "a portal customer's own token is admitted to the operator action {action:?} — \
             the escalation this policy exists to stop"
        );
    }
}

/// The escalation chain, named: with only their own token, a customer could
/// grant themselves a portal login on any other customer's account, read that
/// customer's DSGVO export or bank details, terminate their contract, or erase
/// their record.
#[test]
fn a_customer_token_cannot_take_over_another_customers_account() {
    let enforcer = enforcer();
    let customer = customer();
    for action in [
        "manage-portal-identitaeten",
        "read-kunde",
        "export-kunde",
        "anonymize-kunde",
        "read-zahlungsinformation",
        "kuendigen-vertrag",
        "tarifwechsel-vertrag",
        // The MCP tools read the same profiles and bank details.
        MCP_GATE,
    ] {
        assert!(
            enforcer.check(&customer, action, TENANT).is_err(),
            "{action} is reachable with a portal customer's token"
        );
    }
}

#[test]
fn an_operator_reaches_every_action_of_its_own_tenant() {
    let enforcer = enforcer();
    let operator = operator();
    for action in actions_permitted_in_policy() {
        assert!(
            enforcer.check(&operator, &action, TENANT).is_ok(),
            "the LF operating this deployment is refused {action:?} — an endpoint no caller \
             can reach is worse than one too many can"
        );
    }
}

#[test]
fn a_token_from_another_tenant_reaches_nothing() {
    let enforcer = enforcer();
    let foreign = CedarPrincipal {
        sub: "svc-foreign".to_owned(),
        tenant: "9900000000001".to_owned(),
        roles: vec!["LF".to_owned(), "MSB".to_owned(), "ADMIN".to_owned()],
    };
    for action in actions_permitted_in_policy() {
        assert!(
            enforcer.check(&foreign, &action, TENANT).is_err(),
            "an operator of another tenant reaches {action:?}"
        );
    }
}

/// The routes that must never be reachable with anything but an operator
/// identity, named as routes rather than as actions — so a later refactor that
/// moves one of them onto a customer-scoped action fails here.
#[test]
fn the_account_takeover_routes_are_operator_actions() {
    let kunden = src("handlers/kunden.rs");
    let vertraege = src("handlers/vertraege.rs");
    for (text, handler, expected) in [
        (&kunden, "upsert_identitaet", "manage-portal-identitaeten"),
        (&kunden, "delete_identitaet", "manage-portal-identitaeten"),
        (&kunden, "anonymize", "anonymize-kunde"),
        (&kunden, "gdpr_export", "export-kunde"),
        (
            &kunden,
            "put_zahlungsinformation",
            "write-zahlungsinformation",
        ),
        (
            &kunden,
            "get_zahlungsinformation",
            "read-zahlungsinformation",
        ),
        (&vertraege, "kuendigen", "kuendigen-vertrag"),
        (&vertraege, "tarifwechsel", "tarifwechsel-vertrag"),
    ] {
        let start = text
            .find(&format!("pub async fn {handler}("))
            .unwrap_or_else(|| panic!("{handler} is defined"));
        let body = compact(item_source(text, start));
        assert!(
            body.contains(&format!("authorize(&enforcer,&claims,\"{expected}\"")),
            "{handler} does not authorize {expected:?}"
        );
        assert!(
            !CUSTOMER_SCOPED.contains(&expected),
            "{handler} authorizes the customer-scoped action {expected:?}"
        );
    }
}
