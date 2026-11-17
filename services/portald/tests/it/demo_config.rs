//! Guard: `deploy/demo/portald.toml` is a configuration `portald` starts with.
//!
//! The demo configuration is what `just dev` runs on the host and `just demo`
//! runs in Docker, and what an operator copies when writing their own. A key
//! the type renamed, a required key nobody added, or a value that no longer
//! parses is an estate that does not start; a key the type does not declare is
//! a setting that appears configured and does nothing.

/// The demo configuration, from this crate's own location.
fn demo_config() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/demo/portald.toml")
}

/// Loaded through the runner's loader, so the file is checked exactly as
/// startup checks it — every missing, unknown or invalid key named.
#[test]
fn the_demo_config_loads() {
    if let Err(e) =
        mako_service::load_config_file::<portald::config::PortaldConfig>("portald", &demo_config())
    {
        panic!("{e}");
    }
}

/// The file alone, without the environment layer: a key `portald` does not
/// declare is refused by name.
#[test]
fn the_demo_config_names_no_key_the_service_ignores() {
    let path = demo_config();
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    if let Err(e) = toml::from_str::<portald::config::PortaldConfig>(&src) {
        let msg = e.to_string();
        assert!(
            !msg.contains("unknown field"),
            "deploy/demo/portald.toml names a key `portald` does not have — fix the config, or the type:\n{msg}"
        );
    }
}
