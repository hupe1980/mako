//! The Cedar policy is the authorization contract — pinned here so a policy
//! edit that silently opens or closes an endpoint fails the build.
//!
//! billingd shipped with the `cedar` feature enabled and **no policy at all**:
//! authentication established who was calling and nothing established what they
//! could do, so any token accepted by the OIDC verifier could reverse an issued
//! invoice or release one the risk gate was holding back.

use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

const POLICY: &str = include_str!("../../policies/billingd.cedar");

/// The handler sources, so the guard can compare the actions the *code* checks
/// against the ones the policy grants. A hand-maintained list drifts silently
/// in both directions: a handler wired to an action no policy mentions is `403`
/// for every caller (Cedar is deny-by-default), and a granted action nobody
/// checks is either a lost check or a dead rule.
const HANDLERS: [&str; 8] = [
    include_str!("../../src/handlers/calculate.rs"),
    include_str!("../../src/handlers/correction.rs"),
    include_str!("../../src/handlers/dispatch.rs"),
    include_str!("../../src/handlers/enrichment.rs"),
    include_str!("../../src/handlers/ggv.rs"),
    include_str!("../../src/handlers/mod.rs"),
    include_str!("../../src/handlers/records.rs"),
    include_str!("../../src/handlers/sammelrechnung.rs"),
];
const VPP_HANDLER: &str = include_str!("../../src/handlers/vpp.rs");

/// Every action literal passed to `authorize(&cedar, &claims, "…", …)`.
///
/// Anchored on the `&claims, ` argument rather than the function name, which
/// also matches the definition and its own string literals.
fn actions_used_in_code() -> std::collections::BTreeSet<String> {
    const CALL: &str = "&claims, \"";
    let mut found = std::collections::BTreeSet::new();
    for src in HANDLERS.iter().chain(std::iter::once(&VPP_HANDLER)) {
        let mut rest = *src;
        while let Some(idx) = rest.find(CALL) {
            rest = &rest[idx + CALL.len()..];
            let Some(close) = rest.find('"') else { break };
            found.insert(rest[..close].to_owned());
            rest = &rest[close..];
        }
    }
    found
}

/// Every `Action::"…"` the policy names.
fn actions_permitted_in_policy() -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let mut rest = POLICY;
    while let Some(idx) = rest.find("Action::\"") {
        rest = &rest[idx + "Action::\"".len()..];
        let Some(close) = rest.find('"') else { break };
        found.insert(rest[..close].to_owned());
        rest = &rest[close..];
    }
    found
}

/// Every action a handler checks is granted somewhere, and every granted action
/// is checked.
#[test]
fn the_policy_and_the_code_name_the_same_actions() {
    let used = actions_used_in_code();
    assert!(
        !used.is_empty(),
        "the extractor found no actions — it has drifted from the call shape"
    );
    // The policy must also grant what the *middleware* checks one crate over.
    // `use-mcp` is the blanket gate `mako_service::mcp_auth::McpAuth` applies to
    // every MCP frame before any tool name is read, so no `authorize(&claims, …)`
    // in this crate names it and the extractor above cannot see it. Leaving it
    // out of this comparison made the guard refuse the grant the MCP surface
    // needs: the policy stayed silent, `CedarEnforcer` is default-deny, and the
    // whole tool and prompt surface answered 403 to every caller — with a green
    // guard over it. It is kept out of `used` because `ALL_ACTIONS` below pins
    // what this crate's own handlers check.
    let mut expected = used.clone();
    expected.insert("use-mcp".to_owned());
    // `read-policy` and `read-config` are checked by
    // `mako_service::einrichtung::router`, the one handler that serves
    // `GET /api/v1/policy` and `GET /admin/einrichtung` for every daemon.
    expected.insert("read-policy".to_owned());
    expected.insert("read-config".to_owned());
    // `manage-subscription` and `read-events` are checked by
    // `mako_service::subscriptions`, whose routes billingd merges.
    expected.insert(mako_service::subscriptions::MANAGE.to_owned());
    expected.insert(mako_service::subscriptions::READ_EVENTS.to_owned());
    let permitted = actions_permitted_in_policy();
    let missing: Vec<_> = expected.difference(&permitted).cloned().collect();
    assert!(
        missing.is_empty(),
        "these actions are checked in code but appear in no policy, so Cedar's \
         default-deny makes those routes 403 for every caller: {missing:?}"
    );
    let dead: Vec<_> = permitted.difference(&expected).cloned().collect();
    assert!(
        dead.is_empty(),
        "these actions are granted by policy but checked nowhere — either a route \
         lost its check or the grant is dead: {dead:?}"
    );
    assert_eq!(
        used,
        ALL_ACTIONS.iter().map(|a| (*a).to_owned()).collect(),
        "ALL_ACTIONS must list exactly what the code checks"
    );
}

fn enforcer() -> CedarEnforcer {
    CedarEnforcer::from_policy_str(POLICY).expect("billingd.cedar parses")
}

fn principal(tenant: &str, roles: &[&str]) -> CedarPrincipal {
    CedarPrincipal {
        sub: "test-sub".to_owned(),
        tenant: tenant.to_owned(),
        roles: roles.iter().map(|r| (*r).to_owned()).collect(),
    }
}

const TENANT: &str = "9910000000002";

/// Every action the router enforces must be decidable by the policy. A typo in
/// either place — a renamed action, a handler wired to a string no policy
/// mentions — turns into an endpoint nobody can reach.
const ALL_ACTIONS: [&str; 10] = [
    "read-billing",
    "read-billing-by-token",
    "preview-billing",
    "run-billing",
    "settle-flexibility",
    "correct-billing",
    "release-billing",
    "review-billing",
    "submit-b2g",
    "issue-billing",
];

/// The service's own market roles reach every action but the token lookup.
#[test]
fn a_lieferant_may_operate_the_service() {
    let e = enforcer();
    for action in ALL_ACTIONS
        .into_iter()
        .filter(|a| *a != "read-billing-by-token")
    {
        assert!(
            e.check(&principal(TENANT, &["LF"]), action, TENANT).is_ok(),
            "LF must be able to {action}"
        );
    }
}

/// Reading is open to any authenticated caller in the tenant; issuing is not.
/// A Netzbetreiber token reaching this service is not a supplier and must not
/// be able to put an invoice in front of a customer.
#[test]
fn a_foreign_market_role_reads_but_does_not_issue() {
    let e = enforcer();
    let nb = principal(TENANT, &["NB"]);
    assert!(e.check(&nb, "read-billing", TENANT).is_ok());
    assert!(e.check(&nb, "preview-billing", TENANT).is_ok());
    for action in [
        "run-billing",
        "correct-billing",
        "release-billing",
        "review-billing",
    ] {
        assert!(
            e.check(&nb, action, TENANT).is_err(),
            "NB must not be able to {action}"
        );
    }
}

/// A caller with no roles at all — a plain authenticated token — may look and
/// simulate, and nothing else.
#[test]
fn a_roleless_token_cannot_issue_or_reverse_anything() {
    let e = enforcer();
    let anon = principal(TENANT, &[]);
    assert!(e.check(&anon, "read-billing", TENANT).is_ok());
    for action in [
        "run-billing",
        "settle-flexibility",
        "correct-billing",
        "release-billing",
        "review-billing",
        "submit-b2g",
        "issue-billing",
    ] {
        assert!(
            e.check(&anon, action, TENANT).is_err(),
            "an unroled token must not be able to {action}"
        );
    }
}

/// Tenant is the outer boundary: no role in one tenant reaches another's data,
/// not even for a read.
#[test]
fn no_role_crosses_the_tenant_boundary() {
    let e = enforcer();
    let other = principal("9908888888888", &["LF", "MSB", "ESA"]);
    for action in ALL_ACTIONS {
        assert!(
            e.check(&other, action, TENANT).is_err(),
            "cross-tenant {action} must be denied"
        );
    }
}

/// The dev posture (`allow_insecure_no_auth`) mints synthetic claims carrying
/// `["NB", "LF", "MSB"]`. Every endpoint must stay reachable under them, or the
/// demos and the local stack break in a way that only shows up at runtime —
/// which is what naming a role no identity provider in this platform issues
/// (`BUCHHALTUNG`, `CONTROLLING`) would do.
#[test]
fn the_dev_admin_principal_reaches_every_endpoint() {
    let e = enforcer();
    let dev = CedarPrincipal {
        sub: "dev-admin".to_owned(),
        ..principal(TENANT, &["NB", "LF", "MSB"])
    };
    for action in ALL_ACTIONS {
        // The token route is portald's alone, in the demo too: there portald
        // calls with its own configured service key, so the dev principal is
        // never what reaches it.
        if action == "read-billing-by-token" {
            assert!(
                e.check(&dev, action, TENANT).is_err(),
                "dev-admin must not read an invoice by its verify token"
            );
            continue;
        }
        assert!(
            e.check(&dev, action, TENANT).is_ok(),
            "dev-admin must be able to {action}"
        );
    }
    assert!(
        e.check(&dev, "read-billing-by-token", TENANT).is_err(),
        "dev-admin must not read an invoice by its verify token"
    );
}

/// An action the policy never mentions is denied, not defaulted — Cedar is
/// deny-by-default and this pins that it stays so.
#[test]
fn an_unknown_action_is_denied() {
    let e = enforcer();
    assert!(
        e.check(&principal(TENANT, &["LF"]), "delete-everything", TENANT)
            .is_err()
    );
}

/// A verify token is the customer's credential, relayed by portald.
/// Its service principal reads by token; an operator holding every market
/// role does not, nor does portald in another tenant.
#[test]
fn only_portald_reads_by_verify_token() {
    let e = enforcer();
    let portald = CedarPrincipal {
        sub: "portald".to_owned(),
        ..principal(TENANT, &[])
    };
    assert!(e.check(&portald, "read-billing-by-token", TENANT).is_ok());
    assert!(
        e.check(&portald, "read-billing-by-token", "9908888888888")
            .is_err()
    );
    let operator = principal(TENANT, &["LF", "MSB", "ESA", "NB"]);
    assert!(e.check(&operator, "read-billing-by-token", TENANT).is_err());
}

/// Registering a webhook endpoint exports every matching invoice event, and
/// the feed discloses the same: an operator capability, not a market role's.
#[test]
fn only_an_admin_manages_webhooks_and_reads_the_feed() {
    let e = enforcer();
    for action in ["manage-subscription", "read-events"] {
        assert!(
            e.check(&principal(TENANT, &["LF"]), action, TENANT)
                .is_err(),
            "an LF must not {action}"
        );
        assert!(
            e.check(&principal(TENANT, &["ADMIN"]), action, TENANT)
                .is_ok(),
            "an ADMIN may {action}"
        );
        assert!(
            e.check(&principal("9900000000001", &["ADMIN"]), action, TENANT)
                .is_err(),
            "another tenant's ADMIN must not {action}"
        );
    }
}
