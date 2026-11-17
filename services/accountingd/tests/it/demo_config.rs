//! Guard: `deploy/demo/accountingd.toml` is a configuration `accountingd` starts with.
//!
//! The demo configuration is what `just dev` runs on the host and `just demo`
//! runs in Docker, and what an operator copies when writing their own. A key
//! the type renamed, a required key nobody added, or a value that no longer
//! parses is an estate that does not start; a key the type does not declare is
//! a setting that appears configured and does nothing.

/// The demo configuration, from this crate's own location.
fn demo_config() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/demo/accountingd.toml")
}

/// Loaded through the runner's loader, so the file is checked exactly as
/// startup checks it — every missing, unknown or invalid key named.
#[test]
fn the_demo_config_loads() {
    if let Err(e) = mako_service::load_config_file::<accountingd::config::AccountingdConfig>(
        "accountingd",
        &demo_config(),
    ) {
        panic!("{e}");
    }
}

/// The file alone, without the environment layer: a key `accountingd` does not
/// declare is refused by name.
#[test]
fn the_demo_config_names_no_key_the_service_ignores() {
    let path = demo_config();
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    if let Err(e) = toml::from_str::<accountingd::config::AccountingdConfig>(&src) {
        let msg = e.to_string();
        assert!(
            !msg.contains("unknown field"),
            "deploy/demo/accountingd.toml names a key `accountingd` does not have — fix the config, or the type:\n{msg}"
        );
    }
}

/// The auth posture is a second gate beside the parse: a demo configuration
/// opts out of it by name (`allow_insecure_no_auth`), or the estate does not
/// start.
#[test]
fn the_demo_config_passes_the_auth_posture() {
    let path = demo_config();
    let cfg = mako_service::load_config_file::<accountingd::config::AccountingdConfig>(
        "accountingd",
        &path,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    if let Err(e) = cfg.check_auth_posture() {
        panic!(
            "deploy/demo/accountingd.toml loads but `accountingd` refuses to start with it:\n{e}"
        );
    }
}
