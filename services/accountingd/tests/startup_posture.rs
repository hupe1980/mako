//! Guard: accountingd refuses to start in a posture that admits every caller.
//!
//! Two doors lead into this daemon and neither is visible in the type system.
//! Without `[oidc]` the verifier starts disabled and every request arrives with
//! synthetic `dev-admin` claims carrying every Marktrolle, after which every
//! Cedar check passes. Without the inbound webhook secret,
//! `webhook::verify_request` is handed `None` and verifies nothing, so
//! `POST /webhook` accepts any body at all.
//!
//! Both are configuration absences, so only a startup check can see them.

use accountingd::config::AccountingdConfig;

fn config(extra: serde_json::Value) -> AccountingdConfig {
    let mut cfg = serde_json::json!({"database": {"url": "postgres://localhost/accountingd"}, "tenant": "9900357000004"});
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
        "audience": "api://mako-accountingd",
    })
}

#[test]
fn startup_refuses_without_oidc() {
    let err = config(serde_json::json!({ "erp_hmac_secret": "s3cret" }))
        .check_auth_posture()
        .expect_err("a deployment with no [oidc] must not start");
    let msg = err.to_string();
    for named in [
        "[oidc]",
        "SEPA",
        "IBAN",
        "Kontokorrent",
        "pain.001",
        "§ 17 DSGVO",
    ] {
        assert!(
            msg.contains(named),
            "the refusal must name what would otherwise be unauthenticated — {named:?} is \
             missing from: {msg}"
        );
    }
}

#[test]
fn startup_refuses_an_unsigned_inbound_webhook() {
    let err = config(serde_json::json!({ "oidc": oidc() }))
        .check_auth_posture()
        .expect_err("an unsigned inbound webhook must not start");
    let msg = err.to_string();
    for named in ["erp_hmac_secret", "ledger entry"] {
        assert!(
            msg.contains(named),
            "the refusal must name the unsigned door — {named:?} is missing from: {msg}"
        );
    }
}

#[test]
fn a_fully_configured_deployment_starts() {
    config(serde_json::json!({ "oidc": oidc(), "erp_hmac_secret": "s3cret" }))
        .check_auth_posture()
        .expect("[oidc] plus a signed inbound webhook is a startable posture");
}

#[test]
fn an_insecure_deployment_starts_only_when_it_is_asked_for_by_name() {
    config(serde_json::json!({ "allow_insecure_no_auth": true }))
        .check_auth_posture()
        .expect("allow_insecure_no_auth is the dev escape hatch");
}
