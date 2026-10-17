//! The `[mabis]` checks run at start-up, where they can still refuse.
//!
//! # Why this is a test
//!
//! A configuration check that is never reached is worse than none, because the
//! doc comment claims a guarantee. Each check below refuses a deployment that
//! could otherwise start cleanly and fail at 05:00 on the
//! Erstaufschlag-Werktag — after a month of metering data has been aggregated
//! and the run's version number consumed. `the_mabis_hub_target_refuses_at_startup`
//! in `tests/it/router_builds.rs` pins that `build` calls them.

use netzbilanzd::config::NetzbilanzConfig;

/// A valid Bilanzierungsgebiet: a 16-character EIC of ENTSO-E object type `Y`.
const VALID_GEBIET: &str = "11YMAKO-TEST-01U";

/// A configuration with a `[mabis]` section, its keys overridden by `mabis`.
fn config_with(mabis: serde_json::Value) -> NetzbilanzConfig {
    let mut section = serde_json::json!({
        "sender_mp_id": "9900357000004",
        "receiver_mp_id": "9900077000006",
        "bilanzierungsgebiet_id": VALID_GEBIET,
    });
    let (Some(base), Some(extra)) = (section.as_object_mut(), mabis.as_object()) else {
        panic!("both must be JSON objects");
    };
    for (k, v) in extra {
        base.insert(k.clone(), v.clone());
    }
    serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://localhost/netzbilanzd" },
        "tenant": "9900357000004",
        "marktd_url": "http://marktd:8180", "marktd_api_key": "k",
        "makod_url": "http://makod:8080", "makod_api_key": "k",
        "edmd_url": "http://edmd:8380", "edmd_api_key": "k",
        "mabis": section,
    }))
    .expect("config parses")
}

/// The default deployment validates.
#[test]
fn the_documented_configuration_is_accepted() {
    config_with(serde_json::json!({}))
        .check_mabis()
        .expect("the default is valid");
}

/// Without `[mabis]` there is nothing to check: a Netzbetreiber that files no
/// Summenzeitreihe starts without one.
#[test]
fn a_deployment_without_mabis_is_accepted() {
    let mut cfg = config_with(serde_json::json!({}));
    cfg.mabis = None;
    cfg.edmd_url = None;
    cfg.check_mabis().expect("no [mabis], no MaBiS check");
}

/// The Hub target refuses at start-up, and says why.
///
/// BK6-24-210 has no Beschluss: no wire format, endpoint or payload shape has
/// been published, so an implementation would be invention. An invented format
/// that reaches a real Hub is indistinguishable, at the point of failure, from
/// a correct one that was rejected.
#[test]
fn an_unimplemented_submission_target_refuses_at_startup() {
    let err = config_with(serde_json::json!({ "submission_target": "mabis-hub" }))
        .check_mabis()
        .expect_err("mabis-hub has no implementation");
    let msg = err.to_string();
    assert!(
        msg.contains("BK6-24-210"),
        "the refusal must cite why: {msg}"
    );
    assert!(
        msg.contains("biko-bilateral"),
        "and name the way forward: {msg}"
    );
}

/// The refusal states no date: BK6-24-210 has no Beschluss, so no published
/// document fixes when a Hub would go live, and a year in the message reads as
/// a commitment nobody made.
#[test]
fn the_hub_refusal_names_no_go_live_date() {
    let msg = config_with(serde_json::json!({ "submission_target": "mabis-hub" }))
        .check_mabis()
        .expect_err("mabis-hub has no implementation")
        .to_string();
    assert!(
        !msg.chars()
            .collect::<Vec<_>>()
            .windows(4)
            .any(|w| { w.iter().all(char::is_ascii_digit) && w[0] == '2' && w[1] == '0' }),
        "the refusal must not state a year: {msg}"
    );
}

/// A Bilanzkreis in the Bilanzierungsgebiet field refuses at start-up.
///
/// Both are 16-character EICs, so only the object type separates them — `Y`
/// (Area) from `X` (Party) — and `LOC+107` carries the value as free text. The
/// BIKO would accept either, which is why this has to be caught here.
#[test]
fn a_bilanzkreis_is_not_a_bilanzierungsgebiet() {
    let party = "11XSUEDWESTSTRO8";
    let err = config_with(serde_json::json!({ "bilanzierungsgebiet_id": party }))
        .check_mabis()
        .expect_err("an X-type EIC is a Bilanzkreis, not a territory");
    let msg = err.to_string();
    assert!(msg.contains("bilanzierungsgebiet_id"), "{msg}");
    assert!(
        msg.contains('Y') && msg.contains('X'),
        "the message must name both object types: {msg}"
    );
}

/// A value that is not an EIC at all is refused too.
#[test]
fn a_free_text_territory_is_refused() {
    for gebiet in ["Regelzone Nord", ""] {
        assert!(
            config_with(serde_json::json!({ "bilanzierungsgebiet_id": gebiet }))
                .check_mabis()
                .is_err(),
            "{gebiet:?} is not a Bilanzierungsgebiet"
        );
    }
}

/// Every Lastgang a Summenzeitreihe sums is read from `edmd`, so `[mabis]`
/// without `edmd_url` cannot build a filing and refuses to start.
#[test]
fn mabis_without_edmd_refuses_at_startup() {
    let mut cfg = config_with(serde_json::json!({}));
    cfg.edmd_url = None;
    let msg = cfg
        .check_mabis()
        .expect_err("no edmd, no aggregate")
        .to_string();
    assert!(msg.contains("edmd_url"), "{msg}");
}

/// A misspelt key in `[mabis]` is refused, not ignored: an ignored
/// `erstaufschlag_werktag` files on a Werktag nobody chose.
#[test]
fn an_unknown_mabis_key_is_refused() {
    let err = serde_json::from_value::<NetzbilanzConfig>(serde_json::json!({
        "database": { "url": "postgres://localhost/netzbilanzd" },
        "marktd_url": "http://marktd:8180", "marktd_api_key": "k",
        "makod_url": "http://makod:8080", "makod_api_key": "k",
        "mabis": {
            "sender_mp_id": "9900357000004",
            "receiver_mp_id": "9900077000006",
            "bilanzierungsgebiet_id": VALID_GEBIET,
            "schedule": { "erstaufschlag_werktag": 10, "run_hour": 5 },
        },
    }))
    .expect_err("run_hour is not a key; run_hour_utc is");
    assert!(err.to_string().contains("run_hour"), "{err}");
}
