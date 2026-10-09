//! Guard: `dev/config/edmd.toml` loads through `edmd`'s own config type.
//!
//! The dev config is what `just dev edmd` starts the service with, so a key
//! the type renamed, a required key nobody added, or a value that no longer
//! parses is a fresh clone that does not start. Loaded through the runner's
//! loader, so the file is checked exactly as startup checks it.

#[test]
fn the_dev_config_loads() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/config/edmd.toml");
    if let Err(e) = mako_service::load_config_file::<edmd::config::Config>("edmd", &path) {
        panic!("{e}");
    }
}
