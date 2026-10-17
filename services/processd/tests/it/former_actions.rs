//! Guard: every Cedar action the process read model and the Sperrauftrag queue
//! carry is still checked by a handler and still decided by processd's policy
//! the way it was decided before the two became processd modules.
//!
//! Merging two policy files into one is where an action goes quiet: a grant
//! nobody copied is a permanent 403, and a grant copied into the wrong block
//! widens it to a role it never had. Both are invisible until a caller hits
//! the route, so each action is evaluated here for a principal it must admit
//! and one it must refuse.

use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

const POLICY: &str = include_str!("../../policies/processd.cedar");
const TENANT: &str = "9900357000004";

fn src(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read src/{file}: {e}"))
}

fn principal(tenant: &str, roles: &[&str]) -> CedarPrincipal {
    CedarPrincipal {
        sub: "former-actions-test".to_owned(),
        tenant: tenant.to_owned(),
        roles: roles.iter().map(|r| (*r).to_owned()).collect(),
    }
}

fn admits(enforcer: &CedarEnforcer, who: &CedarPrincipal, action: &str) -> bool {
    enforcer.check(who, action, TENANT).is_ok()
}

/// The read model's actions: any principal of the tenant, no role required.
const READ_MODEL: [&str; 3] = ["read-process", "read-kpi", "read-overdue"];

/// The Sperrauftrag queue's actions: the Netzbetreiber alone.
const SPERRAUFTRAG: [&str; 4] = [
    "read-sperr-order",
    "create-sperr-order",
    "execute-sperr-order",
    "cancel-sperr-order",
];

#[test]
fn the_read_model_actions_are_tenant_scoped_and_role_free() {
    let enforcer = CedarEnforcer::from_policy_str(POLICY).expect("processd.cedar parses");
    for action in READ_MODEL {
        assert!(
            admits(&enforcer, &principal(TENANT, &[]), action),
            "{action}: a role-less principal of the tenant reads the process read model"
        );
        assert!(
            !admits(&enforcer, &principal("9900000000002", &["NB"]), action),
            "{action}: another tenant's principal must not read this tenant's processes"
        );
    }
}

#[test]
fn the_sperrauftrag_actions_need_the_nb_role() {
    let enforcer = CedarEnforcer::from_policy_str(POLICY).expect("processd.cedar parses");
    for action in SPERRAUFTRAG {
        assert!(
            admits(&enforcer, &principal(TENANT, &["NB"]), action),
            "{action}: the Netzbetreiber works its own Sperrauftrag queue"
        );
        for roles in [&[][..], &["LF"], &["MSB"]] {
            assert!(
                !admits(&enforcer, &principal(TENANT, roles), action),
                "{action}: {roles:?} has no basis to order, execute or read a disconnection"
            );
        }
        assert!(
            !admits(&enforcer, &principal("9900000000002", &["NB"]), action),
            "{action}: another tenant's NB must not reach this tenant's queue"
        );
    }
}

#[test]
fn the_rebuild_is_an_operator_capability() {
    let enforcer = CedarEnforcer::from_policy_str(POLICY).expect("processd.cedar parses");
    assert!(admits(
        &enforcer,
        &principal(TENANT, &["ADMIN"]),
        "rebuild-projection"
    ));
    for roles in [&[][..], &["NB"], &["LF"]] {
        assert!(
            !admits(&enforcer, &principal(TENANT, roles), "rebuild-projection"),
            "{roles:?} must not rewrite the Gleichbehandlung evidence"
        );
    }
}

/// Each action is still checked where its route is served — a grant with no
/// check behind it would mean the route lost its authorization.
#[test]
fn every_former_action_is_checked_by_its_handler() {
    let checked_in = [
        ("obs/rest.rs", &READ_MODEL[..]),
        ("sperr/handlers.rs", &SPERRAUFTRAG[..]),
    ];
    for (file, actions) in checked_in {
        let text = src(file);
        for action in actions {
            assert!(
                text.contains(&format!("\"{action}\"")),
                "{action} is not checked in src/{file}"
            );
        }
    }
    assert!(
        src("server.rs").contains("\"rebuild-projection\""),
        "the rebuild route does not check rebuild-projection"
    );
}
