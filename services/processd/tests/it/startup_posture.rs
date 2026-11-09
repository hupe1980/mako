//! Guard: processd refuses to start in a posture that admits every caller.
//!
//! Two doors lead into this daemon and neither is visible in the type system.
//! Without `[oidc]` the verifier starts disabled and every request arrives with
//! synthetic `dev-admin` claims carrying every Marktrolle, after which every
//! Cedar check passes. Without the inbound webhook secret,
//! `webhook::verify_request` is handed `None` and verifies nothing, so
//! `POST /webhook` accepts any body at all.
//!
//! Both are configuration absences, so only a startup check can see them.

use processd::config::Config;

fn config(extra: serde_json::Value) -> Config {
    let mut cfg = serde_json::json!({"http": {}, "database": {"url": "postgres://localhost/processd"}, "identity": {"own_mp_id": "9900357000004", "verbundene_lieferanten": []}, "makod": {"url": "http://localhost:8080", "api_key": "test"}, "marktd": {"url": "http://localhost:8180", "api_key": "test"}});
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
        "audience": "api://mako-processd",
    })
}

#[test]
fn startup_refuses_without_oidc() {
    let err = config(serde_json::json!({ "webhook": { "inbound_secret": "s3cret" } }))
        .check_auth_posture()
        .expect_err("a deployment with no [oidc] must not start");
    let msg = err.to_string();
    for named in ["[oidc]", "decision queues", "Anmeldung", "Kündigung"] {
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
    for named in ["webhook.inbound_secret", "de.mako.process.initiated"] {
        assert!(
            msg.contains(named),
            "the refusal must name the unsigned door — {named:?} is missing from: {msg}"
        );
    }
}

#[test]
fn a_fully_configured_deployment_starts() {
    config(serde_json::json!({ "oidc": oidc(), "webhook": { "inbound_secret": "s3cret" } }))
        .check_auth_posture()
        .expect("[oidc] plus a signed inbound webhook is a startable posture");
}

#[test]
fn an_insecure_deployment_starts_only_when_it_is_asked_for_by_name() {
    config(serde_json::json!({ "allow_insecure_no_auth": true }))
        .check_auth_posture()
        .expect("allow_insecure_no_auth is the dev escape hatch");
}

/// `[lf] eog_auto_respond = true` sends every resolved Gas Bestätigung EoG
/// 44014 unattended, and the 44014 states the LF's Abrechnungsintervall in
/// `SG6 DTM+Z20` (UTILMD AHB Gas 1.2 Kap. 5.9, Muss). A config that enables
/// it without the interval is refused at startup, naming the key — not sent
/// and refused by makod one 44013 at a time.
#[cfg(feature = "role-lf-gas")]
#[test]
fn startup_refuses_eog_auto_respond_without_an_abrechnungsintervall() {
    use mako_service::Daemon as _;
    let cfg = config(serde_json::json!({ "lf": { "eog_auto_respond": true } }));
    let err = processd::Processd::validate(&cfg)
        .expect_err("eog_auto_respond without an Abrechnungsintervall must not start");
    assert!(
        err.to_string().contains("eog_abrechnungsintervall_monate"),
        "the refusal names the key: {err}"
    );
    let cfg = config(serde_json::json!({
        "lf": { "eog_auto_respond": true, "eog_abrechnungsintervall_monate": 12 }
    }));
    processd::Processd::validate(&cfg).expect("an interval the MIG admits starts");
}

/// UTILMD MIG Gas G1.2 S. 55 admits 1, 3, 6 and 12 months in `DTM+Z20`; any
/// other value would render a 44014 the counterparty cannot read.
#[test]
fn startup_refuses_an_abrechnungsintervall_the_mig_does_not_admit() {
    use mako_service::Daemon as _;
    for n in [0, 2, 24, 99] {
        let cfg = config(serde_json::json!({ "lf": { "eog_abrechnungsintervall_monate": n } }));
        let err = processd::Processd::validate(&cfg).expect_err("refused");
        assert!(err.to_string().contains("1, 3, 6, 12"), "{n}: {err}");
    }
}

/// A `[[lf.bilanzkreise]]` row names its Sparte, so a supplier of Strom and
/// Gas never answers one with the other's Bilanzkreis. A row without one does
/// not parse; two rows for one (Sparte, Bilanzierungsgebiet) and an EEG or
/// KWKG list on a Gas row are refused at startup, naming the rows.
#[test]
fn startup_refuses_an_ambiguous_bilanzkreis_table() {
    use mako_service::Daemon as _;
    let row = |sparte: &str, bg: Option<&str>, regime: &str| {
        let mut r = serde_json::json!({ "sparte": sparte, regime: ["BK1"] });
        if let Some(bg) = bg {
            r["bilanzierungsgebiet"] = bg.into();
        }
        r
    };
    let with = |rows: Vec<serde_json::Value>| {
        config(serde_json::json!({ "lf": { "bilanzkreise": rows } }))
    };

    let ohne_sparte = serde_json::json!({ "lf": { "bilanzkreise": [{ "standard": ["BK1"] }] } });
    assert!(
        std::panic::catch_unwind(|| config(ohne_sparte)).is_err(),
        "a row without `sparte` must not parse"
    );

    let bg = Some("11YN-BG-EON----A");
    processd::Processd::validate(&with(vec![
        row("strom", bg, "standard"),
        row("gas", bg, "standard"),
        row("STROM", None, "standard"),
        row("gas", None, "standard"),
    ]))
    .expect("one row per Sparte and Bilanzierungsgebiet starts");

    let err = processd::Processd::validate(&with(vec![
        row("gas", None, "standard"),
        row("gas", None, "standard"),
    ]))
    .expect_err("two Gas fallback rows");
    assert!(err.to_string().contains("rows 1 and 2"), "{err}");

    for regime in ["eeg", "kwkg"] {
        let err = processd::Processd::validate(&with(vec![row("gas", bg, regime)]))
            .expect_err("an EEG/KWKG list on a Gas row");
        assert!(err.to_string().contains("sparte GAS"), "{regime}: {err}");
    }
}
