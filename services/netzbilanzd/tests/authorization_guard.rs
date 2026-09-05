//! Guards on the authentication and authorization surface.
//!
//! netzbilanzd bills market partners: a route here dispatches an INVOIC to a
//! counterparty over AS4, reverses one it already holds, marks a receivable
//! paid, exports the § 147 AO / § 14b UStG record of a whole period, or files
//! the month's Redispatch cost sheet. None of that is safe to serve to whoever
//! can reach the port, and nothing in the type system notices when a handler
//! forgets to say so — `Claims` is an extractor a signature can simply omit,
//! and a Cedar enforcer can be injected into a router no handler ever consults.
//! So it is pinned here.
//!
//! Three failure classes, none of which the compiler can see:
//!
//! 1. **A handler with no `Claims` extractor is unauthenticated.** There is no
//!    global auth middleware; `Claims` is a `FromRequestParts` extractor, so a
//!    handler that does not name it is served to anyone.
//! 2. **A handler that takes `Claims` and never authorizes is authenticated and
//!    unguarded** — any token the verifier accepts reaches every route.
//! 3. **A Cedar action checked in code but in no policy is a permanent 403**,
//!    because Cedar is default-deny; a policy action nothing checks is a dead
//!    grant, which means an endpoint lost its check or the grant describes a
//!    route that is not mounted.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const POLICY: &str = include_str!("../policies/netzbilanzd.cedar");

/// The modules the router mounts handlers from.
const HANDLER_MODULES: [&str; 4] = ["handlers", "autorun", "kostenblatt", "ausfallarbeit_api"];

/// `POST /api/v1/webhooks/remadv` carries no bearer token: it is authenticated
/// by the inbound HMAC (`inbound_secret`), which startup refuses to run
/// without. It is the only route here that authorizes nothing.
const HMAC_AUTHENTICATED: [&str; 1] = ["post_remadv_webhook"];

fn src(file: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read src/{file}: {e}"))
}

/// The identifier that follows `needle` in `text`, for every occurrence.
fn identifiers_after(text: &str, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find(needle) {
        rest = &rest[idx + needle.len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            found.push(name);
        }
    }
    found
}

/// Every handler the domain router mounts, as `(module, function)`.
///
/// Read out of the routing calls themselves rather than every `module::name`
/// mention, so the background workers `main` also names are not mistaken for
/// routes.
fn routed_handlers() -> Vec<(&'static str, String)> {
    let main = src("main.rs");
    let mut routed = Vec::new();
    for module in HANDLER_MODULES {
        for verb in ["get(", "post(", "put(", "patch(", "delete("] {
            for name in identifiers_after(&main, &format!("{verb}{module}::")) {
                routed.push((module, name));
            }
        }
    }
    routed
}

/// Every `"…"` literal passed as the action argument of an `authorize` call.
fn actions_used_in_code() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for module in HANDLER_MODULES {
        let text = src(&format!("{module}.rs"));
        let mut rest = text.as_str();
        while let Some(idx) = rest.find("authorize(&cedar, &claims, \"") {
            rest = &rest[idx + "authorize(&cedar, &claims, \"".len()..];
            let Some(close) = rest.find('"') else { break };
            found.insert(rest[..close].to_owned());
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

/// Every routed handler authenticates and authorizes.
#[test]
fn every_rest_handler_authenticates_and_authorizes() {
    let routed = routed_handlers();
    assert!(
        routed.len() >= 25,
        "only {} routed handlers found — the extractor has drifted from main.rs",
        routed.len()
    );

    let mut offenders = Vec::new();
    for (module, name) in routed {
        if HMAC_AUTHENTICATED.contains(&name.as_str()) {
            continue;
        }
        let text = src(&format!("{module}.rs"));
        let Some(start) = text.find(&format!("pub async fn {name}(")) else {
            offenders.push(format!("{module}::{name}: routed but not defined"));
            continue;
        };
        // The signature plus the first statements of the body.
        let body = &text[start..text.len().min(start + 1_400)];
        if !body.contains("claims: Claims") {
            offenders.push(format!(
                "{module}::{name}: no `Claims` extractor — reachable without a bearer token"
            ));
        }
        if !body.contains("authorize(&cedar, &claims,") {
            offenders.push(format!(
                "{module}::{name}: takes a token and authorizes nothing — every caller the \
                 verifier admits reaches it"
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "REST handlers missing authentication or authorization:\n  {}",
        offenders.join("\n  ")
    );
}

/// The one route without a Cedar action verifies the inbound HMAC instead.
#[test]
fn the_webhook_verifies_its_signature() {
    let text = src("handlers.rs");
    let start = text
        .find("pub async fn post_remadv_webhook(")
        .expect("the REMADV webhook is defined");
    let body = &text[start..text.len().min(start + 1_400)];
    assert!(
        body.contains("mako_service::webhook::verify_request"),
        "post_remadv_webhook takes no token and does not verify the inbound HMAC, so a \
         forged REMADV marks an invoice paid"
    );
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

/// Reading and writing are separable, so a token that carries no market role —
/// an auditor's — reaches the § 147 AO export and dispatches nothing.
#[test]
fn an_auditor_reads_the_export_and_dispatches_nothing() {
    let cedar = mako_service::cedar::CedarEnforcer::from_policy_str(POLICY)
        .expect("netzbilanzd.cedar parses");
    let auditor = mako_service::cedar::CedarPrincipal {
        sub: "auditor".to_owned(),
        tenant: "9900357000004".to_owned(),
        roles: vec![],
    };
    for action in ["export-audit", "read-settlement", "read-kostenblatt"] {
        assert!(
            cedar.check(&auditor, action, "9900357000004").is_ok(),
            "a role-less caller of the tenant must reach `{action}`"
        );
    }
    for action in [
        "dispatch-settlement",
        "correct-settlement",
        "record-payment",
        "run-settlement",
        "amend-settlement",
        "submit-kostenblatt",
        "compute-kostenblatt",
        "compute-verguetung",
    ] {
        assert!(
            cedar.check(&auditor, action, "9900357000004").is_err(),
            "`{action}` moves money and must not be reachable without a market role"
        );
    }
}

/// A caller of another tenant reaches nothing at all.
#[test]
fn the_tenant_boundary_holds_for_every_action() {
    let cedar = mako_service::cedar::CedarEnforcer::from_policy_str(POLICY)
        .expect("netzbilanzd.cedar parses");
    let fremd = mako_service::cedar::CedarPrincipal {
        sub: "nb-of-another-tenant".to_owned(),
        tenant: "9900012345678".to_owned(),
        roles: vec!["NB".to_owned(), "MSB".to_owned(), "UENB".to_owned()],
    };
    for action in actions_permitted_in_policy() {
        assert!(
            cedar.check(&fremd, &action, "9900357000004").is_err(),
            "`{action}` is reachable across the tenant boundary"
        );
    }
}

/// `Claims` only exists as a `FromRequestParts` extractor; nothing inserts it
/// into request extensions, so `Extension<Claims>` is a guaranteed 500.
#[test]
fn no_handler_extracts_claims_as_a_request_extension() {
    for module in HANDLER_MODULES {
        let file = format!("{module}.rs");
        assert!(
            !src(&file).contains("Extension<Claims>"),
            "src/{file} extracts `Extension<Claims>`, but no layer inserts it, so every \
             request to those handlers returns 500 — take `claims: Claims` instead"
        );
    }
}

// ── Startup refusal ───────────────────────────────────────────────────────────

/// A configuration with every mandatory field, plus whatever `extra` adds.
fn config(extra: serde_json::Value) -> netzbilanzd::config::NetzbilanzConfig {
    let mut cfg = serde_json::json!({
        "database": { "url": "postgres://localhost/netzbilanzd" },
        "tenant": "9900357000004",
        "marktd_url": "http://localhost:9180",
        "marktd_api_key": "test",
        "makod_url": "http://localhost:8080",
        "makod_api_key": "test",
    });
    let (Some(base), Some(extra)) = (cfg.as_object_mut(), extra.as_object()) else {
        panic!("both must be JSON objects");
    };
    for (k, v) in extra {
        base.insert(k.clone(), v.clone());
    }
    serde_json::from_value(cfg).expect("the test configuration parses")
}

fn oidc() -> serde_json::Value {
    serde_json::json!({
        "issuer": "https://login.example.test/v2.0",
        "audience": "api://mako-netzbilanzd",
    })
}

#[test]
fn startup_refuses_without_oidc() {
    let err = config(serde_json::json!({ "inbound_secret": "s3cret" }))
        .check_auth_posture()
        .expect_err("a deployment with no [oidc] must not start");
    let msg = err.to_string();
    for named in [
        "[oidc]",
        "dispatch",
        "storno",
        "mark-paid",
        "audit",
        "kostenblatt/submit",
    ] {
        assert!(
            msg.contains(named),
            "the refusal must name what would otherwise be unauthenticated — {named:?} is \
             missing from: {msg}"
        );
    }
}

#[test]
fn startup_refuses_without_an_inbound_webhook_secret() {
    let err = config(serde_json::json!({ "oidc": oidc() }))
        .check_auth_posture()
        .expect_err("an unsigned REMADV webhook must not start");
    assert!(
        err.to_string().contains("inbound_secret"),
        "the refusal must name inbound_secret: {err}"
    );
}

#[test]
fn startup_refuses_an_open_mcp_surface() {
    let err = config(serde_json::json!({ "inbound_secret": "s3cret" }))
        .check_auth_posture()
        .expect_err("an MCP surface with neither [oidc] nor a key must not start");
    assert!(
        err.to_string().contains("[mcp] api_key"),
        "the refusal must name the open MCP surface: {err}"
    );

    // An `[mcp]` key closes that door on its own; the REST surface is refused
    // by its own entry, which is what the message must then say.
    let err = config(serde_json::json!({
        "inbound_secret": "s3cret",
        "mcp": { "api_key": "agent-key" },
    }))
    .check_auth_posture()
    .expect_err("[oidc] is still missing");
    assert!(
        !err.to_string().contains("[mcp] api_key"),
        "a configured MCP key must not be reported as missing: {err}"
    );
}

#[test]
fn a_fully_configured_deployment_starts() {
    config(serde_json::json!({ "oidc": oidc(), "inbound_secret": "s3cret" }))
        .check_auth_posture()
        .expect("[oidc] plus an inbound secret is a startable posture");
}

#[test]
fn an_insecure_deployment_starts_only_when_it_is_asked_for_by_name() {
    config(serde_json::json!({ "allow_insecure_no_auth": true }))
        .check_auth_posture()
        .expect("allow_insecure_no_auth is the dev escape hatch");
}
