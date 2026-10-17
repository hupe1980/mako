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
