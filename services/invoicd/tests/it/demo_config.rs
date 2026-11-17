//! Guard: `deploy/demo/invoicd.toml` is a configuration `invoicd` starts with.
//!
//! The demo configuration is what `just dev` runs on the host and `just demo`
//! runs in Docker, and what an operator copies when writing their own. A key
//! the type renamed, a required key nobody added, or a value that no longer
//! parses is an estate that does not start; a key the type does not declare is
//! a setting that appears configured and does nothing.

/// The demo configuration, from this crate's own location.
fn demo_config() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/demo/invoicd.toml")
}

/// Loaded through the runner's loader, so the file is checked exactly as
/// startup checks it — every missing, unknown or invalid key named.
#[test]
fn the_demo_config_loads() {
    if let Err(e) =
        mako_service::load_config_file::<invoicd::config::Config>("invoicd", &demo_config())
    {
        panic!("{e}");
    }
}
