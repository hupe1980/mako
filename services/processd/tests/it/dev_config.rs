//! Guard: `dev/config/processd.toml` loads through `processd`'s own config type.
//!
//! The dev config is what `just dev processd` starts the service with, so a key
//! the type renamed, a required key nobody added, or a value that no longer
//! parses is a fresh clone that does not start. Loaded through the runner's
//! loader, so the file is checked exactly as startup checks it.

#[test]
fn the_dev_config_loads() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/config/processd.toml");
    let cfg = match mako_service::load_config_file::<processd::config::Config>("processd", &path) {
        Ok(cfg) => cfg,
        Err(e) => panic!("{e}"),
    };
    // What startup checks beyond the parse — `[lf] eog_auto_respond` without
    // its Abrechnungsintervall among them.
    if let Err(e) = <processd::Processd as mako_service::Daemon>::validate(&cfg) {
        panic!("dev/config/processd.toml parses but is refused at startup: {e:#}");
    }
}
