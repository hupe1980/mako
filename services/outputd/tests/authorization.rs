//! The Cedar policy is the authorization contract — pinned here so a policy
//! edit that silently opens or closes an endpoint fails the build.
//!
//! outputd shipped with the `cedar` feature enabled in `Cargo.toml` and **no
//! policy file at all**. Authentication established who was calling and nothing
//! established what they could do, so any token the OIDC verifier accepted
//! could roll out the layout every invoice and Mahnung of the tenant renders
//! with — or render arbitrary content under the operator's Briefkopf. A
//! template is not one document; it is the shape of all of them.

use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

const POLICY: &str = include_str!("../policies/outputd.cedar");
/// The handlers, so the guard can compare the actions the *code* checks against
/// the ones the policy grants. A hand-maintained list drifts silently in both
/// directions: a handler wired to an action no policy mentions is `403` for
/// every caller (Cedar is deny-by-default), and a granted action nobody checks
/// is either a lost check or a dead rule.
const HANDLERS: &str = include_str!("../src/handlers.rs");

/// Every action literal passed to `authorize(&cedar, &claims, "…", …)`.
///
/// Anchored on the `&claims, ` argument rather than on the function name:
/// `authorize(` also matches the definition, whose body contains string
/// literals of its own.
fn actions_used_in_code() -> std::collections::BTreeSet<String> {
    const CALL: &str = "&claims, \"";
    let mut found = std::collections::BTreeSet::new();
    let mut rest = HANDLERS;
    while let Some(idx) = rest.find(CALL) {
        rest = &rest[idx + CALL.len()..];
        let Some(close) = rest.find('"') else { break };
        found.insert(rest[..close].to_owned());
        rest = &rest[close..];
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
    let permitted = actions_permitted_in_policy();
    let missing: Vec<_> = used.difference(&permitted).cloned().collect();
    assert!(
        missing.is_empty(),
        "these actions are checked in code but appear in no policy, so Cedar's \
         default-deny makes those routes 403 for every caller: {missing:?}"
    );
    let dead: Vec<_> = permitted.difference(&used).cloned().collect();
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
    CedarEnforcer::from_policy_str(POLICY).expect("outputd.cedar parses")
}

fn principal(tenant: &str, roles: &[&str]) -> CedarPrincipal {
    CedarPrincipal {
        sub: "test-sub".to_owned(),
        tenant: tenant.to_owned(),
        roles: roles.iter().map(|r| (*r).to_owned()).collect(),
    }
}

const TENANT: &str = "9900357000004";

/// Every action the router enforces must be decidable by the policy. A typo in
/// either place — a renamed action, a handler wired to a string no policy
/// mentions — turns into an endpoint nobody can reach.
const ALL_ACTIONS: [&str; 8] = [
    "read-template",
    "preview-template",
    "publish-template",
    "rollout-template",
    "render-document",
    "issue-document",
    "read-document",
    "report-delivery",
];

/// The service's own market roles reach every action.
#[test]
fn a_lieferant_may_operate_the_service() {
    let e = enforcer();
    for action in ALL_ACTIONS {
        assert!(
            e.check(&principal(TENANT, &["LF"]), action, TENANT).is_ok(),
            "LF must be able to {action}"
        );
    }
}

/// The other two document-issuing roles reach it too — a Messstellenbetreiber
/// bills metering services and an Energiedienstleister sells services, and both
/// put documents in front of customers under their own letterhead.
#[test]
fn the_other_issuing_roles_reach_every_action() {
    let e = enforcer();
    for role in ["MSB", "ESA"] {
        for action in ALL_ACTIONS {
            assert!(
                e.check(&principal(TENANT, &[role]), action, TENANT).is_ok(),
                "{role} must be able to {action}"
            );
        }
    }
}

/// A Netzbetreiber token reaching this service is not the party whose
/// letterhead this is. It may look, and it may render mako's own specimen to
/// see what a candidate layout does — neither of which reaches a customer.
#[test]
fn a_foreign_market_role_looks_but_does_not_publish_or_render() {
    let e = enforcer();
    let nb = principal(TENANT, &["NB"]);
    assert!(e.check(&nb, "read-template", TENANT).is_ok());
    assert!(
        e.check(&nb, "preview-template", TENANT).is_ok(),
        "a preview renders the specimen and stores nothing"
    );
    for action in [
        "publish-template",
        "rollout-template",
        "render-document",
        "issue-document",
        "report-delivery",
    ] {
        assert!(
            e.check(&nb, action, TENANT).is_err(),
            "NB must not be able to {action}"
        );
    }
    assert!(
        e.check(&nb, "read-document", TENANT).is_ok(),
        "reading issued documents is a tenant read — the scope that protects a customer is \
         the query, which refuses to answer without a MaLo or a Kundennummer"
    );
}

/// Issuing a document is gated exactly like rendering one.
///
/// `POST /render` produces bytes and forgets them; `POST /documents` writes a
/// row kept for eight years and sends it to a named person. If anything, the
/// second is the stronger act — so a caller who cannot render must not be able
/// to issue, and this pins that the two never drift apart.
#[test]
fn issuing_is_gated_at_least_as_tightly_as_rendering() {
    let e = enforcer();
    for role in [
        vec!["LF"],
        vec!["MSB"],
        vec!["ESA"],
        vec!["NB"],
        vec!["UENB"],
        vec![],
    ] {
        let p = principal(TENANT, &role);
        assert_eq!(
            e.check(&p, "render-document", TENANT).is_ok(),
            e.check(&p, "issue-document", TENANT).is_ok(),
            "render and issue must be reachable by exactly the same callers ({role:?})"
        );
    }
}

/// Tenant equality is a condition of **every** rule, reads included.
///
/// A template's source carries the operator's Briefkopf and whatever they put
/// in a comment. `template_store::by_hash` is tenant-scoped for the same
/// reason; this is the other half of that lock.
#[test]
fn no_action_crosses_a_tenant_boundary() {
    let e = enforcer();
    let other = principal("9910000000002", &["LF", "MSB", "ESA"]);
    for action in ALL_ACTIONS {
        assert!(
            e.check(&other, action, TENANT).is_err(),
            "{action} must not cross tenants"
        );
    }
}

/// An action the policy does not mention is denied, not defaulted.
///
/// Cedar is deny-by-default, and this pins that a handler wired to a
/// mistyped action string fails closed rather than silently permitting.
#[test]
fn an_unknown_action_is_denied() {
    assert!(
        enforcer()
            .check(&principal(TENANT, &["LF"]), "delete-template", TENANT)
            .is_err(),
        "an action no policy names must be denied"
    );
}

/// Rolling out is the act that reaches customers, and it is gated exactly like
/// publishing.
///
/// Publishing writes a row nobody may ever see; rolling out re-points every
/// subsequent render, so one careless PUT changes the appearance of a whole
/// month's invoicing. Both need an issuing role — the policy's own comment
/// explains why they are not *separately* authorised, and this pins that the
/// weaker of the two is not accidentally open.
#[test]
fn rolling_out_is_gated_like_publishing() {
    let e = enforcer();
    for role in [vec!["NB"], vec!["UENB"], vec![]] {
        let p = principal(TENANT, &role);
        assert_eq!(
            e.check(&p, "publish-template", TENANT).is_ok(),
            e.check(&p, "rollout-template", TENANT).is_ok(),
            "publish and rollout must be reachable by exactly the same callers ({role:?})"
        );
    }
}
