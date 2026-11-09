//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with a lazy pool and the runner's infra routes are merged in
//! exactly as at boot — so a route axum refuses, a merge collision with the
//! runner's `/metrics` or `/health/*`, or a `build` that refuses its own config
//! fails `cargo test` instead of the container start.

/// `dev/config/einsd.toml` — what `just dev einsd` starts — builds a router.
#[tokio::test]
async fn the_dev_config_builds_a_router() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/config/einsd.toml");
    let cfg = mako_service::load_config_file::<einsd::config::EinsdConfig>("einsd", &path)
        .expect("the dev configuration loads");
    let _app = mako_service::build_for_test::<einsd::Einsd>(cfg)
        .await
        .expect("the router builds");
}

#[tokio::test]
async fn the_router_builds_as_the_runner_mounts_it() {
    let cfg = serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://startup-test/einsd"},
        "tenant": "9900357000004",
        "allow_insecure_no_auth": true,
    }))
    .expect("the configuration parses");
    let _app = mako_service::build_for_test::<einsd::Einsd>(cfg)
        .await
        .expect("the router builds");
}

/// With no `[policy]`, the Beihilfevorbehalt reading is reported as the
/// tightening default — in the startup report, on `/metrics` and at
/// `GET /api/v1/policy` — with its source.
#[tokio::test]
async fn an_unset_policy_is_reported_as_its_default_with_its_source() {
    use tower::ServiceExt as _;

    let cfg: einsd::config::EinsdConfig = serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://startup-test/einsd"},
        "tenant": "9900357000004",
        "allow_insecure_no_auth": true,
    }))
    .expect("the configuration parses");
    let lines = cfg.policy_report().lines("einsd");
    assert!(
        lines.iter().any(|l| l.starts_with(
            "einsd.beihilfe_politik=verweigern origin=default class=policy quelle=\"BT-Drs. 21/7867"
        )),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("einsd.allow_insecure_no_auth=true") && l.ends_with("insecure")),
        "{lines:?}"
    );
    let app = mako_service::build_for_test::<einsd::Einsd>(cfg)
        .await
        .expect("the router builds");
    let get = |path: &'static str| {
        app.clone().oneshot(
            axum::http::Request::get(path)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
    };
    let metrics = get("/metrics").await.unwrap();
    let text = String::from_utf8(
        axum::body::to_bytes(metrics.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(
        text.lines().any(|l| l.starts_with("mako_policy_info{")
            && l.contains(r#"key="beihilfe_politik""#)
            && l.contains(r#"value="verweigern""#)
            && l.contains(r#"origin="default""#)
            && l.contains(r#"service="einsd""#)
            && l.ends_with(" 1")),
        "{text}"
    );
    let policy = get("/api/v1/policy").await.unwrap();
    assert_eq!(policy.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(policy.into_body(), 1 << 16)
        .await
        .unwrap();
    let report: mako_service::policy::PolicyReport = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        report.get("beihilfe_politik").map(|e| e.value.as_str()),
        Some("verweigern")
    );
}

/// The old key is refused naming the new one, never run under the default.
#[test]
fn the_removed_politik_key_is_refused_naming_the_policy_key() {
    let cfg: einsd::config::EinsdConfig = serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://startup-test/einsd"},
        "tenant": "9900357000004",
        "allow_insecure_no_auth": true,
        "beihilfevorbehalt": {"politik": "unter_vorbehalt"},
    }))
    .expect("the configuration parses");
    let err = format!(
        "{:#}",
        mako_service::validate_config::<einsd::Einsd>(&cfg).unwrap_err()
    );
    assert!(err.contains("[policy] beihilfe_politik"), "{err}");
}

/// A config with two defects reports both, each at its path, not the first.
#[test]
fn two_defects_are_both_reported() {
    use mako_service::Daemon as _;
    let cfg: einsd::config::EinsdConfig = serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://startup-test/einsd"},
        "tenant": "9900357000004",
        "beihilfevorbehalt": {"politik": "unter_vorbehalt"},
    }))
    .expect("the configuration parses");
    let pfade: Vec<String> = einsd::Einsd::validate(&cfg)
        .into_iter()
        .map(|b| b.pfad)
        .collect();
    assert_eq!(pfade, ["beihilfevorbehalt.politik", "oidc"]);
}

/// A recorded approval names its scheme, and the configuration is refused
/// where the gate could not read it: an extension of the EEG 2023 approval
/// without its end, an extension and an EEG 2027 approval covering the same
/// day, or the free-text `umfang` in place of the scheme.
#[test]
fn an_unreadable_approval_is_refused_at_load() {
    let parse = |genehmigungen: serde_json::Value| {
        serde_json::from_value::<einsd::config::EinsdConfig>(serde_json::json!({
            "database": {"url": "postgres://startup-test/einsd"},
            "tenant": "9900357000004",
            "allow_insecure_no_auth": true,
            "beihilfevorbehalt": {"genehmigungen": genehmigungen},
        }))
    };
    let verlaengerung = serde_json::json!({
        "regelung": "eeg2023_verlaengerung",
        "eingang": "2026-12-18",
        "gilt_ab": "2027-01-01",
        "gilt_bis": "2027-06-30",
        "bundesanzeiger": "BAnz AT (Fixture)",
    });
    let eeg2027 = serde_json::json!({
        "regelung": "eeg2027",
        "eingang": "2027-02-10",
        "gilt_ab": "2027-01-01",
        "bundesanzeiger": "BAnz AT (Fixture)",
    });

    let cfg = parse(serde_json::json!([verlaengerung])).expect("an extension parses");
    assert_eq!(
        cfg.beihilfevorbehalt.genehmigungen[0].regelung,
        eeg_billing::Beihilferegelung::Eeg2023Verlaengerung
    );

    let mut ohne_ende = verlaengerung.clone();
    ohne_ende.as_object_mut().unwrap().remove("gilt_bis");
    let err = parse(serde_json::json!([ohne_ende]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("gilt_bis"), "{err}");

    let err = parse(serde_json::json!([verlaengerung, eeg2027]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("2027-01-01"), "{err}");

    let mut frei = eeg2027.clone();
    let obj = frei.as_object_mut().unwrap();
    obj.remove("regelung");
    obj.insert("umfang".to_owned(), serde_json::json!("Teil 3"));
    assert!(parse(serde_json::json!([frei])).is_err());
}

/// A database failure answers the problem shape with an `error_id`, never the
/// driver's text — the pool here points at a port nothing listens on.
#[tokio::test]
async fn a_database_failure_answers_an_error_id_and_no_driver_text() {
    use tower::ServiceExt as _;

    let cfg: einsd::config::EinsdConfig = serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://startup-test/einsd"},
        "tenant": "9900357000004",
        "allow_insecure_no_auth": true,
    }))
    .expect("the configuration parses");
    let app = mako_service::build_for_test::<einsd::Einsd>(cfg)
        .await
        .expect("the router builds");
    let res = app
        .oneshot(
            axum::http::Request::get("/api/v1/anlagen")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap(),
    )
    .expect("the body is the JSON problem shape");
    assert!(body["error_id"].is_string(), "{body}");
    assert_eq!(body["detail"], "internal server error", "{body}");
}
