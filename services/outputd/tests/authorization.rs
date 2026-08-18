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
const ALL_ACTIONS: [&str; 5] = [
    "read-template",
    "preview-template",
    "publish-template",
    "rollout-template",
    "render-document",
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
    for action in ["publish-template", "rollout-template", "render-document"] {
        assert!(
            e.check(&nb, action, TENANT).is_err(),
            "NB must not be able to {action}"
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
