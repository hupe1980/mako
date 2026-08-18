//! The Cedar policy is the authorization contract — pinned here so a policy
//! edit that silently opens or closes an endpoint fails the build.
//!
//! billingd shipped with the `cedar` feature enabled and **no policy at all**:
//! authentication established who was calling and nothing established what they
//! could do, so any token accepted by the OIDC verifier could reverse an issued
//! invoice or release one the risk gate was holding back.

use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

const POLICY: &str = include_str!("../policies/billingd.cedar");

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
const ALL_ACTIONS: [&str; 7] = [
    "read-billing",
    "preview-billing",
    "run-billing",
    "settle-flexibility",
    "correct-billing",
    "release-billing",
    "submit-b2g",
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

/// Reading is open to any authenticated caller in the tenant; issuing is not.
/// A Netzbetreiber token reaching this service is not a supplier and must not
/// be able to put an invoice in front of a customer.
#[test]
fn a_foreign_market_role_reads_but_does_not_issue() {
    let e = enforcer();
    let nb = principal(TENANT, &["NB"]);
    assert!(e.check(&nb, "read-billing", TENANT).is_ok());
    assert!(e.check(&nb, "preview-billing", TENANT).is_ok());
    for action in ["run-billing", "correct-billing", "release-billing"] {
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
        "submit-b2g",
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
    let dev = principal(TENANT, &["NB", "LF", "MSB"]);
    for action in ALL_ACTIONS {
        assert!(
            e.check(&dev, action, TENANT).is_ok(),
            "dev-admin must be able to {action}"
        );
    }
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
