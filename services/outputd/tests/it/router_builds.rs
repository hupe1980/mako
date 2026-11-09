//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with the shipped demo configuration and a lazy pool, and the
//! runner's infra routes are merged in exactly as at boot — so a route axum
//! refuses, a merge collision with the runner's `/metrics` or `/health/*`, or
//! a `build` that panics fails `cargo test` instead of the container start.

#[tokio::test]
async fn the_router_builds_as_the_runner_mounts_it() {
    let cfg = toml::from_str(include_str!("../../../../demos/o2c/outputd.toml"))
        .expect("the demo configuration parses");
    let _app = mako_service::build_for_test::<outputd::Outputd>(cfg)
        .await
        .expect("the router builds");
}

/// A config with two defects reports both: no `[oidc]` and a relay URL
/// without its credential are each a finding at its own path.
#[test]
fn a_config_with_two_defects_reports_both() {
    use mako_service::Daemon as _;
    let mut cfg: outputd::config::OutputdConfig =
        toml::from_str(include_str!("../../../../demos/o2c/outputd.toml"))
            .expect("the demo configuration parses");
    cfg.allow_insecure_no_auth = false;
    cfg.oidc = None;
    cfg.delivery.email_relay_url = Some("https://relay.example".to_owned());
    cfg.delivery.email_relay_api_key = None;
    let pfade: Vec<String> = outputd::Outputd::validate(&cfg)
        .into_iter()
        .map(|b| b.pfad)
        .collect();
    assert!(pfade.contains(&"oidc".to_owned()), "{pfade:?}");
    assert!(
        pfade.contains(&"delivery.email_relay_api_key".to_owned()),
        "{pfade:?}"
    );
}
