//! Role gating in `policies/edmd.cedar`.
//!
//! The tenant check alone is not enough: an LF-role service account of the
//! same tenant (a portal integration, a billing reader) must never be able to
//! write measurement data, dispatch field work, or erase a MaLo. These tests
//! evaluate the shipped policy file — the same bytes `main.rs` loads — so a
//! policy edit that widens a write action fails CI.

use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

const TENANT: &str = "9900001000001";

fn enforcer() -> CedarEnforcer {
    CedarEnforcer::from_policy_str(include_str!("../policies/edmd.cedar"))
        .expect("shipped policy parses")
}

fn principal(roles: &[&str]) -> CedarPrincipal {
    CedarPrincipal {
        sub: "svc-test".to_owned(),
        tenant: TENANT.to_owned(),
        roles: roles.iter().map(|r| (*r).to_owned()).collect(),
    }
}

#[test]
fn same_tenant_reads_are_role_agnostic() {
    let e = enforcer();
    let lf = principal(&["LF"]);
    for action in [
        "read-timeseries",
        "read-imbalance",
        "read-billing-period",
        "read-archive-olap",
        "read-archive-status",
        "read-reading-order",
        "use-mcp",
    ] {
        assert!(e.check(&lf, action, TENANT).is_ok(), "LF may {action}");
    }
}

#[test]
fn cross_tenant_is_always_denied() {
    let e = enforcer();
    let msb = principal(&["MSB"]);
    for action in ["read-timeseries", "write-meter-reads", "write-timeseries"] {
        assert!(
            e.check(&msb, action, "9900002000002").is_err(),
            "{action} across tenants must be denied"
        );
    }
}

#[test]
fn lf_cannot_write_measurement_data() {
    let e = enforcer();
    let lf = principal(&["LF"]);
    for action in [
        "write-meter-reads",
        "write-timeseries",
        "write-corrections",
        "write-quality-rescore",
        "write-reading-order",
        "write-gdpr-erasure",
    ] {
        assert!(
            e.check(&lf, action, TENANT).is_err(),
            "an LF-role token must not be able to {action}"
        );
    }
}

#[test]
fn msb_writes_readings_nb_dispatches_orders() {
    let e = enforcer();
    let msb = principal(&["MSB"]);
    assert!(e.check(&msb, "write-meter-reads", TENANT).is_ok());
    assert!(e.check(&msb, "write-timeseries", TENANT).is_ok());
    assert!(e.check(&msb, "write-reading-order", TENANT).is_ok());

    let nb = principal(&["NB"]);
    assert!(e.check(&nb, "write-reading-order", TENANT).is_ok());
    assert!(e.check(&nb, "write-timeseries", TENANT).is_ok());
    assert!(
        e.check(&nb, "write-meter-reads", TENANT).is_err(),
        "direct push is the MSB's channel — an NB token does not deliver readings"
    );
}

#[test]
fn admin_role_covers_all_writes() {
    let e = enforcer();
    let admin = principal(&["admin"]);
    for action in [
        "write-meter-reads",
        "write-timeseries",
        "write-corrections",
        "write-quality-rescore",
        "write-reading-order",
        "write-gdpr-erasure",
    ] {
        assert!(
            e.check(&admin, action, TENANT).is_ok(),
            "admin may {action}"
        );
    }
}
